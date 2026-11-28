//! The press that wakes a dark screen (#68).
//!
//! swayidle blanks the screens through `/usr/libexec/thinfloe/screen-power`,
//! which runs `wlopm` and notes the state in `$XDG_RUNTIME_DIR/thinfloe-screen`:
//! `off` while the screens are dark, `on` once they are back, the file's
//! modification time saying when. labwc keeps delivering pointer input while
//! the outputs are off, so without a guard the press that woke the screen
//! also started the tile under the pointer.
//!
//! The menu cannot watch `zwlr_output_power_management_v1` itself: wlroots
//! gives an output's power control to one client only, and `wlopm` would then
//! fail to blank. Nor does `zwlr_output_manager_v1` tell, since a powered-off
//! output stays enabled there.
//!
//! `Screens` is what the file says at one moment, `WakeGuard` the decision
//! which presses to drop; both are pure.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use winit::event::{ElementState, MouseButton, TouchPhase, WindowEvent};

/// The state file's name under `XDG_RUNTIME_DIR`; `screen-power` writes it.
pub const SCREEN_FILE: &str = "thinfloe-screen";

/// A press this soon after the screens came back on is still the press that
/// woke them: swayidle runs `screen-power on` on the first input, so the
/// file can say `on` before the menu sees the press itself.
pub const WAKE_GRACE: Duration = Duration::from_millis(300);

/// How long the presses under one `off` note are dropped after the first.
/// The note says `off` until the `on` run has ended (sh, the lock,
/// `wlopm --on`, a modeset, the rename), which can take well over 100 ms,
/// so the second press of a double-click lands under the same note. A note
/// still `off` this long after a drop is stale (a swayidle that died while
/// dark) and gives way.
pub const STALE_OFF_AFTER: Duration = Duration::from_secs(2);

/// More than the file ever holds (`off` or `on` and a newline).
const READ_LIMIT: u64 = 64;

/// The state file, or `None` without `XDG_RUNTIME_DIR`; then no press is
/// dropped.
pub fn path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(|d| PathBuf::from(d).join(SCREEN_FILE))
}

/// What the screens are doing at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Screens {
    /// The modification time of the `off` note while the screens are
    /// blanked; `None` when they are not.
    pub off_note: Option<SystemTime>,
    /// How long ago they came back on; `None` when the file does not say.
    pub since_lit: Option<Duration>,
}

impl Screens {
    /// From the file's content and modification time. Only `off` and `on`
    /// count; anything else is a file that says nothing. An `on` note
    /// modified after `now` (the clock stepped back) is no recent wake:
    /// counted as one, it would drop every press for the whole step.
    pub fn parse(content: &[u8], modified: SystemTime, now: SystemTime) -> Screens {
        let first = content.split(|&b| b == b'\n').next().unwrap_or_default();
        match first.trim_ascii() {
            b"off" => Screens {
                off_note: Some(modified),
                since_lit: None,
            },
            b"on" => Screens {
                off_note: None,
                since_lit: now.duration_since(modified).ok(),
            },
            _ => Screens::default(),
        }
    }

    /// Reads the file now. A missing or unreadable file says nothing.
    pub fn read(path: &Path) -> Screens {
        let read = || -> std::io::Result<Screens> {
            let f = std::fs::File::open(path)?;
            let modified = f.metadata()?.modified()?;
            let mut content = Vec::new();
            f.take(READ_LIMIT).read_to_end(&mut content)?;
            Ok(Screens::parse(&content, modified, SystemTime::now()))
        };
        read().unwrap_or_default()
    }

    pub fn dark(&self) -> bool {
        self.off_note.is_some()
    }

    /// Whether the screens came back on less than `WAKE_GRACE` ago.
    pub fn just_lit(&self) -> bool {
        self.since_lit.is_some_and(|d| d < WAKE_GRACE)
    }
}

/// The part of a window event the guard looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Press(MouseButton),
    Release(MouseButton),
    TouchStart(u64),
    /// A touch's move (`false`), or its end or cancel (`true`).
    TouchRest(u64, bool),
    Other,
}

