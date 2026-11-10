//! The network graph element: the received (filled) and sent (line) rate over
//! the last five minutes across the element's full width, with a rate axis
//! (two to four round ticks with k/M/G, the unit `bit/s` above them), a time
//! axis (`-5m` to `now`), light grid lines and a legend. The numbers are in
//! the list beside it (`network_info`). Its data is a `netstat::Snapshot` the
//! menu copies from the sampler thread; it is drawn again every second while
//! the menu shows.
//!
//! In a single row of cells (`3x1`, its size on a short screen, or `2x1`) it
//! is the graph alone, edge to edge across the element: grid and curves, with
//! the rate labels (none for 0), the unit and the legend inside the plot, each
//! on a backing so a curve does not run through it.

use super::{Data, Element, Size, Source, background, cell_height, sizes_for};
use crate::brand::{StatusSizes, role};
use crate::netstat::{Snapshot, WINDOW};
use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, pos2};
use shadcn_egui::Theme;
use std::time::{Duration, Instant};

pub struct Network;

const SIZES: &[Size] = &[Size { w: 2, h: 2 }, Size { w: 3, h: 1 }, Size { w: 2, h: 1 }];

/// The unit of the rate axis, above its ticks.
pub const UNIT: &str = "bit/s";
/// Room kept for the widest tick label (`1.5k`, `500M`), so the plot does
/// not move when the scale changes.
const TICK_CHARS: usize = 4;
/// The advance of the monospace font in em (Hack: 1233 of 2048 units).
const MONO_ADVANCE: f32 = 1233.0 / 2048.0;
/// A line of axis text, in times its size.
const AXIS_LINE: f32 = 1.2;
/// The widths of the two curves, and so how far they stay from the plot's
/// edge: a stroke does not leave the element.
const RX_WIDTH: f32 = 1.25;
const TX_WIDTH: f32 = 1.5;
const INSET: f32 = TX_WIDTH / 2.0;
/// The received curve's area under it, and a label's backing over the
/// curves, as fractions of their colour.
const AREA_ALPHA: f32 = 0.25;
const BACKING_ALPHA: f32 = 0.8;
/// The grid and the axes as fractions of the muted colour.
const GRID_ALPHA: f32 = 0.22;
const AXIS_ALPHA: f32 = 0.7;
/// The lowest and highest top a rate graph gets.
pub const MIN_TOP: f64 = 1000.0;
pub const MAX_TOP: f64 = 1e12;

fn rx_colour(t: &Theme) -> Color32 {
    t.palette().primary
}

fn tx_colour(t: &Theme) -> Color32 {
    t.role(role::GOOD)
}

pub(super) fn grid_colour(t: &Theme) -> Color32 {
    t.palette().muted_foreground.gamma_multiply(GRID_ALPHA)
}

pub(super) fn axis_colour(t: &Theme) -> Color32 {
    t.palette().muted_foreground.gamma_multiply(AXIS_ALPHA)
}

fn backing_colour(t: &Theme) -> Color32 {
    t.palette().card.gamma_multiply(BACKING_ALPHA)
}

/// The space between an axis and its labels, and the length of a tick mark.
fn tick_gap(m: &StatusSizes) -> f32 {
    0.4 * m.axis
}

/// A coordinate for a line one point wide: on the middle of a pixel at
/// scale 1, on a pixel edge pair at scale 2, so it is drawn sharp; kept
/// inside `lo..=hi`.
fn crisp(v: f32, lo: f32, hi: f32) -> f32 {
    (v.floor() + 0.5).clamp(lo + 0.5, (hi - 0.5).max(lo + 0.5))
}

/// Where the plot is inside an element: its left edge is the rate axis, its
/// bottom the time axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub plot: Rect,
    /// One row: only the plot, with the unit and the legend inside it.
    pub graph_only: bool,
}

