//! Choice of the Wi-Fi card FloeDrop may take over, and the regulatory country.

use serde_json::Value;

pub const NO_STATE: &str = "Cannot read the network state.";
pub const NO_CARD: &str = "AirDrop needs a Wi-Fi card this machine does not use for its network.";

/// Cards that Owly's probe accepts but that fail in AWDL mode, by PCI vendor and device.
/// Intel Wireless 3160: on thinfloe-56db68 (2026-11-25 and 2026-11-26) the probe said
/// "exclusive mode up", then the firmware crashed about every 11 s ("Microcode SW error
/// detected", 40 times in one boot) and iPhones never saw the receiver.
pub const AWDL_BROKEN: &[(u16, u16, &str)] = &[
    (0x8086, 0x08b3, "Intel Wireless 3160"),
    (0x8086, 0x08b4, "Intel Wireless 3160"),
];

/// The bus a card sits on, from `/sys/class/net/<if>/device/subsystem`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Bus {
    Usb,
    Pci,
    #[default]
    Other,
}

/// What sysfs tells about one Wi-Fi interface's card; unknown parts stay at the default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardInfo {
    pub bus: Bus,
    /// PCI vendor and device; only for a PCI card.
    pub pci_id: Option<(u16, u16)>,
    /// The name of the bound driver, e.g. `rtw88_8822bu`.
    pub driver: Option<String>,
}

/// The model of a card in [`AWDL_BROKEN`].
pub fn broken_model(info: &CardInfo) -> Option<&'static str> {
    let id = info.pci_id?;
    AWDL_BROKEN
        .iter()
        .find(|(v, d, _)| (*v, *d) == id)
        .map(|(_, _, m)| *m)
}

/// The error for an unused card that cannot run AirDrop.
pub fn broken_text(iface: &str, model: &str) -> String {
    format!(
        "The Wi-Fi card {iface} ({model}) cannot run AirDrop; plug in a USB Wi-Fi stick that can."
    )
}

/// What the choice looks at; the helper fills it from `ip -j addr`, `ip -j route`, the
/// netplan Wi-Fi files and /sys. A card matched by MAC in the netplan file is found through the
/// `address` of its entry in `ip -j addr`.
#[derive(Debug, Clone, Default)]
pub struct CardInputs {
    pub ip_addr_json: String,
    pub ip_route_json: String,
    pub wifi_yaml: Option<String>,
    pub wifi_ifaces: Vec<String>,
    /// Bus, PCI ID and driver per interface of `wifi_ifaces`; a missing entry is unknown.
    pub cards: std::collections::HashMap<String, CardInfo>,
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches(|c| c == '"' || c == '\'')
}

/// `*` matches any run of characters, as netplan's `match: name:` does.
fn glob(pat: &str, s: &str) -> bool {
    match pat.split_once('*') {
        None => pat == s,
        Some((head, rest)) => {
            let Some(mut tail) = s.strip_prefix(head) else {
                return false;
            };
            if rest.is_empty() {
                return true;
            }
            loop {
                if glob(rest, tail) {
                    return true;
                }
                let mut it = tail.chars();
                if it.next().is_none() {
                    return false;
                }
                tail = it.as_str();
            }
        }
    }
}

/// Is the card named in the netplan text, as an interface key, a `match: name:` or a `match: macaddress:`?
fn configured(yaml: &str, iface: &str, mac: Option<&str>) -> bool {
    yaml.lines().any(|l| {
        let l = l.split('#').next().unwrap_or("").trim();
        let Some((k, v)) = l.split_once(':') else {
            return false;
        };
        let (k, v) = (unquote(k), unquote(v));
        if v.is_empty() {
            return k == iface;
        }
        match k {
            "name" => glob(v, iface),
            "macaddress" => mac.is_some_and(|m| m.eq_ignore_ascii_case(v)),
            _ => false,
        }
    })
}

