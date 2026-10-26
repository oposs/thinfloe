//! The Settings screen: one row per add-on tile, joined with the status
//! thinfloe-addon wrote. What a click starts is decided in menu.rs; this file
//! says what each row shows, and remembers across menu restarts which failed
//! add-ons were offered a retry in this boot.

use crate::addons::{self, AddonState, AddonStatus};
use crate::menu::Menu;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// What the person did on the Settings screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsEvent {
    /// The tick box of an add-on: on installs, off removes.
    Tick {
        id: String,
        on: bool,
    },
    InstallNow(String),
    UpdateNow(String),
    CheckNow,
    Back,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowButton {
    InstallNow,
    UpdateNow,
}

/// One add-on as its row shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The add-on's tile, for its name and icon.
    pub tile: usize,
    pub id: String,
    pub name: String,
    pub source: Option<String>,
    pub ticked: bool,
    pub state: String,
    pub button: Option<RowButton>,
    /// The source is neither HTTPS nor signed: nothing checks it by itself.
    pub manual: bool,
    /// An add-on job for this row runs; its tick box and button are off.
    pub busy: bool,
    /// An update waits for the next boot (state `pending`).
    pub update_ready: bool,
    /// thinfloe-addon refuses the tile's file name: tick box off, no button.
    pub unusable: bool,
}

pub const MANUAL_NOTE: &str = "updates only by hand";

/// Why is in the add-on log: thinfloe-addon names the file and the rule.
pub const UNUSABLE_TEXT: &str = "file name not usable";

fn with_reason(text: &str, message: Option<&str>) -> String {
    match message {
        Some(m) => format!("{text}: {m}"),
        None => text.to_string(),
    }
}

fn state_text(status: Option<&AddonStatus>) -> String {
    let Some(s) = status else {
        return "not installed".to_string();
    };
    let message = s.message.as_deref();
    match (s.state, s.installed.as_deref()) {
        (AddonState::Installed, Some(v)) => format!("{v} · up to date"),
        (AddonState::Pending, Some(v)) => match s.pending.as_deref() {
            Some(p) => format!("{v} installed · {p} ready for next boot"),
            None => format!("{v} installed · an update is ready for next boot"),
        },
        (AddonState::Failed, Some(v)) => format!("{v} installed · {}", with_reason("check failed", message)),
        (AddonState::InstallFailed, _) => format!("not installed · {}", with_reason("install failed", message)),
        (AddonState::Failed, None) => format!("not installed · {}", with_reason("failed", message)),
        _ => "not installed".to_string(),
    }
}

fn button(status: Option<&AddonStatus>) -> Option<RowButton> {
    match status {
        Some(s) if s.present() && s.state == AddonState::Pending => Some(RowButton::UpdateNow),
        // A manual-only source is never checked by itself, so no update is
        // ever found for it: Update now fetches and installs what is offered.
        Some(s) if s.present() && !s.auto => Some(RowButton::UpdateNow),
        Some(s) if s.present() => None,
        _ => Some(RowButton::InstallNow),
    }
}

/// The rows of the Settings screen, in tile order.
pub fn rows(menu: &Menu) -> Vec<Row> {
    menu.tiles
        .iter()
        .enumerate()
        .filter(|(_, t)| t.addon)
        .map(|(i, t)| {
            let id = crate::menu::addon_id(t).to_string();
            if !menu.addon_usable(i) {
                return Row {
                    tile: i,
                    name: t.name.clone(),
                    source: t.source.clone(),
                    ticked: false,
                    state: UNUSABLE_TEXT.to_string(),
                    button: None,
                    manual: false,
                    busy: false,
                    update_ready: false,
                    unusable: true,
                    id,
                };
            }
            let status = menu.addons().iter().find(|a| a.id == id);
            Row {
                tile: i,
                name: t.name.clone(),
                source: t.source.clone(),
                ticked: status.is_some_and(|s| s.ticked),
                state: state_text(status),
                button: button(status),
                manual: status.is_some_and(|s| !s.auto),
                busy: menu.addon_busy(&id),
                update_ready: status.is_some_and(|s| s.state == AddonState::Pending),
                unusable: false,
                id,
            }
        })
        .collect()
}

/// The add-ons offered a retry in this boot. The file lives in /tmp, which
/// is in RAM: it survives the menu and the session restarting after a
/// logout, and is gone at the next boot. XDG_RUNTIME_DIR is removed when
/// the session ends, so a logout would ask again.
#[derive(Debug, Default)]
pub struct RetryMarker {
    path: Option<PathBuf>,
    seen: Vec<String>,
}

/// The marker file is read up to this size.
const MARKER_BYTES: u64 = 4096;

impl RetryMarker {
    /// Kept in memory only.
    pub fn memory() -> RetryMarker {
        RetryMarker::default()
    }

