//! `thinfloe-menu --migrate-home`: moves a 4.x home into
//! `~/.config/thinfloe/` once. Old files are read and converted into
//! `config.toml`, the generated files and the copied files are written,
//! then the old files move into `~/cfg-attic/`. A failure before the move
//! leaves every old file where it was; no old file is ever deleted.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::atomic;
use crate::config::{ConfigDir, MonitorsCfg, ThinfloeConfig};
use crate::convert;
use crate::generated::{Inputs, Table};
use crate::netfiles::{self, FileState};
use crate::store::{self, OpenMode};

/// The first line of the files the 4.x menu wrote in the home.
pub const LEGACY_MENU_HEADER: &str = "# Written by the ThinFloe menu. Edits here are replaced.";

/// Every old path, relative to the home.
pub const OLD_FILES: &[&str] = &[
    ENVIRONMENT,
    SESSIONRC,
    ".thinfloe-home-version",
    SCREEN,
    ".config/thinfloe/screen.by-hand",
    STATUS,
    KANSHI,
    KANSHI_PREV,
    ".config/kanshi/config.by-hand",
    KANSHI_MARKER,
    WIFI,
    "wifi.yaml.by-hand",
    WIRED,
    "ethernet.yaml.by-hand",
    ".thinfloe-password",
    ".thinfloe-ssh-host-keys",
    "wg0.conf",
    ".save-home-exclude",
];

const ENVIRONMENT: &str = ".thinfloe-environment";
const SESSIONRC: &str = ".thinfloe-sessionrc";
const SCREEN: &str = ".config/thinfloe/screen";
const STATUS: &str = ".config/thinfloe/status";
const KANSHI: &str = ".config/kanshi/config";
const KANSHI_PREV: &str = ".config/kanshi/config.prev";
const KANSHI_MARKER: &str = ".config/kanshi/thinfloe-layout-unconfirmed";
const WIFI: &str = "wifi.yaml";
const WIRED: &str = "ethernet.yaml";

/// The old files copied unchanged, and their names in `~/.config/thinfloe/`.
const COPIED: [(&str, &str); 4] = [
    (".thinfloe-password", "password"),
    (".thinfloe-ssh-host-keys", "ssh-host-keys"),
    ("wg0.conf", "wg0.conf"),
    (".save-home-exclude", "save-home-exclude"),
];

/// Today's cap for the menu's files in the home (`owned`).
const MAX_BYTES: u64 = 64 * 1024;
const LAYOUT_FILE: &str = "home-layout";
const ATTIC: &str = "cfg-attic";
const HOME_VERSION: &[u8] = b"2\n";
const ENV_TEMPLATE: &str = include_str!("../../../overlay/home/tfuser/.config/thinfloe/environment");
const SESSIONRC_TEMPLATE: &str = include_str!("../../../overlay/home/tfuser/.config/thinfloe/sessionrc");

/// Which layout this boot's home has, as the initramfs recorded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeLayout {
    Legacy,
    New,
}

/// Only the exact word `legacy` (one newline allowed) is the old layout;
/// a missing, empty or torn file is the new one.
pub fn home_layout(state_dir: &Path) -> HomeLayout {
    match atomic::read(&state_dir.join(LAYOUT_FILE), 64) {
        Ok(Some(b)) if b == b"legacy" || b == b"legacy\n" => HomeLayout::Legacy,
        _ => HomeLayout::New,
    }
}

/// Rewrite `home-layout` to `new`. In place: the state directory is
/// root's, so no file can be made beside it for a rename.
pub fn set_new(state_dir: &Path) -> io::Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(state_dir.join(LAYOUT_FILE))?;
    f.write_all(b"new\n")
}

/// What a migration did, for `~/cfg-attic/README` and the Settings notice.
/// Each entry is one English sentence naming the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub converted: Vec<String>,
    pub copied: Vec<String>,
    pub dropped: Vec<String>,
    pub not_moved: Vec<String>,
}

impl Report {
    pub fn readme(&self) -> String {
        let mut t = String::from(
            "ThinFloe moved the settings of this home to ~/.config/thinfloe/.\n\
             The menu now keeps them in ~/.config/thinfloe/config.toml and sets them\n\
             in Settings. This directory holds the old files under their old paths;\n\
             nothing reads them any more. Delete it once you no longer need them.\n",
        );
        for (title, lines) in [
            ("Converted", &self.converted),
            ("Copied", &self.copied),
            ("Dropped or only partly converted", &self.dropped),
            ("Not moved (left in place, no longer read)", &self.not_moved),
        ] {
            if lines.is_empty() {
                continue;
            }
            t.push_str(&format!("\n{title}:\n"));
            for l in lines {
                t.push_str(&format!("- {l}\n"));
            }
        }
        t
    }

    pub fn summary(&self) -> String {
        let mut s =
            String::from("The settings of this home moved to ~/.config/thinfloe and the old files to ~/cfg-attic.");
        match self.dropped.len() {
            0 => s.push_str(" ~/cfg-attic/README lists what was converted."),
            n => s.push_str(&format!(
                " {n} setting{} could not be carried over in full; ~/cfg-attic/README lists {}.",
                if n == 1 { "" } else { "s" },
                if n == 1 { "it" } else { "them" }
            )),
        }
        match self.not_moved.len() {
            0 => {}
            n => s.push_str(&format!(
                " {n} old file{} could not be moved and stay{} in place, no longer read.",
                if n == 1 { "" } else { "s" },
                if n == 1 { "s" } else { "" }
            )),
        }
        s
    }
}

