//! The battery element: one line per device (`battery::lines`), the kind in
//! Geo on the left, a battery symbol filled to the level, the percent in the
//! body font on the right, `+` while charging. A line is in the alert colour
//! below 10 % or when the device says `critical`, muted with `–` when it is
//! not reachable. Lines that do not fit turn page every `battery::PAGE_EVERY`
//! (4 s). It changes with the battery file and with its pages.

use super::{Data, Element, Size, Source, background, sizes_for};
use crate::battery::{self, CHARGING_MARK, Line, Tone};
use crate::brand::{StatusSizes, role};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Color32, FontId, Galley, Painter, Rect, Stroke, pos2, vec2};
use shadcn_egui::Theme;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Battery;

const SIZES: &[Size] = &[Size { w: 1, h: 1 }, Size { w: 2, h: 1 }];

/// The widest percent; the column is kept this wide.
const WIDEST: &str = "100 %";

/// The sizes for an element `rect`: it is one row high, so its height is
/// its cell's.
fn sizes(rect: Rect) -> StatusSizes {
    sizes_for(rect.height())
}

/// How many lines fit an element `height` points high; one at least.
pub fn per_page(height: f32, m: &StatusSizes) -> usize {
    (((height - 2.0 * m.pad_y) / m.row).floor() as usize).max(1)
}

fn page(data: &Data, rect: Rect, now: Instant) -> (Vec<Line>, battery::Page) {
    let lines = battery::lines(data.battery);
    let elapsed = data
        .battery_since
        .map_or(Duration::ZERO, |t| now.saturating_duration_since(t));
    let page = battery::page(
        lines.len(),
        per_page(rect.height(), &sizes(rect)),
        elapsed,
        battery::PAGE_EVERY,
    );
    (lines, page)
}

fn colour(t: &Theme, tone: Tone) -> Color32 {
    match tone {
        Tone::Normal => t.palette().foreground,
        Tone::Low => t.role(role::ALERT),
        Tone::Muted => t.palette().muted_foreground,
    }
}

/// `text` on one line, cut after the last glyph that ends within `width`.
fn cut(p: &Painter, text: &str, font: FontId, colour: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat::simple(font, colour));
    job.wrap = TextWrapping {
        max_width: width.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: None,
    };
    p.layout_job(job)
}

/// Draw `g` with its left edge at `x`, centred on `mid`, unless it is
/// wider than `room` (a cut that kept one glyph too many).
fn put(p: &Painter, g: Arc<Galley>, x: f32, mid: f32, room: f32, colour: Color32) {
    if g.size().x <= room + 0.01 {
        p.galley(pos2(x, mid - g.size().y / 2.0), g, colour);
    }
}

/// Where the columns of a battery element in `rect` go.
struct Columns {
    /// The right edge of the labels.
    label_right: f32,
    /// The left edge of the battery symbols.
    symbol_x: f32,
    /// The right edge of the percents.
    pct_right: f32,
    /// The left edge of the charging mark.
    mark_x: f32,
}

fn symbol_size(value: f32) -> (f32, f32) {
    ((2.6 * value).round(), (0.8 * value).round())
}

/// The nub right of the symbol's body.
const NUB: f32 = 2.0;

fn columns(p: &Painter, rect: Rect, m: &StatusSizes, value: &FontId) -> Columns {
    let width = |t: &str| {
        p.layout_no_wrap(t.to_string(), value.clone(), Color32::TRANSPARENT)
            .size()
            .x
    };
    let mark_x = rect.right() - m.pad_x - width(CHARGING_MARK);
    let space = 0.35 * m.value;
    let pct_right = mark_x - space;
    let symbol_x = (pct_right - width(WIDEST) - m.gap - symbol_size(m.value).0 - NUB).floor();
    Columns {
        label_right: symbol_x - m.gap,
        symbol_x,
        pct_right,
        mark_x,
    }
}

