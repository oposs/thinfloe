//! The network element (`Type=network`, shipped 2 x 2 right of the battery).
//! A graph of the received (filled) and sent (line) rate over the last five
//! minutes across the element's full width, with a rate axis (two to four
//! round ticks with k/M/G, the unit `bit/s` above them), a time axis (`-5m` to
//! `now`), light grid lines and a legend. The numbers are in the list beside
//! it (`network_info`). Sizes, fonts and colours come from `style`. Its data is a `netstat::Snapshot` the menu copies
//! from the sampler thread; `Refresh=1` draws it every second while the
//! menu shows.
//!
//! In a single row of cells (`ShortSize=3x1`, on a short screen) it is the
//! graph alone, edge to edge across the element: grid and curves, with the
//! rate labels (none for 0), the unit and the legend inside the plot.

use super::style::{self, Anchor, DOWN, Face, Style, UP, crisp, hex, mono_width};
use super::{Element, Kind, Room, Sources};
use crate::desktop::Entry;
use crate::netstat::{Snapshot, WINDOW};
use std::fmt::Write;
use std::time::{Duration, Instant};

pub const KIND: Kind = Kind {
    name: "network",
    options: &[],
    build,
};

fn build(_: &Entry) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(Network))
}

pub struct Network;

/// The unit of the rate axis, above its ticks.
pub const UNIT: &str = "bit/s";
/// Room kept for the widest tick label (`1.5k`, `500M`), so the plot does
/// not move when the scale changes.
const TICK_CHARS: &str = "1.5k";

/// The plot inside an element: the rectangle the curves are drawn in, in
/// points. Its left edge is the rate axis, its bottom the time axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plot {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// Where everything goes in `room`: the plot, the baseline of the header
/// (unit and legend) and of the time labels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub plot: Plot,
    pub header: f32,
    pub ticks: f32,
    /// One row: only the plot, with unit and legend inside it. `ticks` is
    /// not used then.
    pub graph_only: bool,
}

/// The space between an axis and its labels, and the length of a tick mark.
fn tick_gap(st: &Style) -> f32 {
    0.4 * st.small
}

/// The baseline of a label inside the plot, just under the grid line at `y`.
fn under_line(y: f32, st: &Style) -> f32 {
    y + tick_gap(st) + 0.75 * st.small
}

/// A backing under a label inside the plot, so a curve does not run through it.
fn backing(s: &mut String, x: f32, baseline: f32, width: f32, st: &Style) {
    let g = tick_gap(st);
    let _ = write!(
        s,
        "<rect class=\"legend\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\" fill-opacity=\"0.8\"/>",
        x - 0.5 * g,
        baseline - 0.9 * st.small,
        width + g,
        1.1 * st.small,
        hex(style::BACKGROUND),
    );
}

pub fn frame(room: Room) -> Frame {
    let st = room.style();
    let (w, h) = (room.size.x, room.size.y);
    let left = st.pad_x + mono_width(TICK_CHARS, st.small) + 2.0 * tick_gap(&st);
    if h < 2.0 * room.cell.y {
        let top = crisp(0.0);
        return Frame {
            plot: Plot {
                left: crisp(0.0),
                top,
                right: crisp(w - 1.0),
                bottom: crisp(h - 1.0),
            },
            header: under_line(top, &st),
            ticks: h,
            graph_only: true,
        };
    }
    let header_top = st.pad_y;
    let plot_top = header_top + st.small_line + st.gap;
    let plot_bottom = h - st.pad_y - st.small_line - tick_gap(&st);
    Frame {
        plot: Plot {
            left: crisp(left),
            top: crisp(plot_top),
            right: crisp(w - st.pad_x),
            bottom: crisp(plot_bottom.max(plot_top + 1.0)),
        },
        header: style::baseline(header_top, st.small_line, st.small),
        // Its descent stays inside the bottom padding.
        ticks: style::baseline(plot_bottom + tick_gap(&st), st.small_line, st.small)
            .min(h - st.pad_y - 0.25 * st.small),
        graph_only: false,
    }
}

