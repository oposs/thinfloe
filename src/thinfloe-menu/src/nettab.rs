//! The Network tab's data: what `thinfloe-network` prints, the checks of the
//! wired address form, and the text lines. The tool's JSON is untrusted
//! input as far as the menu cares: rows are capped, text is cut.

use crate::netfiles::{self, FileState, WifiFile, Wired};
use serde_json::Value;
use std::net::Ipv4Addr;

/// More rows than this are refused as a whole.
pub const ROWS_MAX: usize = 64;
/// Longest text kept from a field.
const TEXT_MAX: usize = 64;
/// Longest SSID in bytes.
const SSID_MAX: usize = 32;
/// Most addresses kept per interface.
const ADDRESSES_MAX: usize = 16;
/// Longest reason kept from a failed `apply`.
const REASON_MAX: usize = 300;
/// A WireGuard peer counts as connected after a handshake this recent
/// (seconds). The tool decides; the menu only shows the age.
pub const HANDSHAKE_FRESH_S: u64 = 180;
/// Signal strength, in dBm, from which a network is called strong or good.
const STRONG_DBM: i32 = -60;
const GOOD_DBM: i32 = -70;
/// A signal the tool did not give.
const NO_SIGNAL_DBM: i32 = -100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfKind {
    Wired,
    Wifi,
    Wireguard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfState {
    Connected,
    NoCable,
    NotConnected,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    pub kind: IfKind,
    pub state: IfState,
    pub addresses: Vec<String>,
    pub ssid: Option<String>,
    pub signal_dbm: Option<i32>,
    pub handshake_age_s: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InRange {
    pub ssid: String,
    pub signal_dbm: i32,
    pub secured: bool,
    pub supported: bool,
    /// False for a name the menu refuses (cut, or `netfiles::check_ssid`
    /// fails); `supported` is then false too.
    pub name_ok: bool,
}

impl InRange {
    /// Why the menu cannot add this network, or None when it can
    /// (Ruling 21: the tool's JSON says only `supported`).
    pub fn unsupported_reason(&self) -> Option<&'static str> {
        if self.supported {
            None
        } else if !self.name_ok {
            Some("This name cannot be used.")
        } else {
            Some("This security type is not supported")
        }
    }
}

fn cut(s: &str) -> String {
    s.chars().take(TEXT_MAX).collect()
}

/// The longest start of `s` that fits `max` bytes, ended on a char boundary.
fn cut_bytes(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn dbm(v: &Value, key: &str) -> Option<i32> {
    v.get(key)?.as_i64().and_then(|n| i32::try_from(n).ok())
}

fn rows(json: &str, key: &str, what: &str) -> Result<Vec<Value>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("{what}: {e}"))?;
    let Some(Value::Array(rows)) = v.get(key) else {
        return Err(format!("{what}: no \"{key}\" list"));
    };
    if rows.len() > ROWS_MAX {
        return Err(format!("{what}: more than {ROWS_MAX} entries"));
    }
    Ok(rows.clone())
}

fn iface(v: &Value) -> Option<Iface> {
    let name = v.get("name")?.as_str().filter(|s| !s.is_empty())?;
    let kind = match v.get("kind")?.as_str()? {
        "wired" => IfKind::Wired,
        "wifi" => IfKind::Wifi,
        "wireguard" => IfKind::Wireguard,
        _ => return None,
    };
    let state = match v.get("state")?.as_str()? {
        "connected" => IfState::Connected,
        "no-cable" => IfState::NoCable,
        "not-connected" => IfState::NotConnected,
        "off" => IfState::Off,
        _ => return None,
    };
    let addresses = match v.get("addresses") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .take(ADDRESSES_MAX)
            .map(cut)
            .collect(),
        _ => Vec::new(),
    };
    Some(Iface {
        name: cut(name),
        kind,
        state,
        addresses,
        ssid: v
            .get("ssid")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| cut_bytes(s, SSID_MAX).to_string()),
        signal_dbm: dbm(v, "signal_dbm"),
        handshake_age_s: v.get("handshake_age_s").and_then(Value::as_u64),
    })
}

