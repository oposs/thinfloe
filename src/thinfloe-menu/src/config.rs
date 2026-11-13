//! `~/.config/thinfloe/config.toml`: everything the menu manages, in one
//! file. This module is the schema only: the types, their TOML text and the
//! conversions to the types the menu already works with.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::heads::Rotation;
use crate::kanshi::{self, OutputSet, Place, Profile, Side};
use crate::netfiles::{Net, Wired};

/// First line of every file the menu writes and owns.
pub const CONFIG_HEADER: &str = "# Written by the ThinFloe menu. Edits here are replaced.";

/// The newest (and only) `version` this build reads.
const VERSION: u32 = 1;

/// Longest `environment` file `read_managed_keys` looks at.
const MAX_ENV_BYTES: u64 = 64 * 1024;

/// Keys of the `environment` file the menu owns when the system file is
/// missing.
const DEFAULT_MANAGED: [&str; 4] = [
    "XKB_DEFAULT_MODEL",
    "XKB_DEFAULT_LAYOUT",
    "XKB_DEFAULT_VARIANT",
    "XKB_DEFAULT_OPTIONS",
];

/// The per-user config directory and the paths in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigDir {
    pub root: PathBuf,
}

impl ConfigDir {
    pub fn from_home(home: &Path) -> ConfigDir {
        ConfigDir {
            root: home.join(".config").join("thinfloe"),
        }
    }

    /// `TF_CONFIG_DIR`, else `$HOME/.config/thinfloe`, else tfuser's.
    pub fn from_env() -> ConfigDir {
        if let Some(d) = std::env::var_os("TF_CONFIG_DIR").filter(|d| !d.is_empty()) {
            return ConfigDir { root: PathBuf::from(d) };
        }
        match std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            Some(h) => ConfigDir::from_home(Path::new(&h)),
            None => ConfigDir::from_home(Path::new("/home/tfuser")),
        }
    }

    pub fn config_toml(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn generated(&self) -> PathBuf {
        self.root.join("generated")
    }

    pub fn generated_file(&self, name: &str) -> PathBuf {
        self.generated().join(name)
    }

    pub fn environment(&self) -> PathBuf {
        self.root.join("environment")
    }

    pub fn sessionrc(&self) -> PathBuf {
        self.root.join("sessionrc")
    }

    pub fn home_version(&self) -> PathBuf {
        self.root.join("home-version")
    }

    pub fn password(&self) -> PathBuf {
        self.root.join("password")
    }

    pub fn ssh_host_keys(&self) -> PathBuf {
        self.root.join("ssh-host-keys")
    }

    pub fn wg0(&self) -> PathBuf {
        self.root.join("wg0.conf")
    }

    pub fn save_home_exclude(&self) -> PathBuf {
        self.root.join("save-home-exclude")
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct ThinfloeConfig {
    pub version: u32,
    pub keyboard: Keyboard,
    pub screen: ScreenCfg,
    pub status: StatusCfg,
    pub monitors: MonitorsCfg,
    pub network: NetworkCfg,
    pub notice: NoticeCfg,
}

impl Default for ThinfloeConfig {
    fn default() -> Self {
        ThinfloeConfig {
            version: VERSION,
            keyboard: Keyboard::default(),
            screen: ScreenCfg::default(),
            status: StatusCfg::default(),
            monitors: MonitorsCfg::default(),
            network: NetworkCfg::default(),
            notice: NoticeCfg::default(),
        }
    }
}

// ---- keyboard ----

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compose {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "ralt")]
    RightAlt,
    #[serde(rename = "menu")]
    Menu,
    #[serde(rename = "rctrl")]
    RightCtrl,
    #[serde(rename = "lwin")]
    LeftWin,
    #[serde(rename = "rwin")]
    RightWin,
    #[serde(rename = "sclk")]
    ScrollLock,
    #[serde(rename = "paus")]
    Pause,
}

impl Compose {
    pub const ALL: [Compose; 8] = [
        Compose::None,
        Compose::RightAlt,
        Compose::Menu,
        Compose::RightCtrl,
        Compose::LeftWin,
        Compose::RightWin,
        Compose::ScrollLock,
        Compose::Pause,
    ];

