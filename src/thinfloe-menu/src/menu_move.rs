//! Moving the menu to the monitor chosen for it. labwc places a window only
//! when it maps, so the menu writes the connector name to its output file,
//! `labwc-scale` turns it into a `MoveToOutput` window rule, reconfigures
//! labwc and confirms the name in `<file>.applied`; the menu then replaces
//! its window and labwc maps the new one on that output. A name labwc-scale
//! refuses or cannot apply is never confirmed, so after `CONFIRM_WAIT` the
//! window is replaced anyway (it lands where labwc puts it) and the menu
//! says so.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long the menu waits for labwc-scale to confirm a change.
pub const CONFIRM_WAIT: Duration = Duration::from_secs(3);
/// How often `.applied` is read while the menu waits.
pub const CONFIRM_POLL: Duration = Duration::from_millis(200);

/// Where the wanted output goes: `TF_MENU_OUTPUT_FILE`, else
/// `$XDG_RUNTIME_DIR/thinfloe-menu-output`. An empty variable counts as
/// unset, as in labwc-scale.
pub fn output_file_path() -> Option<PathBuf> {
    output_file_from(
        std::env::var_os("TF_MENU_OUTPUT_FILE"),
        std::env::var_os("XDG_RUNTIME_DIR"),
    )
}

fn output_file_from(file: Option<std::ffi::OsString>, runtime_dir: Option<std::ffi::OsString>) -> Option<PathBuf> {
    if let Some(p) = file.filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let dir = runtime_dir.filter(|d| !d.is_empty())?;
    Some(PathBuf::from(dir).join("thinfloe-menu-output"))
}

/// `<file>.applied`, where labwc-scale confirms.
pub fn applied_path(file: &Path) -> PathBuf {
    let mut p = file.as_os_str().to_owned();
    p.push(".applied");
    PathBuf::from(p)
}

/// Write the wanted output (one line; empty for none) through a temp file
/// in the same directory and a rename: labwc-scale reads the file each poll
/// and must never see half of it.
pub fn write_output_file(path: &Path, output: Option<&str>) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let text = output.map(|o| format!("{o}\n")).unwrap_or_default();
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The output labwc-scale has in effect: the first line of `.applied`
/// (`Some("")` for none); `None` while the file is missing or unreadable.
pub fn read_applied(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(applied_path(path)).ok()?;
    Some(text.lines().next().unwrap_or("").to_string())
}

/// What `MenuMove::poll` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replace {
    /// labwc-scale confirmed the change: replace the window.
    Confirmed,
    /// No confirmation came in time: replace the window anyway. Holds the
    /// output that was asked for (`None`: back to labwc's placement).
    Unconfirmed(Option<String>),
}

/// The output the menu asked for and whether it waits for labwc-scale.
#[derive(Debug, Clone, Default)]
pub struct MenuMove {
    /// The output last asked for; `None`: labwc places the menu, which is
    /// also how a session starts.
    asked: Option<String>,
    /// Since when the menu waits for the confirmation of `asked`.
    since: Option<Instant>,
}

impl MenuMove {
    /// The menu should be on `output` now. Returns true when that is a
    /// change, which the caller then writes to the output file. The same
    /// answer again changes nothing, so monitors that report again cause
    /// no second window.
    pub fn want(&mut self, output: Option<&str>, now: Instant) -> bool {
        if self.asked.as_deref() == output {
            return false;
        }
        self.asked = output.map(str::to_string);
        self.since = Some(now);
        true
    }

    /// While a change waits: replace the window once `applied` (what
    /// `.applied` holds, `None` when it is missing) names the asked output,
    /// or once `CONFIRM_WAIT` has passed. Returns `Some` once per change.
    pub fn poll(&mut self, applied: Option<&str>, now: Instant) -> Option<Replace> {
        let since = self.since?;
        let asked = self.asked.as_deref().unwrap_or("");
        if applied == Some(asked) {
            self.since = None;
            return Some(Replace::Confirmed);
        }
        if now.saturating_duration_since(since) >= CONFIRM_WAIT {
            self.since = None;
            return Some(Replace::Unconfirmed(self.asked.clone()));
        }
        None
    }

    /// Whether a change waits for its confirmation.
    pub fn waiting(&self) -> bool {
        self.since.is_some()
    }

