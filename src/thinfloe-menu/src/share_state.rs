//! The FloeDrop band of the Network tab: the `[share]` setting as the band
//! shows it, and the state file `$XDG_RUNTIME_DIR/thinfloe-share.state`
//! the daemon `thinfloe-share` writes. The menu parses that file on its
//! own, as it does the Deskflow one: its first line is `starting`,
//! `ready<TAB>iface<TAB>mode<TAB>channel<TAB>flags<TAB>station<TAB>driver` or
//! `error<TAB>text` (station and driver: the Wi-Fi card under the AWDL link
//! `iface`; an older daemon leaves them out); readers ignore unknown trailing fields. Of the
//! lines after it, the band shows the transfers to this machine,
//! `sender<TAB>folder<TAB>receiving<TAB>since<TAB>files<TAB>bytes<TAB>first`; the outbox's
//! lines (one per phone) are not shown. Saved at once, like the Time band; drawing is in
//! network_ui.rs.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::config::ShareCfg;
use crate::deskflow_sup::Daemon;
use crate::settings::Tone;
use crate::sharecfg;

/// The program, unless `TF_SHARE` names another.
pub const PROGRAM: &str = "/usr/bin/thinfloe-share";
/// The source the daemon's lines carry in the menu's log.
pub const LOG_SOURCE: &str = "share";
/// Where received files go and files to send are put.
pub const FOLDER: &str = "/media/tfuser/FloeDrop";
/// The switch's text.
pub const SWITCH_TEXT: &str = "FloeDrop: receive and send files with iPhones nearby (AirDrop)";
/// The name field's label.
pub const NAME_LABEL: &str = "Name shown on phones";
/// A head line and a few phones fit many times over.
const MAX_BYTES: u64 = 64 * 1024;
/// File times come from a coarse clock: a state file this much older than
/// the daemon's start still counts as written by it.
const MTIME_SLACK: Duration = Duration::from_secs(1);

/// The menu's pid for the daemon, so a daemon whose menu died before it set
/// PR_SET_PDEATHSIG exits at once; `thinfloe-share` reads the same name.
pub const PARENT_ENV: &str = "TF_SHARE_PARENT";

/// `thinfloe-share` with no arguments. It starts with SIGHUP blocked and
/// unblocks it once its handler is set. A reload reaches the daemon only:
/// its radio helper runs in the same group, under sudo, which relays
/// signals to it.
pub const SHARE: Daemon = Daemon {
    source: LOG_SOURCE,
    label: "FloeDrop",
    prefixes: &["thinfloe-share: "],
    parent_env: Some(PARENT_ENV),
    block_hup: true,
    reload_group: false,
    error_head,
};

fn error_head(text: &str) -> Option<String> {
    match parse(text) {
        Head::Error(why) => Some(why),
        _ => None,
    }
}

/// The first line of the state file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Starting,
    Ready {
        iface: String,
        mode: String,
        channel: u32,
        flags: Vec<String>,
        /// The Wi-Fi card under the link and its driver; empty when unknown.
        station: String,
        driver: String,
    },
    Error(String),
}

/// The head of the state file's text; a line that does not fit is
/// `Starting`, as the daemon has not said more yet.
pub fn parse(text: &str) -> Head {
    let Some(line) = text.lines().next() else {
        return Head::Starting;
    };
    let f: Vec<&str> = line.split('\t').collect();
    match f[0] {
        "ready" if f.len() >= 5 => match f[3].parse() {
            Ok(channel) => Head::Ready {
                iface: f[1].to_string(),
                mode: f[2].to_string(),
                channel,
                flags: f[4].split(',').filter(|x| !x.is_empty()).map(String::from).collect(),
                station: f.get(5).copied().unwrap_or("").to_string(),
                driver: f.get(6).copied().unwrap_or("").to_string(),
            },
            Err(_) => Head::Starting,
        },
        "error" => Head::Error(f.get(1).copied().unwrap_or("").to_string()),
        _ => Head::Starting,
    }
}

/// A transfer to this machine under way, from a `receiving` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receiving {
    pub sender: String,
    pub files: u32,
    /// The name of the first file.
    pub first: String,
}