/// A Wi-Fi interface with no address other than IPv6 link-local, no route other than IPv6
/// link-local and multicast, and no network configured, and not in [`AWDL_BROKEN`]: a USB card
/// first, then by sorted name. When the only unused cards are known to fail, the error names the
/// first of them. Unreadable network state is an error: the card is not taken on a guess.
pub fn choose_station(i: &CardInputs) -> Result<String, String> {
    let (Ok(addrs), Ok(routes)) = (
        serde_json::from_str::<Vec<Value>>(&i.ip_addr_json),
        serde_json::from_str::<Vec<Value>>(&i.ip_route_json),
    ) else {
        return Err(NO_STATE.into());
    };
    let mut ifaces = i.wifi_ifaces.clone();
    ifaces.sort();
    let info = |iface: &str| i.cards.get(iface).cloned().unwrap_or_default();
    // Stable: equal buses keep the sorted-name order.
    ifaces.sort_by_key(|iface| info(iface).bus != Bus::Usb);
    let mut broken: Option<String> = None;
    for iface in ifaces {
        let entry = addrs.iter().find(|a| a["ifname"] == iface.as_str());
        let has_addr = entry
            .and_then(|a| a["addr_info"].as_array())
            .is_some_and(|l| {
                l.iter()
                    .any(|a| !(a["family"] == "inet6" && a["scope"] == "link"))
            });
        let has_route = routes.iter().any(|r| {
            r["dev"] == iface.as_str()
                && !matches!(r["dst"].as_str(), Some("fe80::/64" | "ff00::/8"))
        });
        let mac = entry.and_then(|a| a["address"].as_str());
        let named = i
            .wifi_yaml
            .as_deref()
            .is_some_and(|y| configured(y, &iface, mac));
        if has_addr || has_route || named {
            continue;
        }
        match broken_model(&info(&iface)) {
            Some(model) => {
                broken.get_or_insert_with(|| broken_text(&iface, model));
            }
            None => return Ok(iface),
        }
    }
    Err(broken.unwrap_or_else(|| NO_CARD.into()))
}

