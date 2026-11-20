//! The fixed facts of the Network tab's "Connections now" band: per
//! interface the MAC, the gateway of the default route and, for WiFi, the
//! chip and its driver; and the DNS servers in use. Read in the
//! `Job::NetStatus` job thread from files only (sysfs, procfs, resolv.conf,
//! `pci.ids`); no program is started.

use crate::bounded;
use crate::netstat::{self, Roots};
use crate::nettab::{IfKind, Iface};
use std::fs;
use std::net::Ipv4Addr;
use std::path::Path;

/// The most a sysfs attribute file may hold here; they are a few bytes.
const ATTR_MAX: u64 = 256;
/// Longest MAC kept: an InfiniBand address, 20 bytes as hex pairs.
const MAC_MAX: usize = 59;
/// Longest driver name kept.
const DRIVER_MAX: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IfDetail {
    pub name: String,
    pub mac: Option<String>,
    pub gateway: Option<Ipv4Addr>,
    pub chip: Option<String>,
    pub driver: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Details {
    pub ifaces: Vec<IfDetail>,
    pub dns_in_use: Vec<String>,
}

/// The details of each interface of `ifaces`, in their order, and the DNS
/// servers in use.
pub fn read(roots: &Roots, ifaces: &[Iface], pci_ids: &Path) -> Details {
    let route = netstat::route(roots);
    let ifaces = ifaces
        .iter()
        .map(|i| {
            let mut d = IfDetail {
                name: i.name.clone(),
                ..IfDetail::default()
            };
            // The name comes from the network tool; one the kernel could
            // not have made is never joined to a path.
            if !netstat::usable_name(&i.name) {
                return d;
            }
            let dir = roots.sys.join("class/net").join(&i.name);
            d.mac = attr(&dir.join("address")).and_then(|a| mac(&a));
            d.gateway = route.as_ref().filter(|r| r.iface == i.name).and_then(|r| r.gateway);
            if i.kind == IfKind::Wifi {
                let dev = dir.join("device");
                d.driver = driver(&dev);
                d.chip = chip(&dev, pci_ids);
            }
            d
        })
        .collect();
    Details {
        ifaces,
        dns_in_use: netstat::dns_in_use(roots),
    }
}

fn attr(path: &Path) -> Option<String> {
    bounded::read_attr(path, ATTR_MAX).map(|s| s.trim().to_string())
}

/// The name of the kernel driver bound to the sysfs device `dev`: the
/// last part of its `driver` link. The Screen tab's graphics band reads it
/// this way too.
pub(crate) fn driver(dev: &Path) -> Option<String> {
    let link = fs::read_link(dev.join("driver")).ok()?;
    link.file_name()
        .map(|n| n.to_string_lossy().chars().take(DRIVER_MAX).collect())
}

/// A hex id as sysfs writes it: `0x8086` for PCI, `0bda` for USB. The
/// Screen tab's graphics band reads the GPU's ids with it too.
pub(crate) fn hex_id(path: &Path) -> Option<u16> {
    let s = attr(path)?;
    u16::from_str_radix(s.strip_prefix("0x").unwrap_or(&s), 16).ok()
}

/// The address as the kernel prints it; an all-zero one (a tunnel, a
/// device that has none) and anything that is not hex pairs is dropped.
fn mac(a: &str) -> Option<String> {
    let shaped = !a.is_empty() && a.len() <= MAC_MAX && a.bytes().all(|b| b.is_ascii_hexdigit() || b == b':');
    (shaped && a.bytes().any(|b| b != b'0' && b != b':')).then(|| a.to_ascii_lowercase())
}

/// The chip behind `dev` (`/sys/class/net/<if>/device`): a PCI device has
/// `vendor` and `device`, named from `pci.ids` or shown as `8086:a0f0`; a
/// USB adapter's `device` is the USB interface, whose parent holds
/// `idVendor` and `idProduct`.
fn chip(dev: &Path, pci_ids: &Path) -> Option<String> {
    if let (Some(v), Some(d)) = (hex_id(&dev.join("vendor")), hex_id(&dev.join("device"))) {
        return Some(crate::pciids::name(pci_ids, v, d).unwrap_or_else(|| format!("{v:04x}:{d:04x}")));
    }
    let usb = dev.join("..");
    let (v, p) = (hex_id(&usb.join("idVendor"))?, hex_id(&usb.join("idProduct"))?);
    Some(format!("USB {v:04x}:{p:04x}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nettab::IfState;
    use crate::testdir::TestDir;
    use std::os::unix::fs::symlink;

    fn iface(kind: IfKind, name: &str) -> Iface {
        Iface {
            name: name.to_string(),
            kind,
            state: IfState::Connected,
            addresses: Vec::new(),
            ssid: None,
            signal_dbm: None,
            handshake_age_s: None,
        }
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// `sys/class/net/<name>` with its address, its `device` link to
    /// `sys/devices/<device>` and that device's driver link.
    fn net_dev(d: &Path, name: &str, mac: &str, device: &str, driver: &str) -> std::path::PathBuf {
        let net = d.join("sys/class/net").join(name);
        write(&net.join("address"), &format!("{mac}\n"));
        let dev = d.join("sys/devices").join(device);
        fs::create_dir_all(&dev).unwrap();
        let drv = d.join("sys/bus/drivers").join(driver);
        fs::create_dir_all(&drv).unwrap();
        symlink(&drv, dev.join("driver")).unwrap();
        symlink(&dev, net.join("device")).unwrap();
        dev
    }

    fn pci_list(d: &Path) -> std::path::PathBuf {
        let p = d.join("pci.ids");
        fs::write(&p, "8086  Intel Corporation\n\ta0f0  Wi-Fi 6 AX201\n").unwrap();
        p
    }

    #[test]
    fn a_wifi_adapter_names_its_chip_and_driver() {
        let d = TestDir::new("netdetail", "pci");
        let dev = net_dev(&d, "wlan0", "3C:52:82:AA:BB:CC", "pci0000:00/0000:00:14.3", "iwlwifi");
        write(&dev.join("vendor"), "0x8086\n");
        write(&dev.join("device"), "0xa0f0\n");
        let got = read(&Roots::under(&d), &[iface(IfKind::Wifi, "wlan0")], &pci_list(&d));
        assert_eq!(
            got.ifaces,
            vec![IfDetail {
                name: "wlan0".into(),
                mac: Some("3c:52:82:aa:bb:cc".into()),
                gateway: None,
                chip: Some("Intel Corporation Wi-Fi 6 AX201".into()),
                driver: Some("iwlwifi".into()),
            }]
        );
        // Not in the list, or no list: the ids.
        write(&dev.join("device"), "0x2723\n");
        let got = read(&Roots::under(&d), &[iface(IfKind::Wifi, "wlan0")], &pci_list(&d));
        assert_eq!(got.ifaces[0].chip.as_deref(), Some("Intel Corporation"));
        write(&dev.join("vendor"), "0x14c3\n");
        let got = read(&Roots::under(&d), &[iface(IfKind::Wifi, "wlan0")], &d.join("missing"));
        assert_eq!(got.ifaces[0].chip.as_deref(), Some("14c3:2723"));
    }

    /// The owner's thin client (4.6.0); there every attribute reports
    /// 4096 bytes, and the MAC and the chip were missing.
    #[test]
    fn the_thin_clients_adapters() {
        let d = TestDir::new("netdetail", "thinclient");
        let mut list = Vec::new();
        let mut want = Vec::new();
        for line in include_str!("testdata/thinclient-drm/net.tsv").lines() {
            if line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            let rel = f[1].strip_prefix("devices/").unwrap();
            let dev = net_dev(&d, f[0], f[2], rel, f[5]);
            write(&dev.join("vendor"), &format!("{}\n", f[3]));
            write(&dev.join("device"), &format!("{}\n", f[4]));
            let kind = if f[0].starts_with('w') {
                IfKind::Wifi
            } else {
                IfKind::Wired
            };
            list.push(iface(kind, f[0]));
            want.push((f[0].to_string(), f[2].to_string()));
        }
        let pci = d.join("pci.ids");
        fs::write(&pci, "8086  Intel Corporation\n\t08b3  Wireless 3160\n").unwrap();
        let got = read(&Roots::under(&d), &list, &pci);
        let macs: Vec<(String, String)> = got
            .ifaces
            .iter()
            .map(|i| (i.name.clone(), i.mac.clone().unwrap_or_default()))
            .collect();
        assert_eq!(macs, want);
        let wifi = got.ifaces.iter().find(|i| i.name == "wlp2s0").unwrap();
        assert_eq!(wifi.chip.as_deref(), Some("Intel Corporation Wireless 3160"));
        assert_eq!(wifi.driver.as_deref(), Some("iwlwifi"));
    }

    /// The real `/sys` of the test host: an interface with a hardware
    /// address shows it, although sysfs reports 4096 bytes for the file.
    #[test]
    fn a_mac_is_read_from_this_hosts_sysfs() {
        let Ok(dir) = fs::read_dir("/sys/class/net") else {
            return;
        };
        let names: Vec<String> = dir
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "lo")
            .collect();
        let Some(name) = names
            .into_iter()
            .find(|n| fs::read_to_string(format!("/sys/class/net/{n}/address")).is_ok_and(|a| mac(a.trim()).is_some()))
        else {
            return;
        };
        let got = read(
            &Roots::under(Path::new("/")),
            &[iface(IfKind::Wired, &name)],
            Path::new("/nonexistent"),
        );
        assert!(got.ifaces[0].mac.is_some(), "{name}: {got:?}");
    }

    #[test]
    fn a_usb_adapter_shows_its_ids() {
        let d = TestDir::new("netdetail", "usb");
        net_dev(&d, "wlx0", "00:e0:4c:01:02:03", "usb1/1-2/1-2:1.0", "rtl8xxxu");
        write(&d.join("sys/devices/usb1/1-2/idVendor"), "0bda\n");
        write(&d.join("sys/devices/usb1/1-2/idProduct"), "8179\n");
        let got = read(&Roots::under(&d), &[iface(IfKind::Wifi, "wlx0")], &pci_list(&d));
        assert_eq!(got.ifaces[0].chip.as_deref(), Some("USB 0bda:8179"));
        assert_eq!(got.ifaces[0].driver.as_deref(), Some("rtl8xxxu"));
        assert_eq!(got.ifaces[0].mac.as_deref(), Some("00:e0:4c:01:02:03"));
    }

    #[test]
    fn the_gateway_belongs_to_the_default_route_interface() {
        let d = TestDir::new("netdetail", "route");
        write(
            &d.join("proc/net/route"),
            "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
             eno1\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
             eno1\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n",
        );
        let got = read(
            &Roots::under(&d),
            &[iface(IfKind::Wired, "eno1"), iface(IfKind::Wifi, "wlan0")],
            &d.join("missing"),
        );
        let gw = Ipv4Addr::from(u32::from_str_radix("0101A8C0", 16).unwrap().to_ne_bytes());
        assert_eq!(got.ifaces[0].gateway, Some(gw));
        assert_eq!(got.ifaces[1].gateway, None);
        assert_eq!(got.ifaces[1].name, "wlan0");
    }

    #[test]
    fn a_wired_interface_has_no_chip_line() {
        let d = TestDir::new("netdetail", "wired");
        let dev = net_dev(&d, "eno1", "3c:52:82:00:00:01", "pci0000:00/0000:00:1f.6", "e1000e");
        write(&dev.join("vendor"), "0x8086\n");
        write(&dev.join("device"), "0xa0f0\n");
        net_dev(&d, "wg0", "00:00:00:00:00:00", "virtual/wg0", "none");
        let got = read(
            &Roots::under(&d),
            &[
                iface(IfKind::Wired, "eno1"),
                iface(IfKind::Wireguard, "wg0"),
                iface(IfKind::Wired, "../x"),
            ],
            &pci_list(&d),
        );
        assert_eq!(got.ifaces[0].mac.as_deref(), Some("3c:52:82:00:00:01"));
        assert_eq!((&got.ifaces[0].chip, &got.ifaces[0].driver), (&None, &None));
        assert_eq!(got.ifaces[1].mac, None, "an all-zero MAC is dropped");
        assert_eq!(
            got.ifaces[2],
            IfDetail {
                name: "../x".into(),
                ..IfDetail::default()
            }
        );
    }
}
