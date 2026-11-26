//! `thinfloe-share --radio`: the root helper that holds AWDL's radio from owlyd.
//!
//! Protocol on stdout (private to thinfloe-share), one line each:
//! `up<TAB>iface<TAB>mode<TAB>channel<TAB>flags<TAB>station<TAB>driver` once the radio is
//! up (iface the AWDL link, flags a comma list, `no-ble` when BlueZ offers no adapter,
//! station the Wi-Fi card under the link and driver its driver, empty when unknown;
//! `no-country` when neither the time zone nor the keyboard layout gave a country), then `error<TAB>text`
//! and exit 1 when the radio is lost or cannot be had. Exit 0 after the lease
//! is released on stdin EOF or SIGTERM.
//!
//! The lease is a plain `Acquire(0)`, never a detached one: it ends when this
//! process leaves the bus, so a crash gives the card back too.

use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use owlyd::api::{self, LinkInfo, RadioProxy, StateChangedStream};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::card::{
    Bus, CardInfo, CardInputs, NO_CARD, candidates, choose_country, country, wedged_text,
};
use crate::health::{self, Health};

/// The file owlyd reads each time the radio comes up (no restart needed).
pub const RADIO_TOML: &str = "/etc/owly/radio.toml";
const ZONEINFO: &str = "/usr/share/zoneinfo/";
/// The error when another program holds owlyd's radio.
pub const BUSY: &str = "The Wi-Fi radio is in use by another program.";
/// How long each BlueZ call of the advert may take before the up line reports `no-ble`.
const BLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// What the helper needs from owlyd; the zbus implementation is [`Owlyd`].
#[allow(async_fn_in_trait)]
pub trait RadioApi {
    /// What owlyd's probe says of the station radio.toml names now.
    async fn probe(&self) -> Result<Probe, String>;
    async fn acquire(&self) -> Result<(u64, LinkInfo), String>;
    /// The channel the radio runs on, read from the station's phy once the link is up.
    async fn link_channel(&self, station: &str) -> Option<u32>;
    async fn release(&self, lease: u64);
    /// A lease held while this helper starts: one a crashed run left
    /// detached. Releasing a lease of a live owner fails, harmlessly.
    async fn stale_detached(&self) -> Option<u64>;
    /// Whether a lease is held now (after the stale one was released): another program
    /// has the radio.
    async fn held(&self) -> bool;
    /// Completes once the card under the link has stopped sending (see [`health`]); pending
    /// for ever when that cannot be watched.
    async fn wait_wedged(&self, station: &str);
    /// Waits until `lease` ends without this helper releasing it; the reason.
    async fn wait_lost(&self, lease: u64) -> String;
}

/// What owlyd's probe says of a station: the mode the radio would run on and, when Owly
/// refuses the card, its first reason. The probe's channel is not the link's (it is 6 before
/// the country applies); the up line reads the link's with [`RadioApi::link_channel`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub mode: String,
    pub refusal: Option<String>,
}

/// owlyd's probe JSON: `mode` (else "unknown") and the `say` of the first `missing` entry.
pub fn parse_probe(json: &str) -> Result<Probe, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("owlyd's probe: {e}"))?;
    Ok(Probe {
        mode: v["mode"].as_str().unwrap_or("unknown").to_string(),
        refusal: v["missing"].as_array().and_then(|m| m.first()).map(|m| {
            m["say"]
                .as_str()
                .unwrap_or("Owly refuses this card.")
                .to_string()
        }),
    })
}

fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// What the up line reports: the AWDL link (`iface`, always `awdl0` with owlyd), the mode
/// of owlyd's probe, the channel the link runs on (0: unknown), the flags, and the Wi-Fi card under the link with its driver
/// (empty when unknown).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Up {
    pub iface: String,
    pub mode: String,
    pub channel: u32,
    pub flags: Vec<String>,
    pub station: String,
    pub driver: String,
}

pub fn status_line(r: &Result<Up, String>) -> String {
    match r {
        Ok(u) => format!(
            "up\t{}\t{}\t{}\t{}\t{}\t{}",
            one_line(&u.iface),
            one_line(&u.mode),
            u.channel,
            one_line(&u.flags.join(",")),
            one_line(&u.station),
            one_line(&u.driver)
        ),
        Err(text) => format!("error\t{}", one_line(text)),
    }
}

fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// radio.toml with `station` and, when known, `country`; everything else
/// stays at owlyd's defaults (mode from the quirks table, `allow_untested` false).
pub fn radio_toml(station: &str, country: Option<&str>) -> String {
    let mut s = format!("station = {}\n", toml_string(station));
    if let Some(c) = country {
        s.push_str(&format!("country = {}\n", toml_string(c)));
    }
    s
}

/// Two JSON arrays as one (`ip -j route` and `ip -6 -j route`); "" when
/// either is not an array, which the card choice reads as unreadable state.
pub fn merge_arrays(a: &str, b: &str) -> String {
    use serde_json::Value;
    match (
        serde_json::from_str::<Vec<Value>>(a),
        serde_json::from_str::<Vec<Value>>(b),
    ) {
        (Ok(mut x), Ok(y)) => {
            x.extend(y);
            Value::Array(x).to_string()
        }
        _ => String::new(),
    }
}