    /// The XKB option, `None` for no compose key.
    pub fn option(self) -> Option<&'static str> {
        Some(match self {
            Compose::None => return None,
            Compose::RightAlt => "compose:ralt",
            Compose::Menu => "compose:menu",
            Compose::RightCtrl => "compose:rctrl",
            Compose::LeftWin => "compose:lwin",
            Compose::RightWin => "compose:rwin",
            Compose::ScrollLock => "compose:sclk",
            Compose::Pause => "compose:paus",
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Compose::None => "None",
            Compose::RightAlt => "Right Alt",
            Compose::Menu => "Menu",
            Compose::RightCtrl => "Right Ctrl",
            Compose::LeftWin => "Left Win",
            Compose::RightWin => "Right Win",
            Compose::ScrollLock => "Scroll Lock",
            Compose::Pause => "Pause",
        }
    }

    pub fn from_option(opt: &str) -> Option<Compose> {
        Compose::ALL.into_iter().find(|c| c.option() == Some(opt))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct Keyboard {
    pub layout: String,
    pub variant: String,
    pub caps_as_ctrl: bool,
    pub compose: Compose,
}

impl Default for Keyboard {
    fn default() -> Self {
        Keyboard {
            layout: "us".into(),
            variant: String::new(),
            caps_as_ctrl: true,
            compose: Compose::RightAlt,
        }
    }
}

/// `XKB_DEFAULT_OPTIONS`: the compose option first, then `ctrl:nocaps`.
pub fn xkb_options(kb: &Keyboard) -> String {
    let mut v: Vec<&str> = Vec::new();
    v.extend(kb.compose.option());
    if kb.caps_as_ctrl {
        v.push("ctrl:nocaps");
    }
    v.join(",")
}

/// An XKB layout or variant name: empty, or 1 to 64 of `A-Z a-z 0-9 _ -`.
pub fn xkb_name_ok(s: &str) -> bool {
    s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

// ---- screen, status, notice ----

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct ScreenCfg {
    pub idle_timeout: u32,
}

impl Default for ScreenCfg {
    fn default() -> Self {
        ScreenCfg {
            idle_timeout: crate::screentab::DEFAULT_IDLE,
        }
    }
}

/// `lines`: the status layout, one `layout::to_text` line per entry;
/// `None` is the default list.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct StatusCfg {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<String>>,
}

/// The migration report the Settings screen shows until dismissed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct NoticeCfg {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migration: Option<String>,
}

// ---- monitors ----

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct MonitorsCfg {
    pub profiles: Vec<ProfileCfg>,
    /// Text of a hand-written kanshi file the menu keeps until it is replaced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ProfileCfg {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub menu_output: Option<String>,
    pub outputs: Vec<OutputCfg>,
}

/// `mode` is kanshi's mode text, `rotation` one of `normal 90 180 270`;
/// `side` and `to` are absent for the output at the origin.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct OutputCfg {
    pub criteria: String,
    pub enabled: bool,
    pub mode: String,
    pub rotation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

impl Default for OutputCfg {
    fn default() -> Self {
        OutputCfg {
            criteria: String::new(),
            enabled: true,
            mode: "0x0".into(),
            rotation: "normal".into(),
            side: None,
            to: None,
        }
    }
}

fn side_word(s: Side) -> &'static str {
    match s {
        Side::LeftOf => "left-of",
        Side::RightOf => "right-of",
        Side::Above => "above",
        Side::Below => "below",
    }
}

fn side_from_word(w: &str) -> Option<Side> {
    [Side::LeftOf, Side::RightOf, Side::Above, Side::Below]
        .into_iter()
        .find(|s| side_word(*s) == w)
}

fn rotation_word(r: Rotation) -> &'static str {
    match r {
        Rotation::Normal => "normal",
        Rotation::R90 => "90",
        Rotation::R180 => "180",
        Rotation::R270 => "270",
    }
}

fn rotation_from_word(w: &str) -> Option<Rotation> {
    [Rotation::Normal, Rotation::R90, Rotation::R180, Rotation::R270]
        .into_iter()
        .find(|r| rotation_word(*r) == w)
}