pub fn frame(rect: Rect, size: Size, m: &StatusSizes) -> Frame {
    if size.h < 2 {
        let plot = Rect::from_min_max(
            pos2(
                crisp(rect.left(), rect.left(), rect.right()),
                crisp(rect.top(), rect.top(), rect.bottom()),
            ),
            pos2(
                crisp(rect.right() - 1.0, rect.left(), rect.right()),
                crisp(rect.bottom() - 1.0, rect.top(), rect.bottom()),
            ),
        );
        return Frame { plot, graph_only: true };
    }
    let g = tick_gap(m);
    let tick_w = TICK_CHARS as f32 * MONO_ADVANCE * m.axis;
    let line = (AXIS_LINE * m.axis).ceil();
    let left = rect.left() + m.pad_x + tick_w + 2.0 * g;
    let top = rect.top() + m.pad_y + line + m.gap;
    let bottom = rect.bottom() - m.pad_y - line - g;
    let right = rect.right() - m.pad_x;
    let (x0, x1) = (rect.left(), rect.right());
    let (y0, y1) = (rect.top(), rect.bottom());
    let plot_top = crisp(top, y0, y1);
    Frame {
        plot: Rect::from_min_max(
            pos2(crisp(left, x0, x1), plot_top),
            pos2(crisp(right, x0, x1), crisp(bottom, y0, y1).max(plot_top + 1.0)),
        ),
        graph_only: false,
    }
}

/// The area the curves are drawn in: the plot, a stroke's half width in.
fn data_area(plot: Rect) -> Rect {
    plot.shrink(INSET)
}

/// The graph's points in `area`: the newest sample at the right edge, one
/// step per second, five minutes across, the top at `top` bit/s.
pub fn points(values: &[f64], area: &Rect, top: f64) -> Vec<Pos2> {
    let n = values.len().min(WINDOW);
    let step = area.width() / WINDOW as f32;
    values[values.len() - n..]
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = area.right() - (n - 1 - i) as f32 * step;
            let frac = (v.max(0.0) / top).min(1.0) as f32;
            pos2(x, area.bottom() - frac * area.height())
        })
        .collect()
}

/// Rate ticks for a graph whose highest value is `peak` bit/s: 0 and up to
/// three equal steps of 1, 2 or 5 times a power of ten, the last at or
/// above the peak (and at least 1 kbit/s, at most 1 Tbit/s). Two to four
/// values; the last one is the top of the graph.
pub fn rate_ticks(peak: f64) -> Vec<f64> {
    let top = if peak.is_finite() { peak } else { 0.0 }.clamp(MIN_TOP, MAX_TOP);
    let mut exp = 0i32;
    loop {
        let unit = 10f64.powi(exp);
        for m in [1.0, 2.0, 5.0] {
            let step = m * unit;
            let n = (top / step - 1e-9).ceil().max(1.0);
            if n <= 3.0 {
                return (0..=n as usize).map(|i| i as f64 * step).collect();
            }
        }
        exp += 1;
    }
}

/// A tick value with k, M, G or T and no unit, as short as it is exact:
/// `0`, `500`, `1k`, `1.5k`, `15M`.
pub fn tick_label(v: f64) -> String {
    let v = if v.is_finite() { v.max(0.0) } else { 0.0 };
    let mut x = v;
    let mut prefix = "";
    for p in ["k", "M", "G", "T"] {
        if x < 1000.0 {
            break;
        }
        x /= 1000.0;
        prefix = p;
    }
    let text = format!("{x:.1}");
    format!("{}{prefix}", text.trim_end_matches('0').trim_end_matches('.'))
}

/// Time ticks of a five-minute graph from `x0` to `x0 + w`, newest at the
/// right: one a minute, `-5m` at the left edge and `now` at the right.
pub fn minute_ticks(x0: f32, w: f32) -> Vec<(f32, &'static str)> {
    const LABELS: [&str; 6] = ["-5m", "-4m", "-3m", "-2m", "-1m", "now"];
    LABELS
        .iter()
        .enumerate()
        .map(|(i, l)| (x0 + w * i as f32 / 5.0, *l))
        .collect()
}

/// Lay out `text` and draw it with `anchor` at `at`, on `backing` when
/// given; the rectangle it takes.
fn label(
    p: &egui::Painter,
    text: &str,
    font: &FontId,
    colour: Color32,
    anchor: Align2,
    at: Pos2,
    backing: Option<Color32>,
) -> Rect {
    let galley = p.layout_no_wrap(text.to_string(), font.clone(), colour);
    let r = anchor.anchor_size(at, galley.size());
    if let Some(b) = backing {
        p.rect_filled(r.expand2(egui::vec2(0.5 * font.size * 0.4, 0.0)), 0.0, b);
    }
    p.galley(r.min, galley, colour);
    r
}

