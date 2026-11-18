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

/// How `zone` is named on the tile: its IANA name, or the abbreviation in
/// force at `ts` (`CET`, `CEST`) for a zone with none. `log::zone_for` names
/// a copied `/etc/localtime` "localtime" and a `TZ=` rule has no name at all;
/// neither tells the reader anything.
pub fn zone_label(zone: &TimeZone, ts: Timestamp) -> String {
    match zone.iana_name() {
        Some(name) if name != "localtime" => name.to_string(),
        _ => zone.to_offset_info(ts).abbreviation().to_string(),
    }
}

/// `Time 14:05`, `Date Thu 5 Nov 2026` and `Zone Europe/Zurich` at `epoch`
/// seconds in `zone`.
fn lines_at(epoch: i64, zone: &TimeZone) -> Vec<(&'static str, String)> {
    let Ok(ts) = Timestamp::from_second(epoch) else {
        return Vec::new();
    };
    let at = zone.to_datetime(ts);
    vec![
        ("Time", at.strftime("%H:%M").to_string()),
        ("Date", at.strftime("%a %-d %b %Y").to_string()),
        ("Zone", zone_label(zone, ts)),
    ]
}

/// Draw label and value lines in `rect` of `size` cells, centred: the label
/// in Geo from the left padding, the value right-aligned at the right one.
/// A label that would run into its value is left out. A value too wide for
/// the space right of the label is cut from the start and prefixed with `…`:
/// the city at the end of `America/Argentina/Buenos_Aires` tells more than
/// the continent. Only when not even `…` and one letter fit beside the label
/// is the label dropped and the whole width left to the value.
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
        let font = FontId::proportional(m.value);
        let l = p.layout_no_wrap(label.to_string(), crate::brand::geo(m.label), fg);
        let beside_label = right - (left + l.size().x + m.gap);
        let width_of = |s: &str| p.layout_no_wrap(s.to_string(), font.clone(), fg).size().x;
        let text = cut_to_fit(value, beside_label, &width_of)
            .or_else(|| cut_to_fit(value, right - left, &width_of))
            .unwrap_or_else(|| "…".to_string());
        let v = p.layout_no_wrap(text, font, fg);
        let vx = right - v.size().x;
        if vx >= left {
            p.galley(pos2(vx, mid - v.size().y / 2.0), v, fg);
        }
        if left + l.size().x + m.gap <= vx {
            p.galley(pos2(left, mid - l.size().y / 2.0), l, fg);
        }
    }
}