/// The graph's points in `plot`: the newest sample at the right edge, one
/// step per second, five minutes across, the top at `top` bit/s.
pub fn points(values: &[f64], plot: &Plot, top: f64) -> Vec<(f32, f32)> {
    let n = values.len().min(WINDOW);
    let step = (plot.right - plot.left) / WINDOW as f32;
    values[values.len() - n..]
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = plot.right - (n - 1 - i) as f32 * step;
            let frac = (v.max(0.0) / top).min(1.0) as f32;
            (x, plot.bottom - frac * (plot.bottom - plot.top))
        })
        .collect()
}

fn path(points: &[(f32, f32)]) -> String {
    let mut s = String::new();
    for (x, y) in points {
        let _ = write!(s, "{x:.1},{y:.1} ");
    }
    s.trim_end().to_string()
}

fn line(s: &mut String, class: &str, (x1, y1): (f32, f32), (x2, y2): (f32, f32), colour: egui::Color32) {
    let _ = write!(
        s,
        "<line class=\"{class}\" x1=\"{x1}\" y1=\"{y1}\" x2=\"{x2}\" y2=\"{y2}\" stroke=\"{}\" stroke-width=\"1\"/>",
        hex(colour)
    );
}

/// The SVG of `snap` in `room`.
pub fn svg(snap: &Snapshot, room: Room) -> String {
    let st = room.style();
    let (w, h) = (room.size.x, room.size.y);
    let f = frame(room);
    let p = f.plot;
    let g = tick_gap(&st);
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\
         <rect width=\"{w}\" height=\"{h}\" fill=\"{}\"/>\
         <clipPath id=\"plot\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath>",
        hex(style::BACKGROUND),
        p.left,
        p.top - 1.0,
        p.right - p.left,
        p.bottom - p.top + 1.0,
    );
    let (prx, ptx) = snap.peak();
    let ticks = style::rate_ticks(prx.max(ptx));
    let top = *ticks.last().unwrap_or(&style::MIN_TOP);
    let y_of = |v: f64| p.bottom - (v / top) as f32 * (p.bottom - p.top);
    let minutes = style::minute_ticks(p.left, p.right - p.left);
    // The grid, under everything.
    for &v in &ticks[1..] {
        let y = crisp(y_of(v));
        line(&mut s, "grid", (p.left, y), (p.right, y), style::GRID);
    }
    for &(x, _) in &minutes[1..] {
        let x = crisp(x);
        line(&mut s, "grid", (x, p.top), (x, p.bottom), style::GRID);
    }
    // The curves: received as an area, sent as a line over it.
    let rx = points(&snap.rx, &p, top);
    let tx = points(&snap.tx, &p, top);
    s.push_str("<g clip-path=\"url(#plot)\">");
    if let (Some(first), Some(last)) = (rx.first(), rx.last()) {
        let _ = write!(
            s,
            "<polygon id=\"rx-area\" points=\"{:.1},{b} {} {:.1},{b}\" fill=\"{c}\" fill-opacity=\"0.25\"/>\
             <polyline id=\"rx\" points=\"{}\" fill=\"none\" stroke=\"{c}\" stroke-width=\"1.25\" stroke-linejoin=\"round\"/>",
            first.0,
            path(&rx),
            last.0,
            path(&rx),
            b = p.bottom,
            c = hex(style::RX),
        );
    }
    if !tx.is_empty() {
        let _ = write!(
            s,
            "<polyline id=\"tx\" points=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1.5\" stroke-linejoin=\"round\"/>",
            path(&tx),
            hex(style::TX),
        );
    }
    s.push_str("</g>");
    // The axes with their tick marks and labels.
    line(&mut s, "axis", (p.left, p.top), (p.left, p.bottom), style::AXIS);
    line(&mut s, "axis", (p.left, p.bottom), (p.right, p.bottom), style::AXIS);
    for &v in &ticks {
        let y = crisp(y_of(v));
        let text = style::tick_label(v);
        if f.graph_only {
            // Under its grid line. The bottom edge says 0 by itself.
            if v == 0.0 {
                continue;
            }
            let ty = under_line(y, &st);
            let x = p.left + 2.0 * g;
            backing(&mut s, x, ty, mono_width(&text, st.small), &st);
            style::text(&mut s, x, ty, Face::Value, st.small, style::MUTED, Anchor::Start, &text);
            continue;
        }
        line(&mut s, "tick", (p.left - g, y), (p.left, y), style::AXIS);
        let ty = y + 0.36 * st.small;
        style::text(
            &mut s,
            p.left - 1.5 * g,
            ty,
            Face::Value,
            st.small,
            style::MUTED,
            Anchor::End,
            &text,
        );
    }
    for (i, &(x, text)) in minutes.iter().enumerate() {
        if f.graph_only {
            break;
        }
        let x = crisp(x);
        line(&mut s, "tick", (x, p.bottom), (x, p.bottom + g), style::AXIS);
        let anchor = match i {
            0 => Anchor::Start,
            5 => Anchor::End,
            _ => Anchor::Middle,
        };
        let tx = match anchor {
            Anchor::Start => x - 0.5,
            Anchor::End => x + 0.5,
            Anchor::Middle => x,
        };
        style::text(&mut s, tx, f.ticks, Face::Value, st.small, style::MUTED, anchor, text);
    }
    // The header: the unit over the rate axis, the legend at the right; in
    // one row both inside the plot, on a backing over the curves.
    let ch = mono_width("x", st.small);
    let (unit_x, legend_x) = if f.graph_only {
        // The unit right of the rate labels, on the top label's line.
        (
            p.left + 2.0 * g + mono_width(TICK_CHARS, st.small) + 2.0 * g,
            p.right - 2.0 * g - 10.0 * ch,
        )
    } else {
        (st.pad_x, p.right - 10.0 * ch)
    };
    if f.graph_only {
        backing(&mut s, unit_x, f.header, mono_width(UNIT, st.small), &st);
        backing(&mut s, legend_x, f.header, 10.0 * ch, &st);
    }
    style::text(
        &mut s,
        unit_x,
        f.header,
        Face::Value,
        st.small,
        style::MUTED,
        Anchor::Start,
        UNIT,
    );
    // `↓ rx  ↑ tx`: arrow, a space, the name; two spaces between.
    let mut x = legend_x;
    for (which, name, colour) in [(DOWN, "rx", style::RX), (UP, "tx", style::TX)] {
        style::draw_arrow(&mut s, x, f.header, st.small, which, colour);
        style::text(
            &mut s,
            x + 2.0 * ch,
            f.header,
            Face::Value,
            st.small,
            colour,
            Anchor::Start,
            name,
        );
        x += 6.0 * ch;
    }
    s.push_str("</svg>");
    s
}

