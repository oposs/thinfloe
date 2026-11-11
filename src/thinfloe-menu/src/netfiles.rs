//! ~/wifi.yaml and ~/ethernet.yaml: the netplan files the Network tab
//! reads and writes. The menu owns a file by the header of `owned`; the
//! shipped WiFi template counts as no file at all. Every new file is
//! checked with `netplan generate` before it is written.

use crate::owned::{self, HEADER, Owner};
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

pub const WIFI_FILE: &str = "wifi.yaml";
pub const WIRED_FILE: &str = "ethernet.yaml";
/// Text of the shipped ~/wifi.yaml template; the menu never writes it.
pub const TEMPLATE_MARKER: &str = "device name of your wifi card";
/// A readable copy of the shipped /etc/netplan/ethernet.yaml (that one is
/// mode 600). Its `eth` definition is the base of a fixed address.
pub const DEFAULT_WIRED: &str = "/usr/share/thinfloe/netplan/ethernet.yaml";

/// Network files are tiny; the same cap as `owned`.
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
    Missing,
    /// Counts as no file; the menu may replace it.
    Template,
    Menu,
    Hand,
}

/// Who wrote `text` (`None`: no file). `template_rule` lets the template
/// count as `Template`; only ~/wifi.yaml has one.
pub fn state(text: Option<&str>, template_rule: bool) -> FileState {
    match owned::owner(text) {
        Owner::Missing => FileState::Missing,
        Owner::Menu => FileState::Menu,
        Owner::ByHand if template_rule && text.is_some_and(is_template) => FileState::Template,
        Owner::ByHand => FileState::Hand,
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
    Ok(format!("{HEADER}\n{body}\n"))
}

/// A file's text without the header line `owned::write` adds itself.
fn body(text: &str) -> &str {
    text.strip_prefix(HEADER)
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
            f.error = Some(format!("~/{WIFI_FILE}: {e}"));
            return f;
        }
    };
    let root = docs.first().and_then(Yaml::as_hash);
    let network = get_hash(root, "network");
    let wifis = get_hash(network, "wifis");
    // Per network: its AP keys are all the menu's own (Ruling 22).
    let mut ap_ok = Vec::new();
    let mut ok = docs.len() == 1
        && root.is_some_and(|h| keys_within(h, &["network"]))
        && network.is_some_and(|h| keys_within(h, &["version", "renderer", "wifis"]))
        && wifis.is_some_and(|h| h.len() == 1);
    for (dev, def) in wifis.into_iter().flatten() {
        let def = def.as_hash();
        let (dns, dns_ok) = def.map_or((Vec::new(), true), |d| {
            dhcp_dns(d, get(d, "dhcp4").and_then(flag) == Some(true))
        });
        ok &= dns_ok;
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
    emit(&Yaml::Hash(root)).unwrap_or_else(|_| format!("{HEADER}\nnetwork:\n  version: 2\n  renderer: networkd\n"))
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
            f.error = Some(format!("~/{WIRED_FILE}: {e}"));
            return f;
        }
    };
    let root = docs.first().and_then(Yaml::as_hash);
    let network = get_hash(root, "network");
    let ethernets = get_hash(network, "ethernets");
    let eth = get_hash(ethernets, "eth");
    let mut ok = docs.len() == 1
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

// ---- Load, save, take over ----

/// Where the files live, and the programs and paths to use.
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

    fn wifi(&self) -> PathBuf {
        self.home.join(WIFI_FILE)
    }

    fn wired(&self) -> PathBuf {
        self.home.join(WIRED_FILE)
    }

    fn default_text(&self) -> Result<String, String> {
        crate::bounded::read_bounded(&self.default_wired, MAX_BYTES, true)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .map_err(|e| format!("{}: {e}", self.default_wired.display()))
    }
}

/// What to change; `None`: leave that file as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    pub wifi: Option<Vec<Net>>,
    pub wired: Option<Wired>,
}

/// The home files as read: their text, or why they cannot be read (a link,
/// a FIFO, too big), which counts as by hand.
struct Current {
    wifi_text: Result<Option<String>, String>,
    wired_text: Result<Option<String>, String>,
    wifi: WifiFile,
    wired: WiredFile,
}

impl Current {
    fn read(h: &Homes) -> Current {
        let read = |p: &Path, name: &str| owned::read(p).map_err(|e| format!("~/{name}: {e}"));
        let wifi_text = read(&h.wifi(), WIFI_FILE);
        let wired_text = read(&h.wired(), WIRED_FILE);
        let wifi = match &wifi_text {
            Ok(t) => parse_wifi(t.as_deref()),
            Err(e) => WifiFile {
                state: FileState::Hand,
                device: None,
                networks: Vec::new(),
                dns: Vec::new(),
                manageable: false,
                error: Some(e.clone()),
            },
        };
        let wired = match &wired_text {
            Ok(t) => parse_wired(t.as_deref()),
            Err(e) => WiredFile {
                state: FileState::Hand,
                wired: Wired::dhcp(),
                manageable: false,
                error: Some(e.clone()),
            },
        };
        Current {
            wifi_text,
            wired_text,
            wifi,
            wired,
        }
    }

    /// The WiFi file in force: the template installs nothing.
    fn wifi_in_force(&self) -> Option<String> {
        match (&self.wifi_text, self.wifi.state) {
            (Ok(Some(t)), FileState::Menu | FileState::Hand) => Some(t.clone()),
            _ => None,
        }
    }