/// The `receiving` lines of the state file's text (after the head, at least seven fields).
pub fn parse_receiving(text: &str) -> Vec<Receiving> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 7 || f[2] != "receiving" {
                return None;
            }
            Some(Receiving {
                sender: f[0].to_string(),
                files: f[4].parse().ok()?,
                first: f[6].to_string(),
            })
        })
        .collect()
}

/// The band's lines for the transfers under way.
pub fn receiving_lines(r: &[Receiving]) -> Vec<(String, Tone)> {
    r.iter()
        .map(|r| {
            let text = match r.files {
                // Only folders: nothing to count.
                0 => format!("Receiving from {}", r.sender),
                1 if !r.first.is_empty() => format!("Receiving {} from {}", r.first, r.sender),
                n => format!("Receiving {n} files from {}", r.sender),
            };
            (text, Tone::Plain)
        })
        .collect()
}

/// `$XDG_RUNTIME_DIR/thinfloe-share.state`.
pub fn path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty())?;
    Some(PathBuf::from(dir).join("thinfloe-share.state"))
}

/// The head of the file at `path`, its transfers under way and when it was written; `None`
/// when it cannot be read.
pub fn read(path: &Path) -> Option<(Head, Vec<Receiving>, SystemTime)> {
    let written = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let text = crate::bounded::read_text(path, MAX_BYTES)?;
    Some((parse(&text), parse_receiving(&text), written))
}

/// The band's warning when the radio helper found no regulatory country (flag `no-country`).
pub const NO_COUNTRY: &str = "No country set: set the time zone in the Time band below for faster FloeDrop.";

/// The band's status lines for `head`, each with its colour.
pub fn status_lines(head: &Head) -> Vec<(String, Tone)> {
    match head {
        Head::Starting => vec![("Starting…".to_string(), Tone::Plain)],
        Head::Ready {
            iface,
            mode,
            channel,
            flags,
            station,
            driver,
        } => {
            let card = if station.is_empty() { iface } else { station };
            let mut how = if driver.is_empty() {
                mode.clone()
            } else {
                format!("{driver}, {mode}")
            };
            // 0: the radio helper could not read the channel.
            if *channel != 0 {
                how.push_str(&format!(", channel {channel}"));
            }
            let mut v = vec![(
                format!("Ready on {card} ({how}). Phones must be set to AirDrop 'Everyone for 10 Minutes'."),
                Tone::Plain,
            )];
            if flags.iter().any(|f| f == "no-ble") {
                v.push((
                    "Sending needs Bluetooth; this machine has none.".to_string(),
                    Tone::Plain,
                ));
            }
            if flags.iter().any(|f| f == "no-country") {
                v.push((NO_COUNTRY.to_string(), Tone::Warn));
            }
            v
        }
        Head::Error(why) => vec![(why.clone(), Tone::Warn)],
    }
}

/// What the band reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareEvent {
    /// The switch.
    Enable(bool),
    /// The name field as typed; `commit`: it lost focus or took Enter.
    NameText { text: String, commit: bool },
    /// Escape in the name field: the saved name is shown again.
    NameCancel,
}

#[derive(Debug, Default)]
pub struct ShareBand {
    /// The setting on screen: what `config.toml` holds, or the one being
    /// saved.
    cfg: ShareCfg,
    name_text: String,
    /// The host name; the field's placeholder is the name phones get
    /// from it for an empty field.
    host: String,
    error: Option<String>,
    /// The state file as last read: head, transfers under way, when it was written.
    file: Option<(Head, Vec<Receiving>, SystemTime)>,
    /// When the running daemon started; `None` while none runs.
    running_since: Option<SystemTime>,
    /// Why the daemon ended the last time it failed.
    failure: Option<String>,
}

impl ShareBand {
    /// The setting as `config.toml` holds it.
    pub fn set_cfg(&mut self, cfg: ShareCfg) {
        self.name_text = cfg.name.clone();
        self.cfg = cfg;
    }

    pub fn cfg(&self) -> &ShareCfg {
        &self.cfg
    }

    pub fn set_host(&mut self, host: String) {
        self.host = host;
    }

    /// The name field's placeholder: what phones show for an empty one.
    pub fn placeholder(&self) -> String {
        sharecfg::default_name(&self.host)
    }

