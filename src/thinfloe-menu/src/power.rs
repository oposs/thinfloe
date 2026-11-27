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
use std::time::{Duration, SystemTime};
use winit::event::{ElementState, MouseButton, TouchPhase, WindowEvent};

/// The state file's name under `XDG_RUNTIME_DIR`; `screen-power` writes it.
pub const SCREEN_FILE: &str = "thinfloe-screen";

/// A press this soon after the screens came back on is still the press that
/// woke them: swayidle runs `screen-power on` on the first input, so the
/// file can say `on` before the menu sees the press itself.
pub const WAKE_GRACE: Duration = Duration::from_millis(300);

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
    /// The screens are blanked now.
    pub dark: bool,
    /// How long ago they came back on; `None` when the file does not say.
    pub since_lit: Option<Duration>,
}

impl Screens {
    /// From the file's content and modification time. Only `off` and `on`
    /// count; anything else is a file that says nothing. A modification time
    /// after `now` (a clock step) is a wake just now.
    pub fn parse(content: &[u8], modified: SystemTime, now: SystemTime) -> Screens {
        let first = content.split(|&b| b == b'\n').next().unwrap_or_default();
        match first.trim_ascii() {
            b"off" => Screens {
                dark: true,
                since_lit: None,
            },
            b"on" => Screens {
                dark: false,
                since_lit: Some(now.duration_since(modified).unwrap_or(Duration::ZERO)),
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

    /// Whether a press now is the one that wakes the screen.
    pub fn woken_by_press(&self) -> bool {
        self.dark || self.since_lit.is_some_and(|d| d < WAKE_GRACE)
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
}

impl WakeGuard {
    /// `screens` is asked only for a press or a touch start.
    pub fn filter(&mut self, input: Input, screens: impl FnOnce() -> Screens) -> Verdict {
        match input {
            Input::Press(b) => {
                self.buttons.retain(|&x| x != b);
                if screens().woken_by_press() {
                    self.buttons.push(b);
                    Verdict::DropWake
                } else {
                    Verdict::Pass
                }
            }
            Input::Release(b) => take(&mut self.buttons, b),
            Input::TouchStart(id) => {
                self.touches.retain(|&x| x != id);
                if screens().woken_by_press() {
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

    fn dark() -> Screens {
        Screens {
            dark: true,
            since_lit: None,
        }
    }

    fn lit_ago(ms: u64) -> Screens {
        Screens {
            dark: false,
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
        assert!(lit_ago(299).woken_by_press());
        assert!(!lit_ago(300).woken_by_press());
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
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let ms = Duration::from_millis;
        assert_eq!(Screens::parse(b"off\n", t0, t0 + ms(5000)), dark());
        assert_eq!(Screens::parse(b"on\n", t0, t0 + ms(120)), lit_ago(120));
        assert_eq!(Screens::parse(b"on", t0, t0 + ms(120)), lit_ago(120));
        // A clock step backwards: a wake just now.
        assert_eq!(Screens::parse(b"on\n", t0 + ms(50), t0), lit_ago(0));
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
        assert!(Screens::read(&p).dark);
        std::fs::write(&p, "on\n").unwrap();
        let s = Screens::read(&p);
        assert!(!s.dark);
        assert!(s.since_lit.is_some_and(|d| d < Duration::from_secs(60)));
    }
}