impl Element for Network {
    fn visible(&self, data: &Sources) -> bool {
        data.network.is_some_and(Snapshot::visible)
    }

    fn svg(&self, data: &Sources, room: Room, _: Instant) -> String {
        data.network.map_or_else(String::new, |snap| svg(snap, room))
    }

    fn next_change(&self, _: &Sources, _: Room, _: Instant) -> Option<Duration> {
        // Its file's Refresh=1 draws it every second while the menu shows.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netstat::{Info, Link, Vpn};
    use crate::status::Cells;
    use crate::status::render::{self, MONO_FAMILY};
    use crate::status::style::{LARGE, SMALL};
    use crate::ui::{TILE_GAP, TILE_H, TILE_H_MIN};
    use egui::{Color32, vec2};
    use std::net::Ipv4Addr;

    /// The shipped room: two cells of 220 by two of 76, gaps of 3.
    fn shipped(cell_h: f32) -> Room {
        Cells::parse("1,0,2,2").unwrap().room(vec2(220.0, cell_h), TILE_GAP)
    }

    fn snap() -> Snapshot {
        Snapshot {
            rx: vec![0.0, 4000.0, 8000.0],
            tx: vec![1000.0, 2000.0, 0.0],
            total: (1_234_000_000, 45_600_000),
            info: Info {
                iface: Some("eth0".into()),
                default_route: true,
                link: Link::Wired(Some(1000)),
                ipv4: Some((Ipv4Addr::new(192, 168, 1, 23), 24)),
                gateway: Some(Ipv4Addr::new(192, 168, 1, 1)),
                dns: vec!["192.168.1.1".into()],
                vpn: Vpn::Up,
                addressed: true,
            },
        }
    }

    struct Text {
        text: String,
        family: String,
        size: f32,
    }

    fn texts(svg: &str) -> Vec<Text> {
        let doc = resvg::usvg::roxmltree::Document::parse(svg).expect("well-formed SVG");
        doc.descendants()
            .filter(|n| n.has_tag_name("text"))
            .map(|n| Text {
                text: n.text().unwrap_or("").to_string(),
                family: n.attribute("font-family").unwrap().into(),
                size: n.attribute("font-size").unwrap().parse().unwrap(),
            })
            .collect()
    }

    #[test]
    fn the_rate_axis_has_round_ticks_the_time_axis_minutes() {
        let room = shipped(TILE_H);
        let s = svg(&snap(), room);
        let t = texts(&s);
        let small: Vec<&str> = t
            .iter()
            .filter(|t| t.size == LARGE.small)
            .map(|t| t.text.as_str())
            .collect();
        // Peak 8 kbit/s: 0, 5k, 10k; then the minutes, the unit and legend.
        assert_eq!(
            small,
            [
                "0", "5k", "10k", "-5m", "-4m", "-3m", "-2m", "-1m", "now", UNIT, "rx", "tx"
            ]
        );
        // The legend's arrows stand on the header's baseline, its last
        // letter ends at the plot's right edge.
        let f = frame(room);
        for mark in [DOWN, UP] {
            let tag = format!("aria-label=\"{mark}\" transform=\"translate(");
            assert_eq!(s.matches(&tag).count(), 1, "in the legend, the only arrows");
        }
        assert!(s.contains(&format!(" {:.2})\"><path", f.header)));
        let doc = resvg::usvg::roxmltree::Document::parse(&s).unwrap();
        let tx = doc.descendants().find(|n| n.text() == Some("tx")).unwrap();
        let tx_x: f32 = tx.attribute("x").unwrap().parse().unwrap();
        assert!((tx_x + mono_width("tx", LARGE.small) - f.plot.right).abs() < 0.01);
        assert!(
            t.iter()
                .filter(|t| t.size == LARGE.small)
                .all(|t| t.family == MONO_FAMILY)
        );
        let doc = resvg::usvg::roxmltree::Document::parse(&s).unwrap();
        let count = |class: &str| {
            doc.descendants()
                .filter(|n| n.attribute("class") == Some(class))
                .count()
        };
        // A grid line per tick but 0, and per minute but the axis.
        assert_eq!(count("grid"), 2 + 5);
        assert_eq!(count("axis"), 2);
        assert_eq!(count("tick"), 3 + 6);
        // The time labels sit below the plot and above the text.
        let f = frame(room);
        assert!(f.ticks > f.plot.bottom + LARGE.small * 0.7 && f.ticks <= room.size.y - LARGE.pad_y + 0.01);
        assert!(f.header < f.plot.top);
    }

    /// One row of three cells: the form on a short screen.
    fn one_row(cell_h: f32) -> Room {
        Cells::parse("1,0,3,1").unwrap().room(vec2(220.0, cell_h), TILE_GAP)
    }

    #[test]
    fn one_row_is_the_graph_alone_with_its_legend_inside() {
        let mut busy = snap();
        busy.rx = (0..WINDOW).map(|i| (i * 40_000) as f64).collect();
        for (h, st) in [(TILE_H, LARGE), (TILE_H_MIN, SMALL)] {
            let room = one_row(h);
            let f = frame(room);
            let p = f.plot;
            assert!(f.graph_only, "{h}");
            assert!(!frame(shipped(h)).graph_only, "{h}: two rows keep the text");
            // Edge to edge: the whole element is the plot.
            assert!(p.left < 1.0 && p.top < 1.0, "{h}: {p:?}");
            assert!(
                p.right >= room.size.x - 1.5 && p.bottom >= room.size.y - 1.5,
                "{h}: {p:?}"
            );
            for v in [p.left, p.top, p.right, p.bottom] {
                assert_eq!(v.fract(), 0.5, "{p:?}");
            }
            let s = svg(&busy, room);
            assert!(s.contains("id=\"rx\"") && s.contains("id=\"tx\""));
            // The rate axis, the unit and the legend; no time labels and
            // neither text line.
            let words: Vec<String> = texts(&s).into_iter().map(|t| t.text).collect();
            for gone in ["-5m", "now", "eth0", "max", "in", "out", "0"] {
                assert!(!words.iter().any(|w| w == gone), "{h}: {gone} in {words:?}");
            }
            for kept in [UNIT, "rx", "tx"] {
                assert!(words.iter().any(|w| w == kept), "{h}: {kept} not in {words:?}");
            }
            assert!(texts(&s).iter().all(|t| t.size == st.small), "{h}: {s}");
            // Every label on a backing over the curves, so a full graph does
            // not run through them: 5M, 10M, 15M, the unit and the legend.
            let curve = s.find("id=\"tx\"").unwrap();
            let backing = s.find("class=\"legend\"").expect("a backing");
            assert!(backing > curve, "{h}");
            assert_eq!(s.matches("class=\"legend\"").count(), 5);
            assert_eq!(s.matches("class=\"tick\"").count(), 0, "no room outside the plot");
            // Everything inside the element and nothing on top of anything.
            let b = ink_boxes(&s);
            for (i, (ta, a)) in b.iter().enumerate() {
                assert!(
                    a.left() >= 0.0 && a.top() >= 0.0 && a.bottom() <= room.size.y && a.right() <= room.size.x,
                    "{h}: {ta} {a:?} outside {room:?}"
                );
                assert!(
                    a.left() > p.left && a.top() > p.top && a.bottom() < p.bottom,
                    "{h}: {ta} {a:?} {p:?}"
                );
                for (tb, c) in &b[i + 1..] {
                    let hit =
                        a.left() < c.right() && c.left() < a.right() && a.top() < c.bottom() && c.top() < a.bottom();
                    assert!(!hit, "{h}: {ta} {a:?} overlaps {tb} {c:?}");
                }
            }
        }
    }

    #[test]
    fn two_by_two_keeps_every_axis_label_and_the_legend_at_its_narrower_width() {
        for h in [TILE_H, TILE_H_MIN] {
            let room = shipped(h);
            assert_eq!(room.size.x, 443.0);
            assert!(!frame(room).graph_only, "{h}");
            let words: Vec<String> = texts(&svg(&snap(), room)).into_iter().map(|t| t.text).collect();
            for want in ["-5m", "-4m", "-3m", "-2m", "-1m", "now", UNIT, "rx", "tx"] {
                assert!(words.iter().any(|w| w == want), "{h}: {want} in {words:?}");
            }
            // The two text lines have left it.
            for gone in ["eth0", "max", "gw", "DNS", "VPN", "since"] {
                assert!(!words.iter().any(|w| w == gone), "{h}: {gone} in {words:?}");
            }
        }
    }

    #[test]
    fn two_rows_keep_the_zero_label() {
        let words: Vec<String> = texts(&svg(&snap(), shipped(TILE_H)))
            .into_iter()
            .map(|t| t.text)
            .collect();
        assert!(words.iter().any(|w| w == "0"), "{words:?}");
    }

    #[test]
    fn the_frame_follows_the_cell_height() {
        for (h, st) in [(TILE_H, LARGE), (TILE_H_MIN, SMALL)] {
            let room = shipped(h);
            let f = frame(room);
            let p = f.plot;
            // Full width: from the tick labels to the right padding.
            assert!(
                p.left < st.pad_x + 40.0 && (p.right - (room.size.x - st.pad_x)).abs() < 1.0,
                "{p:?}"
            );
            assert!(p.bottom - p.top > 50.0, "{h}: a tall plot: {p:?}");
            // Lines on half points, so one-point lines are sharp.
            for v in [p.left, p.top, p.right, p.bottom] {
                assert_eq!(v.fract(), 0.5, "{p:?}");
            }
            // The time labels end above the bottom padding: no text line
            // takes room any more.
            assert!(f.ticks + 0.25 * st.small <= room.size.y - st.pad_y + 0.01);
        }
    }

    #[test]
    fn the_graph_is_scaled_to_the_top_tick_newest_at_the_right() {
        let room = shipped(TILE_H);
        let p = frame(room).plot;
        let s = svg(&snap(), room);
        let polyline = |id: &str| -> Vec<(f32, f32)> {
            let doc = resvg::usvg::roxmltree::Document::parse(&s).unwrap();
            let n = doc.descendants().find(|n| n.attribute("id") == Some(id)).unwrap();
            n.attribute("points")
                .unwrap()
                .split_whitespace()
                .map(|q| {
                    let (x, y) = q.split_once(',').unwrap();
                    (x.parse().unwrap(), y.parse().unwrap())
                })
                .collect()
        };
        let rx = polyline("rx");
        let tx = polyline("tx");
        assert_eq!(rx.len(), 3);
        let height = p.bottom - p.top;
        // The peak 8 kbit/s at 8/10 of the 10k top, 0 at the bottom.
        assert!((rx[2].1 - (p.bottom - 0.8 * height)).abs() < 0.1, "{rx:?}");
        assert!((rx[0].1 - p.bottom).abs() < 0.1);
        assert!((tx[1].1 - (p.bottom - 0.2 * height)).abs() < 0.1);
        // The newest at the right edge, one step per second, five minutes
        // across the plot.
        let step = (p.right - p.left) / WINDOW as f32;
        assert!((rx[2].0 - p.right).abs() < 0.1);
        assert!((rx[2].0 - rx[1].0 - step).abs() < 0.1);
        let full = points(&vec![1.0; WINDOW], &p, 1.0);
        assert!((full[0].0 - (p.left + step)).abs() < 0.01);
        // A quiet line stays near the bottom: the top is at least 1 kbit/s.
        let quiet = Snapshot {
            rx: vec![10.0, 20.0],
            tx: vec![0.0, 0.0],
            ..snap()
        };
        let q = svg(&quiet, room);
        assert!(texts(&q).iter().any(|t| t.text == "1k"));
        // No sample yet: no curve, the axes are still there.
        let empty = Snapshot {
            rx: vec![],
            tx: vec![],
            ..snap()
        };
        let e = svg(&empty, room);
        assert!(!e.contains("polyline"));
        assert!(texts(&e).iter().any(|t| t.text == "now"));
    }

    fn sources(snap: Option<&Snapshot>) -> Sources<'_> {
        Sources {
            battery: &[],
            battery_since: None,
            network: snap,
            network_since: None,
            hardware: None,
        }
    }

