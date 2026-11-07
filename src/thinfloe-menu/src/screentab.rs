//! The screen settings: how long the session idles before the displays
//! power off, kept in `~/.config/thinfloe/screen`.

use crate::owned::{self, Owner};
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
/// within `MAX_IDLE` on that line is no value at all.
pub fn parse_idle(text: &str) -> Option<u32> {
    let value = text.lines().rev().find_map(|l| l.strip_prefix("idle_timeout="))?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok().filter(|&n| n <= MAX_IDLE)
}

/// The settings found at `path`. `rc_set` says the session profile sets
/// the idle time, which then wins and its value is not needed here.
pub fn load(path: &Path, rc_set: bool) -> ScreenTab {
    let mut tab = ScreenTab::default();
    if rc_set {
        tab.source = IdleSource::Rc;
        return tab;
    }
    let text = match owned::read(path) {
        Ok(t) => t,
        Err(e) => {
            tab.error = Some(e.to_string());
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
        for t in ["idle_timeout=-5", "idle_timeout=5m", "idle_timeout=86401", "idle_timeout= 60", ""] {
            assert_eq!(parse_idle(t), None, "{t:?}");
        }
    }

    #[test]
    fn load_without_file_is_default_unset() {
        let d = TestDir::new("screentab", "none");
        let t = load(&d.join("screen"), false);
        assert_eq!((t.idle, t.source, t.error), (300, IdleSource::Unset, None));
    }

    #[test]
    fn load_with_rc_set_is_rc() {
        let d = TestDir::new("screentab", "rc");
        owned::write(&d.join("screen"), &body(60)).unwrap();
        assert_eq!(load(&d.join("screen"), true).source, IdleSource::Rc);
    }

    #[test]
    fn load_hand_file_shows_its_value() {
        let d = TestDir::new("screentab", "hand");
        fs::write(d.join("screen"), "idle_timeout=900\n").unwrap();
        let t = load(&d.join("screen"), false);
        assert_eq!((t.idle, t.source), (900, IdleSource::ByHand));
    }

    #[test]
    fn load_menu_file_shows_its_value() {
        let d = TestDir::new("screentab", "menu");
        owned::write(&d.join("screen"), &body(120)).unwrap();
        let t = load(&d.join("screen"), false);
        assert_eq!((t.idle, t.source), (120, IdleSource::Menu));
    }

    #[test]
    fn options_insert_an_odd_value() {
        assert_eq!(options(420), [0, 60, 120, 300, 420, 600, 900, 1800, 3600]);
        assert_eq!(options(300), CHOICES);
    }

    #[test]
    fn labels() {
        assert_eq!(label(0), "Never");
        assert_eq!(label(60), "1 minute");
        assert_eq!(label(300), "5 minutes");
        assert_eq!(label(450), "450 seconds");
    }
}