/// The interfaces of `thinfloe-network status`. Bad rows are dropped; text
/// that is not the expected object, or has more than `ROWS_MAX` rows, is
/// an error.
pub fn parse_status(json: &str) -> Result<Vec<Iface>, String> {
    Ok(rows(json, "interfaces", "network status")?
        .iter()
        .filter_map(iface)
        .collect())
}

fn in_range(v: &Value) -> Option<InRange> {
    let full = v.get("ssid")?.as_str().filter(|s| !s.is_empty())?;
    let ssid = cut_bytes(full, SSID_MAX);
    // A cut name is not the network's name; the menu refuses names the
    // netplan files cannot carry (backslash, control characters).
    let usable = ssid.len() == full.len() && netfiles::check_ssid(ssid).is_ok();
    Some(InRange {
        ssid: ssid.to_string(),
        signal_dbm: dbm(v, "signal_dbm").unwrap_or(NO_SIGNAL_DBM),
        secured: v.get("secured").and_then(Value::as_bool).unwrap_or(true),
        supported: v.get("supported").and_then(Value::as_bool).unwrap_or(false) && usable,
        name_ok: usable,
    })
}

/// The networks of `thinfloe-network scan`; rules as for `parse_status`.
pub fn parse_scan(json: &str) -> Result<Vec<InRange>, String> {
    Ok(rows(json, "networks", "WiFi scan")?
        .iter()
        .filter_map(in_range)
        .collect())
}

/// The outcome of `apply`: the last line that is JSON. `failed` carries the
/// tool's reason.
pub fn parse_result(stdout: &str) -> Result<(), String> {
    let line = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with('{'))
        .ok_or_else(|| "The network tool printed no result.".to_string())?;
    let v: Value = serde_json::from_str(line).map_err(|_| "The network tool printed no result.".to_string())?;
    match v.get("result").and_then(Value::as_str) {
        Some("ok") => Ok(()),
        Some("failed") => {
            let reason: String = v
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .chars()
                .take(REASON_MAX)
                .collect();
            Err(if reason.trim().is_empty() {
                "The network tool failed.".to_string()
            } else {
                reason
            })
        }
        _ => Err("The network tool printed no result.".to_string()),
    }
}

/// The reason of a last JSON line `{"result": "failed", "reason": ...}`;
/// `None` for any other output or an empty reason.
pub fn failed_reason(stdout: &str) -> Option<String> {
    let line = stdout.lines().rev().map(str::trim).find(|l| l.starts_with('{'))?;
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("result").and_then(Value::as_str) != Some("failed") {
        return None;
    }
    let reason: String = v.get("reason")?.as_str()?.chars().take(REASON_MAX).collect();
    (!reason.trim().is_empty()).then_some(reason)
}

// ---- The wired address form ----

fn in_subnet(a: Ipv4Addr, b: Ipv4Addr, prefix: u8) -> bool {
    let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
    u32::from(a) & mask == u32::from(b) & mask
}

/// The network or the broadcast address of `a`'s network; none for /31
/// (RFC 3021: two hosts) and /32.
fn network_end(a: Ipv4Addr, prefix: u8) -> bool {
    if prefix > 30 {
        return false;
    }
    let host_bits = u32::MAX >> prefix;
    let host = u32::from(a) & host_bits;
    host == 0 || host == host_bits
}

const ENDS_ERR: &str = "the first and last address of the network cannot be used";

