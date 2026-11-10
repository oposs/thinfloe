//! The battery element: one line per device (`battery::lines`), the kind in
//! Geo on the left, a battery symbol filled to the level, the percent in Hack
//! on the right, `+` while charging, sizes and colours from `style`. Lines that do not fit turn page every `PageEvery=` seconds of
//! its file (1 to 60, default `battery::PAGE_EVERY`, 4 s).
//! It changes with the battery file and with its pages; it takes no click.

use super::style::{self, Anchor, Face, Style, battery_width, hex, mono_width};
use super::{Element, Kind, Room, Sources};
use crate::battery::{self, CHARGING_MARK, Line, Tone};
use crate::desktop::Entry;
use egui::Color32;
use std::time::{Duration, Instant};

pub const KIND: Kind = Kind {
    name: "battery",
    options: &["PageEvery"],
    build,
};

fn build(entry: &Entry) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(Battery {
        page_every: page_every(entry)?,
    }))
}

/// The `PageEvery=` of an element's file, 1 to 60 seconds.
fn page_every(entry: &Entry) -> Result<Duration, String> {
    Ok(match entry.get("PageEvery") {
        None => battery::PAGE_EVERY,
        Some(v) => {
            super::config::seconds(v, 60).ok_or_else(|| format!("PageEvery must be 1 to 60 seconds, not '{v}'"))?
        }
    })
}

pub struct Battery {
    /// How long one page of lines stays.
    pub page_every: Duration,
}

impl Default for Battery {
    fn default() -> Battery {
        Battery {
            page_every: battery::PAGE_EVERY,
        }
    }
}

/// How many lines fit an element `height` points high in style `st`; one
/// at least.
pub fn per_page(height: f32, st: &Style) -> usize {
    (((height - 2.0 * st.pad_y) / st.row).floor() as usize).max(1)
}

fn colour(tone: Tone) -> Color32 {
    match tone {
        Tone::Normal => style::foreground(),
        Tone::Low => style::ALERT,
        Tone::Muted => style::MUTED,
    }
}

/// The widest percent; the column is kept this wide.
const WIDEST: &str = "100 %";

/// Where the columns of a battery element `w` points wide go.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Columns {
    /// The right edge of the labels' clip.
    pub label_right: f32,
    /// The left edge of the battery symbols.
    pub symbol_x: f32,
    /// The right edge of the percents (right-aligned, Hack).
    pub pct_right: f32,
    /// The left edge of the charging mark.
    pub mark_x: f32,
}

pub fn columns(w: f32, st: &Style) -> Columns {
    let mark_x = w - st.pad_x - mono_width(CHARGING_MARK, st.value);
    let pct_right = mark_x - st.space();
    let symbol_x = (pct_right - mono_width(WIDEST, st.value) - st.gap - battery_width(st.value)).floor();
    Columns {
        label_right: symbol_x - st.gap,
        symbol_x,
        pct_right,
        mark_x,
    }
}

