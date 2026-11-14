//! Readers for the old settings files and for `generated/`: each turns a
//! file's text into the config table that replaces it, with report lines
//! (English sentences naming the file) for what could not be carried over.
//! A WiFi or wired file the menu cannot keep without losing a setting is
//! kept verbatim, never converted in part.

use std::collections::HashMap;
use std::path::Path;

use crate::config::{Compose, Keyboard, MonitorsCfg, NetworkCfg, StatusCfg, ThinfloeConfig, xkb_name_ok};
use crate::netfiles::{FileState, parse_wifi, parse_wired};
use crate::status::{self, layout};
use crate::{atomic, generated, kanshi, screentab};

/// A converted table and the report lines for what was dropped or kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted<T> {
    pub value: T,
    pub notes: Vec<String>,
}

/// Longest generated file `from_generated` reads.
const MAX_BYTES: u64 = generated::MAX_BYTES;

fn note_kept(file: &str, why: &str) -> String {
    format!("{file} is kept as written; the menu shows it read-only. {why}")
}

// ---- environment ----

/// What follows `KEY=` on a shell line.
#[derive(Debug, PartialEq, Eq)]
enum Value {
    /// The shell word; a comment after it is ignored.
    Word(String),
    /// A command or operator follows the word: not a plain assignment.
    Trailing,
}

/// The shell word at the start of `v`, quotes removed: it ends at the first
/// unquoted blank, `#`, `;`, `&` or `|`.
fn shell_word(v: &str) -> Value {
    let mut word = String::new();
    let mut chars = v.chars();
    let mut tail = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' => {
                for q in chars.by_ref() {
                    if q == c {
                        break;
                    }
                    word.push(q);
                }
            }
            ' ' | '\t' | '#' | ';' | '&' | '|' => {
                tail.push(c);
                tail.extend(chars);
                break;
            }
            _ => word.push(c),
        }
    }
    let tail = tail.trim_start_matches([' ', '\t']);
    if tail.is_empty() || tail.starts_with('#') {
        Value::Word(word)
    } else {
        Value::Trailing
    }
}

/// `KEY=value` of an active shell line (`export ` allowed).
fn assignment(line: &str) -> Option<(&str, Value)> {
    let l = line.trim_start();
    if l.starts_with('#') {
        return None;
    }
    let l = l.strip_prefix("export ").map_or(l, str::trim_start);
    let (k, v) = l.split_once('=')?;
    Some((k.trim_end(), shell_word(v)))
}

const KB_KEYS: [&str; 4] = [
    "XKB_DEFAULT_MODEL",
    "XKB_DEFAULT_LAYOUT",
    "XKB_DEFAULT_VARIANT",
    "XKB_DEFAULT_OPTIONS",
];

/// The first entry of a comma list; a longer list is noted.
fn first_entry(key: &str, value: &str, notes: &mut Vec<String>) -> String {
    let first = value.split(',').next().unwrap_or("");
    if value.contains(',') {
        notes.push(format!(
            "environment: {key}={value} lists several; only \"{first}\" is kept."
        ));
    }
    first.to_string()
}