/// `value`, or its end behind a `…` when `width_of` says it is wider than
/// `room`: the longest end that fits. `None` when not even `…` and the last
/// letter fit, because a bare `…` tells the reader nothing.
fn cut_to_fit(value: &str, room: f32, width_of: &dyn Fn(&str) -> f32) -> Option<String> {
    if width_of(value) <= room {
        return Some(value.to_string());
    }
    value
        .char_indices()
        .skip(1)
        .map(|(i, _)| format!("…{}", &value[i..]))
        .find(|s| width_of(s) <= room)
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
            [
                pair("Time", "12:00"),
                pair("Date", "Thu 1 Jan 1970"),
                pair("Zone", "UTC")
            ]
        );
        assert_eq!(
            at(12 * 3600, "Asia/Kolkata"),
            [
                pair("Time", "17:30"),
                pair("Date", "Thu 1 Jan 1970"),
                pair("Zone", "Asia/Kolkata")
            ]
        );
        // The date turns over with the zone, not with UTC.
        assert_eq!(
            at(20 * 3600, "Asia/Kolkata"),
            [
                pair("Time", "01:30"),
                pair("Date", "Fri 2 Jan 1970"),
                pair("Zone", "Asia/Kolkata")
            ]
        );
    }

    fn label_at(zone: &TimeZone, epoch: i64) -> String {
        zone_label(zone, Timestamp::from_second(epoch).unwrap())
    }

    const JANUARY: i64 = 1_767_225_600; // 2026-01-01
    const JULY: i64 = 1_782_864_000; // 2026-07-01

    #[test]
    fn the_zone_line_names_an_iana_zone() {
        let zurich = jiff::tz::db().get("Europe/Zurich").unwrap();
        assert_eq!(lines_at(JANUARY, &zurich)[2], pair("Zone", "Europe/Zurich"));
    }

    #[test]
    fn a_posix_tz_shows_its_abbreviation() {
        let tz = TimeZone::posix("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        assert_eq!(label_at(&tz, JANUARY), "CET");
        assert_eq!(label_at(&tz, JULY), "CEST");
    }

    #[test]
    fn a_copied_localtime_shows_its_abbreviation() {
        let dir = crate::testdir::TestDir::new("clock", "localtime");
        let copy = dir.join("localtime");
        std::fs::copy("/usr/share/zoneinfo/Europe/Zurich", &copy).unwrap();
        let tz = crate::log::zone_for(None, &copy);
        assert_eq!(label_at(&tz, JANUARY), "CET");
    }

    #[test]
    fn utc_is_utc() {
        assert_eq!(label_at(&TimeZone::UTC, JANUARY), "UTC");
    }

    #[test]
    fn a_long_zone_keeps_the_city() {
        // The tile is 220 points wide and the zone fits there; a narrower
        // rect stands for a larger font scale or a longer name.
        let one = Size { w: 1, h: 1 };
        let zone = "America/Argentina/Buenos_Aires";
        for h in testing::CELL_HEIGHTS {
            let mut rect = rect_of(one, h);
            rect.set_width(150.0);
            let lines = [pair("Zone", zone)];
            let ctx = egui::Context::default();
            crate::brand::apply(&ctx);
            let mut out = None;
            for _ in 0..3 {
                let mut o = ctx.run_ui(Default::default(), |ui| paint(ui, rect, one, &lines, 3.0));
                o.textures_delta.clear();
                out = Some(o);
            }
            let shapes: Vec<egui::Shape> = out.unwrap().shapes.into_iter().map(|c| c.shape).collect();
            let got = testing::texts(&shapes);
            let value = &got[0].0;
            assert!(
                value.starts_with('…') && value.ends_with("Buenos_Aires"),
                "{h}: {got:?}"
            );
            for s in testing::flatten(&shapes) {
                if let egui::Shape::Text(t) = s {
                    let r = t.visual_bounding_rect();
                    assert!(rect.contains_rect(r), "{h}: {r:?} outside {rect:?}");
                }
            }
        }
    }

    #[test]
    fn clock_draws_inside_its_rect_at_every_size() {
        let buenos_aires = jiff::tz::db().get("America/Argentina/Buenos_Aires").unwrap();
        let data = Data {
            wall: Some(UNIX_EPOCH + Duration::from_secs(JANUARY as u64)),
            zone: Some(&buenos_aires),
            ..Data::EMPTY
        };
        testing::assert_inside(&Clock, &data, Instant::now());
        // All three lines are there at both heights.
        let one = Size { w: 1, h: 1 };
        for h in testing::CELL_HEIGHTS {
            let drawn: Vec<String> = texts(&shapes(&Clock, rect_of(one, h), one, &data, Instant::now()))
                .into_iter()
                .map(|(s, _)| s)
                .collect();
            for label in ["Time", "Date", "Zone"] {
                assert!(drawn.contains(&label.to_string()), "{h}: {drawn:?}");
            }
        }
    }

    #[test]
    fn a_cut_needs_a_real_letter_after_the_ellipsis() {
        let one_per_char = |s: &str| s.chars().count() as f32;
        assert_eq!(cut_to_fit("abc", 3.0, &one_per_char).as_deref(), Some("abc"));
        assert_eq!(cut_to_fit("abcd", 3.0, &one_per_char).as_deref(), Some("…cd"));
        assert_eq!(cut_to_fit("abcd", 2.0, &one_per_char).as_deref(), Some("…d"));
        assert_eq!(cut_to_fit("abcd", 1.0, &one_per_char), None);
    }

    #[test]
    fn the_label_gives_way_when_no_letter_fits_beside_it() {
        let one = Size { w: 1, h: 1 };
        let mut rect = rect_of(one, 64.0);
        rect.set_width(55.0);
        let lines = [pair("Zone", "America/Argentina/Buenos_Aires")];
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
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].starts_with('…') && got[0].len() > '…'.len_utf8(), "{got:?}");
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
