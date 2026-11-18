//! The connection element: one fact per row, the label in Geo on the left,
//! the value from one column: the interface and its link, the IPv4 address,
//! the VPN, and the public address while its lookup is on and has answered.
//! The rates stay in the traffic graph. It draws from the same
//! `netstat::Snapshot` as the graph and has content when the graph has.
//!
//! Rows move closer together as far as `min_row`, then rows are left out in
//! the order `Row::drop`: the public address, then the VPN. The link and the
//! address stay. A WiFi link
//! row too wide for the column leaves out the word `WiFi`. A long value is
//! cut at the padding. Only in a rect squeezed below the shipped cell
//! heights do the rows get so tight that the labels are half a point
//! smaller, so a `p` does not reach the `k` under it (`label_in_row`); with
//! at most four rows the shipped sizes never get there.

use super::battery::cut;
use super::{Data, Element, Size, Source, background, cell_height, sizes_for};
use crate::brand::{StatusSizes, role};
use crate::netstat::{self, Link, Snapshot, Vpn};
use egui::{Color32, FontId, Rect, pos2};
use shadcn_egui::Theme;
use std::time::{Duration, Instant};

pub struct Connection;

const SIZES: &[Size] = &[Size { w: 1, h: 2 }, Size { w: 2, h: 1 }];

/// Every label there can be; the value column starts after the widest, so it
/// does not move when a row is left out, or when `Public` comes and goes
/// with the lookup's switch.
const LABELS: [&str; 4] = ["Link", "IPv4", "VPN", "Public"];

/// Which font a run of text is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Geo: a word that is not a number (`up`, `WiFi`, `no VPN`).
    Label,
    /// The monospace font: numbers, units, addresses, names of interfaces.
    Value,
}

/// A colour by what it says; the theme gives the colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Muted,
    Warn,
    Good,
}

/// A piece of a row in one font and colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub face: Face,
    pub tone: Tone,
    pub text: String,
}

fn label(text: impl Into<String>, tone: Tone) -> Run {
    Run {
        face: Face::Label,
        tone,
        text: text.into(),
    }
}

fn value(text: impl Into<String>, tone: Tone) -> Run {
    Run {
        face: Face::Value,
        tone,
        text: clean(&text.into()),
    }
}

/// Names come from the system: a control character would break a line or
/// draw as a box.
fn clean(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { '?' } else { c }).collect()
}

fn colour(t: &Theme, tone: Tone) -> Color32 {
    match tone {
        Tone::Normal => t.palette().foreground,
        Tone::Muted => t.palette().muted_foreground,
        Tone::Good => t.role(role::GOOD),
        Tone::Warn => t.role(role::WARN),
    }
}

/// One fact.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: &'static str,
    pub runs: Vec<Run>,
    /// 0 stays. Of the others the highest is left out first.
    pub drop: u8,
}

/// The rows in their order; `public` is the public IP lookup's answer, with
/// no row while there is none.
pub fn rows(snap: &Snapshot, public: Option<&crate::publicip::Answer>) -> Vec<Row> {
    let fg = Tone::Normal;
    let i = &snap.info;
    let link = match (&i.iface, i.link) {
        (None, _) => vec![label("no interface", Tone::Warn)],
        (Some(n), Link::Wired(Some(mbit))) => vec![value(format!("{n} {}", netstat::speed(mbit)), fg)],
        (Some(n), Link::Wifi(Some(dbm))) => vec![
            value(n.as_str(), fg),
            label("WiFi", Tone::Muted),
            value(format!("{dbm} dBm"), fg),
        ],
        (Some(n), Link::Wifi(None)) => vec![value(n.as_str(), fg), label("WiFi", Tone::Muted)],
        (Some(n), Link::Wired(None) | Link::Unknown) => vec![value(n.as_str(), fg)],
    };
    let ip = match i.ipv4 {
        Some((a, p)) => vec![value(format!("{a}/{p}"), fg)],
        None => vec![label("no IPv4 address", Tone::Warn)],
    };
    let vpn = match i.vpn {
        Vpn::Up => vec![value(netstat::VPN_IF, fg), label("up", Tone::Good)],
        Vpn::Down => vec![value(netstat::VPN_IF, fg), label("down", Tone::Warn)],
        Vpn::Absent => vec![label("no VPN", Tone::Muted)],
    };
    let row = |label: &'static str, runs: Vec<Run>, drop: u8| Row { label, runs, drop };
    let mut out = vec![row("Link", link, 0), row("IPv4", ip, 0), row("VPN", vpn, 1)];
    match public {
        Some(crate::publicip::Answer::Addr(a)) => out.push(row("Public", vec![value(a.to_string(), fg)], 2)),
        Some(crate::publicip::Answer::Failed) => out.push(row("Public", vec![label("–", Tone::Muted)], 2)),
        None => {}
    }
    out
}

