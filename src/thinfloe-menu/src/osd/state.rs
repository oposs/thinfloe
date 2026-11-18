//! What the OSD shows, apart from the drawing: the panel of the last key
//! until it times out, the microphone mark and each monitor's brightness.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::audio::Level;
use super::ddc::Bright;
use super::draw::PanelView;

/// What the last key changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Sink(Level),
    /// Whether the microphone is now muted.
    Mic(bool),
    /// Each output shows its own monitor's brightness.
    Brightness,
}

#[derive(Default)]
pub struct OsdState {
    /// What the panel shows and since when.
    panel: Option<(Shown, Instant)>,
    mic_muted: bool,
    /// By connector name, as the outputs are named (`DP-1`).
    bright: BTreeMap<String, Bright>,
}

impl OsdState {
    pub const HIDE_AFTER: Duration = Duration::from_millis(1500);

    pub fn new() -> OsdState {
        OsdState::default()
    }

    /// Show `s` on the panel from `now` on; an earlier panel is replaced.
    pub fn show(&mut self, s: Shown, now: Instant) {
        self.panel = Some((s, now));
    }

    pub fn set_bright(&mut self, connector: &str, b: Bright) {
        self.bright.insert(connector.to_string(), b);
    }

    /// Record the default source's mute state; true when it changed.
    pub fn set_mic(&mut self, muted: bool) -> bool {
        std::mem::replace(&mut self.mic_muted, muted) != muted
    }

    /// The panel `output` shows at `now`, if any.
    pub fn panel_for(&self, output: &str, now: Instant) -> Option<PanelView> {
        let (s, since) = self.panel?;
        if now >= since + Self::HIDE_AFTER {
            return None;
        }
        Some(match s {
            Shown::Sink(l) if l.muted => PanelView::Muted { percent: l.percent },
            Shown::Sink(l) => PanelView::Volume { percent: l.percent },
            Shown::Mic(true) => PanelView::MicOff,
            Shown::Mic(false) => PanelView::MicOn,
            Shown::Brightness => match self.bright.get(output) {
                Some(Bright::Percent(p)) => PanelView::Brightness { percent: *p },
                Some(Bright::Unsupported) => PanelView::BrightnessUnsupported,
                Some(Bright::Unknown) | None => PanelView::BrightnessUnknown,
            },
        })
    }

    /// Whether the muted-microphone mark shows (on every output).
    pub fn mark(&self) -> bool {
        self.mic_muted
    }

    /// When the panel goes away, while one shows.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.panel.map(|(_, since)| since + Self::HIDE_AFTER)
    }

    /// Drop the panel once its time is up; true when it went away now.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.next_deadline().is_some_and(|d| now >= d) {
            self.panel = None;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_hides_1500_ms_after_the_last_key() {
        let t0 = Instant::now();
        let mut s = OsdState::new();
        let l = Level {
            percent: 40,
            muted: false,
        };
        s.show(Shown::Sink(l), t0);
        s.show(Shown::Sink(l), t0 + Duration::from_secs(1));
        assert_eq!(
            s.panel_for("DP-1", t0 + Duration::from_millis(2400)),
            Some(PanelView::Volume { percent: 40 })
        );
        assert_eq!(s.panel_for("DP-1", t0 + Duration::from_millis(2500)), None);
        assert!(!s.expire(t0 + Duration::from_millis(2400)));
        assert!(s.expire(t0 + Duration::from_millis(2500)));
        assert_eq!(s.next_deadline(), None);
        assert!(!s.expire(t0 + Duration::from_millis(2600)));
    }

    #[test]
    fn muted_sink_shows_muted_with_the_level() {
        let t0 = Instant::now();
        let mut s = OsdState::new();
        s.show(
            Shown::Sink(Level {
                percent: 45,
                muted: true,
            }),
            t0,
        );
        assert_eq!(s.panel_for("DP-1", t0), Some(PanelView::Muted { percent: 45 }));
    }

    #[test]
    fn the_mic_panel_says_off_or_on() {
        let t0 = Instant::now();
        let mut s = OsdState::new();
        s.show(Shown::Mic(true), t0);
        assert_eq!(s.panel_for("DP-1", t0), Some(PanelView::MicOff));
        s.show(Shown::Mic(false), t0);
        assert_eq!(s.panel_for("DP-1", t0), Some(PanelView::MicOn));
    }

    #[test]
    fn brightness_is_per_output() {
        let t0 = Instant::now();
        let mut s = OsdState::new();
        s.set_bright("DP-1", Bright::Percent(70));
        s.set_bright("HDMI-A-1", Bright::Unsupported);
        s.set_bright("DP-3", Bright::Unknown);
        s.show(Shown::Brightness, t0);
        assert_eq!(s.panel_for("DP-1", t0), Some(PanelView::Brightness { percent: 70 }));
        assert_eq!(s.panel_for("HDMI-A-1", t0), Some(PanelView::BrightnessUnsupported));
        assert_eq!(s.panel_for("DP-2", t0), Some(PanelView::BrightnessUnknown));
        assert_eq!(s.panel_for("DP-3", t0), Some(PanelView::BrightnessUnknown));
    }

    #[test]
    fn the_mark_follows_the_source_mute() {
        let mut s = OsdState::new();
        assert!(!s.mark());
        assert!(s.set_mic(true));
        assert!(s.mark());
        assert!(!s.set_mic(true));
        assert!(s.set_mic(false));
        assert!(!s.mark());
    }
}