impl OutputCfg {
    fn to_set(&self) -> Result<OutputSet, String> {
        let name = &self.criteria;
        let mode = kanshi::parse_mode(&self.mode).ok_or(format!("Output {name}: bad mode {}", self.mode))?;
        let rotation =
            rotation_from_word(&self.rotation).ok_or(format!("Output {name}: bad rotation {}", self.rotation))?;
        let place = match (&self.side, &self.to) {
            (None, None) => Place::Origin,
            (Some(side), Some(to)) => Place::Next {
                to: to.clone(),
                side: side_from_word(side).ok_or(format!("Output {name}: bad side {side}"))?,
            },
            _ => return Err(format!("Output {name}: side and to belong together")),
        };
        Ok(OutputSet {
            criteria: name.clone(),
            enabled: self.enabled,
            mode,
            rotation,
            place,
        })
    }

    fn from_set(o: &OutputSet) -> OutputCfg {
        let (side, to) = match &o.place {
            Place::Origin => (None, None),
            Place::Next { to, side } => (Some(side_word(*side).to_string()), Some(to.clone())),
        };
        OutputCfg {
            criteria: o.criteria.clone(),
            enabled: o.enabled,
            mode: kanshi::mode_text(&o.mode),
            rotation: rotation_word(o.rotation).to_string(),
            side,
            to,
        }
    }
}

impl MonitorsCfg {
    pub fn to_profiles(&self) -> Result<Vec<Profile>, String> {
        self.profiles
            .iter()
            .map(|p| {
                Ok(Profile {
                    outputs: p.outputs.iter().map(OutputCfg::to_set).collect::<Result<_, _>>()?,
                    menu_output: p.menu_output.clone(),
                })
            })
            .collect()
    }

    pub fn from_profiles(p: &[Profile]) -> MonitorsCfg {
        MonitorsCfg {
            profiles: p
                .iter()
                .map(|p| ProfileCfg {
                    menu_output: p.menu_output.clone(),
                    outputs: p.outputs.iter().map(OutputCfg::from_set).collect(),
                })
                .collect(),
            kept: None,
        }
    }
}

// ---- network ----

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct NetworkCfg {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wifi_device: Option<String>,
    pub networks: Vec<NetCfg>,
    /// The servers in the WiFi file; on netboot they differ from `wired.dns`.
    pub wifi_dns: Vec<String>,
    pub wired: WiredCfg,
    /// Text of a hand-written WiFi file the menu keeps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_wifi: Option<String>,
    /// Text of a hand-written wired file the menu keeps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_wired: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct NetCfg {
    pub ssid: String,
    pub hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// `mode` is `"dhcp"` or `"static"`; the address fields belong to static.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct WiredCfg {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    pub dns: Vec<String>,
}

impl Default for WiredCfg {
    fn default() -> Self {
        WiredCfg {
            mode: "dhcp".into(),
            address: None,
            prefix: None,
            gateway: None,
            dns: Vec::new(),
        }
    }
}

fn ipv4(s: &str, what: &str) -> Result<Ipv4Addr, String> {
    s.parse().map_err(|_| format!("{what}: not an IPv4 address: {s}"))
}

fn ipv4_list(v: &[String], what: &str) -> Result<Vec<Ipv4Addr>, String> {
    v.iter().map(|s| ipv4(s, what)).collect()
}

impl NetworkCfg {
    pub fn nets(&self) -> Vec<Net> {
        self.networks
            .iter()
            .map(|n| Net {
                ssid: n.ssid.clone(),
                hidden: n.hidden,
                password: n.password.clone(),
            })
            .collect()
    }

    pub fn set_nets(&mut self, n: &[Net]) {
        self.networks = n
            .iter()
            .map(|n| NetCfg {
                ssid: n.ssid.clone(),
                hidden: n.hidden,
                password: n.password.clone(),
            })
            .collect();
    }

    pub fn wired(&self) -> Result<Wired, String> {
        let w = &self.wired;
        let dns = ipv4_list(&w.dns, "Wired DNS")?;
        match w.mode.as_str() {
            "dhcp" => Ok(Wired::Dhcp { dns }),
            "static" => {
                let address = ipv4(
                    w.address.as_deref().ok_or("Static wired: address missing")?,
                    "Wired address",
                )?;
                let prefix = w
                    .prefix
                    .filter(|p| *p <= 32)
                    .ok_or("Static wired: prefix missing or over 32")?;
                let gateway = w.gateway.as_deref().map(|g| ipv4(g, "Wired gateway")).transpose()?;
                Ok(Wired::Static {
                    address,
                    prefix,
                    gateway,
                    dns,
                })
            }
            other => Err(format!("Wired mode must be dhcp or static, not {other}")),
        }
    }