/// The SVG of `lines` (one page) in `room`: a tile-coloured box, the lines
/// centred in it, the kind in Geo at the title size, the battery symbol, the
/// percent in Hack at the value size, right-aligned, then the charging mark
/// in a slot of its own, so all percents end at one edge. Labels are clipped
/// where the symbol column starts, so a long name is cut off and never runs
/// into it; a number that would be cut off with it ends at that edge
/// instead and the name is clipped before it, so two such lines stay apart.
pub fn svg(lines: &[Line], room: Room) -> String {
    let st = room.style();
    let (w, h) = (room.size.x, room.size.y);
    let Columns {
        label_right,
        symbol_x,
        pct_right,
        mark_x,
    } = columns(w, &st);
    let top = (h - lines.len() as f32 * st.row) / 2.0;
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\
         <rect width=\"{w}\" height=\"{h}\" fill=\"{}\"/>\
         <clipPath id=\"label\"><rect width=\"{label_right}\" height=\"{h}\"/></clipPath>",
        hex(style::BACKGROUND)
    );
    for (i, line) in lines.iter().enumerate() {
        let fill = colour(line.tone);
        let y = style::baseline(top + i as f32 * st.row, st.row, st.title);
        let split = line
            .label
            .rsplit_once(' ')
            .filter(|_| line.numbered && st.pad_x + style::label_width(&line.label, st.title) > label_right);
        if let Some((_, number)) = split {
            let right = label_right - style::label_width(number, st.title) - st.space();
            s.push_str(&format!(
                "<clipPath id=\"name{i}\"><rect width=\"{right}\" height=\"{h}\"/></clipPath>\
                 <g clip-path=\"url(#name{i})\">"
            ));
        } else {
            s.push_str("<g clip-path=\"url(#label)\">");
        }
        style::text(
            &mut s,
            st.pad_x,
            y,
            Face::Label,
            st.title,
            fill,
            Anchor::Start,
            split.map_or(line.label.as_str(), |(name, _)| name),
        );
        s.push_str("</g>");
        if let Some((_, number)) = split {
            style::text(&mut s, label_right, y, Face::Label, st.title, fill, Anchor::End, number);
        }
        style::draw_battery(&mut s, symbol_x, y, st.value, line.level, fill);
        style::text(
            &mut s,
            pct_right,
            y,
            Face::Value,
            st.value,
            fill,
            Anchor::End,
            &line.percent,
        );
        if line.charging {
            style::text(
                &mut s,
                mark_x,
                y,
                Face::Value,
                st.value,
                fill,
                Anchor::Start,
                CHARGING_MARK,
            );
        }
    }
    s.push_str("</svg>");
    s
}

fn page(data: &Sources, room: Room, now: Instant, every: Duration) -> (Vec<Line>, battery::Page) {
    let lines = battery::lines(data.battery);
    let elapsed = data
        .battery_since
        .map_or(Duration::ZERO, |t| now.saturating_duration_since(t));
    let page = battery::page(lines.len(), per_page(room.size.y, &room.style()), elapsed, every);
    (lines, page)
}

impl Element for Battery {
    fn visible(&self, data: &Sources) -> bool {
        !data.battery.is_empty()
    }

    fn svg(&self, data: &Sources, room: Room, now: Instant) -> String {
        let (lines, page) = page(data, room, now, self.page_every);
        svg(&lines[page.start..page.end], room)
    }

    fn next_change(&self, data: &Sources, room: Room, now: Instant) -> Option<Duration> {
        page(data, room, now, self.page_every).1.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery::{Device, DeviceKind};
    use crate::status::render::{GEO_FAMILY, MONO_FAMILY};
    use crate::status::style::{LARGE, SMALL, escape};
    use crate::ui::{TILE_H, TILE_H_MIN};
    use egui::{Vec2, vec2};

    fn dev(name: &str, kind: DeviceKind, percent: Option<u8>, state: &str) -> Device {
        Device {
            name: name.into(),
            percent,
            state: state.into(),
            read_epoch: 1,
            kind,
        }
    }

    const CELL: Room = Room {
        size: Vec2::new(220.0, TILE_H),
        cell: Vec2::new(220.0, TILE_H),
    };

    #[test]
    fn a_long_name_keeps_its_number_visible() {
        let long = "A very long wireless device name indeed";
        let devices = [
            dev(long, DeviceKind::Unknown, Some(50), "discharging"),
            dev(long, DeviceKind::Unknown, Some(50), "discharging"),
        ];
        let lines = battery::lines(&devices);
        let one = svg(&lines[..1], CELL);
        let two = svg(&lines[1..2], CELL);
        // Each drawn on its own, so nothing but the label differs.
        let draw = |s: &str| super::super::render::render(s, [440, 152]).unwrap().pixels;
        assert_ne!(draw(&one), draw(&two), "the number was clipped away");
        // The number ends where the labels end, in front of the symbol.
        let right = columns(CELL.size.x, &CELL.style()).label_right;
        assert!(texts(&one).iter().any(|(t, _)| t == "1"), "{one}");
        assert!(texts(&two).iter().any(|(t, _)| t == "2"), "{two}");
        assert!(one.contains(&format!("x=\"{right:.2}\"")), "{one}");
        // A short numbered label stays one text.
        let short = [
            dev("Pad", DeviceKind::Unknown, Some(50), "discharging"),
            dev("Pad", DeviceKind::Unknown, Some(50), "discharging"),
        ];
        let lines = battery::lines(&short);
        assert!(texts(&svg(&lines, CELL)).iter().any(|(t, _)| t == "Pad 2"));
    }

    /// The text nodes of an SVG with their fill, in order.
    fn texts(svg: &str) -> Vec<(String, String)> {
        let doc = resvg::usvg::roxmltree::Document::parse(svg).expect("well-formed SVG");
        doc.descendants()
            .filter(|n| n.has_tag_name("text"))
            .map(|n| {
                (
                    n.text().unwrap_or("").to_string(),
                    n.attribute("fill").unwrap().to_string(),
                )
            })
            .collect()
    }

    fn data(devices: &[Device], since: Instant) -> Sources<'_> {
        Sources {
            battery: devices,
            battery_since: Some(since),
            network: None,
            network_since: None,
            hardware: None,
        }
    }