    #[test]
    fn hidden_without_a_route_or_an_address() {
        let now = Instant::now();
        assert!(Network.visible(&sources(Some(&snap()))));
        assert!(!Network.visible(&sources(None)), "no sampler: nothing");
        let mut none = snap();
        none.info = Info::default();
        assert!(!Network.visible(&sources(Some(&none))));
        none.info.addressed = true;
        assert!(Network.visible(&sources(Some(&none))), "an address without a route");
        assert_eq!(Network.next_change(&sources(Some(&snap())), shipped(TILE_H), now), None);
    }

    /// The ink boxes of every text, laid out by resvg with the real fonts.
    fn ink_boxes(svg: &str) -> Vec<(String, resvg::usvg::Rect)> {
        let opt = resvg::usvg::Options {
            fontdb: render::fonts(),
            font_family: render::SANS_FAMILY.into(),
            ..Default::default()
        };
        let tree = resvg::usvg::Tree::from_str(svg, &opt).unwrap();
        let doc = resvg::usvg::roxmltree::Document::parse(svg).unwrap();
        let bodies: Vec<String> = doc
            .descendants()
            .filter(|n| n.has_tag_name("text"))
            .map(|n| n.text().unwrap_or("").to_string())
            .collect();
        let mut out = Vec::new();
        fn walk(g: &resvg::usvg::Group, out: &mut Vec<resvg::usvg::Rect>) {
            for n in g.children() {
                match n {
                    resvg::usvg::Node::Group(g) => walk(g, out),
                    resvg::usvg::Node::Text(t) => out.push(t.flattened().abs_bounding_box()),
                    _ => {}
                }
            }
        }
        let mut boxes = Vec::new();
        walk(tree.root(), &mut boxes);
        assert_eq!(boxes.len(), bodies.len());
        for (b, t) in boxes.into_iter().zip(bodies) {
            out.push((t, b));
        }
        out
    }