/// The tightest a row may be: its value and a point.
pub fn min_row(m: &StatusSizes) -> f32 {
    m.value + 1.0
}

/// The rows that fit `height` and how high each is: as tall as a line when
/// there is room, as tight as `min_row` before any row is left out.
pub fn fit(mut rows: Vec<Row>, height: f32, m: &StatusSizes) -> (Vec<Row>, f32) {
    let avail = (height - 2.0 * m.pad_y).max(0.0);
    let fits = (avail / min_row(m)).floor() as usize;
    while rows.len() > fits {
        let worst = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.drop > 0)
            .max_by_key(|(_, r)| r.drop)
            .map(|(i, _)| i);
        match worst {
            Some(i) => {
                rows.remove(i);
            }
            None => break,
        }
    }
    let row = (avail / rows.len().max(1) as f32).min(m.row).max(min_row(m));
    (rows, row)
}

/// Geo's ink from the top of a `k` to the bottom of a `p`, in em.
const GEO_INK: f32 = 0.88;

/// The size of a Geo label in lines `row` apart: `size`, or the half point
/// below it at which a descender no longer reaches the line under it.
pub fn label_in_row(size: f32, row: f32) -> f32 {
    size.min((row / GEO_INK * 2.0).floor() / 2.0)
}

fn font(face: Face, m: &StatusSizes) -> FontId {
    match face {
        Face::Label => crate::brand::geo(m.label),
        Face::Value => FontId::monospace(m.value),
    }
}

/// Draw `snap` in `rect` of `size` cells: the rows centred, each label at
/// the left padding, its runs from the value column. A row too wide for the
/// column leaves out the word `WiFi`; a run still too wide is cut at the
/// right padding.
fn paint(ui: &egui::Ui, rect: Rect, size: Size, snap: &Snapshot, public: Option<&crate::publicip::Answer>) {
    let t = Theme::current(ui.ctx());
    let p = ui.painter();
    let m = sizes_for(cell_height(rect, size));
    let (rows, row_h) = fit(rows(snap, public), rect.height(), &m);
    let width = |text: &str, font: &FontId| {
        p.layout_no_wrap(text.to_string(), font.clone(), Color32::TRANSPARENT)
            .size()
            .x
    };
    let widest = LABELS
        .iter()
        .map(|l| width(l, &font(Face::Label, &m)))
        .fold(0.0, f32::max);
    let left = rect.left() + m.pad_x;
    let (x, right) = (left + widest + m.gap, rect.right() - m.pad_x);
    let space = 0.35 * m.value;
    let label_font = crate::brand::geo(label_in_row(m.label, row_h));
    let top = rect.top() + (rect.height() - rows.len() as f32 * row_h) / 2.0;
    for (i, row) in rows.iter().enumerate() {
        let mid = top + (i as f32 + 0.5) * row_h;
        if !row.label.is_empty() {
            let g = p.layout_no_wrap(row.label.to_string(), label_font.clone(), colour(&t, Tone::Muted));
            p.galley(pos2(left, mid - g.size().y / 2.0), g, colour(&t, Tone::Muted));
        }
        let span = |runs: &[&Run]| -> f32 {
            runs.iter().map(|r| width(&r.text, &font(r.face, &m))).sum::<f32>()
                + space * runs.len().saturating_sub(1) as f32
        };
        let mut runs: Vec<&Run> = row.runs.iter().collect();
        if span(&runs) > right - x {
            runs.retain(|r| !(r.face == Face::Label && r.text == "WiFi"));
        }
        let mut at = x;
        for r in runs {
            let room = right - at;
            if room <= 0.0 {
                break;
            }
            let c = colour(&t, r.tone);
            let g = cut(p, &r.text, font(r.face, &m), c, room);
            let w = g.size().x;
            if w <= room + 0.01 {
                p.galley(pos2(at, mid - g.size().y / 2.0), g, c);
            }
            at += w + space;
        }
    }
}