    #[test]
    fn one_line_per_device_with_percent_and_charging_mark() {
        let devices = [
            dev("MX Anywhere 3S", DeviceKind::Mouse, Some(95), "charging"),
            dev("MX Mechanical", DeviceKind::Keyboard, Some(100), "discharging"),
        ];
        let now = Instant::now();
        let s = Battery::default().svg(&data(&devices, now), CELL, now);
        let fg = hex(style::foreground());
        assert_eq!(
            texts(&s),
            vec![
                ("Keyboard".into(), fg.clone()),
                ("100 %".into(), fg.clone()),
                ("Mouse".into(), fg.clone()),
                ("95 %".into(), fg.clone()),
                (CHARGING_MARK.into(), fg),
            ]
        );
        assert!(s.contains("width=\"220\" height=\"76\""));
    }

    /// Each text node's family, size, x and anchor.
    fn placed(svg: &str) -> Vec<(String, String, f32, f32, Option<String>)> {
        let doc = resvg::usvg::roxmltree::Document::parse(svg).unwrap();
        doc.descendants()
            .filter(|n| n.has_tag_name("text"))
            .map(|n| {
                (
                    n.text().unwrap_or("").to_string(),
                    n.attribute("font-family").unwrap().to_string(),
                    n.attribute("font-size").unwrap().parse().unwrap(),
                    n.attribute("x").unwrap().parse().unwrap(),
                    n.attribute("text-anchor").map(str::to_string),
                )
            })
            .collect()
    }