/// The keyboard of an `environment` file, its other active lines (kept in
/// the user's own file) and the report lines.
pub fn keyboard_from_env(text: &str) -> (Keyboard, Vec<String>, Vec<String>) {
    let mut last: HashMap<&str, String> = HashMap::new();
    let mut other = Vec::new();
    let mut notes = Vec::new();
    for line in text.lines() {
        match assignment(line) {
            Some((k, Value::Word(v))) if KB_KEYS.contains(&k) => {
                last.insert(k, v);
            }
            Some((k, Value::Trailing)) if KB_KEYS.contains(&k) => {
                notes.push(format!(
                    "environment: the line \"{}\" is not a plain assignment and is ignored.",
                    line.trim()
                ));
            }
            Some(_) => other.push(line.trim_end().to_string()),
            None if !line.trim().is_empty() && !line.trim_start().starts_with('#') => {
                notes.push(format!(
                    "environment: the line \"{}\" is not a variable assignment; it is kept in the attic only.",
                    line.trim()
                ));
            }
            None => {}
        }
    }
    let mut kb = Keyboard::default();

    if let Some(model) = last
        .get("XKB_DEFAULT_MODEL")
        .filter(|m| !m.is_empty() && **m != "pc105")
    {
        notes.push(format!(
            "environment: XKB_DEFAULT_MODEL={model} is dropped; the menu uses pc105."
        ));
    }

    let layout = last
        .get("XKB_DEFAULT_LAYOUT")
        .filter(|v| !v.is_empty())
        .map(|v| first_entry("XKB_DEFAULT_LAYOUT", v, &mut notes));
    let variant = last
        .get("XKB_DEFAULT_VARIANT")
        .map(|v| first_entry("XKB_DEFAULT_VARIANT", v, &mut notes));
    match layout {
        Some(l) if !xkb_name_ok(&l) || l.is_empty() => {
            notes.push(format!(
                "environment: the layout \"{l}\" is not an XKB name and is dropped; the layout stays {}.",
                kb.layout
            ));
        }
        Some(l) => {
            kb.layout = l;
            match variant {
                Some(v) if xkb_name_ok(&v) => kb.variant = v,
                Some(v) => notes.push(format!(
                    "environment: the variant \"{v}\" is not an XKB name and is dropped."
                )),
                None => {}
            }
        }
        None => {
            if variant.is_some_and(|v| !v.is_empty()) {
                notes.push("environment: XKB_DEFAULT_VARIANT is dropped; it needs a layout.".into());
            }
        }
    }

    if let Some(options) = last.get("XKB_DEFAULT_OPTIONS") {
        kb.caps_as_ctrl = false;
        kb.compose = Compose::None;
        let mut compose_seen = false;
        for opt in options.split(',').map(str::trim).filter(|o| !o.is_empty()) {
            if opt == "ctrl:nocaps" {
                kb.caps_as_ctrl = true;
            } else if let (Some(c), false) = (Compose::from_option(opt), compose_seen) {
                kb.compose = c;
                compose_seen = true;
            } else {
                notes.push(format!(
                    "environment: the option {opt} is dropped; the menu does not manage it."
                ));
            }
        }
    }
    (kb, other, notes)
}

// ---- sessionrc ----

/// `TF_IDLE_TIMEOUT` as the session reads it: blanks and `+` ignored,
/// plain digits above zero, else 0; capped at the menu's maximum.
fn normalise_idle(v: &str) -> u32 {
    let v = v.trim_matches([' ', '\t']);
    let v = v.strip_prefix('+').unwrap_or(v);
    match v.parse::<u32>() {
        Ok(n) if v.bytes().all(|b| b.is_ascii_digit()) => n.min(screentab::MAX_IDLE),
        _ => 0,
    }
}

/// The idle time from the last `TF_IDLE_TIMEOUT=` assignment (else
/// `DBRRG_IDLE_TIMEOUT=`) and the file without those lines.
pub fn idle_from_sessionrc(text: &str) -> (Option<u32>, String, Vec<String>) {
    let (mut tf, mut dbrrg) = (None, None);
    let mut rest = String::new();
    let mut notes = Vec::new();
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n');
        match assignment(trimmed) {
            Some((k @ ("TF_IDLE_TIMEOUT" | "DBRRG_IDLE_TIMEOUT"), Value::Word(v))) => {
                let n = Some(normalise_idle(&v));
                if k == "TF_IDLE_TIMEOUT" {
                    tf = n;
                } else {
                    dbrrg = n;
                }
            }
            Some(("TF_IDLE_TIMEOUT" | "DBRRG_IDLE_TIMEOUT", Value::Trailing)) => {
                notes.push(format!(
                    "sessionrc: the line \"{}\" is not a plain assignment and is left as it is.",
                    trimmed.trim()
                ));
                rest.push_str(line);
            }
            _ => rest.push_str(line),
        }
    }
    (tf.or(dbrrg), rest, notes)
}

// ---- screen, status, monitors ----