    pub fn name_text(&self) -> &str {
        &self.name_text
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The state file as read now.
    pub fn set_file(&mut self, file: Option<(Head, Vec<Receiving>, SystemTime)>) {
        self.file = file;
    }

    /// What the supervisor says: when the running daemon started, and why
    /// the last one failed.
    pub fn set_daemon(&mut self, running_since: Option<SystemTime>, failure: Option<String>) {
        self.running_since = running_since;
        self.failure = failure;
    }

    /// The head the band shows; `None` while the switch is off, so the
    /// error the daemon writes as it stops is never shown. A file older
    /// than the running daemon is from an earlier run.
    pub fn head(&self) -> Option<Head> {
        if !self.cfg.enabled {
            return None;
        }
        Some(match (self.running_since, self.this_run()) {
            (Some(_), Some((head, _, _))) => head.clone(),
            (Some(_), None) => Head::Starting,
            (None, _) => self.failure.clone().map_or(Head::Starting, Head::Error),
        })
    }

    /// The state file when the running daemon wrote it.
    fn this_run(&self) -> Option<&(Head, Vec<Receiving>, SystemTime)> {
        let start = self.running_since?;
        self.file
            .as_ref()
            .filter(|(_, _, written)| *written + MTIME_SLACK >= start)
    }

    /// The transfers under way; empty unless the band shows this run's ready head.
    pub fn receiving(&self) -> &[Receiving] {
        match (self.head(), self.this_run()) {
            (Some(Head::Ready { .. }), Some((_, r, _))) => r,
            _ => &[],
        }
    }

    /// One change; `Some` is the setting to save now.
    pub fn event(&mut self, e: ShareEvent) -> Option<ShareCfg> {
        match e {
            ShareEvent::Enable(on) => {
                if on == self.cfg.enabled {
                    return None;
                }
                self.cfg.enabled = on;
                Some(self.cfg.clone())
            }
            ShareEvent::NameText { text, commit } => {
                self.name_text = text;
                if !commit {
                    return None;
                }
                let name = sharecfg::clean_name(&self.name_text);
                self.name_text = name.clone();
                if name == self.cfg.name {
                    return None;
                }
                self.cfg.name = name;
                Some(self.cfg.clone())
            }
            ShareEvent::NameCancel => {
                self.name_text = self.cfg.name.clone();
                None
            }
        }
    }

    /// The save ended: `stored` is the table as the store holds it now,
    /// `error` why the write failed; the band shows the stored setting.
    pub fn written(&mut self, stored: ShareCfg, error: Option<String>) {
        if error.is_some() {
            self.name_text = stored.name.clone();
        }
        self.cfg = stored;
        self.error = error;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(flags: &[&str]) -> Head {
        Head::Ready {
            iface: "wlx1".into(),
            mode: "exclusive".into(),
            channel: 6,
            flags: flags.iter().map(|f| f.to_string()).collect(),
            station: String::new(),
            driver: String::new(),
        }
    }

    fn ready_on(station: &str, driver: &str) -> Head {
        Head::Ready {
            iface: "awdl0".into(),
            mode: "exclusive".into(),
            channel: 6,
            flags: vec![],
            station: station.into(),
            driver: driver.into(),
        }
    }

    #[test]
    fn share_state_parse_names_the_card() {
        assert_eq!(
            parse("ready\tawdl0\texclusive\t6\t\twlxc03a55f0d060\trtw88_8822bu\tfuture\n"),
            ready_on("wlxc03a55f0d060", "rtw88_8822bu")
        );
        assert_eq!(parse("ready\tawdl0\texclusive\t6\t\twlx1\n"), ready_on("wlx1", ""));
    }

    #[test]
    fn the_band_names_the_card_and_its_driver() {
        let phones = "Phones must be set to AirDrop 'Everyone for 10 Minutes'.";
        assert_eq!(
            status_lines(&ready_on("wlxc03a55f0d060", "rtw88_8822bu")),
            [(
                format!("Ready on wlxc03a55f0d060 (rtw88_8822bu, exclusive, channel 6). {phones}"),
                Tone::Plain
            )]
        );
        assert_eq!(
            status_lines(&ready_on("wlx1", "")),
            [(format!("Ready on wlx1 (exclusive, channel 6). {phones}"), Tone::Plain)]
        );
    }

    #[test]
    fn no_country_warns_in_the_band() {
        let phones = "Phones must be set to AirDrop 'Everyone for 10 Minutes'.";
        let ready_line = (format!("Ready on wlx1 (exclusive, channel 6). {phones}"), Tone::Plain);
        assert_eq!(
            status_lines(&ready(&["no-country"])),
            [ready_line.clone(), (NO_COUNTRY.to_string(), Tone::Warn)]
        );
        assert_eq!(
            status_lines(&ready(&["no-ble", "no-country"])),
            [
                ready_line,
                (
                    "Sending needs Bluetooth; this machine has none.".to_string(),
                    Tone::Plain
                ),
                (NO_COUNTRY.to_string(), Tone::Warn),
            ]
        );
    }

    fn jane_receiving(files: u32) -> Receiving {
        Receiving {
            sender: "Jane's iPhone".into(),
            files,
            first: "IMG_0042.DNG".into(),
        }
    }

    #[test]
    fn receiving_lines_parse_and_read() {
        let text = "ready\tawdl0\texclusive\t44\t\twlx1\trtw88_8822bu\nJane's iPhone\tJane's iPhone\tidle\t1\nJane's iPhone\tJane's iPhone\treceiving\t2\t1\t75000000\tIMG_0042.DNG\textra\nX\tX\treceiving\t3\n";
        assert_eq!(parse_receiving(text), [jane_receiving(1)]);
        let d = crate::testdir::TestDir::new("share-state", "receiving");
        let p = d.join("thinfloe-share.state");
        std::fs::write(&p, text).unwrap();
        assert_eq!(read(&p).unwrap().1, [jane_receiving(1)]);
    }

    #[test]
    fn the_band_says_what_is_arriving() {
        assert_eq!(
            receiving_lines(&[jane_receiving(1)]),
            [("Receiving IMG_0042.DNG from Jane's iPhone".to_string(), Tone::Plain)]
        );
        assert_eq!(
            receiving_lines(&[jane_receiving(3)]),
            [("Receiving 3 files from Jane's iPhone".to_string(), Tone::Plain)]
        );
        assert_eq!(
            receiving_lines(&[jane_receiving(0)]),
            [("Receiving from Jane's iPhone".to_string(), Tone::Plain)]
        );
        assert_eq!(receiving_lines(&[]), []);
    }

    #[test]
    fn a_transfer_shows_only_while_ready() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mut b = ShareBand::default();
        b.set_cfg(on(""));
        b.set_daemon(Some(t0), None);
        b.set_file(Some((Head::Error("x".into()), vec![jane_receiving(1)], t0)));
        assert!(b.receiving().is_empty());
        b.set_file(Some((ready(&[]), vec![jane_receiving(1)], t0)));
        assert_eq!(b.receiving(), [jane_receiving(1)]);
        // A file from the run before this one.
        b.set_daemon(Some(t0 + Duration::from_secs(10)), None);
        assert!(b.receiving().is_empty());
        b.set_daemon(Some(t0), None);
        b.set_cfg(ShareCfg {
            enabled: false,
            name: String::new(),
        });
        assert!(b.receiving().is_empty());
    }

    #[test]
    fn an_unknown_channel_is_left_out() {
        let mut head = ready_on("wlx1", "rtw88_8822bu");
        if let Head::Ready { channel, .. } = &mut head {
            *channel = 0;
        }
        assert_eq!(
            status_lines(&head),
            [(
                "Ready on wlx1 (rtw88_8822bu, exclusive). Phones must be set to AirDrop 'Everyone for 10 Minutes'."
                    .to_string(),
                Tone::Plain
            )]
        );
    }

    #[test]
    fn share_state_parse_ready_no_ble() {
        let text =
            "ready\twlx1\texclusive\t6\tno-ble\t\t\tfuture\nJane's iPhone\tJane's iPhone\tsending\t1764000000\textra\n";
        assert_eq!(parse(text), ready(&["no-ble"]));
        assert_eq!(parse("ready\twlx1\texclusive\t6\t\n"), ready(&[]));
        assert_eq!(
            parse("ready\twlx1\texclusive\t6\tfuture,no-ble\n"),
            ready(&["future", "no-ble"])
        );
    }

    #[test]
    fn share_state_parse_error() {
        assert_eq!(
            parse("error\tno WiFi card can do AirDrop\textra\n"),
            Head::Error("no WiFi card can do AirDrop".into())
        );
        assert_eq!(parse("starting\n"), Head::Starting);
        // Half a line, a bad channel or nothing at all: not ready yet.
        assert_eq!(parse(""), Head::Starting);
        assert_eq!(parse("ready\twlx1\n"), Head::Starting);
        assert_eq!(parse("ready\twlx1\texclusive\tsix\t\n"), Head::Starting);
        assert_eq!(parse("error\n"), Head::Error(String::new()));
    }

    #[test]
    fn band_status_texts() {
        let phones = "Phones must be set to AirDrop 'Everyone for 10 Minutes'.";
        assert_eq!(
            status_lines(&ready(&[])),
            [(format!("Ready on wlx1 (exclusive, channel 6). {phones}"), Tone::Plain)]
        );
        assert_eq!(
            status_lines(&ready(&["no-ble"])),
            [
                (format!("Ready on wlx1 (exclusive, channel 6). {phones}"), Tone::Plain),
                (
                    "Sending needs Bluetooth; this machine has none.".to_string(),
                    Tone::Plain
                ),
            ]
        );
        assert_eq!(
            status_lines(&Head::Error("no WiFi card can do AirDrop".into())),
            [("no WiFi card can do AirDrop".to_string(), Tone::Warn)]
        );
        assert_eq!(status_lines(&Head::Starting), [("Starting…".to_string(), Tone::Plain)]);
    }

    fn on(name: &str) -> ShareCfg {
        ShareCfg {
            enabled: true,
            name: name.into(),
        }
    }

    #[test]
    fn the_switch_and_a_committed_name_save_at_once() {
        let mut b = ShareBand::default();
        assert_eq!(b.event(ShareEvent::Enable(false)), None);
        assert_eq!(b.event(ShareEvent::Enable(true)), Some(on("")));
        let typed = |text: &str, commit| ShareEvent::NameText {
            text: text.into(),
            commit,
        };
        assert_eq!(b.event(typed(" Front\tdesk ", false)), None);
        assert_eq!(b.name_text(), " Front\tdesk ");
        assert_eq!(b.event(typed(" Front\tdesk ", true)), Some(on("Frontdesk")));
        assert_eq!(b.name_text(), "Frontdesk");
        assert_eq!(b.event(typed("Frontdesk", true)), None, "no change, no save");
        b.event(typed("Other", false));
        assert_eq!(b.event(ShareEvent::NameCancel), None);
        assert_eq!(b.name_text(), "Frontdesk");
    }

    #[test]
    fn a_failed_save_shows_the_stored_setting_and_why() {
        let mut b = ShareBand::default();
        b.set_cfg(on("Desk"));
        let tried = b.event(ShareEvent::NameText {
            text: "New".into(),
            commit: true,
        });
        assert_eq!(tried, Some(on("New")));
        b.written(on("Desk"), Some("The settings are read-only.".into()));
        assert_eq!(b.cfg(), &on("Desk"));
        assert_eq!(b.name_text(), "Desk");
        assert_eq!(b.error(), Some("The settings are read-only."));
        b.written(on("Desk"), None);
        assert_eq!(b.error(), None);
    }

    #[test]
    fn the_band_shows_this_runs_head_and_nothing_while_off() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mut b = ShareBand::default();
        b.set_file(Some((Head::Error("FloeDrop is stopped.".into()), Vec::new(), t0)));
        assert_eq!(b.head(), None, "off: no status at all");
        b.set_cfg(on(""));
        assert_eq!(b.head(), Some(Head::Starting), "on, no daemon yet");
        // The file is from the run before this one.
        b.set_daemon(Some(t0 + Duration::from_secs(10)), None);
        assert_eq!(b.head(), Some(Head::Starting));
        b.set_file(Some((ready(&[]), Vec::new(), t0 + Duration::from_secs(11))));
        assert_eq!(b.head(), Some(ready(&[])));
        // It ended with an error and waits to start again.
        b.set_daemon(None, Some("no WiFi card can do AirDrop".into()));
        assert_eq!(b.head(), Some(Head::Error("no WiFi card can do AirDrop".into())));
    }