/// The fields of the fixed-address form. The messages are shown as they
/// are.
pub fn parse_wired_form(address: &str, gateway: &str, dns: &str) -> Result<Wired, String> {
    const ADDRESS_ERR: &str = "Address: use the form 192.168.1.10/24";
    let (ip, prefix) = address.trim().split_once('/').ok_or(ADDRESS_ERR)?;
    let ip: Ipv4Addr = ip.parse().map_err(|_| ADDRESS_ERR)?;
    let prefix: u8 = prefix.parse().map_err(|_| ADDRESS_ERR)?;
    if !(1..=32).contains(&prefix) {
        return Err(ADDRESS_ERR.to_string());
    }
    if network_end(ip, prefix) {
        return Err(format!("Address: {ENDS_ERR}"));
    }
    let gateway = match gateway.trim() {
        "" => None,
        g => {
            let gw: Ipv4Addr = g
                .parse()
                .map_err(|_| "Gateway: not in the network of the address".to_string())?;
            if !in_subnet(ip, gw, prefix) {
                return Err("Gateway: not in the network of the address".to_string());
            }
            if gw == ip {
                return Err("Gateway: must differ from the address".to_string());
            }
            if network_end(gw, prefix) {
                return Err(format!("Gateway: {ENDS_ERR}"));
            }
            Some(gw)
        }
    };
    const DNS_ERR: &str = "DNS servers: up to three IPv4 addresses";
    let servers = dns
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<Ipv4Addr>().map_err(|_| DNS_ERR))
        .collect::<Result<Vec<_>, _>>()?;
    if servers.len() > 3 {
        return Err(DNS_ERR.to_string());
    }
    Ok(Wired::Static {
        address: ip,
        prefix,
        gateway,
        dns: servers,
    })
}

// ---- Text ----

/// `strong` from -60 dBm up, `good` from -70 dBm up, else `weak`.
pub fn signal_word(dbm: i32) -> &'static str {
    if dbm >= STRONG_DBM {
        "strong"
    } else if dbm >= GOOD_DBM {
        "good"
    } else {
        "weak"
    }
}

/// One line for the status list, for example
/// `WiFi wlp58s0 · Home Net · strong · 192.168.1.23/24`.
pub fn iface_line(i: &Iface) -> String {
    let label = match i.kind {
        IfKind::Wired => "Wired",
        IfKind::Wifi => "WiFi",
        IfKind::Wireguard => "WireGuard",
    };
    let mut parts: Vec<String> = Vec::new();
    match i.state {
        IfState::Off => parts.push("off".to_string()),
        IfState::NoCable => parts.push("no cable".to_string()),
        IfState::NotConnected => parts.push("not connected".to_string()),
        IfState::Connected => match i.kind {
            IfKind::Wifi => {
                if let Some(s) = &i.ssid {
                    parts.push(s.clone());
                }
                if let Some(d) = i.signal_dbm {
                    parts.push(signal_word(d).to_string());
                }
                if i.ssid.is_none() && i.signal_dbm.is_none() {
                    parts.push("connected".to_string());
                }
            }
            IfKind::Wired => {
                if i.addresses.is_empty() {
                    parts.push("connected".to_string());
                }
            }
            IfKind::Wireguard => parts.push("connected".to_string()),
        },
    }
    if i.kind == IfKind::Wireguard && i.state != IfState::Off {
        parts.push(match i.handshake_age_s {
            Some(s) => format!("handshake {s} s ago"),
            None => "no handshake".to_string(),
        });
    }
    if i.state == IfState::Connected && !i.addresses.is_empty() {
        parts.push(i.addresses.join(", "));
    }
    format!("{label} {} · {}", i.name, parts.join(" · "))
}