    #[test]
    fn labels_in_geo_percents_in_hack_in_one_right_aligned_column() {
        let devices = [
            dev("K", DeviceKind::Keyboard, Some(100), "discharging"),
            dev("M", DeviceKind::Mouse, Some(7), "charging"),
            dev("P", DeviceKind::Unknown, None, "unknown"),
        ];
        let now = Instant::now();
        for (room, st) in [
            (CELL, LARGE),
            (Room::one(vec2(220.0, TILE_H_MIN)), SMALL),
            (Room::one(vec2(160.0, TILE_H)), LARGE),
        ] {
            let t = placed(&Battery::default().svg(&data(&devices, now), room, now));
            let Columns {
                label_right,
                symbol_x,
                pct_right,
                mark_x,
            } = columns(room.size.x, &st);
            for (text, family, size, x, anchor) in &t {
                match text.as_str() {
                    "Keyboard" | "Mouse" | "P" => {
                        assert_eq!((family.as_str(), *size, *x), (GEO_FAMILY, st.title, st.pad_x));
                        assert_eq!(anchor, &None);
                    }
                    "100 %" | "7 %" | "–" => {
                        assert_eq!((family.as_str(), *size), (MONO_FAMILY, st.value), "{text}");
                        assert!((x - pct_right).abs() < 0.01, "{text}: one column");
                        assert_eq!(anchor.as_deref(), Some("end"));
                    }
                    "+" => {
                        assert_eq!((family.as_str(), *size), (MONO_FAMILY, st.value));
                        assert!((x - mark_x).abs() < 0.01);
                    }
                    other => panic!("unexpected text {other}"),
                }
            }
            // The mark ends at the padding; the widest percent clears the
            // label's clip.
            assert!((mark_x + mono_width("+", st.value) - (room.size.x - st.pad_x)).abs() < 0.01);
            assert!(label_right + st.gap <= symbol_x + 0.01);
            assert!(symbol_x + battery_width(st.value) + st.gap + mono_width("100 %", st.value) <= pct_right + 0.01);
        }
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
        let t = texts(&Battery::default().svg(&data(&devices, now), CELL, now));
        let red = hex(style::ALERT);
        let muted = hex(style::MUTED);
        let fg = hex(style::foreground());
        assert_eq!(
            t,
            vec![
                ("Keyboard".into(), red.clone()),
                ("9 %".into(), red.clone()),
                ("Mouse".into(), red.clone()),
                ("50 %".into(), red),
                ("Pen".into(), muted.clone()),
                ("–".into(), muted),
                ("Pad".into(), fg.clone()),
                ("–".into(), fg),
            ]
        );
    }

    #[test]
    fn device_names_are_escaped_and_long_ones_clipped_before_the_percent() {
        let long = "W".repeat(64);
        let devices = [
            dev("<b>&\"'", DeviceKind::Unknown, Some(50), "discharging"),
            dev(&long, DeviceKind::Unknown, Some(40), "discharging"),
        ];
        let now = Instant::now();
        let s = Battery::default().svg(&data(&devices, now), CELL, now);
        assert!(s.contains("&lt;b&gt;&amp;&quot;&apos;"), "{s}");
        assert!(!s.contains("<b>"));
        assert_eq!(escape("a&b"), "a&amp;b");
        let doc = resvg::usvg::roxmltree::Document::parse(&s).unwrap();
        let clip = doc
            .descendants()
            .find(|n| n.has_tag_name("clipPath"))
            .and_then(|c| c.first_element_child())
            .unwrap();
        let clip_right: f32 = clip.attribute("width").unwrap().parse().unwrap();
        let texts: Vec<_> = doc.descendants().filter(|n| n.has_tag_name("text")).collect();
        let label = texts.iter().find(|n| n.text() == Some(long.as_str())).unwrap();
        assert_eq!(label.text(), Some(long.as_str()), "the whole name, no ellipsis");
        assert_eq!(label.parent().unwrap().attribute("clip-path"), Some("url(#label)"));
        let pcts: Vec<_> = texts
            .iter()
            .filter(|n| n.attribute("text-anchor") == Some("end"))
            .map(|n| n.attribute("x").unwrap().parse::<f32>().unwrap())
            .collect();
        assert_eq!(pcts.len(), 2);
        assert_eq!(pcts[0], pcts[1], "the percents keep their column");
        let widest = mono_width("100 %", LARGE.value);
        assert!(clip_right + widest < pcts[0], "the clip ends before the widest percent");
        // Rendered: no label ink right of the clip.
        let img = super::super::render::render(&s, [220, 76]).unwrap();
        let fg = |p: &Color32| p.r() > 150 && p.g() > 150;
        let row = (76 / 2) as usize;
        let ink_cols: Vec<usize> = (0..220)
            .filter(|&x| (row - 8..row + 8).any(|y| fg(&img.pixels[y * 220 + x])))
            .collect();
        let symbol_x = columns(220.0, &LARGE).symbol_x;
        let gap = (clip_right as usize + 1..symbol_x as usize).all(|x| !ink_cols.contains(&x));
        assert!(gap, "blank between the cut label and the symbol: {ink_cols:?}");
    }

