//! The connection element: one fact per row, the label in Geo at the left
//! padding, the value ending at the right padding: the interface and its
//! link, the IPv4 address, the gateway, the DNS servers in use, the VPN
//! while it is up, the public address while its lookup is on and has
//! answered, the bytes received and sent, and the machine's uptime. The
//! rates stay in the traffic graph. It draws from the same
//! `netstat::Snapshot` as the graph and has content when the graph has.
//!
//! The facts of where the machine is (link, address, gateway, DNS) always
//! have a row, with a warning word when one is missing (a default route
//! without a gateway, as on PPP or wwan, is no fault: `direct`); the measures
//! (traffic, uptime) have a row only when known. The 1 x 2 size draws one
//! column, the 2 x 1 size two: the left holds the fixed rows (link, address,
//! gateway, DNS), the right the others in their order (`Row::right`), both
//! from the top at one row height, so the rows line up and none changes
//! column when the VPN or public row comes or goes.
//!
//! Rows move closer together as far as `min_row`, then, while a column holds
//! more rows than fit, that column's rows are left out in the order
//! `Row::drop`: uptime, traffic, DNS, gateway, the public address, the VPN.
//! The link and the address stay; with room for one line only, the 2 x 1
//! size puts the address in the right column. A row too wide for its
//! room leaves out the word `WiFi`; a value still too long starts after its
//! label and is cut at the padding. Only in a rect squeezed below the
//! shipped cell heights do the rows get so tight that the labels are half a
//! point smaller, so a `p` does not reach the `k` under it
//! (`label_in_row`); the shipped sizes never get there.

use super::battery::cut;
use super::{Data, Element, Size, Source, background, cell_height, sizes_for};
use crate::brand::{StatusSizes, role};
use crate::netstat::{self, Link, Snapshot, Vpn};
use egui::{Color32, FontId, Rect, pos2};
use shadcn_egui::Theme;
use std::time::{Duration, Instant};

pub struct Connection;

const SIZES: &[Size] = &[Size { w: 1, h: 2 }, Size { w: 2, h: 1 }];

/// Which font a run of text is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Geo: a word that is not a number (`up`, `WiFi`, `none`).
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
    /// In two columns the row is in the right one: the rows that come and
    /// go (VPN, Public) and the measures; the left holds the fixed rows.
    pub right: bool,
}

/// The rows in their order; `public` is the public IP lookup's answer, with
/// no row while there is none.
pub fn rows(snap: &Snapshot, public: Option<&crate::publicip::Answer>) -> Vec<Row> {
    let fg = Tone::Normal;
    let i = &snap.info;
    let none = || vec![label("none", Tone::Warn)];
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
    let gateway = match i.gateway {
        Some(g) => vec![value(g.to_string(), fg)],
        None if i.direct => vec![label("direct", Tone::Muted)],
        None => none(),
    };
    let dns = match i.dns.split_first() {
        Some((first, [])) => vec![value(first.as_str(), fg)],
        Some((first, rest)) => vec![
            value(first.as_str(), fg),
            value(format!("+{}", rest.len()), Tone::Muted),
        ],
        None => none(),
    };
    let row = |label: &'static str, runs: Vec<Run>, drop: u8| Row {
        label,
        runs,
        drop,
        right: true,
    };
    let fixed = |label: &'static str, runs: Vec<Run>, drop: u8| Row {
        right: false,
        ..row(label, runs, drop)
    };
    let mut out = vec![
        fixed("Link", link, 0),
        fixed("IPv4", ip, 0),
        fixed("Gateway", gateway, 3),
        fixed("DNS", dns, 4),
    ];
    if i.vpn == Vpn::Up {
        out.push(row("VPN", vec![value(netstat::VPN_IF, fg), label("up", Tone::Good)], 1));
    }
    match public {
        Some(crate::publicip::Answer::Addr(a)) => out.push(row("Public", vec![value(a.to_string(), fg)], 2)),
        Some(crate::publicip::Answer::Failed) => out.push(row("Public", vec![label("–", Tone::Muted)], 2)),
        None => {}
    }
    if let Some((rx, tx)) = snap.totals {
        out.push(row(
            "Traffic",
            vec![
                value(format!("↓ {}", netstat::bytes(rx)), fg),
                value(format!("↑ {}", netstat::bytes(tx)), fg),
            ],
            5,
        ));
    }
    if let Some(s) = snap.uptime {
        out.push(row("Uptime", vec![value(super::hardware::uptime(s), fg)], 6));
    }
    out
}

