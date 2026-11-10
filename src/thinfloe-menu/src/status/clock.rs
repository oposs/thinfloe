//! The clock element: the time and the date in the machine's zone, a label
//! in Geo on the left and the value in the body font on the right. It always
//! has something to show. Time and zone come from `Data`: the app reads
//! the clock and looks the zone up once per frame, so a DHCP zone that
//! arrives after the menu started shows at the next minute.

use super::{Data, Element, Size, Source, background, sizes_for};
use crate::header::until_next_minute;
use egui::{FontId, Rect, pos2};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use shadcn_egui::Theme;
use std::time::{Duration, Instant, UNIX_EPOCH};

pub struct Clock;

const SIZES: &[Size] = &[Size { w: 1, h: 1 }];

/// `Time 14:05` and `Date Thu 5 Nov 2026` at `epoch` seconds in `zone`.
fn lines_at(epoch: i64, zone: &TimeZone) -> Vec<(&'static str, String)> {
    let Ok(ts) = Timestamp::from_second(epoch) else {
        return Vec::new();
    };
    let at = zone.to_datetime(ts);
    vec![
        ("Time", at.strftime("%H:%M").to_string()),
        ("Date", at.strftime("%a %-d %b %Y").to_string()),
    ]
}

/// Draw label and value lines in `rect` of `size` cells, centred: the label
/// in Geo from the left padding, the value right-aligned at the right one.
/// A label that would run into its value is left out.
fn paint(ui: &egui::Ui, rect: Rect, size: Size, lines: &[(&str, String)], gap_between_cells: f32) {
    let t = Theme::current(ui.ctx());
    let fg = t.palette().foreground;
    let p = ui.painter();
    let cell_h = (rect.height() - f32::from(size.h - 1) * gap_between_cells) / f32::from(size.h);
    let m = sizes_for(cell_h);
    let top = rect.top() + (rect.height() - lines.len() as f32 * m.row) / 2.0;
    let (left, right) = (rect.left() + m.pad_x, rect.right() - m.pad_x);
    for (i, (label, value)) in lines.iter().enumerate() {
        let mid = top + (i as f32 + 0.5) * m.row;
        let v = p.layout_no_wrap(value.clone(), FontId::proportional(m.value), fg);
        let l = p.layout_no_wrap(label.to_string(), crate::brand::geo(m.label), fg);
        let vx = right - v.size().x;
        if vx >= left {
            p.galley(pos2(vx, mid - v.size().y / 2.0), v, fg);
        }
        if left + l.size().x + m.gap <= vx {
            p.galley(pos2(left, mid - l.size().y / 2.0), l, fg);
        }
    }
}