    fn five() -> Vec<Device> {
        ["A", "B", "C", "D", "E"]
            .iter()
            .map(|n| dev(&format!("Dev {n}"), DeviceKind::Unknown, Some(50), "discharging"))
            .collect()
    }

    fn labels(svg: &str) -> Vec<String> {
        texts(svg)
            .into_iter()
            .map(|(t, _)| t)
            .filter(|t| t.starts_with("Dev"))
            .collect()
    }

    #[test]
    fn four_lines_fit_the_cell_three_the_smallest_and_more_turn_page() {
        assert_eq!(per_page(TILE_H, &LARGE), 4);
        assert_eq!(per_page(TILE_H_MIN, &SMALL), 3);
        assert_eq!(per_page(5.0, &LARGE), 1);
        // Two rows of cells hold more.
        assert_eq!(per_page(2.0 * TILE_H + 3.0, &LARGE), 8);
        // The element follows its cell height.
        let small = Room::one(vec2(220.0, TILE_H_MIN));
        let devices = five();
        let t0 = Instant::now();
        assert_eq!(
            labels(&Battery::default().svg(&data(&devices, t0), small, t0)),
            ["Dev A", "Dev B", "Dev C"]
        );
        let devices = five();
        let t0 = Instant::now();
        let d = data(&devices, t0);
        let at = |ms| t0 + Duration::from_millis(ms);
        assert_eq!(
            labels(&Battery::default().svg(&d, CELL, at(0))),
            ["Dev A", "Dev B", "Dev C", "Dev D"]
        );
        assert_eq!(
            Battery::default().next_change(&d, CELL, at(1500)),
            Some(Duration::from_millis(2500))
        );
        assert_eq!(labels(&Battery::default().svg(&d, CELL, at(4000))), ["Dev E"]);
        assert_eq!(
            labels(&Battery::default().svg(&d, CELL, at(8000))),
            ["Dev A", "Dev B", "Dev C", "Dev D"]
        );
        // The same page is the same text: nothing to render again.
        assert_eq!(
            Battery::default().svg(&d, CELL, at(100)),
            Battery::default().svg(&d, CELL, at(3900))
        );
        // Lines that fit do not page.
        let four = &devices[..4];
        assert_eq!(Battery::default().next_change(&data(four, t0), CELL, at(1500)), None);
        assert_eq!(
            labels(&Battery::default().svg(&data(four, t0), CELL, at(4000))).len(),
            4
        );
    }

    #[test]
    fn page_every_comes_from_the_file() {
        let b = |v: &str| {
            let e = crate::desktop::parse_group(&format!("[G]\nPageEvery={v}\n"), "[G]").unwrap();
            build(&e).map(|_| ())
        };
        assert_eq!(b("2"), Ok(()));
        assert_eq!(b("60s"), Ok(()));
        for bad in ["0", "61", "+3", "x", ""] {
            assert_eq!(b(bad), Err(format!("PageEvery must be 1 to 60 seconds, not '{bad}'")));
        }
        let fast = Battery {
            page_every: Duration::from_secs(2),
        };
        let devices = five();
        let t0 = Instant::now();
        let d = data(&devices, t0);
        assert_eq!(labels(&fast.svg(&d, CELL, t0 + Duration::from_secs(2))), ["Dev E"]);
        assert_eq!(
            fast.next_change(&d, CELL, t0 + Duration::from_millis(500)),
            Some(Duration::from_millis(1500))
        );
    }

    /// Each symbol's label, and the widths of its outline and of its fill.
    fn symbols(svg: &str) -> Vec<(String, f32, Option<f32>, bool)> {
        let doc = resvg::usvg::roxmltree::Document::parse(svg).unwrap();
        doc.descendants()
            .filter(|n| n.attribute("class") == Some("battery"))
            .map(|g| {
                let part = |class: &str| g.descendants().find(|n| n.attribute("class") == Some(class));
                let width = |n: resvg::usvg::roxmltree::Node| n.attribute("width").unwrap().parse::<f32>().unwrap();
                let outline = part("outline").expect("an outline");
                (
                    g.attribute("aria-label").unwrap().to_string(),
                    width(outline),
                    part("level").map(width),
                    outline.attribute("stroke-dasharray").is_some(),
                )
            })
            .collect()
    }