    #[test]
    fn no_text_overlaps_another_or_leaves_the_element() {
        let mut busy = snap();
        busy.rx = (0..WINDOW).map(|i| (i * 40_000) as f64).collect();
        for room in [
            shipped(TILE_H),
            shipped(TILE_H_MIN),
            shipped(70.0),
            Room {
                size: vec2(300.0, 131.0),
                cell: vec2(100.0, 64.0),
            },
        ] {
            let p = frame(room).plot;
            let b = ink_boxes(&svg(&busy, room));
            for (i, (ta, a)) in b.iter().enumerate() {
                assert!(
                    a.left() >= 0.0 && a.top() >= 0.0 && a.bottom() <= room.size.y && a.right() <= room.size.x,
                    "{ta} {a:?} outside {room:?}"
                );
                // No text in the plot.
                let inside = a.left() < p.right && a.right() > p.left && a.top() < p.bottom && a.bottom() > p.top;
                assert!(!inside, "{ta} {a:?} in the plot {p:?}");
                for (tb, c) in &b[i + 1..] {
                    let hit =
                        a.left() < c.right() && c.left() < a.right() && a.top() < c.bottom() && c.top() < a.bottom();
                    assert!(!hit, "{ta} {a:?} overlaps {tb} {c:?} in {room:?}");
                }
            }
        }
    }