    /// The ids recorded in `path`. A missing or unreadable file is empty.
    pub fn load(path: &Path) -> RetryMarker {
        let seen = crate::bounded::read_bounded(path, MARKER_BYTES, false)
            .map(|b| {
                String::from_utf8_lossy(&b)
                    .lines()
                    .filter(|l| addons::valid_id(l))
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        RetryMarker {
            path: Some(path.to_path_buf()),
            seen,
        }
    }

    pub fn seen(&self, id: &str) -> bool {
        self.seen.iter().any(|s| s == id)
    }

    /// Remember `id`. A file that cannot be written costs at most one more
    /// question after a menu restart.
    pub fn record(&mut self, id: &str) {
        if self.seen(id) {
            return;
        }
        self.seen.push(id.to_string());
        let Some(path) = &self.path else { return };
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .and_then(|mut f| writeln!(f, "{id}"));
        if let Err(e) = written {
            eprintln!("thinfloe-menu: cannot record the retry in {}: {e}", path.display());
        }
    }
}

/// Where the retry marker lives: `TF_RETRY_MARKER`, else
/// `/tmp/thinfloe-addon-retry.<uid>`.
pub fn retry_marker_path() -> PathBuf {
    if let Some(p) = std::env::var_os("TF_RETRY_MARKER") {
        return PathBuf::from(p);
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/thinfloe-addon-retry.{uid}"))
}

/// "last checked today 06:12", or with the date when it was another day.
pub fn last_checked(epoch: Option<u64>, now: i64) -> String {
    let Some(epoch) = epoch else {
        return "not checked yet".to_string();
    };
    let (date, time) = local_date_time(epoch as i64);
    if date == local_date_time(now).0 {
        format!("last checked today {time}")
    } else {
        format!("last checked {date} {time}")
    }
}

/// The local date (YYYY-MM-DD) and time of day (HH:MM) at `epoch`.
fn local_date_time(epoch: i64) -> (String, String) {
    let t = epoch as libc::time_t;
    // SAFETY: localtime_r writes only into the struct passed to it, and a
    // failure leaves it zeroed.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    };
    (
        format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday),
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    fn status(state: AddonState, installed: Option<&str>, auto: bool) -> AddonStatus {
        AddonStatus {
            id: "a".into(),
            ticked: true,
            state,
            installed: installed.map(String::from),
            pending: Some("2".into()),
            auto,
            message: Some("HTTP 404".into()),
        }
    }

    #[test]
    fn state_texts_and_buttons() {
        let cases = [
            (None, "not installed", Some(RowButton::InstallNow)),
            (
                Some(status(AddonState::Absent, None, true)),
                "not installed",
                Some(RowButton::InstallNow),
            ),
            (
                Some(status(AddonState::Installed, Some("1"), true)),
                "1 · up to date",
                None,
            ),
            (
                Some(status(AddonState::Pending, Some("1"), true)),
                "1 installed · 2 ready for next boot",
                Some(RowButton::UpdateNow),
            ),
            (
                Some(status(AddonState::Failed, Some("1"), true)),
                "1 installed · check failed: HTTP 404",
                None,
            ),
            (
                Some(status(AddonState::Failed, None, true)),
                "not installed · failed: HTTP 404",
                Some(RowButton::InstallNow),
            ),
            (
                Some(status(AddonState::InstallFailed, None, true)),
                "not installed · install failed: HTTP 404",
                Some(RowButton::InstallNow),
            ),
            (
                Some(status(AddonState::Installed, Some("1"), false)),
                "1 · up to date",
                Some(RowButton::UpdateNow),
            ),
        ];
        for (s, text, b) in cases {
            assert_eq!(state_text(s.as_ref()), text);
            assert_eq!(button(s.as_ref()), b, "{text}");
        }
    }

    #[test]
    fn retry_marker_survives_a_reload_and_ignores_junk() {
        let d = TestDir::new("settings", "marker");
        let p = d.join("m");
        let mut m = RetryMarker::load(&p);
        assert!(!m.seen("10-a"));
        m.record("10-a");
        m.record("10-a");
        assert!(m.seen("10-a"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "10-a\n");
        std::fs::write(&p, "10-a\nbad id!\n\x1b[31m\n20-b\n").unwrap();
        let m = RetryMarker::load(&p);
        assert!(m.seen("10-a") && m.seen("20-b") && !m.seen("bad id!"));
        // A symlink in its place is not followed.
        let link = d.join("link");
        std::os::unix::fs::symlink(d.join("target"), &link).unwrap();
        let mut m = RetryMarker::load(&link);
        m.record("10-a");
        assert!(!d.join("target").exists());
        assert!(m.seen("10-a"), "still remembered in memory");
    }

    #[test]
    fn last_checked_names_today_or_the_date() {
        assert_eq!(last_checked(None, 0), "not checked yet");
        let now = 1_800_000_000;
        assert!(last_checked(Some(now as u64), now).starts_with("last checked today "));
        let old = last_checked(Some(86_400 * 3), now);
        assert!(old.starts_with("last checked 1970-01-0"), "{old}");
    }
}