impl Input {
    pub fn of(e: &WindowEvent) -> Input {
        match e {
            WindowEvent::MouseInput { state, button, .. } => match state {
                ElementState::Pressed => Input::Press(*button),
                ElementState::Released => Input::Release(*button),
            },
            WindowEvent::Touch(t) => match t.phase {
                TouchPhase::Started => Input::TouchStart(t.id),
                TouchPhase::Moved => Input::TouchRest(t.id, false),
                TouchPhase::Ended | TouchPhase::Cancelled => Input::TouchRest(t.id, true),
            },
            _ => Input::Other,
        }
    }
}

/// What to do with one event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    /// A press or touch start that woke the screen.
    DropWake,
    /// The rest of a dropped press or touch.
    Drop,
}

/// Drops the press that wakes a dark screen, with its release, so egui
/// never sees half a click. Motion, the wheel and keys always pass.
#[derive(Default)]
pub struct WakeGuard {
    buttons: Vec<MouseButton>,
    touches: Vec<u64>,
    /// The `off` note a press was dropped under (its modification time),
    /// and when the first press under it was dropped. Presses under the same
    /// note are dropped until `STALE_OFF_AFTER` has gone by, then pass, so
    /// a note left behind cannot eat every click.
    dropped_under: Option<(SystemTime, Instant)>,
}

impl WakeGuard {
    /// `screens` is asked only for a press or a touch start.
    pub fn filter(&mut self, input: Input, screens: impl FnOnce() -> Screens) -> Verdict {
        self.filter_at(input, screens, Instant::now())
    }

    /// `filter` with the time of the event given.
    pub fn filter_at(&mut self, input: Input, screens: impl FnOnce() -> Screens, now: Instant) -> Verdict {
        match input {
            Input::Press(b) => {
                self.buttons.retain(|&x| x != b);
                if self.wakes(screens(), now) {
                    self.buttons.push(b);
                    Verdict::DropWake
                } else {
                    Verdict::Pass
                }
            }
            Input::Release(b) => take(&mut self.buttons, b),
            Input::TouchStart(id) => {
                self.touches.retain(|&x| x != id);
                if self.wakes(screens(), now) {
                    self.touches.push(id);
                    Verdict::DropWake
                } else {
                    Verdict::Pass
                }
            }
            Input::TouchRest(id, true) => take(&mut self.touches, id),
            Input::TouchRest(id, false) if self.touches.contains(&id) => Verdict::Drop,
            Input::TouchRest(..) | Input::Other => Verdict::Pass,
        }
    }

    /// Whether a press at `now` is part of waking the screen: any under an
    /// `off` note until `STALE_OFF_AFTER` after the first drop under it, or
    /// any within `WAKE_GRACE` after an `on` note.
    fn wakes(&mut self, s: Screens, now: Instant) -> bool {
        match (s.off_note, self.dropped_under) {
            (Some(note), Some((seen, first))) if seen == note => now.saturating_duration_since(first) < STALE_OFF_AFTER,
            (Some(note), _) => {
                self.dropped_under = Some((note, now));
                true
            }
            (None, _) => s.just_lit(),
        }
    }
}