    #[test]
    fn each_line_has_a_symbol_filled_to_its_level() {
        let devices = [
            dev("K", DeviceKind::Keyboard, Some(100), "discharging"),
            dev("M", DeviceKind::Mouse, Some(50), "charging"),
            dev("N", DeviceKind::Numpad, Some(1), "discharging"),
            dev("P", DeviceKind::Presenter, Some(0), "discharging"),
        ];
        let now = Instant::now();
        let s = Battery::default().svg(&data(&devices, now), CELL, now);
        let got = symbols(&s);
        assert_eq!(got.len(), 4, "{s}");
        let full = got[0].2.unwrap();
        assert_eq!(got[0].0, "100");
        assert!(
            full > 20.0 && full < got[0].1,
            "the fill sits inside the outline: {got:?}"
        );
        assert_eq!(got[1].2.unwrap(), (full * 0.5).round());
        assert_eq!(got[2].2.unwrap(), 1.0, "any charge shows: {got:?}");
        assert_eq!(got[3].2, None, "0 % is an empty outline");
        assert!(got.iter().all(|g| !g.3), "known levels have a solid outline");
    }

    #[test]
    fn an_unreachable_or_unknown_device_has_a_dashed_empty_symbol_and_a_dash() {
        let devices = [
            dev("K", DeviceKind::Keyboard, Some(80), "not reachable"),
            dev("M", DeviceKind::Mouse, None, "unknown"),
        ];
        let now = Instant::now();
        let s = Battery::default().svg(&data(&devices, now), CELL, now);
        let got = symbols(&s);
        assert_eq!(got.len(), 2, "{s}");
        for g in &got {
            assert_eq!(g.0, "unknown");
            assert_eq!(g.2, None, "no fill");
            assert!(g.3, "dashed");
        }
        // The first is muted as a whole, the second keeps the normal colour.
        let doc = resvg::usvg::roxmltree::Document::parse(&s).unwrap();
        let strokes: Vec<&str> = doc
            .descendants()
            .filter(|n| n.attribute("class") == Some("outline"))
            .map(|n| n.attribute("stroke").unwrap())
            .collect();
        assert_eq!(strokes, [hex(style::MUTED), hex(style::foreground())]);
        assert_eq!(texts(&s).iter().filter(|(t, _)| t == "–").count(), 2);
    }

    #[test]
    fn the_symbol_takes_the_colour_of_its_line_and_draws_ink() {
        let devices = [dev("K", DeviceKind::Keyboard, Some(7), "discharging")];
        let now = Instant::now();
        let s = Battery::default().svg(&data(&devices, now), CELL, now);
        let img = super::super::render::render(&s, [220, 76]).unwrap();
        let c = columns(220.0, &LARGE);
        let reds = |x0: usize, x1: usize| {
            (x0..x1)
                .flat_map(|x| (0..76).map(move |y| (x, y)))
                .filter(|&(x, y)| {
                    let p = img.pixels[y * 220 + x];
                    p.r() > 180 && p.g() < 150 && p.b() < 150
                })
                .count()
        };
        let sym = c.symbol_x as usize;
        assert!(
            reds(sym, sym + battery_width(LARGE.value) as usize) > 40,
            "a red outline and fill"
        );
    }