impl Element for Clock {
    fn id(&self) -> &'static str {
        "clock"
    }

    fn name(&self) -> &'static str {
        "Clock"
    }

    fn sizes(&self) -> &'static [Size] {
        SIZES
    }

    fn short_size(&self) -> Option<Size> {
        None
    }

    fn sources(&self) -> &'static [Source] {
        &[]
    }

    fn has_content(&self, _data: &Data) -> bool {
        true
    }

    fn draw(&self, ui: &mut egui::Ui, rect: Rect, size: Size, data: &Data, _now: Instant) {
        background(ui, rect);
        let Some(wall) = data.wall else { return };
        let epoch = wall.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
        let utc = TimeZone::UTC;
        let zone = data.zone.unwrap_or(&utc);
        paint(ui, rect, size, &lines_at(epoch, zone), crate::ui::TILE_GAP);
    }

    fn next_change(&self, data: &Data, _rect: Rect, _now: Instant) -> Option<Duration> {
        data.wall.map(until_next_minute)
    }

    fn refresh(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{self, rect_of, shapes, texts};
    use super::*;
    use std::time::SystemTime;

    fn pair(label: &'static str, value: &str) -> (&'static str, String) {
        (label, value.to_string())
    }

    #[test]
    fn time_and_date_follow_the_zone() {
        let at = |epoch, zone: &str| lines_at(epoch, &jiff::tz::db().get(zone).unwrap());
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
    fn clock_draws_inside_its_rect_at_every_size() {
        let now = Instant::now();
        testing::assert_inside(&Clock, &Data::EMPTY, now);
    }

    #[test]
    fn the_longest_date_fits_beside_its_label_at_every_cell_height() {
        // `Wed 30 Sep 2026` is about as wide as a date gets.
        let one = Size { w: 1, h: 1 };
        for h in testing::CELL_HEIGHTS {
            let rect = rect_of(one, h);
            let lines = [pair("Time", "23:59"), pair("Date", "Wed 30 Sep 2026")];
            let ctx = egui::Context::default();
            crate::brand::apply(&ctx);
            let mut out = None;
            for _ in 0..3 {
                let mut o = ctx.run_ui(Default::default(), |ui| paint(ui, rect, one, &lines, 3.0));
                o.textures_delta.clear();
                out = Some(o);
            }
            let shapes: Vec<egui::Shape> = out.unwrap().shapes.into_iter().map(|c| c.shape).collect();
            let got: Vec<String> = texts(&shapes).into_iter().map(|(s, _)| s).collect();
            assert_eq!(got, ["23:59", "Time", "Wed 30 Sep 2026", "Date"], "{h}");
        }
    }

    #[test]
    fn it_always_shows_draws_time_and_date_and_wakes_within_the_minute() {
        let now = Instant::now();
        let one = Size { w: 1, h: 1 };
        let rect = rect_of(one, 76.0);
        let wall = SystemTime::now();
        let data = Data {
            wall: Some(wall),
            ..Data::EMPTY
        };
        assert!(Clock.has_content(&data));
        let got: Vec<String> = texts(&shapes(&Clock, rect, one, &data, now))
            .into_iter()
            .map(|(s, _)| s)
            .collect();
        assert!(
            got.contains(&"Time".to_string()) && got.contains(&"Date".to_string()),
            "{got:?}"
        );
        let next = Clock.next_change(&data, rect, now).unwrap();
        assert!(next <= Duration::from_millis(60_020), "{next:?}");
        assert_eq!(Clock.refresh(), None);
        assert_eq!(Clock.short_size(), None);
    }

    #[test]
    fn the_clock_draws_the_time_and_zone_it_is_given() {
        let now = Instant::now();
        let one = Size { w: 1, h: 1 };
        let rect = rect_of(one, 76.0);
        let kolkata = jiff::tz::db().get("Asia/Kolkata").unwrap();
        let wall = UNIX_EPOCH + Duration::from_secs(12 * 3600 + 30);
        let at = |zone| Data {
            wall: Some(wall),
            zone,
            ..Data::EMPTY
        };
        let drawn = |data: &Data| -> Vec<String> {
            texts(&shapes(&Clock, rect, one, data, now))
                .into_iter()
                .map(|(s, _)| s)
                .collect()
        };
        let utc = drawn(&at(None));
        assert!(utc.contains(&"12:00".to_string()), "no zone is UTC: {utc:?}");
        assert!(utc.contains(&"Thu 1 Jan 1970".to_string()), "{utc:?}");
        let ist = drawn(&at(Some(&kolkata)));
        assert!(ist.contains(&"17:30".to_string()), "{ist:?}");
        // The same inputs draw the same picture.
        assert_eq!(drawn(&at(Some(&kolkata))), ist);
        // 30 s into the minute: the next picture in 30 s and a little.
        assert_eq!(
            Clock.next_change(&at(None), rect, now),
            Some(Duration::from_millis(30_020))
        );
        // Without a time there is nothing to tell and nothing to wait for.
        let none: Vec<String> = drawn(&Data::EMPTY);
        assert!(none.is_empty(), "{none:?}");
        assert_eq!(Clock.next_change(&Data::EMPTY, rect, now), None);
    }
}
