//! What the status area's network element draws from: the traffic of the
//! interface that holds the default route and its basic configuration.
//!
//! A thread of the menu (`spawn`) reads, once a second and without root,
//! `/proc/net/route` (default route, gateway), the interface's
//! `statistics/{rx,tx}_bytes`, `speed` and `/proc/net/wireless` (link),
//! the addresses through `getifaddrs`, the DNS servers from systemd-resolved's
//! `resolv.conf` (else `/etc/resolv.conf`) and whether `wg0` is up. It keeps
//! the last `WINDOW` rates per direction; the menu copies a `Snapshot` before
//! each frame. Every path starts at a `Roots`, so tests read a directory.

use std::collections::VecDeque;
use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Samples kept per direction: five minutes at one a second.
pub const WINDOW: usize = 300;
/// How often the thread samples.
pub const EVERY: Duration = Duration::from_secs(1);
/// A sample this soon after the last one is not taken: the rate of a few
/// milliseconds says nothing.
const MIN_STEP: Duration = Duration::from_millis(200);
/// The most any of the read files may hold.
const MAX_BYTES: u64 = 64 * 1024;
/// DNS servers kept, and the longest one.
const MAX_DNS: usize = 3;
const MAX_DNS_LEN: usize = 64;
/// The WireGuard interface `~/wg0.conf` brings up.
pub const VPN_IF: &str = "wg0";
const IFF_UP: u32 = 0x1;

/// Where /sys, /proc, /run and /etc are.
#[derive(Debug, Clone)]
pub struct Roots {
    pub sys: PathBuf,
    pub proc_: PathBuf,
    pub run: PathBuf,
    pub etc: PathBuf,
}

impl Roots {
    pub fn system() -> Roots {
        Roots {
            sys: "/sys".into(),
            proc_: "/proc".into(),
            run: "/run".into(),
            etc: "/etc".into(),
        }
    }

    /// All four under one directory: `<dir>/sys`, `<dir>/proc`, ...
    #[cfg(test)]
    pub fn under(dir: &Path) -> Roots {
        Roots {
            sys: dir.join("sys"),
            proc_: dir.join("proc"),
            run: dir.join("run"),
            etc: dir.join("etc"),
        }
    }

    fn net(&self, iface: &str) -> PathBuf {
        self.sys.join("class/net").join(iface)
    }
}

fn read(path: &Path) -> Option<String> {
    crate::bounded::read_text(path, MAX_BYTES)
}

/// An interface name the kernel could have made: 1 to 15 bytes, no `/`,
/// not `.` or `..`. Anything else is never joined to a path.
pub fn usable_name(name: &str) -> bool {
    (1..=15).contains(&name.len())
        && name != "."
        && name != ".."
        && name.bytes().all(|b| b.is_ascii_graphic() && b != b'/')
}

/// The default route: its interface and gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub iface: String,
    pub gateway: Option<Ipv4Addr>,
}

/// The default route in `/proc/net/route`: destination and mask 0, up; the
/// lowest metric wins, the first line on a tie. Addresses there are the
/// kernel's 32-bit value printed in the machine's byte order.
pub fn default_route(text: &str) -> Option<Route> {
    let mut best: Option<(u32, Route)> = None;
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 8 || !usable_name(f[0]) {
            continue;
        }
        let hex = |s: &str| u32::from_str_radix(s, 16).ok();
        let (Some(dest), Some(gw), Some(flags), Some(mask)) = (hex(f[1]), hex(f[2]), hex(f[3]), hex(f[7])) else {
            continue;
        };
        let Ok(metric) = f[6].parse::<u32>() else { continue };
        if dest != 0 || mask != 0 || flags & 0x1 == 0 {
            continue;
        }
        let gateway = (flags & 0x2 != 0 && gw != 0).then(|| Ipv4Addr::from(gw.to_ne_bytes()));
        if best.as_ref().is_none_or(|(m, _)| metric < *m) {
            best = Some((
                metric,
                Route {
                    iface: f[0].to_string(),
                    gateway,
                },
            ));
        }
    }
    best.map(|(_, r)| r)
}