/// A battery symbol from `x`, centred on `mid`: an outline with a nub on the
/// right, filled to `level` percent. No level (not reachable, no percent) is
/// a dashed empty outline, which reads differently from a solid empty one
/// at 0 %. On whole points, so its edges are sharp at whole scales.
fn symbol(p: &Painter, x: f32, mid: f32, value: f32, level: Option<u8>, c: Color32) {
    let (w, h) = symbol_size(value);
    let body = Rect::from_min_size(pos2(x.floor(), (mid - h / 2.0).round()), vec2(w, h));
    let stroke = Stroke::new(1.0, c);
    match level {
        Some(_) => {
            p.rect_stroke(body, 0.0, stroke, egui::StrokeKind::Inside);
        }
        None => {
            let r = body.shrink(0.5);
            let path = [
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
                r.left_top(),
            ];
            p.extend(egui::Shape::dashed_line(&path, stroke, 2.0, 2.0));
        }
    }
    let nub_h = (h / 2.0).round();
    p.rect_filled(
        Rect::from_min_size(
            pos2(body.right(), body.top() + ((h - nub_h) / 2.0).round()),
            vec2(NUB, nub_h),
        ),
        0.0,
        c,
    );
    if let Some(l) = level.filter(|l| *l > 0) {
        let inner = body.shrink(2.0);
        let fill = (inner.width() * f32::from(l.min(100)) / 100.0).round().max(1.0);
        p.rect_filled(Rect::from_min_size(inner.min, vec2(fill, inner.height())), 0.0, c);
    }
}

/// Draw `lines` (one page) in `rect`, centred: the kind in Geo, the symbol,
/// the percent right-aligned in the body font, then the charging mark in a
/// slot of its own, so all percents end at one edge. A label is cut where
/// the symbol column starts; a numbered one that would lose its number keeps
/// it at that edge and cuts the name before it, so two such lines stay
/// apart.
fn paint(ui: &egui::Ui, rect: Rect, lines: &[Line]) {
    let t = Theme::current(ui.ctx());
    let p = ui.painter();
    let m = sizes(rect);
    let label_font = crate::brand::geo(m.label);
    let value_font = FontId::proportional(m.value);
    let c = columns(p, rect, &m, &value_font);
    let left = rect.left() + m.pad_x;
    let space = 0.35 * m.value;
    let top = rect.top() + (rect.height() - lines.len() as f32 * m.row) / 2.0;
    for (i, line) in lines.iter().enumerate() {
        let fill = colour(&t, line.tone);
        let mid = top + (i as f32 + 0.5) * m.row;
        let whole = p.layout_no_wrap(line.label.clone(), label_font.clone(), fill);
        let room = c.label_right - left;
        let split = line
            .label
            .rsplit_once(' ')
            .filter(|_| line.numbered && whole.size().x > room);
        match split {
            Some((name, number)) => {
                let n = p.layout_no_wrap(number.to_string(), label_font.clone(), fill);
                let name_room = room - n.size().x - space;
                put(
                    p,
                    cut(p, name, label_font.clone(), fill, name_room),
                    left,
                    mid,
                    name_room,
                    fill,
                );
                let nx = c.label_right - n.size().x;
                put(p, n, nx, mid, c.label_right - nx, fill);
            }
            None => put(
                p,
                cut(p, &line.label, label_font.clone(), fill, room),
                left,
                mid,
                room,
                fill,
            ),
        }
        symbol(p, c.symbol_x, mid, m.value, line.level, fill);
        let pct = p.layout_no_wrap(line.percent.clone(), value_font.clone(), fill);
        let px = c.pct_right - pct.size().x;
        put(p, pct, px, mid, c.pct_right - px, fill);
        if line.charging {
            let mark = p.layout_no_wrap(CHARGING_MARK.to_string(), value_font.clone(), fill);
            let room = rect.right() - m.pad_x - c.mark_x;
            put(p, mark, c.mark_x, mid, room, fill);
        }
    }
}