    #[test]
    fn the_svg_renders_both_curves_the_grid_and_the_arrows() {
        let room = shipped(TILE_H);
        let p = frame(room).plot;
        let s = svg(&snap(), room);
        let img = render::render(&s, [443, 155]).unwrap();
        let near = |q: &Color32, c: Color32| {
            (q.r() as i32 - c.r() as i32).abs() < 40
                && (q.g() as i32 - c.g() as i32).abs() < 40
                && (q.b() as i32 - c.b() as i32).abs() < 40
        };
        let in_plot = |c| {
            (p.top as usize..p.bottom as usize)
                .flat_map(|y| (p.left as usize..p.right as usize).map(move |x| (x, y)))
                .filter(|&(x, y)| near(&img.pixels[y * 443 + x], c))
                .count()
        };
        assert!(in_plot(style::RX) > 5, "the received curve draws");
        assert!(in_plot(style::TX) > 5, "the sent curve draws");
        // A grid line one pixel wide, exactly its colour.
        let mid_y = ((p.top + p.bottom) / 2.0) as usize;
        let grid_x = crisp(style::minute_ticks(p.left, p.right - p.left)[1].0) as usize;
        assert_eq!(img.pixels[mid_y * 443 + grid_x], style::GRID);
        assert_eq!(img.pixels[0], style::BACKGROUND);
        // The arrows have glyphs in Hack.
        let glyph = |c: char| {
            let svg = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\">\
                 <text x=\"2\" y=\"15\" font-family=\"{MONO_FAMILY}\" font-size=\"14\" fill=\"#fff\">{c}</text></svg>"
            );
            render::render(&svg, [20, 20]).unwrap().pixels
        };
        for c in ['↓', '↑', '·'] {
            assert_ne!(glyph(c), glyph('\u{e000}'), "{c} has a glyph");
        }
    }
}
