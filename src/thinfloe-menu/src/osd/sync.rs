//! The shared audio state of the volume keys over every speaker (sink)
//! and microphone (source): level `L`, speaker mute `M`, microphone mute
//! `Q`. Pure: each call says which device to set to what (`Set`); the
//! audio worker runs the sets through `pactl`.
//!
//! The state is read from the default devices once, at start; after that
//! it changes only by a key or a mode change. A device that becomes the
//! default later (a headset at 100 %) gets the shared state, it does not
//! give it.

use super::audio::{Device, Kind, Level};

/// Whether the volume keys act on this machine (`THINFLOE_VOLUME_KEYS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Every device follows the shared state.
    Keys,
    /// Every device at 100 % and unmuted; the remote client sets the
    /// loudness.
    Off,
}

/// The switch in the generated environment (`THINFLOE_VOLUME_KEYS`), by
/// the composer's rule: a line starting with `#` is skipped, key and value
/// are trimmed of blanks, the last assignment wins, and `on`, `"on"` or
/// `'on'` is on. Anything else, or no such line, is off.
pub fn keys_mode(env_text: &str) -> Mode {
    let mut last = "";
    for line in env_text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim_matches([' ', '\t', '\r']) == "THINFLOE_VOLUME_KEYS" {
            last = v.trim_matches([' ', '\t', '\r']);
        }
    }
    match last {
        "on" | "\"on\"" | "'on'" => Mode::Keys,
        _ => Mode::Off,
    }
}

/// One `pactl` set on one device, by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Set {
    Volume(Kind, String, u8),
    Mute(Kind, String, bool),
}

/// Full volume, the level of every device with the switch off.
const FULL: u8 = 100;

#[derive(Clone, Debug)]
pub struct Shared {
    mode: Mode,
    level: u8,
    sink_muted: bool,
    mic_muted: bool,
}

impl Shared {
    /// The shared state, and what brings every device in line with it.
    /// `Keys` takes `L` and `M` from the default sink and `Q` from the
    /// default source (100 % and unmuted for a default not in the list);
    /// `Off` is 100 % and unmuted.
    pub fn start(
        mode: Mode,
        sinks: &[Device],
        sources: &[Device],
        default_sink: Option<&str>,
        default_source: Option<&str>,
    ) -> (Shared, Vec<Set>) {
        let find = |list: &[Device], name: Option<&str>| name.and_then(|n| list.iter().find(|d| d.name == n)).cloned();
        let mut s = Shared {
            mode,
            level: FULL,
            sink_muted: false,
            mic_muted: false,
        };
        if mode == Mode::Keys {
            if let Some(d) = find(sinks, default_sink) {
                s.level = d.percent.min(FULL);
                s.sink_muted = d.muted;
            }
            if let Some(d) = find(sources, default_source) {
                s.mic_muted = d.muted;
            }
        }
        let sets = s.reapply(sinks, sources);
        (s, sets)
    }

    /// A volume key: `L` moves by `delta` within 0..=100 and every sink
    /// gets it; the mute stays as it is.
    pub fn step(&mut self, delta: i32, sinks: &[Device]) -> Vec<Set> {
        self.level = (i32::from(self.level).saturating_add(delta)).clamp(0, i32::from(FULL)) as u8;
        sinks.iter().flat_map(|d| volume(d, self.level)).collect()
    }

    /// The mute key: `M` flips and every sink gets it.
    pub fn toggle_mute(&mut self, sinks: &[Device]) -> Vec<Set> {
        self.sink_muted = !self.sink_muted;
        sinks.iter().flat_map(|d| mute(d, self.sink_muted)).collect()
    }

    /// The mic mute key: `Q` flips and every source gets it.
    pub fn toggle_mic(&mut self, sources: &[Device]) -> Vec<Set> {
        self.mic_muted = !self.mic_muted;
        sources.iter().flat_map(|d| mute(d, self.mic_muted)).collect()
    }