/// The signal of `iface` in `/proc/net/wireless`, in dBm; `None` when the
/// interface is not listed or reports no level.
pub fn wireless_level(text: &str, iface: &str) -> Option<i32> {
    wireless_line(text, iface).and_then(|f| {
        let level: f32 = f.get(2)?.trim_end_matches('.').parse().ok()?;
        let level = level as i32;
        // Old drivers print the level as an unsigned byte.
        match level {
            0 => None,
            l if l > 0 => Some(l - 256),
            l => Some(l),
        }
    })
}

fn wireless_line<'a>(text: &'a str, iface: &str) -> Option<Vec<&'a str>> {
    text.lines().skip(2).find_map(|line| {
        let (name, rest) = line.split_once(':')?;
        (name.trim() == iface).then(|| rest.split_whitespace().collect())
    })
}

/// The `nameserver` lines of a resolv.conf, at most `MAX_DNS`.
pub fn nameservers(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            (w.next() == Some("nameserver")).then(|| w.next()).flatten()
        })
        .map(|s| s.chars().take(MAX_DNS_LEN).collect())
        .take(MAX_DNS)
        .collect()
}

/// The WireGuard interface: missing, there but down, or up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Vpn {
    #[default]
    Absent,
    Down,
    Up,
}

/// `wg0` is up when it exists and its flags carry IFF_UP. Its operstate is
/// `unknown` while it works, so that file says nothing.
pub fn vpn(roots: &Roots) -> Vpn {
    let dir = roots.net(VPN_IF);
    if !dir.exists() {
        return Vpn::Absent;
    }
    let flags = read(&dir.join("flags"))
        .and_then(|t| u32::from_str_radix(t.trim().trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    if flags & IFF_UP != 0 { Vpn::Up } else { Vpn::Down }
}

/// One address of one interface, from `getifaddrs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfAddr {
    pub name: String,
    pub addr: IpAddr,
    pub prefix: u8,
}

impl IfAddr {
    /// An address that says the machine is on a network: not loopback, and
    /// for IPv6 not link-local (every link that is up has one).
    fn counts(&self) -> bool {
        self.name != "lo"
            && match self.addr {
                IpAddr::V4(a) => !a.is_loopback(),
                IpAddr::V6(a) => !a.is_loopback() && !a.is_unicast_link_local(),
            }
    }
}

/// The machine's addresses now.
pub fn addresses() -> Vec<IfAddr> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list freed below.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut p = head;
    while !p.is_null() {
        // SAFETY: `p` is a node of the list getifaddrs made; its pointers
        // are null or point at data of the family they name.
        let ifa = unsafe { &*p };
        p = ifa.ifa_next;
        if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() {
            continue;
        }
        let name = unsafe { CStr::from_ptr(ifa.ifa_name) }.to_string_lossy().into_owned();
        let family = unsafe { (*ifa.ifa_addr).sa_family } as i32;
        let mask = ifa.ifa_netmask;
        let entry = match family {
            libc::AF_INET => {
                let a = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
                let prefix = if mask.is_null() {
                    32
                } else {
                    let m = unsafe { &*(mask as *const libc::sockaddr_in) };
                    u32::from_be(m.sin_addr.s_addr).count_ones() as u8
                };
                IfAddr {
                    name,
                    addr: IpAddr::V4(Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr))),
                    prefix,
                }
            }
            libc::AF_INET6 => {
                let a = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
                let prefix = if mask.is_null() {
                    128
                } else {
                    let m = unsafe { &*(mask as *const libc::sockaddr_in6) };
                    m.sin6_addr.s6_addr.iter().map(|b| b.count_ones()).sum::<u32>() as u8
                };
                IfAddr {
                    name,
                    addr: IpAddr::V6(Ipv6Addr::from(a.sin6_addr.s6_addr)),
                    prefix,
                }
            }
            _ => continue,
        };
        out.push(entry);
    }
    // SAFETY: `head` came from getifaddrs and nothing points into it now.
    unsafe { libc::freeifaddrs(head) };
    out
}

