//! The screen settings: how long the session idles before the displays
//! power off, kept in `~/.config/thinfloe/screen`.

use crate::owned::{self, Owner};
use std::ffi::OsStr;
use std::path::Path;

/// Relative to $HOME.
pub const SCREEN_FILE: &str = ".config/thinfloe/screen";
/// Five minutes.
pub const DEFAULT_IDLE: u32 = 300;
/// A day; a bigger number is a typo, not a wish.
pub const MAX_IDLE: u32 = 86400;
/// What the select offers, in seconds. 0 is never.
pub const CHOICES: [u32; 8] = [0, 60, 120, 300, 600, 900, 1800, 3600];

/// Who decides the idle time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleSource {
    /// The session profile sets it; the select is disabled.
    Rc,
    /// The menu wrote the file.
    Menu,
    /// Somebody wrote the file by hand; the menu leaves it alone.
    ByHand,
    /// No file yet.
    Unset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenTab {
    pub idle: u32,
    pub source: IdleSource,
    pub error: Option<String>,
}

impl Default for ScreenTab {
    fn default() -> ScreenTab {
        ScreenTab {
            idle: DEFAULT_IDLE,
            source: IdleSource::Unset,
            error: None,
        }
    }
}

/// The value of the last `idle_timeout=` line. Anything but plain digits
/// within `MAX_IDLE` on that line is no value at all. One `\r` at the end
/// of a line is dropped, as the session does.
pub fn parse_idle(text: &str) -> Option<u32> {
    let value = text
        .split('\n')
        .rev()
        .find_map(|l| l.strip_suffix('\r').unwrap_or(l).strip_prefix("idle_timeout="))?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok().filter(|&n| n <= MAX_IDLE)
}

/// The settings found at `path`. `rc` is the idle time the session profile
/// sets; it wins over the file.
pub fn load(path: &Path, rc: Option<u32>) -> ScreenTab {
    let mut tab = ScreenTab::default();
    if let Some(idle) = rc {
        tab.source = IdleSource::Rc;
        tab.idle = idle;
        return tab;
    }
    let text = match owned::read(path) {
        Ok(t) => t,
        Err(e) => {
            tab.error = Some(format!("Could not read {}: {e}", owned::home_relative(path)));
            return tab;
        }
    };
    tab.source = match owned::owner(text.as_deref()) {
        Owner::Missing => IdleSource::Unset,
        Owner::Menu => IdleSource::Menu,
        Owner::ByHand => IdleSource::ByHand,
    };
    if let Some(idle) = text.as_deref().and_then(parse_idle) {
        tab.idle = idle;
    }
    tab
}

/// The idle time the session profile sets, from `TF_IDLE_TIMEOUT_RC`
/// (`flag`) and `TF_IDLE_TIMEOUT` (`value`); `None` when it sets none.
/// thinfloe-session exports the value it uses, as plain digits; anything
/// else, as in the session, turns blanking off. No `MAX_IDLE` cap: the
/// session takes any positive number from the profile.
pub fn rc_idle(flag: Option<&OsStr>, value: Option<&OsStr>) -> Option<u32> {
    if flag? != "1" {
        return None;
    }
    let v = value.and_then(OsStr::to_str).unwrap_or("");
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return Some(0);
    }
    Some(v.parse().unwrap_or(u32::MAX))
}

/// The idle helper's PID from `TF_IDLE_PID`: plain digits above 1, so a
/// signal never goes to init or to a process group.
pub fn idle_pid(v: Option<&OsStr>) -> Option<i32> {
    let v = v?.to_str()?;
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    v.parse().ok().filter(|&pid| pid > 1)
}

/// The file body for `idle` seconds, without the header.
pub fn body(idle: u32) -> String {
    format!("idle_timeout={idle}\n")
}

/// `CHOICES`, with `current` added at its sorted place when it is not one
/// of them, so a value set by hand still shows in the select.
pub fn options(current: u32) -> Vec<u32> {
    let mut v = CHOICES.to_vec();
    if !v.contains(&current) {
        v.push(current);
        v.sort_unstable();
    }
    v
}