/// Completes when `r` reaches EOF (or fails).
pub async fn stdin_closed(mut r: impl AsyncRead + Unpin) {
    let mut buf = [0u8; 256];
    while let Ok(n) = r.read(&mut buf).await {
        if n == 0 {
            break;
        }
    }
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.thinfloe-tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

fn say(out: &mut impl Write, r: &Result<Up, String>) {
    let _ = writeln!(out, "{}", status_line(r));
    let _ = out.flush();
}

/// The whole helper after its inputs are read: card choice (each candidate written to
/// radio.toml and probed in turn, the first Owly accepts taken), the
/// lease, the BLE advert (`ble`, run once the radio is up) and the wait for
/// `stop` or the loss of the lease. Returns the exit code.
pub async fn hold<R: RadioApi, G>(
    api: &R,
    card: &CardInputs,
    country: Option<&str>,
    radio_toml_path: &Path,
    ble: impl Future<Output = (Option<G>, Vec<String>)>,
    stop: impl Future<Output = ()>,
    out: &mut impl Write,
) -> i32 {
    let stations = match candidates(card) {
        Ok(s) => s,
        Err(e) => {
            say(out, &Err(e.to_string()));
            return 1;
        }
    };
    if let Some(stale) = api.stale_detached().await {
        api.release(stale).await;
    }
    // A live owner's lease: radio.toml is left as that owner has it.
    if api.held().await {
        say(out, &Err(BUSY.into()));
        return 1;
    }
    let mut first_refusal: Option<String> = None;
    let mut last_error: Option<String> = None;
    let mut chosen = None;
    for station in stations {
        if let Err(e) = write_atomic(radio_toml_path, &radio_toml(&station, country)) {
            say(
                out,
                &Err(format!("Cannot write {}: {e}", radio_toml_path.display())),
            );
            return 1;
        }
        // A probe that fails on one card may work on the next: tried like a refusal.
        let probe = match api.probe().await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("thinfloe-share: probe of {station}: {e}");
                last_error = Some(e);
                continue;
            }
        };
        match probe.refusal {
            None => {
                chosen = Some((station, probe));
                break;
            }
            Some(why) => {
                eprintln!("thinfloe-share: Owly refuses {station}: {why}");
                first_refusal.get_or_insert(why);
            }
        }
    }
    let Some((station, Probe { mode, .. })) = chosen else {
        say(
            out,
            &Err(first_refusal
                .or(last_error)
                .unwrap_or_else(|| NO_CARD.into())),
        );
        return 1;
    };
    let bus = card.cards.get(&station).map(|c| c.bus).unwrap_or_default();
    let (lease, link) = match api.acquire().await {
        Ok(l) => l,
        // EBUSY: the card stopped sending in an earlier session and is still stuck.
        Err(e) if health::busy(&e) => {
            eprintln!("thinfloe-share: acquire on {station}: {e}");
            say(out, &Err(wedged_text(&station, bus)));
            return 1;
        }
        Err(e) => {
            say(out, &Err(e));
            return 1;
        }
    };
    let (guard, mut flags) = ble.await;
    if country.is_none() {
        flags.push("no-country".into());
    }
    let channel = api.link_channel(&station).await.unwrap_or(0);
    let driver = card
        .cards
        .get(&station)
        .and_then(|c| c.driver.clone())
        .unwrap_or_default();
    say(
        out,
        &Ok(Up {
            iface: link.interface.clone(),
            mode,
            channel,
            flags,
            station: station.clone(),
            driver,
        }),
    );
    let code = tokio::select! {
        _ = stop => {
            api.release(lease).await;
            0
        }
        reason = api.wait_lost(lease) => {
            say(out, &Err(format!("The radio stopped: {reason}")));
            1
        }
        () = api.wait_wedged(&station) => {
            api.release(lease).await;
            say(out, &Err(wedged_text(&station, bus)));
            1
        }
    };
    drop(guard);
    code
}

/// owlyd on the system bus. The state stream is opened at connect, before
/// any Acquire, so a loss right after it is not missed.
pub struct Owlyd {
    radio: RadioProxy<'static>,
    states: tokio::sync::Mutex<StateChangedStream>,
    owner: tokio::sync::Mutex<zbus::proxy::OwnerChangedStream<'static>>,
}

fn unreachable(e: &zbus::Error) -> String {
    format!("owlyd cannot be reached ({e}).")
}

fn api_text(e: &zbus::Error) -> String {
    match api::api_error(e) {
        Some((_, m)) if !m.is_empty() => m,
        _ if api::unreachable(e) => unreachable(e),
        _ => e.to_string(),
    }
}

impl Owlyd {
    pub async fn connect() -> Result<Self, String> {
        let conn = api::connect(None).await.map_err(|e| unreachable(&e))?;
        let radio = api::radio(&conn).await.map_err(|e| unreachable(&e))?;
        let states = radio
            .receive_radio_state()
            .await
            .map_err(|e| unreachable(&e))?;
        let owner = radio
            .inner()
            .receive_owner_changed()
            .await
            .map_err(|e| unreachable(&e))?;
        Ok(Owlyd {
            radio,
            states: tokio::sync::Mutex::new(states),
            owner: tokio::sync::Mutex::new(owner),
        })
    }
}

impl RadioApi for Owlyd {
    async fn probe(&self) -> Result<Probe, String> {
        parse_probe(&self.radio.probe().await.map_err(|e| api_text(&e))?)
    }

    async fn acquire(&self) -> Result<(u64, LinkInfo), String> {
        let (lease, dict) = self.radio.acquire(0).await.map_err(|e| api_text(&e))?;
        match LinkInfo::from_dict(&dict) {
            Some(l) => Ok((lease, l)),
            None => {
                self.release(lease).await;
                Err("owlyd gave a link this helper cannot read.".into())
            }
        }
    }

    async fn link_channel(&self, station: &str) -> Option<u32> {
        let index =
            std::fs::read_to_string(Path::new(SYS_NET).join(station).join("phy80211/index"))
                .ok()?;
        let o = tokio::process::Command::new("iw")
            .arg("dev")
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())?;
        iw_channel(&String::from_utf8_lossy(&o.stdout), index.trim())
    }

    async fn release(&self, lease: u64) {
        if let Err(e) = self.radio.release(lease).await {
            eprintln!("thinfloe-share: releasing lease {lease}: {}", api_text(&e));
        }
    }

    async fn wait_wedged(&self, station: &str) {
        let child = tokio::process::Command::new("journalctl")
            .args([
                "-u",
                "owlyd.service",
                "-f",
                "-n",
                "0",
                "-o",
                "cat",
                "--no-pager",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                eprintln!("thinfloe-share: cannot follow owlyd's journal ({e}); no wedge watch");
                return std::future::pending().await;
            }
        };
        let Some(stdout) = child.stdout.take() else {
            return std::future::pending().await;
        };
        if !watch_journal(tokio::io::BufReader::new(stdout), station).await {
            eprintln!("thinfloe-share: owlyd's journal ended; no wedge watch");
            std::future::pending().await
        }
    }

    async fn stale_detached(&self) -> Option<u64> {
        match self.radio.lease().await {
            Ok(0) | Err(_) => None,
            Ok(l) => Some(l),
        }
    }

    async fn held(&self) -> bool {
        // An unreadable property is not a holder: Acquire then says what is wrong.
        matches!(self.radio.lease().await, Ok(l) if l != 0)
    }

    async fn wait_lost(&self, lease: u64) -> String {
        let mut states = self.states.lock().await;
        // Ended before the wait began: the signal went by in the stream, or
        // the properties tell.
        if let Ok(now) = self.radio.lease().await
            && now != lease
            && let Ok((last, reason)) = self.radio.last_end().await
            && last == lease
        {
            return reason;
        }
        let mut owner = self.owner.lock().await;
        loop {
            tokio::select! {
                s = states.next() => {
                    let Some(s) = s else { return "owlyd left the bus".into() };
                    let Ok(a) = s.args() else { continue };
                    if a.lease == lease && a.state == "idle" {
                        return a.reason;
                    }
                }
                change = owner.next() => {
                    if owner_lost(change) {
                        return "owlyd stopped".into();
                    }
                }
            }
        }
    }
}