/// How the measured interface is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Link {
    #[default]
    Unknown,
    /// A cable, at this many Mbit/s when the driver says.
    Wired(Option<u32>),
    /// WiFi, with the signal in dBm when known.
    Wifi(Option<i32>),
}

/// The configuration the element shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Info {
    /// The measured interface: the default route's, else the first one with
    /// an address; `None`: nothing to measure.
    pub iface: Option<String>,
    /// Whether `iface` holds the default route.
    pub default_route: bool,
    pub link: Link,
    /// The first IPv4 address of `iface` and its prefix.
    pub ipv4: Option<(Ipv4Addr, u8)>,
    pub gateway: Option<Ipv4Addr>,
    pub dns: Vec<String>,
    pub vpn: Vpn,
    /// Some interface but `lo` has an address that is not link-local.
    pub addressed: bool,
}

impl Info {
    /// Whether the element shows: a default route, or some interface with an
    /// address that is not loopback or link-local.
    pub fn visible(&self) -> bool {
        self.default_route || self.addressed
    }
}

/// One reading: the configuration and the byte counters of its interface.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reading {
    pub info: Info,
    /// rx and tx bytes of `info.iface`.
    pub counters: Option<(u64, u64)>,
}

fn counter(path: &Path) -> Option<u64> {
    read(path)?.trim().parse().ok()
}

/// Read everything once, with `addrs` the machine's addresses.
pub fn reading(roots: &Roots, addrs: &[IfAddr]) -> Reading {
    let route = read(&roots.proc_.join("net/route")).and_then(|t| default_route(&t));
    let addressed = addrs.iter().any(IfAddr::counts);
    let iface = match &route {
        Some(r) => Some(r.iface.clone()),
        None => {
            let mut names: Vec<&str> = addrs
                .iter()
                .filter(|a| a.counts() && usable_name(&a.name))
                .map(|a| a.name.as_str())
                .collect();
            names.sort();
            names.first().map(|s| s.to_string())
        }
    };
    let dns = read(&roots.run.join("systemd/resolve/resolv.conf"))
        .map(|t| nameservers(&t))
        .filter(|d| !d.is_empty())
        .or_else(|| read(&roots.etc.join("resolv.conf")).map(|t| nameservers(&t)))
        .unwrap_or_default();
    let mut info = Info {
        iface: iface.clone(),
        default_route: route.is_some(),
        gateway: route.as_ref().and_then(|r| r.gateway),
        dns,
        vpn: vpn(roots),
        addressed,
        ..Info::default()
    };
    let Some(iface) = iface else {
        return Reading { info, counters: None };
    };
    let dir = roots.net(&iface);
    let wireless = read(&roots.proc_.join("net/wireless"));
    let in_wireless = wireless.as_deref().and_then(|t| wireless_line(t, &iface)).is_some();
    info.link = if in_wireless || dir.join("wireless").exists() {
        Link::Wifi(wireless.as_deref().and_then(|t| wireless_level(t, &iface)))
    } else if dir.exists() {
        Link::Wired(
            counter(&dir.join("speed"))
                .and_then(|s| u32::try_from(s).ok())
                .filter(|s| *s > 0),
        )
    } else {
        Link::Unknown
    };
    info.ipv4 = addrs.iter().find_map(|a| match a.addr {
        IpAddr::V4(v4) if a.name == iface && !v4.is_loopback() => Some((v4, a.prefix)),
        _ => None,
    });
    let counters = counter(&dir.join("statistics/rx_bytes")).zip(counter(&dir.join("statistics/tx_bytes")));
    Reading { info, counters }
}

/// The rates of the last `WINDOW` seconds and the totals, fed one reading
/// at a time.
#[derive(Debug, Default)]
pub struct Series {
    /// Bits per second, oldest first.
    rx: VecDeque<f64>,
    tx: VecDeque<f64>,
    /// The last counters, when, and of which interface.
    last: Option<(Instant, String, u64, u64)>,
    /// Bytes counted since the series began, over every interface it
    /// followed: never smaller than before.
    total: (u64, u64),
    info: Info,
}

