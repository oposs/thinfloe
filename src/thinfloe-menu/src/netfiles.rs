//! The Network tab's view of `[network]` in config.toml, and the netplan
//! text for it. `view` turns the table into what the tab shows, `plan`
//! turns a change into the candidate table, `take_over` makes kept text the
//! menu's own. Nothing here writes a file: the store does, after the
//! candidate rendered as generated/wifi.yaml and generated/ethernet.yaml
//! passed `netplan generate`. The parsers also read the old
//! ~/wifi.yaml and ~/ethernet.yaml for the converter.

use crate::config::NetworkCfg;
use crate::generated::{self, Inputs};
use std::fs;
use std::io::{self, Read, Write};
use std::net::Ipv4Addr;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::{Marker, TScalarStyle};
use yaml_rust2::yaml::Hash;
use yaml_rust2::{Yaml, YamlEmitter};

/// First line of the text `render_wifi` and `render_wired` give (and of the
/// files the 3.x menu wrote into the home): the mark of text in the menu's
/// own form. The generated files carry `generated::HEADER` instead.
pub const MENU_HEADER: &str = "# Written by the ThinFloe menu. Edits here are replaced.";

pub const WIFI_FILE: &str = "wifi.yaml";
pub const WIRED_FILE: &str = "ethernet.yaml";
/// Text of the shipped ~/wifi.yaml template; the menu never writes it.
pub const TEMPLATE_MARKER: &str = "device name of your wifi card";
/// A readable copy of the shipped /etc/netplan/ethernet.yaml (that one is
/// mode 600). Its `eth` definition is the base of a fixed address.
pub const DEFAULT_WIRED: &str = "/usr/share/thinfloe/netplan/ethernet.yaml";

/// The DNS servers the form takes; WiFi or wired text with more is not
/// the menu's to keep.
pub const MAX_DNS: usize = 3;