/// The ISO country of `zone`: the first code of its line in `zone1970.tab`.
pub fn country(zone: &str, zone1970: &str) -> Option<String> {
    zone1970.lines().find_map(|l| {
        if l.starts_with('#') {
            return None;
        }
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 3 || f[2] != zone {
            return None;
        }
        f[0].split(',')
            .next()
            .filter(|c| !c.is_empty())
            .map(String::from)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &str = include_str!("../testdata/ip-addr.json");
    const ROUTE: &str = include_str!("../testdata/ip-route.json");
    const WIFI: &str = include_str!("../testdata/wifi.yaml");
    const ZONES: &str = include_str!("../testdata/zone1970.tab");

    fn inputs(ifaces: &[&str], yaml: Option<&str>) -> CardInputs {
        CardInputs {
            ip_addr_json: ADDR.into(),
            ip_route_json: ROUTE.into(),
            wifi_yaml: yaml.map(String::from),
            wifi_ifaces: ifaces.iter().map(|s| s.to_string()).collect(),
            cards: Default::default(),
        }
    }

    fn pci(vendor: u16, device: u16) -> CardInfo {
        CardInfo {
            bus: Bus::Pci,
            pci_id: Some((vendor, device)),
            driver: Some("iwlwifi".into()),
        }
    }

    fn usb() -> CardInfo {
        CardInfo {
            bus: Bus::Usb,
            pci_id: None,
            driver: Some("rtw88_8822bu".into()),
        }
    }

    fn with(mut i: CardInputs, cards: &[(&str, CardInfo)]) -> CardInputs {
        for (n, c) in cards {
            i.cards.insert(n.to_string(), c.clone());
        }
        i
    }

    #[test]
    fn the_intel_3160_is_skipped_for_the_stick() {
        // thinfloe-56db68: the 3160 sorts first and was taken by 4.12.0.
        let i = with(
            inputs(&["wlp3s0", "wlx00e04c123456"], None),
            &[("wlp3s0", pci(0x8086, 0x08b3)), ("wlx00e04c123456", usb())],
        );
        assert_eq!(choose_station(&i).unwrap(), "wlx00e04c123456");
    }

    #[test]
    fn a_3160_alone_is_named_in_the_error() {
        for dev in [0x08b3, 0x08b4] {
            let i = with(inputs(&["wlp3s0"], None), &[("wlp3s0", pci(0x8086, dev))]);
            assert_eq!(
                choose_station(&i),
                Err("The Wi-Fi card wlp3s0 (Intel Wireless 3160) cannot run AirDrop; plug in a USB Wi-Fi stick that can.".into())
            );
        }
        // A 3160 the machine uses for its network is not the reason: the plain text.
        let i = with(
            inputs(&["wlp2s0"], None),
            &[("wlp2s0", pci(0x8086, 0x08b3))],
        );
        assert_eq!(choose_station(&i), Err(NO_CARD.into()));
    }

    #[test]
    fn other_intel_cards_stay_usable() {
        let i = with(
            inputs(&["wlp3s0"], None),
            &[("wlp3s0", pci(0x8086, 0x2723))],
        );
        assert_eq!(choose_station(&i).unwrap(), "wlp3s0");
        // Beside a 3160, too.
        let i = with(
            inputs(&["wlp3s0", "wlx00e04c654321"], None),
            &[
                ("wlp3s0", pci(0x8086, 0x08b3)),
                ("wlx00e04c654321", pci(0x8086, 0x2723)),
            ],
        );
        assert_eq!(choose_station(&i).unwrap(), "wlx00e04c654321");
    }

    #[test]
    fn a_usb_card_is_preferred() {
        let i = with(
            inputs(&["wlp3s0", "wlx00e04c123456"], None),
            &[("wlp3s0", pci(0x8086, 0x2723)), ("wlx00e04c123456", usb())],
        );
        assert_eq!(choose_station(&i).unwrap(), "wlx00e04c123456");
        // Among equals the sorted name decides.
        let i = with(
            inputs(&["wlx00e04c654321", "wlx00e04c123456"], None),
            &[("wlx00e04c123456", usb()), ("wlx00e04c654321", usb())],
        );
        assert_eq!(choose_station(&i).unwrap(), "wlx00e04c123456");
        // A USB card in use is not taken.
        let i = with(
            inputs(&["wlp2s0", "wlp3s0"], None),
            &[("wlp2s0", usb()), ("wlp3s0", pci(0x8086, 0x2723))],
        );
        assert_eq!(choose_station(&i).unwrap(), "wlp3s0");
    }

    #[test]
    fn a_card_without_sysfs_facts_is_still_taken() {
        assert_eq!(
            choose_station(&inputs(&["wlp3s0"], None)).unwrap(),
            "wlp3s0"
        );
    }

    // Fixture: wlp2s0 is connected (address and route), wlx00e04c123456 is a free stick,
    // wlp3s0 is down without address, wlan9 is a card matched by MAC in the fixture.

    #[test]
    fn a_connected_card_is_not_taken() {
        let i = inputs(&["wlp2s0", "wlx00e04c123456"], None);
        assert_eq!(choose_station(&i).unwrap(), "wlx00e04c123456");
        let only = inputs(&["wlp2s0"], None);
        assert_eq!(choose_station(&only), Err(NO_CARD.into()));
    }

    #[test]
    fn a_configured_card_is_never_taken() {
        // wlp3s0 has no address and no route (out of range) but wifi.yaml names it.
        let i = inputs(&["wlp3s0"], Some(WIFI));
        assert_eq!(choose_station(&i), Err(NO_CARD.into()));
        // Without the file it would be free.
        assert_eq!(
            choose_station(&inputs(&["wlp3s0"], None)).unwrap(),
            "wlp3s0"
        );
        // Matched by MAC and by name pattern.
        let by_mac = "network:\n  wifis:\n    home:\n      match:\n        macaddress: \"AA:BB:CC:00:00:09\"\n";
        assert_eq!(
            choose_station(&inputs(&["wlan9"], Some(by_mac))),
            Err(NO_CARD.into())
        );
        let by_name = "network:\n  wifis:\n    home:\n      match:\n        name: wlp*\n";
        assert_eq!(
            choose_station(&inputs(&["wlp3s0"], Some(by_name))),
            Err(NO_CARD.into())
        );
    }

    #[test]
    fn own_up_card_with_only_link_local_is_taken() {
        let i = inputs(&["wlx00e04c654321"], None);
        assert_eq!(choose_station(&i).unwrap(), "wlx00e04c654321");
        // An IPv4 link-local address still counts.
        let mut j = inputs(&["wlx00e04c654321"], None);
        j.ip_addr_json = j.ip_addr_json.replace(
            "\"family\":\"inet6\",\"local\":\"fe80::2e0:4cff:fe65:4321\",\"prefixlen\":64",
            "\"family\":\"inet\",\"local\":\"169.254.3.4\",\"prefixlen\":16",
        );
        assert_eq!(choose_station(&j), Err(NO_CARD.into()));
    }

    #[test]
    fn unreadable_state_is_an_error_not_a_free_card() {
        let mut i = inputs(&["wlx00e04c123456"], None);
        i.ip_addr_json = "not json".into();
        assert_eq!(choose_station(&i), Err(NO_STATE.into()));
        let mut i = inputs(&["wlx00e04c123456"], None);
        i.ip_route_json = String::new();
        assert_eq!(choose_station(&i), Err(NO_STATE.into()));
    }

    #[test]
    fn the_stick_beside_a_configured_card_is_taken() {
        let i = inputs(&["wlp3s0", "wlx00e04c123456"], Some(WIFI));
        assert_eq!(choose_station(&i).unwrap(), "wlx00e04c123456");
    }

    #[test]
    fn no_free_card_is_the_error_text() {
        assert_eq!(
            choose_station(&inputs(&[], None)),
            Err("AirDrop needs a Wi-Fi card this machine does not use for its network.".into())
        );
        assert_eq!(
            choose_station(&inputs(&["wlp2s0", "wlp3s0"], Some(WIFI))),
            Err(NO_CARD.into())
        );
    }

    #[test]
    fn country_of_europe_zurich_is_ch() {
        assert_eq!(country("Europe/Zurich", ZONES).as_deref(), Some("CH"));
        assert_eq!(country("Asia/Tokyo", ZONES).as_deref(), Some("JP"));
    }

    #[test]
    fn utc_has_no_country() {
        assert_eq!(country("UTC", ZONES), None);
        assert_eq!(country("", ZONES), None);
    }
}