/// The end of a press or touch: dropped when its start was.
fn take<T: PartialEq>(held: &mut Vec<T>, x: T) -> Verdict {
    match held.iter().position(|h| *h == x) {
        Some(i) => {
            held.swap_remove(i);
            Verdict::Drop
        }
        None => Verdict::Pass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    const LEFT: MouseButton = MouseButton::Left;

    const T0: SystemTime = SystemTime::UNIX_EPOCH;

    /// Dark under the `off` note of `T0` plus `s` seconds.
    fn dark_since(s: u64) -> Screens {
        Screens {
            off_note: Some(T0 + Duration::from_secs(1_800_000_000 + s)),
            since_lit: None,
        }
    }

    fn dark() -> Screens {
        dark_since(0)
    }

    fn lit_ago(ms: u64) -> Screens {
        Screens {
            off_note: None,
            since_lit: Some(Duration::from_millis(ms)),
        }
    }

    #[test]
    fn a_press_on_a_dark_screen_is_dropped_with_its_release() {
        let mut g = WakeGuard::default();
        assert_eq!(g.filter(Input::Press(LEFT), dark), Verdict::DropWake);
        // Motion in between, and the screen lit by the time the button comes up.
        assert_eq!(g.filter(Input::Other, || unreachable!()), Verdict::Pass);
        assert_eq!(g.filter(Input::Release(LEFT), || unreachable!()), Verdict::Drop);
        // The next click is a click.
        assert_eq!(g.filter(Input::Press(LEFT), || lit_ago(2000)), Verdict::Pass);
        assert_eq!(g.filter(Input::Release(LEFT), || unreachable!()), Verdict::Pass);
    }

    #[test]
    fn a_press_within_the_grace_is_dropped_and_after_it_passes() {
        let mut g = WakeGuard::default();
        assert_eq!(g.filter(Input::Press(LEFT), || lit_ago(200)), Verdict::DropWake);
        assert_eq!(g.filter(Input::Release(LEFT), || unreachable!()), Verdict::Drop);
        assert_eq!(g.filter(Input::Press(LEFT), || lit_ago(400)), Verdict::Pass);
        assert_eq!(g.filter(Input::Release(LEFT), || unreachable!()), Verdict::Pass);
        assert!(lit_ago(299).just_lit());
        assert!(!lit_ago(300).just_lit());
    }

    #[test]
    fn a_release_of_a_button_not_dropped_passes() {
        let mut g = WakeGuard::default();
        assert_eq!(g.filter(Input::Press(LEFT), dark), Verdict::DropWake);
        // The right button went down before the screen went dark.
        assert_eq!(
            g.filter(Input::Release(MouseButton::Right), || unreachable!()),
            Verdict::Pass
        );
        assert_eq!(g.filter(Input::Release(LEFT), || unreachable!()), Verdict::Drop);
        // A file that says nothing: every event passes.
        let mut g = WakeGuard::default();
        for i in [
            Input::Press(LEFT),
            Input::Release(LEFT),
            Input::TouchStart(1),
            Input::TouchRest(1, false),
            Input::TouchRest(1, true),
            Input::Other,
        ] {
            assert_eq!(g.filter(i, Screens::default), Verdict::Pass);
        }
    }

    #[test]
    fn a_press_that_passes_forgets_an_earlier_dropped_one() {
        // A release lost to the dialog's refusal must not eat a later one.
        let mut g = WakeGuard::default();
        assert_eq!(g.filter(Input::Press(LEFT), dark), Verdict::DropWake);
        assert_eq!(g.filter(Input::Press(LEFT), || lit_ago(5000)), Verdict::Pass);
        assert_eq!(g.filter(Input::Release(LEFT), || unreachable!()), Verdict::Pass);
    }

    #[test]
    fn one_off_note_drops_presses_until_it_is_stale() {
        let t = Instant::now();
        let at = |ms: u64| t + Duration::from_millis(ms);
        let mut g = WakeGuard::default();
        assert_eq!(g.filter_at(Input::Press(LEFT), dark, at(0)), Verdict::DropWake);
        assert_eq!(
            g.filter_at(Input::Release(LEFT), || unreachable!(), at(80)),
            Verdict::Drop
        );
        // A double-click: the second press comes while the "on" run is still
        // under way and the note still says off. It is dropped too, and so is
        // a second finger.
        assert_eq!(g.filter_at(Input::Press(LEFT), dark, at(150)), Verdict::DropWake);
        assert_eq!(
            g.filter_at(Input::Release(LEFT), || unreachable!(), at(230)),
            Verdict::Drop
        );
        assert_eq!(g.filter_at(Input::TouchStart(3), dark, at(1900)), Verdict::DropWake);
        assert_eq!(
            g.filter_at(Input::TouchRest(3, true), || unreachable!(), at(1950)),
            Verdict::Drop
        );
        // A note left behind by a swayidle that died while dark gives way
        // STALE_OFF_AFTER after the first drop under it.
        assert_eq!(g.filter_at(Input::Press(LEFT), dark, at(2500)), Verdict::Pass);
        assert_eq!(
            g.filter_at(Input::Release(LEFT), || unreachable!(), at(2550)),
            Verdict::Pass
        );
        assert_eq!(g.filter_at(Input::TouchStart(4), dark, at(2600)), Verdict::Pass);
        // The grace after an `on` note still drops, without touching the
        // memory of the old `off` note.
        assert_eq!(
            g.filter_at(Input::Press(LEFT), || lit_ago(100), at(3000)),
            Verdict::DropWake
        );
        assert_eq!(
            g.filter_at(Input::Release(LEFT), || unreachable!(), at(3050)),
            Verdict::Drop
        );
        assert_eq!(g.filter_at(Input::Press(LEFT), dark, at(3100)), Verdict::Pass);
        // The next blanking writes a new note: its presses are dropped again.
        let later = || dark_since(600);
        assert_eq!(g.filter_at(Input::Press(LEFT), later, at(9000)), Verdict::DropWake);
        assert_eq!(
            g.filter_at(Input::Release(LEFT), || unreachable!(), at(9050)),
            Verdict::Drop
        );
        assert_eq!(g.filter_at(Input::Press(LEFT), later, at(9150)), Verdict::DropWake);
        assert_eq!(g.filter_at(Input::Press(LEFT), later, at(11_500)), Verdict::Pass);
    }

    #[test]
    fn a_touch_on_a_dark_screen_is_dropped_whole() {
        let mut g = WakeGuard::default();
        assert_eq!(g.filter(Input::TouchStart(7), dark), Verdict::DropWake);
        assert_eq!(g.filter(Input::TouchRest(7, false), || unreachable!()), Verdict::Drop);
        // Another finger, put down later, is a touch.
        assert_eq!(g.filter(Input::TouchStart(8), || lit_ago(1000)), Verdict::Pass);
        assert_eq!(g.filter(Input::TouchRest(8, false), || unreachable!()), Verdict::Pass);
        assert_eq!(g.filter(Input::TouchRest(8, true), || unreachable!()), Verdict::Pass);
        assert_eq!(g.filter(Input::TouchRest(7, true), || unreachable!()), Verdict::Drop);
        assert_eq!(g.filter(Input::TouchRest(7, true), || unreachable!()), Verdict::Pass);
    }

    #[test]
    fn the_file_says_off_on_or_nothing() {
        let t0 = T0 + Duration::from_secs(1_800_000_000);
        let ms = Duration::from_millis;
        assert_eq!(Screens::parse(b"off\n", t0, t0 + ms(5000)), dark());
        assert_eq!(Screens::parse(b"on\n", t0, t0 + ms(120)), lit_ago(120));
        assert_eq!(Screens::parse(b"on", t0, t0 + ms(120)), lit_ago(120));
        // The clock stepped back after the wake: no recent wake, a press passes.
        let stepped = Screens::parse(b"on\n", t0 + ms(50), t0);
        assert_eq!(stepped, Screens::default());
        let mut g = WakeGuard::default();
        assert_eq!(g.filter(Input::Press(LEFT), || stepped), Verdict::Pass);
        for junk in [&b""[..], b"\n", b"of", b"offf\n", b"1\n", b"\non\n"] {
            assert_eq!(Screens::parse(junk, t0, t0), Screens::default(), "{junk:?}");
        }
    }

    #[test]
    fn a_missing_file_says_nothing_and_a_written_one_is_read() {
        let dir = TestDir::new("power", "file");
        let p = dir.join(SCREEN_FILE);
        assert_eq!(Screens::read(&p), Screens::default());
        std::fs::write(&p, "off\n").unwrap();
        assert!(Screens::read(&p).dark());
        std::fs::write(&p, "on\n").unwrap();
        let s = Screens::read(&p);
        assert!(!s.dark());
        assert!(s.since_lit.is_some_and(|d| d < Duration::from_secs(60)));
    }
}