    /// How long the loop may sleep before `poll` must run again.
    pub fn next_wake(&self, now: Instant) -> Option<Duration> {
        let since = self.since?;
        let left = (since + CONFIRM_WAIT).saturating_duration_since(now);
        Some(CONFIRM_POLL.min(left))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    #[test]
    fn the_path_follows_the_variables() {
        let o = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(output_file_from(o("/a/f"), o("/run")), Some(PathBuf::from("/a/f")));
        assert_eq!(
            output_file_from(o(""), o("/run")),
            Some(PathBuf::from("/run/thinfloe-menu-output"))
        );
        assert_eq!(output_file_from(None, o("")), None);
        assert_eq!(applied_path(Path::new("/run/x")), PathBuf::from("/run/x.applied"));
    }

    #[test]
    fn the_output_file_is_one_line_and_leaves_no_temp_file() {
        let d = TestDir::new("menu_move", "write");
        let f = d.join("thinfloe-menu-output");
        write_output_file(&f, Some("HDMI-A-1")).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "HDMI-A-1\n");
        write_output_file(&f, None).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "");
        let names: Vec<_> = std::fs::read_dir(&*d)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("thinfloe-menu-output")]);
        assert!(write_output_file(&d.join("missing/dir/f"), Some("X")).is_err());
    }

    #[test]
    fn applied_reads_the_first_line_and_none_when_missing() {
        let d = TestDir::new("menu_move", "applied");
        let f = d.join("out");
        assert_eq!(read_applied(&f), None);
        std::fs::write(applied_path(&f), "DP-2\n").unwrap();
        assert_eq!(read_applied(&f).as_deref(), Some("DP-2"));
        std::fs::write(applied_path(&f), "").unwrap();
        assert_eq!(read_applied(&f).as_deref(), Some(""));
    }

    #[test]
    fn a_confirmed_change_replaces_the_window_once() {
        let t0 = Instant::now();
        let mut m = MenuMove::default();
        assert!(!m.want(None, t0), "the start is labwc's placement: no change");
        assert!(m.want(Some("DP-2"), t0));
        assert_eq!(m.next_wake(t0), Some(CONFIRM_POLL));
        assert_eq!(m.poll(Some(""), t0 + Duration::from_millis(200)), None);
        assert_eq!(m.poll(None, t0 + Duration::from_millis(400)), None);
        assert_eq!(
            m.poll(Some("DP-2"), t0 + Duration::from_secs(1)),
            Some(Replace::Confirmed)
        );
        assert!(!m.waiting());
        assert_eq!(m.poll(Some("DP-2"), t0 + Duration::from_secs(2)), None);
        assert_eq!(m.next_wake(t0), None);
        // The monitors report again with the same answer: nothing happens.
        assert!(!m.want(Some("DP-2"), t0 + Duration::from_secs(2)));
        assert_eq!(m.poll(Some("DP-2"), t0 + Duration::from_secs(9)), None);
    }

    #[test]
    fn no_answer_replaces_the_window_after_three_seconds() {
        let t0 = Instant::now();
        let mut m = MenuMove::default();
        assert!(m.want(Some("HEADLESS-2"), t0));
        assert_eq!(m.poll(None, t0 + Duration::from_millis(2999)), None);
        assert_eq!(
            m.next_wake(t0 + Duration::from_millis(2900)),
            Some(Duration::from_millis(100))
        );
        assert_eq!(
            m.poll(None, t0 + CONFIRM_WAIT),
            Some(Replace::Unconfirmed(Some("HEADLESS-2".into())))
        );
        assert_eq!(m.poll(None, t0 + Duration::from_secs(10)), None);
    }

    #[test]
    fn an_unplugged_menu_monitor_clears_the_choice_once() {
        let t0 = Instant::now();
        let mut m = MenuMove::default();
        assert!(m.want(Some("DP-2"), t0));
        assert_eq!(m.poll(Some("DP-2"), t0), Some(Replace::Confirmed));
        // Unplugged: an empty file, confirmed by an empty .applied.
        let t1 = t0 + Duration::from_secs(5);
        assert!(m.want(None, t1));
        assert_eq!(m.poll(Some("DP-2"), t1), None);
        assert_eq!(m.poll(Some(""), t1 + Duration::from_secs(1)), Some(Replace::Confirmed));
        // The monitors report again without it: no second window.
        for i in 2..6 {
            assert!(!m.want(None, t1 + Duration::from_secs(i)));
            assert_eq!(m.poll(Some(""), t1 + Duration::from_secs(i)), None);
        }
        // Unconfirmed, the window is replaced after 3 s all the same.
        let t2 = t1 + Duration::from_secs(10);
        assert!(m.want(Some("DP-2"), t2));
        assert!(m.want(None, t2));
        assert_eq!(
            m.poll(Some("DP-2"), t2 + CONFIRM_WAIT),
            Some(Replace::Unconfirmed(None))
        );
    }

    #[test]
    fn a_new_choice_while_waiting_starts_the_wait_again() {
        let t0 = Instant::now();
        let mut m = MenuMove::default();
        assert!(m.want(Some("DP-1"), t0));
        let t1 = t0 + Duration::from_secs(2);
        assert!(m.want(Some("DP-2"), t1));
        assert_eq!(m.poll(Some("DP-1"), t1), None, "the old name confirms nothing");
        assert_eq!(
            m.poll(None, t0 + CONFIRM_WAIT),
            None,
            "the wait counts from the new choice"
        );
        assert_eq!(
            m.poll(Some("DP-2"), t1 + Duration::from_secs(1)),
            Some(Replace::Confirmed)
        );
    }
}