/// Network files are tiny.
const MAX_BYTES: u64 = 64 * 1024;
/// How long `netplan generate` may take before the check gives up.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// The shipped template, or a file that says nothing: it contains the
/// marker, is empty, or holds only comments and blank lines.
pub fn is_template(text: &str) -> bool {
    text.contains(TEMPLATE_MARKER)
        || text.lines().all(|l| {
            let l = l.trim();
            l.is_empty() || l.starts_with('#')
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// Nothing set: the default applies.
    Missing,
    /// The shipped template of the old ~/wifi.yaml; only the converter sees it.
    Template,
    /// The menu's own settings.
    Menu,
    /// Kept as written (`kept_wifi`, `kept_wired`): the menu cannot change it.
    Hand,
}

/// Who wrote `text` (`None`: no file). `template_rule` lets the template
/// count as `Template`; only ~/wifi.yaml has one.
pub fn state(text: Option<&str>, template_rule: bool) -> FileState {
    match text {
        None => FileState::Missing,
        Some(t) if t.lines().next() == Some(MENU_HEADER) => FileState::Menu,
        Some(t) if template_rule && is_template(t) => FileState::Template,
        Some(_) => FileState::Hand,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Net {
    pub ssid: String,
    pub hidden: bool,
    /// `None`: an open network.
    pub password: Option<String>,
}

/// The wired addressing. The DNS servers are the machine's one DNS
/// setting: empty means those DHCP gives (DHCP) or none (fixed). The menu
/// writes them into ~/wifi.yaml too, see `save`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wired {
    /// `dns` not empty: these replace the servers DHCP gives.
    Dhcp { dns: Vec<Ipv4Addr> },
    Static {
        address: Ipv4Addr,
        prefix: u8,
        gateway: Option<Ipv4Addr>,
        dns: Vec<Ipv4Addr>,
    },
}

impl Wired {
    /// DHCP with the servers DHCP gives.
    pub fn dhcp() -> Wired {
        Wired::Dhcp { dns: Vec::new() }
    }

    pub fn dns(&self) -> &[Ipv4Addr] {
        match self {
            Wired::Dhcp { dns } | Wired::Static { dns, .. } => dns,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiFile {
    pub state: FileState,
    /// The device of the only WiFi definition.
    pub device: Option<String>,
    /// Passwords are kept when `manageable`; a menu file that is not keeps
    /// those of the networks whose AP has no keys besides `password` and
    /// `hidden`.
    pub networks: Vec<Net>,
    /// The DNS servers that replace those DHCP gives on that device.
    pub dns: Vec<Ipv4Addr>,
    /// The menu can take the file over without losing a setting.
    pub manageable: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiredFile {
    pub state: FileState,
    pub wired: Wired,
    pub manageable: bool,
    pub error: Option<String>,
}

// ---- YAML ----

/// A YAML loader that keeps scalars as netplan reads them: a plain scalar
/// becomes a number, a flag or null only when that type writes it back
/// unchanged, so `password: 00012345` stays `"00012345"`. Aliases are
/// refused: netplan files have no use for them, and expanding them lets a
/// small file grow without bound.
#[derive(Default)]
struct Loader {
    docs: Vec<Yaml>,
    stack: Vec<Yaml>,
    /// One slot per open mapping: its pending key.
    keys: Vec<Option<Yaml>>,
    error: Option<String>,
}

impl MarkedEventReceiver for Loader {
    fn on_event(&mut self, ev: Event, mark: Marker) {
        if self.error.is_some() {
            return;
        }
        let node = match ev {
            Event::DocumentEnd => {
                let doc = self.stack.pop().unwrap_or(Yaml::Null);
                self.docs.push(doc);
                return;
            }
            Event::SequenceStart(..) => {
                self.stack.push(Yaml::Array(Vec::new()));
                return;
            }
            Event::MappingStart(..) => {
                self.stack.push(Yaml::Hash(Hash::new()));
                self.keys.push(None);
                return;
            }
            Event::SequenceEnd => self.stack.pop(),
            Event::MappingEnd => {
                self.keys.pop();
                self.stack.pop()
            }
            Event::Scalar(v, style, ..) => Some(scalar(v, style)),
            Event::Alias(_) => {
                self.error = Some(format!("aliases are not supported (line {})", mark.line()));
                return;
            }
            _ => return,
        };
        let Some(node) = node else {
            self.error = Some("unbalanced document".to_string());
            return;
        };
        match self.stack.last_mut() {
            None => self.stack.push(node),
            Some(Yaml::Array(a)) => a.push(node),
            Some(Yaml::Hash(h)) => match self.keys.last_mut() {
                Some(slot @ None) => *slot = Some(node),
                Some(slot) => {
                    let key = slot.take().unwrap_or(Yaml::Null);
                    if h.contains_key(&key) {
                        self.error = Some(format!("duplicate key (line {})", mark.line()));
                    } else {
                        h.insert(key, node);
                    }
                }
                None => self.error = Some("unbalanced document".to_string()),
            },
            Some(_) => self.error = Some("unbalanced document".to_string()),
        }
    }
}

fn scalar(v: String, style: TScalarStyle) -> Yaml {
    if style != TScalarStyle::Plain {
        return Yaml::String(v);
    }
    match v.as_str() {
        "" | "~" | "null" => Yaml::Null,
        "true" => Yaml::Boolean(true),
        "false" => Yaml::Boolean(false),
        _ => match v.parse::<i64>() {
            Ok(i) if i.to_string() == v => Yaml::Integer(i),
            _ => Yaml::String(v),
        },
    }
}

/// The documents of `text`.
fn load_yaml(text: &str) -> Result<Vec<Yaml>, String> {
    let mut l = Loader::default();
    Parser::new_from_str(text)
        .load(&mut l, true)
        .map_err(|e| e.to_string())?;
    match l.error {
        Some(e) => Err(e),
        None => Ok(l.docs),
    }
}

/// `y` as YAML text after the header line.
fn emit(y: &Yaml) -> Result<String, String> {
    let mut out = String::new();
    YamlEmitter::new(&mut out).dump(y).map_err(|e| format!("{e:?}"))?;
    let body = out.strip_prefix("---\n").unwrap_or(&out);
    Ok(format!("{MENU_HEADER}\n{body}\n"))
}

/// A text of the menu's own form without its header line.
pub fn without_menu_header(text: &str) -> &str {
    text.strip_prefix(MENU_HEADER)
        .and_then(|t| t.strip_prefix('\n'))
        .unwrap_or(text)
}

fn key(k: &str) -> Yaml {
    Yaml::String(k.to_string())
}

fn str_val(s: &str) -> Yaml {
    Yaml::String(s.to_string())
}

/// Set `k` in `h`; a key there already keeps its place.
fn set(h: &mut Hash, k: &str, v: Yaml) {
    match h.get_mut(&key(k)) {
        Some(slot) => *slot = v,
        None => {
            h.insert(key(k), v);
        }
    }
}

fn get<'a>(h: &'a Hash, k: &str) -> Option<&'a Yaml> {
    h.get(&key(k))
}

fn get_hash<'a>(h: Option<&'a Hash>, k: &str) -> Option<&'a Hash> {
    h.and_then(|h| get(h, k)).and_then(Yaml::as_hash)
}

/// Every key of `h` is one of `allowed`.
fn keys_within(h: &Hash, allowed: &[&str]) -> bool {
    h.keys().all(|k| k.as_str().is_some_and(|k| allowed.contains(&k)))
}

/// A scalar as the text netplan sees.
fn text_of(y: &Yaml) -> Option<String> {
    match y {
        Yaml::String(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        Yaml::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A flag the way netplan reads one.
fn flag(y: &Yaml) -> Option<bool> {
    match y {
        Yaml::Boolean(b) => Some(*b),
        Yaml::String(s) => match s.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "y" => Some(true),
            "false" | "no" | "off" | "n" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// The file says another netplan version than 2, or another renderer than
/// the image's networkd, for the whole file or one definition. Rewritten by
/// the menu it would say `version: 2` and `renderer: networkd` and so do
/// something else; it counts as by hand even under the menu's header.
/// netplan takes both as these when they are left out.
fn foreign_format(network: Option<&Hash>) -> bool {
    let other_renderer = |h: &Hash| get(h, "renderer").is_some_and(|r| text_of(r).as_deref() != Some("networkd"));
    network.is_some_and(|n| {
        get(n, "version").is_some_and(|v| text_of(v).as_deref() != Some("2"))
            || other_renderer(n)
            // netplan takes a renderer on one definition too.
            || ["wifis", "ethernets"].iter().any(|kind| {
                get_hash(Some(n), kind)
                    .is_some_and(|defs| defs.values().filter_map(Yaml::as_hash).any(other_renderer))
            })
    })
}

// ---- WiFi ----

pub fn parse_wifi(text: Option<&str>) -> WifiFile {
    let mut f = WifiFile {
        state: state(text, true),
        device: None,
        networks: Vec::new(),
        dns: Vec::new(),
        manageable: true,
        error: None,
    };
    let Some(text) = text.filter(|_| f.state != FileState::Template) else {
        return f;
    };
    let docs = match load_yaml(text) {
        Ok(d) => d,
        Err(e) => {
            f.manageable = false;
            f.error = Some(format!("WiFi settings: {e}"));
            return f;
        }
    };
    let root = docs.first().and_then(Yaml::as_hash);
    let network = get_hash(root, "network");
    let wifis = get_hash(network, "wifis");
    // Per network: its AP keys are all the menu's own (Ruling 22).
    let mut ap_ok = Vec::new();
    // No `wifis` at all is the file `render_wifi` writes without a device
    // or networks; a `wifis` key holds exactly one definition.
    let no_wifi = network.is_some_and(|h| get(h, "wifis").is_none());
    if foreign_format(network) {
        f.state = FileState::Hand;
    }
    let mut ok = !foreign_format(network)
        && docs.len() == 1
        && root.is_some_and(|h| keys_within(h, &["network"]))
        && network.is_some_and(|h| keys_within(h, &["version", "renderer", "wifis"]))
        && (no_wifi || wifis.is_some_and(|h| h.len() == 1));
    for (dev, def) in wifis.into_iter().flatten() {
        let def = def.as_hash();
        let dhcp = def.is_some_and(|d| get(d, "dhcp4").and_then(flag) == Some(true));
        let (dns, dns_ok) = def.map_or((Vec::new(), true), |d| dhcp_dns(d, dhcp));
        // `render_wifi` writes `dhcp4: true` and `optional: true`: a
        // definition without DHCP, or one boot waits for, is not the
        // menu's (as Ruling 25 for `optional` on the wired one).
        // As on the wired file: the form takes at most MAX_DNS servers.
        ok &= dns.len() <= MAX_DNS;
        ok &= dns_ok && dhcp && def.is_some_and(|d| get(d, "optional").is_none_or(|o| flag(o) == Some(true)));
        if wifis.is_some_and(|h| h.len() == 1) {
            f.device = text_of(dev);
            f.dns = dns;
        }
        ok &= def.is_some_and(|d| {
            keys_within(
                d,
                &["dhcp4", "dhcp4-overrides", "nameservers", "optional", "access-points"],
            )
        });
        let aps = get_hash(def, "access-points");
        ok &= aps.is_some();
        for (ssid, ap) in aps.into_iter().flatten() {
            let Some(ssid) = text_of(ssid) else {
                ok = false;
                continue;
            };
            let mut n = Net {
                ssid,
                hidden: false,
                password: None,
            };
            let mut this_ok = true;
            match ap {
                Yaml::Null => {}
                Yaml::Hash(a) => {
                    this_ok &= keys_within(a, &["password", "hidden"]);
                    if let Some(p) = get(a, "password") {
                        n.password = text_of(p);
                        this_ok &= n.password.is_some();
                    }
                    if let Some(h) = get(a, "hidden") {
                        n.hidden = flag(h).unwrap_or(false);
                        this_ok &= flag(h).is_some();
                    }
                }
                _ => this_ok = false,
            }
            ok &= this_ok;
            ap_ok.push(this_ok);
            f.networks.push(n);
        }
    }
    // A file the loader cannot fully keep loses its passwords; the menu's
    // own file only those of the networks whose AP has other keys.
    if !ok {
        let menu = f.state == FileState::Menu;
        for (n, kept) in f.networks.iter_mut().zip(ap_ok) {
            if !(menu && kept) {
                n.password = None;
            }
        }
    }
    f.manageable = ok;
    f
}

/// The menu's ~/wifi.yaml: one definition for `device` with the networks
/// that pass the input checks (`save` refuses the others before it gets
/// here). No device or no network: a file with no WiFi at all, which
/// netplan accepts. `dns` not empty: these servers replace those DHCP
/// gives.
pub fn render_wifi(device: Option<&str>, networks: &[Net], dns: &[Ipv4Addr]) -> String {
    let mut net = Hash::new();
    net.insert(key("version"), Yaml::Integer(2));
    net.insert(key("renderer"), str_val("networkd"));
    let mut aps = Hash::new();
    for n in networks.iter().filter(|n| check_net(n).is_ok()) {
        let mut ap = Hash::new();
        if let Some(p) = &n.password {
            ap.insert(key("password"), str_val(p));
        }
        if n.hidden {
            ap.insert(key("hidden"), Yaml::Boolean(true));
        }
        aps.insert(str_val(&n.ssid), Yaml::Hash(ap));
    }
    let device = device.filter(|d| !d.is_empty() && !d.contains(TEMPLATE_MARKER));
    if let Some(device) = device.filter(|_| !aps.is_empty()) {
        let mut def = Hash::new();
        def.insert(key("dhcp4"), Yaml::Boolean(true));
        set_dhcp_dns(&mut def, dns);
        // Boot never waits for this device: the home may move to a
        // machine that does not have it.
        def.insert(key("optional"), Yaml::Boolean(true));
        def.insert(key("access-points"), Yaml::Hash(aps));
        let mut wifis = Hash::new();
        wifis.insert(str_val(device), Yaml::Hash(def));
        net.insert(key("wifis"), Yaml::Hash(wifis));
    }
    let mut root = Hash::new();
    root.insert(key("network"), Yaml::Hash(net));
    emit(&Yaml::Hash(root)).unwrap_or_else(|_| format!("{MENU_HEADER}\nnetwork:\n  version: 2\n  renderer: networkd\n"))
}

/// The WiFi devices under `sys_net` (/sys/class/net), sorted.
pub fn wifi_devices(sys_net: &Path) -> Vec<String> {
    let Ok(dir) = fs::read_dir(sys_net) else {
        return Vec::new();
    };
    let mut devs: Vec<String> = dir
        .flatten()
        .filter(|e| e.path().join("wireless").exists())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    devs.sort();
    devs
}

/// A character the netplan files cannot carry: control characters (the
/// emitter writes them unquoted), U+2028 and U+2029 (libyaml refuses them
/// in a plain key) and U+FFFE, U+FFFF (libyaml refuses them anywhere).
fn unusable_char(c: char) -> bool {
    c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{fffe}' | '\u{ffff}')
}

/// A backslash is refused: netplan 1.2 writes `ssid=P"..."` without escaping
/// it, and wpa_supplicant's printf_decode then drops or reinterprets it (seen
/// in the image: `back\slash` is joined as `backslash`). Passwords keep
/// backslashes, since wpa_supplicant does not decode `psk="..."`.
pub fn check_ssid(s: &str) -> Result<(), String> {
    if s.is_empty() || s.len() > 32 {
        return Err("The name needs 1 to 32 bytes.".to_string());
    }
    if s.contains(TEMPLATE_MARKER) || s.contains('\\') || s.chars().any(unusable_char) {
        return Err("This name cannot be used.".to_string());
    }
    Ok(())
}

/// 8 to 63 bytes, as netplan and wpa_supplicant count them, or the key
/// itself as 64 hex digits. The message says characters; for an ASCII
/// password that is the same.
pub fn check_password(p: &str) -> Result<(), String> {
    let hex_key = p.len() == 64 && p.bytes().all(|b| b.is_ascii_hexdigit());
    if !hex_key && !(8..=63).contains(&p.len()) {
        return Err("The password needs 8 to 63 characters.".to_string());
    }
    if p.contains(TEMPLATE_MARKER) || p.chars().any(unusable_char) {
        return Err("This password cannot be used.".to_string());
    }
    Ok(())
}

fn check_net(n: &Net) -> Result<(), String> {
    check_ssid(&n.ssid)?;
    match &n.password {
        Some(p) => check_password(p),
        None => Ok(()),
    }
}

// ---- Wired ----

pub fn parse_wired(text: Option<&str>) -> WiredFile {
    let mut f = WiredFile {
        state: state(text, false),
        wired: Wired::dhcp(),
        manageable: true,
        error: None,
    };
    let Some(text) = text else {
        return f;
    };
    let docs = match load_yaml(text) {
        Ok(d) => d,
        Err(e) => {
            f.manageable = false;
            f.error = Some(format!("Wired settings: {e}"));
            return f;
        }
    };
    let root = docs.first().and_then(Yaml::as_hash);
    let network = get_hash(root, "network");
    let ethernets = get_hash(network, "ethernets");
    let eth = get_hash(ethernets, "eth");
    if foreign_format(network) {
        f.state = FileState::Hand;
    }
    let mut ok = !foreign_format(network)
        && docs.len() == 1
        && root.is_some_and(|h| keys_within(h, &["network"]))
        && network.is_some_and(|h| keys_within(h, &["version", "renderer", "ethernets"]))
        && ethernets.is_some_and(|h| h.len() == 1)
        && eth.is_some_and(|h| {
            keys_within(
                h,
                &[
                    "match",
                    "dhcp4",
                    "dhcp4-overrides",
                    "addresses",
                    "routes",
                    "nameservers",
                    "optional",
                ],
            )
        });
    let Some(eth) = eth else {
        f.manageable = false;
        return f;
    };
    // Ruling 25: the menu writes the default's `match` and `optional`, so
    // a take-over keeps only a file that has those, or neither.
    ok &= get(eth, "match").is_none_or(|m| {
        m.as_hash()
            .is_some_and(|h| h.len() == 1 && get(h, "name").and_then(text_of).as_deref() == Some("en*"))
    });
    ok &= get(eth, "optional").is_none_or(|o| flag(o) == Some(true));
    let dhcp4 = get(eth, "dhcp4").and_then(flag);
    match get(eth, "addresses") {
        None => {
            let (dns, dns_ok) = dhcp_dns(eth, dhcp4 == Some(true));
            ok &= dhcp4 == Some(true) && get(eth, "routes").is_none() && dns_ok;
            f.wired = Wired::Dhcp { dns };
        }
        Some(addresses) => {
            let cidr = addresses.as_vec().filter(|a| a.len() == 1).and_then(|a| text_of(&a[0]));
            match cidr.as_deref().and_then(parse_cidr) {
                Some((address, prefix)) => {
                    let (gateway, gw_ok) = default_route(get(eth, "routes"));
                    let (dns, dns_ok) = nameservers(get(eth, "nameservers"));
                    // `dhcp4-overrides` belongs to the DHCP form only.
                    ok &= gw_ok && dns_ok && dhcp4 != Some(true) && get(eth, "dhcp4-overrides").is_none();
                    f.wired = Wired::Static {
                        address,
                        prefix,
                        gateway,
                        dns,
                    };
                }
                None => ok = false,
            }
        }
    }
    // The form refuses more servers, so every edit of such a file would fail.
    ok &= f.wired.dns().len() <= MAX_DNS;
    f.manageable = ok;
    f
}

fn parse_cidr(s: &str) -> Option<(Ipv4Addr, u8)> {
    let (a, p) = s.split_once('/')?;
    let p: u8 = p.parse().ok()?;
    (p <= 32).then_some(())?;
    Some((a.parse().ok()?, p))
}

/// The gateway of `routes` and whether the menu can keep the list: at most
/// one route, `to: default`.
fn default_route(routes: Option<&Yaml>) -> (Option<Ipv4Addr>, bool) {
    let Some(routes) = routes else {
        return (None, true);
    };
    let Some([route]) = routes.as_vec().map(Vec::as_slice) else {
        return (None, false);
    };
    let Some(r) = route.as_hash().filter(|r| keys_within(r, &["to", "via"])) else {
        return (None, false);
    };
    let to = get(r, "to").and_then(text_of);
    let via = get(r, "via").and_then(text_of).and_then(|v| v.parse().ok());
    match (to.as_deref(), via) {
        (Some("default" | "0.0.0.0/0"), Some(via)) => (Some(via), true),
        _ => (None, false),
    }
}

/// The DNS servers and whether the menu can keep them.
fn nameservers(ns: Option<&Yaml>) -> (Vec<Ipv4Addr>, bool) {
    let Some(ns) = ns else {
        return (Vec::new(), true);
    };
    let Some(h) = ns.as_hash().filter(|h| keys_within(h, &["addresses"])) else {
        return (Vec::new(), false);
    };
    let Some(list) = get(h, "addresses").and_then(Yaml::as_vec) else {
        return (Vec::new(), false);
    };
    let dns: Vec<Ipv4Addr> = list.iter().filter_map(|a| text_of(a)?.parse().ok()).collect();
    let ok = dns.len() == list.len();
    (dns, ok)
}

/// The DNS servers that replace those DHCP gives on the definition `def`,
/// and whether the menu can keep them. The menu writes neither key, or
/// `dhcp4-overrides: {use-dns: false}` together with a non-empty
/// `nameservers` on a definition with `dhcp4: true` (`dhcp`); servers
/// added to those of DHCP, or no DNS at all, are not its settings.
fn dhcp_dns(def: &Hash, dhcp: bool) -> (Vec<Ipv4Addr>, bool) {
    let overrides = get(def, "dhcp4-overrides");
    let ns = get(def, "nameservers");
    if overrides.is_none() && ns.is_none() {
        return (Vec::new(), true);
    }
    let (dns, ns_ok) = nameservers(ns);
    let dns_off = overrides
        .and_then(Yaml::as_hash)
        .is_some_and(|o| o.len() == 1 && get(o, "use-dns").and_then(flag) == Some(false));
    let ok = dhcp && dns_off && ns_ok && !dns.is_empty();
    (dns, ok)
}

fn nameservers_yaml(dns: &[Ipv4Addr]) -> Yaml {
    let mut ns = Hash::new();
    let list = dns.iter().map(|d| str_val(&d.to_string())).collect();
    ns.insert(key("addresses"), Yaml::Array(list));
    Yaml::Hash(ns)
}

/// The DHCP definition `def` with `dns` in place of the servers DHCP
/// gives; empty: those of DHCP.
fn set_dhcp_dns(def: &mut Hash, dns: &[Ipv4Addr]) {
    if dns.is_empty() {
        def.remove(&key("dhcp4-overrides"));
        def.remove(&key("nameservers"));
        return;
    }
    let mut overrides = Hash::new();
    overrides.insert(key("use-dns"), Yaml::Boolean(false));
    set(def, "dhcp4-overrides", Yaml::Hash(overrides));
    set(def, "nameservers", nameservers_yaml(dns));
}

/// The menu's ~/ethernet.yaml: `default` (the shipped file) with only the
/// addressing and the DNS servers of `eth` changed. `None`: DHCP with the
/// servers DHCP gives, which is the default itself.
pub fn render_wired(default: &str, wired: &Wired) -> Result<Option<String>, String> {
    if let Wired::Static { prefix, .. } = wired
        && *prefix > 32
    {
        return Err(format!("The prefix /{prefix} is not between 0 and 32."));
    }
    if matches!(wired, Wired::Dhcp { dns } if dns.is_empty()) {
        return Ok(None);
    }
    let mut root = load_yaml(default)
        .map_err(|e| format!("{DEFAULT_WIRED}: {e}"))?
        .into_iter()
        .next()
        .unwrap_or(Yaml::Null);
    let eth = root
        .as_mut_hash()
        .and_then(|h| h.get_mut(&key("network")))
        .and_then(Yaml::as_mut_hash)
        .and_then(|h| h.get_mut(&key("ethernets")))
        .and_then(Yaml::as_mut_hash)
        .and_then(|h| h.get_mut(&key("eth")))
        .and_then(Yaml::as_mut_hash)
        .ok_or_else(|| format!("{DEFAULT_WIRED} has no network.ethernets.eth"))?;
    let Wired::Static {
        address,
        prefix,
        gateway,
        dns,
    } = wired
    else {
        set(eth, "dhcp4", Yaml::Boolean(true));
        eth.remove(&key("addresses"));
        eth.remove(&key("routes"));
        set_dhcp_dns(eth, wired.dns());
        return emit(&root).map(Some);
    };
    set(eth, "dhcp4", Yaml::Boolean(false));
    eth.remove(&key("dhcp4-overrides"));
    set(
        eth,
        "addresses",
        Yaml::Array(vec![str_val(&format!("{address}/{prefix}"))]),
    );
    match gateway {
        Some(gw) => {
            let mut r = Hash::new();
            r.insert(key("to"), str_val("default"));
            r.insert(key("via"), str_val(&gw.to_string()));
            set(eth, "routes", Yaml::Array(vec![Yaml::Hash(r)]));
        }
        None => {
            eth.remove(&key("routes"));
        }
    }
    if dns.is_empty() {
        eth.remove(&key("nameservers"));
    } else {
        set(eth, "nameservers", nameservers_yaml(dns));
    }
    emit(&root).map(Some)
}

// ---- The check ----

/// Run `<netplan> generate` on `files` (name, text) as if they were all of
/// /etc/netplan, in a fresh directory under $TMPDIR (else
/// $XDG_RUNTIME_DIR) that is removed afterwards. `Err` holds netplan's
/// reason: its first `ERROR:` line, else the first line that names one of
/// the files, else its last line.
pub fn check_netplan(netplan: &str, files: &[(&str, &str)]) -> Result<(), String> {
    check_netplan_within(netplan, files, CHECK_TIMEOUT)
}

fn check_netplan_within(netplan: &str, files: &[(&str, &str)], timeout: Duration) -> Result<(), String> {
    let base = ["TMPDIR", "XDG_RUNTIME_DIR"]
        .iter()
        .filter_map(std::env::var_os)
        .find(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or("No folder for the netplan check: TMPDIR and XDG_RUNTIME_DIR are not set.")?;
    let dir = fresh_dir(&base)?;
    let result = run_check(netplan, &dir, files, timeout);
    let _ = fs::remove_dir_all(&dir);
    result
}

fn fresh_dir(base: &Path) -> Result<PathBuf, String> {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    loop {
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let d = base.join(format!("thinfloe-netplan-check-{}-{n}", std::process::id()));
        match fs::DirBuilder::new().mode(0o700).create(&d) {
            Ok(()) => return Ok(d),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < u32::MAX => continue,
            Err(e) => return Err(format!("{}: {e}", d.display())),
        }
    }
}

fn run_check(netplan: &str, dir: &Path, files: &[(&str, &str)], timeout: Duration) -> Result<(), String> {
    let etc = dir.join("etc/netplan");
    fs::create_dir_all(&etc).map_err(|e| format!("{}: {e}", etc.display()))?;
    for (name, text) in files {
        if name.is_empty() || name.contains('/') {
            return Err(format!("Bad file name for the netplan check: {name}"));
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(etc.join(name))
            .and_then(|mut f| f.write_all(text.as_bytes()))
            .map_err(|e| format!("{name}: {e}"))?;
    }
    let mut child = spawn(
        Command::new(netplan)
            .arg("generate")
            .arg("--root-dir")
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped()),
    )
    .map_err(|e| format!("Cannot run {netplan}: {e}"))?;
    // netplan says little; read it beside the wait so a full pipe never
    // stalls it.
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = (&mut stderr).take(MAX_BYTES).read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                // The reader may wait on a child of netplan that kept the
                // pipe; it ends with that one.
                return Err(format!(
                    "netplan generate did not finish within {} s.",
                    timeout.as_secs()
                ));
            }
            Err(e) => return Err(format!("netplan generate: {e}")),
        }
    };
    let err = String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned();
    if status.success() {
        return Ok(());
    }
    // A parse error has no ERROR: line but names the file by its path in
    // the scratch dir, and as tfuser netplan ends with noise of its
    // failed `systemctl daemon-reload`. The file name alone is what the
    // user knows.
    let prefix = format!("{}/", etc.display());
    let mut lines = err.lines().map(str::trim).filter(|l| !l.is_empty());
    let reason = lines
        .clone()
        .find(|l| l.contains("ERROR:"))
        .or_else(|| lines.clone().find(|l| l.contains(&prefix)))
        .or_else(|| lines.next_back())
        .map(|l| l.replace(&prefix, ""));
    Err(reason.unwrap_or_else(|| format!("netplan generate failed ({status}).")))
}

/// Spawn `cmd`, retrying while the program is busy: a script written just
/// before can still be open in a child forked meanwhile by another thread.
fn spawn(cmd: &mut Command) -> io::Result<std::process::Child> {
    let mut tries = 0;
    loop {
        match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && tries < 50 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            r => return r,
        }
    }
}

// ---- View, plan, take over ----

/// Where the system lives, and the programs and paths to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Homes {
    pub home: PathBuf,
    pub sys_net: PathBuf,
    pub netplan: String,
    pub default_wired: PathBuf,
}

impl Homes {
    /// $HOME, /sys/class/net, `/usr/sbin/netplan` and `DEFAULT_WIRED`; tests set
    /// `TF_NETWORK_SYS`, `TF_NETPLAN` and `TF_NETWORK_DEFAULT_WIRED`.
    pub fn from_env() -> Homes {
        let var = |name: &str, default: &str| std::env::var_os(name).unwrap_or_else(|| default.into());
        Homes {
            home: var("HOME", "/home/tfuser").into(),
            sys_net: var("TF_NETWORK_SYS", "/sys/class/net").into(),
            netplan: var("TF_NETPLAN", "/usr/sbin/netplan").to_string_lossy().into_owned(),
            default_wired: var("TF_NETWORK_DEFAULT_WIRED", DEFAULT_WIRED).into(),
        }
    }

    fn default_text(&self) -> Result<String, String> {
        crate::bounded::read_bounded(&self.default_wired, MAX_BYTES, true)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .map_err(|e| format!("{}: {e}", self.default_wired.display()))
    }
}

/// What to change; `None`: leave that part as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    pub wifi: Option<Vec<Net>>,
    pub wired: Option<Wired>,
}

/// What the Network tab shows for `[network]`. Text kept from the old
/// files reads as `Hand`: shown, not editable, and `manageable` says
/// whether *Let the menu manage it* can take it over.
pub fn view(cfg: &NetworkCfg) -> (WifiFile, WiredFile) {
    (view_wifi(cfg), view_wired(cfg))
}

fn view_wifi(cfg: &NetworkCfg) -> WifiFile {
    if let Some(text) = &cfg.kept_wifi {
        let mut f = parse_wifi(Some(text));
        f.state = FileState::Hand;
        return f;
    }
    WifiFile {
        state: if cfg.networks.is_empty() {
            FileState::Missing
        } else {
            FileState::Menu
        },
        device: cfg.wifi_device.clone(),
        networks: cfg.nets(),
        dns: cfg.wifi_dns.iter().filter_map(|d| d.parse().ok()).collect(),
        manageable: true,
        error: None,
    }
}

fn view_wired(cfg: &NetworkCfg) -> WiredFile {
    if let Some(text) = &cfg.kept_wired {
        let mut f = parse_wired(Some(text));
        f.state = FileState::Hand;
        return f;
    }
    match cfg.wired() {
        Ok(wired) => WiredFile {
            state: if wired == Wired::dhcp() {
                FileState::Missing
            } else {
                FileState::Menu
            },
            wired,
            manageable: true,
            error: None,
        },
        // A table edited into something the menu cannot render: shown as
        // kept, so the form does not offer to change what it cannot read.
        Err(e) => WiredFile {
            state: FileState::Hand,
            wired: Wired::dhcp(),
            manageable: false,
            error: Some(e),
        },
    }
}

/// Check the set that will be in force, the WiFi file only when there is
/// one.
fn check_set(h: &Homes, wifi: Option<&str>, wired: &str) -> Result<(), String> {
    let mut files = vec![(WIRED_FILE, wired)];
    if let Some(w) = wifi {
        files.push((WIFI_FILE, w));
    }
    check_netplan(&h.netplan, &files)
}

/// The machine's DNS servers as the WiFi settings are to carry them, when
/// the menu sets them there: those of `wired`, wired settings the menu keeps
/// (nothing set: none). `None`: the WiFi settings keep their own, beside
/// kept wired text or wired settings that cannot be read (the menu does not
/// take servers from text it does not own), and on netboot, which installs
/// no wired settings.
pub fn machine_dns(wired: &WiredFile, netboot: bool) -> Option<&[Ipv4Addr]> {
    match wired.state {
        _ if netboot => None,
        // A menu file edited into a form the menu cannot keep reads only
        // in part (a second address as DHCP, servers added to DHCP's as
        // a replacement): it counts as by hand here.
        FileState::Menu if wired.manageable => Some(wired.wired.dns()),
        FileState::Menu => None,
        FileState::Missing | FileState::Template => Some(&[]),
        FileState::Hand => None,
    }
}

/// The networks pass the input checks; else the first that fails, named.
pub fn check_saved(networks: &[Net]) -> Result<(), String> {
    for n in networks {
        check_net(n).map_err(|e| format!("WiFi, {}: {e}", n.ssid))?;
    }
    Ok(())
}

fn kept_refusal(what: &str) -> String {
    format!("The {what} settings are kept from your old settings; the menu cannot change them.")
}

fn address_strings(v: &[Ipv4Addr]) -> Vec<String> {
    v.iter().map(Ipv4Addr::to_string).collect()
}

/// Render `cand` as the generated files and let netplan check the set that
/// will be in force: the WiFi file only when there is one, the wired file
/// only off netboot (which installs none; the shipped default applies).
fn check_candidate(cand: &NetworkCfg, netboot: bool, h: &Homes, inputs: &Inputs) -> Result<(), String> {
    let full = crate::config::ThinfloeConfig {
        network: cand.clone(),
        ..Default::default()
    };
    let wifi = generated::render(&full, WIFI_FILE, inputs)?;
    let wired = match generated::render(&full, WIRED_FILE, inputs)? {
        Some(t) if !netboot => t,
        _ => h.default_text()?,
    };
    check_set(h, wifi.as_deref(), &wired)
}

/// The table `cfg` after change `c`, or why not. Nothing is written: the
/// caller stages the candidate. Refused when the part to change is kept
/// text, when netboot cannot do it, when an input check fails, when there is
/// no WiFi adapter for a network, or when netplan refuses the new set.
pub fn plan(cfg: &NetworkCfg, c: &Change, netboot: bool, h: &Homes, inputs: &Inputs) -> Result<NetworkCfg, String> {
    if c.wifi.is_none() && c.wired.is_none() {
        return Ok(cfg.clone());
    }
    let (wifi_now, wired_now) = view(cfg);
    if c.wifi.is_some() && wifi_now.state == FileState::Hand {
        return Err(kept_refusal("WiFi"));
    }
    if c.wired.is_some() && wired_now.state == FileState::Hand {
        return Err(kept_refusal("wired"));
    }
    if netboot {
        match &c.wired {
            Some(Wired::Static { .. }) => return Err("A fixed address cannot be set on netboot.".to_string()),
            Some(w) if !w.dns().is_empty() => return Err("DNS servers cannot be set on netboot.".to_string()),
            _ => {}
        }
    }
    // One DNS setting for the whole machine, kept with the wired settings
    // and copied into the WiFi settings the menu keeps: systemd-resolved
    // queries the DNS servers of all links, so servers set on the wired
    // link alone would still leak queries to the WiFi link's DHCP servers.
    // `None` (netboot, kept wired text): the WiFi settings keep the servers
    // they have.
    let dns: Option<&[Ipv4Addr]> = match &c.wired {
        _ if netboot => None,
        Some(w) => Some(w.dns()),
        None => machine_dns(&wired_now, netboot),
    };
    let mut cand = cfg.clone();
    match &c.wifi {
        None => {
            if let Some(dns) = dns
                && wifi_now.state == FileState::Menu
                && !wifi_now.networks.is_empty()
                && wifi_now.dns != dns
            {
                // A network the checks refuse would be dropped, so it
                // stops the rewrite instead.
                check_saved(&wifi_now.networks)?;
                cand.wifi_dns = address_strings(dns);
            }
        }
        Some(nets) => {
            for n in nets {
                check_net(n)?;
            }
            let devices = wifi_devices(&h.sys_net);
            if !nets.is_empty() && devices.is_empty() {
                return Err("No WiFi adapter found".to_string());
            }
            cand.wifi_device = devices.first().cloned();
            cand.set_nets(nets);
            if let Some(dns) = dns {
                cand.wifi_dns = address_strings(dns);
            }
        }
    }
    if let Some(w) = &c.wired {
        cand.set_wired(w);
    }
    check_candidate(&cand, netboot, h, inputs)?;
    Ok(cand)
}

/// *Let the menu manage it*: the kept text `which` ("wifi" or "wired")
/// becomes the menu's own settings, with the same networks and passwords
/// (device kept) or the same addressing. Checked as `plan` checks; the
/// caller stores the candidate.
pub fn take_over(cfg: &NetworkCfg, which: &str, h: &Homes, inputs: &Inputs) -> Result<NetworkCfg, String> {
    let (wifi_now, wired_now) = view(cfg);
    let (what, kept, manageable) = match which {
        "wifi" => ("WiFi", wifi_now.state == FileState::Hand, wifi_now.manageable),
        "wired" => ("wired", wired_now.state == FileState::Hand, wired_now.manageable),
        _ => return Err(format!("Unknown network setting: {which}")),
    };
    if !kept {
        return Err(format!("The {what} settings are not kept from your old settings."));
    }
    if !manageable {
        return Err(format!("The kept {what} settings have something the menu cannot keep."));
    }
    let mut cand = cfg.clone();
    if which == "wifi" {
        check_saved(&wifi_now.networks)?;
        cand.kept_wifi = None;
        cand.wifi_device = wifi_now.device;
        cand.set_nets(&wifi_now.networks);
        cand.wifi_dns = address_strings(&wifi_now.dns);
    } else {
        cand.kept_wired = None;
        cand.set_wired(&wired_now.wired);
    }
    check_candidate(&cand, inputs.netboot, h, inputs)?;
    Ok(cand)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use yaml_rust2::{Yaml, YamlLoader};

    const SHIPPED_WIRED: &str = include_str!("../../../overlay/etc/netplan/ethernet.yaml");
    const SHIPPED_WIFI: &str = include_str!("../../../overlay/home/tfuser/wifi.yaml");

    /// Ruling 23: netplan by its full path, whatever PATH the session has.
    #[test]
    fn netplan_runs_by_its_full_path() {
        if std::env::var_os("TF_NETPLAN").is_none() {
            assert_eq!(Homes::from_env().netplan, "/usr/sbin/netplan");
        }
    }

    /// Stands in for netplan: records what it was asked to check, and fails
    /// like netplan when the file `fail` lies beside it.
    const FAKE_NETPLAN: &str = r#"#!/bin/sh
d=$(dirname "$0")
echo "$3" > "$d/root"
ls "$3/etc/netplan" > "$d/names"
stat -c %a "$3"/etc/netplan/* > "$d/modes"
cat "$3"/etc/netplan/* > "$d/checked"
if [ -e "$d/slow" ]; then exec sleep 5; fi
if [ -e "$d/fail" ]; then
    echo "some noise" >&2
    echo "ERROR: x: bad" >&2
    echo "more noise" >&2
    exit 1
fi
if [ -e "$d/fail-in-file" ]; then
    echo "$3/etc/netplan/wifi.yaml: Error in network definition: wlp1: No access points defined" >&2
    echo "Failed to send reload request: Permission denied" >&2
    echo "Command '['systemctl', 'daemon-reload', '--no-ask-password']' returned non-zero exit status 1." >&2
    exit 1
fi
if [ -e "$d/fail-quietly" ]; then
    echo "first line" >&2
    echo "last line" >&2
    exit 1
fi
exit 0
"#;

    struct Fixture {
        d: TestDir,
        h: Homes,
    }

    impl Fixture {
        fn new(tag: &str, devices: &[&str]) -> Fixture {
            let d = TestDir::new("netfiles", tag);
            let home = d.join("home");
            fs::create_dir_all(&home).unwrap();
            let sys_net = d.join("sys/class/net");
            fs::create_dir_all(sys_net.join("eno1")).unwrap();
            for dev in devices {
                fs::create_dir_all(sys_net.join(dev).join("wireless")).unwrap();
            }
            let bin = d.join("bin");
            fs::create_dir_all(&bin).unwrap();
            let netplan = bin.join("netplan");
            fs::write(&netplan, FAKE_NETPLAN).unwrap();
            fs::set_permissions(&netplan, fs::Permissions::from_mode(0o755)).unwrap();
            let default_wired = d.join("usr-share-ethernet.yaml");
            fs::write(&default_wired, SHIPPED_WIRED).unwrap();
            let h = Homes {
                home,
                sys_net,
                netplan: netplan.to_str().unwrap().to_string(),
                default_wired,
            };
            Fixture { d, h }
        }

        fn flag(&self, name: &str) {
            fs::write(self.d.join("bin").join(name), "").unwrap();
        }

        fn recorded(&self, name: &str) -> String {
            fs::read_to_string(self.d.join("bin").join(name)).unwrap_or_default()
        }
    }

    fn net(ssid: &str, password: Option<&str>, hidden: bool) -> Net {
        Net {
            ssid: ssid.to_string(),
            hidden,
            password: password.map(str::to_string),
        }
    }

    fn fixed() -> Wired {
        Wired::Static {
            address: Ipv4Addr::new(192, 168, 1, 10),
            prefix: 24,
            gateway: Some(Ipv4Addr::new(192, 168, 1, 1)),
            dns: vec![Ipv4Addr::new(1, 1, 1, 1), Ipv4Addr::new(9, 9, 9, 9)],
        }
    }

    fn yaml(text: &str) -> Yaml {
        YamlLoader::load_from_str(text).unwrap().remove(0)
    }

    fn keys(y: &Yaml) -> Vec<String> {
        y.as_hash()
            .unwrap()
            .keys()
            .map(|k| k.as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn template_rules() {
        assert_eq!(state(Some(SHIPPED_WIFI), true), FileState::Template);
        assert_eq!(state(Some(""), true), FileState::Template);
        assert_eq!(state(Some("# a\n\n   # b\n  \n"), true), FileState::Template);
        assert_eq!(
            state(Some(&format!("{MENU_HEADER}\nnetwork: {{}}\n")), true),
            FileState::Menu
        );
        assert_eq!(state(Some(&format!("{MENU_HEADER}\n")), true), FileState::Menu);
        assert_eq!(state(Some("network: {}\n"), true), FileState::Hand);
        assert_eq!(
            state(Some(&format!("network: {{}}\n{MENU_HEADER}\n")), true),
            FileState::Hand
        );
        assert_eq!(state(Some(""), false), FileState::Hand);
        assert_eq!(state(Some(SHIPPED_WIFI), false), FileState::Hand);
        assert_eq!(state(None, true), FileState::Missing);
        assert!(is_template(SHIPPED_WIFI));
        assert!(!is_template("network: {}\n"));
    }

    #[test]
    fn render_wifi_is_optional() {
        let t = render_wifi(Some("wlp58s0"), &[net("home", Some("secret123"), false)], &[]);
        assert!(t.starts_with(&format!("{MENU_HEADER}\n")), "{t}");
        assert!(!t.contains("auth"), "{t}");
        assert!(!t.contains(TEMPLATE_MARKER), "{t}");
        let y = yaml(&t);
        assert_eq!(y["network"]["version"], Yaml::Integer(2));
        assert_eq!(y["network"]["renderer"].as_str(), Some("networkd"));
        assert_eq!(keys(&y["network"]["wifis"]), ["wlp58s0"]);
        let def = &y["network"]["wifis"]["wlp58s0"];
        assert_eq!(def["dhcp4"], Yaml::Boolean(true));
        assert_eq!(def["optional"], Yaml::Boolean(true));
        assert_eq!(def["access-points"]["home"]["password"].as_str(), Some("secret123"));
        assert_eq!(keys(&def["access-points"]["home"]), ["password"]);
    }

    #[test]
    fn render_wifi_without_networks_is_bare() {
        for t in [
            render_wifi(Some("wlp58s0"), &[], &[]),
            render_wifi(None, &[net("home", Some("secret123"), false)], &[]),
        ] {
            assert!(t.starts_with(&format!("{MENU_HEADER}\n")), "{t}");
            let y = yaml(&t);
            assert_eq!(keys(&y), ["network"]);
            assert_eq!(keys(&y["network"]), ["version", "renderer"]);
            assert_eq!(y["network"]["version"], Yaml::Integer(2));
            assert_eq!(y["network"]["renderer"].as_str(), Some("networkd"));
        }
    }

    #[test]
    fn render_round_trips_odd_text() {
        let pw = "p@ss: \"x\" #1\\";
        let mut nets: Vec<Net> = ["Café \"Nord\"", " lead", "a:b #c", "yes", "123"]
            .iter()
            .enumerate()
            .map(|(i, s)| net(s, Some(pw), i % 2 == 0))
            .collect();
        nets.push(net("open", None, false));
        nets.push(net("hidden open", None, true));
        let t = render_wifi(Some("wlp58s0"), &nets, &[]);
        let f = parse_wifi(Some(&t));
        assert_eq!(f.error, None);
        assert_eq!(f.state, FileState::Menu);
        assert!(f.manageable);
        assert_eq!(f.device.as_deref(), Some("wlp58s0"));
        assert_eq!(f.networks, nets);
        // A plain YAML reader sees strings only, as netplan does.
        let y = yaml(&t);
        let aps = y["network"]["wifis"]["wlp58s0"]["access-points"].as_hash().unwrap();
        assert_eq!(aps.len(), nets.len());
        for (k, v) in aps {
            assert!(matches!(k, Yaml::String(_)), "{k:?}");
            if let Some(p) = v.as_hash().and_then(|h| h.get(&Yaml::String("password".into()))) {
                assert_eq!(p, &Yaml::String(pw.to_string()));
            }
        }
        assert_eq!(aps[&Yaml::String("open".into())], Yaml::Hash(Default::default()));
    }

    #[test]
    fn render_wired_keeps_eth_match_optional() {
        let t = render_wired(SHIPPED_WIRED, &fixed()).unwrap().unwrap();
        assert!(t.starts_with(&format!("{MENU_HEADER}\n")), "{t}");
        let y = yaml(&t);
        assert_eq!(y["network"]["version"], Yaml::Integer(2));
        assert_eq!(y["network"]["renderer"].as_str(), Some("networkd"));
        assert_eq!(keys(&y["network"]["ethernets"]), ["eth"]);
        let eth = &y["network"]["ethernets"]["eth"];
        assert_eq!(eth["match"]["name"].as_str(), Some("en*"));
        assert_eq!(eth["optional"], Yaml::Boolean(true));
        assert_eq!(eth["dhcp4"], Yaml::Boolean(false));
        assert_eq!(eth["addresses"][0].as_str(), Some("192.168.1.10/24"));
        assert_eq!(eth["routes"][0]["to"].as_str(), Some("default"));
        assert_eq!(eth["routes"][0]["via"].as_str(), Some("192.168.1.1"));
        assert_eq!(eth["nameservers"]["addresses"][0].as_str(), Some("1.1.1.1"));
        assert_eq!(eth["nameservers"]["addresses"][1].as_str(), Some("9.9.9.9"));

        let bare = Wired::Static {
            address: Ipv4Addr::new(10, 0, 0, 5),
            prefix: 8,
            gateway: None,
            dns: vec![],
        };
        let y = yaml(&render_wired(SHIPPED_WIRED, &bare).unwrap().unwrap());
        let eth = &y["network"]["ethernets"]["eth"];
        assert_eq!(keys(eth), ["match", "dhcp4", "optional", "addresses"]);
        assert_eq!(render_wired(SHIPPED_WIRED, &Wired::dhcp()), Ok(None));
        assert!(render_wired("network: [\n", &fixed()).is_err());
        assert!(render_wired("network: {version: 2}\n", &fixed()).is_err());
    }

    #[test]
    fn parse_wifi_manageable_rules() {
        let good = "network:\n  version: 2\n  renderer: networkd\n  wifis:\n    wlp2s0:\n      dhcp4: true\n      access-points:\n        \"home\":\n          password: 00012345\n          hidden: true\n        cafe:\n        bar: {}\n";
        let f = parse_wifi(Some(good));
        assert_eq!(f.state, FileState::Hand);
        assert!(f.manageable);
        assert_eq!(f.error, None);
        assert_eq!(f.device.as_deref(), Some("wlp2s0"));
        assert_eq!(
            f.networks,
            [
                net("home", Some("00012345"), true),
                net("cafe", None, false),
                net("bar", None, false)
            ]
        );

        let auth = good.replace("hidden: true", "auth:\n            key-management: psk");
        let f = parse_wifi(Some(&auth));
        assert!(!f.manageable);
        assert_eq!(f.error, None);
        assert_eq!(
            f.networks,
            [
                net("home", None, false),
                net("cafe", None, false),
                net("bar", None, false)
            ]
        );

        // Ruling 22: a menu file keeps the passwords of the networks it
        // can keep; a hand file loses them all (above).
        let menu = format!(
            "{MENU_HEADER}\n{}",
            good.replace(
                "cafe:\n",
                "cafe:\n          password: secret123\n          auth:\n            key-management: psk\n"
            )
        );
        let f = parse_wifi(Some(&menu));
        assert_eq!(f.state, FileState::Menu);
        assert!(!f.manageable);
        assert_eq!(
            f.networks,
            [
                net("home", Some("00012345"), true),
                net("cafe", None, false),
                net("bar", None, false)
            ]
        );

        let two = format!("{good}    wlp3s0:\n      access-points:\n        other: {{}}\n");
        let f = parse_wifi(Some(&two));
        assert!(!f.manageable);
        assert_eq!(f.device, None);
        let names: Vec<_> = f.networks.iter().map(|n| n.ssid.as_str()).collect();
        assert_eq!(names, ["home", "cafe", "bar", "other"]);
        assert!(f.networks.iter().all(|n| n.password.is_none()));

        let extra = format!("{good}  ethernets: {{}}\n");
        assert!(!parse_wifi(Some(&extra)).manageable);

        let broken = parse_wifi(Some("network: [\n"));
        assert!(!broken.manageable);
        assert!(broken.networks.is_empty());
        assert!(
            broken.error.as_deref().unwrap().starts_with("WiFi settings: "),
            "{broken:?}"
        );

        let t = parse_wifi(Some(SHIPPED_WIFI));
        assert_eq!(t.state, FileState::Template);
        assert!(t.networks.is_empty());
        assert_eq!(t.error, None);
        let m = parse_wifi(None);
        assert_eq!(m.state, FileState::Missing);
        assert!(m.networks.is_empty());
    }

    #[test]
    fn parse_wired_reads_back_render_wired() {
        let bare = Wired::Static {
            address: Ipv4Addr::new(10, 0, 0, 5),
            prefix: 8,
            gateway: None,
            dns: vec![],
        };
        for w in [fixed(), bare] {
            let t = render_wired(SHIPPED_WIRED, &w).unwrap().unwrap();
            let f = parse_wired(Some(&t));
            assert_eq!(f.state, FileState::Menu);
            assert!(f.manageable, "{t}");
            assert_eq!(f.error, None);
            assert_eq!(f.wired, w);
        }
        let shipped = parse_wired(Some(SHIPPED_WIRED));
        assert_eq!(shipped.state, FileState::Hand);
        assert!(shipped.manageable);
        assert_eq!(shipped.wired, Wired::dhcp());
        assert_eq!(parse_wired(None).state, FileState::Missing);
        assert_eq!(parse_wired(None).wired, Wired::dhcp());
        assert_eq!(parse_wired(Some("")).state, FileState::Hand);

        let mtu = SHIPPED_WIRED.replace("optional: true", "optional: true\n            mtu: 9000");
        assert!(!parse_wired(Some(&mtu)).manageable);
        let two = format!("{SHIPPED_WIRED}        eth2:\n            dhcp4: true\n");
        assert!(!parse_wired(Some(&two)).manageable);
        let renamed = SHIPPED_WIRED.replace("        eth:", "        lan:");
        assert!(!parse_wired(Some(&renamed)).manageable);
        let broken = parse_wired(Some("network: [\n"));
        assert!(!broken.manageable);
        assert!(broken.error.as_deref().unwrap().starts_with("Wired settings: "));
    }

    #[test]
    fn wifi_devices_are_sorted_and_wireless_only() {
        let f = Fixture::new("devices", &["wlp58s0", "wlan0"]);
        assert_eq!(wifi_devices(&f.h.sys_net), ["wlan0", "wlp58s0"]);
        assert!(wifi_devices(&f.d.join("nowhere")).is_empty());
    }

    #[test]
    fn check_netplan_writes_private_files_and_cleans_up() {
        let f = Fixture::new("check", &[]);
        check_netplan(&f.h.netplan, &[("wifi.yaml", "a: 1\n"), ("ethernet.yaml", "b: 2\n")]).unwrap();
        assert_eq!(f.recorded("names"), "ethernet.yaml\nwifi.yaml\n");
        assert_eq!(f.recorded("modes"), "600\n600\n");
        let root = f.recorded("root");
        assert!(!root.trim().is_empty());
        assert!(!Path::new(root.trim()).exists(), "{root}");

        f.flag("fail");
        assert_eq!(
            check_netplan(&f.h.netplan, &[("wifi.yaml", "a: 1\n")]),
            Err("ERROR: x: bad".into())
        );
        assert!(!Path::new(f.recorded("root").trim()).exists());
        fs::remove_file(f.d.join("bin/fail")).unwrap();
        f.flag("fail-in-file");
        assert_eq!(
            check_netplan(&f.h.netplan, &[("wifi.yaml", "a: 1\n")]),
            Err("wifi.yaml: Error in network definition: wlp1: No access points defined".into())
        );
        fs::remove_file(f.d.join("bin/fail-in-file")).unwrap();
        f.flag("fail-quietly");
        assert_eq!(
            check_netplan(&f.h.netplan, &[("wifi.yaml", "a: 1\n")]),
            Err("last line".into())
        );
        assert!(check_netplan(&f.d.join("bin/none").to_string_lossy(), &[]).is_err());
    }

    #[test]
    fn check_netplan_gives_up_after_its_timeout() {
        let f = Fixture::new("check-slow", &[]);
        f.flag("slow");
        let started = std::time::Instant::now();
        let e = check_netplan_within(
            &f.h.netplan,
            &[("wifi.yaml", "a: 1\n")],
            std::time::Duration::from_millis(300),
        )
        .unwrap_err();
        assert!(e.contains("did not finish"), "{e}");
        assert!(started.elapsed() < std::time::Duration::from_secs(4));
        assert!(!Path::new(f.recorded("root").trim()).exists());
    }

    fn inputs(netboot: bool) -> Inputs {
        Inputs {
            default_wired: SHIPPED_WIRED.to_string(),
            netboot,
        }
    }

    fn net_cfg(nets: &[Net]) -> NetworkCfg {
        let mut c = NetworkCfg {
            wifi_device: Some("wlp58s0".into()),
            ..NetworkCfg::default()
        };
        c.set_nets(nets);
        c
    }

    fn strs(v: &[Ipv4Addr]) -> Vec<String> {
        v.iter().map(Ipv4Addr::to_string).collect()
    }

    fn plan_in(f: &Fixture, cfg: &NetworkCfg, c: &Change, netboot: bool) -> Result<NetworkCfg, String> {
        plan(cfg, c, netboot, &f.h, &inputs(netboot))
    }

    fn wifi_change(nets: Vec<Net>) -> Change {
        Change {
            wifi: Some(nets),
            wired: None,
        }
    }

    fn wired_change(w: Wired) -> Change {
        Change {
            wifi: None,
            wired: Some(w),
        }
    }

    const HAND_WIFI: &str = "# my own\nnetwork:\n  version: 2\n  renderer: networkd\n  wifis:\n    wlp2s0:\n      dhcp4: true\n      access-points:\n        home:\n          password: \"secret123\"\n        hidden one:\n          password: 'p#ss word'\n          hidden: true\n        cafe: {}\n";

    #[test]
    fn view_reads_the_table() {
        let (w, e) = view(&NetworkCfg::default());
        assert_eq!((w.state, e.state), (FileState::Missing, FileState::Missing));
        assert!(w.manageable && e.manageable);
        assert_eq!(e.wired, Wired::dhcp());
        let nets = vec![net("home", Some("secret123"), true)];
        let mut c = net_cfg(&nets);
        c.wifi_dns = strs(&dns2());
        c.set_wired(&fixed());
        let (w, e) = view(&c);
        assert_eq!(w.state, FileState::Menu);
        assert_eq!(w.device.as_deref(), Some("wlp58s0"));
        assert_eq!((&w.networks, &w.dns), (&nets, &dns2()));
        assert_eq!((e.state, e.wired), (FileState::Menu, fixed()));
        // Servers alone make the wired settings the menu's.
        c.set_wired(&dhcp_dns());
        assert_eq!(view(&c).1.state, FileState::Menu);
        // A table the menu cannot render reads as kept: nothing to edit.
        c.wired.mode = "bogus".into();
        let e = view(&c).1;
        assert_eq!((e.state, e.manageable), (FileState::Hand, false));
        assert!(e.error.is_some());
    }

    #[test]
    fn view_shows_kept_text_as_kept() {
        let mtu = SHIPPED_WIRED.replace("optional: true", "optional: true\n            mtu: 9000");
        let mut c = NetworkCfg {
            kept_wifi: Some(HAND_WIFI.into()),
            kept_wired: Some(mtu),
            ..NetworkCfg::default()
        };
        let (w, e) = view(&c);
        assert_eq!((w.state, w.manageable), (FileState::Hand, true));
        assert_eq!(w.networks.len(), 3);
        assert_eq!((e.state, e.manageable), (FileState::Hand, false));
        c.kept_wifi = Some(HAND_WIFI.replace("dhcp4: true", "dhcp4: false"));
        assert!(!view(&c).0.manageable);
    }

    #[test]
    fn plan_without_a_change_is_the_table_itself() {
        let f = Fixture::new("plan-noop", &["wlp58s0"]);
        let cfg = net_cfg(&[net("home", Some("secret123"), false)]);
        assert_eq!(plan_in(&f, &cfg, &Change::default(), false), Ok(cfg));
        assert_eq!(f.recorded("root"), "", "netplan was not asked");
    }

    #[test]
    fn plan_writes_the_current_device_and_checks_the_set() {
        let f = Fixture::new("plan-device", &["wlp58s0"]);
        let nets = vec![net("home", Some("secret123"), false), net("cafe", None, true)];
        let mut cfg = net_cfg(&[]);
        cfg.wifi_device = Some("wlp2s0".into());
        let got = plan_in(&f, &cfg, &wifi_change(nets.clone()), false).unwrap();
        assert_eq!(got.wifi_device.as_deref(), Some("wlp58s0"));
        assert_eq!(got.nets(), nets);
        // No wired change: the check used the default beside the new WiFi file.
        assert!(f.recorded("checked").contains("dhcp4: true"));
        assert!(f.recorded("checked").contains("secret123"));
        assert_eq!(f.recorded("names"), "ethernet.yaml\nwifi.yaml\n");
        // A wired change is checked as rendered.
        plan_in(&f, &got, &wired_change(fixed()), false).unwrap();
        assert!(f.recorded("checked").contains("192.168.1.10/24"));
        assert!(f.recorded("checked").contains("secret123"));
    }

    #[test]
    fn plan_refuses_what_netplan_refuses() {
        let f = Fixture::new("plan-refused", &["wlp58s0"]);
        f.flag("fail");
        let cfg = net_cfg(&[net("home", Some("secret123"), false)]);
        let c = Change {
            wifi: Some(vec![net("cafe", Some("password9"), false)]),
            wired: Some(fixed()),
        };
        assert_eq!(plan_in(&f, &cfg, &c, false), Err("ERROR: x: bad".into()));
        assert_eq!(
            plan_in(&f, &cfg, &wired_change(Wired::dhcp()), false),
            Err("ERROR: x: bad".into())
        );
    }

    #[test]
    fn plan_never_touches_kept_text() {
        let f = Fixture::new("plan-kept", &["wlp58s0"]);
        let cfg = NetworkCfg {
            kept_wifi: Some(HAND_WIFI.into()),
            kept_wired: Some(SHIPPED_WIRED.into()),
            ..NetworkCfg::default()
        };
        let e = plan_in(&f, &cfg, &wifi_change(vec![]), false).unwrap_err();
        assert!(e.contains("kept from your old settings"), "{e}");
        for w in [fixed(), Wired::dhcp()] {
            let e = plan_in(&f, &cfg, &wired_change(w), false).unwrap_err();
            assert!(e.contains("kept from your old settings"), "{e}");
        }
        assert_eq!(f.recorded("root"), "");
    }

    #[test]
    fn plan_static_and_servers_refused_on_netboot() {
        let f = Fixture::new("plan-netboot", &["wlp58s0"]);
        let cfg = NetworkCfg::default();
        assert_eq!(
            plan_in(&f, &cfg, &wired_change(fixed()), true),
            Err("A fixed address cannot be set on netboot.".into())
        );
        assert_eq!(
            plan_in(&f, &cfg, &wired_change(dhcp_dns()), true),
            Err("DNS servers cannot be set on netboot.".into())
        );
        assert_eq!(plan_in(&f, &cfg, &wired_change(Wired::dhcp()), true), Ok(cfg.clone()));
        // Wired settings left from a stick boot are not installed on
        // netboot, but the generated files are still rendered from them,
        // and that refuses: the WiFi change waits for a stick boot.
        let mut stick = net_cfg(&[net("home", Some("secret123"), false)]);
        stick.set_wired(&fixed());
        stick.wifi_dns = strs(fixed().dns());
        assert_eq!(
            plan_in(&f, &stick, &wifi_change(vec![net("lab", None, false)]), true),
            Err("A fixed address cannot be set on netboot.".into())
        );
        // Going back from a fixed address leaves the WiFi servers.
        let got = plan_in(&f, &stick, &wired_change(Wired::dhcp()), true).unwrap();
        assert_eq!(got.wired, NetworkCfg::default().wired);
        assert_eq!(got.wifi_dns, stick.wifi_dns);
    }

    #[test]
    fn plan_without_adapter_fails() {
        let f = Fixture::new("plan-no-adapter", &[]);
        let cfg = NetworkCfg::default();
        assert_eq!(
            plan_in(
                &f,
                &cfg,
                &wifi_change(vec![net("home", Some("secret123"), false)]),
                false
            ),
            Err("No WiFi adapter found".into())
        );
        assert_eq!(f.recorded("root"), "");
        let got = plan_in(&f, &cfg, &wifi_change(vec![]), false).unwrap();
        assert!(got.networks.is_empty());
    }

    #[test]
    fn plan_refuses_bad_input() {
        let f = Fixture::new("plan-bad-input", &["wlp58s0"]);
        for n in [
            net("home", Some("short"), false),
            net("", None, false),
            net(TEMPLATE_MARKER, None, false),
            net("back\\slash", None, false),
            net("line\u{2028}sep", None, false),
            net("home", Some("secret\u{ffff}123"), false),
        ] {
            assert!(plan_in(&f, &NetworkCfg::default(), &wifi_change(vec![n]), false).is_err());
        }
        assert_eq!(f.recorded("root"), "");
    }

    #[test]
    fn plan_dhcp_dns_goes_to_both() {
        let f = Fixture::new("plan-dns", &["wlp58s0"]);
        let nets = vec![net("home", Some("secret123"), false), net("cafe", None, true)];
        let cfg = net_cfg(&nets);
        let one = plan_in(&f, &cfg, &wired_change(dhcp_dns()), false).unwrap();
        assert_eq!(one.wired.dns, strs(&dns2()));
        assert_eq!(one.wifi_dns, strs(&dns2()));
        assert_eq!(one.nets(), nets, "passwords and hidden kept");
        assert!(f.recorded("checked").contains("use-dns: false"));
        // A WiFi change takes the wired servers.
        let two = plan_in(&f, &one, &wifi_change(vec![net("lab", None, false)]), false).unwrap();
        assert_eq!(two.nets(), [net("lab", None, false)]);
        assert_eq!(two.wifi_dns, strs(&dns2()));
        // Fixed-address servers reach WiFi too.
        let three = plan_in(&f, &two, &wired_change(fixed()), false).unwrap();
        assert_eq!(three.wifi_dns, strs(&dns2()));
        assert_eq!(three.wired.mode, "static");
        // Back to the servers DHCP gives: both lose them.
        let four = plan_in(&f, &three, &wired_change(Wired::dhcp()), false).unwrap();
        assert_eq!(four.wired, NetworkCfg::default().wired);
        assert!(four.wifi_dns.is_empty());
        assert_eq!(four.nets(), [net("lab", None, false)]);
    }

    #[test]
    fn plan_dns_leaves_other_wifi_settings_alone() {
        let f = Fixture::new("plan-dns-alone", &["wlp58s0"]);
        // No networks: nothing to carry the servers.
        let got = plan_in(&f, &NetworkCfg::default(), &wired_change(dhcp_dns()), false).unwrap();
        assert_eq!(got.wired.dns, strs(&dns2()));
        assert!(got.wifi_dns.is_empty() && got.networks.is_empty());
        // Kept WiFi text stays as it is.
        let kept = NetworkCfg {
            kept_wifi: Some(HAND_WIFI.into()),
            ..NetworkCfg::default()
        };
        let got = plan_in(&f, &kept, &wired_change(dhcp_dns()), false).unwrap();
        assert_eq!(got.kept_wifi.as_deref(), Some(HAND_WIFI));
        assert!(got.wifi_dns.is_empty());
    }

    #[test]
    fn a_dns_rewrite_never_drops_a_network() {
        let f = Fixture::new("plan-dns-drop", &["wlp58s0"]);
        let cfg = net_cfg(&[net("home", Some("short12"), false), net("cafe", None, false)]);
        assert_eq!(
            plan_in(&f, &cfg, &wired_change(dhcp_dns()), false),
            Err("WiFi, home: The password needs 8 to 63 characters.".into())
        );
    }

    /// Only wired settings the menu keeps give WiFi its servers; beside kept
    /// wired text the WiFi settings keep their own.
    #[test]
    fn wifi_keeps_its_dns_beside_kept_wired_text() {
        let f = Fixture::new("plan-dns-kept-wired", &["wlp58s0"]);
        let additive = format!("{SHIPPED_WIRED}            nameservers:\n                addresses: [8.8.8.8]\n");
        for own in [strs(&dns2()), vec![]] {
            let mut cfg = net_cfg(&[net("home", None, false)]);
            cfg.wifi_dns = own.clone();
            cfg.kept_wired = Some(additive.clone());
            let got = plan_in(&f, &cfg, &wifi_change(vec![net("lab", None, false)]), false).unwrap();
            assert_eq!((got.nets(), got.wifi_dns), (vec![net("lab", None, false)], own));
            assert_eq!(got.kept_wired.as_deref(), Some(additive.as_str()));
        }
    }

    #[test]
    fn plan_keeps_the_wifi_dns_on_netboot() {
        let f = Fixture::new("plan-dns-netboot", &["wlp58s0"]);
        let nets = vec![net("home", Some("secret123"), false)];
        let mut cfg = net_cfg(&nets);
        cfg.wifi_dns = strs(&dns2());
        // Netboot sets no servers, and takes none from the WiFi change.
        let e = plan_in(&f, &cfg, &wired_change(dhcp_dns()), true).unwrap_err();
        assert!(e.contains("netboot"), "{e}");
        let got = plan_in(&f, &cfg, &wifi_change(nets.clone()), true).unwrap();
        assert_eq!(got, cfg, "nothing changes, nothing is lost");
        let mut other = net_cfg(&nets);
        other.wifi_dns = vec![];
        let got = plan_in(&f, &other, &wifi_change(nets), true).unwrap();
        assert!(got.wifi_dns.is_empty());
    }

    fn kept(wifi: Option<&str>, wired: Option<&str>) -> NetworkCfg {
        NetworkCfg {
            kept_wifi: wifi.map(str::to_string),
            kept_wired: wired.map(str::to_string),
            ..NetworkCfg::default()
        }
    }

    #[test]
    fn take_over_keeps_networks_and_passwords() {
        let f = Fixture::new("take", &["wlp58s0"]);
        let cfg = kept(Some(HAND_WIFI), None);
        let got = take_over(&cfg, "wifi", &f.h, &inputs(false)).unwrap();
        assert_eq!(got.kept_wifi, None);
        assert_eq!(got.wifi_device.as_deref(), Some("wlp2s0"));
        assert_eq!(
            got.nets(),
            [
                net("home", Some("secret123"), false),
                net("hidden one", Some("p#ss word"), true),
                net("cafe", None, false)
            ]
        );
        assert!(f.recorded("names").contains(WIFI_FILE), "checked with netplan");

        let hand_wired = SHIPPED_WIRED.replace(
            "dhcp4: true",
            "dhcp4: false\n            addresses: [192.168.1.10/24]\n            routes:\n              - to: default\n                via: 192.168.1.1\n            nameservers:\n              addresses: [1.1.1.1, 9.9.9.9]",
        );
        let got = take_over(&kept(None, Some(&hand_wired)), "wired", &f.h, &inputs(false)).unwrap();
        assert_eq!(got.kept_wired, None);
        assert_eq!(got.wired().unwrap(), fixed());

        // A kept file that says DHCP becomes the default.
        let got = take_over(&kept(None, Some(SHIPPED_WIRED)), "wired", &f.h, &inputs(false)).unwrap();
        assert_eq!((got.kept_wired, got.wired), (None, NetworkCfg::default().wired));
        // The WiFi text beside it stays kept.
        let both = kept(Some(HAND_WIFI), Some(SHIPPED_WIRED));
        let got = take_over(&both, "wired", &f.h, &inputs(false)).unwrap();
        assert_eq!(got.kept_wifi.as_deref(), Some(HAND_WIFI));
    }

    /// Ruling 25: a take-over keeps the content, so kept text that matches
    /// another NIC, or is not optional, is not the menu's to manage.
    #[test]
    fn wired_take_over_needs_the_default_match_and_optional() {
        assert!(parse_wired(Some(SHIPPED_WIRED)).manageable);
        let bare = "network:\n  version: 2\n  ethernets:\n    eth:\n      dhcp4: true\n";
        assert!(parse_wired(Some(bare)).manageable, "absent is the default");
        for other in [
            SHIPPED_WIRED.replace("name: 'en*'", "macaddress: '00:11:22:33:44:55'"),
            SHIPPED_WIRED.replace("name: 'en*'", "name: eno1"),
            SHIPPED_WIRED.replace("name: 'en*'", "name: 'en*'\n                driver: e1000e"),
            SHIPPED_WIRED.replace("optional: true", "optional: false"),
        ] {
            assert!(!parse_wired(Some(&other)).manageable, "{other}");
        }
        let f = Fixture::new("take-wired-match", &["wlp58s0"]);
        let mac = SHIPPED_WIRED.replace("name: 'en*'", "macaddress: '00:11:22:33:44:55'");
        let e = take_over(&kept(None, Some(&mac)), "wired", &f.h, &inputs(false)).unwrap_err();
        assert!(e.contains("cannot keep"), "{e}");
    }

    /// The menu writes `dhcp4: true` and `optional: true` on its WiFi
    /// definition, so a take-over keeps only a definition that has DHCP
    /// and is not marked as required for boot.
    #[test]
    fn wifi_take_over_needs_dhcp_and_optional() {
        assert!(parse_wifi(Some(HAND_WIFI)).manageable);
        let yes = HAND_WIFI.replace("dhcp4: true", "dhcp4: yes");
        assert!(parse_wifi(Some(&yes)).manageable, "{yes}");
        let opt = HAND_WIFI.replace("dhcp4: true", "dhcp4: true\n      optional: true");
        assert!(parse_wifi(Some(&opt)).manageable, "{opt}");
        for other in [
            HAND_WIFI.replace("      dhcp4: true\n", ""),
            HAND_WIFI.replace("dhcp4: true", "dhcp4: false"),
            HAND_WIFI.replace("dhcp4: true", "dhcp4: maybe"),
            HAND_WIFI.replace("dhcp4: true", "dhcp4: true\n      optional: false"),
            HAND_WIFI.replace("dhcp4: true", "dhcp4: true\n      optional: sometimes"),
        ] {
            let w = parse_wifi(Some(&other));
            assert!(!w.manageable, "{other}");
            // Still shown, without the passwords.
            assert_eq!(w.networks.len(), 3, "{other}");
            assert!(w.networks.iter().all(|n| n.password.is_none()), "{other}");
        }
        let f = Fixture::new("take-wifi-static", &["wlp58s0"]);
        let cfg = kept(Some(&HAND_WIFI.replace("      dhcp4: true\n", "")), None);
        let e = take_over(&cfg, "wifi", &f.h, &inputs(false)).unwrap_err();
        assert!(e.contains("cannot keep"), "{e}");
    }

    /// A file without WiFi, as the menu writes it with no device or no
    /// networks, is manageable; an empty or second `wifis` entry is not.
    #[test]
    fn a_file_without_wifi_is_manageable() {
        for t in [
            render_wifi(Some("wlp58s0"), &[], &[]),
            render_wifi(None, &[net("home", Some("secret123"), false)], &[]),
        ] {
            let w = parse_wifi(Some(&t));
            assert_eq!(w.state, FileState::Menu);
            assert!(w.manageable, "{t}");
            assert_eq!(
                (&w.device, &w.networks, &w.dns, &w.error),
                (&None, &vec![], &vec![], &None)
            );
            // Manageable, but with no network to carry DNS servers, it is
            // never out of step with the wired ones.
            let wired = parse_wired(render_wired(SHIPPED_WIRED, &fixed()).unwrap().as_deref());
            let mut tab = crate::network::NetTab::new(false);
            tab.loaded(w, wired, vec!["wlp58s0".to_string()]);
            assert!(!tab.wifi_dns_out_of_step() && !tab.dirty(), "{t}");
        }
        let bare = "network:\n  version: 2\n  renderer: networkd\n";
        let w = parse_wifi(Some(bare));
        assert_eq!(w.state, FileState::Hand);
        assert!(w.manageable);
        for other in [
            format!("{bare}  wifis: {{}}\n"),
            format!("{bare}  wifis:\n"),
            format!("{bare}  wifis: []\n"),
            "network: {}\nother: 1\n".to_string(),
            format!("{HAND_WIFI}    wlp3s0:\n      dhcp4: true\n      access-points:\n        x: {{}}\n"),
        ] {
            assert!(!parse_wifi(Some(&other)).manageable, "{other}");
        }

        // Kept text without WiFi is taken over as no networks and no device.
        let f = Fixture::new("take-bare", &["wlp58s0"]);
        let got = take_over(&kept(Some(bare), None), "wifi", &f.h, &inputs(false)).unwrap();
        assert_eq!((got.kept_wifi, got.wifi_device, got.networks), (None, None, vec![]));
    }

    #[test]
    fn take_over_refuses_what_it_cannot_do() {
        let f = Fixture::new("take-no", &["wlp58s0"]);
        let auth = HAND_WIFI.replace("cafe: {}", "cafe:\n          auth:\n            key-management: none");
        let mtu = SHIPPED_WIRED.replace("optional: true", "optional: true\n            mtu: 9000");
        let cfg = kept(Some(&auth), Some(&mtu));
        for which in ["wifi", "wired"] {
            let e = take_over(&cfg, which, &f.h, &inputs(false)).unwrap_err();
            assert!(e.contains("cannot keep"), "{e}");
        }
        assert!(take_over(&cfg, "other", &f.h, &inputs(false)).is_err());
        // Nothing kept: nothing to take over.
        let e = take_over(&NetworkCfg::default(), "wifi", &f.h, &inputs(false)).unwrap_err();
        assert!(e.contains("not kept"), "{e}");
        // Nor a netplan failure.
        f.flag("fail");
        assert_eq!(
            take_over(&kept(Some(HAND_WIFI), None), "wifi", &f.h, &inputs(false)),
            Err("ERROR: x: bad".into())
        );
    }

    #[test]
    fn take_over_checks_the_set_that_netboot_installs() {
        let f = Fixture::new("take-netboot", &["wlp58s0"]);
        let wired = format!("{SHIPPED_WIRED}# kept-marker\n");
        let cfg = kept(Some(HAND_WIFI), Some(&wired));
        take_over(&cfg, "wifi", &f.h, &inputs(false)).unwrap();
        assert!(f.recorded("checked").contains("kept-marker"));
        // Netboot installs no wired file: the default is what is checked.
        take_over(&cfg, "wifi", &f.h, &inputs(true)).unwrap();
        assert!(!f.recorded("checked").contains("kept-marker"));
    }

    #[test]
    fn take_over_keeps_the_dns() {
        let f = Fixture::new("dns-take-over", &["wlp58s0"]);
        let hand = "network:\n  version: 2\n  renderer: networkd\n  wifis:\n    wlp2s0:\n      dhcp4: true\n      dhcp4-overrides:\n        use-dns: false\n      nameservers:\n        addresses: [1.1.1.1, 9.9.9.9]\n      access-points:\n        home:\n          password: secret123\n";
        let got = take_over(&kept(Some(hand), None), "wifi", &f.h, &inputs(false)).unwrap();
        assert_eq!(got.wifi_dns, strs(&dns2()));
        assert_eq!(got.nets(), [net("home", Some("secret123"), false)]);
        let hand = format!(
            "{SHIPPED_WIRED}            dhcp4-overrides:\n                use-dns: false\n            nameservers:\n                addresses: [1.1.1.1, 9.9.9.9]\n"
        );
        let got = take_over(&kept(None, Some(&hand)), "wired", &f.h, &inputs(false)).unwrap();
        assert_eq!(got.wired().unwrap(), dhcp_dns());
    }

    #[test]
    fn input_checks() {
        let need = Err("The password needs 8 to 63 characters.".to_string());
        assert_eq!(check_password(&"a".repeat(7)), need);
        assert_eq!(check_password(&"a".repeat(8)), Ok(()));
        assert_eq!(check_password(&"a".repeat(63)), Ok(()));
        // Bytes are counted, as netplan does: é is two.
        assert_eq!(check_password(&"é".repeat(31)), Ok(()));
        assert_eq!(check_password(&format!("{}a", "é".repeat(31))), Ok(()));
        assert_eq!(check_password(&"é".repeat(32)), need);
        assert_eq!(check_password(&"é".repeat(4)), Ok(()));
        assert_eq!(check_password(&"é".repeat(3)), need);
        assert_eq!(check_password(&"0123456789abcdefABCDEF".repeat(3)[..64]), Ok(()));
        assert_eq!(check_password(&"g".repeat(64)), need);
        assert!(check_password(&format!("x{TEMPLATE_MARKER}")).is_err());
        assert!(check_password("secret\u{1b}123").is_err());

        assert_eq!(check_ssid("home"), Ok(()));
        assert_eq!(check_ssid(&"a".repeat(32)), Ok(()));
        let size = Err("The name needs 1 to 32 bytes.".to_string());
        assert_eq!(check_ssid(""), size);
        assert_eq!(check_ssid(&"a".repeat(33)), size);
        assert_eq!(check_ssid(&"é".repeat(17)), size);
        assert_eq!(
            check_ssid(TEMPLATE_MARKER),
            Err("This name cannot be used.".to_string())
        );
        assert_eq!(check_ssid("a\nb"), Err("This name cannot be used.".to_string()));
        let bad = Err("This name cannot be used.".to_string());
        assert_eq!(check_ssid("back\\slash"), bad);
        assert_eq!(check_ssid("trailing\\"), bad);
        // libyaml refuses these in a plain key (U+2028, U+2029) or anywhere
        // (U+FFFE, U+FFFF), and the emitter writes them unquoted.
        for c in ['\u{2028}', '\u{2029}', '\u{fffe}', '\u{ffff}'] {
            assert_eq!(check_ssid(&format!("a{c}b")), bad, "{c:?}");
            assert_eq!(
                check_password(&format!("secret{c}123")),
                Err("This password cannot be used.".to_string()),
                "{c:?}"
            );
        }
        assert_eq!(check_ssid("caf\u{e9}\u{feff}"), Ok(()));
    }

    #[test]
    fn render_wifi_drops_what_the_checks_refuse() {
        let t = render_wifi(
            Some("wlp58s0"),
            &[
                net(TEMPLATE_MARKER, None, false),
                net("ok", Some(&format!("{TEMPLATE_MARKER}!")), false),
                net("fine", None, false),
            ],
            &[],
        );
        assert!(!t.contains(TEMPLATE_MARKER), "{t}");
        assert_eq!(parse_wifi(Some(&t)).networks, [net("fine", None, false)]);
        assert!(!render_wifi(Some(TEMPLATE_MARKER), &[net("fine", None, false)], &[]).contains(TEMPLATE_MARKER));
    }

    fn dns2() -> Vec<Ipv4Addr> {
        vec![Ipv4Addr::new(1, 1, 1, 1), Ipv4Addr::new(9, 9, 9, 9)]
    }

    fn dhcp_dns() -> Wired {
        Wired::Dhcp { dns: dns2() }
    }

    #[test]
    fn render_wired_dhcp_with_dns_overrides_the_dhcp_servers() {
        let t = render_wired(SHIPPED_WIRED, &dhcp_dns()).unwrap().unwrap();
        let y = yaml(&t);
        let eth = &y["network"]["ethernets"]["eth"];
        assert_eq!(
            keys(eth),
            ["match", "dhcp4", "optional", "dhcp4-overrides", "nameservers"]
        );
        assert_eq!(eth["match"]["name"].as_str(), Some("en*"));
        assert_eq!(eth["dhcp4"], Yaml::Boolean(true));
        assert_eq!(keys(&eth["dhcp4-overrides"]), ["use-dns"]);
        assert_eq!(eth["dhcp4-overrides"]["use-dns"], Yaml::Boolean(false));
        assert_eq!(eth["nameservers"]["addresses"][0].as_str(), Some("1.1.1.1"));
        assert_eq!(eth["nameservers"]["addresses"][1].as_str(), Some("9.9.9.9"));
        let f = parse_wired(Some(&t));
        assert_eq!(f.state, FileState::Menu);
        assert!(f.manageable, "{t}");
        assert_eq!(f.wired, dhcp_dns());
        // A fixed address keeps its form: no dhcp4-overrides.
        let t = render_wired(SHIPPED_WIRED, &fixed()).unwrap().unwrap();
        assert!(!t.contains("dhcp4-overrides"), "{t}");
        // DHCP with the servers DHCP gives is the default itself.
        assert_eq!(render_wired(SHIPPED_WIRED, &Wired::dhcp()), Ok(None));
    }

    #[test]
    fn parse_wired_takes_dhcp_dns_only_in_the_menu_form() {
        let eth = |extra: &str| format!("{SHIPPED_WIRED}{extra}");
        let good = eth(
            "            dhcp4-overrides:\n                use-dns: false\n            nameservers:\n                addresses: [1.1.1.1, 9.9.9.9]\n",
        );
        let f = parse_wired(Some(&good));
        assert!(f.manageable, "{good}");
        assert_eq!(f.wired, dhcp_dns());
        for bad in [
            // Servers on top of those DHCP gives: the menu has no such setting.
            "            nameservers:\n                addresses: [1.1.1.1]\n",
            // The override alone: no DNS at all.
            "            dhcp4-overrides:\n                use-dns: false\n",
            "            dhcp4-overrides:\n                use-dns: true\n            nameservers:\n                addresses: [1.1.1.1]\n",
            "            dhcp4-overrides:\n                use-dns: false\n                use-ntp: false\n            nameservers:\n                addresses: [1.1.1.1]\n",
            "            dhcp4-overrides:\n                use-dns: false\n            nameservers:\n                addresses: []\n",
            "            dhcp4-overrides:\n                use-dns: false\n            nameservers:\n                addresses: [1.1.1.1]\n                search: [example.org]\n",
        ] {
            let t = eth(bad);
            assert!(!parse_wired(Some(&t)).manageable, "{t}");
        }
        // A fixed address with dhcp4-overrides is not the menu's.
        let fixed_text = render_wired(SHIPPED_WIRED, &fixed()).unwrap().unwrap();
        let t = fixed_text.replace(
            "dhcp4: false\n",
            "dhcp4: false\n      dhcp4-overrides:\n        use-dns: false\n",
        );
        assert_ne!(t, fixed_text);
        assert!(!parse_wired(Some(&t)).manageable, "{t}");
    }

    /// The form takes up to three DNS servers, so a ~/ethernet.yaml with
    /// more is not the menu's to keep: a hand file is not taken over, and a
    /// menu file counts as by hand for WiFi.
    #[test]
    fn parse_wired_keeps_at_most_three_dns_servers() {
        use crate::network::{NetEvent, NetRequest};
        let servers = |n: u8| (1..=n).map(|i| Ipv4Addr::new(10, 0, 0, i)).collect::<Vec<_>>();
        let static_with = |n: u8| Wired::Static {
            address: Ipv4Addr::new(192, 168, 1, 10),
            prefix: 24,
            gateway: Some(Ipv4Addr::new(192, 168, 1, 1)),
            dns: servers(n),
        };
        let dhcp_with = |n: u8| Wired::Dhcp { dns: servers(n) };
        for w in [static_with(3), dhcp_with(3)] {
            let t = render_wired(SHIPPED_WIRED, &w).unwrap().unwrap();
            let f = parse_wired(Some(&t));
            assert!(f.manageable, "{t}");
            assert_eq!(f.wired, w);
        }
        for (tag, w) in [("static", static_with(4)), ("dhcp", dhcp_with(4))] {
            let t = render_wired(SHIPPED_WIRED, &w).unwrap().unwrap();
            let f = parse_wired(Some(&t));
            assert_eq!(f.state, FileState::Menu);
            assert!(!f.manageable, "{t}");
            // Read as it is, so the DNS row shows all four.
            assert_eq!(f.wired, w);
            assert_eq!(machine_dns(&f, false), None);
            // The tab: an address edit fails on the DNS row, which can be
            // cut to three; WiFi is never out of step with such a file.
            let mut tab = crate::network::NetTab::new(false);
            let wifi = parse_wifi(Some(&render_wifi(
                Some("wlp58s0"),
                &[net("home", Some("secret123"), false)],
                &[],
            )));
            tab.loaded(wifi, f, vec!["wlp58s0".to_string()]);
            assert!(!tab.wifi_dns_out_of_step() && !tab.dirty(), "{t}");
            assert_eq!(tab.event(NetEvent::WiredMode(true), false), None);
            assert_eq!(
                tab.event(
                    NetEvent::WiredText {
                        address: "192.168.1.20/24".into(),
                        gateway: "192.168.1.1".into(),
                    },
                    false
                ),
                None
            );
            assert_eq!(tab.event(NetEvent::Apply, false), None);
            assert_eq!(tab.error(), Some("DNS servers: up to three IPv4 addresses"));
            assert_eq!(
                tab.event(NetEvent::DnsText("10.0.0.1, 10.0.0.2, 10.0.0.3".into()), false),
                None
            );
            assert!(matches!(tab.event(NetEvent::Apply, false), Some(NetRequest::Apply(_))));

            // The same text kept is not offered for take-over.
            let cfg = kept(None, Some(without_menu_header(&t)));
            assert_eq!(view(&cfg).1.state, FileState::Hand);
            let f = Fixture::new(&format!("take-four-dns-{tag}"), &["wlp58s0"]);
            let e = take_over(&cfg, "wired", &f.h, &inputs(false)).unwrap_err();
            assert!(e.contains("cannot keep"), "{e}");
        }
    }

    #[test]
    fn render_wifi_with_dns_overrides_the_dhcp_servers() {
        let nets = [net("home", Some("secret123"), false)];
        let t = render_wifi(Some("wlp58s0"), &nets, &dns2());
        let y = yaml(&t);
        let def = &y["network"]["wifis"]["wlp58s0"];
        assert_eq!(
            keys(def),
            ["dhcp4", "dhcp4-overrides", "nameservers", "optional", "access-points"]
        );
        assert_eq!(def["dhcp4-overrides"]["use-dns"], Yaml::Boolean(false));
        assert_eq!(def["nameservers"]["addresses"][1].as_str(), Some("9.9.9.9"));
        let f = parse_wifi(Some(&t));
        assert!(f.manageable, "{t}");
        assert_eq!(f.dns, dns2());
        assert_eq!(f.networks, nets);
        // Without servers: neither key.
        let t = render_wifi(Some("wlp58s0"), &nets, &[]);
        assert!(!t.contains("dhcp4-overrides") && !t.contains("nameservers"), "{t}");
        // No networks: no definition to carry them.
        let t = render_wifi(Some("wlp58s0"), &[], &dns2());
        assert!(!t.contains("nameservers"), "{t}");
    }

    #[test]
    fn parse_wifi_takes_dns_only_in_the_menu_form() {
        let base = "network:\n  version: 2\n  wifis:\n    wlp2s0:\n      dhcp4: true\n      access-points:\n        home: {}\n";
        let with = |extra: &str| base.replace("      dhcp4: true\n", &format!("      dhcp4: true\n{extra}"));
        let good =
            with("      dhcp4-overrides: {use-dns: false}\n      nameservers: {addresses: [1.1.1.1, 9.9.9.9]}\n");
        let f = parse_wifi(Some(&good));
        assert!(f.manageable, "{good}");
        assert_eq!(f.dns, dns2());
        for bad in [
            "      nameservers: {addresses: [1.1.1.1]}\n",
            "      dhcp4-overrides: {use-dns: false}\n",
            "      dhcp4-overrides: {use-dns: false, use-routes: false}\n      nameservers: {addresses: [1.1.1.1]}\n",
            "      dhcp4-overrides: {use-dns: false}\n      nameservers: {addresses: [nope]}\n",
        ] {
            let t = with(bad);
            assert!(!parse_wifi(Some(&t)).manageable, "{t}");
        }
        let no_dhcp = good.replace("dhcp4: true", "dhcp4: false");
        assert!(!parse_wifi(Some(&no_dhcp)).manageable, "{no_dhcp}");
    }

    /// Writes the files that test/integration/test-network-netplan.sh feeds to
    /// the netplan of the built image. Does nothing unless TF_NETPLAN_FIXTURES
    /// names a directory.
    #[test]
    fn write_netplan_fixtures() {
        let Some(dir) = std::env::var_os("TF_NETPLAN_FIXTURES") else {
            return;
        };
        let dir = PathBuf::from(dir);
        fs::create_dir_all(&dir).unwrap();
        let two = render_wifi(
            Some("wlp58s0"),
            &[net("hidden net", Some("secret123"), true), net("open net", None, false)],
            &[],
        );
        fs::write(dir.join("wifi-two.yaml"), two).unwrap();
        let pw = "p@ss: \"x\" #1\\";
        let mut odd: Vec<Net> = ["Caf\u{e9} \"Nord\"", " lead", "a:b #c", "yes", "123"]
            .iter()
            .enumerate()
            .map(|(i, s)| net(s, Some(pw), i % 2 == 0))
            .collect();
        odd.push(net("open", None, false));
        fs::write(dir.join("wifi-odd.yaml"), render_wifi(Some("wlp58s0"), &odd, &[])).unwrap();
        fs::write(dir.join("wifi-bare.yaml"), render_wifi(Some("wlp58s0"), &[], &[])).unwrap();
        let wired = Wired::Static {
            address: Ipv4Addr::new(192, 168, 1, 10),
            prefix: 24,
            gateway: Some(Ipv4Addr::new(192, 168, 1, 1)),
            dns: vec![Ipv4Addr::new(1, 1, 1, 1)],
        };
        let text = render_wired(SHIPPED_WIRED, &wired).unwrap().unwrap();
        fs::write(dir.join("ethernet-static.yaml"), text).unwrap();
        let text = render_wired(SHIPPED_WIRED, &dhcp_dns()).unwrap().unwrap();
        fs::write(dir.join("ethernet-dhcp-dns.yaml"), text).unwrap();
        let text = render_wifi(Some("wlp58s0"), &[net("home", Some("secret123"), false)], &dns2());
        fs::write(dir.join("wifi-dns.yaml"), text).unwrap();
    }

    #[test]
    fn another_version_or_renderer_is_by_hand() {
        let wifi_body = "  wifis:\n    wlp58s0:\n      dhcp4: true\n      optional: true\n      access-points:\n        home:\n          password: \"secret123\"\n";
        let wired_body =
            "  ethernets:\n    eth:\n      match:\n        name: 'en*'\n      dhcp4: true\n      optional: true\n";
        let heads = [
            "  version: 3\n  renderer: networkd\n",
            "  version: 2\n  renderer: NetworkManager\n",
            "  renderer: NetworkManager\n",
        ];
        for head in heads {
            for header in ["", MENU_HEADER] {
                let wifi = format!("{header}\nnetwork:\n{head}{wifi_body}");
                let w = parse_wifi(Some(&wifi));
                assert_eq!(w.state, FileState::Hand, "{wifi}");
                assert!(!w.manageable, "{wifi}");
                assert_eq!(w.networks.len(), 1, "{wifi}");
                let wired = format!("{header}\nnetwork:\n{head}{wired_body}");
                let e = parse_wired(Some(&wired));
                assert_eq!(e.state, FileState::Hand, "{wired}");
                assert!(!e.manageable, "{wired}");
            }
        }
        // A renderer on the definition itself counts the same.
        for header in ["", MENU_HEADER] {
            let wifi = format!(
                "{header}\nnetwork:\n  version: 2\n  wifis:\n    wlp58s0:\n      renderer: NetworkManager\n      dhcp4: true\n      access-points:\n        home: {{}}\n"
            );
            let w = parse_wifi(Some(&wifi));
            assert_eq!((w.state, w.manageable), (FileState::Hand, false), "{wifi}");
            let wired = format!(
                "{header}\nnetwork:\n  version: 2\n  ethernets:\n    eth:\n      renderer: NetworkManager\n      dhcp4: true\n"
            );
            let e = parse_wired(Some(&wired));
            assert_eq!((e.state, e.manageable), (FileState::Hand, false), "{wired}");
        }
        // netplan's defaults, written or left out, are the menu's.
        for head in ["  version: 2\n  renderer: networkd\n", "  version: 2\n", ""] {
            let wifi = format!("network:\n{head}{wifi_body}");
            assert!(parse_wifi(Some(&wifi)).manageable, "{wifi}");
            let wired = format!("network:\n{head}{wired_body}");
            assert!(parse_wired(Some(&wired)).manageable, "{wired}");
        }

        // Kept text of that kind is not offered for take-over, and the
        // form does not change it.
        let f = Fixture::new("foreign", &["wlp58s0"]);
        let cfg = kept(
            Some(&format!("{MENU_HEADER}\nnetwork:\n{}{wifi_body}", heads[1])),
            Some(&format!("{MENU_HEADER}\nnetwork:\n{}{wired_body}", heads[0])),
        );
        let (w, e) = view(&cfg);
        assert_eq!((w.state, w.manageable), (FileState::Hand, false));
        assert_eq!((e.state, e.manageable), (FileState::Hand, false));
        let err = plan_in(&f, &cfg, &wifi_change(vec![net("cafe", None, false)]), false).unwrap_err();
        assert!(err.contains("kept from your old settings"), "{err}");
        let err = plan_in(&f, &cfg, &wired_change(fixed()), false).unwrap_err();
        assert!(err.contains("kept from your old settings"), "{err}");
        for which in ["wifi", "wired"] {
            let err = take_over(&cfg, which, &f.h, &inputs(false)).unwrap_err();
            assert!(err.contains("cannot keep"), "{which}: {err}");
        }
        assert_eq!(f.recorded("root"), "", "netplan was not asked");
    }

    #[test]
    fn parse_wifi_keeps_at_most_three_dns_servers() {
        let servers = |n: u8| (1..=n).map(|i| Ipv4Addr::new(10, 0, 0, i)).collect::<Vec<_>>();
        let nets = [net("home", Some("secret123"), false)];
        let t = render_wifi(Some("wlp58s0"), &nets, &servers(3));
        let f = parse_wifi(Some(&t));
        assert!(f.manageable, "{t}");
        assert_eq!(f.dns, servers(3));
        let t = render_wifi(Some("wlp58s0"), &nets, &servers(4));
        let f = parse_wifi(Some(&t));
        assert_eq!(f.state, FileState::Menu);
        assert!(!f.manageable, "{t}");
        // Read as it is, and the menu's own password stays.
        assert_eq!(f.dns, servers(4));
        assert_eq!(f.networks, nets);
        // The same text kept is not offered for take-over.
        let fx = Fixture::new("take-four-dns-wifi", &["wlp58s0"]);
        let cfg = kept(Some(without_menu_header(&t)), None);
        assert_eq!(view(&cfg).0.state, FileState::Hand);
        let e = take_over(&cfg, "wifi", &fx.h, &inputs(false)).unwrap_err();
        assert!(e.contains("cannot keep"), "{e}");
    }
}