/// The menu's WiFi file names a device this machine does not have, and the
/// machine has WiFi adapters: the card was swapped.
pub fn adapter_changed(wifi: &WifiFile, devices: &[String]) -> bool {
    wifi.state == FileState::Menu
        && !devices.is_empty()
        && wifi.device.as_deref().is_some_and(|d| !devices.iter().any(|x| x == d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_reads_the_contract() {
        let j = r#"{"interfaces":[
            {"name":"wlp58s0","kind":"wifi","state":"connected","addresses":["192.168.1.23/24"],"ssid":"Home Net","signal_dbm":-55,"handshake_age_s":null},
            {"name":"enp0s31f6","kind":"wired","state":"no-cable","addresses":[],"ssid":null,"signal_dbm":null,"handshake_age_s":null},
            {"name":"wg0","kind":"wireguard","state":"connected","addresses":["10.0.0.2/32"],"ssid":null,"signal_dbm":null,"handshake_age_s":42},
            {"name":"x","kind":"bogus","state":"off"}]}"#;
        let got = parse_status(j).unwrap();
        assert_eq!(got.len(), 3, "the bogus row is dropped");
        assert_eq!(got[0].kind, IfKind::Wifi);
        assert_eq!(got[0].ssid.as_deref(), Some("Home Net"));
        assert_eq!(got[0].signal_dbm, Some(-55));
        assert_eq!(got[1].state, IfState::NoCable);
        assert_eq!(got[2].handshake_age_s, Some(42));
        assert_eq!(got[2].addresses, ["10.0.0.2/32"]);
    }

    #[test]
    fn parse_status_refuses_junk() {
        assert!(parse_status("not json").is_err());
        assert!(parse_status("[]").is_err());
        assert!(parse_status(r#"{"interfaces":"x"}"#).is_err());
        let row = r#"{"name":"a","kind":"wired","state":"off"}"#;
        let many = |n: usize| format!(r#"{{"interfaces":[{}]}}"#, vec![row; n].join(","));
        assert_eq!(parse_status(&many(64)).unwrap().len(), 64);
        assert!(parse_status(&many(65)).is_err());
        // Wrong types in a row: the row goes, the list stays.
        let j = r#"{"interfaces":[{"name":5,"kind":"wired","state":"off"},{"name":"b","kind":"wired","state":"off","signal_dbm":"loud","addresses":"x"}]}"#;
        let got = parse_status(j).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].signal_dbm, None);
        assert!(got[0].addresses.is_empty());
        // Long text is cut.
        let j = format!(
            r#"{{"interfaces":[{{"name":"{}","kind":"wifi","state":"connected","ssid":"{}"}}]}}"#,
            "n".repeat(500),
            "é".repeat(40)
        );
        let got = parse_status(&j).unwrap();
        assert_eq!(got[0].name.chars().count(), 64);
        assert_eq!(got[0].ssid.as_deref().unwrap().len(), 32);
    }

    #[test]
    fn parse_scan_cuts_long_ssid() {
        let long = "ä".repeat(30); // 60 bytes
        let j = format!(
            r#"{{"device":"wlp1","networks":[{{"ssid":"{long}","signal_dbm":-50,"secured":true,"supported":true}},{{"ssid":"Café","signal_dbm":-65,"secured":false,"supported":true}}]}}"#
        );
        let got = parse_scan(&j).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].ssid.len(), 32);
        assert!(!got[0].supported, "a cut name is not the network's name");
        assert_eq!(got[0].unsupported_reason(), Some("This name cannot be used."));
        assert_eq!(got[1].ssid, "Café");
        assert!(got[1].supported && !got[1].secured);
        assert!(parse_scan("{").is_err());
        let row = r#"{"ssid":"a","signal_dbm":-50,"secured":true,"supported":true}"#;
        let many = format!(r#"{{"networks":[{}]}}"#, vec![row; 65].join(","));
        assert!(parse_scan(&many).is_err());
    }

    #[test]
    fn parse_scan_marks_names_the_menu_refuses() {
        let j = r#"{"device":"w","networks":[
            {"ssid":"back\\slash","signal_dbm":-50,"secured":true,"supported":true},
            {"ssid":"tab\there","signal_dbm":-50,"secured":true,"supported":true},
            {"ssid":"fine","signal_dbm":-50,"secured":true,"supported":false},
            {"ssid":"ok","signal_dbm":-50,"secured":true,"supported":true},
            {"ssid":"line\u2028sep","signal_dbm":-50,"secured":true,"supported":true},
            {"ssid":"non\uffffchar","signal_dbm":-50,"secured":true,"supported":true}]}"#;
        let got = parse_scan(j).unwrap();
        let ok: Vec<bool> = got.iter().map(|n| n.supported).collect();
        assert_eq!(ok, [false, false, false, true, false, false]);
        let why: Vec<_> = got.iter().map(InRange::unsupported_reason).collect();
        let name = Some("This name cannot be used.");
        assert_eq!(
            why,
            [
                name,
                name,
                Some("This security type is not supported"),
                None,
                name,
                name
            ]
        );
    }

    #[test]
    fn parse_result_reads_the_last_line() {
        assert_eq!(parse_result("noise\n{\"result\":\"ok\",\"reason\":\"\"}\n"), Ok(()));
        assert_eq!(
            parse_result("{\"result\":\"ok\"}\n{\"result\":\"failed\",\"reason\":\"no link\"}\n"),
            Err("no link".to_string())
        );
        assert!(parse_result("").is_err());
        assert!(parse_result("{broken").is_err());
        assert!(parse_result("{\"result\":\"maybe\"}").is_err());
        assert_eq!(
            parse_result("{\"result\":\"failed\",\"reason\":\"\"}"),
            Err("The network tool failed.".to_string())
        );
    }

    #[test]
    fn parse_wired_form_rules() {
        let addr_err = "Address: use the form 192.168.1.10/24";
        for bad in [
            "",
            "192.168.1.10",
            "192.168.1.10/0",
            "192.168.1.10/33",
            "192.168.1.300/24",
            "a/24",
            "1.2.3.4/x",
        ] {
            assert_eq!(parse_wired_form(bad, "", "").unwrap_err(), addr_err, "{bad}");
        }
        let gw_err = "Gateway: not in the network of the address";
        assert_eq!(
            parse_wired_form("192.168.1.10/24", "192.168.2.1", "").unwrap_err(),
            gw_err
        );
        assert_eq!(parse_wired_form("192.168.1.10/24", "nope", "").unwrap_err(), gw_err);
        let dns_err = "DNS servers: up to three IPv4 addresses";
        for bad in ["1.1.1.1 2.2.2.2 3.3.3.3 4.4.4.4", "x", "1.1.1.1,foo"] {
            assert_eq!(
                parse_wired_form("192.168.1.10/24", "", bad).unwrap_err(),
                dns_err,
                "{bad}"
            );
        }
        let ok = parse_wired_form(" 192.168.1.10/24 ", "192.168.1.1", "9.9.9.9, 1.1.1.1 8.8.8.8").unwrap();
        assert_eq!(
            ok,
            Wired::Static {
                address: "192.168.1.10".parse().unwrap(),
                prefix: 24,
                gateway: Some("192.168.1.1".parse().unwrap()),
                dns: ["9.9.9.9", "1.1.1.1", "8.8.8.8"].map(|s| s.parse().unwrap()).to_vec(),
            }
        );
        let bare = parse_wired_form("10.0.0.2/32", "", "").unwrap();
        assert!(matches!(bare, Wired::Static { gateway: None, ref dns, prefix: 32, .. } if dns.is_empty()));
        assert!(parse_wired_form("10.0.0.2/1", "10.1.0.1", "").is_ok());
    }

    #[test]
    fn parse_wired_form_refuses_addresses_that_cut_the_machine_off() {
        let ends = "the first and last address of the network cannot be used";
        let host_err = format!("Address: {ends}");
        let gw_err = format!("Gateway: {ends}");
        let same = "Gateway: must differ from the address";
        // The network and the broadcast address as host, up to /30.
        for bad in ["192.168.1.0/24", "192.168.1.255/24", "10.0.0.4/30", "10.0.0.7/30"] {
            assert_eq!(parse_wired_form(bad, "", "").unwrap_err(), host_err, "{bad}");
        }
        // And as gateway.
        for gw in ["192.168.1.0", "192.168.1.255"] {
            assert_eq!(parse_wired_form("192.168.1.10/24", gw, "").unwrap_err(), gw_err, "{gw}");
        }
        assert_eq!(
            parse_wired_form("192.168.1.10/24", "192.168.1.10", "").unwrap_err(),
            same
        );
        // The last host is fine, as host and as gateway; so is the first.
        assert!(parse_wired_form("192.168.1.254/24", "192.168.1.1", "").is_ok());
        assert!(parse_wired_form("192.168.1.1/24", "192.168.1.254", "").is_ok());
        assert!(parse_wired_form("10.0.0.5/30", "10.0.0.6", "").is_ok());
        // /31 has two hosts and no network or broadcast address (RFC 3021).
        assert!(parse_wired_form("10.0.0.0/31", "10.0.0.1", "").is_ok());
        assert!(parse_wired_form("10.0.0.1/31", "10.0.0.0", "").is_ok());
        assert_eq!(parse_wired_form("10.0.0.0/31", "10.0.0.0", "").unwrap_err(), same);
        // /32 is the address alone: no gateway can be in its network.
        assert!(parse_wired_form("10.0.0.2/32", "", "").is_ok());
        assert_eq!(parse_wired_form("10.0.0.2/32", "10.0.0.2", "").unwrap_err(), same);
        assert_eq!(
            parse_wired_form("10.0.0.2/32", "10.0.0.1", "").unwrap_err(),
            "Gateway: not in the network of the address"
        );
    }

    #[test]
    fn signal_words() {
        assert_eq!(signal_word(-30), "strong");
        assert_eq!(signal_word(-60), "strong");
        assert_eq!(signal_word(-61), "good");
        assert_eq!(signal_word(-70), "good");
        assert_eq!(signal_word(-71), "weak");
    }

    fn iface_of(kind: IfKind, state: IfState, name: &str) -> Iface {
        Iface {
            name: name.to_string(),
            kind,
            state,
            addresses: Vec::new(),
            ssid: None,
            signal_dbm: None,
            handshake_age_s: None,
        }
    }

    #[test]
    fn iface_lines() {
        let mut w = iface_of(IfKind::Wifi, IfState::Connected, "wlp58s0");
        w.ssid = Some("Home Net".into());
        w.signal_dbm = Some(-50);
        w.addresses = vec!["192.168.1.23/24".into()];
        assert_eq!(iface_line(&w), "WiFi wlp58s0 · Home Net · strong · 192.168.1.23/24");
        let e = iface_of(IfKind::Wired, IfState::NoCable, "enp0s31f6");
        assert_eq!(iface_line(&e), "Wired enp0s31f6 · no cable");
        let mut g = iface_of(IfKind::Wireguard, IfState::Connected, "wg0");
        g.handshake_age_s = Some(42);
        g.addresses = vec!["10.0.0.2/32".into()];
        assert_eq!(
            iface_line(&g),
            "WireGuard wg0 · connected · handshake 42 s ago · 10.0.0.2/32"
        );
        g.handshake_age_s = None;
        g.state = IfState::NotConnected;
        assert_eq!(iface_line(&g), "WireGuard wg0 · not connected · no handshake");
        let mut d = iface_of(IfKind::Wired, IfState::Connected, "eno1");
        assert_eq!(iface_line(&d), "Wired eno1 · connected");
        d.addresses = vec!["1.2.3.4/24".into(), "5.6.7.8/24".into()];
        assert_eq!(iface_line(&d), "Wired eno1 · 1.2.3.4/24, 5.6.7.8/24");
        assert_eq!(
            iface_line(&iface_of(IfKind::Wifi, IfState::Off, "wlp1")),
            "WiFi wlp1 · off"
        );
        assert_eq!(
            iface_line(&iface_of(IfKind::Wifi, IfState::NotConnected, "wlp1")),
            "WiFi wlp1 · not connected"
        );
    }

    fn wifi_file(state: FileState, device: Option<&str>) -> WifiFile {
        WifiFile {
            state,
            device: device.map(String::from),
            networks: Vec::new(),
            manageable: true,
            error: None,
        }
    }

    #[test]
    fn adapter_changed_rule() {
        let machine = vec!["wlp58s0".to_string()];
        assert!(adapter_changed(&wifi_file(FileState::Menu, Some("wlp2s0")), &machine));
        assert!(!adapter_changed(&wifi_file(FileState::Menu, Some("wlp58s0")), &machine));
        assert!(!adapter_changed(&wifi_file(FileState::Hand, Some("wlp2s0")), &machine));
        assert!(!adapter_changed(&wifi_file(FileState::Menu, None), &machine));
        assert!(!adapter_changed(&wifi_file(FileState::Menu, Some("wlp2s0")), &[]));
    }
}