    /// The wired file in force: the home one, except on netboot.
    fn wired_in_force(&self, h: &Homes, netboot: bool) -> Result<String, String> {
        match &self.wired_text {
            Ok(Some(t)) if !netboot => Ok(t.clone()),
            _ => h.default_text(),
        }
    }
}

pub fn load(h: &Homes) -> (WifiFile, WiredFile) {
    let c = Current::read(h);
    (c.wifi, c.wired)
}

fn by_hand(name: &str) -> String {
    format!("~/{name} is set by hand")
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

/// The machine's DNS servers as ~/wifi.yaml is to carry them, when the
/// menu sets them there: those of `wired`, a ~/ethernet.yaml the menu owns
/// (missing: none). `None`: ~/wifi.yaml keeps its own, beside a wired file
/// by hand or one that cannot be read (the menu does not take servers from
/// a file it does not own), and on netboot, which installs no
/// ~/ethernet.yaml.
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

/// The menu's WiFi file `now` again with the servers `dns`, when they
/// differ from its own. A file by hand, the template and a file without
/// networks (no definition to carry them) stay as they are. A network the
/// checks refuse would be dropped, so it stops the rewrite instead.
fn wifi_with_dns(now: &WifiFile, dns: &[Ipv4Addr]) -> Result<Option<String>, String> {
    if now.state != FileState::Menu || now.networks.is_empty() || now.dns == dns {
        return Ok(None);
    }
    if !now.manageable {
        return Err(format!(
            "~/{WIFI_FILE} has settings the menu cannot keep, so the DNS servers cannot be set for WiFi."
        ));
    }
    check_saved(&now.networks)?;
    Ok(Some(render_wifi(now.device.as_deref(), &now.networks, dns)))
}

/// The networks of ~/wifi.yaml pass the input checks; else the first that
/// fails, named.
pub fn check_saved(networks: &[Net]) -> Result<(), String> {
    for n in networks {
        check_net(n).map_err(|e| format!("~/{WIFI_FILE}, {}: {e}", n.ssid))?;
    }
    Ok(())
}

/// Write the changes of `c` to the home. Nothing is written unless every
/// file of the new set passes the input checks and `netplan generate`.
pub fn save(h: &Homes, c: &Change, netboot: bool) -> Result<(), String> {
    if c.wifi.is_none() && c.wired.is_none() {
        return Ok(());
    }
    let now = Current::read(h);
    if c.wifi.is_some() && now.wifi.state == FileState::Hand {
        return Err(by_hand(WIFI_FILE));
    }
    if c.wired.is_some() && now.wired.state == FileState::Hand {
        return Err(by_hand(WIRED_FILE));
    }
    if netboot {
        match &c.wired {
            Some(Wired::Static { .. }) => return Err("A fixed address cannot be set on netboot.".to_string()),
            Some(w) if !w.dns().is_empty() => return Err("DNS servers cannot be set on netboot.".to_string()),
            _ => {}
        }
    }
    // One DNS setting for the whole machine, kept in ~/ethernet.yaml and
    // copied into every ~/wifi.yaml the menu writes: systemd-resolved
    // queries the DNS servers of all links, so servers set on the wired
    // link alone would still leak queries to the WiFi link's DHCP servers.
    // `None` (netboot, a wired file the menu does not own): ~/wifi.yaml
    // keeps the servers it has.
    let dns: Option<&[Ipv4Addr]> = match &c.wired {
        _ if netboot => None,
        Some(w) => Some(w.dns()),
        None => machine_dns(&now.wired, netboot),
    };
    let new_wifi = match &c.wifi {
        None => match dns {
            Some(dns) => wifi_with_dns(&now.wifi, dns)?,
            None => None,
        },
        Some(nets) => {
            for n in nets {
                check_net(n)?;
            }
            let devices = wifi_devices(&h.sys_net);
            if !nets.is_empty() && devices.is_empty() {
                return Err("No WiFi adapter found".to_string());
            }
            Some(render_wifi(
                devices.first().map(String::as_str),
                nets,
                dns.unwrap_or(&now.wifi.dns),
            ))
        }
    };
    // Some(None): back to DHCP with its own servers, the default.
    let new_wired = match &c.wired {
        None => None,
        Some(w) => Some(render_wired(&h.default_text()?, w)?),
    };
    let wifi_check = new_wifi.clone().or_else(|| now.wifi_in_force());
    let wired_check = match &new_wired {
        Some(Some(t)) => t.clone(),
        Some(None) => h.default_text()?,
        None => now.wired_in_force(h, netboot)?,
    };
    check_set(h, wifi_check.as_deref(), &wired_check)?;
    if let Some(t) = &new_wifi {
        owned::write_with(&h.wifi(), body(t), is_template).map_err(|e| e.to_string())?;
    }
    match &new_wired {
        Some(Some(t)) => owned::write(&h.wired(), body(t)).map_err(|e| e.to_string())?,
        Some(None) => owned::remove(&h.wired()).map_err(|e| e.to_string())?,
        None => {}
    }
    Ok(())
}

/// *Let the menu manage it*: move a hand-written file `which` ("wifi" or
/// "wired") aside to `<file>.by-hand` and write the menu's own file with
/// the same networks and passwords (device kept) or the same addressing.
/// Checked as `save` checks; nothing changes when a step fails.
pub fn take_over(h: &Homes, which: &str) -> Result<(), String> {
    let now = Current::read(h);
    let (name, path, state, manageable) = match which {
        "wifi" => (WIFI_FILE, h.wifi(), now.wifi.state, now.wifi.manageable),
        "wired" => (WIRED_FILE, h.wired(), now.wired.state, now.wired.manageable),
        _ => return Err(format!("Unknown network file: {which}")),
    };
    if state != FileState::Hand {
        return Err(format!("~/{name} is not set by hand"));
    }
    if !manageable {
        return Err(format!("~/{name} has settings the menu cannot keep"));
    }
    let new = match which {
        "wifi" => {
            check_saved(&now.wifi.networks)?;
            Some(render_wifi(
                now.wifi.device.as_deref(),
                &now.wifi.networks,
                &now.wifi.dns,
            ))
        }
        _ => render_wired(&h.default_text()?, &now.wired.wired)?,
    };
    if which == "wifi" {
        check_set(h, new.as_deref(), &now.wired_in_force(h, false)?)?;
    } else {
        let wired = match &new {
            Some(t) => t.clone(),
            None => h.default_text()?,
        };
        check_set(h, now.wifi_in_force().as_deref(), &wired)?;
    }
    owned::take_over(&path)?;
    if let Some(t) = &new
        && let Err(e) = owned::write(&path, body(t))
    {
        // The hand file goes back, unless something took its place.
        let aside = owned::aside(&path);
        let back = match fs::symlink_metadata(&path) {
            Err(m) if m.kind() == io::ErrorKind::NotFound => fs::rename(&aside, &path),
            _ => Err(io::Error::other("the place is taken")),
        };
        return Err(match back {
            Ok(()) => e.to_string(),
            Err(b) => format!("{e}; ~/{name} stays as {}: {b}", owned::home_relative(&aside)),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
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

        fn home(&self, name: &str) -> PathBuf {
            self.h.home.join(name)
        }

        fn put(&self, name: &str, text: &str) {
            fs::write(self.home(name), text).unwrap();
        }

        fn text(&self, name: &str) -> Option<String> {
            fs::read_to_string(self.home(name)).ok()
        }

        fn flag(&self, name: &str) {
            fs::write(self.d.join("bin").join(name), "").unwrap();
        }

        fn recorded(&self, name: &str) -> String {
            fs::read_to_string(self.d.join("bin").join(name)).unwrap_or_default()
        }

        /// Every entry of the home, links by their target.
        fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
            let mut m = BTreeMap::new();
            for e in fs::read_dir(&self.h.home).unwrap().flatten() {
                let p = e.path();
                let v = if fs::symlink_metadata(&p).unwrap().file_type().is_symlink() {
                    format!("link -> {}", fs::read_link(&p).unwrap().display()).into_bytes()
                } else {
                    fs::read(&p).unwrap()
                };
                m.insert(e.file_name().to_string_lossy().into_owned(), v);
            }
            m
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
            state(Some(&format!("{HEADER}\nnetwork: {{}}\n")), true),
            FileState::Menu
        );
        assert_eq!(state(Some(&format!("{HEADER}\n")), true), FileState::Menu);
        assert_eq!(state(Some("network: {}\n"), true), FileState::Hand);
        assert_eq!(
            state(Some(&format!("network: {{}}\n{HEADER}\n")), true),
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
        assert!(t.starts_with(&format!("{HEADER}\n")), "{t}");
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
            assert!(t.starts_with(&format!("{HEADER}\n")), "{t}");
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
        assert!(t.starts_with(&format!("{HEADER}\n")), "{t}");
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
            "{HEADER}\n{}",
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
            broken.error.as_deref().unwrap().starts_with("~/wifi.yaml: "),
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
        assert!(broken.error.as_deref().unwrap().starts_with("~/ethernet.yaml: "));
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

    #[test]
    fn save_keeps_unchanged_file_alone() {
        let f = Fixture::new("unchanged", &["wlp58s0"]);
        // It has the servers of the new wired file already.
        let wifi = render_wifi(Some("wlp58s0"), &[net("home", Some("secret123"), false)], fixed().dns());
        f.put(WIFI_FILE, &wifi);
        save(
            &f.h,
            &Change {
                wifi: None,
                wired: Some(fixed()),
            },
            false,
        )
        .unwrap();
        assert_eq!(f.text(WIFI_FILE).as_deref(), Some(wifi.as_str()));
        assert_eq!(parse_wired(f.text(WIRED_FILE).as_deref()).wired, fixed());
        // The check saw the set: the old WiFi file and the new wired one.
        assert_eq!(f.recorded("names"), "ethernet.yaml\nwifi.yaml\n");
        assert!(f.recorded("checked").contains("secret123"));
        assert!(f.recorded("checked").contains("192.168.1.10/24"));

        let wired = f.text(WIRED_FILE);
        save(
            &f.h,
            &Change {
                wifi: Some(vec![net("cafe", None, false)]),
                wired: None,
            },
            false,
        )
        .unwrap();
        assert_eq!(f.text(WIRED_FILE), wired);
        assert!(f.recorded("checked").contains("192.168.1.10/24"));

        let before = f.snapshot();
        fs::remove_file(f.d.join("bin/root")).unwrap();
        save(&f.h, &Change::default(), false).unwrap();
        assert_eq!(f.snapshot(), before);
        assert_eq!(f.recorded("root"), "");
    }

    #[test]
    fn save_writes_the_current_device() {
        let f = Fixture::new("device", &["wlp58s0"]);
        let nets = vec![net("home", Some("secret123"), false), net("cafe", None, true)];
        f.put(WIFI_FILE, &render_wifi(Some("wlp2s0"), &nets, &[]));
        let (w, _) = load(&f.h);
        assert_eq!(w.device.as_deref(), Some("wlp2s0"));
        save(
            &f.h,
            &Change {
                wifi: Some(w.networks),
                wired: None,
            },
            false,
        )
        .unwrap();
        let w = parse_wifi(f.text(WIFI_FILE).as_deref());
        assert_eq!(w.state, FileState::Menu);
        assert_eq!(w.device.as_deref(), Some("wlp58s0"));
        assert_eq!(w.networks, nets);
        // No wired file at home: the check used the default.
        assert!(f.recorded("checked").contains("dhcp4: true"));
        assert_eq!(f.recorded("names"), "ethernet.yaml\nwifi.yaml\n");
    }

    #[test]
    fn save_refuses_what_netplan_refuses() {
        let f = Fixture::new("refused", &["wlp58s0"]);
        f.put(
            WIFI_FILE,
            &render_wifi(Some("wlp58s0"), &[net("home", Some("secret123"), false)], &[]),
        );
        f.flag("fail");
        let before = f.snapshot();
        let c = Change {
            wifi: Some(vec![net("cafe", Some("password9"), false)]),
            wired: Some(fixed()),
        };
        assert_eq!(save(&f.h, &c, false), Err("ERROR: x: bad".into()));
        assert_eq!(f.snapshot(), before);
        let c = Change {
            wifi: None,
            wired: Some(Wired::dhcp()),
        };
        f.put(WIRED_FILE, &render_wired(SHIPPED_WIRED, &fixed()).unwrap().unwrap());
        let before = f.snapshot();
        assert_eq!(save(&f.h, &c, false), Err("ERROR: x: bad".into()));
        assert_eq!(f.snapshot(), before);
    }

    #[test]
    fn save_never_touches_hand_or_link() {
        let f = Fixture::new("hand", &["wlp58s0"]);
        let hand = "network:\n  version: 2\n  wifis:\n    wlp2s0:\n      access-points:\n        x: {}\n";
        f.put(WIFI_FILE, hand);
        // A link to a menu-owned file outside the home.
        let outside = f.d.join("outside.yaml");
        let menu_wired = render_wired(SHIPPED_WIRED, &fixed()).unwrap().unwrap();
        fs::write(&outside, &menu_wired).unwrap();
        symlink(&outside, f.home(WIRED_FILE)).unwrap();
        let before = f.snapshot();
        let wifi = Change {
            wifi: Some(vec![]),
            wired: None,
        };
        assert_eq!(save(&f.h, &wifi, false), Err("~/wifi.yaml is set by hand".into()));
        for w in [fixed(), Wired::dhcp()] {
            let c = Change {
                wifi: None,
                wired: Some(w),
            };
            assert_eq!(save(&f.h, &c, false), Err("~/ethernet.yaml is set by hand".into()));
        }
        assert_eq!(f.snapshot(), before);
        assert_eq!(fs::read_to_string(&outside).unwrap(), menu_wired);

        // A link to the template is not the template.
        let template = f.d.join("template.yaml");
        fs::write(&template, SHIPPED_WIFI).unwrap();
        fs::remove_file(f.home(WIFI_FILE)).unwrap();
        symlink(&template, f.home(WIFI_FILE)).unwrap();
        let before = f.snapshot();
        assert_eq!(save(&f.h, &wifi, false), Err("~/wifi.yaml is set by hand".into()));
        assert_eq!(f.snapshot(), before);
        assert_eq!(fs::read_to_string(&template).unwrap(), SHIPPED_WIFI);
        let (w, e) = load(&f.h);
        assert_eq!((w.state, e.state), (FileState::Hand, FileState::Hand));
        assert!(!w.manageable && !e.manageable);
        assert!(take_over(&f.h, "wifi").is_err());
        assert!(take_over(&f.h, "wired").is_err());
        assert_eq!(f.snapshot(), before);
    }

    #[test]
    fn save_replaces_the_template() {
        let f = Fixture::new("template", &["wlp58s0"]);
        f.put(WIFI_FILE, SHIPPED_WIFI);
        assert_eq!(load(&f.h).0.state, FileState::Template);
        let nets = vec![net("home", Some("secret123"), false)];
        save(
            &f.h,
            &Change {
                wifi: Some(nets.clone()),
                wired: None,
            },
            false,
        )
        .unwrap();
        let w = parse_wifi(f.text(WIFI_FILE).as_deref());
        assert_eq!(w.state, FileState::Menu);
        assert_eq!(w.networks, nets);
        // The template itself was not part of the check.
        f.put(WIFI_FILE, SHIPPED_WIFI);
        save(
            &f.h,
            &Change {
                wifi: None,
                wired: Some(fixed()),
            },
            false,
        )
        .unwrap();
        assert_eq!(f.recorded("names"), "ethernet.yaml\n");
        assert_eq!(f.text(WIFI_FILE).as_deref(), Some(SHIPPED_WIFI));
    }

    #[test]
    fn save_static_refused_on_netboot() {
        let f = Fixture::new("netboot", &["wlp58s0"]);
        let c = Change {
            wifi: None,
            wired: Some(fixed()),
        };
        let e = save(&f.h, &c, true).unwrap_err();
        assert!(e.contains("netboot"), "{e}");
        assert_eq!(f.text(WIRED_FILE), None);
        save(
            &f.h,
            &Change {
                wifi: None,
                wired: Some(Wired::dhcp()),
            },
            true,
        )
        .unwrap();
        assert_eq!(f.text(WIRED_FILE), None);
        // On netboot the home wired file is not in force: the check uses the default.
        f.put(WIRED_FILE, &render_wired(SHIPPED_WIRED, &fixed()).unwrap().unwrap());
        save(
            &f.h,
            &Change {
                wifi: Some(vec![]),
                wired: None,
            },
            true,
        )
        .unwrap();
        assert!(!f.recorded("checked").contains("192.168.1.10"));
    }

    #[test]
    fn save_dhcp_removes_menu_wired_only() {
        let f = Fixture::new("dhcp", &["wlp58s0"]);
        f.put(WIRED_FILE, &render_wired(SHIPPED_WIRED, &fixed()).unwrap().unwrap());
        save(
            &f.h,
            &Change {
                wifi: None,
                wired: Some(Wired::dhcp()),
            },
            false,
        )
        .unwrap();
        assert_eq!(f.text(WIRED_FILE), None);
        assert!(f.recorded("checked").contains("dhcp4: true"));
        save(
            &f.h,
            &Change {
                wifi: None,
                wired: Some(Wired::dhcp()),
            },
            false,
        )
        .unwrap();
        f.put(WIRED_FILE, SHIPPED_WIRED);
        assert_eq!(
            save(
                &f.h,
                &Change {
                    wifi: None,
                    wired: Some(Wired::dhcp())
                },
                false
            ),
            Err("~/ethernet.yaml is set by hand".into())
        );
        assert_eq!(f.text(WIRED_FILE).as_deref(), Some(SHIPPED_WIRED));
    }

    #[test]
    fn save_without_adapter_fails() {
        let f = Fixture::new("no-adapter", &[]);
        let c = Change {
            wifi: Some(vec![net("home", Some("secret123"), false)]),
            wired: None,
        };
        assert_eq!(save(&f.h, &c, false), Err("No WiFi adapter found".into()));
        assert_eq!(f.text(WIFI_FILE), None);
        assert_eq!(f.recorded("root"), "");
        save(
            &f.h,
            &Change {
                wifi: Some(vec![]),
                wired: None,
            },
            false,
        )
        .unwrap();
        let w = parse_wifi(f.text(WIFI_FILE).as_deref());
        assert_eq!(w.state, FileState::Menu);
        assert!(w.networks.is_empty());
    }

    #[test]
    fn save_refuses_bad_input() {
        let f = Fixture::new("bad-input", &["wlp58s0"]);
        for n in [
            net("home", Some("short"), false),
            net("", None, false),
            net(TEMPLATE_MARKER, None, false),
            net("back\\slash", None, false),
            net("line\u{2028}sep", None, false),
            net("home", Some("secret\u{ffff}123"), false),
        ] {
            assert!(
                save(
                    &f.h,
                    &Change {
                        wifi: Some(vec![n]),
                        wired: None
                    },
                    false
                )
                .is_err()
            );
        }
        assert_eq!(f.text(WIFI_FILE), None);
        assert_eq!(f.recorded("root"), "");
    }

    const HAND_WIFI: &str = "# my own\nnetwork:\n  version: 2\n  renderer: networkd\n  wifis:\n    wlp2s0:\n      dhcp4: true\n      access-points:\n        home:\n          password: \"secret123\"\n        hidden one:\n          password: 'p#ss word'\n          hidden: true\n        cafe: {}\n";

    #[test]
    fn take_over_keeps_networks_and_passwords() {
        let f = Fixture::new("take", &["wlp58s0"]);
        f.put(WIFI_FILE, HAND_WIFI);
        take_over(&f.h, "wifi").unwrap();
        assert_eq!(f.text("wifi.yaml.by-hand").as_deref(), Some(HAND_WIFI));
        let w = parse_wifi(f.text(WIFI_FILE).as_deref());
        assert_eq!(w.state, FileState::Menu);
        assert_eq!(w.device.as_deref(), Some("wlp2s0"));
        assert_eq!(
            w.networks,
            [
                net("home", Some("secret123"), false),
                net("hidden one", Some("p#ss word"), true),
                net("cafe", None, false)
            ]
        );

        let hand_wired = SHIPPED_WIRED.replace(
            "dhcp4: true",
            "dhcp4: false\n            addresses: [192.168.1.10/24]\n            routes:\n              - to: default\n                via: 192.168.1.1\n            nameservers:\n              addresses: [1.1.1.1, 9.9.9.9]",
        );
        f.put(WIRED_FILE, &hand_wired);
        assert_eq!(load(&f.h).1.wired, fixed());
        take_over(&f.h, "wired").unwrap();
        assert_eq!(f.text("ethernet.yaml.by-hand").as_deref(), Some(hand_wired.as_str()));
        let e = parse_wired(f.text(WIRED_FILE).as_deref());
        assert_eq!((e.state, e.wired), (FileState::Menu, fixed()));

        // A hand file that says DHCP leaves no home file: the default applies.
        let g = Fixture::new("take-dhcp", &["wlp58s0"]);
        g.put(WIRED_FILE, SHIPPED_WIRED);
        take_over(&g.h, "wired").unwrap();
        assert_eq!(g.text(WIRED_FILE), None);
        assert_eq!(g.text("ethernet.yaml.by-hand").as_deref(), Some(SHIPPED_WIRED));
    }

    /// Ruling 25: a take-over keeps the content, so a hand file that
    /// matches another NIC, or is not optional, is not the menu's to manage.
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
        // And take-over refuses it, leaving the file alone.
        let f = Fixture::new("take-wired-match", &["wlp58s0"]);
        let mac = SHIPPED_WIRED.replace("name: 'en*'", "macaddress: '00:11:22:33:44:55'");
        f.put(WIRED_FILE, &mac);
        let before = f.snapshot();
        assert_eq!(
            take_over(&f.h, "wired"),
            Err("~/ethernet.yaml has settings the menu cannot keep".into())
        );
        assert_eq!(f.snapshot(), before);
    }

    #[test]
    fn take_over_refuses_unmanageable() {
        let f = Fixture::new("take-no", &["wlp58s0"]);
        let auth = HAND_WIFI.replace("cafe: {}", "cafe:\n          auth:\n            key-management: none");
        f.put(WIFI_FILE, &auth);
        let mtu = SHIPPED_WIRED.replace("optional: true", "optional: true\n            mtu: 9000");
        f.put(WIRED_FILE, &mtu);
        let before = f.snapshot();
        assert!(take_over(&f.h, "wifi").is_err());
        assert!(take_over(&f.h, "wired").is_err());
        assert!(take_over(&f.h, "other").is_err());
        assert_eq!(f.snapshot(), before);
        // Nor a file the menu owns already, nor a netplan failure.
        let g = Fixture::new("take-menu", &["wlp58s0"]);
        g.put(WIFI_FILE, &render_wifi(Some("wlp58s0"), &[], &[]));
        assert!(take_over(&g.h, "wifi").is_err());
        g.put(WIFI_FILE, HAND_WIFI);
        g.flag("fail");
        let before = g.snapshot();
        assert_eq!(take_over(&g.h, "wifi"), Err("ERROR: x: bad".into()));
        assert_eq!(g.snapshot(), before);
    }

    /// The write after the rename fails: the hand file comes back.
    #[test]
    fn take_over_that_cannot_write_puts_the_hand_file_back() {
        let f = Fixture::new("take-write-fails", &["wlp58s0"]);
        f.put(WIFI_FILE, HAND_WIFI);
        fs::create_dir_all(f.h.home.join("wifi.yaml.tmp/x")).unwrap();
        assert!(take_over(&f.h, "wifi").is_err());
        assert_eq!(f.text(WIFI_FILE).as_deref(), Some(HAND_WIFI));
        assert!(fs::symlink_metadata(f.h.home.join("wifi.yaml.by-hand")).is_err());
    }

    #[test]
    fn take_over_refuses_existing_by_hand() {
        let f = Fixture::new("take-twice", &["wlp58s0"]);
        f.put(WIFI_FILE, HAND_WIFI);
        f.put("wifi.yaml.by-hand", "older\n");
        let before = f.snapshot();
        let e = take_over(&f.h, "wifi").unwrap_err();
        assert!(e.ends_with("exists already"), "{e}");
        assert_eq!(f.snapshot(), before);
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

    #[test]
    fn save_dhcp_dns_goes_to_both_files() {
        let f = Fixture::new("dhcp-dns", &["wlp58s0"]);
        let nets = vec![net("home", Some("secret123"), false), net("cafe", None, true)];
        f.put(WIFI_FILE, &render_wifi(Some("wlp58s0"), &nets, &[]));
        let c = Change {
            wifi: None,
            wired: Some(dhcp_dns()),
        };
        save(&f.h, &c, false).unwrap();
        assert_eq!(parse_wired(f.text(WIRED_FILE).as_deref()).wired, dhcp_dns());
        let w = parse_wifi(f.text(WIFI_FILE).as_deref());
        assert!(w.manageable);
        assert_eq!(w.state, FileState::Menu);
        assert_eq!(w.networks, nets, "passwords and hidden kept");
        assert_eq!(w.dns, dns2());
        assert!(f.recorded("checked").contains("use-dns: false"));
        // A WiFi change takes the servers of ~/ethernet.yaml.
        save(
            &f.h,
            &Change {
                wifi: Some(vec![net("lab", None, false)]),
                wired: None,
            },
            false,
        )
        .unwrap();
        let w = parse_wifi(f.text(WIFI_FILE).as_deref());
        assert_eq!(w.networks, [net("lab", None, false)]);
        assert_eq!(w.dns, dns2());
        // Fixed-address servers reach WiFi too.
        save(
            &f.h,
            &Change {
                wifi: None,
                wired: Some(fixed()),
            },
            false,
        )
        .unwrap();
        assert_eq!(parse_wifi(f.text(WIFI_FILE).as_deref()).dns, dns2());
        // Back to the servers DHCP gives: both lose them.
        save(
            &f.h,
            &Change {
                wifi: None,
                wired: Some(Wired::dhcp()),
            },
            false,
        )
        .unwrap();
        assert_eq!(f.text(WIRED_FILE), None);
        let t = f.text(WIFI_FILE).unwrap();
        assert!(!t.contains("nameservers") && !t.contains("dhcp4-overrides"), "{t}");
        assert_eq!(parse_wifi(Some(&t)).networks, [net("lab", None, false)]);
    }

    #[test]
    fn save_dhcp_dns_leaves_other_wifi_files_alone() {
        // No ~/wifi.yaml: none is made.
        let f = Fixture::new("dns-no-wifi", &["wlp58s0"]);
        let c = Change {
            wifi: None,
            wired: Some(dhcp_dns()),
        };
        save(&f.h, &c, false).unwrap();
        assert_eq!(parse_wired(f.text(WIRED_FILE).as_deref()).wired, dhcp_dns());
        assert_eq!(f.text(WIFI_FILE), None);
        // The template and a file by hand stay as they are.
        for (tag, text) in [
            ("dns-template", SHIPPED_WIFI),
            (
                "dns-hand",
                "network:\n  version: 2\n  wifis:\n    wlp2s0:\n      dhcp4: true\n      access-points:\n        x: {}\n",
            ),
        ] {
            let f = Fixture::new(tag, &["wlp58s0"]);
            f.put(WIFI_FILE, text);
            save(&f.h, &c, false).unwrap();
            assert_eq!(f.text(WIFI_FILE).as_deref(), Some(text));
            assert_eq!(parse_wired(f.text(WIRED_FILE).as_deref()).wired, dhcp_dns());
        }
        // A menu file with no networks needs no rewrite.
        let f = Fixture::new("dns-bare-wifi", &[]);
        let bare = render_wifi(None, &[], &[]);
        f.put(WIFI_FILE, &bare);
        save(&f.h, &c, false).unwrap();
        assert_eq!(f.text(WIFI_FILE), Some(bare));
    }

    /// A menu ~/wifi.yaml from an older menu with a network today's checks
    /// refuse: password `short12`.
    fn wifi_with_short_password() -> String {
        format!(
            "{HEADER}\nnetwork:\n  version: 2\n  renderer: networkd\n  wifis:\n    wlp58s0:\n      dhcp4: true\n      optional: true\n      access-points:\n        home:\n          password: short12\n        cafe: {{}}\n"
        )
    }

    #[test]
    fn a_dns_rewrite_never_drops_a_network() {
        let f = Fixture::new("dns-drop", &["wlp58s0"]);
        f.put(WIFI_FILE, &wifi_with_short_password());
        let w = parse_wifi(f.text(WIFI_FILE).as_deref());
        assert!(w.manageable && w.networks.len() == 2, "{w:?}");
        let before = f.snapshot();
        let c = Change {
            wifi: None,
            wired: Some(dhcp_dns()),
        };
        assert_eq!(
            save(&f.h, &c, false),
            Err("~/wifi.yaml, home: The password needs 8 to 63 characters.".into())
        );
        assert_eq!(f.snapshot(), before);
    }

    #[test]
    fn a_dns_rewrite_of_a_file_the_menu_cannot_keep_says_why() {
        let f = Fixture::new("dns-unkept", &["wlp58s0"]);
        let unkept = format!(
            "{HEADER}\nnetwork:\n  version: 2\n  wifis:\n    wlp58s0:\n      dhcp4: true\n      access-points:\n        home:\n          password: secret123\n          auth:\n            key-management: psk\n"
        );
        f.put(WIFI_FILE, &unkept);
        let before = f.snapshot();
        let c = Change {
            wifi: None,
            wired: Some(dhcp_dns()),
        };
        assert_eq!(
            save(&f.h, &c, false),
            Err("~/wifi.yaml has settings the menu cannot keep, so the DNS servers cannot be set for WiFi.".into())
        );
        assert_eq!(f.snapshot(), before);
    }

    /// Only a ~/ethernet.yaml the menu owns gives WiFi its servers; with
    /// one by hand or one that cannot be read, ~/wifi.yaml keeps its own.
    #[test]
    fn wifi_keeps_its_dns_beside_a_wired_file_the_menu_does_not_own() {
        let additive = format!("{SHIPPED_WIRED}            nameservers:\n                addresses: [8.8.8.8]\n");
        for (tag, own) in [("dns-hand-keep", dns2()), ("dns-hand-none", vec![])] {
            let f = Fixture::new(tag, &["wlp58s0"]);
            f.put(WIRED_FILE, &additive);
            f.put(
                WIFI_FILE,
                &render_wifi(Some("wlp58s0"), &[net("home", None, false)], &own),
            );
            let nets = vec![net("lab", None, false)];
            let c = Change {
                wifi: Some(nets.clone()),
                wired: None,
            };
            save(&f.h, &c, false).unwrap();
            let w = parse_wifi(f.text(WIFI_FILE).as_deref());
            assert_eq!((w.networks, w.dns), (nets, own));
            assert_eq!(f.text(WIRED_FILE).as_deref(), Some(additive.as_str()));
        }
        // Unreadable: a directory in its place.
        let f = Fixture::new("dns-unreadable", &["wlp58s0"]);
        fs::create_dir(f.home(WIRED_FILE)).unwrap();
        f.put(
            WIFI_FILE,
            &render_wifi(Some("wlp58s0"), &[net("home", None, false)], &dns2()),
        );
        let c = Change {
            wifi: Some(vec![net("lab", None, false)]),
            wired: None,
        };
        save(&f.h, &c, false).unwrap();
        assert_eq!(parse_wifi(f.text(WIFI_FILE).as_deref()).dns, dns2());
    }

    /// A menu ~/ethernet.yaml edited by hand into a form the menu cannot
    /// keep gives WiFi no servers: ~/wifi.yaml keeps its own.
    #[test]
    fn wifi_keeps_its_dns_beside_a_menu_wired_file_the_menu_cannot_keep() {
        let menu = render_wired(SHIPPED_WIRED, &fixed()).unwrap().unwrap();
        let two_addresses = menu.replace("- 192.168.1.10/24", "- 192.168.1.10/24\n          - 10.0.0.5/8");
        let additive = format!(
            "{HEADER}\n{}            nameservers:\n                addresses: [8.8.8.8]\n",
            body(SHIPPED_WIRED)
        );
        for (tag, wired) in [("dns-unkept-two", &two_addresses), ("dns-unkept-add", &additive)] {
            let w = parse_wired(Some(wired));
            assert_eq!((w.state, w.manageable), (FileState::Menu, false), "{wired}");
            assert_eq!(machine_dns(&w, false), None);
            let f = Fixture::new(tag, &["wlp58s0"]);
            f.put(WIRED_FILE, wired);
            f.put(
                WIFI_FILE,
                &render_wifi(Some("wlp58s0"), &[net("home", None, false)], &dns2()),
            );
            let nets = vec![net("lab", None, false)];
            let c = Change {
                wifi: Some(nets.clone()),
                wired: None,
            };
            save(&f.h, &c, false).unwrap();
            let w = parse_wifi(f.text(WIFI_FILE).as_deref());
            assert_eq!((w.networks, w.dns), (nets, dns2()));
            assert_eq!(f.text(WIRED_FILE).as_deref(), Some(wired.as_str()));
        }
    }

    #[test]
    fn save_keeps_the_wifi_dns_on_netboot() {
        let f = Fixture::new("dns-netboot", &["wlp58s0"]);
        let nets = vec![net("home", Some("secret123"), false)];
        // Left in the home from a stick boot.
        f.put(WIRED_FILE, &render_wired(SHIPPED_WIRED, &dhcp_dns()).unwrap().unwrap());
        f.put(WIFI_FILE, &render_wifi(Some("wlp58s0"), &nets, &dns2()));
        let e = save(
            &f.h,
            &Change {
                wifi: None,
                wired: Some(dhcp_dns()),
            },
            true,
        )
        .unwrap_err();
        assert!(e.contains("netboot"), "{e}");
        save(
            &f.h,
            &Change {
                wifi: Some(nets.clone()),
                wired: None,
            },
            true,
        )
        .unwrap();
        // Nothing written, nothing removed.
        let w = parse_wifi(f.text(WIFI_FILE).as_deref());
        assert_eq!(w.networks, nets);
        assert_eq!(w.dns, dns2());
        // Going back from a fixed address leaves them too.
        f.put(WIRED_FILE, &render_wired(SHIPPED_WIRED, &fixed()).unwrap().unwrap());
        let c = Change {
            wifi: None,
            wired: Some(Wired::dhcp()),
        };
        save(&f.h, &c, true).unwrap();
        assert_eq!(parse_wifi(f.text(WIFI_FILE).as_deref()).dns, dns2());
        let f = Fixture::new("dns-netboot-none", &["wlp58s0"]);
        f.put(WIFI_FILE, &render_wifi(Some("wlp58s0"), &nets, &[]));
        f.put(WIRED_FILE, &render_wired(SHIPPED_WIRED, &dhcp_dns()).unwrap().unwrap());
        save(
            &f.h,
            &Change {
                wifi: Some(nets.clone()),
                wired: None,
            },
            true,
        )
        .unwrap();
        assert!(parse_wifi(f.text(WIFI_FILE).as_deref()).dns.is_empty());
    }

    #[test]
    fn take_over_keeps_the_dns() {
        let f = Fixture::new("dns-take-over", &["wlp58s0"]);
        let hand = "network:\n  version: 2\n  renderer: networkd\n  wifis:\n    wlp2s0:\n      dhcp4: true\n      dhcp4-overrides:\n        use-dns: false\n      nameservers:\n        addresses: [1.1.1.1, 9.9.9.9]\n      access-points:\n        home:\n          password: secret123\n";
        f.put(WIFI_FILE, hand);
        take_over(&f.h, "wifi").unwrap();
        let w = parse_wifi(f.text(WIFI_FILE).as_deref());
        assert_eq!(w.state, FileState::Menu);
        assert_eq!(w.dns, dns2());
        assert_eq!(w.networks, [net("home", Some("secret123"), false)]);
        let f = Fixture::new("dns-take-over-wired", &["wlp58s0"]);
        let hand = format!(
            "{SHIPPED_WIRED}            dhcp4-overrides:\n                use-dns: false\n            nameservers:\n                addresses: [1.1.1.1, 9.9.9.9]\n"
        );
        f.put(WIRED_FILE, &hand);
        take_over(&f.h, "wired").unwrap();
        let w = parse_wired(f.text(WIRED_FILE).as_deref());
        assert_eq!((w.state, w.wired), (FileState::Menu, dhcp_dns()));
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
}
