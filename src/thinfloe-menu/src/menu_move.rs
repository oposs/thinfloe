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

/// When `.applied` was last replaced: its inode and modification time.
/// labwc-scale writes a new file and renames it over the old one, so each
/// write it makes gives a new stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp(pub u64, pub i128);

/// What `.applied` holds and when it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// The first line: the output in effect, `""` for none.
    pub text: String,
    pub stamp: Stamp,
}

/// The output labwc-scale has in effect, from `.applied`; `None` while the
/// file is missing or unreadable.
pub fn read_applied(path: &Path) -> Option<Applied> {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    let mut f = std::fs::File::open(applied_path(path)).ok()?;
    let meta = f.metadata().ok()?;
    let mut text = String::new();
    f.read_to_string(&mut text).ok()?;
    Some(Applied {
        text: text.lines().next().unwrap_or("").to_string(),
        stamp: Stamp(
            meta.ino(),
            meta.mtime() as i128 * 1_000_000_000 + meta.mtime_nsec() as i128,
        ),
    })
}

/// What `MenuMove::poll` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replace {
    /// labwc-scale has the asked output in effect: replace the window.
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
    /// The stamp `.applied` had when `asked` changed. Only a later write
    /// confirms: one that already named the output may be from before a
    /// quick change away and back, which labwc-scale is still applying.
    baseline: Option<Stamp>,
    /// `asked` is not in the output file yet: the write failed or is due.
    unwritten: bool,
    /// The last write attempt.
    last_try: Option<Instant>,
    /// A failed write of this change was reported already.
    failure_told: bool,
}

impl MenuMove {
    /// The menu should be on `output` now; `applied` is what `.applied`
    /// holds at this moment. Returns true when that is a change; the
    /// output file is then due (`write_due`). The same answer again changes
    /// nothing, so monitors that report again cause no second window.
    pub fn want(&mut self, output: Option<&str>, applied: Option<&Applied>, now: Instant) -> bool {
        if !self.changes(output) {
            return false;
        }
        self.asked = output.map(str::to_string);
        self.since = Some(now);
        self.baseline = applied.map(|a| a.stamp);
        self.unwritten = true;
        self.last_try = None;
        self.failure_told = false;
        true
    }

    /// Whether `want(output, ..)` would be a change.
    pub fn changes(&self, output: Option<&str>) -> bool {
        self.asked.as_deref() != output
    }

    /// The output last asked for.
    pub fn asked(&self) -> Option<&str> {
        self.asked.as_deref()
    }

    /// Whether the output file must be written now: after a change, and
    /// after a failed write again every `CONFIRM_POLL` for as long as the
    /// change waits (at most `CONFIRM_WAIT`).
    pub fn write_due(&self, now: Instant) -> bool {
        self.unwritten
            && self.since.is_some()
            && self
                .last_try
                .is_none_or(|t| now.saturating_duration_since(t) >= CONFIRM_POLL)
    }

    /// The write was tried. Returns true for the first failure of a
    /// change, the one worth reporting.
    pub fn wrote(&mut self, ok: bool, now: Instant) -> bool {
        self.last_try = Some(now);
        if ok {
            self.unwritten = false;
            return false;
        }
        !std::mem::replace(&mut self.failure_told, true)
    }