/// The old files step 1 reads, as found.
struct OldFiles {
    environment: Option<String>,
    sessionrc: Option<String>,
    screen: Option<String>,
    status: Option<Vec<u8>>,
    kanshi: Option<String>,
    kanshi_prev: Option<String>,
    kanshi_marker: Option<String>,
    wifi: Option<String>,
    wired: Option<String>,
    /// The new `environment` and `sessionrc` as they are now.
    new_environment: Option<String>,
    new_sessionrc: Option<String>,
}

fn read_bytes(path: &Path, name: &str) -> Result<Option<Vec<u8>>, String> {
    atomic::read(path, MAX_BYTES).map_err(|e| format!("{name} could not be read: {e}"))
}

fn read_text(path: &Path, name: &str) -> Result<Option<String>, String> {
    read_bytes(path, name).map(|b| b.map(|b| String::from_utf8_lossy(&b).into_owned()))
}

impl OldFiles {
    fn read(home: &Path, dir: &ConfigDir) -> Result<OldFiles, String> {
        let old = |rel: &str| read_text(&home.join(rel), &format!("~/{rel}"));
        Ok(OldFiles {
            environment: old(ENVIRONMENT)?,
            sessionrc: old(SESSIONRC)?,
            screen: old(SCREEN)?,
            status: read_bytes(&home.join(STATUS), &format!("~/{STATUS}"))?,
            kanshi: old(KANSHI)?,
            kanshi_prev: old(KANSHI_PREV)?,
            kanshi_marker: old(KANSHI_MARKER)?,
            wifi: old(WIFI)?,
            wired: old(WIRED)?,
            new_environment: read_text(&dir.environment(), "~/.config/thinfloe/environment")?,
            new_sessionrc: read_text(&dir.sessionrc(), "~/.config/thinfloe/sessionrc")?,
        })
    }
}

/// `note` about the old file `rel`, its own one-word label (`screen: `)
/// replaced by the path.
fn tag(rel: &str, note: &str) -> String {
    let n = note
        .split_once(": ")
        .filter(|(label, _)| !label.contains(' '))
        .map_or(note, |(_, rest)| rest);
    format!("~/{rel}: {n}")
}

/// A line that is neither blank nor a comment.
fn is_active(line: &str) -> bool {
    let l = line.trim();
    !l.is_empty() && !l.starts_with('#')
}

/// The file assigns an `XKB_DEFAULT_*` key on an active line.
fn sets_keyboard(text: &str) -> bool {
    text.lines().filter(|l| is_active(l)).any(|l| {
        let l = l.trim_start();
        l.strip_prefix("export ")
            .map_or(l, str::trim_start)
            .starts_with("XKB_DEFAULT_")
    })
}

/// What step 2 writes besides the store.
struct Converted {
    cfg: ThinfloeConfig,
    environment: Option<String>,
    sessionrc: Option<String>,
}

