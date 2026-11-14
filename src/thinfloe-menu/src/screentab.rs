//! The screen settings: how long the session idles before the displays
//! power off, kept in the `[screen]` table of `config.toml`.

use crate::config::ScreenCfg;
use std::ffi::OsStr;

/// Five minutes.
pub const DEFAULT_IDLE: u32 = 300;
/// A day; a bigger number is a typo, not a wish.
pub const MAX_IDLE: u32 = 86400;
/// What the select offers, in seconds. 0 is never.
pub const CHOICES: [u32; 8] = [0, 60, 120, 300, 600, 900, 1800, 3600];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenTab {
    pub idle: u32,
    pub error: Option<String>,
}

impl Default for ScreenTab {
    fn default() -> ScreenTab {
        ScreenTab {
            idle: DEFAULT_IDLE,
            error: None,
        }
    }
}

/// The tab for the settings in `config.toml`.
pub fn from_config(c: &ScreenCfg) -> ScreenTab {
    ScreenTab {
        idle: c.idle_timeout,
        error: None,
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
    fn parse_idle_ignores_a_trailing_cr() {
        assert_eq!(parse_idle("idle_timeout=60\r"), Some(60));
        assert_eq!(parse_idle("idle_timeout=60\r\nidle_timeout=120\r\n"), Some(120));
        // One \r only, as the session strips: two are garbage.
        assert_eq!(parse_idle("idle_timeout=60\r\r"), None);
        assert_eq!(parse_idle("idle_timeout=60\r\r\n"), None);
    }

    #[test]
    fn from_config_shows_the_stored_idle_time() {
        assert_eq!(
            from_config(&ScreenCfg { idle_timeout: 120 }),
            ScreenTab { idle: 120, error: None }
        );
        assert_eq!(from_config(&ScreenCfg::default()).idle, DEFAULT_IDLE);
        assert_eq!(from_config(&ScreenCfg { idle_timeout: 0 }).idle, 0);
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