pub fn screen_from_file(text: &str) -> Converted<Option<u32>> {
    let value = screentab::parse_idle(text);
    let mut notes = Vec::new();
    if value.is_none() && text.lines().any(|l| l.starts_with("idle_timeout=")) {
        notes.push("screen: the idle_timeout value could not be read; the default is used.".into());
    }
    Converted { value, notes }
}

pub fn status_from_file(bytes: &[u8]) -> Converted<StatusCfg> {
    let l = layout::parse(bytes, &status::offered);
    let lines = (!l.refused && !l.choices.is_empty()).then(|| {
        l.choices
            .iter()
            .map(|c| format!("{}{} {}", if c.shown { "" } else { "-" }, c.id, c.size))
            .collect()
    });
    Converted {
        value: StatusCfg { lines },
        notes: l.notes,
    }
}

pub fn monitors_from_kanshi(text: &str) -> Converted<MonitorsCfg> {
    // A name the generated file could not hold again is no profile either.
    let parsed = kanshi::parse(text).and_then(|p| {
        let cfg = MonitorsCfg::from_profiles(&p);
        cfg.to_profiles().map(|_| cfg)
    });
    match parsed {
        Ok(cfg) => Converted {
            value: cfg,
            notes: Vec::new(),
        },
        Err(e) => Converted {
            value: MonitorsCfg {
                kept: Some(text.to_string()),
                ..MonitorsCfg::default()
            },
            notes: vec![note_kept("kanshi", &e)],
        },
    }
}

// ---- network ----

pub fn network_from_files(wifi: Option<&str>, wired: Option<&str>) -> Converted<NetworkCfg> {
    let mut net = NetworkCfg::default();
    let mut notes = Vec::new();

    let w = parse_wifi(wifi);
    if !matches!(w.state, FileState::Missing | FileState::Template) {
        if w.manageable {
            net.wifi_device = w.device;
            net.set_nets(&w.networks);
            net.wifi_dns = w.dns.iter().map(|d| d.to_string()).collect();
        } else {
            net.kept_wifi = wifi.map(str::to_string);
            notes.push(note_kept(
                "wifi.yaml",
                w.error.as_deref().unwrap_or("It has settings the menu cannot edit."),
            ));
        }
    }

    let e = parse_wired(wired);
    if e.state != FileState::Missing {
        if e.manageable {
            net.set_wired(&e.wired);
        } else {
            net.kept_wired = wired.map(str::to_string);
            notes.push(note_kept(
                "ethernet.yaml",
                e.error.as_deref().unwrap_or("It has settings the menu cannot edit."),
            ));
        }
    }
    Converted { value: net, notes }
}

// ---- generated/ ----

/// `text` without the generated header line.
fn strip_header(text: &str) -> &str {
    text.strip_prefix(generated::HEADER)
        .and_then(|r| r.strip_prefix('\n'))
        .unwrap_or(text)
}

fn read_generated(dir: &Path, name: &str) -> Result<Option<String>, String> {
    atomic::read(&dir.join(name), MAX_BYTES)
        .map(|b| b.map(|b| String::from_utf8_lossy(&b).into_owned()))
        .map_err(|e| format!("generated/{name}: {e}"))
}

