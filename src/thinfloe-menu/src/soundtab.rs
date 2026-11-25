//! The Sound tab: the volume switch, a slider and a mute switch for people
//! without volume keys. The OSD (`thinfloe-menu --osd`) owns the speaker
//! state; the tab sends it `volume-set <n>` and `volume-mute-set 0|1` on
//! its socket and reads the state back from the volume file the OSD keeps
//! (`$XDG_RUNTIME_DIR/thinfloe-volume`). While the person drags, the tab
//! shows its own value and sends at most one set per `SEND_EVERY`; the
//! file's value counts again `HOLD` after the last send.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::osd::Action;

/// How often the open tab reads the volume file.
pub const READ_EVERY: Duration = Duration::from_secs(1);
/// The shortest time between two `volume-set` of a drag.
pub const SEND_EVERY: Duration = Duration::from_millis(50);
/// How long after the last send the tab shows its own value, not the
/// file's: the OSD needs a moment to set the speakers and write the file.
pub const HOLD: Duration = Duration::from_secs(1);

/// The volume file is read up to this size; one line is a few bytes.
const FILE_BYTES: u64 = 64;

/// The note under the switch.
pub const SWITCH_NOTE: &str = "When on, the slider and the volume keys set every speaker, and the volume keys no longer reach the ThinLinc session or any other program. When off, every speaker is at 100 % and the remote session sets the loudness.";
/// Why the slider is off: the switch is off.
pub const REMOTE_NOTE: &str = "The remote session sets the loudness.";
/// Why the slider is off: no volume file.
pub const UNKNOWN_NOTE: &str = "The volume is not known yet.";

/// The speaker state as the volume file says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume {
    pub percent: u8,
    pub muted: bool,
}

/// What the person did on the Sound tab, besides the switch (which is the
/// Keyboard tab's `KbEvent::VolumeKeys`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundEvent {
    /// The slider moved; `done` when the pointer let go of it.
    Level {
        percent: u8,
        done: bool,
    },
    Mute(bool),
}

/// The volume file's line: `<percent> <muted>`, percent 0 to 100 and
/// muted `0` or `1`; more fields after them are ignored. Anything else
/// is `None`.
pub fn parse(text: &str) -> Option<Volume> {
    let mut words = text.lines().next()?.split_whitespace();
    let percent = words.next()?;
    if percent.is_empty() || percent.len() > 3 || !percent.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let percent: u8 = percent.parse().ok().filter(|p| *p <= 100)?;
    let muted = match words.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    Some(Volume { percent, muted })
}

/// Whether the slider and the mute switch take input, and if not, why.
pub fn controls(switch_on: bool, shown: Option<Volume>) -> (bool, Option<&'static str>) {
    match (switch_on, shown) {
        (false, _) => (false, Some(REMOTE_NOTE)),
        (true, None) => (false, Some(UNKNOWN_NOTE)),
        (true, Some(_)) => (true, None),
    }
}

#[derive(Debug, Default)]
pub struct SoundTab {
    /// The volume file; `None` reads nothing (the tests' menus).
    path: Option<PathBuf>,
    /// The file's last value; `None` when there is none.
    file: Option<Volume>,
    /// When the open tab reads the file next; `None` reads at once.
    next_read: Option<Instant>,
    /// What the person set last, shown until `HOLD` after the last send.
    own: Option<Volume>,
    last_send: Option<Instant>,
    /// A slider value held back by the throttle.
    pending: Option<u8>,
    /// The last level sent.
    sent_level: Option<u8>,
    /// Send failures already logged.
    logged: BTreeSet<String>,
}

impl SoundTab {
    pub fn set_path(&mut self, path: Option<PathBuf>) {
        self.path = path;
        self.next_read = None;
    }

    /// The value the tab shows: the person's own while a send is held back
    /// or less than `HOLD` old, else the file's.
    pub fn shown(&self, now: Instant) -> Option<Volume> {
        if self.holding(now) {
            return self.own.or(self.file);
        }
        self.file
    }

    fn holding(&self, now: Instant) -> bool {
        self.pending.is_some() || self.last_send.is_some_and(|t| now < t + HOLD)
    }

    /// The file read (by the app, or a test).
    pub fn set_file(&mut self, v: Option<Volume>) {
        self.file = v;
    }

