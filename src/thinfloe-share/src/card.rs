//! Choice of the Wi-Fi card FloeDrop may take over, and the regulatory country.

use serde_json::Value;

pub const NO_STATE: &str = "Cannot read the network state.";
pub const NO_CARD: &str = "AirDrop needs a Wi-Fi card this machine does not use for its network.";

/// What the choice looks at; the helper fills it from `ip -j addr`, `ip -j route`, the
/// netplan Wi-Fi files and /sys. A card matched by MAC in the netplan file is found through the
/// `address` of its entry in `ip -j addr`.
#[derive(Debug, Clone, Default)]
pub struct CardInputs {
    pub ip_addr_json: String,
    pub ip_route_json: String,
    pub wifi_yaml: Option<String>,
    pub wifi_ifaces: Vec<String>,
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

/// The first Wi-Fi interface (sorted) with no address other than IPv6 link-local, no route
/// other than IPv6 link-local and multicast, and no network configured. Unreadable input is an
/// error: the card is not taken on a guess.
pub fn choose_station(i: &CardInputs) -> Result<String, &'static str> {
    let (Ok(addrs), Ok(routes)) = (
        serde_json::from_str::<Vec<Value>>(&i.ip_addr_json),
        serde_json::from_str::<Vec<Value>>(&i.ip_route_json),
    ) else {
        return Err(NO_STATE);
    };
    let mut ifaces = i.wifi_ifaces.clone();
    ifaces.sort();
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
        if !has_addr && !has_route && !named {
            return Ok(iface);
        }
    }
    Err(NO_CARD)
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
        }
    }

    // Fixture: wlp2s0 is connected (address and route), wlx00e04c123456 is a free stick,
    // wlp3s0 is down without address, wlan9 is a card matched by MAC in the fixture.

    #[test]
    fn a_connected_card_is_not_taken() {
        let i = inputs(&["wlp2s0", "wlx00e04c123456"], None);
        assert_eq!(choose_station(&i).unwrap(), "wlx00e04c123456");
        let only = inputs(&["wlp2s0"], None);
        assert_eq!(choose_station(&only), Err(NO_CARD));
    }

    #[test]
    fn a_configured_card_is_never_taken() {
        // wlp3s0 has no address and no route (out of range) but wifi.yaml names it.
        let i = inputs(&["wlp3s0"], Some(WIFI));
        assert_eq!(choose_station(&i), Err(NO_CARD));
        // Without the file it would be free.
        assert_eq!(
            choose_station(&inputs(&["wlp3s0"], None)).unwrap(),
            "wlp3s0"
        );
        // Matched by MAC and by name pattern.
        let by_mac = "network:\n  wifis:\n    home:\n      match:\n        macaddress: \"AA:BB:CC:00:00:09\"\n";
        assert_eq!(
            choose_station(&inputs(&["wlan9"], Some(by_mac))),
            Err(NO_CARD)
        );
        let by_name = "network:\n  wifis:\n    home:\n      match:\n        name: wlp*\n";
        assert_eq!(
            choose_station(&inputs(&["wlp3s0"], Some(by_name))),
            Err(NO_CARD)
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
        assert_eq!(choose_station(&j), Err(NO_CARD));
    }

    #[test]
    fn unreadable_state_is_an_error_not_a_free_card() {
        let mut i = inputs(&["wlx00e04c123456"], None);
        i.ip_addr_json = "not json".into();
        assert_eq!(choose_station(&i), Err(NO_STATE));
        let mut i = inputs(&["wlx00e04c123456"], None);
        i.ip_route_json = String::new();
        assert_eq!(choose_station(&i), Err(NO_STATE));
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
            Err("AirDrop needs a Wi-Fi card this machine does not use for its network.")
        );
        assert_eq!(
            choose_station(&inputs(&["wlp2s0", "wlp3s0"], Some(WIFI))),
            Err(NO_CARD)
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