/// The config rebuilt from `generated/`; `Ok(None)` when it holds no file.
/// `[status]` and `[notice]` stay default.
pub fn from_generated(dir: &Path) -> Result<Option<ThinfloeConfig>, String> {
    let env = read_generated(dir, "environment")?;
    let screen = read_generated(dir, "screen")?;
    let kanshi_text = read_generated(dir, "kanshi")?;
    let wifi = read_generated(dir, "wifi.yaml")?;
    let wired = read_generated(dir, "ethernet.yaml")?;
    if [&env, &screen, &kanshi_text, &wifi, &wired].iter().all(|f| f.is_none()) {
        return Ok(None);
    }
    let mut cfg = ThinfloeConfig::default();
    if let Some(t) = &env {
        cfg.keyboard = keyboard_from_env(strip_header(t)).0;
    }
    if let Some(n) = screen.as_deref().and_then(|t| screen_from_file(strip_header(t)).value) {
        cfg.screen.idle_timeout = n;
    }
    if let Some(t) = &kanshi_text {
        // `kanshi::parse` takes the generated header itself; only a kept
        // hand file loses it, so that it is kept as the user wrote it.
        cfg.monitors = monitors_from_kanshi(t).value;
        if cfg.monitors.kept.is_some() {
            cfg.monitors.kept = Some(strip_header(t).to_string());
        }
    }
    cfg.network = network_from_files(wifi.as_deref().map(strip_header), wired.as_deref().map(strip_header)).value;
    Ok(Some(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NetCfg;
    use crate::generated::Inputs;
    use crate::netfiles::Wired;
    use crate::testdir::TestDir;

    fn kb(layout: &str, variant: &str, caps: bool, compose: Compose) -> Keyboard {
        Keyboard {
            layout: layout.into(),
            variant: variant.into(),
            caps_as_ctrl: caps,
            compose,
        }
    }

    #[test]
    fn keyboard_from_a_hand_environment() {
        let text = "XKB_DEFAULT_LAYOUT=ch\nXKB_DEFAULT_VARIANT=de_nodeadkeys\nXKB_DEFAULT_OPTIONS=grp:alt_shift_toggle,compose:menu,ctrl:nocaps\nXCURSOR_SIZE=32\n";
        let (k, other, notes) = keyboard_from_env(text);
        assert_eq!(k, kb("ch", "de_nodeadkeys", true, Compose::Menu));
        assert_eq!(other, ["XCURSOR_SIZE=32"]);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("grp:alt_shift_toggle"), "{notes:?}");
    }

    #[test]
    fn two_layouts_keep_the_first() {
        let (k, _, notes) = keyboard_from_env("XKB_DEFAULT_LAYOUT=us,de\nXKB_DEFAULT_VARIANT=,nodeadkeys\n");
        assert_eq!(k.layout, "us");
        assert_eq!(k.variant, "");
        let all = notes.join("\n");
        assert!(all.contains("us,de") && all.contains(",nodeadkeys"), "{notes:?}");
    }

    #[test]
    fn a_model_other_than_pc105_is_dropped() {
        let (_, _, notes) = keyboard_from_env("XKB_DEFAULT_MODEL=pc104\n");
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("pc104"));
        let (_, other, notes) = keyboard_from_env("XKB_DEFAULT_MODEL=pc105\n");
        assert!(notes.is_empty(), "{notes:?}");
        assert!(other.is_empty());
    }

    #[test]
    fn options_without_nocaps_turn_caps_off() {
        let (k, _, _) = keyboard_from_env("XKB_DEFAULT_OPTIONS=compose:ralt\n");
        assert!(!k.caps_as_ctrl);
        assert_eq!(k.compose, Compose::RightAlt);
    }

    #[test]
    fn absent_options_keep_the_default() {
        let (k, other, notes) = keyboard_from_env("XKB_DEFAULT_LAYOUT=de\n# c\n\n");
        assert_eq!(k, kb("de", "", true, Compose::RightAlt));
        assert!(other.is_empty() && notes.is_empty());
    }

    #[test]
    fn the_last_assignment_wins() {
        let (k, _, _) = keyboard_from_env("XKB_DEFAULT_LAYOUT=fr\nexport XKB_DEFAULT_LAYOUT=\"de\"\n");
        assert_eq!(k.layout, "de");
    }

    #[test]
    fn a_line_that_is_no_assignment_is_noted() {
        let (k, other, notes) = keyboard_from_env("export FOO\nXKB_DEFAULT_LAYOUT=de\n# c\n\n");
        assert_eq!(k.layout, "de");
        assert!(other.is_empty(), "{other:?}");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("\"export FOO\""), "{notes:?}");
        assert!(notes[0].contains("not a variable assignment"), "{notes:?}");
    }

    #[test]
    fn a_name_with_a_blank_is_dropped() {
        let (k, _, notes) = keyboard_from_env("XKB_DEFAULT_LAYOUT=de x\n");
        assert_eq!(k.layout, "us");
        assert_eq!(notes.len(), 1, "{notes:?}");
    }

    #[test]
    fn idle_from_sessionrc_takes_the_last() {
        let (n, rest, _) = idle_from_sessionrc(
            "TF_IDLE_TIMEOUT=60\nwlr-randr --output DP-1 --transform 90\nexport TF_IDLE_TIMEOUT=0900\n",
        );
        assert_eq!(n, Some(900));
        assert_eq!(rest, "wlr-randr --output DP-1 --transform 90\n");
    }

    #[test]
    fn idle_from_sessionrc_reads_the_3x_name() {
        assert_eq!(idle_from_sessionrc("DBRRG_IDLE_TIMEOUT=120").0, Some(120));
        assert_eq!(idle_from_sessionrc("TF_IDLE_TIMEOUT=abc").0, Some(0));
        assert_eq!(idle_from_sessionrc("TF_IDLE_TIMEOUT=\" +30 \"").0, Some(30));
        assert_eq!(
            idle_from_sessionrc("echo hi\n"),
            (None, "echo hi\n".to_string(), Vec::new())
        );
    }

    #[test]
    fn a_trailing_comment_ends_the_value() {
        let (n, rest, notes) = idle_from_sessionrc("TF_IDLE_TIMEOUT=300  # five minutes\necho hi\n");
        assert_eq!(n, Some(300));
        assert_eq!(rest, "echo hi\n");
        assert!(notes.is_empty(), "{notes:?}");
        let (k, _, _) = keyboard_from_env("XKB_DEFAULT_LAYOUT=de # x\n");
        assert_eq!(k.layout, "de");
        let (k, _, notes) = keyboard_from_env("XKB_DEFAULT_LAYOUT=\"de\"#x\n");
        assert_eq!(k.layout, "de");
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn a_trailing_command_leaves_the_line_alone() {
        let text = "TF_IDLE_TIMEOUT=300; echo hi\n";
        let (n, rest, notes) = idle_from_sessionrc(text);
        assert_eq!(n, None);
        assert_eq!(rest, text);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("TF_IDLE_TIMEOUT=300; echo hi"), "{notes:?}");
        let (n, _, _) = idle_from_sessionrc("TF_IDLE_TIMEOUT=60\nTF_IDLE_TIMEOUT=300 && x\n");
        assert_eq!(n, Some(60));
    }

    #[test]
    fn screen_file_values() {
        assert_eq!(screen_from_file("idle_timeout=600\n").value, Some(600));
        let c = screen_from_file("idle_timeout=soon\n");
        assert_eq!(c.value, None);
        assert_eq!(c.notes.len(), 1);
        assert!(screen_from_file("# nothing\n").notes.is_empty());
    }

    #[test]
    fn status_file_lines_and_refusal() {
        let c =
            status_from_file(b"# thinfloe status area, written by the menu\nclock 1x1\n-bogus 2x1\nbad line here\n");
        let lines = c.value.lines.unwrap();
        assert_eq!(lines[0], "clock 1x1");
        assert_eq!(lines[1], "-bogus 2x1");
        assert!(!c.notes.is_empty());
        let c = status_from_file(&[0xff, 0xfe]);
        assert_eq!(c.value.lines, None);
        assert_eq!(c.notes.len(), 1);
    }

    const EAP: &str = "network:\n  version: 2\n  wifis:\n    wlan0:\n      access-points:\n        \"corp\":\n          auth:\n            key-management: eap\n            method: peap\n            identity: me\n            password: hunter2\n";

    #[test]
    fn an_eap_wifi_file_is_kept() {
        let c = network_from_files(Some(EAP), None);
        assert_eq!(c.value.kept_wifi.as_deref(), Some(EAP));
        assert!(c.value.networks.is_empty());
        assert_eq!(c.notes.len(), 1, "{:?}", c.notes);
    }

    #[test]
    fn a_hand_wifi_file_the_menu_can_keep_converts() {
        let text = "network:\n  version: 2\n  renderer: networkd\n  wifis:\n    wlan0:\n      dhcp4: true\n      access-points:\n        \"home\":\n          password: \"secret123\"\n";
        let c = network_from_files(Some(text), None);
        assert_eq!(c.value.kept_wifi, None, "{:?}", c.notes);
        assert_eq!(
            c.value.networks,
            [NetCfg {
                ssid: "home".into(),
                hidden: false,
                password: Some("secret123".into())
            }]
        );
        assert_eq!(c.value.wifi_device.as_deref(), Some("wlan0"));
    }

    /// An old file for another netplan version or renderer, even one the
    /// 4.x menu wrote, is kept as written: converted, it would render as
    /// version 2 for networkd.
    #[test]
    fn a_file_of_another_version_or_renderer_is_kept() {
        let header = "# Written by the ThinFloe menu. Edits here are replaced.\n";
        let wifi = format!(
            "{header}network:\n  version: 2\n  renderer: NetworkManager\n  wifis:\n    wlan0:\n      dhcp4: true\n      access-points:\n        \"home\":\n          password: \"secret123\"\n"
        );
        let wired = format!(
            "{header}network:\n  version: 3\n  ethernets:\n    eth:\n      match:\n        name: \"en*\"\n      dhcp4: true\n"
        );
        let c = network_from_files(Some(&wifi), Some(&wired));
        assert_eq!(c.value.kept_wifi.as_deref(), Some(wifi.as_str()));
        assert!(c.value.networks.is_empty());
        assert_eq!(c.value.kept_wired.as_deref(), Some(wired.as_str()));
        assert_eq!(c.notes.len(), 2, "{:?}", c.notes);
    }

    #[test]
    fn the_template_counts_as_no_wifi() {
        let c = network_from_files(Some(include_str!("testdata/wifi-template.yaml")), None);
        assert_eq!(c.value, NetworkCfg::default());
        assert!(c.notes.is_empty(), "{:?}", c.notes);
        let c = network_from_files(None, None);
        assert_eq!(c.value, NetworkCfg::default());
    }

    #[test]
    fn a_hand_wired_file_converts_or_is_kept() {
        let fixed = "network:\n  version: 2\n  ethernets:\n    eth:\n      match:\n        name: \"en*\"\n      addresses: [10.0.0.5/24]\n";
        let c = network_from_files(None, Some(fixed));
        assert_eq!(
            c.value.wired().unwrap(),
            Wired::Static {
                address: "10.0.0.5".parse().unwrap(),
                prefix: 24,
                gateway: None,
                dns: vec![]
            },
            "{:?}",
            c.notes
        );
        let odd = "network:\n  version: 2\n  bridges:\n    br0: {}\n";
        let c = network_from_files(None, Some(odd));
        assert_eq!(c.value.kept_wired.as_deref(), Some(odd));
        assert_eq!(c.notes.len(), 1);
    }

    #[test]
    fn a_hand_kanshi_file_is_kept() {
        let text = "profile {\n output * enable\n}\n";
        let c = monitors_from_kanshi(text);
        assert_eq!(c.value.kept.as_deref(), Some(text));
        assert!(c.value.profiles.is_empty());
        assert!(c.notes[0].contains("kept as written; the menu shows it read-only"));
    }

    #[test]
    fn a_menu_kanshi_file_with_a_name_the_settings_cannot_hold_is_kept() {
        // A brace parses, but would not render again; a quote is no
        // parsable name at all. Either way the text stays as written.
        for name in ["a}b", "a\"b"] {
            let text = format!(
                "{}\n\nprofile {{\n\toutput {name} enable mode 1920x1080 position 0,0 transform normal scale 1\n}}\n",
                crate::migrate::LEGACY_MENU_HEADER
            );
            let c = monitors_from_kanshi(&text);
            assert_eq!(c.value.kept.as_deref(), Some(text.as_str()), "{name}");
            assert!(c.value.profiles.is_empty(), "{name}");
            assert_eq!(c.notes.len(), 1, "{name}");
        }
    }

    #[test]
    fn a_menu_kanshi_file_converts() {
        let p = vec![crate::kanshi::Profile {
            outputs: vec![crate::kanshi::OutputSet {
                criteria: "eDP-1".into(),
                enabled: true,
                mode: crate::heads::Mode {
                    w: 1920,
                    h: 1080,
                    refresh_mhz: 60000,
                    preferred: false,
                },
                rotation: crate::heads::Rotation::Normal,
                place: crate::kanshi::Place::Origin,
            }],
            menu_output: None,
        }];
        let text = format!(
            "{}\n\n{}",
            crate::migrate::LEGACY_MENU_HEADER,
            crate::kanshi::render(&p)
        );
        let c = monitors_from_kanshi(&text);
        assert_eq!(c.value, MonitorsCfg::from_profiles(&p));
        assert!(c.notes.is_empty());
    }

    fn inp() -> Inputs {
        Inputs {
            default_wired: include_str!("../../../overlay/etc/netplan/ethernet.yaml").into(),
            netboot: false,
            registry: None,
        }
    }

    #[test]
    fn from_generated_is_none_for_an_empty_dir() {
        let d = TestDir::new("convert", "empty");
        assert_eq!(from_generated(&d).unwrap(), None);
        assert_eq!(from_generated(&d.join("absent")).unwrap(), None);
    }

    fn sample() -> ThinfloeConfig {
        let mut c = ThinfloeConfig {
            keyboard: kb("ch", "de_nodeadkeys", false, Compose::Menu),
            ..ThinfloeConfig::default()
        };
        c.screen.idle_timeout = 900;
        let p = vec![crate::kanshi::Profile {
            outputs: vec![crate::kanshi::OutputSet {
                criteria: "eDP-1".into(),
                enabled: true,
                mode: crate::heads::Mode {
                    w: 1920,
                    h: 1080,
                    refresh_mhz: 60000,
                    preferred: false,
                },
                rotation: crate::heads::Rotation::Normal,
                place: crate::kanshi::Place::Origin,
            }],
            menu_output: None,
        }];
        c.monitors = MonitorsCfg::from_profiles(&p);
        c.network.wifi_device = Some("wlan0".into());
        c.network.networks = vec![
            NetCfg {
                ssid: "home".into(),
                hidden: false,
                password: Some("secret-pw".into()),
            },
            NetCfg {
                ssid: "cafe".into(),
                hidden: true,
                password: None,
            },
        ];
        c.network.wifi_dns = vec!["1.1.1.1".into()];
        c.network.set_wired(&Wired::Static {
            address: "10.0.0.5".parse().unwrap(),
            prefix: 24,
            gateway: Some("10.0.0.1".parse().unwrap()),
            dns: vec!["9.9.9.9".parse().unwrap()],
        });
        c
    }

    #[test]
    fn from_generated_reads_back_what_render_wrote() {
        let d = TestDir::new("convert", "roundtrip");
        let cfg = sample();
        for name in ["environment", "screen", "kanshi", "wifi.yaml", "ethernet.yaml"] {
            let body = generated::render(&cfg, name, &inp()).unwrap();
            generated::write_if_changed(&d, name, body.as_deref()).unwrap();
        }
        assert_eq!(from_generated(&d).unwrap(), Some(cfg));
    }

    #[test]
    fn from_generated_keeps_kept_text() {
        let d = TestDir::new("convert", "kept");
        let mut cfg = ThinfloeConfig::default();
        cfg.network.kept_wifi = Some(EAP.into());
        cfg.monitors.kept = Some("profile {\n output * enable\n}\n".into());
        for name in ["wifi.yaml", "kanshi"] {
            let body = generated::render(&cfg, name, &inp()).unwrap();
            generated::write_if_changed(&d, name, body.as_deref()).unwrap();
        }
        let back = from_generated(&d).unwrap().unwrap();
        assert_eq!(back.network.kept_wifi.as_deref(), Some(EAP));
        assert_eq!(back.monitors.kept, cfg.monitors.kept);
    }
}