    fn read_file(&mut self) {
        let Some(path) = &self.path else { return };
        self.file = crate::bounded::read_bounded(path, FILE_BYTES, false)
            .ok()
            .and_then(|b| parse(&String::from_utf8_lossy(&b)));
    }

    /// The person moved the slider or the mute switch: the action to send
    /// now, if any.
    pub fn event(&mut self, e: SoundEvent, now: Instant) -> Option<Action> {
        let base = self.shown(now)?;
        match e {
            SoundEvent::Level { percent, done } => {
                let percent = percent.min(100);
                self.own = Some(Volume { percent, ..base });
                let due = self.last_send.is_none_or(|t| now >= t + SEND_EVERY);
                if done && self.pending.is_none() && self.sent_level == Some(percent) {
                    return None;
                }
                if !done && !due {
                    self.pending = Some(percent);
                    return None;
                }
                Some(self.send_level(percent, now))
            }
            SoundEvent::Mute(m) => {
                self.own = Some(Volume { muted: m, ..base });
                self.last_send = Some(now);
                Some(Action::VolumeMuteSet(m))
            }
        }
    }

    fn send_level(&mut self, percent: u8, now: Instant) -> Action {
        self.pending = None;
        self.sent_level = Some(percent);
        self.last_send = Some(now);
        Action::VolumeSet(percent)
    }

    /// Called on every frame: reads the file when the open tab is due, and
    /// sends a value the throttle held back once it may.
    pub fn poll(&mut self, now: Instant, open: bool) -> Option<Action> {
        if open {
            if self.next_read.is_none_or(|t| now >= t) {
                self.read_file();
                self.next_read = Some(now + READ_EVERY);
            }
        } else {
            // Opened again, the tab reads at once.
            self.next_read = None;
        }
        let p = self.pending?;
        let due = self.last_send.is_none_or(|t| now >= t + SEND_EVERY);
        due.then(|| self.send_level(p, now))
    }

    /// How long the loop may sleep before `poll` has work.
    pub fn next_wake(&self, now: Instant, open: bool) -> Option<Duration> {
        let send = self
            .pending
            .and(self.last_send)
            .map(|t| (t + SEND_EVERY).saturating_duration_since(now));
        let read = open.then(|| {
            self.next_read
                .map_or(Duration::ZERO, |t| t.saturating_duration_since(now))
        });
        let hold = (open && self.holding(now))
            .then(|| self.last_send.map(|t| (t + HOLD).saturating_duration_since(now)))
            .flatten();
        [send, read, hold].into_iter().flatten().min()
    }