/// "Never", "1 minute", "5 minutes", or "450 seconds".
pub fn label(secs: u32) -> String {
    match secs {
        0 => "Never".to_string(),
        60 => "1 minute".to_string(),
        n if n % 60 == 0 => format!("{} minutes", n / 60),
        n => format!("{n} seconds"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::fs;

    #[test]
    fn parse_idle_takes_the_last_valid_line() {
        assert_eq!(parse_idle("idle_timeout=60\nidle_timeout=120\n"), Some(120));
        assert_eq!(parse_idle("idle_timeout=0"), Some(0));
    }

    #[test]
    fn parse_idle_refuses_garbage() {
        for t in [
            "idle_timeout=-5",
            "idle_timeout=5m",
            "idle_timeout=86401",
            "idle_timeout= 60",
            "",
        ] {
            assert_eq!(parse_idle(t), None, "{t:?}");
        }
    }

    #[test]
    fn load_without_file_is_default_unset() {
        let d = TestDir::new("screentab", "none");
        let t = load(&d.join("screen"), None);
        assert_eq!((t.idle, t.source, t.error), (300, IdleSource::Unset, None));
    }

    #[test]
    fn load_with_rc_shows_the_rc_value() {
        let d = TestDir::new("screentab", "rc");
        owned::write(&d.join("screen"), &body(60)).unwrap();
        for v in [900, 0, 420] {
            let t = load(&d.join("screen"), Some(v));
            assert_eq!((t.idle, t.source, t.error), (v, IdleSource::Rc, None));
        }
    }

    #[test]
    fn rc_idle_reads_the_session_value() {
        use std::ffi::OsStr;
        let one = Some(OsStr::new("1"));
        assert_eq!(rc_idle(one, Some(OsStr::new("900"))), Some(900));
        assert_eq!(rc_idle(one, Some(OsStr::new("0"))), Some(0));
        // No day cap: the sessionrc may say what it likes.
        assert_eq!(rc_idle(one, Some(OsStr::new("172800"))), Some(172800));
        // The session exports digits only; anything else is no blanking.
        for v in ["", "-5", "+900", " 900", "9e2"] {
            assert_eq!(rc_idle(one, Some(OsStr::new(v))), Some(0), "{v:?}");
        }
        assert_eq!(rc_idle(one, None), Some(0));
        assert_eq!(rc_idle(one, Some(OsStr::new("99999999999"))), Some(u32::MAX));
        for flag in [None, Some(OsStr::new("")), Some(OsStr::new("0"))] {
            assert_eq!(rc_idle(flag, Some(OsStr::new("900"))), None, "{flag:?}");
        }
    }

    #[test]
    fn load_read_error_names_the_file() {
        let d = TestDir::new("screentab", "read-error");
        // A directory where the file belongs cannot be read as one.
        fs::create_dir(d.join("screen")).unwrap();
        let e = load(&d.join("screen"), None).error.unwrap();
        let want = format!("Could not read {}: ", owned::home_relative(&d.join("screen")));
        assert!(e.starts_with(&want), "{e}");
    }

    #[test]
    fn parse_idle_ignores_a_trailing_cr() {
        assert_eq!(parse_idle("idle_timeout=60\r"), Some(60));
        assert_eq!(parse_idle("idle_timeout=60\r\nidle_timeout=120\r\n"), Some(120));
        // One \r only, as the session strips: two are garbage.
        assert_eq!(parse_idle("idle_timeout=60\r\r"), None);
        assert_eq!(parse_idle("idle_timeout=60\r\r\n"), None);
    }

    #[test]
    fn load_hand_file_shows_its_value() {
        let d = TestDir::new("screentab", "hand");
        fs::write(d.join("screen"), "idle_timeout=900\n").unwrap();
        let t = load(&d.join("screen"), None);
        assert_eq!((t.idle, t.source), (900, IdleSource::ByHand));
    }

    #[test]
    fn load_menu_file_shows_its_value() {
        let d = TestDir::new("screentab", "menu");
        owned::write(&d.join("screen"), &body(120)).unwrap();
        let t = load(&d.join("screen"), None);
        assert_eq!((t.idle, t.source), (120, IdleSource::Menu));
    }

    #[test]
    fn options_insert_an_odd_value() {
        assert_eq!(options(420), [0, 60, 120, 300, 420, 600, 900, 1800, 3600]);
        assert_eq!(options(300), CHOICES);
    }

    #[test]
    fn idle_pid_parses_only_pids() {
        use std::ffi::OsStr;
        assert_eq!(idle_pid(Some(OsStr::new("1234"))), Some(1234));
        for v in ["1", "0", "-5", "12a", "", "+7", "99999999999"] {
            assert_eq!(idle_pid(Some(OsStr::new(v))), None, "{v:?}");
        }
        assert_eq!(idle_pid(None), None);
    }

    #[test]
    fn labels() {
        assert_eq!(label(0), "Never");
        assert_eq!(label(60), "1 minute");
        assert_eq!(label(300), "5 minutes");
        assert_eq!(label(450), "450 seconds");
    }
}