/// The area under the received curve, down to the plot's bottom.
fn area_under(curve: &[Pos2], bottom: f32, colour: Color32) -> Shape {
    let mut mesh = Mesh::default();
    for (i, q) in curve.iter().enumerate() {
        let base = (2 * i) as u32;
        mesh.vertices.push(Vertex {
            pos: *q,
            uv: WHITE_UV,
            color: colour,
        });
        mesh.vertices.push(Vertex {
            pos: pos2(q.x, bottom),
            uv: WHITE_UV,
            color: colour,
        });
        if i > 0 {
            mesh.indices
                .extend([base - 2, base - 1, base, base - 1, base, base + 1]);
        }
    }
    Shape::mesh(mesh)
}

/// Draw `snap` in `rect` of `size` cells: grid, curves, axes, then the
/// labels, so a backing covers the curves under it.
fn paint(ui: &egui::Ui, rect: Rect, size: Size, snap: &Snapshot) {
    let t = Theme::current(ui.ctx());
    let p = ui.painter();
    let m = sizes_for(cell_height(rect, size));
    let f = frame(rect, size, &m);
    let (plot, area) = (f.plot, data_area(f.plot));
    let g = tick_gap(&m);
    let font = FontId::monospace(m.axis);
    let (grid, axis, muted) = (grid_colour(&t), axis_colour(&t), t.palette().muted_foreground);
    let (rx_c, tx_c) = (rx_colour(&t), tx_colour(&t));
    let (prx, ptx) = snap.peak();
    let ticks = rate_ticks(prx.max(ptx));
    let top = *ticks.last().unwrap_or(&MIN_TOP);
    let y_of = |v: f64| area.bottom() - (v / top) as f32 * area.height();
    let tick_y = |v: f64| crisp(y_of(v), plot.top(), plot.bottom());
    let minutes = minute_ticks(plot.left(), plot.width());
    for &v in &ticks[1..] {
        let y = tick_y(v);
        p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0, grid));
    }
    for &(x, _) in &minutes[1..] {
        let x = crisp(x, plot.left(), plot.right() + 1.0).min(plot.right());
        p.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0, grid));
    }
    // The curves: received as an area with a line, sent as a line over it.
    let rx = points(&snap.rx, &area, top);
    let tx = points(&snap.tx, &area, top);
    if rx.len() > 1 {
        p.add(area_under(&rx, plot.bottom(), rx_c.gamma_multiply(AREA_ALPHA)));
    }
    for (curve, width, c) in [(rx, RX_WIDTH, rx_c), (tx, TX_WIDTH, tx_c)] {
        if !curve.is_empty() {
            p.add(Shape::line(curve, Stroke::new(width, c)));
        }
    }
    // The axes.
    let axis_stroke = Stroke::new(1.0, axis);
    p.line_segment([plot.left_top(), plot.left_bottom()], axis_stroke);
    p.line_segment([plot.left_bottom(), plot.right_bottom()], axis_stroke);
    let backing = f.graph_only.then(|| backing_colour(&t));
    let ch = MONO_ADVANCE * m.axis;
    let legend = [("↓ rx", rx_c), ("↑ tx", tx_c)];
    if f.graph_only {
        // Under its grid line; the bottom edge says 0 by itself. The unit
        // right of the labels, the legend at the right, on the top row.
        for &v in ticks.iter().skip(1) {
            let at = pos2(plot.left() + 2.0 * g, tick_y(v) + g);
            label(p, &tick_label(v), &font, muted, Align2::LEFT_TOP, at, backing);
        }
        let row = plot.top() + g;
        let unit_x = plot.left() + 2.0 * g + TICK_CHARS as f32 * ch + 2.0 * g;
        label(p, UNIT, &font, muted, Align2::LEFT_TOP, pos2(unit_x, row), backing);
        let mut x = plot.right() - 2.0 * g;
        for (text, c) in legend.iter().rev() {
            let r = label(p, text, &font, *c, Align2::RIGHT_TOP, pos2(x, row), backing);
            x = r.left() - 2.0 * ch;
        }
        return;
    }
    let line = (AXIS_LINE * m.axis).ceil();
    for &v in &ticks {
        let y = tick_y(v);
        p.line_segment([pos2(plot.left() - g, y), pos2(plot.left(), y)], axis_stroke);
        label(
            p,
            &tick_label(v),
            &font,
            muted,
            Align2::RIGHT_CENTER,
            pos2(plot.left() - 1.5 * g, y),
            None,
        );
    }
    for (i, &(x, text)) in minutes.iter().enumerate() {
        let x = crisp(x, plot.left(), plot.right() + 1.0).min(plot.right());
        p.line_segment([pos2(x, plot.bottom()), pos2(x, plot.bottom() + g)], axis_stroke);
        let (anchor, at) = match i {
            0 => (Align2::LEFT_TOP, x - 0.5),
            5 => (Align2::RIGHT_TOP, x + 0.5),
            _ => (Align2::CENTER_TOP, x),
        };
        label(p, text, &font, muted, anchor, pos2(at, plot.bottom() + g), None);
    }
    // The header: the unit over the rate axis, the legend at the right.
    let mid = rect.top() + m.pad_y + line / 2.0;
    label(
        p,
        UNIT,
        &font,
        muted,
        Align2::LEFT_CENTER,
        pos2(rect.left() + m.pad_x, mid),
        None,
    );
    let mut x = plot.right();
    for (text, c) in legend.iter().rev() {
        let r = label(p, text, &font, *c, Align2::RIGHT_CENTER, pos2(x, mid), None);
        x = r.left() - 2.0 * ch;
    }
}

