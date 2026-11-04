//! The clock element (`Type=clock`): the time and the date in the machine's
//! zone, drawn like a hardware tile. It always has something to show, so an
//! area holding it is never hidden. The zone is looked up on every picture, so
//! a DHCP zone that arrives after the menu started shows at the next minute.

use super::hardware::{Line, line, svg};
use super::{Element, Kind, Room, Sources};
use crate::desktop::Entry;
use crate::header::until_next_minute;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const KIND: Kind = Kind {
    name: "clock",
    options: &[],
    build,
};

fn build(_entry: &Entry) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(Clock))
}

pub struct Clock;

/// `Time 14:05` and `Date Thu 5 Nov 2026` at `epoch` seconds in `zone`.
fn lines_at(epoch: i64, zone: &TimeZone) -> Vec<Line> {
    let Ok(ts) = Timestamp::from_second(epoch) else {
        return Vec::new();
    };
    let at = zone.to_datetime(ts);
    vec![
        line("Time", at.strftime("%H:%M").to_string()),
        line("Date", at.strftime("%a %-d %b %Y").to_string()),
    ]
}

impl Element for Clock {
    fn visible(&self, _data: &Sources) -> bool {
        true
    }

    fn svg(&self, _data: &Sources, room: Room, _now: Instant) -> String {
        let epoch = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
        let zone = crate::log::current_zone(Path::new("/etc/localtime"));
        svg(&lines_at(epoch, &zone), room)
    }

    fn next_change(&self, _data: &Sources, _room: Room, _now: Instant) -> Option<Duration> {
        Some(until_next_minute(SystemTime::now()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::style::{self, mono_width};
    use super::*;
    use egui::vec2;

    fn text(lines: &[Line]) -> Vec<(String, String)> {
        lines
            .iter()
            .map(|l| (l.label.clone(), l.value.clone().unwrap_or_default()))
            .collect()
    }

    fn pair(label: &str, value: &str) -> (String, String) {
        (label.to_string(), value.to_string())
    }

    #[test]
    fn time_and_date_follow_the_zone() {
        let at = |epoch, zone: &str| text(&lines_at(epoch, &jiff::tz::db().get(zone).unwrap()));
        assert_eq!(
            at(12 * 3600, "UTC"),
            [pair("Time", "12:00"), pair("Date", "Thu 1 Jan 1970")]
        );
        assert_eq!(
            at(12 * 3600, "Asia/Kolkata"),
            [pair("Time", "17:30"), pair("Date", "Thu 1 Jan 1970")]
        );
        // The date turns over with the zone, not with UTC.
        assert_eq!(
            at(20 * 3600, "Asia/Kolkata"),
            [pair("Time", "01:30"), pair("Date", "Fri 2 Jan 1970")]
        );
    }

    #[test]
    fn the_longest_date_fits_one_cell_at_every_cell_height() {
        for h in [76.0, 64.0] {
            let r = Room::one(vec2(220.0, h));
            let st = r.style();
            let free = r.size.x - 2.0 * st.pad_x - st.gap;
            let width = style::label_width("Date", st.title) + mono_width("Wed 30 Sep 2026", st.value);
            assert!(width <= free, "{width} of {free} at {h}");
        }
    }

    #[test]
    fn it_always_shows_and_wakes_within_the_minute() {
        const DATA: Sources = Sources {
            battery: &[],
            battery_since: None,
            network: None,
            network_since: None,
            hardware: None,
        };
        let r = Room::one(vec2(220.0, 76.0));
        assert!(Clock.visible(&DATA));
        let s = Clock.svg(&DATA, r, Instant::now());
        assert!(s.contains("Time") && s.contains("Date"), "{s}");
        crate::status::render::render(&s, [220, 76]).expect("renders");
        let next = Clock.next_change(&DATA, r, Instant::now()).unwrap();
        assert!(next <= Duration::from_millis(60_020), "{next:?}");
    }
}