/// Step 1: `start` with every table an old file is left for replaced by
/// that file's conversion. A table without an old file keeps what
/// `start` holds, so a second run loses nothing the first converted.
fn convert_old(old: &OldFiles, start: &ThinfloeConfig, r: &mut Report) -> Converted {
    let mut cfg = start.clone();

    let mut env_lines = Vec::new();
    if let Some(t) = &old.environment {
        let (kb, other, notes) = convert::keyboard_from_env(t);
        if sets_keyboard(t) {
            r.converted.push(format!(
                "~/{ENVIRONMENT}: the keyboard (layout {}{}) is now set in Settings, Keyboard.",
                kb.layout,
                if kb.variant.is_empty() {
                    String::new()
                } else {
                    format!(", variant {}", kb.variant)
                }
            ));
            cfg.keyboard = kb;
        }
        if !other.is_empty() {
            r.converted.push(format!(
                "~/{ENVIRONMENT}: its other lines are now in ~/.config/thinfloe/environment."
            ));
        }
        r.dropped.extend(notes.iter().map(|n| tag(ENVIRONMENT, n)));
        env_lines = other;
    }

    let mut rc_idle = None;
    let mut rc_rest = None;
    if let Some(t) = &old.sessionrc {
        let (idle, rest, notes) = convert::idle_from_sessionrc(t);
        r.dropped.extend(notes.iter().map(|n| tag(SESSIONRC, n)));
        if let Some(n) = idle {
            r.converted.push(format!(
                "~/{SESSIONRC}: the screen timeout ({n} s) is now set in Settings, Screen."
            ));
        }
        if rest.lines().any(is_active) {
            r.converted.push(format!(
                "~/{SESSIONRC}: its commands are now in ~/.config/thinfloe/sessionrc."
            ));
            rc_rest = Some(rest);
        }
        rc_idle = idle;
    }
    if let Some(t) = &old.screen {
        let c = convert::screen_from_file(t);
        r.dropped.extend(c.notes.iter().map(|n| tag(SCREEN, n)));
        match (c.value, rc_idle) {
            (Some(n), None) => {
                cfg.screen.idle_timeout = n;
                r.converted.push(format!(
                    "~/{SCREEN}: the screen timeout ({n} s) is now set in Settings, Screen."
                ));
            }
            (Some(n), Some(_)) => r.dropped.push(format!(
                "~/{SCREEN}: idle_timeout={n} is dropped; TF_IDLE_TIMEOUT in ~/{SESSIONRC} set the timeout."
            )),
            (None, _) => {}
        }
    }
    if let Some(n) = rc_idle {
        cfg.screen.idle_timeout = n;
    }

    if let Some(b) = &old.status {
        let c = convert::status_from_file(b);
        r.dropped.extend(c.notes.iter().map(|n| tag(STATUS, n)));
        if c.value.lines.is_some() {
            cfg.status = c.value;
            r.converted
                .push(format!("~/{STATUS}: the status area is now set in Settings, Status."));
        }
    }

    // As thinfloe-session undoes an unconfirmed layout: `prev` puts
    // config.prev back (no config without it), `none` leaves no config.
    let marker = old
        .kanshi_marker
        .as_deref()
        .map(|t| t.lines().next().unwrap_or("").trim().to_string());
    let kanshi = match marker.as_deref() {
        Some("prev") => Some((KANSHI_PREV, old.kanshi_prev.as_deref())),
        Some("none") => Some((KANSHI_MARKER, None)),
        _ => old.kanshi.as_deref().map(|t| (KANSHI, Some(t))),
    };
    match kanshi {
        Some((rel, Some(t))) => {
            let c = convert::monitors_from_kanshi(t);
            r.dropped.extend(c.notes.iter().map(|n| tag(rel, n)));
            cfg.monitors = c.value;
            r.converted.push(format!(
                "~/{rel}: the monitor arrangement is now set in Settings, Screen."
            ));
        }
        Some((rel, None)) => {
            cfg.monitors = MonitorsCfg::default();
            r.converted.push(format!(
                "~/{rel}: an unconfirmed monitor arrangement was undone; no arrangement is set."
            ));
        }
        None => {}
    }

    // The shipped template says nothing: it leaves the networks alone.
    let wifi = old.wifi.as_deref().filter(|t| {
        !matches!(
            netfiles::parse_wifi(Some(t)).state,
            FileState::Missing | FileState::Template
        )
    });
    if wifi.is_some() || old.wired.is_some() {
        let c = convert::network_from_files(wifi, old.wired.as_deref());
        r.dropped.extend(c.notes.iter().map(|n| format!("~/{n}")));
        let n = c.value;
        if wifi.is_some() {
            cfg.network.wifi_device = n.wifi_device;
            cfg.network.networks = n.networks;
            cfg.network.wifi_dns = n.wifi_dns;
            cfg.network.kept_wifi = n.kept_wifi;
            r.converted
                .push(format!("~/{WIFI}: the WiFi networks are now set in Settings, Network."));
        }
        if old.wired.is_some() {
            cfg.network.wired = n.wired;
            cfg.network.kept_wired = n.kept_wired;
            r.converted
                .push(format!("~/{WIRED}: the wired network is now set in Settings, Network."));
        }
    }

    Converted {
        cfg,
        environment: new_environment(old.new_environment.as_deref(), &env_lines),
        sessionrc: new_sessionrc(old.new_sessionrc.as_deref(), rc_rest.as_deref()),
    }
}

fn with_newline(mut s: String) -> String {
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// The new `environment`: the file as it is (the template when there is
/// none) plus the old file's lines it does not hold yet. `None`: leave it.
fn new_environment(now: Option<&str>, lines: &[String]) -> Option<String> {
    let base = now.unwrap_or(ENV_TEMPLATE);
    let missing: Vec<&String> = lines
        .iter()
        .filter(|l| !base.lines().any(|b| b == l.as_str()))
        .collect();
    if missing.is_empty() && now.is_some() {
        return None;
    }
    let mut t = with_newline(base.to_string());
    if !missing.is_empty() {
        t.push_str(&format!("\n# From ~/{ENVIRONMENT}:\n"));
        for l in missing {
            t.push_str(l);
            t.push('\n');
        }
    }
    Some(t)
}

/// The new `sessionrc`: the file as it is (the template when there is
/// none) plus the old file's commands, unless it holds them already.
fn new_sessionrc(now: Option<&str>, rest: Option<&str>) -> Option<String> {
    let base = now.unwrap_or(SESSIONRC_TEMPLATE);
    let block = rest
        .map(|r| format!("\n# From ~/{SESSIONRC}:\n{}", with_newline(r.to_string())))
        .filter(|b| !base.contains(b.as_str()));
    if block.is_none() && now.is_some() {
        return None;
    }
    Some(with_newline(base.to_string()) + block.as_deref().unwrap_or(""))
}

/// Copy `src` to `dst`, modes kept; a directory with its contents.
/// Anything but regular files and directories is refused, and so is a
/// link (at `src` or in the way at `dst`). False when `src` is missing.
fn copy_old(src: &Path, dst: &Path) -> io::Result<bool> {
    let meta = match fs::symlink_metadata(src) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        r => r?,
    };
    let mode = meta.permissions().mode() & 0o7777;
    if meta.is_file() {
        let bytes = atomic::read(src, MAX_BYTES)?.ok_or_else(|| io::Error::other("vanished"))?;
        atomic::write(dst, &bytes, mode)?;
    } else if meta.is_dir() {
        match fs::symlink_metadata(dst) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(io::Error::other(format!("{} is in the way", dst.display()))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir_all(dst)?,
            Err(e) => return Err(e),
        }
        // Writable while it is filled; its own mode last.
        fs::set_permissions(dst, fs::Permissions::from_mode(0o700))?;
        for entry in fs::read_dir(src)? {
            let name = entry?.file_name();
            copy_old(&src.join(&name), &dst.join(&name))?;
        }
        fs::set_permissions(dst, fs::Permissions::from_mode(mode))?;
    } else {
        return Err(io::Error::other(format!(
            "{} is not a regular file or a directory",
            src.display()
        )));
    }
    Ok(true)
}