    #[test]
    fn read_takes_the_head_and_the_time() {
        let d = crate::testdir::TestDir::new("share-state", "read");
        let p = d.join("thinfloe-share.state");
        assert_eq!(read(&p), None);
        std::fs::write(&p, "ready\twlx1\texclusive\t6\tno-ble\n").unwrap();
        let (head, receiving, written) = read(&p).unwrap();
        assert_eq!(head, ready(&["no-ble"]));
        assert!(receiving.is_empty());
        assert_eq!(written, std::fs::metadata(&p).unwrap().modified().unwrap());
    }

    #[test]
    fn the_share_daemon_starts_with_sighup_blocked() {
        use crate::deskflow_sup::Supervisor;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Instant;
        let d = crate::testdir::TestDir::new("share-state", "hup");
        let out = d.join("out");
        let prog = d.join("daemon");
        std::fs::write(
            &prog,
            format!(
                "#!/bin/bash\ngrep SigBlk /proc/$$/status >{0}.tmp\nmv {0}.tmp {0}\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut s = Supervisor::new(prog).with_daemon(SHARE).with_state_file(None);
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !out.exists() {
            assert!(Instant::now() < deadline, "the stand-in did not run");
            std::thread::sleep(Duration::from_millis(5));
        }
        let blk = std::fs::read_to_string(&out).unwrap();
        let mask = u64::from_str_radix(blk.split_whitespace().nth(1).unwrap(), 16).unwrap();
        assert_ne!(mask & (1 << (libc::SIGHUP - 1)), 0, "{blk}");
        s.stop(Duration::from_secs(1));
    }

    #[test]
    fn the_daemon_is_told_the_menu_pid() {
        use crate::deskflow_sup::Supervisor;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Instant;
        let d = crate::testdir::TestDir::new("share-state", "parent");
        let out = d.join("out");
        let prog = d.join("daemon");
        std::fs::write(
            &prog,
            format!(
                "#!/bin/sh\necho \"$TF_SHARE_PARENT\" >{0}.tmp\nmv {0}.tmp {0}\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut s = Supervisor::new(prog).with_daemon(SHARE).with_state_file(None);
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !out.exists() {
            assert!(Instant::now() < deadline, "the stand-in did not run");
            std::thread::sleep(Duration::from_millis(5));
        }
        let text = std::fs::read_to_string(&out).unwrap();
        assert_eq!(text.trim(), std::process::id().to_string());
        s.stop(Duration::from_secs(1));
    }

    /// Whether a reload of a stand-in daemon reaches the daemon and a
    /// child of it in its group: (daemon, child).
    fn reload_reaches(daemon: Daemon, tag: &str) -> (bool, bool) {
        use crate::deskflow_sup::Supervisor;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Instant;
        let d = crate::testdir::TestDir::new("share-state", tag);
        let (parent, kid, ready) = (d.join("parent"), d.join("kid"), d.join("ready"));
        let prog = d.join("daemon");
        std::fs::write(
            &prog,
            format!(
                "#!/bin/sh\ntrap 'echo hup >>{p}' HUP\nsh -c 'trap \"echo hup >>{k}\" HUP; : >{r}; while :; do sleep 0.05; done' &\nwhile :; do sleep 0.05; done\n",
                p = parent.display(),
                k = kid.display(),
                r = ready.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut s = Supervisor::new(prog).with_daemon(daemon).with_state_file(None);
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "the stand-in did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        s.reload();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !parent.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        // Time for the child's trap too, had it got the signal.
        std::thread::sleep(Duration::from_millis(400));
        let got = (parent.exists(), kid.exists());
        s.stop(Duration::from_secs(1));
        got
    }

    #[test]
    fn a_floedrop_reload_reaches_the_daemon_and_not_its_helper() {
        // Unblocked, as the real daemon is once its handler is set.
        let share = Daemon {
            block_hup: false,
            ..SHARE
        };
        assert_eq!(reload_reaches(share, "reload-pid"), (true, false));
        // Control: sent to the group, the child gets it too.
        let group = Daemon {
            reload_group: true,
            ..share
        };
        assert_eq!(reload_reaches(group, "reload-group"), (true, true));
    }
}