    /// A device that just appeared joins the shared state.
    pub fn on_new(&self, d: &Device) -> Vec<Set> {
        match (self.mode, d.kind) {
            (Mode::Keys, Kind::Sink) => [volume(d, self.level), mute(d, self.sink_muted)].concat(),
            // A source's volume is not the keys' business.
            (Mode::Keys, Kind::Source) => mute(d, self.mic_muted),
            (Mode::Off, _) => [volume(d, FULL), mute(d, false)].concat(),
        }
    }

    /// The switch changed. To `Off`, the state becomes 100 % and unmuted;
    /// back to `Keys`, it keeps what it is. Every device follows.
    pub fn set_mode(&mut self, mode: Mode, sinks: &[Device], sources: &[Device]) -> Vec<Set> {
        self.mode = mode;
        if mode == Mode::Off {
            self.level = FULL;
            self.sink_muted = false;
            self.mic_muted = false;
        }
        self.reapply(sinks, sources)
    }

    /// Every device in line with the shared state again.
    pub fn reapply(&self, sinks: &[Device], sources: &[Device]) -> Vec<Set> {
        sinks.iter().chain(sources).flat_map(|d| self.on_new(d)).collect()
    }

    pub fn level(&self) -> Level {
        Level {
            percent: self.level,
            muted: self.sink_muted,
        }
    }

    pub fn mic_muted(&self) -> bool {
        self.mic_muted
    }
}

/// A volume set for `d`, unless it is at `percent` already.
fn volume(d: &Device, percent: u8) -> Vec<Set> {
    if d.percent == percent {
        return Vec::new();
    }
    vec![Set::Volume(d.kind, d.name.clone(), percent)]
}

