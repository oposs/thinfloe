//! The Network tab's Time band as a state machine: the zone selects and
//! the NTP servers field. Saved at once, like the public IP switch: a
//! choice becomes a `TimeCfg` the menu writes into `[time]` and hands to
//! `thinfloe-time apply`. Nothing here waits for the network files or an
//! Apply; drawing is in network_ui.rs.

use std::sync::Arc;

use crate::config::TimeCfg;
use crate::jobs::TimeSaveFail;
use crate::timecfg::{self, Zones};

/// The first two entries of the region select.
pub const AUTOMATIC: &str = "Automatic";
const FIRST_REGION: usize = 2;

/// What the band reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeEvent {
    /// The region select: 0 Automatic, 1 UTC, then the regions.
    Region(usize),
    /// The city select of the region shown.
    City(usize),
    /// The NTP servers field as typed; `commit`: it lost focus or took
    /// Enter.
    NtpText { text: String, commit: bool },
    /// Escape in the field: the saved servers are shown again.
    NtpCancel,
}

#[derive(Debug, Default)]
pub struct TimeBand {
    zones: Arc<Zones>,
    /// The setting on screen: the last saved one, or the one a running or
    /// queued save writes.
    cfg: TimeCfg,
    /// What `config.toml` holds; a failed save goes back to it.
    stored: TimeCfg,
    /// A region picked whose city is not chosen yet.
    region_pick: Option<usize>,
    ntp_text: String,
    error: Option<String>,
    busy: bool,
}

impl TimeBand {
    /// The zones the selects offer.
    pub fn set_zones(&mut self, zones: Zones) {
        self.zones = Arc::new(zones);
    }

    /// The setting as `config.toml` holds it.
    pub fn set_cfg(&mut self, cfg: TimeCfg) {
        self.ntp_text = timecfg::servers_text(&cfg.ntp);
        self.stored = cfg.clone();
        self.cfg = cfg;
    }

    pub fn cfg(&self) -> &TimeCfg {
        &self.cfg
    }

    /// The region select's entries.
    pub fn regions(&self) -> Vec<&str> {
        let mut v = vec![AUTOMATIC, timecfg::UTC];
        v.extend(self.zones.regions.iter().map(String::as_str));
        v
    }

    /// The region select's choice.
    pub fn region_index(&self) -> usize {
        if let Some(r) = self.region_pick {
            return r;
        }
        match self.cfg.zone.as_deref() {
            None => 0,
            Some(timecfg::UTC) => 1,
            Some(z) => self.zones.region_of(z).map_or(0, |(r, _)| r + FIRST_REGION),
        }
    }

    fn region_name(&self) -> Option<&str> {
        let i = self.region_index().checked_sub(FIRST_REGION)?;
        self.zones.regions.get(i).map(String::as_str)
    }

    /// The city select's entries; empty when no region is chosen.
    pub fn cities(&self) -> Vec<&str> {
        self.region_name()
            .and_then(|r| self.zones.cities.get(r))
            .map(|c| c.iter().map(|(_, label)| label.as_str()).collect())
            .unwrap_or_default()
    }

    /// The city chosen, `None` while a region waits for its city.
    pub fn city_index(&self) -> Option<usize> {
        if self.region_pick.is_some() {
            return None;
        }
        let z = self.cfg.zone.as_deref()?;
        self.zones.region_of(z).map(|(_, c)| c)
    }

    pub fn ntp_text(&self) -> &str {
        &self.ntp_text
    }