impl Series {
    /// Take a reading made at `now`. A rate needs two readings of one
    /// interface; a new interface (the default route moved) or a counter
    /// that went back (the interface was reset or replaced) starts the
    /// graph again instead of drawing a spike.
    pub fn push(&mut self, now: Instant, r: Reading) {
        let iface = r.info.iface.clone();
        self.info = r.info;
        let (Some(iface), Some((rx, tx))) = (iface, r.counters) else {
            self.rx.clear();
            self.tx.clear();
            self.last = None;
            return;
        };
        match &self.last {
            Some((t, name, lrx, ltx)) if *name == iface && rx >= *lrx && tx >= *ltx => {
                let dt = now.saturating_duration_since(*t);
                if dt < MIN_STEP {
                    return;
                }
                let (drx, dtx) = (rx - lrx, tx - ltx);
                let secs = dt.as_secs_f64();
                self.add(drx as f64 * 8.0 / secs, dtx as f64 * 8.0 / secs);
                self.total.0 = self.total.0.saturating_add(drx);
                self.total.1 = self.total.1.saturating_add(dtx);
            }
            _ => {
                self.rx.clear();
                self.tx.clear();
            }
        }
        self.last = Some((now, iface, rx, tx));
    }

    fn add(&mut self, rx: f64, tx: f64) {
        self.rx.push_back(rx);
        self.tx.push_back(tx);
        while self.rx.len() > WINDOW {
            self.rx.pop_front();
            self.tx.pop_front();
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            rx: self.rx.iter().copied().collect(),
            tx: self.tx.iter().copied().collect(),
            total: self.total,
            info: self.info.clone(),
        }
    }
}

/// What the element draws: a copy of the series and the configuration.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    /// Bits per second, oldest first, at most `WINDOW`, same length.
    pub rx: Vec<f64>,
    pub tx: Vec<f64>,
    /// Bytes since the menu started.
    pub total: (u64, u64),
    pub info: Info,
}

fn peak(v: &[f64]) -> f64 {
    v.iter().copied().fold(0.0, f64::max)
}

impl Snapshot {
    /// The newest rates, 0 before the first.
    pub fn current(&self) -> (f64, f64) {
        (
            self.rx.last().copied().unwrap_or(0.0),
            self.tx.last().copied().unwrap_or(0.0),
        )
    }

    /// The highest rates in the window.
    pub fn peak(&self) -> (f64, f64) {
        (peak(&self.rx), peak(&self.tx))
    }

    /// Whether the element shows: a default route, or some interface with an
    /// address that is not loopback or link-local.
    pub fn visible(&self) -> bool {
        self.info.visible()
    }
}

/// Where the session total counts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Since {
    /// The Save-On-Exit program counted from; `None`: from the menu start.
    pub job: Option<crate::jobs::JobId>,
    /// `since 14:02` or `since menu start`.
    pub label: String,
    /// `Snapshot::total` when counting began.
    pub base: (u64, u64),
}

impl Default for Since {
    fn default() -> Since {
        Since {
            job: None,
            label: "since menu start".into(),
            base: (0, 0),
        }
    }
}

/// Whether the element's visibility changed, with `was` the last value.
pub fn flipped(was: &mut bool, now: bool) -> bool {
    std::mem::replace(was, now) != now
}

/// A sampler thread for the menu. It samples whether or not the menu is on
/// screen, so the graph is whole when it shows again. It calls `wake` only when
/// `Snapshot::visible` changes: a hidden element asks for no frames, so it
/// would appear only with the next minute's. While it shows, the element's
/// `Refresh=1` asks for the frames.
pub fn spawn(roots: Roots, wake: impl Fn() + Send + 'static) -> Arc<Mutex<Series>> {
    let series = Arc::new(Mutex::new(Series::default()));
    let shared = series.clone();
    std::thread::spawn(move || {
        let mut visible = false;
        loop {
            // Read outside the lock, so a frame never waits for the files.
            let r = reading(&roots, &addresses());
            let now_visible = r.info.visible();
            match shared.lock() {
                Ok(mut s) => s.push(Instant::now(), r),
                Err(_) => return,
            }
            // After the push, so the frame it asks for sees the change.
            if flipped(&mut visible, now_visible) {
                wake();
            }
            std::thread::sleep(EVERY);
        }
    });
    series
}