impl Element for Connection {
    fn id(&self) -> &'static str {
        "connection"
    }

    fn name(&self) -> &'static str {
        "Connection"
    }

    fn sizes(&self) -> &'static [Size] {
        SIZES
    }

    fn short_size(&self) -> Option<Size> {
        None
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
            paint(ui, rect, size, snap, data.public_ip);
        }
    }

    fn next_change(&self, _data: &Data, _rect: Rect, _now: Instant) -> Option<Duration> {
        None
    }

    fn refresh(&self) -> Option<Duration> {
        Some(netstat::EVERY)
    }
}

/// A network as it might be: for the tests and the pictures.
#[cfg(test)]
pub fn sample() -> Snapshot {
    use crate::netstat::Info;
    use std::net::Ipv4Addr;
    Snapshot {
        rx: vec![0.0, 11_200_000.0, 4_000_000.0],
        tx: vec![1000.0, 2000.0, 600_000.0],
        info: Info {
            iface: Some("eth0".into()),
            default_route: true,
            link: Link::Wired(Some(1000)),
            ipv4: Some((Ipv4Addr::new(192, 168, 1, 23), 24)),
            gateway: Some(Ipv4Addr::new(192, 168, 1, 1)),
            vpn: Vpn::Up,
            addressed: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{self, CELL_HEIGHTS, flatten, rect_of, shapes, text_boxes};
    use super::*;
    use crate::brand::{STATUS_CELL_LARGE_H, STATUS_CELL_SMALL_H, STATUS_LARGE, STATUS_SMALL};
    use crate::netstat::Info;
    use egui::{FontFamily, Shape, vec2};

    const TALL: Size = Size { w: 1, h: 2 };
    const WIDE: Size = Size { w: 2, h: 1 };

    fn theme() -> Theme {
        crate::brand::theme()
    }

    fn data(s: &Snapshot) -> Data<'_> {
        Data {
            network: Some(s),
            ..Data::EMPTY
        }
    }

    fn draw(s: &Snapshot, size: Size, h: f32) -> Vec<Shape> {
        shapes(&Connection, rect_of(size, h), size, &data(s), Instant::now())
    }

    fn tall(s: &Snapshot, h: f32) -> Vec<Shape> {
        draw(s, TALL, h)
    }

    /// Each row as the words it draws, left to right, top to bottom.
    fn drawn(shapes: &[Shape]) -> Vec<String> {
        let mut boxes = text_boxes(shapes);
        boxes.sort_by(|a, b| a.1.center().y.total_cmp(&b.1.center().y));
        let mut rows: Vec<(f32, Vec<(f32, String)>)> = Vec::new();
        for (text, r, _) in boxes {
            let y = r.center().y;
            match rows.last_mut().filter(|(at, _)| (y - *at).abs() < 4.0) {
                Some((_, words)) => words.push((r.left(), text)),
                None => rows.push((y, vec![(r.left(), text)])),
            }
        }
        rows.into_iter()
            .map(|(_, mut w)| {
                w.sort_by(|a, b| a.0.total_cmp(&b.0));
                w.into_iter().map(|(_, t)| t).collect::<Vec<_>>().join(" ")
            })
            .collect()
    }

    fn colour_of(shapes: &[Shape], text: &str) -> Color32 {
        text_boxes(shapes)
            .into_iter()
            .find(|(s, _, _)| s == text)
            .unwrap_or_else(|| panic!("{text} not drawn"))
            .2
    }

    /// The families and galley positions of every text.
    fn faces(shapes: &[Shape]) -> Vec<(String, FontFamily, f32)> {
        flatten(shapes)
            .into_iter()
            .filter_map(|s| match s {
                Shape::Text(t) => Some((
                    t.galley.text().to_string(),
                    t.galley.job.sections[0].format.font_id.family.clone(),
                    t.pos.x,
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn one_fact_a_row_label_left_value_in_a_column() {
        let h = STATUS_CELL_LARGE_H;
        let out = tall(&sample(), h);
        assert_eq!(
            drawn(&out),
            ["Link eth0 1 Gbit/s", "IPv4 192.168.1.23/24", "VPN wg0 up"]
        );
        // Labels in Geo at the padding, numbers in the monospace font from
        // one column.
        let rect = rect_of(TALL, h);
        let pad = sizes_for(h).pad_x;
        let geo = FontFamily::Name(crate::brand::GEO.into());
        let mut column: Option<f32> = None;
        for (text, family, x) in faces(&out) {
            if LABELS.contains(&text.as_str()) {
                assert_eq!(family, geo, "{text}");
                assert!((x - (rect.left() + pad)).abs() < 0.01, "{text}: {x}");
            } else if text.contains("192.168") || text.contains("Gbit") {
                assert_eq!(family, FontFamily::Monospace, "{text}");
                let c = *column.get_or_insert(x);
                assert!((x - c).abs() < 0.01, "{text}: {x} vs {c}");
            }
        }
        assert!(column.unwrap() > rect.left() + pad + 30.0);
        // Labels muted, values in the foreground, an up VPN good.
        let t = theme();
        assert_eq!(colour_of(&out, "Link"), t.palette().muted_foreground);
        assert_eq!(colour_of(&out, "192.168.1.23/24"), t.palette().foreground);
        assert_eq!(colour_of(&out, "up"), t.role(role::GOOD));
    }

    #[test]
    fn what_is_missing_keeps_its_words_and_colours() {
        let t = theme();
        let mut bare = sample();
        bare.info = Info {
            iface: Some("wlan0".into()),
            default_route: false,
            link: Link::Wifi(Some(-61)),
            addressed: true,
            vpn: Vpn::Down,
            ..Info::default()
        };
        let out = tall(&bare, STATUS_CELL_LARGE_H);
        assert_eq!(
            drawn(&out),
            ["Link wlan0 WiFi -61 dBm", "IPv4 no IPv4 address", "VPN wg0 down"]
        );
        for text in ["no IPv4 address", "down"] {
            assert_eq!(colour_of(&out, text), t.role(role::WARN), "{text}");
        }
        let mut up = sample();
        up.info.vpn = Vpn::Absent;
        up.info.link = Link::Unknown;
        up.info.iface = None;
        let out = tall(&up, STATUS_CELL_LARGE_H);
        assert_eq!(drawn(&out), ["Link no interface", "IPv4 192.168.1.23/24", "VPN no VPN"]);
        assert_eq!(colour_of(&out, "no interface"), t.role(role::WARN));
        assert_eq!(colour_of(&out, "no VPN"), t.palette().muted_foreground);
    }

    fn labels_of(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.label).collect()
    }

    /// The height of a 1 x 2 element with cells `cell_h` high.
    fn tall_height(cell_h: f32) -> f32 {
        2.0 * cell_h + crate::ui::TILE_GAP
    }

    #[test]
    fn rows_tighten_first_then_the_vpn() {
        // A usual network loses nothing at either height, tall or wide.
        for (h, m) in [(STATUS_CELL_LARGE_H, STATUS_LARGE), (STATUS_CELL_SMALL_H, STATUS_SMALL)] {
            for height in [tall_height(h), h] {
                let (r, _) = fit(rows(&sample(), None), height, &m);
                assert_eq!(labels_of(&r), ["Link", "IPv4", "VPN"], "{h} {height}");
            }
        }
        // The row is no taller than a line, however much room there is.
        assert_eq!(fit(rows(&sample(), None), 400.0, &STATUS_LARGE).1, STATUS_LARGE.row);
        // Squeezed, the rows first move together as far as `min_row`.
        let three = 2.0 * STATUS_LARGE.pad_y + 3.0 * min_row(&STATUS_LARGE);
        let (r, h) = fit(rows(&sample(), None), three, &STATUS_LARGE);
        assert_eq!(r.len(), 3);
        assert!((h - min_row(&STATUS_LARGE)).abs() < 0.01, "{h}");
        // Squeezed further the VPN goes; the link and the address stay.
        let mut seen = Vec::new();
        for h in (5..=150).rev() {
            let got: Vec<String> = labels_of(&fit(rows(&sample(), None), h as f32, &STATUS_LARGE).0)
                .iter()
                .map(|s| s.to_string())
                .collect();
            for keep in ["Link", "IPv4"] {
                assert!(got.iter().any(|g| g == keep), "{h}: {keep} in {got:?}");
            }
            if seen.last() != Some(&got) {
                seen.push(got);
            }
        }
        let gone =
            |a: &[String], b: &[String]| -> Vec<String> { a.iter().filter(|x| !b.contains(x)).cloned().collect() };
        let order: Vec<String> = seen.windows(2).flat_map(|w| gone(&w[0], &w[1])).collect();
        assert_eq!(order, ["VPN"]);
    }

    fn with_public<'a>(s: &'a Snapshot, a: &'a crate::publicip::Answer) -> Data<'a> {
        Data {
            public_ip: Some(a),
            ..data(s)
        }
    }

    #[test]
    fn the_public_row_shows_the_answer_and_drops_first() {
        use crate::publicip::Answer;
        let addr = Answer::Addr("203.0.113.7".parse().unwrap());
        let snap = sample();
        // Four rows fit in either size at either cell height.
        for h in CELL_HEIGHTS {
            for size in [TALL, WIDE] {
                let out = shapes(
                    &Connection,
                    rect_of(size, h),
                    size,
                    &with_public(&snap, &addr),
                    Instant::now(),
                );
                assert_eq!(
                    drawn(&out),
                    [
                        "Link eth0 1 Gbit/s",
                        "IPv4 192.168.1.23/24",
                        "VPN wg0 up",
                        "Public 203.0.113.7"
                    ],
                    "{size} {h}"
                );
            }
        }
        let t = theme();
        let out = shapes(
            &Connection,
            rect_of(TALL, STATUS_CELL_LARGE_H),
            TALL,
            &with_public(&snap, &Answer::Failed),
            Instant::now(),
        );
        assert_eq!(drawn(&out).last().map(String::as_str), Some("Public –"));
        assert_eq!(colour_of(&out, "–"), t.palette().muted_foreground);
        // No answer: no row.
        assert_eq!(rows(&sample(), None).len(), 3);
        // Squeezed, `Public` goes first, then the VPN; the link and the
        // address stay.
        let mut seen: Vec<Vec<&str>> = Vec::new();
        for h in (5..=150).rev() {
            let got: Vec<&str> = fit(rows(&sample(), Some(&addr)), h as f32, &STATUS_SMALL)
                .0
                .iter()
                .map(|r| r.label)
                .collect();
            assert!(got.contains(&"Link") && got.contains(&"IPv4"), "{h}: {got:?}");
            if seen.last() != Some(&got) {
                seen.push(got);
            }
        }
        let gone: Vec<&str> = seen
            .windows(2)
            .flat_map(|w| w[0].iter().filter(|x| !w[1].contains(x)).copied().collect::<Vec<_>>())
            .collect();
        assert_eq!(gone, ["Public", "VPN"]);
    }

    #[test]
    fn the_wide_size_keeps_the_link_the_address_and_the_vpn() {
        for h in CELL_HEIGHTS {
            let d = drawn(&draw(&sample(), WIDE, h));
            assert_eq!(d, ["Link eth0 1 Gbit/s", "IPv4 192.168.1.23/24", "VPN wg0 up"], "{h}");
        }
    }

    #[test]
    fn a_wifi_row_that_does_not_fit_leaves_out_the_word_wifi() {
        let wifi = |name: &str| {
            let mut snap = sample();
            snap.info.iface = Some(name.into());
            snap.info.link = Link::Wifi(Some(-61));
            snap
        };
        let row_of = |name: &str, h: f32| {
            drawn(&tall(&wifi(name), h))
                .into_iter()
                .find(|r| r.starts_with("Link"))
                .unwrap()
        };
        // The short names keep the word at either height.
        for h in CELL_HEIGHTS {
            assert_eq!(row_of("wlan0", h), "Link wlan0 WiFi -61 dBm", "{h}");
            assert_eq!(row_of("wlp0s20f3", h), "Link wlp0s20f3 WiFi -61 dBm", "{h}");
        }
        // A name that does not fit with the word loses it, and keeps the
        // rest.
        assert_eq!(
            row_of("wlp0s20f3u1", STATUS_CELL_LARGE_H),
            "Link wlp0s20f3u1 -61 dBm",
            "the word goes first"
        );
        assert_eq!(
            row_of("wlx00e04c123456", STATUS_CELL_SMALL_H),
            "Link wlx00e04c123456 -61 dBm"
        );
    }

    #[test]
    fn a_long_value_is_cut_at_the_padding() {
        let mut long = sample();
        let name = "enp0s31f6-a-name-much-longer-than-the-kernel-makes";
        long.info.iface = Some(name.into());
        let h = STATUS_CELL_LARGE_H;
        let rect = rect_of(TALL, h);
        let out = tall(&long, h);
        let right = rect.right() - sizes_for(h).pad_x;
        let link = text_boxes(&out)
            .into_iter()
            .find(|(s, _, _)| s.starts_with("enp0s31f6"))
            .expect("the link is drawn, cut");
        // The galley keeps the whole text; what it draws ends at the padding.
        let whole = (name.len() + " 1 Gbit/s".len()) as f32;
        assert!(link.1.width() < whole * 0.6 * sizes_for(h).value, "{:?}", link.1);
        assert!(link.1.right() <= right + 0.5, "{:?} past {right}", link.1);
        for (text, r, _) in text_boxes(&out) {
            assert!(r.right() <= right + 0.5, "{text}: {r:?}");
        }
    }

    #[test]
    fn names_from_the_system_never_break_a_row() {
        let mut odd = sample();
        odd.info.iface = Some("e<0>\n".into());
        let d = drawn(&tall(&odd, STATUS_CELL_LARGE_H));
        assert!(d.contains(&"Link e<0>? 1 Gbit/s".to_string()), "{d:?}");
        odd.info.iface = Some("<b>&\"'\x01".into());
        odd.info.link = Link::Wifi(None);
        let r = rows(&odd, None);
        assert_eq!(r[0].runs[0].text, "<b>&\"'?");
        let d = drawn(&tall(&odd, STATUS_CELL_LARGE_H));
        assert!(d.iter().any(|r| r.starts_with("Link <b>&\"'?")), "{d:?}");
    }

    #[test]
    fn no_text_overlaps_another_or_leaves_the_element() {
        let mut busy = sample();
        busy.info.iface = Some("enx00e04c123456".into());
        busy.info.ipv4 = Some((std::net::Ipv4Addr::new(192, 168, 100, 123), 24));
        busy.info.link = Link::Wired(Some(2500));
        let mut warn = sample();
        warn.info = Info {
            iface: Some("wlp0s20f3".into()),
            default_route: false,
            link: Link::Wifi(Some(-61)),
            addressed: true,
            vpn: Vpn::Down,
            ..Info::default()
        };
        for snap in [&busy, &warn] {
            for h in [STATUS_CELL_LARGE_H, STATUS_CELL_SMALL_H, 70.0] {
                for size in [TALL, WIDE] {
                    let rect = rect_of(size, h);
                    let right = rect.right() - sizes_for(h).pad_x;
                    let b = text_boxes(&draw(snap, size, h));
                    for (i, (ta, a, _)) in b.iter().enumerate() {
                        assert!(rect.contains_rect(*a), "{size} {h}: {ta} {a:?}");
                        assert!(a.right() <= right + 0.5, "{size} {h}: {ta} {a:?} past the padding");
                        for (tb, c, _) in &b[i + 1..] {
                            // The ink boxes of two Geo labels one row apart
                            // may touch by a descender's rounding (the
                            // shots show no collision), never by more.
                            let (a, c) = (a.shrink2(vec2(0.0, 0.75)), c.shrink2(vec2(0.0, 0.75)));
                            assert!(!a.intersects(c), "{size} {h}: {ta} {a:?} overlaps {tb} {c:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn connection_draws_inside_its_rect_at_every_size() {
        let now = Instant::now();
        let mut hostile = sample();
        // A 40-character interface on WiFi.
        hostile.info.iface = Some("e".repeat(40));
        hostile.info.link = Link::Wifi(Some(-61));
        // The longest an address can be written, as the public answer.
        let v6 = crate::publicip::Answer::Addr("1111:2222:3333:4444:5555:6666:7777:8888".parse().unwrap());
        let hostile_data = with_public(&hostile, &v6);
        testing::assert_inside(&Connection, &hostile_data, now);
        testing::assert_inside(&Connection, &data(&sample()), now);
        for size in SIZES {
            for h in [STATUS_CELL_LARGE_H, STATUS_CELL_SMALL_H, 70.3] {
                let rect = rect_of(*size, h).translate(vec2(0.37, 0.61));
                let out = shapes(&Connection, rect, *size, &hostile_data, now);
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
    fn labels_shrink_only_on_the_tightest_rows_of_the_large_cell() {
        assert_eq!(
            label_in_row(STATUS_LARGE.label, 14.3),
            STATUS_LARGE.label,
            "a loose row"
        );
        assert_eq!(label_in_row(STATUS_LARGE.label, 13.0), 14.5, "a tight row");
        assert_eq!(label_in_row(STATUS_SMALL.label, 12.1), STATUS_SMALL.label);
    }

    #[test]
    fn it_has_content_when_the_graph_has() {
        let s = sample();
        assert!(Connection.has_content(&data(&s)));
        assert!(!Connection.has_content(&Data::EMPTY));
        let nothing = Snapshot::default();
        assert!(!Connection.has_content(&data(&nothing)));
        assert_eq!(
            Connection.has_content(&data(&nothing)),
            super::super::network::Network.has_content(&data(&nothing))
        );
        assert_eq!(Connection.refresh(), Some(Duration::from_secs(1)));
        assert_eq!(
            Connection.next_change(&data(&s), rect_of(TALL, 76.0), Instant::now()),
            None
        );
        assert_eq!(Connection.id(), "connection");
        assert_eq!(Connection.name(), "Connection");
        assert_eq!(Connection.sizes(), [TALL, WIDE]);
        assert_eq!(Connection.short_size(), None);
        assert_eq!(Connection.sources(), [Source::Network]);
    }
}