impl Element for Network {
    fn id(&self) -> &'static str {
        "network"
    }

    fn name(&self) -> &'static str {
        "Network traffic"
    }

    fn sizes(&self) -> &'static [Size] {
        SIZES
    }

    fn short_size(&self) -> Option<Size> {
        Some(SIZES[1])
    }

    fn sources(&self) -> &'static [Source] {
        &[Source::Network]
    }

    fn has_content(&self, data: &Data) -> bool {
        data.network.is_some_and(Snapshot::visible)
    }

    fn draw(&self, ui: &mut egui::Ui, rect: Rect, size: Size, data: &Data, _now: Instant) {
        background(ui, rect);
        if let Some(snap) = data.network {
            paint(ui, rect, size, snap);
        }
    }

    fn next_change(&self, _data: &Data, _rect: Rect, _now: Instant) -> Option<Duration> {
        None
    }

    fn refresh(&self) -> Option<Duration> {
        Some(crate::netstat::EVERY)
    }
}

/// A network as it might be, `n` seconds of a curve: for the tests and the
/// pictures.
#[cfg(test)]
pub(crate) fn sample(n: usize) -> Snapshot {
    use crate::netstat::{Info, Link, Vpn};
    let curve = |k: f64| -> Vec<f64> {
        (0..n)
            .map(|i| {
                let t = i as f64 / 300.0 * std::f64::consts::TAU;
                (k * (1.2 + (t * 3.0).sin() + 0.6 * (t * 11.0).sin())).max(0.0)
            })
            .collect()
    };
    Snapshot {
        rx: curve(4.0e6),
        tx: curve(6.0e5),
        total: (1_234_000_000, 98_700_000),
        info: Info {
            iface: Some("eth0".into()),
            default_route: true,
            link: Link::Wired(Some(1000)),
            ipv4: Some((std::net::Ipv4Addr::new(192, 168, 1, 23), 24)),
            gateway: Some(std::net::Ipv4Addr::new(192, 168, 1, 1)),
            dns: vec!["192.168.1.1".into(), "9.9.9.9".into()],
            vpn: Vpn::Up,
            addressed: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{self, CELL_HEIGHTS, flatten, paths, rect_of, rects, segments, shapes, text_boxes};
    use super::*;
    use crate::brand::{STATUS_CELL_LARGE_H, STATUS_CELL_SMALL_H};
    use crate::netstat::Info;

    const TWO_BY_TWO: Size = Size { w: 2, h: 2 };
    const THREE_BY_ONE: Size = Size { w: 3, h: 1 };

    fn theme() -> Theme {
        crate::brand::theme()
    }

    /// A short curve: 8 kbit/s peak received, 2 kbit/s sent.
    fn snap() -> Snapshot {
        Snapshot {
            rx: vec![0.0, 4000.0, 8000.0],
            tx: vec![1000.0, 2000.0, 0.0],
            ..sample(0)
        }
    }

    /// The window full: the received rate climbs to about 12 Mbit/s.
    fn busy() -> Snapshot {
        Snapshot {
            rx: (0..WINDOW).map(|i| (i * 40_000) as f64).collect(),
            ..snap()
        }
    }

    fn data(s: &Snapshot) -> Data<'_> {
        Data {
            network: Some(s),
            ..Data::EMPTY
        }
    }

    fn draw(snap: &Snapshot, size: Size, h: f32) -> Vec<Shape> {
        shapes(&Network, rect_of(size, h), size, &data(snap), Instant::now())
    }

    fn words(shapes: &[Shape]) -> Vec<String> {
        text_boxes(shapes).into_iter().map(|(s, _, _)| s).collect()
    }

    fn sorted(v: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = v.iter().map(|s| s.to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn rate_ticks_are_nice_numbers_up_to_the_peak() {
        let k = |v: &[f64]| v.iter().map(|x| tick_label(*x)).collect::<Vec<_>>();
        // Nothing, or very little: the floor of 1 kbit/s.
        for quiet in [0.0, 1.0, 999.0, 1000.0, -5.0, f64::NAN, f64::INFINITY] {
            assert_eq!(k(&rate_ticks(quiet)), ["0", "500", "1k"], "{quiet}");
        }
        assert_eq!(k(&rate_ticks(1001.0)), ["0", "500", "1k", "1.5k"]);
        assert_eq!(k(&rate_ticks(4.0e6)), ["0", "2M", "4M"]);
        assert_eq!(k(&rate_ticks(4.1e6)), ["0", "2M", "4M", "6M"]);
        assert_eq!(k(&rate_ticks(11.2e6)), ["0", "5M", "10M", "15M"]);
        assert_eq!(k(&rate_ticks(25e6)), ["0", "10M", "20M", "30M"]);
        assert_eq!(k(&rate_ticks(940e6)), ["0", "500M", "1G"]);
        assert_eq!(k(&rate_ticks(1e15)), ["0", "500G", "1T"], "capped");
        // Every peak: 2 to 4 values from 0, equal steps, the top at or
        // above the peak.
        let mut p = 1.0;
        while p < 2e12 {
            let t = rate_ticks(p);
            assert!((2..=4).contains(&t.len()), "{p}: {t:?}");
            assert_eq!(t[0], 0.0);
            let step = t[1];
            for w in t.windows(2) {
                assert!((w[1] - w[0] - step).abs() < step * 1e-9, "{p}: {t:?}");
            }
            let top = *t.last().unwrap();
            assert!(top >= p.clamp(MIN_TOP, MAX_TOP) * (1.0 - 1e-9), "{p}: {t:?}");
            p *= 1.37;
        }
    }

    #[test]
    fn tick_labels_are_short_and_exact() {
        for (v, want) in [
            (0.0, "0"),
            (500.0, "500"),
            (1000.0, "1k"),
            (1500.0, "1.5k"),
            (2e6, "2M"),
            (15e6, "15M"),
            (500e6, "500M"),
            (1e9, "1G"),
            (1e12, "1T"),
            (-1.0, "0"),
        ] {
            assert_eq!(tick_label(v), want, "{v}");
        }
    }

    #[test]
    fn minute_ticks_span_the_graph() {
        let t = minute_ticks(40.0, 500.0);
        let labels: Vec<_> = t.iter().map(|(_, l)| *l).collect();
        assert_eq!(labels, ["-5m", "-4m", "-3m", "-2m", "-1m", "now"]);
        assert_eq!(t[0].0, 40.0);
        assert_eq!(t[5].0, 540.0);
        for w in t.windows(2) {
            assert!((w[1].0 - w[0].0 - 100.0).abs() < 1e-3);
        }
    }

    #[test]
    fn network_draws_inside_its_rect_at_every_size() {
        let now = Instant::now();
        testing::assert_inside(&Network, &data(&snap()), now);
        testing::assert_inside(&Network, &data(&busy()), now);
        // Fractional positions, as a scaled screen gives them.
        let mut full = busy();
        full.tx = full.rx.iter().map(|v| v * 0.7).collect();
        for &size in SIZES {
            for h in [STATUS_CELL_LARGE_H, STATUS_CELL_SMALL_H, 70.3] {
                let rect = rect_of(size, h).translate(egui::vec2(0.37, 0.61));
                let out = shapes(&Network, rect, size, &data(&full), now);
                for s in &out {
                    assert!(
                        rect.expand(0.01).contains_rect(s.visual_bounding_rect()),
                        "{size} at {h}: {s:?} outside {rect:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_rate_axis_has_round_ticks_the_time_axis_minutes() {
        let t = theme();
        for h in CELL_HEIGHTS {
            let out = draw(&snap(), TWO_BY_TWO, h);
            // Peak 8 kbit/s: 0, 5k, 10k; then the minutes, the unit and legend.
            assert_eq!(
                {
                    let mut w = words(&out);
                    w.sort();
                    w
                },
                sorted(&[
                    "0", "5k", "10k", "-5m", "-4m", "-3m", "-2m", "-1m", "now", UNIT, "↓ rx", "↑ tx"
                ]),
                "{h}"
            );
            // The legend is in the curves' colours.
            let colour = |text: &str| text_boxes(&out).into_iter().find(|(s, _, _)| s == text).unwrap().2;
            assert_eq!(colour("↓ rx"), rx_colour(&t));
            assert_eq!(colour("↑ tx"), tx_colour(&t));
            assert_eq!(colour("5k"), t.palette().muted_foreground);
            // A grid line per tick but 0, and per minute but the axis.
            let segs = segments(&out);
            let count = |c: Color32| segs.iter().filter(|(_, s)| s.color == c).count();
            assert_eq!(count(grid_colour(&t)), 2 + 5, "{h}");
            // Two axes, a mark per tick and per minute.
            assert_eq!(count(axis_colour(&t)), 2 + 3 + 6, "{h}");
            // The unit and the legend are above the plot, the minutes below.
            let m = sizes_for(h);
            let f = frame(rect_of(TWO_BY_TWO, h), TWO_BY_TWO, &m);
            let boxes = text_boxes(&out);
            let at = |text: &str| boxes.iter().find(|(s, _, _)| s == text).unwrap().1;
            assert!(
                at(UNIT).bottom() <= f.plot.top() && at("↓ rx").bottom() <= f.plot.top(),
                "{h}"
            );
            assert!(at("-3m").top() >= f.plot.bottom(), "{h}");
            // The legend ends at the plot's right edge.
            assert!((at("↑ tx").right() - f.plot.right()).abs() < 1.5, "{h}");
        }
    }

    #[test]
    fn short_network_is_the_graph_alone() {
        let t = theme();
        for h in CELL_HEIGHTS {
            for size in [THREE_BY_ONE, Size { w: 2, h: 1 }] {
                let rect = rect_of(size, h);
                let m = sizes_for(h);
                let f = frame(rect, size, &m);
                let p = f.plot;
                assert!(f.graph_only, "{size} {h}");
                assert!(!frame(rect_of(TWO_BY_TWO, h), TWO_BY_TWO, &m).graph_only);
                // Edge to edge: the whole element is the plot.
                assert!(p.left() - rect.left() < 1.0 && p.top() - rect.top() < 1.0, "{p:?}");
                assert!(
                    rect.right() - p.right() < 1.5 && rect.bottom() - p.bottom() < 1.5,
                    "{p:?}"
                );
                for v in [p.left(), p.top(), p.right(), p.bottom()] {
                    assert_eq!(v.fract(), 0.5, "{p:?}");
                }
                let out = shapes(&Network, rect, size, &data(&busy()), Instant::now());
                // The rate labels (no 0), the unit and the legend; no time
                // labels and no text line.
                assert_eq!(
                    {
                        let mut w = words(&out);
                        w.sort();
                        w
                    },
                    sorted(&["5M", "10M", "15M", UNIT, "↓ rx", "↑ tx"]),
                    "{size} {h}"
                );
                // Every text inside the plot, none on another.
                let boxes = text_boxes(&out);
                for (i, (ta, a, _)) in boxes.iter().enumerate() {
                    assert!(p.contains_rect(*a), "{size} {h}: {ta} {a:?} outside the plot {p:?}");
                    for (tb, c, _) in &boxes[i + 1..] {
                        assert!(!a.intersects(*c), "{size} {h}: {ta} {a:?} overlaps {tb} {c:?}");
                    }
                }
                // Each on a backing over the curves: 5M, 10M, 15M, the unit
                // and the two legend entries, and no tick mark outside the
                // plot.
                let flat = flatten(&out);
                let curve = flat.iter().position(|s| matches!(s, Shape::Path(_))).expect("a curve");
                let backings: Vec<usize> = flat
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| matches!(s, Shape::Rect(r) if r.fill == backing_colour(&t)))
                    .map(|(i, _)| i)
                    .collect();
                assert_eq!(backings.len(), 6, "{size} {h}");
                assert!(backings.iter().all(|i| *i > curve), "{size} {h}");
                let segs = segments(&out);
                assert_eq!(segs.iter().filter(|(_, s)| s.color == axis_colour(&t)).count(), 2);
            }
        }
    }

    #[test]
    fn the_frame_follows_the_cell_height() {
        for h in CELL_HEIGHTS {
            let rect = rect_of(TWO_BY_TWO, h);
            let m = sizes_for(h);
            let p = frame(rect, TWO_BY_TWO, &m).plot;
            // Full width: from the tick labels to the right padding.
            assert!(
                p.left() < rect.left() + m.pad_x + 40.0 && (p.right() - (rect.right() - m.pad_x)).abs() < 1.0,
                "{p:?}"
            );
            assert!(p.height() > 50.0, "{h}: a tall plot: {p:?}");
            // Lines on half points, so one-point lines are sharp.
            for v in [p.left(), p.top(), p.right(), p.bottom()] {
                assert_eq!(v.fract(), 0.5, "{p:?}");
            }
        }
    }

    #[test]
    fn the_graph_scales_to_the_peak() {
        let t = theme();
        let h = STATUS_CELL_LARGE_H;
        let rect = rect_of(TWO_BY_TWO, h);
        let p = frame(rect, TWO_BY_TWO, &sizes_for(h)).plot;
        let line = |out: &[Shape], c: Color32| {
            paths(out)
                .into_iter()
                .find(|(_, _, col)| *col == c)
                .map(|(pts, _, _)| pts)
                .unwrap_or_default()
        };
        // A peak on a round top: the highest point of the received line
        // touches the top of the plot.
        let on_top = Snapshot {
            rx: vec![0.0, 4000.0, 10_000.0],
            ..snap()
        };
        let out = draw(&on_top, TWO_BY_TWO, h);
        let rx = line(&out, rx_colour(&t));
        let highest = rx.iter().map(|q| q.y).fold(f32::MAX, f32::min);
        assert!((highest - p.top()).abs() < 1.0, "{highest} vs {}", p.top());
        // The same on every size.
        for size in [THREE_BY_ONE, Size { w: 2, h: 1 }] {
            for h in CELL_HEIGHTS {
                let q = frame(rect_of(size, h), size, &sizes_for(h)).plot;
                let out = shapes(&Network, rect_of(size, h), size, &data(&on_top), Instant::now());
                let top = line(&out, rx_colour(&t)).iter().map(|q| q.y).fold(f32::MAX, f32::min);
                assert!((top - q.top()).abs() < 1.0, "{size} {h}: {top} vs {}", q.top());
            }
        }
        // 8 kbit/s of a 10k top: four fifths up, 0 at the bottom, the sent
        // 2k a fifth up.
        let out = draw(&snap(), TWO_BY_TWO, h);
        let rx = line(&out, rx_colour(&t));
        let tx = line(&out, tx_colour(&t));
        assert_eq!((rx.len(), tx.len()), (3, 3));
        let a = data_area(p);
        assert!((rx[2].y - (a.bottom() - 0.8 * a.height())).abs() < 0.1, "{rx:?}");
        assert!((rx[0].y - a.bottom()).abs() < 0.1);
        assert!((tx[1].y - (a.bottom() - 0.2 * a.height())).abs() < 0.1);
        // The newest at the right edge, one step per second, five minutes
        // across the plot.
        let step = a.width() / WINDOW as f32;
        assert!((rx[2].x - a.right()).abs() < 0.1);
        assert!((rx[2].x - rx[1].x - step).abs() < 0.1);
        assert!(a.right() - p.right() <= 0.0 && p.right() - a.right() <= 1.0);
        let full = points(&vec![1.0; WINDOW], &a, 1.0);
        assert!((full[0].x - (a.left() + step)).abs() < 0.01);
        // The received rate is also an area under its curve, in its colour.
        let area_fill = flatten(&out).into_iter().any(|s| match s {
            Shape::Mesh(m) => m
                .vertices
                .iter()
                .all(|v| v.color == rx_colour(&t).gamma_multiply(AREA_ALPHA)),
            _ => false,
        });
        assert!(area_fill, "{out:?}");
        // A quiet line stays near the bottom: the top is at least 1 kbit/s.
        let quiet = Snapshot {
            rx: vec![10.0, 20.0],
            tx: vec![0.0, 0.0],
            ..snap()
        };
        assert!(words(&draw(&quiet, TWO_BY_TWO, h)).contains(&"1k".to_string()));
        // No sample yet: no curve, the axes are still there.
        let empty = Snapshot {
            rx: vec![],
            tx: vec![],
            ..snap()
        };
        let e = draw(&empty, TWO_BY_TWO, h);
        assert!(paths(&e).is_empty());
        assert!(words(&e).contains(&"now".to_string()));
    }

    #[test]
    fn it_shows_with_a_route_or_an_address_and_asks_for_a_frame_a_second() {
        let now = Instant::now();
        assert!(Network.has_content(&data(&snap())));
        assert!(!Network.has_content(&Data::EMPTY), "no sampler: nothing");
        let mut none = snap();
        none.info = Info::default();
        assert!(!Network.has_content(&data(&none)));
        none.info.addressed = true;
        assert!(Network.has_content(&data(&none)), "an address without a route");
        let rect = rect_of(TWO_BY_TWO, 76.0);
        assert_eq!(Network.next_change(&data(&snap()), rect, now), None);
        assert_eq!(Network.refresh(), Some(Duration::from_secs(1)));
        assert_eq!(Network.sources(), [Source::Network]);
        assert_eq!(Network.sizes(), [TWO_BY_TWO, THREE_BY_ONE, Size { w: 2, h: 1 }]);
        assert_eq!(Network.short_size(), Some(THREE_BY_ONE));
    }

    #[test]
    fn no_text_overlaps_another_or_enters_the_plot() {
        let t = theme();
        let _ = t;
        for h in [STATUS_CELL_LARGE_H, STATUS_CELL_SMALL_H, 70.0] {
            let rect = rect_of(TWO_BY_TWO, h);
            let p = frame(rect, TWO_BY_TWO, &sizes_for(h)).plot;
            let boxes = text_boxes(&draw(&busy(), TWO_BY_TWO, h));
            for (i, (ta, a, _)) in boxes.iter().enumerate() {
                assert!(rect.contains_rect(*a), "{h}: {ta} {a:?} outside {rect:?}");
                assert!(!a.intersects(p), "{h}: {ta} {a:?} in the plot {p:?}");
                for (tb, c, _) in &boxes[i + 1..] {
                    assert!(!a.intersects(*c), "{h}: {ta} {a:?} overlaps {tb} {c:?}");
                }
            }
        }
    }

    #[test]
    fn it_draws_its_curves_in_the_theme_colours() {
        let t = theme();
        let out = draw(&snap(), TWO_BY_TWO, 76.0);
        let colours: Vec<(f32, Color32)> = paths(&out).into_iter().map(|(_, w, c)| (w, c)).collect();
        assert_eq!(colours, [(RX_WIDTH, rx_colour(&t)), (TX_WIDTH, tx_colour(&t))]);
        // The background is the tiles' colour.
        let first = rects(&out).first().map(|(_, c)| *c);
        assert_eq!(first, Some(t.palette().card));
        let _ = Stroke::NONE;
    }

    #[test]
    fn a_mesh_needs_no_texture() {
        // The area is a plain coloured mesh: the CPU renderer needs no texture for it.
        let out = draw(&snap(), TWO_BY_TWO, 76.0);
        let meshes: Vec<&Mesh> = flatten(&out)
            .into_iter()
            .filter_map(|s| if let Shape::Mesh(m) = s { Some(&**m) } else { None })
            .collect();
        assert!(!meshes.is_empty());
        assert!(meshes.iter().all(|m| m.texture_id == egui::TextureId::default()));
        let _ = (
            Vertex {
                pos: Pos2::ZERO,
                uv: WHITE_UV,
                color: Color32::WHITE,
            },
            Align2::CENTER_CENTER,
            FontId::default(),
        );
    }
}