    /// While a change waits: replace the window once `.applied` (`applied`,
    /// `None` when missing) was written after the change and names the
    /// asked output, or once `CONFIRM_WAIT` has passed. Returns `Some` once
    /// per change.
    pub fn poll(&mut self, applied: Option<&Applied>, now: Instant) -> Option<Replace> {
        let since = self.since?;
        let asked = self.asked.as_deref().unwrap_or("");
        let names_it = !self.unwritten && applied.is_some_and(|a| a.text == asked);
        let fresh = applied.is_some_and(|a| Some(a.stamp) != self.baseline);
        if names_it && fresh {
            self.since = None;
            return Some(Replace::Confirmed);
        }
        if now.saturating_duration_since(since) >= CONFIRM_WAIT {
            self.since = None;
            // Still the asked output after the wait: labwc-scale never saw
            // the change away, the rule in effect is the asked one.
            if names_it {
                return Some(Replace::Confirmed);
            }
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

    /// `.applied` holding `text`, written as the `n`th file.
    fn ap(text: &str, n: u64) -> Applied {
        Applied {
            text: text.into(),
            stamp: Stamp(n, 0),
        }
    }

    /// A change whose output file was written.
    fn ask(m: &mut MenuMove, output: Option<&str>, applied: Option<&Applied>, now: Instant) -> bool {
        let changed = m.want(output, applied, now);
        if m.write_due(now) {
            m.wrote(true, now);
        }
        changed
    }

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
        let first = read_applied(&f).unwrap();
        assert_eq!(first.text, "DP-2");
        assert_eq!(
            read_applied(&f).unwrap().stamp,
            first.stamp,
            "unchanged file, same stamp"
        );
        // labwc-scale's way: a new file renamed over the old one.
        let tmp = d.join(".applied.x");
        std::fs::write(&tmp, "").unwrap();
        std::fs::rename(&tmp, applied_path(&f)).unwrap();
        let second = read_applied(&f).unwrap();
        assert_eq!(second.text, "");
        assert_ne!(second.stamp, first.stamp);
    }

    #[test]
    fn a_confirmed_change_replaces_the_window_once() {
        let t0 = Instant::now();
        let mut m = MenuMove::default();
        assert!(
            !ask(&mut m, None, None, t0),
            "the start is labwc's placement: no change"
        );
        assert!(ask(&mut m, Some("DP-2"), Some(&ap("", 1)), t0));
        assert_eq!(m.next_wake(t0), Some(CONFIRM_POLL));
        assert_eq!(m.poll(Some(&ap("", 1)), t0 + Duration::from_millis(200)), None);
        assert_eq!(m.poll(None, t0 + Duration::from_millis(400)), None);
        assert_eq!(
            m.poll(Some(&ap("DP-2", 2)), t0 + Duration::from_secs(1)),
            Some(Replace::Confirmed)
        );
        assert!(!m.waiting());
        assert_eq!(m.poll(Some(&ap("DP-2", 2)), t0 + Duration::from_secs(2)), None);
        assert_eq!(m.next_wake(t0), None);
        // The monitors report again with the same answer: nothing happens.
        assert!(!ask(
            &mut m,
            Some("DP-2"),
            Some(&ap("DP-2", 2)),
            t0 + Duration::from_secs(2)
        ));
        assert!(!m.write_due(t0 + Duration::from_secs(2)));
        assert_eq!(m.poll(Some(&ap("DP-2", 2)), t0 + Duration::from_secs(9)), None);
    }

    #[test]
    fn no_answer_replaces_the_window_after_three_seconds() {
        let t0 = Instant::now();
        let mut m = MenuMove::default();
        assert!(ask(&mut m, Some("HEADLESS-2"), None, t0));
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
        assert!(ask(&mut m, Some("DP-2"), None, t0));
        assert_eq!(m.poll(Some(&ap("DP-2", 1)), t0), Some(Replace::Confirmed));
        // Unplugged: an empty file, confirmed by an empty .applied.
        let t1 = t0 + Duration::from_secs(5);
        assert!(ask(&mut m, None, Some(&ap("DP-2", 1)), t1));
        assert_eq!(m.poll(Some(&ap("DP-2", 1)), t1), None);
        assert_eq!(
            m.poll(Some(&ap("", 2)), t1 + Duration::from_secs(1)),
            Some(Replace::Confirmed)
        );
        // The monitors report again without it: no second window.
        for i in 2..6 {
            assert!(!ask(&mut m, None, Some(&ap("", 2)), t1 + Duration::from_secs(i)));
            assert_eq!(m.poll(Some(&ap("", 2)), t1 + Duration::from_secs(i)), None);
        }
        // Unconfirmed, the window is replaced after 3 s all the same.
        let t2 = t1 + Duration::from_secs(10);
        assert!(ask(&mut m, Some("DP-2"), Some(&ap("", 2)), t2));
        assert!(ask(&mut m, None, Some(&ap("", 2)), t2));
        assert_eq!(
            m.poll(Some(&ap("DP-2", 3)), t2 + CONFIRM_WAIT),
            Some(Replace::Unconfirmed(None))
        );
    }

    #[test]
    fn a_new_choice_while_waiting_starts_the_wait_again() {
        let t0 = Instant::now();
        let mut m = MenuMove::default();
        assert!(ask(&mut m, Some("DP-1"), None, t0));
        let t1 = t0 + Duration::from_secs(2);
        assert!(ask(&mut m, Some("DP-2"), None, t1));
        assert_eq!(m.poll(Some(&ap("DP-1", 1)), t1), None, "the old name confirms nothing");
        assert_eq!(
            m.poll(None, t0 + CONFIRM_WAIT),
            None,
            "the wait counts from the new choice"
        );
        assert_eq!(
            m.poll(Some(&ap("DP-2", 2)), t1 + Duration::from_secs(1)),
            Some(Replace::Confirmed)
        );
    }

    #[test]
    fn a_quick_change_back_waits_for_a_new_confirmation() {
        let t0 = Instant::now();
        let mut m = MenuMove::default();
        assert!(ask(&mut m, Some("DP-2"), None, t0));
        assert_eq!(m.poll(Some(&ap("DP-2", 1)), t0), Some(Replace::Confirmed));
        // The monitor flaps: gone, and back before labwc-scale answered.
        let t1 = t0 + Duration::from_secs(5);
        assert!(ask(&mut m, None, Some(&ap("DP-2", 1)), t1));
        let t2 = t1 + Duration::from_millis(300);
        assert!(ask(&mut m, Some("DP-2"), Some(&ap("DP-2", 1)), t2));
        assert_eq!(
            m.poll(Some(&ap("DP-2", 1)), t2),
            None,
            "the old .applied is from before the flap"
        );
        // labwc-scale handles the empty file, then the name again.
        assert_eq!(m.poll(Some(&ap("", 2)), t2 + Duration::from_secs(1)), None);
        assert_eq!(
            m.poll(Some(&ap("DP-2", 3)), t2 + Duration::from_secs(2)),
            Some(Replace::Confirmed)
        );
    }

    #[test]
    fn a_flap_labwc_scale_never_saw_still_confirms_after_the_wait() {
        let t0 = Instant::now();
        let mut m = MenuMove::default();
        assert!(ask(&mut m, Some("DP-2"), None, t0));
        assert_eq!(m.poll(Some(&ap("DP-2", 1)), t0), Some(Replace::Confirmed));
        let t1 = t0 + Duration::from_secs(5);
        assert!(ask(&mut m, None, Some(&ap("DP-2", 1)), t1));
        assert!(ask(&mut m, Some("DP-2"), Some(&ap("DP-2", 1)), t1));
        assert_eq!(m.poll(Some(&ap("DP-2", 1)), t1 + Duration::from_secs(2)), None);
        // The rule in effect is the asked one: no warning.
        assert_eq!(
            m.poll(Some(&ap("DP-2", 1)), t1 + CONFIRM_WAIT),
            Some(Replace::Confirmed)
        );
    }

    #[test]
    fn a_failed_write_is_tried_again_while_the_change_waits() {
        let t0 = Instant::now();
        let mut m = MenuMove::default();
        assert!(m.want(Some("DP-2"), None, t0));
        assert!(m.write_due(t0));
        assert!(m.wrote(false, t0), "the first failure is reported");
        assert!(!m.write_due(t0 + Duration::from_millis(100)));
        let t1 = t0 + CONFIRM_POLL;
        assert!(m.write_due(t1), "tried again after a poll interval");
        assert!(!m.wrote(false, t1), "a second failure is not reported again");
        // Unwritten, a matching .applied confirms nothing.
        assert_eq!(m.poll(Some(&ap("DP-2", 1)), t1), None);
        let t2 = t1 + CONFIRM_POLL;
        assert!(m.write_due(t2));
        assert!(!m.wrote(true, t2));
        assert!(!m.write_due(t2 + CONFIRM_POLL), "written: nothing more to do");
        assert_eq!(
            m.poll(Some(&ap("DP-2", 2)), t2 + Duration::from_millis(500)),
            Some(Replace::Confirmed)
        );
    }

    #[test]
    fn retries_end_with_the_wait() {
        let t0 = Instant::now();
        let mut m = MenuMove::default();
        assert!(m.want(Some("DP-2"), None, t0));
        assert!(m.wrote(false, t0));
        assert_eq!(
            m.poll(None, t0 + CONFIRM_WAIT),
            Some(Replace::Unconfirmed(Some("DP-2".into())))
        );
        assert!(!m.write_due(t0 + Duration::from_secs(10)));
        // The same answer again is no change: no retry and no second window.
        assert!(!m.want(Some("DP-2"), None, t0 + Duration::from_secs(10)));
        // The next change writes again, and a failure is reported again.
        assert!(m.want(None, None, t0 + Duration::from_secs(11)));
        assert!(m.write_due(t0 + Duration::from_secs(11)));
        assert!(m.wrote(false, t0 + Duration::from_secs(11)));
    }
}
