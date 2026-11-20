//! The `[time]` setting: the zones the menu offers, the checks of a zone
//! and of an NTP server name, and the text of `generated/time`, which the
//! initramfs and `thinfloe-time apply` install as root.

use std::collections::BTreeMap;
use std::net::IpAddr;

use crate::config::TimeCfg;

/// The zones the menu offers: the third column of this file, and UTC.
pub const ZONE_TAB: &str = "/usr/share/zoneinfo/zone.tab";
/// The one zone offered that zone.tab does not list.
pub const UTC: &str = "UTC";
/// At most this many NTP servers; the root tools check the same.
pub const MAX_SERVERS: usize = 8;
const MAX_NAME: usize = 253;
const MAX_LABEL: usize = 63;

/// The zones by region, as the two selects show them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Zones {
    /// Sorted region names (`Europe`, `America`, …).
    pub regions: Vec<String>,
    /// Per region: (zone, label), sorted by label. The label is the zone
    /// without its region, `_` shown as a space.
    pub cities: BTreeMap<String, Vec<(String, String)>>,
}

impl Zones {
    /// `zone` is offered: UTC or a zone of zone.tab.
    pub fn contains(&self, zone: &str) -> bool {
        zone == UTC || self.region_of(zone).is_some()
    }

    /// The region and the city index of `zone`.
    pub fn region_of(&self, zone: &str) -> Option<(usize, usize)> {
        let (region, _) = zone.split_once('/')?;
        let r = self.regions.iter().position(|x| x == region)?;
        let c = self.cities.get(region)?.iter().position(|(z, _)| z == zone)?;
        Some((r, c))
    }
}

/// The zones of a zone.tab text; lines that are not a zone are skipped.
pub fn parse_zone_tab(text: &str) -> Zones {
    let mut cities: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let Some(zone) = line.split('\t').nth(2) else {
            continue;
        };
        let Some((region, city)) = zone.split_once('/') else {
            continue;
        };
        let plain = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_-+/".contains(c));
        if !plain(region) || !plain(city) || region.contains('/') || zone.contains("..") {
            continue;
        }
        let list = cities.entry(region.to_string()).or_default();
        if !list.iter().any(|(z, _)| z == zone) {
            list.push((zone.to_string(), city.replace('_', " ")));
        }
    }
    for list in cities.values_mut() {
        list.sort_by(|a, b| a.1.cmp(&b.1));
    }
    Zones {
        regions: cities.keys().cloned().collect(),
        cities,
    }
}

/// The zones of `ZONE_TAB`; none when it cannot be read.
pub fn read_zones() -> Zones {
    std::fs::read_to_string(ZONE_TAB)
        .map(|t| parse_zone_tab(&t))
        .unwrap_or_default()
}