    /// The ink boxes of the texts and the symbols of one rendered element.
    fn boxes(svg: &str) -> Vec<(String, resvg::usvg::Rect)> {
        let opt = resvg::usvg::Options {
            fontdb: super::super::render::fonts(),
            font_family: super::super::render::SANS_FAMILY.into(),
            ..Default::default()
        };
        let tree = resvg::usvg::Tree::from_str(svg, &opt).unwrap();
        let mut out = Vec::new();
        fn walk(g: &resvg::usvg::Group, out: &mut Vec<(String, resvg::usvg::Rect)>) {
            for n in g.children() {
                match n {
                    resvg::usvg::Node::Group(c) if c.id().starts_with("symbol") => {
                        out.push((c.id().to_string(), c.abs_bounding_box()));
                    }
                    resvg::usvg::Node::Group(c) => walk(c, out),
                    resvg::usvg::Node::Text(t) => out.push(("text".into(), t.flattened().abs_bounding_box())),
                    _ => {}
                }
            }
        }
        walk(tree.root(), &mut out);
        out
    }

    #[test]
    fn three_devices_fit_without_paging_or_overlap_at_both_cell_heights() {
        let devices = [
            dev("MX Mechanical", DeviceKind::Keyboard, Some(100), "charging"),
            dev("MX Anywhere 3S", DeviceKind::Mouse, Some(95), "charging"),
            dev("R", DeviceKind::Trackball, None, "not reachable"),
        ];
        let now = Instant::now();
        for (h, st) in [(TILE_H, LARGE), (TILE_H_MIN, SMALL)] {
            let room = Room::one(vec2(220.0, h));
            let b = Battery::default();
            let d = data(&devices, now);
            assert_eq!(b.next_change(&d, room, now), None, "{h}: no paging");
            assert!(per_page(h, &st) >= 3);
            let s = b.svg(&d, room, now);
            assert_eq!(symbols(&s).len(), 3, "{h}");
            let got = boxes(&s);
            assert_eq!(
                got.iter().filter(|(t, _)| t.starts_with("symbol")).count(),
                3,
                "{h}: {got:?}"
            );
            for (i, (ta, a)) in got.iter().enumerate() {
                assert!(
                    a.left() >= 0.0 && a.top() >= 0.0 && a.right() <= room.size.x && a.bottom() <= room.size.y,
                    "{h}: {ta} {a:?}"
                );
                for (tb, c) in &got[i + 1..] {
                    let hit =
                        a.left() < c.right() && c.left() < a.right() && a.top() < c.bottom() && c.top() < a.bottom();
                    assert!(!hit, "{h}: {ta} {a:?} overlaps {tb} {c:?}");
                }
            }
        }
    }

    #[test]
    fn shown_only_with_a_device() {
        let now = Instant::now();
        assert!(!Battery::default().visible(&data(&[], now)));
        assert!(Battery::default().visible(&data(&five(), now)));
    }

    #[test]
    fn the_svg_renders_with_the_real_fonts() {
        let devices = [
            dev("MX Mechanical", DeviceKind::Keyboard, Some(100), "discharging"),
            dev("Pen", DeviceKind::Unknown, Some(7), "discharging"),
        ];
        let now = Instant::now();
        let img =
            super::super::render::render(&Battery::default().svg(&data(&devices, now), CELL, now), [440, 152]).unwrap();
        assert_eq!(img.size, [440, 152]);
        let red = img
            .pixels
            .iter()
            .filter(|p| p.r() > 200 && p.g() < 150 && p.b() < 150)
            .count();
        let light = img
            .pixels
            .iter()
            .filter(|p| p.r() > 180 && p.g() > 180 && p.b() > 180)
            .count();
        assert!(red > 50, "the 7 % line draws red ink: {red}");
        assert!(light > 50, "the keyboard line draws light ink: {light}");
        // The background is the tile colour, opaque.
        assert_eq!(img.pixels[0], style::BACKGROUND);
        // The dash and the charging mark have glyphs in Hack.
        let glyph = |c: &str| {
            let svg = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\">\
                 <text x=\"2\" y=\"15\" font-family=\"{MONO_FAMILY}\" font-size=\"14\" fill=\"#fff\">{c}</text></svg>"
            );
            super::super::render::render(&svg, [20, 20]).unwrap().pixels
        };
        for c in ["–", CHARGING_MARK, "%"] {
            assert_ne!(glyph(c), glyph("\u{e000}"), "{c} has a glyph");
        }
    }
}