/// `n` bits per second with k, M or G, three digits: `850 bit/s`,
/// `12.3 kbit/s`, `1.20 Mbit/s`.
pub fn bits(rate: f64) -> String {
    scaled(rate, "bit/s")
}

/// `n` bytes the same way: `512 B`, `45.6 MB`, `1.23 GB`.
pub fn bytes(n: u64) -> String {
    scaled(n as f64, "B")
}

fn scaled(v: f64, unit: &str) -> String {
    let v = if v.is_finite() { v.max(0.0) } else { 0.0 };
    if v < 999.5 {
        return format!("{} {unit}", v.round() as u64);
    }
    let mut x = v;
    let mut prefix = "";
    for p in ["k", "M", "G"] {
        if x < 999.5 {
            break;
        }
        x /= 1000.0;
        prefix = p;
    }
    // Three digits; rounding up may reach the next width (9.996 -> 10.0).
    let text = if x < 9.995 {
        format!("{x:.2}")
    } else if x < 99.95 {
        format!("{x:.1}")
    } else {
        format!("{x:.0}")
    };
    format!("{text} {prefix}{unit}")
}

/// A link speed in Mbit/s as `100 Mbit/s`, `1 Gbit/s`, `2.5 Gbit/s`.
pub fn speed(mbit: u32) -> String {
    if mbit >= 1000 {
        let g = mbit as f64 / 1000.0;
        let t = format!("{g:.1}");
        format!("{} Gbit/s", t.trim_end_matches(".0"))
    } else {
        format!("{mbit} Mbit/s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::fs;

    const ROUTE: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
        wlan0\t00000000\t0102A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
        eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
        eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n";

    fn gw(a: u8, b: u8, c: u8, d: u8) -> Option<Ipv4Addr> {
        Some(Ipv4Addr::new(a, b, c, d))
    }

    #[test]
    fn the_default_route_is_the_lowest_metric_up_with_its_gateway() {
        // The test machine is little-endian, like the image's x86_64.
        assert_eq!(
            default_route(ROUTE),
            Some(Route {
                iface: "eth0".into(),
                gateway: gw(192, 168, 1, 1)
            })
        );
        let only_net = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\n\
                        eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\n";
        assert_eq!(default_route(only_net), None, "no default route");
        let down = "h\neth0\t00000000\t0101A8C0\t0002\t0\t0\t100\t00000000\n";
        assert_eq!(default_route(down), None, "not up");
        let no_gw = "h\nwg0\t00000000\t00000000\t0001\t0\t0\t0\t00000000\n";
        assert_eq!(
            default_route(no_gw),
            Some(Route {
                iface: "wg0".into(),
                gateway: None
            }),
            "a point-to-point default route has no gateway"
        );
        assert_eq!(default_route(""), None);
        assert_eq!(default_route("h\ngarbage\nx y z\n"), None);
        let hostile = "h\n../x\t00000000\t0101A8C0\t0003\t0\t0\t1\t00000000\n";
        assert_eq!(default_route(hostile), None, "no path out of a name");
    }

    #[test]
    fn interface_names_are_what_the_kernel_makes() {
        for ok in ["eth0", "wlp0s20f3", "enx001122334455"] {
            assert!(usable_name(ok), "{ok}");
        }
        for bad in ["", ".", "..", "a/b", "sixteen-chars-xx", "a b"] {
            assert!(!usable_name(bad), "{bad}");
        }
    }

    const WIRELESS: &str = "Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE\n \
        face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22\n\
        wlan0: 0000   54.  -56.  -256        0      0      0      0      0        0\n\
        wlan1: 0000   30.  200.  -256        0      0      0      0      0        0\n\
        wlan2: 0000    0.    0.  -256        0      0      0      0      0        0\n";

    #[test]
    fn the_wifi_signal_comes_from_proc_net_wireless() {
        assert_eq!(wireless_level(WIRELESS, "wlan0"), Some(-56));
        assert_eq!(wireless_level(WIRELESS, "wlan1"), Some(-56), "an unsigned byte");
        assert_eq!(wireless_level(WIRELESS, "wlan2"), None, "no level");
        assert_eq!(wireless_level(WIRELESS, "eth0"), None);
        assert_eq!(wireless_level("", "wlan0"), None);
        assert_eq!(wireless_level("a\nb\nwlan0: x y\n", "wlan0"), None);
    }

    #[test]
    fn dns_servers_are_the_nameserver_lines() {
        let text = "# This is /run/systemd/resolve/resolv.conf\n\
                    nameserver 192.168.1.1\n\
                    nameserver  2001:db8::53\n\
                    search example.org\n\
                    nameserver\n\
                    nameserver 9.9.9.9\n\
                    nameserver 1.1.1.1\n";
        assert_eq!(nameservers(text), ["192.168.1.1", "2001:db8::53", "9.9.9.9"]);
        assert!(nameservers("").is_empty());
        let long = format!("nameserver {}\n", "x".repeat(500));
        assert_eq!(nameservers(&long)[0].len(), MAX_DNS_LEN);
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn wg0_is_up_when_it_exists_with_iff_up() {
        let d = TestDir::new("network", "vpn");
        let roots = Roots::under(&d);
        assert_eq!(vpn(&roots), Vpn::Absent);
        let flags = d.join("sys/class/net/wg0/flags");
        write(&flags, "0x90\n");
        assert_eq!(vpn(&roots), Vpn::Down);
        write(&flags, "0x91\n");
        assert_eq!(vpn(&roots), Vpn::Up);
        // operstate says unknown while it works; it is not read.
        write(&d.join("sys/class/net/wg0/operstate"), "unknown\n");
        assert_eq!(vpn(&roots), Vpn::Up);
        write(&flags, "junk\n");
        assert_eq!(vpn(&roots), Vpn::Down);
    }

    fn addr(name: &str, ip: &str, prefix: u8) -> IfAddr {
        IfAddr {
            name: name.into(),
            addr: ip.parse().unwrap(),
            prefix,
        }
    }

    /// A machine on eth0 at 1 Gbit/s with systemd-resolved.
    fn machine(d: &Path) -> Roots {
        write(&d.join("proc/net/route"), ROUTE);
        let eth = d.join("sys/class/net/eth0");
        write(&eth.join("speed"), "1000\n");
        write(&eth.join("statistics/rx_bytes"), "1000\n");
        write(&eth.join("statistics/tx_bytes"), "500\n");
        write(&d.join("run/systemd/resolve/resolv.conf"), "nameserver 192.168.1.1\n");
        write(&d.join("etc/resolv.conf"), "nameserver 127.0.0.53\n");
        Roots::under(d)
    }

    #[test]
    fn a_reading_follows_the_default_route() {
        let d = TestDir::new("network", "reading");
        let roots = machine(&d);
        let addrs = [
            addr("lo", "127.0.0.1", 8),
            addr("eth0", "fe80::1", 64),
            addr("eth0", "192.168.1.23", 24),
            addr("wlan0", "192.168.2.5", 24),
        ];
        let r = reading(&roots, &addrs);
        assert_eq!(r.counters, Some((1000, 500)));
        assert_eq!(
            r.info,
            Info {
                iface: Some("eth0".into()),
                default_route: true,
                link: Link::Wired(Some(1000)),
                ipv4: Some((Ipv4Addr::new(192, 168, 1, 23), 24)),
                gateway: gw(192, 168, 1, 1),
                dns: vec!["192.168.1.1".into()],
                vpn: Vpn::Absent,
                addressed: true,
            }
        );
        // An unknown speed (-1) is no speed; resolved's file missing, the
        // stub's resolv.conf is used.
        write(&d.join("sys/class/net/eth0/speed"), "-1\n");
        fs::remove_file(d.join("run/systemd/resolve/resolv.conf")).unwrap();
        let r = reading(&roots, &addrs);
        assert_eq!(r.info.link, Link::Wired(None));
        assert_eq!(r.info.dns, ["127.0.0.53"]);
    }

    #[test]
    fn wifi_is_named_with_its_signal() {
        let d = TestDir::new("network", "wifi");
        let roots = machine(&d);
        write(
            &d.join("proc/net/route"),
            "h\nwlan0\t00000000\t0102A8C0\t0003\t0\t0\t600\t00000000\n",
        );
        write(&d.join("proc/net/wireless"), WIRELESS);
        let r = reading(&roots, &[addr("wlan0", "192.168.2.5", 24)]);
        assert_eq!(r.info.link, Link::Wifi(Some(-56)));
        assert_eq!(r.counters, None, "no statistics for wlan0 in the fixture");
        // A wireless/ directory says WiFi too, even without a level.
        write(&d.join("proc/net/wireless"), "");
        fs::create_dir_all(d.join("sys/class/net/wlan0/wireless")).unwrap();
        assert_eq!(reading(&roots, &[]).info.link, Link::Wifi(None));
    }

    #[test]
    fn without_a_default_route_the_first_addressed_interface_is_measured() {
        let d = TestDir::new("network", "noroute");
        let roots = machine(&d);
        write(&d.join("proc/net/route"), "h\n");
        let r = reading(&roots, &[addr("eth0", "10.0.0.2", 8), addr("lo", "127.0.0.1", 8)]);
        assert_eq!(r.info.iface.as_deref(), Some("eth0"));
        assert!(!r.info.default_route && r.info.addressed);
        assert_eq!(r.info.gateway, None);
        let s = Snapshot {
            info: r.info,
            ..Snapshot::default()
        };
        assert!(s.visible());
        // Only loopback and link-local: nothing to show.
        let r = reading(&roots, &[addr("lo", "127.0.0.1", 8), addr("eth0", "fe80::2", 64)]);
        assert_eq!(r.info.iface, None);
        assert_eq!(r.counters, None);
        let s = Snapshot {
            info: r.info,
            ..Snapshot::default()
        };
        assert!(!s.visible());
        assert!(!Snapshot::default().visible());
    }

    fn rd(iface: &str, rx: u64, tx: u64) -> Reading {
        Reading {
            info: Info {
                iface: Some(iface.into()),
                default_route: true,
                ..Info::default()
            },
            counters: Some((rx, tx)),
        }
    }

    #[test]
    fn only_a_change_of_visibility_wakes_the_menu() {
        let mut was = false;
        assert!(!flipped(&mut was, false), "still nothing to show");
        assert!(flipped(&mut was, true), "it appears");
        assert!(!flipped(&mut was, true), "still there");
        assert!(flipped(&mut was, false), "it goes");
        assert!(!flipped(&mut was, false));
    }

    #[test]
    fn a_sampler_that_finds_a_route_wakes_the_menu_once() {
        let d = TestDir::new("network", "wake");
        let roots = machine(&d);
        let (tx, rx) = std::sync::mpsc::channel();
        let _series = spawn(roots, move || {
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(5)).expect("a wake when it appears");
        let again = rx.recv_timeout(EVERY + Duration::from_millis(300));
        assert!(again.is_err(), "the next sample changes nothing");
    }

    #[test]
    fn rates_are_byte_deltas_over_the_real_elapsed_time() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut s = Series::default();
        s.push(at(0), rd("eth0", 1000, 100));
        assert!(s.snapshot().rx.is_empty(), "one reading has no rate");
        s.push(at(1000), rd("eth0", 2000, 200));
        // 1000 bytes in 1 s: 8000 bit/s.
        assert_eq!(s.snapshot().rx, [8000.0]);
        assert_eq!(s.snapshot().tx, [800.0]);
        // A late sample (the thread slept 2 s) is divided by 2 s, not 1.
        s.push(at(3000), rd("eth0", 4000, 200));
        assert_eq!(s.snapshot().rx, [8000.0, 8000.0]);
        assert_eq!(s.snapshot().tx, [800.0, 0.0]);
        // Too soon after the last: not taken, and not lost from the total.
        s.push(at(3050), rd("eth0", 4100, 200));
        assert_eq!(s.snapshot().rx.len(), 2);
        s.push(at(4000), rd("eth0", 5000, 300));
        assert_eq!(s.snapshot().rx[2], 8000.0);
        assert_eq!(s.snapshot().total, (4000, 200));
        assert_eq!(s.snapshot().current(), (8000.0, 800.0));
        assert_eq!(s.snapshot().peak(), (8000.0, 800.0));
    }

    #[test]
    fn a_counter_that_goes_back_or_a_new_interface_restarts_the_graph() {
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        let mut s = Series::default();
        s.push(at(0), rd("eth0", 1_000_000, 1000));
        s.push(at(1), rd("eth0", 2_000_000, 2000));
        assert_eq!(s.snapshot().rx.len(), 1);
        // The interface was reset: no spike of 2^64, the graph starts over.
        s.push(at(2), rd("eth0", 10, 10));
        assert!(s.snapshot().rx.is_empty());
        s.push(at(3), rd("eth0", 1010, 10));
        assert_eq!(s.snapshot().rx, [8000.0]);
        // The total keeps what was counted.
        assert_eq!(s.snapshot().total, (1_001_000, 1000));
        // The default route moved to WiFi: its counters are not compared
        // with the cable's.
        s.push(at(4), rd("wlan0", 5, 5));
        assert!(s.snapshot().rx.is_empty());
        s.push(at(5), rd("wlan0", 1005, 5));
        assert_eq!(s.snapshot().rx, [8000.0]);
        assert_eq!(s.snapshot().total, (1_002_000, 1000));
        // The interface is gone: nothing to draw.
        s.push(at(6), Reading::default());
        assert!(s.snapshot().rx.is_empty());
        assert_eq!(s.snapshot().total, (1_002_000, 1000));
    }

    #[test]
    fn the_ring_holds_five_minutes() {
        let t0 = Instant::now();
        let mut s = Series::default();
        for i in 0..=WINDOW as u64 + 50 {
            s.push(t0 + Duration::from_secs(i), rd("eth0", i * i, i));
        }
        let snap = s.snapshot();
        assert_eq!(snap.rx.len(), WINDOW);
        assert_eq!(snap.tx.len(), WINDOW);
        // Oldest first: the last one is (i^2 - (i-1)^2) * 8 for i = 350.
        assert_eq!(*snap.rx.last().unwrap(), (2.0 * 350.0 - 1.0) * 8.0);
        assert_eq!(snap.rx[0], (2.0 * 51.0 - 1.0) * 8.0);
    }

    #[test]
    fn units_are_three_digits_with_k_m_g() {
        assert_eq!(bits(0.0), "0 bit/s");
        assert_eq!(bits(850.0), "850 bit/s");
        assert_eq!(bits(999.0), "999 bit/s");
        assert_eq!(bits(999.6), "1.00 kbit/s");
        assert_eq!(bits(12_345.0), "12.3 kbit/s");
        assert_eq!(bits(123_456.0), "123 kbit/s");
        assert_eq!(bits(1_204_000.0), "1.20 Mbit/s");
        assert_eq!(bits(9_999_000.0), "10.0 Mbit/s");
        assert_eq!(bits(940_000_000.0), "940 Mbit/s");
        assert_eq!(bits(2.5e9), "2.50 Gbit/s");
        assert_eq!(bits(4.0e12), "4000 Gbit/s", "G is the largest");
        assert_eq!(bits(f64::NAN), "0 bit/s");
        assert_eq!(bits(-5.0), "0 bit/s");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(45_600_000), "45.6 MB");
        assert_eq!(bytes(1_234_000_000), "1.23 GB");
        assert_eq!(speed(100), "100 Mbit/s");
        assert_eq!(speed(1000), "1 Gbit/s");
        assert_eq!(speed(2500), "2.5 Gbit/s");
        assert_eq!(speed(10000), "10 Gbit/s");
    }

    #[test]
    fn the_machine_s_own_addresses_include_loopback() {
        let a = addresses();
        assert!(
            a.iter()
                .any(|a| a.addr == IpAddr::V4(Ipv4Addr::LOCALHOST) && a.prefix == 8),
            "{a:?}"
        );
    }
}