/// The channel in the `phy#<phy_index>` section of `iw dev` output: the number of the first
/// `channel N (` line of any of its interfaces. Other phys are ignored.
pub fn iw_channel(iw_dev: &str, phy_index: &str) -> Option<u32> {
    let head = format!("phy#{phy_index}");
    let mut inside = false;
    for line in iw_dev.lines() {
        if !line.starts_with(char::is_whitespace) && !line.is_empty() {
            inside = line.trim_end() == head;
            continue;
        }
        if !inside {
            continue;
        }
        if let Some(rest) = line.trim_start().strip_prefix("channel ")
            && let Some((n, _)) = rest.split_once(" (")
            && let Ok(n) = n.parse()
        {
            return Some(n);
        }
    }
    None
}

/// Feeds owlyd's journal lines (`journalctl -o cat`) to the wedge rule: true once the card
/// stopped sending (logged), false when the journal ends first.
pub async fn watch_journal(journal: impl tokio::io::AsyncBufRead + Unpin, station: &str) -> bool {
    use tokio::io::AsyncBufReadExt;
    let mut lines = journal.lines();
    let mut health = Health::default();
    while let Ok(Some(l)) = lines.next_line().await {
        if let Some(c) = health::parse_stats(&l)
            && health.feed(c)
        {
            let (gave_up, sent) = health.run();
            eprintln!(
                "thinfloe-share: {station} gave up {gave_up} of {sent} frames for {} s; the card stopped sending",
                health::STUCK_INTERVALS * 10
            );
            return true;
        }
    }
    false
}

/// Whether an item of owlyd's name-owner stream means owlyd is gone (and
/// its leases with it): the name lost its owner, or the stream ended. A new
/// owner is not a loss: D-Bus activation by the helper's own first call
/// gives one, and a restart shows the name gone first.
fn owner_lost<T>(change: Option<Option<T>>) -> bool {
    !matches!(change, Some(Some(_)))
}

const SYS_NET: &str = "/sys/class/net";

/// Wi-Fi interfaces: those with a `wireless` directory in sysfs.
fn wifi_ifaces() -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(SYS_NET) else {
        return Vec::new();
    };
    dir.filter_map(|e| e.ok())
        .filter(|e| e.path().join("wireless").exists())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

/// Bus, PCI ID and driver of `iface`'s card, from `<net>/<iface>/device` (`net` is
/// `/sys/class/net`); what cannot be read stays unknown.
fn card_info(net: &Path, iface: &str) -> CardInfo {
    let dev = net.join(iface).join("device");
    let link_name = |name: &str| {
        std::fs::read_link(dev.join(name))
            .ok()
            .and_then(|t| t.file_name()?.to_str().map(String::from))
    };
    let hex = |name: &str| {
        let s = std::fs::read_to_string(dev.join(name)).ok()?;
        u16::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
    };
    let bus = match link_name("subsystem").as_deref() {
        Some("usb") => Bus::Usb,
        Some("pci") => Bus::Pci,
        _ => Bus::Other,
    };
    CardInfo {
        bus,
        pci_id: (bus == Bus::Pci)
            .then(|| hex("vendor").zip(hex("device")))
            .flatten(),
        driver: link_name("driver"),
    }
}

fn ip_json(args: &[&str]) -> String {
    std::process::Command::new("ip")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default()
}