    /// Log a failed send once per message.
    pub fn log_once(&mut self, msg: String) {
        if self.logged.insert(msg.clone()) {
            eprintln!("thinfloe-menu: {msg}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    const V40: Volume = Volume {
        percent: 40,
        muted: false,
    };

    #[test]
    fn parse_reads_the_line_and_ignores_trailing_fields() {
        assert_eq!(parse("40 0\n"), Some(V40));
        assert_eq!(
            parse("100 1 more fields\nsecond line"),
            Some(Volume {
                percent: 100,
                muted: true
            })
        );
        assert_eq!(
            parse("0 0"),
            Some(Volume {
                percent: 0,
                muted: false
            })
        );
        for bad in [
            "", "\n", "40", "40 2", "101 0", "-1 0", "x 0", "40 yes", "4.5 0", "1000 0", "\n40 0",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_file_is_read_when_the_tab_opens_and_then_once_a_second() {
        let d = TestDir::new("soundtab", "read");
        let p = d.join("thinfloe-volume");
        let mut tab = SoundTab::default();
        tab.set_path(Some(p.clone()));
        let t0 = Instant::now();
        // Missing: not known.
        assert_eq!(tab.poll(t0, true), None);
        assert_eq!(tab.shown(t0), None);
        std::fs::write(&p, "40 0\n").unwrap();
        tab.poll(t0 + Duration::from_millis(500), true);
        assert_eq!(tab.shown(t0), None, "not read again before a second");
        tab.poll(t0 + READ_EVERY, true);
        assert_eq!(tab.shown(t0), Some(V40));
        // Garbage is not known.
        std::fs::write(&p, "garbage\n").unwrap();
        tab.poll(t0 + READ_EVERY * 2, true);
        assert_eq!(tab.shown(t0), None);
        // Closed, nothing is read; opened again, at once.
        std::fs::write(&p, "40 0 extra\n").unwrap();
        tab.poll(t0 + READ_EVERY * 3, false);
        assert_eq!(tab.shown(t0), None);
        tab.poll(t0 + READ_EVERY * 3, true);
        assert_eq!(tab.shown(t0), Some(V40));
        // A link is not followed.
        let link = d.join("link");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        tab.set_path(Some(link));
        tab.poll(t0 + READ_EVERY * 4, true);
        assert_eq!(tab.shown(t0), None);
    }

    #[test]
    fn the_controls_are_off_with_the_switch_off_or_no_volume() {
        assert_eq!(controls(false, Some(V40)), (false, Some(REMOTE_NOTE)));
        assert_eq!(controls(false, None), (false, Some(REMOTE_NOTE)));
        assert_eq!(controls(true, None), (false, Some(UNKNOWN_NOTE)));
        assert_eq!(controls(true, Some(V40)), (true, None));
    }

    #[test]
    fn a_drag_sends_at_most_one_set_per_50_ms_and_the_last_on_release() {
        let mut tab = SoundTab::default();
        tab.set_file(Some(V40));
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let level = |percent, done| SoundEvent::Level { percent, done };
        assert_eq!(tab.event(level(41, false), ms(0)), Some(Action::VolumeSet(41)));
        assert_eq!(tab.event(level(42, false), ms(10)), None);
        assert_eq!(tab.event(level(43, false), ms(20)), None);
        assert_eq!(tab.shown(ms(20)).unwrap().percent, 43, "the own value shows");
        assert_eq!(tab.next_wake(ms(20), false), Some(Duration::from_millis(30)));
        // The held value goes out once 50 ms passed.
        assert_eq!(tab.poll(ms(40), false), None);
        assert_eq!(tab.poll(ms(50), false), Some(Action::VolumeSet(43)));
        assert_eq!(tab.poll(ms(60), false), None);
        assert_eq!(tab.event(level(44, false), ms(70)), None);
        // The release sends the final value at once.
        assert_eq!(tab.event(level(45, true), ms(75)), Some(Action::VolumeSet(45)));
        assert_eq!(tab.poll(ms(200), false), None, "nothing left");
        // A release on the value already sent sends nothing.
        assert_eq!(tab.event(level(45, true), ms(300)), None);
        // The file still says 40: the own value shows until 1 s after
        // the last send, then the file's.
        assert_eq!(tab.shown(ms(1000)).unwrap().percent, 45);
        assert_eq!(tab.shown(ms(1075)), Some(V40));
    }

    #[test]
    fn mute_sends_at_once_and_shows_its_own_value() {
        let mut tab = SoundTab::default();
        let t0 = Instant::now();
        // Not known: nothing to send.
        assert_eq!(tab.event(SoundEvent::Mute(true), t0), None);
        tab.set_file(Some(V40));
        assert_eq!(tab.event(SoundEvent::Mute(true), t0), Some(Action::VolumeMuteSet(true)));
        assert_eq!(
            tab.shown(t0),
            Some(Volume {
                percent: 40,
                muted: true
            })
        );
        assert_eq!(tab.shown(t0 + HOLD), Some(V40));
    }

    #[test]
    fn the_wake_covers_the_read_the_send_and_the_hold() {
        let mut tab = SoundTab::default();
        let t0 = Instant::now();
        assert_eq!(tab.next_wake(t0, false), None);
        assert_eq!(tab.next_wake(t0, true), Some(Duration::ZERO), "a first read is due");
        tab.poll(t0, true);
        assert_eq!(tab.next_wake(t0, true), Some(READ_EVERY));
        tab.set_file(Some(V40));
        tab.event(
            SoundEvent::Level {
                percent: 50,
                done: false,
            },
            t0,
        );
        tab.event(
            SoundEvent::Level {
                percent: 60,
                done: false,
            },
            t0,
        );
        assert_eq!(
            tab.next_wake(t0, false),
            Some(SEND_EVERY),
            "a held send counts on any tab"
        );
    }
}
