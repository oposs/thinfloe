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
/// Why the slider is off: the switch is on, the OSD still off.
pub const TURNING_ON_NOTE: &str = "Turning on…";
/// Why the slider is off: no volume file.
pub const UNKNOWN_NOTE: &str = "The volume is not known yet.";

/// The speaker state as the volume file says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume {
    pub percent: u8,
    pub muted: bool,
    /// The OSD acts on the volume switch (third field `on`). A file
    /// without the field, or with anything else there, counts as off.
    pub on: bool,
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

/// The volume file's line: `<percent> <muted> <on|off>`, percent 0 to
/// 100 and muted `0` or `1`; more fields after them are ignored. A
/// missing third field is off. Anything else is `None`.
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
    let on = words.next() == Some("on");
    Some(Volume { percent, muted, on })
}

/// Whether the slider and the mute switch take input, and if not, why.
pub fn controls(switch_on: bool, shown: Option<Volume>) -> (bool, Option<&'static str>) {
    match (switch_on, shown) {
        (false, _) => (false, Some(REMOTE_NOTE)),
        (true, None) => (false, Some(UNKNOWN_NOTE)),
        // The switch is on, but the OSD has not heard it yet (the
        // keyboard apply runs), or the apply failed.
        (true, Some(v)) if !v.on => (false, Some(TURNING_ON_NOTE)),
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
    /// When the file was last read; a read before the hold ended does
    /// not end the person's own value.
    read_at: Option<Instant>,
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
    /// or less than `HOLD` old, and until the file was read again after
    /// that; else the file's.
    pub fn shown(&self, now: Instant) -> Option<Volume> {
        if self.holding(now) {
            return self.own.or(self.file);
        }
        self.file
    }

    fn holding(&self, now: Instant) -> bool {
        self.pending.is_some()
            || self.last_send.is_some_and(|t| {
                // The file read before the hold ended may predate the
                // OSD's write: the own value stays until a read after it.
                now < t + HOLD || self.read_at.is_none_or(|r| r < t + HOLD)
            })
    }

    /// The file read at `now` (by `poll`, or a test).
    pub fn set_file(&mut self, v: Option<Volume>, now: Instant) {
        self.file = v;
        self.read_at = Some(now);
    }

    fn read_file(&mut self, now: Instant) {
        let Some(path) = &self.path else {
            // Nothing to read: the hold ends with its time.
            self.read_at = Some(now);
            return;
        };
        let v = crate::bounded::read_bounded(path, FILE_BYTES, false)
            .ok()
            .and_then(|b| parse(&String::from_utf8_lossy(&b)));
        self.set_file(v, now);
    }

    /// The next read: a second on, but no later than the end of the hold,
    /// so the file's value is fresh when the own value goes.
    fn schedule_read(&mut self, now: Instant) {
        let mut next = now + READ_EVERY;
        if let Some(end) = self.last_send.map(|t| t + HOLD)
            && end > now
        {
            next = next.min(end);
        }
        self.next_read = Some(next);
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
                self.sent(now);
                Some(Action::VolumeMuteSet(m))
            }
        }
    }

    fn send_level(&mut self, percent: u8, now: Instant) -> Action {
        self.pending = None;
        self.sent_level = Some(percent);
        self.sent(now);
        Action::VolumeSet(percent)
    }

    fn sent(&mut self, now: Instant) {
        self.last_send = Some(now);
        let end = now + HOLD;
        self.next_read = Some(self.next_read.map_or(end, |t| t.min(end)));
    }

    /// Called on every frame: reads the file when the open tab is due, and
    /// sends a value the throttle held back once it may.
    pub fn poll(&mut self, now: Instant, open: bool) -> Option<Action> {
        if open {
            if self.next_read.is_none_or(|t| now >= t) {
                self.read_file(now);
                self.schedule_read(now);
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
        // A send pulls the read in to the end of its hold, so the read
        // also ends the hold.
        let read = open.then(|| {
            self.next_read
                .map_or(Duration::ZERO, |t| t.saturating_duration_since(now))
        });
        [send, read].into_iter().flatten().min()
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
        on: true,
    };

    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    #[test]
    fn parse_reads_the_line_and_ignores_trailing_fields() {
        assert_eq!(parse("40 0 on\n"), Some(V40));
        assert_eq!(
            parse("100 1 on more fields\nsecond line"),
            Some(Volume {
                percent: 100,
                muted: true,
                on: true,
            })
        );
        // No mode, or not "on": the OSD does not act on the switch (yet).
        for off in ["40 0", "40 0 off", "40 0 On", "40 0 yes\n"] {
            assert_eq!(parse(off), Some(Volume { on: false, ..V40 }), "{off:?}");
        }
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
        std::fs::write(&p, "40 0 on\n").unwrap();
        tab.poll(ms(t0, 500), true);
        assert_eq!(tab.shown(t0), None, "not read again before a second");
        tab.poll(t0 + READ_EVERY, true);
        assert_eq!(tab.shown(t0), Some(V40));
        // Garbage is not known.
        std::fs::write(&p, "garbage\n").unwrap();
        tab.poll(t0 + READ_EVERY * 2, true);
        assert_eq!(tab.shown(t0), None);
        // Closed, nothing is read; opened again, at once.
        std::fs::write(&p, "40 0 on extra\n").unwrap();
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
    fn the_controls_need_the_switch_and_the_osd_on() {
        assert_eq!(controls(false, Some(V40)), (false, Some(REMOTE_NOTE)));
        assert_eq!(controls(false, None), (false, Some(REMOTE_NOTE)));
        assert_eq!(controls(true, None), (false, Some(UNKNOWN_NOTE)));
        // Switched on, the OSD not yet (the apply runs, or failed).
        assert_eq!(
            controls(true, Some(Volume { on: false, ..V40 })),
            (false, Some(TURNING_ON_NOTE))
        );
        assert_eq!(controls(true, Some(V40)), (true, None));
    }

    #[test]
    fn a_drag_sends_at_most_one_set_per_50_ms_and_the_last_on_release() {
        let mut tab = SoundTab::default();
        let t0 = Instant::now();
        tab.set_file(Some(V40), t0);
        let level = |percent, done| SoundEvent::Level { percent, done };
        assert_eq!(tab.event(level(41, false), ms(t0, 0)), Some(Action::VolumeSet(41)));
        assert_eq!(tab.event(level(42, false), ms(t0, 10)), None);
        assert_eq!(tab.event(level(43, false), ms(t0, 20)), None);
        assert_eq!(tab.shown(ms(t0, 20)).unwrap().percent, 43, "the own value shows");
        assert_eq!(tab.next_wake(ms(t0, 20), false), Some(Duration::from_millis(30)));
        // The held value goes out once 50 ms passed.
        assert_eq!(tab.poll(ms(t0, 40), false), None);
        assert_eq!(tab.poll(ms(t0, 50), false), Some(Action::VolumeSet(43)));
        assert_eq!(tab.poll(ms(t0, 60), false), None);
        assert_eq!(tab.event(level(44, false), ms(t0, 70)), None);
        // The release sends the final value at once.
        assert_eq!(tab.event(level(45, true), ms(t0, 75)), Some(Action::VolumeSet(45)));
        assert_eq!(tab.poll(ms(t0, 200), false), None, "nothing left");
        // A release on the value already sent sends nothing.
        assert_eq!(tab.event(level(45, true), ms(t0, 300)), None);
        // The own value shows for 1 s after the last send, and after that
        // until the file was read again.
        assert_eq!(tab.shown(ms(t0, 1000)).unwrap().percent, 45);
        assert_eq!(tab.shown(ms(t0, 1075)).unwrap().percent, 45, "the read is from before");
        tab.set_file(Some(V40), ms(t0, 1075));
        assert_eq!(tab.shown(ms(t0, 1075)), Some(V40));
    }

    #[test]
    fn the_hold_ends_with_a_fresh_read_not_an_old_one() {
        let d = TestDir::new("soundtab", "hold");
        let p = d.join("thinfloe-volume");
        std::fs::write(&p, "40 0 on\n").unwrap();
        let mut tab = SoundTab::default();
        tab.set_path(Some(p.clone()));
        let t0 = Instant::now();
        tab.poll(t0, true);
        let release = SoundEvent::Level {
            percent: 50,
            done: true,
        };
        assert_eq!(tab.event(release, ms(t0, 500)), Some(Action::VolumeSet(50)));
        // The read of its own clock, before the OSD wrote: still 50 shown.
        tab.poll(ms(t0, 1000), true);
        assert_eq!(tab.shown(ms(t0, 1000)).unwrap().percent, 50);
        // The next read is due when the hold ends, not a second later.
        assert_eq!(tab.next_wake(ms(t0, 1000), true), Some(Duration::from_millis(500)));
        // At the hold's end, before that read: no jump back to 40.
        assert_eq!(tab.shown(ms(t0, 1500)).unwrap().percent, 50);
        std::fs::write(&p, "50 0 on\n").unwrap();
        tab.poll(ms(t0, 1500), true);
        assert_eq!(tab.shown(ms(t0, 1500)), Some(Volume { percent: 50, ..V40 }));
        // From the file now: a change elsewhere shows at the next read.
        std::fs::write(&p, "30 0 on\n").unwrap();
        tab.poll(ms(t0, 2500), true);
        assert_eq!(tab.shown(ms(t0, 2500)).unwrap().percent, 30);
    }

    #[test]
    fn mute_sends_at_once_and_shows_its_own_value() {
        let mut tab = SoundTab::default();
        let t0 = Instant::now();
        // Not known: nothing to send.
        assert_eq!(tab.event(SoundEvent::Mute(true), t0), None);
        tab.set_file(Some(V40), t0);
        assert_eq!(tab.event(SoundEvent::Mute(true), t0), Some(Action::VolumeMuteSet(true)));
        assert_eq!(tab.shown(t0), Some(Volume { muted: true, ..V40 }));
        tab.set_file(Some(V40), t0 + HOLD);
        assert_eq!(tab.shown(t0 + HOLD), Some(V40));
    }

    #[test]
    fn the_wake_covers_the_read_and_the_send() {
        let mut tab = SoundTab::default();
        let t0 = Instant::now();
        assert_eq!(tab.next_wake(t0, false), None);
        assert_eq!(tab.next_wake(t0, true), Some(Duration::ZERO), "a first read is due");
        tab.poll(t0, true);
        assert_eq!(tab.next_wake(t0, true), Some(READ_EVERY));
        tab.set_file(Some(V40), t0);
        for percent in [50, 60] {
            tab.event(SoundEvent::Level { percent, done: false }, t0);
        }
        assert_eq!(
            tab.next_wake(t0, false),
            Some(SEND_EVERY),
            "a held send counts on any tab"
        );
        // Without a file path the hold still ends with its time.
        tab.poll(t0 + SEND_EVERY, true);
        tab.poll(t0 + HOLD + SEND_EVERY, true);
        assert_eq!(tab.shown(t0 + HOLD + SEND_EVERY), Some(V40));
    }
}