    pub fn set_wired(&mut self, w: &Wired) {
        let strings = |v: &[Ipv4Addr]| v.iter().map(Ipv4Addr::to_string).collect();
        self.wired = match w {
            Wired::Dhcp { dns } => WiredCfg {
                dns: strings(dns),
                ..WiredCfg::default()
            },
            Wired::Static {
                address,
                prefix,
                gateway,
                dns,
            } => WiredCfg {
                mode: "static".into(),
                address: Some(address.to_string()),
                prefix: Some(*prefix),
                gateway: gateway.map(|g| g.to_string()),
                dns: strings(dns),
            },
        };
    }
}

// ---- text ----

/// Reads `config.toml` text. Unknown keys are ignored; an unknown `version`
/// or a layout or variant that is no XKB name is an error.
pub fn parse(text: &str) -> Result<ThinfloeConfig, String> {
    let mut cfg: ThinfloeConfig = toml::from_str(text).map_err(|e| format!("config.toml: {e}"))?;
    if cfg.version != VERSION {
        return Err(format!("config.toml: version {} is not supported", cfg.version));
    }
    for (what, v) in [("layout", &cfg.keyboard.layout), ("variant", &cfg.keyboard.variant)] {
        if !xkb_name_ok(v) {
            return Err(format!("config.toml: keyboard {what} is not a valid XKB name"));
        }
    }
    cfg.screen.idle_timeout = cfg.screen.idle_timeout.min(crate::screentab::MAX_IDLE);
    Ok(cfg)
}

pub fn to_text(cfg: &ThinfloeConfig) -> String {
    let body = toml::to_string(cfg).expect("config serialises to TOML");
    format!("{CONFIG_HEADER}\n{body}")
}

// ---- environment file ----

/// The key of an active `environment` line: after optional leading blanks
/// and `export `, up to the `=`.
fn env_key(line: &str) -> Option<&str> {
    let l = line.trim_start();
    if l.starts_with('#') {
        return None;
    }
    let l = l.strip_prefix("export ").map_or(l, str::trim_start);
    l.split_once('=').map(|(k, _)| k.trim_end())
}

/// The active lines of the user's `environment` whose key is managed.
pub fn dropped_env_lines(user_env: &str, managed: &[String]) -> Vec<String> {
    user_env
        .lines()
        .filter(|l| env_key(l).is_some_and(|k| managed.iter().any(|m| m == k)))
        .map(str::to_string)
        .collect()
}