/// A mute set for `d`, unless it is in that state already.
fn mute(d: &Device, muted: bool) -> Vec<Set> {
    if d.muted == muted {
        return Vec::new();
    }
    vec![Set::Mute(d.kind, d.name.clone(), muted)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osd::audio::{Device, Kind, Level};

    fn dev(kind: Kind, index: u32, name: &str, percent: u8, muted: bool) -> Device {
        Device {
            kind,
            index,
            name: name.into(),
            percent,
            muted,
        }
    }

    fn sink(index: u32, name: &str, percent: u8, muted: bool) -> Device {
        dev(Kind::Sink, index, name, percent, muted)
    }

    fn source(index: u32, name: &str, percent: u8, muted: bool) -> Device {
        dev(Kind::Source, index, name, percent, muted)
    }

    fn vol(kind: Kind, name: &str, p: u8) -> Set {
        Set::Volume(kind, name.into(), p)
    }

    fn mute(kind: Kind, name: &str, m: bool) -> Set {
        Set::Mute(kind, name.into(), m)
    }

    #[test]
    fn start_takes_the_default_and_aligns_the_rest() {
        let sinks = [sink(1, "a", 40, false), sink(2, "b", 100, false)];
        let sources = [source(3, "mic", 100, false)];
        let (s, sets) = Shared::start(Mode::Keys, &sinks, &sources, Some("a"), Some("mic"));
        assert_eq!(
            s.level(),
            Level {
                percent: 40,
                muted: false
            }
        );
        assert!(!s.mic_muted());
        assert_eq!(sets, [vol(Kind::Sink, "b", 40)]);

        // The default's mute and the default source's mute are shared too.
        let sinks = [sink(1, "a", 40, true), sink(2, "b", 40, false)];
        let sources = [source(3, "mic", 100, true), source(4, "cam", 70, false)];
        let (s, sets) = Shared::start(Mode::Keys, &sinks, &sources, Some("a"), Some("mic"));
        assert!(s.level().muted);
        assert!(s.mic_muted());
        // A source's volume is left alone while the switch is on.
        assert_eq!(sets, [mute(Kind::Sink, "b", true), mute(Kind::Source, "cam", true)]);

        // No default: full and unmuted.
        let (s, _) = Shared::start(Mode::Keys, &sinks, &[], None, None);
        assert_eq!(
            s.level(),
            Level {
                percent: 100,
                muted: false
            }
        );
        assert!(!s.mic_muted());
        // A default that is not listed (a monitor source) counts as none.
        let (s, _) = Shared::start(Mode::Keys, &sinks, &sources, Some("gone"), Some("a.monitor"));
        assert_eq!(s.level().percent, 100);
        assert!(!s.mic_muted());
    }

    #[test]
    fn off_resets_every_device_to_full_and_unmuted() {
        let sinks = [sink(1, "a", 40, true), sink(2, "b", 100, false)];
        let sources = [source(3, "mic", 100, true), source(4, "cam", 30, true)];
        let (s, sets) = Shared::start(Mode::Off, &sinks, &sources, Some("a"), Some("mic"));
        assert_eq!(
            s.level(),
            Level {
                percent: 100,
                muted: false
            }
        );
        assert!(!s.mic_muted());
        assert_eq!(
            sets,
            [
                vol(Kind::Sink, "a", 100),
                mute(Kind::Sink, "a", false),
                mute(Kind::Source, "mic", false),
                vol(Kind::Source, "cam", 100),
                mute(Kind::Source, "cam", false),
            ]
        );
    }

    #[test]
    fn step_moves_every_sink_from_the_shared_level() {
        let sinks = [sink(1, "a", 40, false), sink(2, "b", 70, false)];
        let (mut s, _) = Shared::start(Mode::Keys, &sinks, &[], Some("a"), None);
        assert_eq!(s.step(5, &sinks), [vol(Kind::Sink, "a", 45), vol(Kind::Sink, "b", 45)]);
        assert_eq!(s.level().percent, 45);
        // A sink already at the level gets no set.
        let sinks = [sink(1, "a", 45, false), sink(2, "b", 70, false)];
        assert_eq!(s.step(0, &sinks), [vol(Kind::Sink, "b", 45)]);

        let sinks = [sink(1, "a", 98, false)];
        let (mut s, _) = Shared::start(Mode::Keys, &sinks, &[], Some("a"), None);
        assert_eq!(s.step(5, &sinks), [vol(Kind::Sink, "a", 100)]);
        assert_eq!(s.step(5, &[sink(1, "a", 100, false)]), []);
        assert_eq!(s.level().percent, 100);

        let sinks = [sink(1, "a", 3, true)];
        let (mut s, _) = Shared::start(Mode::Keys, &sinks, &[], Some("a"), None);
        assert_eq!(s.step(-5, &sinks), [vol(Kind::Sink, "a", 0)]);
        // The mute stays as it is.
        assert_eq!(
            s.level(),
            Level {
                percent: 0,
                muted: true
            }
        );
    }

    #[test]
    fn toggle_mute_sets_every_sink() {
        let sinks = [sink(1, "a", 40, false), sink(2, "b", 40, true)];
        let (mut s, _) = Shared::start(Mode::Keys, &[sinks[0].clone()], &[], Some("a"), None);
        assert_eq!(s.toggle_mute(&sinks), [mute(Kind::Sink, "a", true)]);
        assert!(s.level().muted);
        assert_eq!(
            s.toggle_mute(&sinks),
            [mute(Kind::Sink, "b", false)],
            "the second press unmutes"
        );
        assert!(!s.level().muted);
    }

    #[test]
    fn toggle_mic_sets_every_source() {
        let sources = [source(3, "mic", 100, false), source(4, "cam", 100, true)];
        let (mut s, _) = Shared::start(Mode::Keys, &[], &[sources[0].clone()], None, Some("mic"));
        assert!(!s.mic_muted());
        assert_eq!(s.toggle_mic(&sources), [mute(Kind::Source, "mic", true)]);
        assert!(s.mic_muted());
    }

    #[test]
    fn a_new_default_device_does_not_become_the_shared_level() {
        let sinks = [sink(1, "speaker", 30, false)];
        let sources = [source(3, "cam", 100, false)];
        let (mut s, _) = Shared::start(Mode::Keys, &sinks, &sources, Some("speaker"), Some("cam"));
        assert_eq!(s.toggle_mic(&sources), [mute(Kind::Source, "cam", true)]);
        // A USB headset: a sink and a source in one burst, both the new
        // defaults, at WirePlumber's 100 %.
        let headset = sink(7, "headset", 100, false);
        assert_eq!(s.on_new(&headset), [vol(Kind::Sink, "headset", 30)]);
        assert_eq!(s.level().percent, 30);
        let mic = source(8, "mic", 100, false);
        assert_eq!(s.on_new(&mic), [mute(Kind::Source, "mic", true)]);
        assert!(s.mic_muted());
        // A key afterwards moves from the shared level, not the headset's.
        let sinks = [sink(1, "speaker", 30, false), sink(7, "headset", 30, false)];
        assert_eq!(
            s.step(5, &sinks),
            [vol(Kind::Sink, "speaker", 35), vol(Kind::Sink, "headset", 35)]
        );
    }

    #[test]
    fn on_new_in_off_mode_gives_full_and_unmuted() {
        let (s, _) = Shared::start(Mode::Off, &[], &[], None, None);
        assert_eq!(
            s.on_new(&sink(7, "headset", 40, true)),
            [vol(Kind::Sink, "headset", 100), mute(Kind::Sink, "headset", false)]
        );
        assert_eq!(
            s.on_new(&source(8, "mic", 60, true)),
            [vol(Kind::Source, "mic", 100), mute(Kind::Source, "mic", false)]
        );
        assert_eq!(s.on_new(&source(9, "cam", 100, false)), []);
    }

    #[test]
    fn set_mode_off_resets_and_back_to_keys_keeps_full() {
        let sinks = [sink(1, "a", 40, true)];
        let sources = [source(3, "mic", 70, true)];
        let (mut s, _) = Shared::start(Mode::Keys, &sinks, &sources, Some("a"), Some("mic"));
        assert_eq!(
            s.set_mode(Mode::Off, &sinks, &sources),
            [
                vol(Kind::Sink, "a", 100),
                mute(Kind::Sink, "a", false),
                vol(Kind::Source, "mic", 100),
                mute(Kind::Source, "mic", false),
            ]
        );
        let sinks = [sink(1, "a", 100, false)];
        let sources = [source(3, "mic", 100, false)];
        assert_eq!(s.set_mode(Mode::Keys, &sinks, &sources), []);
        assert_eq!(
            s.level(),
            Level {
                percent: 100,
                muted: false
            }
        );
        assert!(!s.mic_muted());
    }

    #[test]
    fn keys_mode_reads_the_switch() {
        for on in ["on", "\"on\"", "'on'", " on \r"] {
            assert_eq!(keys_mode(&format!("THINFLOE_VOLUME_KEYS={on}\n")), Mode::Keys, "{on:?}");
        }
        for off in ["off", "yes", "", "On", "\"on"] {
            assert_eq!(
                keys_mode(&format!("THINFLOE_VOLUME_KEYS={off}\n")),
                Mode::Off,
                "{off:?}"
            );
        }
        assert_eq!(keys_mode(""), Mode::Off, "missing");
        assert_eq!(keys_mode("THINFLOE_BRIGHTNESS_KEYS=on\n"), Mode::Off);
        assert_eq!(keys_mode("# THINFLOE_VOLUME_KEYS=on\n"), Mode::Off, "a comment");
        assert_eq!(keys_mode("XTHINFLOE_VOLUME_KEYS=on\n"), Mode::Off);
        // The last assignment wins, as in the composer.
        assert_eq!(
            keys_mode("THINFLOE_VOLUME_KEYS=on\nTHINFLOE_VOLUME_KEYS=off\n"),
            Mode::Off
        );
        assert_eq!(
            keys_mode("THINFLOE_VOLUME_KEYS=off\n THINFLOE_VOLUME_KEYS = on\n"),
            Mode::Keys
        );
    }

    #[test]
    fn reapply_brings_every_device_back_to_the_shared_state() {
        let sinks = [sink(1, "a", 40, false)];
        let (s, _) = Shared::start(Mode::Keys, &sinks, &[], Some("a"), None);
        // Someone else moved a sink and muted a source.
        let sinks = [sink(1, "a", 80, true)];
        let sources = [source(3, "mic", 50, true)];
        assert_eq!(
            s.reapply(&sinks, &sources),
            [
                vol(Kind::Sink, "a", 40),
                mute(Kind::Sink, "a", false),
                mute(Kind::Source, "mic", false),
            ]
        );
    }
}