fn host_name_ok(s: &str) -> bool {
    s.len() <= MAX_NAME
        && s.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= MAX_LABEL
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// An NTP server: an IPv4 or IPv6 address, or a host name.
pub fn check_server(s: &str) -> Result<(), String> {
    if s.parse::<IpAddr>().is_ok() || host_name_ok(s) {
        Ok(())
    } else {
        let shown: String = s.chars().take(64).collect();
        Err(format!("Not used: {shown} is not a host name or address."))
    }
}

/// The NTP servers field: names apart by commas or spaces.
pub fn parse_servers(text: &str) -> Result<Vec<String>, String> {
    let servers: Vec<String> = text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    for s in &servers {
        check_server(s)?;
    }
    if servers.len() > MAX_SERVERS {
        return Err(format!("At most {MAX_SERVERS} servers."));
    }
    Ok(servers)
}

/// The NTP servers as the field shows them.
pub fn servers_text(servers: &[String]) -> String {
    servers.join(", ")
}

/// The body of `generated/time` without its header; `None` when both are
/// automatic and the file must be absent.
pub fn body(cfg: &TimeCfg) -> Option<String> {
    if cfg.zone.is_none() && cfg.ntp.is_empty() {
        return None;
    }
    let mut out = String::new();
    if let Some(z) = &cfg.zone {
        out.push_str(&format!("zone={z}\n"));
    }
    for s in &cfg.ntp {
        out.push_str(&format!("ntp={s}\n"));
    }
    Some(out)
}

/// The setting a `generated/time` body holds; unknown lines are skipped.
pub fn from_body(text: &str) -> TimeCfg {
    let mut cfg = TimeCfg::default();
    for line in text.lines() {
        if let Some(z) = line.strip_prefix("zone=") {
            cfg.zone = Some(z.to_string());
        } else if let Some(s) = line.strip_prefix("ntp=") {
            cfg.ntp.push(s.to_string());
        }
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAB: &str = "# comment\n\
CH\t+4723+00832\tEurope/Zurich\n\
AR\t-3436-05827\tAmerica/Argentina/Buenos_Aires\tBuenos Aires (BA, CF)\n\
US\t+404251-0740023\tAmerica/New_York\tEastern (most areas)\n\
XX\t+0\t../etc/passwd\n\
XX\t+0\tnoslash\n";

    #[test]
    fn zone_tab_gives_regions_and_city_labels() {
        let z = parse_zone_tab(TAB);
        assert_eq!(z.regions, ["America", "Europe"]);
        assert_eq!(
            z.cities["America"],
            [
                (
                    "America/Argentina/Buenos_Aires".to_string(),
                    "Argentina/Buenos Aires".to_string()
                ),
                ("America/New_York".to_string(), "New York".to_string()),
            ]
        );
        assert!(z.contains("UTC"));
        assert!(z.contains("Europe/Zurich"));
        assert!(!z.contains("../etc/passwd"));
        assert!(!z.contains("Europe/Nowhere"));
        assert!(!z.contains("noslash"));
        assert_eq!(z.region_of("Europe/Zurich"), Some((1, 0)));
    }

    #[test]
    fn servers_good_and_hostile() {
        for ok in ["ntp.example.ch", "10.1.2.3", "fd00::1", "a-b.c", "time"] {
            assert!(check_server(ok).is_ok(), "{ok}");
        }
        let long_label = format!("{}.ch", "a".repeat(64));
        let long_name = ["a".repeat(63).as_str(); 4].join(".") + ".ab";
        assert!(long_name.len() > 253);
        for bad in [
            "-x",
            "a..b",
            "a.",
            ".a",
            "ex ample",
            "server x",
            "a;b",
            "x-.ch",
            "",
            long_label.as_str(),
            long_name.as_str(),
        ] {
            assert!(check_server(bad).is_err(), "{bad}");
        }
        assert_eq!(
            check_server("a;b").unwrap_err(),
            "Not used: a;b is not a host name or address."
        );
    }

    #[test]
    fn field_text_splits_on_commas_and_spaces() {
        assert_eq!(
            parse_servers(" a.ch, 10.0.0.1  b.ch,").unwrap(),
            ["a.ch", "10.0.0.1", "b.ch"]
        );
        assert_eq!(parse_servers("").unwrap(), Vec::<String>::new());
        assert!(parse_servers("a.ch, a;b").is_err());
    }

    #[test]
    fn nine_servers_are_refused() {
        let text = (1..=9).map(|i| format!("10.0.0.{i}")).collect::<Vec<_>>().join(" ");
        assert_eq!(parse_servers(&text).unwrap_err(), "At most 8 servers.");
        let eight = (1..=8).map(|i| format!("10.0.0.{i}")).collect::<Vec<_>>().join(" ");
        assert_eq!(parse_servers(&eight).unwrap().len(), 8);
    }

    #[test]
    fn body_round_trip() {
        let cfg = TimeCfg {
            zone: Some("Europe/Zurich".into()),
            ntp: vec!["a.example".into(), "10.0.0.1".into()],
        };
        let text = body(&cfg).unwrap();
        assert_eq!(text, "zone=Europe/Zurich\nntp=a.example\nntp=10.0.0.1\n");
        assert_eq!(from_body(&text), cfg);
    }

    #[test]
    fn automatic_has_no_file() {
        assert_eq!(body(&TimeCfg::default()), None);
        let only_ntp = TimeCfg {
            zone: None,
            ntp: vec!["a.ch".into()],
        };
        assert_eq!(body(&only_ntp).unwrap(), "ntp=a.ch\n");
    }
}