/// Keys of the system `environment` file; the four keyboard keys when it is
/// missing.
pub fn read_managed_keys(path: &Path) -> Vec<String> {
    match crate::bounded::read_text(path, MAX_ENV_BYTES) {
        Some(t) => t.lines().filter_map(env_key).map(str::to_string).collect(),
        None => DEFAULT_MANAGED.iter().map(|s| s.to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heads::Mode;

    fn sample_profiles() -> Vec<Profile> {
        let mode = |w, h, mhz| Mode {
            w,
            h,
            refresh_mhz: mhz,
            preferred: false,
        };
        let set = |c: &str, enabled, mode, place| OutputSet {
            criteria: c.into(),
            enabled,
            mode,
            rotation: Rotation::Normal,
            place,
        };
        let mut right = set(
            "DP-2",
            true,
            mode(1080, 1920, 60000),
            Place::Next {
                to: "Dell Inc. U2419H ABC".into(),
                side: Side::RightOf,
            },
        );
        right.rotation = Rotation::R270;
        let off = set(
            "DP-3",
            false,
            mode(0, 0, 0),
            Place::Next {
                to: "DP-2".into(),
                side: Side::Below,
            },
        );
        vec![
            Profile {
                outputs: vec![
                    set("Dell Inc. U2419H ABC", true, mode(1920, 1080, 59940), Place::Origin),
                    right,
                    off,
                ],
                menu_output: Some("Dell Inc. U2419H ABC".into()),
            },
            Profile {
                outputs: vec![set("eDP-1", true, mode(1920, 1080, 0), Place::Origin)],
                menu_output: None,
            },
        ]
    }

    #[test]
    fn a_default_config_round_trips() {
        let d = ThinfloeConfig::default();
        let text = to_text(&d);
        assert!(text.starts_with(CONFIG_HEADER), "{text}");
        assert!(text.contains("version = 1"), "{text}");
        assert_eq!(parse(&text).unwrap(), d);
    }

    #[test]
    fn each_table_round_trips() {
        let mut c = ThinfloeConfig {
            keyboard: Keyboard {
                layout: "ch".into(),
                variant: "de_nodeadkeys".into(),
                caps_as_ctrl: false,
                compose: Compose::Menu,
            },
            ..ThinfloeConfig::default()
        };
        c.screen.idle_timeout = 900;
        c.status.lines = Some(vec!["clock 1x1".into(), "-battery 2x1".into()]);
        c.monitors = MonitorsCfg::from_profiles(&sample_profiles());
        c.network.wifi_device = Some("wlan0".into());
        c.network.networks = vec![NetCfg {
            ssid: "Office".into(),
            hidden: true,
            password: Some("00012345".into()),
        }];
        c.network.wifi_dns = vec!["1.1.1.1".into()];
        c.network.set_wired(&Wired::Static {
            address: "192.168.1.10".parse().unwrap(),
            prefix: 24,
            gateway: Some("192.168.1.1".parse().unwrap()),
            dns: vec!["9.9.9.9".parse().unwrap()],
        });
        c.network.kept_wifi = Some("network:\n  \"x\": 1\n".into());
        c.network.kept_wired = Some("a \"b\"".into());
        c.notice.migration = Some("x".into());
        let text = to_text(&c);
        let back = parse(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(back, c);
        assert_eq!(back.network.networks[0].password.as_deref(), Some("00012345"));
    }

    #[test]
    fn xkb_options_order_and_empties() {
        let mut kb = Keyboard::default();
        assert_eq!(xkb_options(&kb), "compose:ralt,ctrl:nocaps");
        kb.caps_as_ctrl = false;
        kb.compose = Compose::None;
        assert_eq!(xkb_options(&kb), "");
        kb.caps_as_ctrl = true;
        assert_eq!(xkb_options(&kb), "ctrl:nocaps");
        kb.caps_as_ctrl = false;
        kb.compose = Compose::Pause;
        assert_eq!(xkb_options(&kb), "compose:paus");
    }

    #[test]
    fn compose_maps_every_choice() {
        let opts: Vec<Option<&str>> = Compose::ALL.iter().map(|c| c.option()).collect();
        assert_eq!(
            opts,
            [
                None,
                Some("compose:ralt"),
                Some("compose:menu"),
                Some("compose:rctrl"),
                Some("compose:lwin"),
                Some("compose:rwin"),
                Some("compose:sclk"),
                Some("compose:paus"),
            ]
        );
        for c in Compose::ALL {
            if let Some(o) = c.option() {
                assert_eq!(Compose::from_option(o), Some(c));
            }
        }
        assert_eq!(Compose::from_option("compose:nope"), None);
        let labels: Vec<&str> = Compose::ALL.iter().map(|c| c.label()).collect();
        assert_eq!(labels[1], "Right Alt");
        assert_eq!(labels[7], "Pause");
    }

    #[test]
    fn compose_is_stored_as_its_short_name() {
        for (c, name) in Compose::ALL
            .into_iter()
            .zip(["none", "ralt", "menu", "rctrl", "lwin", "rwin", "sclk", "paus"])
        {
            let mut cfg = ThinfloeConfig::default();
            cfg.keyboard.compose = c;
            assert!(to_text(&cfg).contains(&format!("compose = \"{name}\"")));
        }
    }

    #[test]
    fn bad_xkb_names_are_refused() {
        for bad in ["us\nX=1", "us de", "us=", "\"us\""] {
            let text = format!("[keyboard]\nlayout = {}\n", toml::Value::String(bad.into()));
            assert!(parse(&text).is_err(), "{bad:?}");
            let text = format!("[keyboard]\nvariant = {}\n", toml::Value::String(bad.into()));
            assert!(parse(&text).is_err(), "{bad:?}");
        }
        assert!(xkb_name_ok("de_nodeadkeys"));
        assert!(xkb_name_ok(""));
        assert!(!xkb_name_ok(&"a".repeat(65)));
    }

    #[test]
    fn an_unknown_version_is_refused() {
        assert!(parse("version = 2").is_err());
        assert!(parse("version = 0").is_err());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let c = parse("[keyboard]\nlayout = \"de\"\nfoo = 1\n[future]\nx = 2\n").unwrap();
        assert_eq!(c.keyboard.layout, "de");
    }

    #[test]
    fn a_long_idle_timeout_is_capped() {
        let c = parse("[screen]\nidle_timeout = 999999\n").unwrap();
        assert_eq!(c.screen.idle_timeout, crate::screentab::MAX_IDLE);
    }

    #[test]
    fn monitors_convert_to_kanshi_profiles_and_back() {
        let sample = sample_profiles();
        let cfg = MonitorsCfg::from_profiles(&sample);
        let back = cfg.to_profiles().unwrap();
        assert_eq!(back, sample);
        assert_eq!(MonitorsCfg::from_profiles(&back), cfg);
        assert_eq!(kanshi::render(&back), kanshi::render(&sample));
    }

    #[test]
    fn broken_monitor_and_network_values_are_errors() {
        let mut m = MonitorsCfg::from_profiles(&sample_profiles());
        m.profiles[0].outputs[1].side = None;
        assert!(m.to_profiles().is_err());
        let mut m = MonitorsCfg::from_profiles(&sample_profiles());
        m.profiles[0].outputs[0].rotation = "45".into();
        assert!(m.to_profiles().is_err());
        let mut n = NetworkCfg::default();
        n.wired.mode = "static".into();
        assert!(n.wired().is_err());
        n.wired.mode = "dhcp".into();
        n.wired.dns = vec!["nope".into()];
        assert!(n.wired().is_err());
    }

    #[test]
    fn nets_and_wired_convert() {
        let mut n = NetworkCfg::default();
        assert_eq!(n.wired().unwrap(), Wired::dhcp());
        let nets = vec![Net {
            ssid: "a".into(),
            hidden: false,
            password: None,
        }];
        n.set_nets(&nets);
        assert_eq!(n.nets(), nets);
        let w = Wired::Dhcp {
            dns: vec!["8.8.8.8".parse().unwrap()],
        };
        n.set_wired(&w);
        assert_eq!(n.wired().unwrap(), w);
    }

    #[test]
    fn dropped_env_lines_name_managed_keys_only() {
        let managed: Vec<String> = DEFAULT_MANAGED.iter().map(|s| s.to_string()).collect();
        let env = "XKB_DEFAULT_LAYOUT=de\n  export XKB_DEFAULT_MODEL=pc104\n#XKB_DEFAULT_VARIANT=x\nXCURSOR_SIZE=32\n";
        assert_eq!(
            dropped_env_lines(env, &managed),
            ["XKB_DEFAULT_LAYOUT=de", "  export XKB_DEFAULT_MODEL=pc104"]
        );
    }

    #[test]
    fn managed_keys_come_from_the_file_or_the_default() {
        let dir = crate::testdir::TestDir::new("config", "managed");
        let f = dir.join("environment");
        std::fs::write(&f, "# c\n\nXKB_DEFAULT_LAYOUT=us\nXCURSOR_SIZE=24\n").unwrap();
        assert_eq!(read_managed_keys(&f), ["XKB_DEFAULT_LAYOUT", "XCURSOR_SIZE"]);
        assert_eq!(read_managed_keys(&dir.join("missing")), DEFAULT_MANAGED);
    }

    #[test]
    fn config_dir_paths() {
        let d = ConfigDir::from_home(Path::new("/h"));
        assert_eq!(d.root, Path::new("/h/.config/thinfloe"));
        assert_eq!(d.config_toml(), Path::new("/h/.config/thinfloe/config.toml"));
        assert_eq!(
            d.generated_file("kanshi"),
            Path::new("/h/.config/thinfloe/generated/kanshi")
        );
        assert_eq!(d.wg0(), Path::new("/h/.config/thinfloe/wg0.conf"));
    }
}