    /// Why the typed servers are not used, if they are not.
    pub fn ntp_refused(&self) -> Option<String> {
        timecfg::parse_servers(&self.ntp_text).err()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// A save or its apply runs; the band takes no clicks.
    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    fn save(&mut self, cfg: TimeCfg) -> Option<TimeCfg> {
        if cfg == self.cfg {
            return None;
        }
        self.cfg = cfg.clone();
        Some(cfg)
    }

    /// One change; `Some` is the setting to save now.
    pub fn event(&mut self, e: TimeEvent) -> Option<TimeCfg> {
        match e {
            TimeEvent::Region(i) => {
                let zone = match i {
                    0 => None,
                    1 => Some(timecfg::UTC.to_string()),
                    i if i - FIRST_REGION < self.zones.regions.len() => {
                        // The zone stays until a city of the new region is
                        // chosen; the region the zone is in shows its city.
                        let of_zone = self.cfg.zone.as_deref().and_then(|z| self.zones.region_of(z));
                        self.region_pick = (of_zone.map(|(r, _)| r + FIRST_REGION) != Some(i)).then_some(i);
                        return None;
                    }
                    _ => return None,
                };
                self.region_pick = None;
                self.save(TimeCfg {
                    zone,
                    ..self.cfg.clone()
                })
            }
            TimeEvent::City(c) => {
                let region = self.region_name()?.to_string();
                let zone = self.zones.cities.get(&region)?.get(c)?.0.clone();
                self.region_pick = None;
                self.save(TimeCfg {
                    zone: Some(zone),
                    ..self.cfg.clone()
                })
            }
            TimeEvent::NtpCancel => {
                self.ntp_text = timecfg::servers_text(&self.cfg.ntp);
                None
            }
            TimeEvent::NtpText { text, commit } => {
                self.ntp_text = text;
                if !commit {
                    return None;
                }
                let ntp = timecfg::parse_servers(&self.ntp_text).ok()?;
                self.save(TimeCfg {
                    ntp,
                    ..self.cfg.clone()
                })
            }
        }
    }

    /// The save of `cfg` ended. A setting that was not written goes back
    /// to what `config.toml` holds; one written but not installed stays,
    /// with the reason: the initramfs installs it at the next start.
    pub fn saved(&mut self, cfg: &TimeCfg, r: Result<(), TimeSaveFail>) {
        match r {
            Ok(()) => {
                self.stored = cfg.clone();
                self.error = None;
            }
            Err(TimeSaveFail::NotApplied(e)) => {
                self.stored = cfg.clone();
                self.error = Some(format!("Saved, in effect after a restart: {e}"));
            }
            Err(TimeSaveFail::NotSaved(e)) => {
                // Servers typed after the failed commit stay in the field.
                if self.ntp_text == timecfg::servers_text(&cfg.ntp) {
                    self.ntp_text = timecfg::servers_text(&self.stored.ntp);
                }
                self.cfg = self.stored.clone();
                self.region_pick = None;
                self.error = Some(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band() -> TimeBand {
        let mut b = TimeBand::default();
        b.set_zones(timecfg::parse_zone_tab(
            "CH\t+0\tEurope/Zurich\nDE\t+0\tEurope/Berlin\nJP\t+0\tAsia/Tokyo\n",
        ));
        b
    }

    fn zone(z: &str) -> TimeCfg {
        TimeCfg {
            zone: Some(z.into()),
            ntp: Vec::new(),
        }
    }

    #[test]
    fn regions_start_with_automatic_and_utc() {
        let b = band();
        assert_eq!(b.regions(), ["Automatic", "UTC", "Asia", "Europe"]);
        assert_eq!(b.region_index(), 0);
        assert!(b.cities().is_empty());
    }

    #[test]
    fn automatic_saves_at_once() {
        let mut b = band();
        b.set_cfg(zone("Europe/Zurich"));
        assert_eq!(b.event(TimeEvent::Region(0)), Some(TimeCfg::default()));
        assert_eq!(b.event(TimeEvent::Region(0)), None);
    }

    #[test]
    fn utc_saves_at_once() {
        let mut b = band();
        assert_eq!(b.event(TimeEvent::Region(1)), Some(zone("UTC")));
        assert_eq!(b.region_index(), 1);
    }

    #[test]
    fn region_alone_saves_nothing() {
        let mut b = band();
        assert_eq!(b.event(TimeEvent::Region(3)), None);
        assert_eq!(b.region_index(), 3);
        assert_eq!(b.cities(), ["Berlin", "Zurich"]);
        assert_eq!(b.city_index(), None);
        assert_eq!(b.cfg().zone, None);
    }

    #[test]
    fn city_saves_the_zone() {
        let mut b = band();
        b.event(TimeEvent::Region(3));
        assert_eq!(b.event(TimeEvent::City(1)), Some(zone("Europe/Zurich")));
        assert_eq!(b.region_index(), 3);
        assert_eq!(b.city_index(), Some(1));
        // Picking the region the zone is in keeps the city.
        assert_eq!(b.event(TimeEvent::Region(3)), None);
        assert_eq!(b.city_index(), Some(1));
        // Another region waits for its city; the zone stays.
        assert_eq!(b.event(TimeEvent::Region(2)), None);
        assert_eq!(b.city_index(), None);
        assert_eq!(b.cfg().zone.as_deref(), Some("Europe/Zurich"));
        assert_eq!(b.event(TimeEvent::City(0)), Some(zone("Asia/Tokyo")));
    }

    #[test]
    fn ntp_commit_saves() {
        let mut b = band();
        let typed = |t: &str, commit| TimeEvent::NtpText { text: t.into(), commit };
        assert_eq!(b.event(typed("ntp.example.ch, 10.0.0.1", false)), None);
        let want = TimeCfg {
            zone: None,
            ntp: vec!["ntp.example.ch".into(), "10.0.0.1".into()],
        };
        assert_eq!(b.event(typed("ntp.example.ch, 10.0.0.1", true)), Some(want));
        // The same servers again save nothing; empty is automatic.
        assert_eq!(b.event(typed("ntp.example.ch 10.0.0.1", true)), None);
        assert_eq!(b.event(typed("", true)), Some(TimeCfg::default()));
    }

    #[test]
    fn ntp_refused_not_saved() {
        let mut b = band();
        let e = TimeEvent::NtpText {
            text: "a;b".into(),
            commit: true,
        };
        assert_eq!(b.event(e), None);
        assert_eq!(b.ntp_refused().unwrap(), "Not used: a;b is not a host name or address.");
        assert_eq!(b.cfg(), &TimeCfg::default());
    }

    #[test]
    fn ntp_cancel_restores() {
        let mut b = band();
        b.set_cfg(TimeCfg {
            zone: None,
            ntp: vec!["a.ch".into()],
        });
        b.event(TimeEvent::NtpText {
            text: "b.ch".into(),
            commit: false,
        });
        b.event(TimeEvent::NtpCancel);
        assert_eq!(b.ntp_text(), "a.ch");
    }

    #[test]
    fn a_failed_save_shows_the_stored_setting_again() {
        let mut b = band();
        b.set_cfg(zone("Europe/Zurich"));
        let sent = b.event(TimeEvent::Region(1)).unwrap();
        b.saved(&sent, Err(TimeSaveFail::NotSaved("disk full".into())));
        assert_eq!(b.cfg(), &zone("Europe/Zurich"));
        assert_eq!(b.error(), Some("disk full"));
        let sent = b.event(TimeEvent::Region(1)).unwrap();
        b.saved(&sent, Ok(()));
        assert_eq!(b.error(), None);
        assert_eq!(b.cfg(), &zone("UTC"));
    }

    #[test]
    fn a_save_not_applied_stays_with_its_reason() {
        let mut b = band();
        let sent = b.event(TimeEvent::Region(1)).unwrap();
        b.saved(
            &sent,
            Err(TimeSaveFail::NotApplied(
                "The time service did not take the servers.".into(),
            )),
        );
        assert_eq!(b.cfg(), &zone("UTC"));
        assert_eq!(
            b.error(),
            Some("Saved, in effect after a restart: The time service did not take the servers.")
        );
        // Back to automatic is a change again.
        assert_eq!(b.event(TimeEvent::Region(0)), Some(TimeCfg::default()));
    }
}