fn tfuser_home() -> Option<PathBuf> {
    let o = std::process::Command::new("getent")
        .args(["passwd", "tfuser"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let line = String::from_utf8(o.stdout).ok()?;
    let home = line.trim_end().split(':').nth(5)?;
    (!home.is_empty()).then(|| PathBuf::from(home))
}

/// generated/wifi.yaml of tfuser and the netplan files the initramfs
/// installed; a card named in any of them is never taken.
fn wifi_yaml() -> Option<String> {
    let mut texts = Vec::new();
    if let Some(home) = tfuser_home()
        && let Ok(t) = std::fs::read_to_string(home.join(".config/thinfloe/generated/wifi.yaml"))
    {
        texts.push(t);
    }
    if let Ok(dir) = std::fs::read_dir("/etc/netplan") {
        let mut paths: Vec<PathBuf> = dir
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
            .collect();
        paths.sort();
        texts.extend(paths.iter().filter_map(|p| std::fs::read_to_string(p).ok()));
    }
    (!texts.is_empty()).then(|| texts.join("\n"))
}

/// The zone a link to zoneinfo names: the part after `zoneinfo/`, without a leading
/// `posix/` or `right/` (the same zones with other leap second rules).
pub fn zone_of(link_target: &str) -> Option<String> {
    let (_, z) = link_target.split_once("zoneinfo/")?;
    let z = z
        .strip_prefix("posix/")
        .or_else(|| z.strip_prefix("right/"))
        .unwrap_or(z);
    (!z.is_empty()).then(|| z.to_string())
}

/// The zone /etc/localtime points to, as timedatectl sets it.
fn zone() -> Option<String> {
    let target = std::fs::read_link("/etc/localtime").ok()?;
    zone_of(target.to_str()?)
}

/// The value of the last `XKB_DEFAULT_LAYOUT=` line of an environment file; None when empty.
pub fn env_layout(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("XKB_DEFAULT_LAYOUT="))
        .next_back()
        .map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        .filter(|v| !v.is_empty())
}

/// The keyboard layout the menu wrote to tfuser's generated/environment; None when the
/// file is missing or unreadable.
fn keyboard_layout() -> Option<String> {
    let home = tfuser_home()?;
    let text = std::fs::read_to_string(home.join(".config/thinfloe/generated/environment")).ok()?;
    env_layout(&text)
}

fn gather() -> (CardInputs, Option<String>) {
    let wifi_ifaces = wifi_ifaces();
    let cards = wifi_ifaces
        .iter()
        .map(|i| (i.clone(), card_info(Path::new(SYS_NET), i)))
        .collect();
    let card = CardInputs {
        ip_addr_json: ip_json(&["-j", "addr"]),
        ip_route_json: merge_arrays(&ip_json(&["-j", "route"]), &ip_json(&["-6", "-j", "route"])),
        wifi_yaml: wifi_yaml(),
        wifi_ifaces,
        cards,
    };
    let tab = std::fs::read_to_string(format!("{ZONEINFO}zone1970.tab")).unwrap_or_default();
    let zone = zone();
    let layout = keyboard_layout();
    let found = choose_country(zone.as_deref(), layout.as_deref(), &tab);
    if let (Some(cc), Some(l)) = (&found, &layout)
        && zone.as_deref().and_then(|z| country(z, &tab)).is_none()
    {
        eprintln!("thinfloe-share: country {cc} from the keyboard layout {l}");
    }
    (card, found)
}

fn random_tag() -> [u8; 3] {
    let mut tag = [0u8; 3];
    // SAFETY: getrandom writes at most tag.len() bytes into the buffer.
    let n = unsafe { libc::getrandom(tag.as_mut_ptr().cast(), tag.len(), 0) };
    if n != tag.len() as isize {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
            ^ std::process::id();
        tag.copy_from_slice(&t.to_le_bytes()[..3]);
    }
    tag
}

async fn ble() -> (Option<crate::bluez::Registration>, Vec<String>) {
    use owly_airdrop::proto::ble::Advert;
    // Each BlueZ call is bounded inside: a timeout after the adapter was turned on turns it
    // off again, which an outer timeout dropping the future could not.
    match crate::bluez::advertise(Advert::anonymous(random_tag()), BLE_TIMEOUT).await {
        Ok(Some(r)) => (Some(r), Vec::new()),
        Ok(None) => (None, vec!["no-ble".into()]),
        Err(e) => {
            eprintln!("thinfloe-share: no BLE advert: {e}");
            (None, vec!["no-ble".into()])
        }
    }
}

/// How long a SIGTERM or SIGINT waits for the daemon to close stdin. The menu stops
/// FloeDrop with SIGTERM to the whole process group, which holds this helper too, and the
/// daemon sends its mDNS goodbye before it closes stdin: the radio must stay up for it.
/// Below the menu's 1 s before its SIGKILL.
pub const TERM_WAIT: std::time::Duration = std::time::Duration::from_millis(700);

/// Completes on EOF of `stdin`; after `signal`, on EOF within [`TERM_WAIT`] or at its end.
pub async fn stop_on(stdin: impl AsyncRead + Unpin, signal: impl Future<Output = ()>) {
    let mut closed = std::pin::pin!(stdin_closed(stdin));
    tokio::select! {
        _ = &mut closed => return,
        _ = signal => {}
    }
    let _ = tokio::time::timeout(TERM_WAIT, closed).await;
}

/// The SIGTERM and SIGINT handlers, installed before anything else, so a signal during the
/// start no longer ends the helper with Bluetooth on: it waits for [`stop`].
struct Stop {
    term: tokio::signal::unix::Signal,
    int: tokio::signal::unix::Signal,
}

fn install_stop() -> Result<Stop, String> {
    use tokio::signal::unix::{SignalKind, signal};
    let term = signal(SignalKind::terminate()).map_err(|e| format!("SIGTERM: {e}"))?;
    let int = signal(SignalKind::interrupt()).map_err(|e| format!("SIGINT: {e}"))?;
    Ok(Stop { term, int })
}

/// Completes on stdin's EOF, or after SIGTERM or SIGINT (see [`stop_on`]).
async fn stop(mut s: Stop) {
    let signalled = async {
        tokio::select! {
            _ = s.term.recv() => {}
            _ = s.int.recv() => {}
        }
    };
    stop_on(tokio::io::stdin(), signalled).await;
}

/// A SIGHUP ends nothing here: sudo relays the signals its group gets, and
/// a reload of the daemon must not end the helper.
fn ignore_hangup() {
    // SAFETY: setting a signal to SIG_IGN has no memory effects.
    unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
}

/// `thinfloe-share --radio`; the exit code.
pub fn main() -> i32 {
    ignore_hangup();
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("thinfloe-share: --radio must run as root");
        return 2;
    }
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("thinfloe-share: {e}");
            return 1;
        }
    };
    rt.block_on(async {
        let mut out = std::io::stdout();
        let signals = match install_stop() {
            Ok(s) => s,
            Err(e) => {
                say(
                    &mut out,
                    &Err(format!("Cannot install signal handlers: {e}")),
                );
                return 1;
            }
        };
        let (card, country) = gather();
        // No free card is said before owlyd is asked, whose error would hide it.
        if let Err(e) = candidates(&card) {
            say(&mut out, &Err(e.to_string()));
            return 1;
        }
        let owlyd = match Owlyd::connect().await {
            Ok(o) => o,
            Err(e) => {
                say(&mut out, &Err(e));
                return 1;
            }
        };
        let mut reg = None;
        let code = hold(
            &owlyd,
            &card,
            country.as_deref(),
            Path::new(RADIO_TOML),
            async {
                let (r, flags) = ble().await;
                reg = r;
                (None::<()>, flags)
            },
            stop(signals),
            &mut out,
        )
        .await;
        if let Some(r) = reg {
            r.stop().await;
        }
        code
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_helper_lives_through_a_sighup() {
        // In a child of its own, so the test binary keeps its handlers. The
        // child calls only async-signal-safe functions.
        // SAFETY: fork; the child uses signal, raise and _exit only.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            unsafe {
                libc::signal(libc::SIGHUP, libc::SIG_DFL);
                super::ignore_hangup();
                libc::raise(libc::SIGHUP);
                libc::_exit(7);
            }
        }
        let mut status = 0;
        // SAFETY: waitpid on our own child with a valid status pointer.
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert!(libc::WIFEXITED(status), "ended by a signal: {status:#x}");
        assert_eq!(libc::WEXITSTATUS(status), 7);
    }

    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    const ADDR: &str = include_str!("../testdata/ip-addr.json");
    const ROUTE: &str = include_str!("../testdata/ip-route.json");

    fn link(iface: &str) -> LinkInfo {
        LinkInfo {
            interface: iface.into(),
            index: 7,
            mac: "aa:bb:cc:dd:ee:01".into(),
            address: "fe80::a8bb:ccff:fedd:ee01".into(),
            announce: None,
        }
    }

    #[derive(Default)]
    struct Fake {
        stale: Option<u64>,
        lost: Option<String>,
        /// The radio.toml the probe reads its station from; `run` sets it.
        toml: RefCell<PathBuf>,
        /// Stations the probe refuses, with the reason.
        refuse: Vec<(String, String)>,
        /// What `link_channel` gives.
        channel: Option<u32>,
        /// `wait_wedged` completes at once.
        wedged: bool,
        /// `acquire` fails with this text.
        acquire_error: Option<String>,
        /// Another program holds the radio.
        held: bool,
        /// Stations whose probe call fails, with the error.
        probe_error: Vec<(String, String)>,
        calls: RefCell<Vec<String>>,
    }

    impl Fake {
        fn station(&self) -> String {
            let t = std::fs::read_to_string(&*self.toml.borrow()).unwrap_or_default();
            t.lines()
                .find_map(|l| l.strip_prefix("station = "))
                .unwrap_or("")
                .trim_matches('"')
                .to_string()
        }
    }

    impl RadioApi for Fake {
        async fn probe(&self) -> Result<Probe, String> {
            let station = self.station();
            self.calls.borrow_mut().push(format!("probe {station}"));
            if let Some((_, e)) = self.probe_error.iter().find(|(s, _)| *s == station) {
                return Err(e.clone());
            }
            let refusal = self
                .refuse
                .iter()
                .find(|(s, _)| *s == station)
                .map(|(_, why)| why.clone());
            Ok(Probe {
                mode: "exclusive".into(),
                refusal,
            })
        }
        async fn link_channel(&self, station: &str) -> Option<u32> {
            self.calls.borrow_mut().push(format!("channel {station}"));
            self.channel
        }
        async fn acquire(&self) -> Result<(u64, LinkInfo), String> {
            self.calls.borrow_mut().push("acquire".into());
            match &self.acquire_error {
                Some(e) => Err(e.clone()),
                None => Ok((5, link("awdl0"))),
            }
        }
        async fn release(&self, lease: u64) {
            self.calls.borrow_mut().push(format!("release {lease}"));
        }
        async fn stale_detached(&self) -> Option<u64> {
            self.stale
        }
        async fn held(&self) -> bool {
            self.held
        }
        async fn wait_wedged(&self, _station: &str) {
            if !self.wedged {
                std::future::pending::<()>().await;
            }
        }
        async fn wait_lost(&self, _lease: u64) -> String {
            match &self.lost {
                Some(r) => r.clone(),
                None => std::future::pending().await,
            }
        }
    }

    fn free_card() -> CardInputs {
        CardInputs {
            ip_addr_json: ADDR.into(),
            ip_route_json: ROUTE.into(),
            wifi_yaml: None,
            wifi_ifaces: vec!["wlp2s0".into(), "wlx00e04c123456".into()],
            cards: Default::default(),
        }
    }

    /// A test's own directory below target/, removed when the guard drops.
    struct Tmp {
        dir: PathBuf,
        toml: PathBuf,
    }

    impl Tmp {
        /// The radio.toml path inside it (not written).
        fn toml(&self) -> PathBuf {
            self.toml.clone()
        }
    }

    impl AsRef<Path> for Tmp {
        fn as_ref(&self) -> &Path {
            &self.toml
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The guard derefs to its radio.toml path, so a test passes `&tmp(..)` as the path.
    impl std::ops::Deref for Tmp {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.toml
        }
    }

    fn tmp(name: &str) -> Tmp {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-tmp"))
            .join(format!("radio-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let toml = dir.join("owly").join("radio.toml");
        Tmp { dir, toml }
    }

    #[test]
    fn a_tmp_dir_goes_with_its_guard() {
        let t = tmp("guard");
        let dir = t.dir.clone();
        std::fs::create_dir_all(t.toml().parent().unwrap()).unwrap();
        std::fs::write(t.toml(), "x").unwrap();
        assert!(dir.is_dir());
        drop(t);
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn a_held_radio_writes_nothing() {
        let fake = Fake {
            held: true,
            channel: Some(44),
            ..Default::default()
        };
        let toml = tmp("held");
        let (code, out) = run(&fake, &free_card(), &toml, async {}).await;
        assert_eq!(code, 1);
        assert_eq!(out, format!("error\t{BUSY}\n"));
        assert!(!toml.exists(), "radio.toml is not written");
        assert!(!fake.calls.borrow().iter().any(|c| c == "acquire"));
    }

    #[tokio::test]
    async fn a_probe_error_tries_the_next_card() {
        let fake = Fake {
            probe_error: vec![("wlx00e04c123456".into(), "owlyd's probe: broken".into())],
            channel: Some(44),
            ..Default::default()
        };
        let (code, out) = run(&fake, &stick_and_internal(), &tmp("probe-err"), async {}).await;
        assert_eq!(code, 0);
        assert!(
            out.starts_with("up\tawdl0\texclusive\t44\tno-ble\twlp3s0\t"),
            "{out}"
        );
        // Every probe fails: the last error is the line.
        let fake = Fake {
            probe_error: vec![
                ("wlx00e04c123456".into(), "owlyd's probe: first".into()),
                ("wlp3s0".into(), "owlyd's probe: last".into()),
            ],
            ..Default::default()
        };
        let (code, out) = run(&fake, &stick_and_internal(), &tmp("probe-err2"), async {}).await;
        assert_eq!(code, 1);
        assert_eq!(out, "error\towlyd's probe: last\n");
        assert!(!fake.calls.borrow().iter().any(|c| c == "acquire"));
        // A refusal beside a failed probe: the refusal is the line.
        let fake = Fake {
            refuse: vec![("wlx00e04c123456".into(), "untested".into())],
            probe_error: vec![("wlp3s0".into(), "owlyd's probe: last".into())],
            ..Default::default()
        };
        let (code, out) = run(&fake, &stick_and_internal(), &tmp("probe-err3"), async {}).await;
        assert_eq!(code, 1);
        assert_eq!(out, "error\tuntested\n");
    }

    #[test]
    fn posix_and_right_zones_have_a_country() {
        for t in [
            "../usr/share/zoneinfo/posix/Europe/Zurich",
            "/usr/share/zoneinfo/right/Europe/Zurich",
            "/usr/share/zoneinfo/Europe/Zurich",
        ] {
            assert_eq!(zone_of(t).as_deref(), Some("Europe/Zurich"), "{t}");
        }
        assert_eq!(zone_of("/usr/share/zoneinfo/UTC").as_deref(), Some("UTC"));
        assert_eq!(zone_of("/etc/foo"), None);
        assert_eq!(zone_of("/usr/share/zoneinfo/"), None);
    }

    #[test]
    fn no_free_card_is_said_before_owlyd() {
        // main() calls candidates() before Owlyd::connect(); the function is pure.
        let mut card = free_card();
        card.wifi_ifaces = vec!["wlp2s0".into()];
        assert_eq!(
            candidates(&card).map_err(|e| e.to_string()),
            Err(NO_CARD.to_string())
        );
    }

    async fn no_ble() -> (Option<()>, Vec<String>) {
        (None, vec!["no-ble".into()])
    }

    async fn run(
        fake: &Fake,
        card: &CardInputs,
        toml: &Path,
        stop: impl Future<Output = ()>,
    ) -> (i32, String) {
        *fake.toml.borrow_mut() = toml.to_path_buf();
        let mut out = Vec::new();
        let code = hold(fake, card, Some("CH"), toml, no_ble(), stop, &mut out).await;
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn only_a_lost_owner_ends_the_wait() {
        // Activation: "" -> ":1.42" is not a loss.
        assert!(!owner_lost(Some(Some(":1.42"))));
        assert!(owner_lost(Some(None::<&str>)));
        assert!(owner_lost(None::<Option<&str>>));
    }

    #[test]
    fn up_line_format() {
        let mut up = Up {
            iface: "awdl0".into(),
            mode: "exclusive".into(),
            channel: 44,
            flags: vec![],
            station: "wlxc03a55f0d060".into(),
            driver: "rtw88_8822bu".into(),
        };
        assert_eq!(
            status_line(&Ok(up.clone())),
            "up\tawdl0\texclusive\t44\t\twlxc03a55f0d060\trtw88_8822bu"
        );
        up.mode = "shared".into();
        up.channel = 6;
        up.flags = vec!["no-ble".into(), "x".into()];
        up.driver = String::new();
        assert_eq!(
            status_line(&Ok(up)),
            "up\tawdl0\tshared\t6\tno-ble,x\twlxc03a55f0d060\t"
        );
        let err: Result<Up, String> = Err("bad\tthing\nhere".into());
        assert_eq!(status_line(&err), "error\tbad thing here");
    }

    #[tokio::test]
    async fn no_free_card_prints_error_and_acquires_nothing() {
        let fake = Fake {
            channel: Some(44),
            ..Default::default()
        };
        let mut card = free_card();
        card.wifi_ifaces = vec!["wlp2s0".into()];
        let toml = tmp("nocard");
        let (code, out) = run(&fake, &card, &toml, async {}).await;
        assert_eq!(code, 1);
        assert_eq!(out, format!("error\t{}\n", crate::card::NO_CARD));
        assert!(fake.calls.borrow().is_empty());
        assert!(!toml.exists());
    }

    #[tokio::test]
    async fn a_stale_detached_lease_is_released_first() {
        let fake = Fake {
            stale: Some(3),
            channel: Some(44),
            ..Default::default()
        };
        let (code, out) = run(&fake, &free_card(), &tmp("stale"), async {}).await;
        assert_eq!(code, 0);
        assert_eq!(out, "up\tawdl0\texclusive\t44\tno-ble\twlx00e04c123456\t\n");
        assert_eq!(
            *fake.calls.borrow(),
            [
                "release 3",
                "probe wlx00e04c123456",
                "acquire",
                "channel wlx00e04c123456",
                "release 5"
            ]
        );
    }

    #[tokio::test]
    async fn the_up_line_names_the_station_and_its_driver() {
        let fake = Fake {
            channel: Some(44),
            ..Default::default()
        };
        let mut card = free_card();
        card.cards.insert(
            "wlx00e04c123456".into(),
            CardInfo {
                bus: Bus::Usb,
                pci_id: None,
                driver: Some("rtw88_8822bu".into()),
            },
        );
        let (code, out) = run(&fake, &card, &tmp("station"), async {}).await;
        assert_eq!(code, 0);
        assert_eq!(
            out,
            "up\tawdl0\texclusive\t44\tno-ble\twlx00e04c123456\trtw88_8822bu\n"
        );
    }

    #[tokio::test]
    async fn lease_lost_is_an_error_line() {
        let fake = Fake {
            lost: Some("link lost".into()),
            channel: Some(44),
            ..Default::default()
        };
        let (code, out) = run(&fake, &free_card(), &tmp("lost"), std::future::pending()).await;
        assert_eq!(code, 1);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("up\tawdl0\t"));
        assert_eq!(lines[1], "error\tThe radio stopped: link lost");
        // The lease is gone already; nothing to release.
        assert_eq!(
            *fake.calls.borrow(),
            [
                "probe wlx00e04c123456",
                "acquire",
                "channel wlx00e04c123456"
            ]
        );
    }

    #[tokio::test]
    async fn stdin_eof_releases_the_lease() {
        let fake = Fake {
            channel: Some(44),
            ..Default::default()
        };
        let stop = stdin_closed(tokio::io::empty());
        let (code, out) = run(&fake, &free_card(), &tmp("eof"), stop).await;
        assert_eq!(code, 0);
        assert!(out.starts_with("up\t"));
        assert_eq!(fake.calls.borrow().last().unwrap(), "release 5");
    }

    #[test]
    fn probe_json_parses() {
        assert_eq!(
            parse_probe(r#"{"mode":"exclusive","channel":6,"missing":[]}"#).unwrap(),
            Probe {
                mode: "exclusive".into(),
                refusal: None
            }
        );
        let p = parse_probe(
            r#"{"mode":"exclusive","missing":[{"id":"hardware","say":"Owly hasn't been tested on this computer's Wi-Fi (rtl8xxxu)."}]}"#,
        )
        .unwrap();
        assert_eq!(
            p.refusal.as_deref(),
            Some("Owly hasn't been tested on this computer's Wi-Fi (rtl8xxxu).")
        );
        assert!(parse_probe("x").unwrap_err().starts_with("owlyd's probe: "));
    }

    fn stick_and_internal() -> CardInputs {
        let mut card = free_card();
        card.wifi_ifaces = vec!["wlx00e04c123456".into(), "wlp3s0".into()];
        card.cards.insert(
            "wlx00e04c123456".into(),
            CardInfo {
                bus: Bus::Usb,
                pci_id: None,
                driver: Some("rtl8xxxu".into()),
            },
        );
        card.cards.insert(
            "wlp3s0".into(),
            CardInfo {
                bus: Bus::Pci,
                pci_id: Some((0x8086, 0x2723)),
                driver: Some("iwlwifi".into()),
            },
        );
        card
    }

    #[tokio::test]
    async fn a_refused_stick_falls_back_to_the_next_card() {
        let fake = Fake {
            refuse: vec![("wlx00e04c123456".into(), "untested".into())],
            channel: Some(44),
            ..Default::default()
        };
        let toml = tmp("refused");
        let (code, out) = run(&fake, &stick_and_internal(), &toml, async {}).await;
        assert_eq!(code, 0);
        assert_eq!(
            *fake.calls.borrow(),
            [
                "probe wlx00e04c123456",
                "probe wlp3s0",
                "acquire",
                "channel wlp3s0",
                "release 5"
            ]
        );
        assert_eq!(out, "up\tawdl0\texclusive\t44\tno-ble\twlp3s0\tiwlwifi\n");
        assert!(
            std::fs::read_to_string(&toml)
                .unwrap()
                .contains("station = \"wlp3s0\"")
        );
    }

    #[tokio::test]
    async fn all_refused_reports_the_first_cards_reason() {
        let fake = Fake {
            refuse: vec![
                ("wlx00e04c123456".into(), "A".into()),
                ("wlp3s0".into(), "B".into()),
            ],
            channel: Some(44),
            ..Default::default()
        };
        let (code, out) = run(&fake, &stick_and_internal(), &tmp("allrefused"), async {}).await;
        assert_eq!(code, 1);
        assert_eq!(out, "error\tA\n");
        assert!(!fake.calls.borrow().iter().any(|c| c == "acquire"));
    }

    const IW_DEV: &str = "phy#1
\tInterface owlyibss0
\t\tifindex 9
\t\ttype IBSS
\t\tchannel 44 (5220 MHz), width: 20 MHz (no HT), center1: 5220 MHz
\tInterface wlxc03a55f0d060
\t\tifindex 5
\t\ttype managed
phy#0
\tInterface wlp2s0
\t\tifindex 3
\t\ttype managed
\t\tchannel 1 (2412 MHz), width: 20 MHz, center1: 2412 MHz
";

    #[test]
    fn the_channel_is_read_from_the_stations_phy_only() {
        assert_eq!(iw_channel(IW_DEV, "1"), Some(44));
        assert_eq!(iw_channel(IW_DEV, "0"), Some(1));
        assert_eq!(iw_channel(IW_DEV, "2"), None);
        assert_eq!(iw_channel("", "1"), None);
        // "phy#1" is not "phy#10".
        assert_eq!(
            iw_channel(&IW_DEV.replace("phy#1\n", "phy#10\n"), "1"),
            None
        );
    }

    #[tokio::test]
    async fn the_up_line_has_the_links_channel() {
        let fake = Fake {
            channel: Some(44),
            ..Default::default()
        };
        let (_, out) = run(&fake, &free_card(), &tmp("chan44"), async {}).await;
        assert_eq!(out, "up\tawdl0\texclusive\t44\tno-ble\twlx00e04c123456\t\n");
        let fake = Fake::default();
        let (_, out) = run(&fake, &free_card(), &tmp("chan0"), async {}).await;
        assert_eq!(out, "up\tawdl0\texclusive\t0\tno-ble\twlx00e04c123456\t\n");
    }

    #[test]
    fn the_layout_comes_from_generated_environment() {
        assert_eq!(
            env_layout("# header\nXKB_DEFAULT_LAYOUT=ch\nXKB_DEFAULT_VARIANT=fr\n").as_deref(),
            Some("ch")
        );
        assert_eq!(env_layout("XKB_DEFAULT_LAYOUT=\n"), None);
        assert_eq!(env_layout("XKB_DEFAULT_VARIANT=fr\n"), None);
    }

    #[tokio::test]
    async fn no_country_is_a_flag() {
        let fake = Fake {
            channel: Some(44),
            ..Default::default()
        };
        let toml = tmp("nocountry");
        *fake.toml.borrow_mut() = toml.toml();
        let mut out = Vec::new();
        hold(
            &fake,
            &free_card(),
            None,
            &toml,
            no_ble(),
            async {},
            &mut out,
        )
        .await;
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out.split('\t').nth(4), Some("no-ble,no-country"), "{out}");
        let (_, out) = run(&fake, &free_card(), &tmp("country"), async {}).await;
        assert_eq!(out.split('\t').nth(4), Some("no-ble"), "{out}");
    }

    fn usb_stick() -> CardInputs {
        let mut card = free_card();
        card.cards.insert(
            "wlx00e04c123456".into(),
            CardInfo {
                bus: Bus::Usb,
                pci_id: None,
                driver: Some("rtw88_8822bu".into()),
            },
        );
        card
    }

    const WEDGED: &str =
        "error\tThe Wi-Fi stick wlx00e04c123456 stopped sending. Unplug it and plug it in again.";

    #[tokio::test]
    async fn a_wedge_releases_the_lease_and_says_why() {
        let fake = Fake {
            channel: Some(44),
            wedged: true,
            ..Default::default()
        };
        let (code, out) = run(&fake, &usb_stick(), &tmp("wedged"), std::future::pending()).await;
        assert_eq!(code, 1);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "{out}");
        assert!(lines[0].starts_with("up\tawdl0\t"));
        assert_eq!(lines[1], WEDGED);
        assert_eq!(fake.calls.borrow().last().unwrap(), "release 5");
    }

    #[tokio::test]
    async fn a_busy_start_is_a_wedged_stick() {
        let fake = Fake {
            acquire_error: Some(
                "Couldn't bring owlyibss0 up: Device or resource busy (os error 16)".into(),
            ),
            ..Default::default()
        };
        let (code, out) = run(&fake, &usb_stick(), &tmp("busy"), std::future::pending()).await;
        assert_eq!(code, 1);
        assert_eq!(out, format!("{WEDGED}\n"));
        assert!(!fake.calls.borrow().iter().any(|c| c.starts_with("release")));
    }

    #[tokio::test]
    async fn another_acquire_error_is_passed_on() {
        let fake = Fake {
            acquire_error: Some("AirDrop's radio is in use (lease 3).".into()),
            ..Default::default()
        };
        let (code, out) = run(&fake, &usb_stick(), &tmp("inuse"), std::future::pending()).await;
        assert_eq!(code, 1);
        assert_eq!(out, "error\tAirDrop's radio is in use (lease 3).\n");
    }

    #[tokio::test]
    async fn a_sigterm_waits_for_the_daemons_eof() {
        use std::time::{Duration, Instant};
        // EOF 100 ms after the signal: the stop waits for it.
        let (w, r) = tokio::io::duplex(64);
        let t = Instant::now();
        let closer = async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(w);
        };
        tokio::join!(stop_on(r, async {}), closer);
        let took = t.elapsed();
        assert!(
            took >= Duration::from_millis(90) && took < TERM_WAIT,
            "{took:?}"
        );
        // No EOF: the stop ends at TERM_WAIT.
        let (_w, r) = tokio::io::duplex(64);
        let t = Instant::now();
        stop_on(r, async {}).await;
        let took = t.elapsed();
        assert!(took >= TERM_WAIT && took < TERM_WAIT * 2, "{took:?}");
        // EOF without a signal ends it at once.
        let t = Instant::now();
        stop_on(tokio::io::empty(), std::future::pending()).await;
        assert!(t.elapsed() < Duration::from_millis(100));
    }

    const STATS_LINE: &str = "2026-11-26T10:00:00.123456Z  INFO owly_awdl::driver: STATS tx_action {s} tx_unicast 0 tx_multicast 0 tx_amsdu 0 | radio full 0 gave up {g} | rx_action 1 rx_data 0 rx_malformed 0 delivered 0 | dropped no_peer 0 queue 0 expired 0 | syn held 0 released 0 | no_overlap 0 | timer late avg 1 µs max 2 µs\n";

    fn journal(steps: &[(u64, u64)]) -> String {
        let mut text = String::from("add peer aa:bb:cc:dd:ee:ff\n");
        for (s, g) in steps {
            text.push_str(
                &STATS_LINE
                    .replace("{s}", &s.to_string())
                    .replace("{g}", &g.to_string()),
            );
        }
        text
    }

    #[tokio::test]
    async fn the_journal_watch_sees_a_wedge_and_an_end() {
        let wedged = journal(&[(0, 0), (100, 60), (200, 120), (300, 180)]);
        assert!(watch_journal(wedged.as_bytes(), "wlx1").await);
        let healthy = journal(&[(0, 0), (100, 0), (200, 1)]);
        assert!(!watch_journal(healthy.as_bytes(), "wlx1").await);
        assert!(!watch_journal(&b""[..], "wlx1").await);
    }

    #[test]
    fn every_missing_entry_of_the_probe_refuses_the_card() {
        // Owly's ids at 21171d8: "hardware" (the card or its driver), "radio" (no channel
        // the card can use, shared mode without Wi-Fi), "backend" (nl80211 cannot be read
        // for this card, hostapd missing for a card without ad-hoc mode). Each is about the
        // station radio.toml names, so another card may do.
        for id in ["hardware", "radio", "backend"] {
            let json = format!(r#"{{"mode":"exclusive","missing":[{{"id":"{id}","say":"no"}}]}}"#);
            assert_eq!(
                parse_probe(&json).unwrap().refusal.as_deref(),
                Some("no"),
                "{id}"
            );
        }
    }

    #[test]
    fn radio_toml_has_station_and_country() {
        assert_eq!(
            radio_toml("wlx00e04c123456", Some("CH")),
            "station = \"wlx00e04c123456\"\ncountry = \"CH\"\n"
        );
    }

    #[test]
    fn radio_toml_without_zone_has_no_country() {
        assert_eq!(radio_toml("wlan1", None), "station = \"wlan1\"\n");
    }

    #[tokio::test]
    async fn radio_toml_is_written_before_the_radio_comes_up() {
        let fake = Fake {
            channel: Some(44),
            ..Default::default()
        };
        let toml = tmp("written");
        let (code, _) = run(&fake, &free_card(), &toml, async {}).await;
        assert_eq!(code, 0);
        assert_eq!(
            std::fs::read_to_string(&toml).unwrap(),
            "station = \"wlx00e04c123456\"\ncountry = \"CH\"\n"
        );
    }

    #[test]
    fn card_facts_come_from_sysfs() {
        use std::os::unix::fs::symlink;
        let t = tmp("sysfs");
        let root = t.dir.join("sys");
        let usb = root.join("devices/usb1/1-1/1-1:1.0");
        let pci = root.join("devices/pci0000:00/0000:02:00.0");
        for d in [&usb, &pci] {
            std::fs::create_dir_all(d).unwrap();
        }
        symlink("../../../../bus/usb", usb.join("subsystem")).unwrap();
        symlink(
            "../../../../bus/usb/drivers/rtw88_8822bu",
            usb.join("driver"),
        )
        .unwrap();
        symlink("../../../bus/pci", pci.join("subsystem")).unwrap();
        symlink("../../../bus/pci/drivers/iwlwifi", pci.join("driver")).unwrap();
        std::fs::write(pci.join("vendor"), "0x8086\n").unwrap();
        std::fs::write(pci.join("device"), "0x08b3\n").unwrap();
        for (iface, dev) in [("wlx1", &usb), ("wlp2s0", &pci)] {
            let n = root.join("class/net").join(iface);
            std::fs::create_dir_all(&n).unwrap();
            symlink(dev, n.join("device")).unwrap();
        }
        let net = root.join("class/net");
        assert_eq!(
            card_info(&net, "wlx1"),
            CardInfo {
                bus: Bus::Usb,
                pci_id: None,
                driver: Some("rtw88_8822bu".into()),
            }
        );
        assert_eq!(
            card_info(&net, "wlp2s0"),
            CardInfo {
                bus: Bus::Pci,
                pci_id: Some((0x8086, 0x08b3)),
                driver: Some("iwlwifi".into()),
            }
        );
        // No device link (a virtual interface): nothing known.
        assert_eq!(card_info(&net, "gone"), CardInfo::default());
    }

    #[test]
    fn route_lists_merge() {
        assert_eq!(
            merge_arrays("[{\"a\":1}]", "[{\"b\":2}]"),
            "[{\"a\":1},{\"b\":2}]"
        );
        assert_eq!(merge_arrays("[]", "garbage"), "");
    }
}