impl Element for Battery {
    fn id(&self) -> &'static str {
        "battery"
    }

    fn name(&self) -> &'static str {
        "Keyboard and mouse batteries"
    }

    fn sizes(&self) -> &'static [Size] {
        SIZES
    }

    fn short_size(&self) -> Option<Size> {
        Some(SIZES[0])
    }

    fn sources(&self) -> &'static [Source] {
        &[Source::Battery]
    }

    fn has_content(&self, data: &Data) -> bool {
        !data.battery.is_empty()
    }

    fn draw(&self, ui: &mut egui::Ui, rect: Rect, _size: Size, data: &Data, now: Instant) {
        background(ui, rect);
        let (lines, page) = page(data, rect, now);
        paint(ui, rect, &lines[page.start..page.end]);
    }

    fn next_change(&self, data: &Data, rect: Rect, now: Instant) -> Option<Duration> {
        page(data, rect, now).1.next
    }

    fn refresh(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{self, CELL_HEIGHTS, rect_of, shapes, texts};
    use super::*;
    use crate::battery::{Device, DeviceKind};

    fn dev(name: &str, kind: DeviceKind, percent: Option<u8>, state: &str) -> Device {
        Device {
            name: name.into(),
            percent,
            state: state.into(),
            read_epoch: 1,
            kind,
        }
    }

    fn data(devices: &[Device], since: Instant) -> Data<'_> {
        Data {
            battery: devices,
            battery_since: Some(since),
            ..Data::EMPTY
        }
    }

    const ONE: Size = Size { w: 1, h: 1 };

    fn drawn(devices: &[Device], at: Instant, since: Instant) -> Vec<(String, Color32)> {
        texts(&shapes(&Battery, rect_of(ONE, 76.0), ONE, &data(devices, since), at))
    }

    fn theme() -> Theme {
        crate::brand::theme()
    }

    #[test]
    fn battery_draws_inside_its_rect_at_every_size() {
        let long = |i: usize| format!("{i} {}", "W".repeat(58));
        let devices: Vec<Device> = (0..10)
            .map(|i| dev(&long(i), DeviceKind::Unknown, Some(10 * i as u8), "charging"))
            .collect();
        assert!(devices.iter().all(|d| d.name.chars().count() == 60));
        let now = Instant::now();
        testing::assert_inside(&Battery, &data(&devices, now), now);
        // Two of a kind, numbered, with long names of their own.
        let same: Vec<Device> = (0..10)
            .map(|_| dev(&"M".repeat(60), DeviceKind::Unknown, None, "not reachable"))
            .collect();
        testing::assert_inside(&Battery, &data(&same, now), now);
    }

    #[test]
    fn one_line_per_device_with_percent_and_charging_mark() {
        let devices = [
            dev("MX Anywhere 3S", DeviceKind::Mouse, Some(95), "charging"),
            dev("MX Mechanical", DeviceKind::Keyboard, Some(100), "discharging"),
        ];
        let now = Instant::now();
        let fg = theme().palette().foreground;
        assert_eq!(
            drawn(&devices, now, now),
            vec![
                ("Keyboard".into(), fg),
                ("100 %".into(), fg),
                ("Mouse".into(), fg),
                ("95 %".into(), fg),
                (CHARGING_MARK.into(), fg),
            ]
        );
    }

    #[test]
    fn red_below_10_percent_or_critical_and_a_dash_when_unknown() {
        let devices = [
            dev("K", DeviceKind::Keyboard, Some(9), "discharging"),
            dev("M", DeviceKind::Mouse, Some(50), "critical"),
            dev("Pen", DeviceKind::Unknown, Some(5), "not reachable"),
            dev("Pad", DeviceKind::Unknown, None, "unknown"),
        ];
        let now = Instant::now();
        let t = theme();
        let (red, muted, fg) = (
            t.role(role::ALERT),
            t.palette().muted_foreground,
            t.palette().foreground,
        );
        assert_ne!(red, fg);
        assert_eq!(
            drawn(&devices, now, now),
            vec![
                ("Keyboard".into(), red),
                ("9 %".into(), red),
                ("Mouse".into(), red),
                ("50 %".into(), red),
                ("Pen".into(), muted),
                ("–".into(), muted),
                ("Pad".into(), fg),
                ("–".into(), fg),
            ]
        );
    }

    #[test]
    fn a_long_name_keeps_its_number_visible() {
        let long = "A very long wireless device name indeed";
        let devices = [
            dev(long, DeviceKind::Unknown, Some(50), "discharging"),
            dev(long, DeviceKind::Unknown, Some(50), "discharging"),
        ];
        let now = Instant::now();
        let rect = rect_of(ONE, 76.0);
        let long_lines = shapes(&Battery, rect, ONE, &data(&devices, now), now);
        // Each text with where its ink is.
        let placed: Vec<(String, Rect)> = long_lines
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Text(t) => Some((t.galley.text().to_string(), s.visual_bounding_rect())),
                _ => None,
            })
            .collect();
        let at = |text: &str| {
            placed
                .iter()
                .filter(|(t, _)| t == text)
                .map(|(_, r)| *r)
                .collect::<Vec<_>>()
        };
        let (names, ones, twos) = (at(long), at("1"), at("2"));
        assert_eq!((names.len(), ones.len(), twos.len()), (2, 1, 1), "{placed:?}");
        // The name is cut before its number, which ends at one edge for both.
        assert!(
            names[0].right() < ones[0].left() && names[1].right() < twos[0].left(),
            "{placed:?}"
        );
        assert!((ones[0].right() - twos[0].right()).abs() < 2.0, "{placed:?}");
        assert!(names[0].width() < rect.width() / 2.0, "cut: {placed:?}");
        // A short numbered label stays one text.
        let short = [
            dev("Pad", DeviceKind::Unknown, Some(50), "discharging"),
            dev("Pad", DeviceKind::Unknown, Some(50), "discharging"),
        ];
        let t: Vec<String> = drawn(&short, now, now).into_iter().map(|(s, _)| s).collect();
        assert!(t.contains(&"Pad 2".to_string()), "{t:?}");
    }

    fn five() -> Vec<Device> {
        ["A", "B", "C", "D", "E"]
            .iter()
            .map(|n| dev(&format!("Dev {n}"), DeviceKind::Unknown, Some(50), "discharging"))
            .collect()
    }

    fn labels(devices: &[Device], rect: Rect, at: Instant, since: Instant) -> Vec<String> {
        texts(&shapes(&Battery, rect, ONE, &data(devices, since), at))
            .into_iter()
            .map(|(t, _)| t)
            .filter(|t| t.starts_with("Dev"))
            .collect()
    }

    #[test]
    fn battery_pages_turn_every_4_s() {
        let full = rect_of(ONE, 76.0);
        let small = rect_of(ONE, 64.0);
        assert_eq!(per_page(full.height(), &sizes(full)), 4);
        assert_eq!(per_page(small.height(), &sizes(small)), 3);
        let devices = five();
        let t0 = Instant::now();
        let d = data(&devices, t0);
        let at = |ms| t0 + Duration::from_millis(ms);
        assert_eq!(Battery.next_change(&d, full, at(0)), Some(Duration::from_secs(4)));
        assert_eq!(
            Battery.next_change(&d, full, at(1500)),
            Some(Duration::from_millis(2500))
        );
        assert_eq!(labels(&devices, full, at(0), t0), ["Dev A", "Dev B", "Dev C", "Dev D"]);
        assert_eq!(labels(&devices, full, at(4000), t0), ["Dev E"]);
        assert_eq!(
            labels(&devices, full, at(8000), t0),
            ["Dev A", "Dev B", "Dev C", "Dev D"]
        );
        // The smaller cell holds three.
        assert_eq!(labels(&devices, small, at(0), t0), ["Dev A", "Dev B", "Dev C"]);
        // Lines that fit do not page.
        let four = &devices[..4];
        assert_eq!(Battery.next_change(&data(four, t0), full, at(1500)), None);
        assert_eq!(Battery.refresh(), None);
    }

    #[test]
    fn three_devices_fit_without_paging_at_both_cell_heights() {
        let devices = [
            dev("MX Mechanical", DeviceKind::Keyboard, Some(100), "charging"),
            dev("MX Anywhere 3S", DeviceKind::Mouse, Some(95), "charging"),
            dev("R", DeviceKind::Trackball, None, "not reachable"),
        ];
        let now = Instant::now();
        for h in CELL_HEIGHTS {
            let rect = rect_of(ONE, h);
            assert_eq!(Battery.next_change(&data(&devices, now), rect, now), None, "{h}");
            let t = texts(&shapes(&Battery, rect, ONE, &data(&devices, now), now));
            assert_eq!(
                t.iter().filter(|(s, _)| s.ends_with('%') || s == "–").count(),
                3,
                "{h}: {t:?}"
            );
        }
    }

    #[test]
    fn shown_only_with_a_device() {
        let now = Instant::now();
        assert!(!Battery.has_content(&data(&[], now)));
        assert!(Battery.has_content(&data(&five(), now)));
        assert_eq!(Battery.sources(), &[Source::Battery]);
    }
}