/// How many columns `size` draws: two side by side when it is wider than
/// high.
pub fn columns(size: Size) -> usize {
    if size.w > size.h { 2 } else { 1 }
}

/// The tightest a row may be: its value and a point.
pub fn min_row(m: &StatusSizes) -> f32 {
    m.value + 1.0
}

/// The column of each of `rows` in `columns` columns and its line in it.
pub fn places(rows: &[Row], columns: usize) -> Vec<(usize, usize)> {
    let mut next = [0usize; 2];
    rows.iter()
        .map(|r| {
            let c = usize::from(columns > 1 && r.right);
            next[c] += 1;
            (c, next[c] - 1)
        })
        .collect()
}

/// The lines the fullest column of `rows` needs.
fn lines(rows: &[Row], columns: usize) -> usize {
    places(rows, columns).iter().map(|p| p.1 + 1).max().unwrap_or(0)
}

/// The rows that fit `height` in `columns` columns and how high each is: as
/// tall as a line when there is room, as tight as `min_row` before any row
/// is left out. While a column holds more rows than fit, its row of the
/// highest `Row::drop` is left out; when the left column holds only rows
/// that stay and still too many, its last moves to the right column.
pub fn fit(mut rows: Vec<Row>, height: f32, columns: usize, m: &StatusSizes) -> (Vec<Row>, f32) {
    let columns = columns.clamp(1, 2);
    let avail = (height - 2.0 * m.pad_y).max(0.0);
    let fits = (avail / min_row(m)).floor() as usize;
    loop {
        let at = places(&rows, columns);
        let worst = (0..columns)
            .filter(|&c| at.iter().filter(|p| p.0 == c).count() > fits)
            .find_map(|c| {
                rows.iter()
                    .zip(&at)
                    .enumerate()
                    .filter(|(_, (r, p))| r.drop > 0 && p.0 == c)
                    .max_by_key(|(_, (r, _))| r.drop)
                    .map(|(i, _)| i)
            });
        if let Some(i) = worst {
            rows.remove(i);
            continue;
        }
        // The left column holds only rows that stay and still too many
        // (one line: Link and IPv4): its last goes to the top of the right
        // column, as long as the right has room for what stays there.
        let count = |c: usize| at.iter().filter(|p| p.0 == c).count();
        let staying_right = rows.iter().zip(&at).filter(|(r, p)| p.0 == 1 && r.drop == 0).count();
        let moved = (columns == 2 && count(0) > fits && staying_right < fits)
            .then(|| rows.iter().rposition(|r| !r.right))
            .flatten();
        match moved {
            Some(i) => rows[i].right = true,
            None => break,
        }
    }
    let per = lines(&rows, columns).max(1);
    let row = (avail / per as f32).min(m.row).max(min_row(m));
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

/// Draw `snap` in `rect` of `size` cells: the rows centred, in one or two
/// columns, each label at its column's left padding, its runs ending at the
/// right padding. A row too wide for its room leaves out the word `WiFi`; a
/// row still too wide starts after its label and its last run is cut at the
/// right padding.
fn paint(ui: &egui::Ui, rect: Rect, size: Size, snap: &Snapshot, public: Option<&crate::publicip::Answer>) {
    let t = Theme::current(ui.ctx());
    let p = ui.painter();
    let m = sizes_for(cell_height(rect, size));
    let cols = columns(size);
    let (rows, row_h) = fit(rows(snap, public), rect.height(), cols, &m);
    let at = places(&rows, cols);
    let per = lines(&rows, cols).max(1);
    let width = |text: &str, font: &FontId| {
        p.layout_no_wrap(text.to_string(), font.clone(), Color32::TRANSPARENT)
            .size()
            .x
    };
    let column_w = rect.width() / cols as f32;
    let space = 0.35 * m.value;
    let label_font = crate::brand::geo(label_in_row(m.label, row_h));
    let top = rect.top() + (rect.height() - per as f32 * row_h) / 2.0;
    for (row, &(c, line)) in rows.iter().zip(&at) {
        let left = rect.left() + c as f32 * column_w + m.pad_x;
        let right = rect.left() + (c + 1) as f32 * column_w - m.pad_x;
        let mid = top + (line as f32 + 0.5) * row_h;
        let g = p.layout_no_wrap(row.label.to_string(), label_font.clone(), colour(&t, Tone::Muted));
        let x = left + g.size().x + m.gap;
        p.galley(pos2(left, mid - g.size().y / 2.0), g, colour(&t, Tone::Muted));
        let span = |runs: &[&Run]| -> f32 {
            runs.iter().map(|r| width(&r.text, &font(r.face, &m))).sum::<f32>()
                + space * runs.len().saturating_sub(1) as f32
        };
        let mut runs: Vec<&Run> = row.runs.iter().collect();
        if span(&runs) > right - x {
            runs.retain(|r| !(r.face == Face::Label && r.text == "WiFi"));
        }
        let mut at = (right - span(&runs)).max(x);
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
            direct: false,
            vpn: Vpn::Up,
            addressed: true,
            dns: vec!["192.168.1.1".into()],
        },
        totals: Some((1_230_000_000, 456_000_000)),
        uptime: Some(15_120),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{self, CELL_HEIGHTS, flatten, rect_of, shapes, text_boxes};
    use super::*;
    use crate::brand::{STATUS_CELL_LARGE_H, STATUS_CELL_SMALL_H, STATUS_LARGE, STATUS_SMALL};
    use crate::netstat::Info;
    use crate::publicip::Answer;
    use egui::{FontFamily, Shape, vec2};

    const TALL: Size = Size { w: 1, h: 2 };
    const WIDE: Size = Size { w: 2, h: 1 };
    const ALL: [&str; 8] = ["Link", "IPv4", "Gateway", "DNS", "VPN", "Public", "Traffic", "Uptime"];

    fn theme() -> Theme {
        crate::brand::theme()
    }

    fn data(s: &Snapshot) -> Data<'_> {
        Data {
            network: Some(s),
            ..Data::EMPTY
        }
    }

    fn with_public<'a>(s: &'a Snapshot, a: &'a Answer) -> Data<'a> {
        Data {
            public_ip: Some(a),
            ..data(s)
        }
    }

    fn public() -> Answer {
        Answer::Addr("203.0.113.7".parse().unwrap())
    }

    fn draw(s: &Snapshot, size: Size, h: f32) -> Vec<Shape> {
        shapes(&Connection, rect_of(size, h), size, &data(s), Instant::now())
    }

    fn tall(s: &Snapshot, h: f32) -> Vec<Shape> {
        draw(s, TALL, h)
    }

    /// Every text: its words, galley left and right, centre y, family.
    fn texts(shapes: &[Shape]) -> Vec<(String, f32, f32, f32, FontFamily)> {
        flatten(shapes)
            .into_iter()
            .filter_map(|s| match s {
                Shape::Text(t) => Some((
                    t.galley.text().to_string(),
                    t.pos.x,
                    t.pos.x + t.galley.size().x,
                    t.pos.y + t.galley.size().y / 2.0,
                    t.galley.job.sections[0].format.font_id.family.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    /// The texts of one row: left edge, right edge, words.
    type Words = Vec<(f32, f32, String)>;

    /// Each row of the texts whose left edge is in `from..to`, as the words
    /// it draws, left to right, top to bottom, with its centre y and the
    /// right edge of its last text.
    fn lines_in(shapes: &[Shape], from: f32, to: f32) -> Vec<(String, f32, f32)> {
        let mut t: Vec<_> = texts(shapes)
            .into_iter()
            .filter(|(_, l, ..)| (from..to).contains(l))
            .collect();
        t.sort_by(|a, b| a.3.total_cmp(&b.3));
        let mut rows: Vec<(f32, Words)> = Vec::new();
        for (text, l, r, y, _) in t {
            match rows.last_mut().filter(|(at, _)| (y - *at).abs() < 4.0) {
                Some((_, words)) => words.push((l, r, text)),
                None => rows.push((y, vec![(l, r, text)])),
            }
        }
        rows.into_iter()
            .map(|(y, mut w)| {
                w.sort_by(|a, b| a.0.total_cmp(&b.0));
                let right = w.iter().map(|x| x.1).fold(f32::MIN, f32::max);
                let words = w.into_iter().map(|(_, _, t)| t).collect::<Vec<_>>().join(" ");
                (words, y, right)
            })
            .collect()
    }

    /// The rows of a one-column draw.
    fn drawn(shapes: &[Shape]) -> Vec<String> {
        lines_in(shapes, f32::MIN, f32::MAX).into_iter().map(|l| l.0).collect()
    }

    fn colour_of(shapes: &[Shape], text: &str) -> Color32 {
        text_boxes(shapes)
            .into_iter()
            .find(|(s, _, _)| s == text)
            .unwrap_or_else(|| panic!("{text} not drawn"))
            .2
    }

    fn labels_of(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.label).collect()
    }

    const FULL: [&str; 7] = [
        "Link eth0 1 Gbit/s",
        "IPv4 192.168.1.23/24",
        "Gateway 192.168.1.1",
        "DNS 192.168.1.1",
        "VPN wg0 up",
        "Traffic ↓ 1.23 GB ↑ 456 MB",
        "Uptime 4 h 12 m",
    ];

    #[test]
    fn labels_left_values_right() {
        let geo = FontFamily::Name(crate::brand::GEO.into());
        let t = theme();
        for h in CELL_HEIGHTS {
            let out = tall(&sample(), h);
            assert_eq!(drawn(&out), FULL, "{h}");
            let rect = rect_of(TALL, h);
            let pad = sizes_for(h).pad_x;
            for (text, l, _, _, family) in texts(&out) {
                if ALL.contains(&text.as_str()) {
                    assert_eq!(family, geo, "{text}");
                    assert!((l - (rect.left() + pad)).abs() < 0.01, "{h} {text}: {l}");
                } else if text != "up" {
                    assert_eq!(family, FontFamily::Monospace, "{text}");
                }
            }
            for (words, _, right) in lines_in(&out, f32::MIN, f32::MAX) {
                assert!(
                    (right - (rect.right() - pad)).abs() < 0.5,
                    "{h} {words}: ends at {right}, not {}",
                    rect.right() - pad
                );
            }
            assert_eq!(colour_of(&out, "Gateway"), t.palette().muted_foreground);
            assert_eq!(colour_of(&out, "192.168.1.23/24"), t.palette().foreground);
            assert_eq!(colour_of(&out, "↓ 1.23 GB"), t.palette().foreground);
            assert_eq!(colour_of(&out, "up"), t.role(role::GOOD));
        }
    }

    #[test]
    fn the_vpn_row_shows_only_while_up() {
        for vpn in [Vpn::Down, Vpn::Absent] {
            let mut s = sample();
            s.info.vpn = vpn;
            assert!(!labels_of(&rows(&s, None)).contains(&"VPN"), "{vpn:?}");
            assert!(
                drawn(&tall(&s, STATUS_CELL_LARGE_H))
                    .iter()
                    .all(|r| !r.starts_with("VPN"))
            );
        }
    }

    #[test]
    fn unknown_facts_warn_measures_drop_out() {
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
        bare.totals = None;
        bare.uptime = None;
        let out = tall(&bare, STATUS_CELL_LARGE_H);
        assert_eq!(
            drawn(&out),
            [
                "Link wlan0 WiFi -61 dBm",
                "IPv4 no IPv4 address",
                "Gateway none",
                "DNS none"
            ]
        );
        assert_eq!(colour_of(&out, "no IPv4 address"), t.role(role::WARN));
        let nones: Vec<_> = text_boxes(&out).into_iter().filter(|b| b.0 == "none").collect();
        assert_eq!(nones.len(), 2);
        assert!(nones.iter().all(|b| b.2 == t.role(role::WARN)));
        let mut gone = sample();
        gone.info.iface = None;
        gone.info.link = Link::Unknown;
        let out = tall(&gone, STATUS_CELL_LARGE_H);
        assert_eq!(drawn(&out)[0], "Link no interface");
        assert_eq!(colour_of(&out, "no interface"), t.role(role::WARN));
    }

    #[test]
    fn a_device_route_has_a_direct_gateway() {
        let t = theme();
        let mut s = sample();
        s.info.iface = Some("wwan0".into());
        s.info.link = Link::Unknown;
        s.info.gateway = None;
        s.info.direct = true;
        let out = tall(&s, STATUS_CELL_LARGE_H);
        assert!(drawn(&out).contains(&"Gateway direct".to_string()), "{:?}", drawn(&out));
        assert_eq!(colour_of(&out, "direct"), t.palette().muted_foreground);
        // No default route at all: the warning stays.
        s.info.default_route = false;
        s.info.direct = false;
        let out = tall(&s, STATUS_CELL_LARGE_H);
        assert!(drawn(&out).contains(&"Gateway none".to_string()), "{:?}", drawn(&out));
        assert_eq!(colour_of(&out, "none"), t.role(role::WARN));
    }

    #[test]
    fn more_dns_servers_show_as_a_count() {
        let mut s = sample();
        s.info.dns = vec!["9.9.9.9".into(), "1.1.1.1".into(), "8.8.8.8".into()];
        let out = tall(&s, STATUS_CELL_LARGE_H);
        assert!(drawn(&out).contains(&"DNS 9.9.9.9 +2".to_string()), "{:?}", drawn(&out));
        assert_eq!(colour_of(&out, "+2"), theme().palette().muted_foreground);
    }

    #[test]
    fn the_wide_size_splits_rows_in_two_level_columns() {
        let addr = public();
        for h in CELL_HEIGHTS {
            let rect = rect_of(WIDE, h);
            let pad = sizes_for(h).pad_x;
            let mid = rect.center().x;
            let out = shapes(&Connection, rect, WIDE, &with_public(&sample(), &addr), Instant::now());
            let left = lines_in(&out, f32::MIN, mid);
            let right = lines_in(&out, mid, f32::MAX);
            let words = |l: &[(String, f32, f32)]| l.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
            assert_eq!(words(&left), FULL[..4], "{h}");
            assert_eq!(
                words(&right),
                [
                    "VPN wg0 up",
                    "Public 203.0.113.7",
                    "Traffic ↓ 1.23 GB ↑ 456 MB",
                    "Uptime 4 h 12 m"
                ],
                "{h}"
            );
            for (a, b) in left.iter().zip(&right) {
                assert!((a.1 - b.1).abs() < 0.5, "{h}: {} and {} not level", a.0, b.0);
                assert!((a.2 - (mid - pad)).abs() < 0.5, "{h}: {} ends at {}", a.0, a.2);
                assert!((b.2 - (rect.right() - pad)).abs() < 0.5, "{h}: {} ends at {}", b.0, b.2);
            }
            // The right column's labels start at its own padding.
            for (text, l, ..) in texts(&out) {
                if ["VPN", "Public", "Traffic", "Uptime"].contains(&text.as_str()) {
                    assert!((l - (mid + pad)).abs() < 0.01, "{h} {text}: {l}");
                }
            }
            // The left column holds the fixed rows whatever else shows, the
            // right the others in their order, both from the top.
            let mut s = sample();
            s.info.vpn = Vpn::Absent;
            let cases: [(&Snapshot, Option<&Answer>, &[&str]); 4] = [
                (&sample(), None, &["VPN", "Traffic", "Uptime"]),
                (&s, Some(&addr), &["Public", "Traffic", "Uptime"]),
                (&s, None, &["Traffic", "Uptime"]),
                (
                    &Snapshot {
                        uptime: None,
                        totals: None,
                        ..s.clone()
                    },
                    None,
                    &[],
                ),
            ];
            for (snap, public, want) in cases {
                let d = Data {
                    public_ip: public,
                    ..data(snap)
                };
                let out = shapes(&Connection, rect, WIDE, &d, Instant::now());
                let left = lines_in(&out, f32::MIN, mid);
                let right = lines_in(&out, mid, f32::MAX);
                assert_eq!(words(&left), FULL[..4], "{h} {want:?}");
                let labels: Vec<String> = right
                    .iter()
                    .map(|l| l.0.split(' ').next().unwrap().to_string())
                    .collect();
                assert_eq!(labels, want, "{h}");
                for (a, b) in left.iter().zip(&right) {
                    assert!((a.1 - b.1).abs() < 0.5, "{h}: {} and {} not level", a.0, b.0);
                }
            }
        }
    }

    /// The height of a 1 x 2 element with cells `cell_h` high.
    fn tall_height(cell_h: f32) -> f32 {
        2.0 * cell_h + crate::ui::TILE_GAP
    }

    #[test]
    fn nothing_leaves_out_at_the_shipped_sizes() {
        let addr = public();
        for (h, m) in [(STATUS_CELL_LARGE_H, STATUS_LARGE), (STATUS_CELL_SMALL_H, STATUS_SMALL)] {
            for (size, height) in [(TALL, tall_height(h)), (WIDE, h)] {
                let (r, row) = fit(rows(&sample(), Some(&addr)), height, columns(size), &m);
                assert_eq!(labels_of(&r), ALL, "{size} {h}");
                assert!(row >= min_row(&m), "{size} {h}: {row}");
                let out = shapes(
                    &Connection,
                    rect_of(size, h),
                    size,
                    &with_public(&sample(), &addr),
                    Instant::now(),
                );
                assert_eq!(
                    text_boxes(&out).iter().filter(|b| ALL.contains(&b.0.as_str())).count(),
                    8
                );
            }
        }
        // The 1 x 2 rows are full lines at either height.
        for (h, m) in [(STATUS_CELL_LARGE_H, STATUS_LARGE), (STATUS_CELL_SMALL_H, STATUS_SMALL)] {
            assert_eq!(fit(rows(&sample(), Some(&addr)), tall_height(h), 1, &m).1, m.row);
        }
        // The row is no taller than a line, however much room there is.
        assert_eq!(fit(rows(&sample(), None), 400.0, 1, &STATUS_LARGE).1, STATUS_LARGE.row);
    }

    #[test]
    fn rows_leave_out_in_rank_order() {
        let addr = public();
        for (cols, m) in [(1, STATUS_LARGE), (2, STATUS_SMALL)] {
            let mut seen: Vec<Vec<&str>> = Vec::new();
            for h in (5..=200).rev() {
                let got: Vec<&'static str> = fit(rows(&sample(), Some(&addr)), h as f32, cols, &m)
                    .0
                    .iter()
                    .map(|r| r.label)
                    .collect();
                assert!(got.contains(&"Link") && got.contains(&"IPv4"), "{h}: {got:?}");
                if seen.last() != Some(&got) {
                    seen.push(got);
                }
            }
            // In two columns a step can take a row of each column; the
            // order holds within each column.
            let all = rows(&sample(), Some(&addr));
            let rank = |l: &str| all.iter().find(|r| r.label == l).unwrap().drop;
            let gone: Vec<&str> = seen
                .windows(2)
                .flat_map(|w| {
                    let mut g: Vec<&str> = w[0].iter().filter(|x| !w[1].contains(x)).copied().collect();
                    g.sort_by_key(|l| std::cmp::Reverse(rank(l)));
                    g
                })
                .collect();
            let order = ["Uptime", "Traffic", "DNS", "Gateway", "Public", "VPN"];
            if cols == 1 {
                assert_eq!(gone, order);
            } else {
                let side = |right: bool| {
                    let of = |l: &str| all.iter().any(|r| r.label == l && r.right == right);
                    (
                        gone.iter().copied().filter(|l| of(l)).collect::<Vec<_>>(),
                        order.iter().copied().filter(|l| of(l)).collect::<Vec<_>>(),
                    )
                };
                for right in [false, true] {
                    let (got, want) = side(right);
                    assert_eq!(got, want, "right column: {right}");
                }
            }
        }
        // Squeezed, the rows first move together as far as `min_row`.
        let eight = 2.0 * STATUS_LARGE.pad_y + 8.0 * min_row(&STATUS_LARGE);
        let (r, h) = fit(rows(&sample(), Some(&addr)), eight, 1, &STATUS_LARGE);
        assert_eq!(r.len(), 8);
        assert!((h - min_row(&STATUS_LARGE)).abs() < 0.01, "{h}");
    }

    #[test]
    fn in_two_columns_no_column_holds_more_rows_than_fit() {
        let addr = public();
        let m = STATUS_SMALL;
        for lines in 1..=4 {
            let h = 2.0 * m.pad_y + lines as f32 * min_row(&m) + 0.5;
            let (r, _) = fit(rows(&sample(), Some(&addr)), h, 2, &m);
            let left = r.iter().filter(|x| !x.right).count();
            let right = r.iter().filter(|x| x.right).count();
            assert!(left <= lines && right <= lines, "{lines}: {:?}", labels_of(&r));
        }
        // Three lines: each column loses its own highest rank, the left
        // DNS, the right Uptime; Traffic stays.
        let h = 2.0 * m.pad_y + 3.0 * min_row(&m) + 0.5;
        let (r, _) = fit(rows(&sample(), Some(&addr)), h, 2, &m);
        assert_eq!(labels_of(&r), ["Link", "IPv4", "Gateway", "VPN", "Public", "Traffic"]);
    }

    #[test]
    fn with_one_line_the_wide_size_puts_link_and_ipv4_side_by_side() {
        let addr = public();
        let m = STATUS_SMALL;
        let h = 2.0 * m.pad_y + min_row(&m) + 0.5;
        let (r, _) = fit(rows(&sample(), Some(&addr)), h, 2, &m);
        assert_eq!(labels_of(&r), ["Link", "IPv4"]);
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(2.0 * 120.0, h));
        let out = shapes(&Connection, rect, WIDE, &with_public(&sample(), &addr), Instant::now());
        let mid = rect.center().x;
        assert_eq!(drawn_words(&lines_in(&out, f32::MIN, mid)), ["Link eth0 1 Gbit/s"]);
        assert_eq!(drawn_words(&lines_in(&out, mid, f32::MAX)), ["IPv4 192.168.1.23/24"]);
        for s in &out {
            assert!(
                rect.expand(0.01).contains_rect(s.visual_bounding_rect()),
                "{s:?} outside {rect:?}"
            );
        }
    }

    fn drawn_words(l: &[(String, f32, f32)]) -> Vec<String> {
        l.iter().map(|x| x.0.clone()).collect()
    }

    #[test]
    fn the_public_row_shows_the_answer() {
        let t = theme();
        let out = shapes(
            &Connection,
            rect_of(TALL, STATUS_CELL_LARGE_H),
            TALL,
            &with_public(&sample(), &Answer::Failed),
            Instant::now(),
        );
        let d = drawn(&out);
        assert_eq!(d[5], "Public –", "{d:?}");
        assert_eq!(colour_of(&out, "–"), t.palette().muted_foreground);
        // No answer: no row.
        assert!(!labels_of(&rows(&sample(), None)).contains(&"Public"));
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
        // The room runs from the label, so a USB stick's name keeps it too
        // on the large cell.
        assert_eq!(
            row_of("wlp0s20f3u1", STATUS_CELL_LARGE_H),
            "Link wlp0s20f3u1 WiFi -61 dBm"
        );
        // A name that does not fit with the word loses it, and keeps the
        // rest.
        for h in CELL_HEIGHTS {
            assert_eq!(
                row_of("wlx00e04c123456", h),
                "Link wlx00e04c123456 -61 dBm",
                "{h}: the word goes first"
            );
        }
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
        // The galley keeps the whole text; what it draws ends at the padding
        // and starts after the label.
        let whole = (name.len() + " 1 Gbit/s".len()) as f32;
        assert!(link.1.width() < whole * 0.6 * sizes_for(h).value, "{:?}", link.1);
        assert!(link.1.right() <= right + 0.5, "{:?} past {right}", link.1);
        let label = text_boxes(&out).into_iter().find(|b| b.0 == "Link").unwrap();
        assert!(link.1.left() > label.1.right(), "{:?} into {:?}", link.1, label.1);
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
        odd.info.dns = vec!["1.2.3.4\x07".into()];
        let r = rows(&odd, None);
        assert_eq!(r[0].runs[0].text, "<b>&\"'?");
        assert_eq!(r[3].runs[0].text, "1.2.3.4?");
        let d = drawn(&tall(&odd, STATUS_CELL_LARGE_H));
        assert!(d.iter().any(|r| r.starts_with("Link <b>&\"'?")), "{d:?}");
    }

    /// A machine with long names and the longest addresses.
    fn hostile() -> Snapshot {
        let mut s = sample();
        s.info.iface = Some("e".repeat(40));
        s.info.link = Link::Wifi(Some(-61));
        s.info.dns = vec![
            "1111:2222:3333:4444:5555:6666:7777:8888".into(),
            "2222:2222:3333:4444:5555:6666:7777:8888".into(),
        ];
        s.totals = Some((999_999_999_999_999, 999_999_999_999_999));
        s.uptime = Some(9_999 * 86_400);
        s
    }

    fn v6() -> Answer {
        Answer::Addr("1111:2222:3333:4444:5555:6666:7777:8888".parse().unwrap())
    }

    #[test]
    fn no_text_overlaps_another_or_leaves_its_column() {
        let mut busy = sample();
        busy.info.iface = Some("enx00e04c123456".into());
        busy.info.ipv4 = Some((std::net::Ipv4Addr::new(192, 168, 100, 123), 24));
        busy.info.gateway = Some(std::net::Ipv4Addr::new(192, 168, 100, 254));
        busy.info.link = Link::Wired(Some(2500));
        busy.info.dns = vec!["192.168.100.254".into(), "9.9.9.9".into()];
        let (h, a) = (hostile(), v6());
        for (snap, public) in [(&busy, Some(&a)), (&h, Some(&a)), (&busy, None)] {
            for cell in [STATUS_CELL_LARGE_H, STATUS_CELL_SMALL_H, 70.0] {
                for size in [TALL, WIDE] {
                    let rect = rect_of(size, cell);
                    let pad = sizes_for(cell).pad_x;
                    let d = Data {
                        public_ip: public,
                        ..data(snap)
                    };
                    let b = text_boxes(&shapes(&Connection, rect, size, &d, Instant::now()));
                    let half = rect.width() / columns(size) as f32;
                    for (i, (ta, a, _)) in b.iter().enumerate() {
                        assert!(rect.contains_rect(*a), "{size} {cell}: {ta} {a:?}");
                        let c = ((a.left() - rect.left()) / half).floor();
                        let right = rect.left() + (c + 1.0) * half - pad;
                        assert!(a.right() <= right + 0.5, "{size} {cell}: {ta} {a:?} past {right}");
                        for (tb, c, _) in &b[i + 1..] {
                            // The ink boxes of two Geo labels one row apart
                            // may touch by a descender's rounding (the
                            // shots show no collision), never by more.
                            let (a, c) = (a.shrink2(vec2(0.0, 0.75)), c.shrink2(vec2(0.0, 0.75)));
                            assert!(!a.intersects(c), "{size} {cell}: {ta} {a:?} overlaps {tb} {c:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn connection_draws_inside_its_rect_at_every_size() {
        let now = Instant::now();
        let (h, a) = (hostile(), v6());
        let hostile_data = with_public(&h, &a);
        testing::assert_inside(&Connection, &hostile_data, now);
        testing::assert_inside(&Connection, &data(&sample()), now);
        for size in SIZES {
            for cell in [STATUS_CELL_LARGE_H, STATUS_CELL_SMALL_H, 70.3] {
                let rect = rect_of(*size, cell).translate(vec2(0.37, 0.61));
                let out = shapes(&Connection, rect, *size, &hostile_data, now);
                for s in &out {
                    assert!(
                        rect.expand(0.01).contains_rect(s.visual_bounding_rect()),
                        "{size} at {cell}: {s:?} outside {rect:?}"
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