fn taken(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// `attic/rel`, or `rel.<epoch>` (then `rel.<epoch>.<n>`) when that is
/// taken: an attic entry is never replaced.
fn attic_slot(attic: &Path, rel: &str, now_epoch: i64) -> PathBuf {
    let first = attic.join(rel);
    if !taken(&first) {
        return first;
    }
    let stamped = attic.join(format!("{rel}.{now_epoch}"));
    if !taken(&stamped) {
        return stamped;
    }
    (1u32..)
        .map(|n| attic.join(format!("{rel}.{now_epoch}.{n}")))
        .find(|p| !taken(p))
        .unwrap_or(stamped)
}

fn move_to_attic(home: &Path, attic: &Path, rel: &str, now_epoch: i64) -> io::Result<()> {
    let dst = attic_slot(attic, rel, now_epoch);
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(home.join(rel), dst)
}

/// Run the migration when `home-layout` says `legacy`; `Ok(None)` when it
/// does not. An `Err` before the old files move leaves all of them in
/// place; the next start tries again.
pub fn migrate_home(home: &Path, state_dir: &Path, inputs: Inputs, now_epoch: i64) -> Result<Option<Report>, String> {
    if home_layout(state_dir) == HomeLayout::New {
        return Ok(None);
    }
    let dir = ConfigDir::from_home(home);
    let mut report = Report::default();

    // Step 1: read everything before anything is written.
    let old = OldFiles::read(home, &dir)?;

    // Step 2. The store starts from config.toml (or rebuilds it from
    // generated/), so a second run keeps what the first converted.
    let opened = store::open(dir.clone(), inputs, OpenMode::Session, now_epoch);
    let mut st = opened.store;
    report
        .dropped
        .extend(opened.notes.iter().map(|n| format!("~/.config/thinfloe/{n}")));
    let conv = convert_old(&old, &st.cfg, &mut report);
    let refused = |e: String| {
        let mut why = e;
        for n in &opened.notes {
            why.push(' ');
            why.push_str(n);
        }
        why
    };
    for table in [
        Table::Keyboard,
        Table::Screen,
        Table::Status,
        Table::Monitors,
        Table::Network,
    ] {
        st.update(table, |c| *c = conv.cfg.clone()).map_err(refused)?;
    }
    for (path, text) in [
        (dir.environment(), &conv.environment),
        (dir.sessionrc(), &conv.sessionrc),
    ] {
        if let Some(t) = text {
            atomic::write(&path, t.as_bytes(), 0o644)
                .map_err(|e| format!("{} could not be written: {e}", path.display()))?;
        }
    }
    for (old_rel, new_name) in COPIED {
        let dst = dir.root.join(new_name);
        if copy_old(&home.join(old_rel), &dst)
            .map_err(|e| format!("~/{old_rel} could not be copied to {}: {e}", dst.display()))?
        {
            report
                .copied
                .push(format!("~/{old_rel} is now ~/.config/thinfloe/{new_name}."));
        }
    }

    // Step 3: a file that does not move stays where it was.
    let attic = home.join(ATTIC);
    for rel in OLD_FILES {
        if !taken(&home.join(rel)) {
            continue;
        }
        if let Err(e) = move_to_attic(home, &attic, rel, now_epoch) {
            report.not_moved.push(format!("~/{rel}: {e}"));
        }
    }

    // Step 4.
    atomic::write(&dir.home_version(), HOME_VERSION, 0o644)
        .map_err(|e| format!("{} could not be written: {e}", dir.home_version().display()))?;
    let readme = attic_slot(&attic, "README", now_epoch);
    atomic::write(&readme, report.readme().as_bytes(), 0o644)
        .map_err(|e| format!("{} could not be written: {e}", readme.display()))?;
    let summary = report.summary();
    st.update(Table::Notice, |c| c.notice.migration = Some(summary))?;
    set_new(state_dir).map_err(|e| format!("{} could not be written: {e}", state_dir.join(LAYOUT_FILE).display()))?;
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, Compose, ConfigDir, Keyboard, NetCfg, ThinfloeConfig};
    use crate::heads::{Mode, Rotation};
    use crate::kanshi::{self, OutputSet, Place, Profile};
    use crate::netfiles::{self, Net, Wired};
    use crate::testdir::TestDir;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    const NOW: i64 = 1_700_000_000;
    const ENV_TEMPLATE: &str = include_str!("../../../overlay/home/tfuser/.config/thinfloe/environment");
    const RC_TEMPLATE: &str = include_str!("../../../overlay/home/tfuser/.config/thinfloe/sessionrc");
    const HAND_ENV: &str = "XKB_DEFAULT_LAYOUT=ch\nXKB_DEFAULT_VARIANT=de_nodeadkeys\nXKB_DEFAULT_OPTIONS=grp:alt_shift_toggle,compose:menu,ctrl:nocaps\nXCURSOR_SIZE=32\n";
    const HAND_WIFI: &str = "network:\n  version: 2\n  renderer: networkd\n  wifis:\n    wlan0:\n      dhcp4: true\n      access-points:\n        \"home\":\n          password: \"secret123\"\n";
    const EAP: &str = "network:\n  version: 2\n  wifis:\n    wlan0:\n      access-points:\n        \"corp\":\n          auth:\n            key-management: eap\n            method: peap\n            identity: me\n            password: hunter2\n";
    const DEFAULT_WIRED: &str = include_str!("../../../overlay/etc/netplan/ethernet.yaml");

    fn inputs() -> Inputs {
        Inputs {
            default_wired: DEFAULT_WIRED.into(),
            netboot: false,
        }
    }

    /// A home and a state dir saying `legacy`.
    struct Home {
        _d: TestDir,
        home: PathBuf,
        state: PathBuf,
    }

    impl Home {
        fn new(tag: &str) -> Home {
            let d = TestDir::new("migrate", tag);
            let home = d.join("home");
            let state = d.join("state");
            fs::create_dir_all(&home).unwrap();
            fs::create_dir_all(&state).unwrap();
            fs::write(state.join("home-layout"), "legacy\n").unwrap();
            Home { _d: d, home, state }
        }

        /// As the shipped home has them.
        fn with_templates(tag: &str) -> Home {
            let h = Home::new(tag);
            h.put(".config/thinfloe/environment", ENV_TEMPLATE, 0o644);
            h.put(".config/thinfloe/sessionrc", RC_TEMPLATE, 0o644);
            h
        }

        fn put(&self, rel: &str, text: &str, mode: u32) {
            let p = self.home.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, text).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
        }

        fn run(&self) -> Result<Option<Report>, String> {
            migrate_home(&self.home, &self.state, inputs(), NOW)
        }

        fn dir(&self) -> ConfigDir {
            ConfigDir::from_home(&self.home)
        }

        fn cfg(&self) -> ThinfloeConfig {
            config::parse(&fs::read_to_string(self.dir().config_toml()).unwrap()).unwrap()
        }

        fn text(&self, rel: &str) -> String {
            fs::read_to_string(self.home.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
        }

        fn exists(&self, rel: &str) -> bool {
            fs::symlink_metadata(self.home.join(rel)).is_ok()
        }

        fn mode(&self, rel: &str) -> u32 {
            fs::metadata(self.home.join(rel)).unwrap().permissions().mode() & 0o777
        }

        fn layout_text(&self) -> String {
            fs::read_to_string(self.state.join("home-layout")).unwrap()
        }
    }

    fn profile(name: &str) -> Profile {
        Profile {
            outputs: vec![OutputSet {
                criteria: name.into(),
                enabled: true,
                mode: Mode {
                    w: 1920,
                    h: 1080,
                    refresh_mhz: 60000,
                    preferred: false,
                },
                rotation: Rotation::Normal,
                place: Place::Origin,
            }],
            menu_output: None,
        }
    }

    fn menu_kanshi(p: &Profile) -> String {
        format!("{LEGACY_MENU_HEADER}\n\n{}", kanshi::render(std::slice::from_ref(p)))
    }

    fn home_net() -> Vec<NetCfg> {
        vec![NetCfg {
            ssid: "home".into(),
            hidden: false,
            password: Some("secret123".into()),
        }]
    }

    #[test]
    fn a_4x_home_with_hand_files_migrates() {
        let h = Home::with_templates("hand");
        h.put(".thinfloe-environment", HAND_ENV, 0o644);
        h.put(
            ".thinfloe-sessionrc",
            "TF_IDLE_TIMEOUT=900\nwlr-randr --output DP-1 --transform 90\n",
            0o644,
        );
        h.put("wifi.yaml", HAND_WIFI, 0o600);
        h.put("wg0.conf", "[Interface]\nPrivateKey = x\n", 0o600);
        h.put(".thinfloe-password", "$6$hash\n", 0o600);
        h.put(".thinfloe-ssh-host-keys/ssh_host_ed25519_key", "KEY\n", 0o600);
        fs::set_permissions(
            h.home.join(".thinfloe-ssh-host-keys"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        h.put(".save-home-exclude", "Downloads\n", 0o644);

        let r = h.run().unwrap().expect("a report");

        let c = h.cfg();
        assert_eq!(
            c.keyboard,
            Keyboard {
                layout: "ch".into(),
                variant: "de_nodeadkeys".into(),
                caps_as_ctrl: true,
                compose: Compose::Menu,
            }
        );
        assert_eq!(c.screen.idle_timeout, 900);
        assert_eq!(c.network.networks, home_net());
        assert!(c.notice.migration.is_some());

        let env = h.text(".config/thinfloe/environment");
        assert!(env.starts_with(ENV_TEMPLATE), "{env}");
        assert!(env.contains("XCURSOR_SIZE=32"), "{env}");
        assert!(!env.lines().any(|l| l.starts_with("XKB_DEFAULT_")), "{env}");
        let rc = h.text(".config/thinfloe/sessionrc");
        assert!(rc.contains("wlr-randr --output DP-1 --transform 90"), "{rc}");
        assert!(rc.contains("# From ~/.thinfloe-sessionrc:"), "{rc}");
        assert!(!rc.contains("TF_IDLE_TIMEOUT="), "{rc}");

        assert!(h.text(".config/thinfloe/generated/wifi.yaml").contains("home"));
        assert_eq!(h.mode(".config/thinfloe/generated/wifi.yaml"), 0o600);
        assert_eq!(h.text(".config/thinfloe/wg0.conf"), "[Interface]\nPrivateKey = x\n");
        assert_eq!(h.mode(".config/thinfloe/wg0.conf"), 0o600);
        assert_eq!(h.text(".config/thinfloe/password"), "$6$hash\n");
        assert_eq!(h.mode(".config/thinfloe/password"), 0o600);
        assert_eq!(h.text(".config/thinfloe/ssh-host-keys/ssh_host_ed25519_key"), "KEY\n");
        assert_eq!(h.mode(".config/thinfloe/ssh-host-keys/ssh_host_ed25519_key"), 0o600);
        assert_eq!(h.mode(".config/thinfloe/ssh-host-keys"), 0o700);
        assert_eq!(h.text(".config/thinfloe/save-home-exclude"), "Downloads\n");

        assert_eq!(h.text("cfg-attic/.thinfloe-environment"), HAND_ENV);
        assert!(h.exists("cfg-attic/wifi.yaml"));
        assert!(h.exists("cfg-attic/.thinfloe-ssh-host-keys/ssh_host_ed25519_key"));
        for rel in OLD_FILES {
            assert!(!h.exists(rel), "{rel} is still in place");
        }

        assert_eq!(h.text(".config/thinfloe/home-version"), "2\n");
        assert_eq!(h.layout_text(), "new\n");
        let readme = h.text("cfg-attic/README");
        assert!(readme.contains("grp:alt_shift_toggle"), "{readme}");
        assert_eq!(readme, r.readme());
        assert!(r.not_moved.is_empty(), "{r:?}");
        assert_eq!(r.copied.len(), 4, "{r:?}");
        assert!(!r.summary().is_empty());
        assert_eq!(c.notice.migration.as_deref(), Some(r.summary().as_str()));
    }

    #[test]
    fn a_home_with_menu_files_migrates() {
        let h = Home::with_templates("menu");
        h.put(
            ".config/thinfloe/screen",
            &format!("{LEGACY_MENU_HEADER}\nidle_timeout=600\n"),
            0o644,
        );
        h.put(".config/thinfloe/status", "clock 1x1\n-battery 2x1\n", 0o644);
        h.put(".config/kanshi/config", &menu_kanshi(&profile("eDP-1")), 0o644);
        let wifi = netfiles::render_wifi(
            Some("wlan0"),
            &[Net {
                ssid: "home".into(),
                hidden: false,
                password: Some("secret123".into()),
            }],
            &[],
        );
        assert!(wifi.starts_with(LEGACY_MENU_HEADER));
        h.put("wifi.yaml", &wifi, 0o600);
        let fixed = Wired::Static {
            address: "10.0.0.5".parse().unwrap(),
            prefix: 24,
            gateway: Some("10.0.0.1".parse().unwrap()),
            dns: vec![],
        };
        let wired = netfiles::render_wired(DEFAULT_WIRED, &fixed).unwrap().unwrap();
        h.put("ethernet.yaml", &wired, 0o600);

        h.run().unwrap().expect("a report");

        let c = h.cfg();
        assert_eq!(c.screen.idle_timeout, 600);
        assert_eq!(
            c.status.lines,
            Some(vec!["clock 1x1".to_string(), "-battery 2x1".to_string()])
        );
        assert_eq!(c.monitors.to_profiles().unwrap(), [profile("eDP-1")]);
        assert_eq!(c.network.networks, home_net());
        assert_eq!(c.network.wired().unwrap(), fixed);
        assert_eq!(c.monitors.kept, None);
        assert_eq!(c.network.kept_wifi, None);
        assert_eq!(c.network.kept_wired, None);
        assert!(h.text(".config/thinfloe/generated/kanshi").contains("eDP-1"));
        assert!(h.text(".config/thinfloe/generated/ethernet.yaml").contains("10.0.0.5"));
        // Nothing to add: the templates stay as shipped.
        assert_eq!(h.text(".config/thinfloe/environment"), ENV_TEMPLATE);
        assert_eq!(h.text(".config/thinfloe/sessionrc"), RC_TEMPLATE);
    }

    #[test]
    fn broken_files_are_converted_as_far_as_possible() {
        let h = Home::with_templates("broken");
        h.put("wifi.yaml", EAP, 0o600);
        let hand_kanshi = "profile {\n output * enable\n}\n";
        h.put(".config/kanshi/config", hand_kanshi, 0o644);
        h.put(".config/thinfloe/status", &"clock 1x1\n".repeat(500), 0o644);

        let r = h.run().unwrap().expect("a report");

        let c = h.cfg();
        assert_eq!(c.network.kept_wifi.as_deref(), Some(EAP));
        assert_eq!(c.monitors.kept.as_deref(), Some(hand_kanshi));
        assert_eq!(c.status.lines, None);
        assert!(h.text(".config/thinfloe/generated/wifi.yaml").contains("hunter2"));
        let readme = h.text("cfg-attic/README");
        assert!(readme.contains("4 KiB"), "{readme}");
        assert!(readme.contains("~/wifi.yaml"), "{readme}");
        assert!(readme.contains("~/.config/kanshi/config"), "{readme}");
        assert_eq!(r.dropped.len(), 3, "{r:?}");
        assert_eq!(h.layout_text(), "new\n");
    }

    #[test]
    fn an_unreadable_old_file_stops_the_migration() {
        let h = Home::with_templates("fifo");
        let fifo = h.home.join(".thinfloe-environment");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        h.put("wifi.yaml", HAND_WIFI, 0o600);
        h.put(".thinfloe-password", "$6$hash\n", 0o600);

        let e = h.run().unwrap_err();
        assert!(e.contains(".thinfloe-environment"), "{e}");

        assert!(h.exists(".thinfloe-environment"));
        assert_eq!(h.text("wifi.yaml"), HAND_WIFI);
        assert_eq!(h.text(".thinfloe-password"), "$6$hash\n");
        assert!(!h.exists(".config/thinfloe/home-version"));
        assert!(!h.exists(".config/thinfloe/config.toml"));
        assert!(!h.exists("cfg-attic"));
        assert_eq!(h.layout_text(), "legacy\n");
    }

    #[test]
    fn a_failed_write_keeps_every_old_file() {
        // SAFETY: no arguments, no side effects.
        if unsafe { libc::geteuid() } == 0 {
            return; // root reads a mode 000 directory
        }
        let h = Home::with_templates("readonly-generated");
        h.put("wifi.yaml", HAND_WIFI, 0o600);
        // generated/ cannot be read: the store opens read-only.
        fs::create_dir_all(h.dir().generated()).unwrap();
        fs::write(h.dir().generated_file("wifi.yaml"), "x").unwrap();
        fs::set_permissions(h.dir().generated(), fs::Permissions::from_mode(0o000)).unwrap();
        let r = h.run();
        fs::set_permissions(h.dir().generated(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.as_ref().is_err_and(|e| e.contains("read-only")), "{r:?}");
        assert_eq!(h.text("wifi.yaml"), HAND_WIFI);
        assert!(!h.exists("cfg-attic"));
        assert!(!h.exists(".config/thinfloe/home-version"));
        assert_eq!(h.layout_text(), "legacy\n");
    }

    #[test]
    fn an_unconfirmed_layout_converts_the_previous_one() {
        let h = Home::with_templates("prev");
        h.put(".config/kanshi/config.prev", &menu_kanshi(&profile("A-1")), 0o644);
        h.put(".config/kanshi/config", &menu_kanshi(&profile("B-1")), 0o644);
        h.put(".config/kanshi/thinfloe-layout-unconfirmed", "prev\n", 0o644);
        h.run().unwrap().expect("a report");
        assert_eq!(h.cfg().monitors.to_profiles().unwrap(), [profile("A-1")]);
        assert!(h.exists("cfg-attic/.config/kanshi/config.prev"));
        assert!(h.exists("cfg-attic/.config/kanshi/thinfloe-layout-unconfirmed"));

        let h = Home::with_templates("none");
        h.put(".config/kanshi/config", &menu_kanshi(&profile("B-1")), 0o644);
        h.put(".config/kanshi/thinfloe-layout-unconfirmed", "none\n", 0o644);
        h.run().unwrap().expect("a report");
        let c = h.cfg();
        assert!(c.monitors.profiles.is_empty(), "{:?}", c.monitors);
        assert_eq!(c.monitors.kept, None);
        assert!(!h.exists(".config/thinfloe/generated/kanshi"));
    }

    #[test]
    fn a_second_run_keeps_what_the_first_converted() {
        let h = Home::with_templates("again");
        h.put(".thinfloe-environment", HAND_ENV, 0o644);
        h.put("wifi.yaml", HAND_WIFI, 0o600);
        fs::create_dir_all(h.dir().home_version()).unwrap();

        let first = h.run();
        assert!(first.is_err(), "{first:?}");
        assert_eq!(h.layout_text(), "legacy\n");
        assert_eq!(h.text("cfg-attic/.thinfloe-environment"), HAND_ENV);
        assert_eq!(h.text("cfg-attic/wifi.yaml"), HAND_WIFI);

        fs::remove_dir(h.dir().home_version()).unwrap();
        // The shipped old templates come back with the next restore.
        h.put(".thinfloe-environment", "# template\n", 0o644);
        h.put("wifi.yaml", include_str!("testdata/wifi-template.yaml"), 0o644);

        h.run().unwrap().expect("a report");
        let c = h.cfg();
        assert_eq!(c.network.networks, home_net());
        assert_eq!(c.keyboard.layout, "ch");
        assert_eq!(h.text("cfg-attic/.thinfloe-environment"), HAND_ENV);
        assert_eq!(h.text("cfg-attic/wifi.yaml"), HAND_WIFI);
        assert_eq!(h.text("cfg-attic/.thinfloe-environment.1700000000"), "# template\n");
        assert!(h.exists("cfg-attic/wifi.yaml.1700000000"));
        let env = h.text(".config/thinfloe/environment");
        assert_eq!(env.matches("XCURSOR_SIZE=32").count(), 1, "{env}");
        assert_eq!(h.text(".config/thinfloe/home-version"), "2\n");
        assert_eq!(h.layout_text(), "new\n");
    }

    #[test]
    fn an_already_migrated_home_is_left_alone() {
        let h = Home::new("done");
        fs::write(h.state.join("home-layout"), "new\n").unwrap();
        h.put(".thinfloe-environment", HAND_ENV, 0o644);
        h.put("wifi.yaml", HAND_WIFI, 0o600);
        assert_eq!(h.run(), Ok(None));
        let mut names: Vec<_> = fs::read_dir(&h.home).unwrap().map(|e| e.unwrap().file_name()).collect();
        names.sort();
        assert_eq!(names, [".thinfloe-environment", "wifi.yaml"]);
        assert_eq!(h.text(".thinfloe-environment"), HAND_ENV);
        assert_eq!(h.layout_text(), "new\n");
    }

    #[test]
    fn home_layout_reads_only_legacy_as_legacy() {
        let d = TestDir::new("migrate", "layout");
        let f = d.join("home-layout");
        let at = |text: Option<&str>| {
            let _ = fs::remove_file(&f);
            if let Some(t) = text {
                fs::write(&f, t).unwrap();
            }
            home_layout(&d)
        };
        assert_eq!(at(Some("legacy\n")), HomeLayout::Legacy);
        assert_eq!(at(Some("legacy")), HomeLayout::Legacy);
        assert_eq!(at(Some("")), HomeLayout::New);
        assert_eq!(at(None), HomeLayout::New);
        assert_eq!(at(Some("new\n")), HomeLayout::New);
        assert_eq!(at(Some("garbage")), HomeLayout::New);
        assert_eq!(at(Some("legacy\n\n")), HomeLayout::New);
        assert_eq!(at(Some(" legacy")), HomeLayout::New);
    }

    #[test]
    fn set_new_rewrites_the_file_in_place() {
        use std::os::unix::fs::MetadataExt;
        let d = TestDir::new("migrate", "set-new");
        let f = d.join("home-layout");
        fs::write(&f, "legacy\n").unwrap();
        let ino = fs::metadata(&f).unwrap().ino();
        set_new(&d).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "new\n");
        assert_eq!(fs::metadata(&f).unwrap().ino(), ino);
        assert_eq!(home_layout(&d), HomeLayout::New);
    }

    #[test]
    fn a_3x_renamed_home_without_templates_migrates() {
        let h = Home::new("3x");
        h.put(
            ".thinfloe-environment",
            "XKB_DEFAULT_LAYOUT=de\nXCURSOR_THEME=redglass\n",
            0o644,
        );
        h.put(".thinfloe-sessionrc", "DBRRG_IDLE_TIMEOUT=120\nkanshi &\n", 0o644);
        h.put(".thinfloe-home-version", "1\n", 0o644);

        h.run().unwrap().expect("a report");

        let c = h.cfg();
        assert_eq!(c.keyboard.layout, "de");
        assert_eq!(c.screen.idle_timeout, 120);
        let env = h.text(".config/thinfloe/environment");
        assert!(env.starts_with(ENV_TEMPLATE), "{env}");
        assert!(env.contains("XCURSOR_THEME=redglass"), "{env}");
        let rc = h.text(".config/thinfloe/sessionrc");
        assert!(rc.starts_with(RC_TEMPLATE), "{rc}");
        assert!(rc.contains("kanshi &"), "{rc}");
        assert!(h.exists("cfg-attic/.thinfloe-home-version"));
    }

    #[test]
    fn a_comment_only_sessionrc_is_not_copied() {
        let h = Home::with_templates("rc-comments");
        h.put(
            ".thinfloe-sessionrc",
            "# a long template\n#TF_IDLE_TIMEOUT=600\n\n",
            0o644,
        );
        h.run().unwrap().expect("a report");
        assert_eq!(h.text(".config/thinfloe/sessionrc"), RC_TEMPLATE);
        assert!(h.exists("cfg-attic/.thinfloe-sessionrc"));
    }

    #[test]
    fn the_summary_names_the_attic() {
        let r = Report {
            converted: vec!["a".into()],
            copied: vec![],
            dropped: vec!["b".into()],
            not_moved: vec!["c".into()],
        };
        let s = r.summary();
        assert!(s.contains("~/cfg-attic"), "{s}");
        assert!(s.matches(". ").count() <= 2, "{s}");
        let readme = r.readme();
        for line in ["a", "b", "c"] {
            assert!(readme.lines().any(|l| l == format!("- {line}")), "{readme}");
        }
    }
}
