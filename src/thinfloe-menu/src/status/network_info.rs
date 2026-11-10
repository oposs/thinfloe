//! The network list element: one fact per row, the label in Geo on the left,
//! the value from one column: the rates, the peak, the total and since when,
//! the interface and link, the IPv4 address, the gateway, DNS and the VPN. It
//! draws from the same `netstat::Snapshot` as the graph and has content when
//! the graph has.
//!
//! Rows move closer together as far as `min_row`, then rows are left out in
//! the order `Row::drop`: further DNS servers (the last first), the peak, the
//! since, the total, DNS, the gateway, the link. The rates, the address and
//! the VPN stay. A WiFi link row too wide for the column leaves out the word
//! `WiFi`. A long value is cut at the padding. On the tightest rows of the
//! largest cell the labels are half a point smaller, so a `p` does not reach
//! the `k` under it (`label_in_row`).

use super::battery::cut;
use super::{Data, Element, Size, Source, background, cell_height, sizes_for};
use crate::brand::{StatusSizes, role};
use crate::netstat::{self, Link, Since, Snapshot, Vpn};
use egui::{Color32, FontId, Rect, pos2};
use shadcn_egui::Theme;
use std::time::{Duration, Instant};

pub struct NetworkInfo;

const SIZES: &[Size] = &[Size { w: 1, h: 2 }, Size { w: 2, h: 1 }];

/// The arrows before a rate.
const DOWN: &str = "↓";
const UP: &str = "↑";

/// Every label there can be; the value column starts after the widest, so it
/// does not move when a row is left out.
const LABELS: [&str; 10] = [
    "Down", "Up", "Peak", "Total", "Since", "Link", "IPv4", "Gateway", "DNS", "VPN",
];

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
    /// Received.
    Rx,
    /// Sent.
    Tx,
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
        Tone::Rx => t.palette().primary,
        Tone::Tx | Tone::Good => t.role(role::GOOD),
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

/// The rows in their order.
pub fn rows(snap: &Snapshot, since: &Since) -> Vec<Row> {
    let fg = Tone::Normal;
    let i = &snap.info;
    let (rx, tx) = snap.current();
    let (prx, ptx) = snap.peak();
    let total = (
        snap.total.0.saturating_sub(since.base.0),
        snap.total.1.saturating_sub(since.base.1),
    );
    let since_runs = match since.label.strip_prefix("since ") {
        Some(t) if t.starts_with(|c: char| c.is_ascii_digit()) => vec![value(t, fg)],
        Some(t) => vec![label(t, Tone::Muted)],
        None => vec![label(since.label.as_str(), Tone::Muted)],
    };
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
    let gw = match (i.default_route, i.gateway) {
        (false, _) => vec![label("no default route", Tone::Warn)],
        (true, Some(g)) => vec![value(g.to_string(), fg)],
        (true, None) => vec![label("no gateway", Tone::Muted)],
    };
    let vpn = match i.vpn {
        Vpn::Up => vec![value(netstat::VPN_IF, fg), label("up", Tone::Good)],
        Vpn::Down => vec![value(netstat::VPN_IF, fg), label("down", Tone::Warn)],
        Vpn::Absent => vec![label("no VPN", Tone::Muted)],
    };
    let row = |label: &'static str, runs: Vec<Run>, drop: u8| Row { label, runs, drop };
    let mut out = vec![
        row(
            "Down",
            vec![value(DOWN, Tone::Rx), value(netstat::bits(rx), Tone::Rx)],
            0,
        ),
        row("Up", vec![value(UP, Tone::Tx), value(netstat::bits(tx), Tone::Tx)], 0),
        row("Peak", vec![value(netstat::bits(prx.max(ptx)), fg)], 7),
        row(
            "Total",
            vec![
                value(DOWN, Tone::Rx),
                value(netstat::bytes(total.0), fg),
                value(UP, Tone::Tx),
                value(netstat::bytes(total.1), fg),
            ],
            5,
        ),
        row("Since", since_runs, 6),
        row("Link", link, 2),
        row("IPv4", ip, 0),
        row("Gateway", gw, 3),
    ];
    if i.dns.is_empty() {
        out.push(row("DNS", vec![label("no DNS server", Tone::Warn)], 4));
    }
    for (n, server) in i.dns.iter().enumerate() {
        // Further servers have no label of their own and go first, the last
        // before the one above it.
        let (name, drop) = if n == 0 { ("DNS", 4) } else { ("", 7 + n as u8) };
        out.push(row(name, vec![value(server.as_str(), fg)], drop));
    }
    out.push(row("VPN", vpn, 0));
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
fn paint(ui: &egui::Ui, rect: Rect, size: Size, snap: &Snapshot, since: &Since) {
    let t = Theme::current(ui.ctx());
    let p = ui.painter();
    let m = sizes_for(cell_height(rect, size));
    let (rows, row_h) = fit(rows(snap, since), rect.height(), &m);
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

impl Element for NetworkInfo {
    fn id(&self) -> &'static str {
        "network-info"
    }

    fn name(&self) -> &'static str {
        "Network details"
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
        let none = Since::default();
        if let Some(snap) = data.network {
            paint(ui, rect, size, snap, data.network_since.unwrap_or(&none));
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
        total: (1_234_000_000, 98_700_000),
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

    fn data<'a>(s: &'a Snapshot, since: &'a Since) -> Data<'a> {
        Data {
            network: Some(s),
            network_since: Some(since),
            ..Data::EMPTY
        }
    }

    fn draw(s: &Snapshot, since: &Since, size: Size, h: f32) -> Vec<Shape> {
        shapes(&NetworkInfo, rect_of(size, h), size, &data(s, since), Instant::now())
    }

    fn tall(s: &Snapshot, h: f32) -> Vec<Shape> {
        draw(s, &Since::default(), TALL, h)
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
            [
                "Down ↓ 4.00 Mbit/s",
                "Up ↑ 600 kbit/s",
                "Peak 11.2 Mbit/s",
                "Total ↓ 1.23 GB ↑ 98.7 MB",
                "Since menu start",
                "Link eth0 1 Gbit/s",
                "IPv4 192.168.1.23/24",
                "Gateway 192.168.1.1",
                "DNS 192.168.1.1",
                "VPN wg0 up",
            ]
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
            } else if text.contains("192.168") || text.contains("Gbit") || text.contains("Mbit") {
                assert_eq!(family, FontFamily::Monospace, "{text}");
                if text.contains("192.168") || text.contains("Gbit") {
                    let c = *column.get_or_insert(x);
                    assert!((x - c).abs() < 0.01, "{text}: {x} vs {c}");
                }
            }
        }
        assert!(column.unwrap() > rect.left() + pad + 30.0);
    }

    #[test]
    fn the_rates_are_in_the_graphs_colours() {
        let t = theme();
        let out = tall(&sample(), STATUS_CELL_LARGE_H);
        assert_eq!(colour_of(&out, "4.00 Mbit/s"), t.palette().primary);
        assert_eq!(colour_of(&out, "600 kbit/s"), t.role(role::GOOD));
        assert_eq!(colour_of(&out, "Down"), t.palette().muted_foreground);
        assert_eq!(colour_of(&out, "192.168.1.23/24"), t.palette().foreground);
        assert_eq!(colour_of(&out, "up"), t.role(role::GOOD));
    }

    #[test]
    fn the_session_total_counts_from_its_start() {
        let since = Since {
            job: Some(3),
            label: "since 14:02".into(),
            base: (1_000_000_000, 600_000),
        };
        let d = drawn(&draw(&sample(), &since, TALL, STATUS_CELL_LARGE_H));
        assert!(d.contains(&"Total ↓ 234 MB ↑ 98.1 MB".to_string()), "{d:?}");
        assert!(d.contains(&"Since 14:02".to_string()), "{d:?}");
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
        let d = drawn(&out);
        for want in [
            "Link wlan0 WiFi -61 dBm",
            "IPv4 no IPv4 address",
            "Gateway no default route",
            "DNS no DNS server",
            "VPN wg0 down",
        ] {
            assert!(d.contains(&want.to_string()), "{want}: {d:?}");
        }
        for text in ["no IPv4 address", "no default route", "no DNS server", "down"] {
            assert_eq!(colour_of(&out, text), t.role(role::WARN), "{text}");
        }
        let mut up = sample();
        up.info.default_route = true;
        up.info.gateway = None;
        up.info.vpn = Vpn::Absent;
        up.info.link = Link::Unknown;
        up.info.iface = None;
        let d = drawn(&tall(&up, STATUS_CELL_LARGE_H));
        for want in ["Link no interface", "Gateway no gateway", "VPN no VPN"] {
            assert!(d.contains(&want.to_string()), "{want}: {d:?}");
        }
    }

    fn labels_of(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.label).collect()
    }

    /// The height of a 1 x 2 element with cells `cell_h` high.
    fn tall_height(cell_h: f32) -> f32 {
        2.0 * cell_h + crate::ui::TILE_GAP
    }

    #[test]
    fn rows_tighten_first_then_further_dns_then_the_peak() {
        let mut many = sample();
        many.info.dns = vec!["192.168.1.1".into(), "9.9.9.9".into(), "1.1.1.1".into()];
        let none = Since::default();
        // 76 pt: 143 pt of room, rows of 13 at the tightest, so 11: of 12
        // rows only the third server goes.
        let (r, h) = fit(rows(&many, &none), tall_height(STATUS_CELL_LARGE_H), &STATUS_LARGE);
        assert_eq!(r.len(), 11, "{:?}", labels_of(&r));
        assert_eq!(h, 13.0);
        assert!(r.iter().any(|x| x.runs[0].text == "9.9.9.9"));
        assert!(!r.iter().any(|x| x.runs[0].text == "1.1.1.1"));
        assert!(labels_of(&r).contains(&"Peak"));
        // 64 pt: 121 pt, rows of 12, so 10: both further servers go, and the
        // peak stays.
        let (r, h) = fit(rows(&many, &none), tall_height(STATUS_CELL_SMALL_H), &STATUS_SMALL);
        assert_eq!(r.len(), 10);
        assert!((h - 12.1).abs() < 0.01);
        assert!(!r.iter().any(|x| x.runs[0].text == "9.9.9.9"));
        assert!(labels_of(&r).contains(&"Peak"));
        // A usual network loses nothing at either height.
        for (h, m) in [(STATUS_CELL_LARGE_H, STATUS_LARGE), (STATUS_CELL_SMALL_H, STATUS_SMALL)] {
            assert_eq!(fit(rows(&sample(), &none), tall_height(h), &m).0.len(), 10, "{h}");
        }
        // The row is no taller than a line, however much room there is.
        assert_eq!(fit(rows(&sample(), &none), 400.0, &STATUS_LARGE).1, STATUS_LARGE.row);
        // Squeezed further the order goes on; rates, address and VPN stay.
        let mut seen = Vec::new();
        for h in (20..=150).rev() {
            let got: Vec<String> = labels_of(&fit(rows(&sample(), &none), h as f32, &STATUS_LARGE).0)
                .iter()
                .map(|s| s.to_string())
                .collect();
            for keep in ["Down", "Up", "IPv4", "VPN"] {
                assert!(got.iter().any(|g| g == keep), "{h}: {keep} in {got:?}");
            }
            if seen.last() != Some(&got) {
                seen.push(got);
            }
        }
        let gone =
            |a: &[String], b: &[String]| -> Vec<String> { a.iter().filter(|x| !b.contains(x)).cloned().collect() };
        let order: Vec<String> = seen.windows(2).flat_map(|w| gone(&w[0], &w[1])).collect();
        assert_eq!(order, ["Peak", "Since", "Total", "DNS", "Gateway", "Link"]);
    }

    #[test]
    fn the_wide_size_keeps_the_rates_the_address_and_the_vpn() {
        for h in CELL_HEIGHTS {
            let d = drawn(&draw(&sample(), &Since::default(), WIDE, h));
            assert_eq!(
                d,
                [
                    "Down ↓ 4.00 Mbit/s",
                    "Up ↑ 600 kbit/s",
                    "IPv4 192.168.1.23/24",
                    "VPN wg0 up"
                ],
                "{h}"
            );
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
        // The short name keeps the word at either height.
        for h in CELL_HEIGHTS {
            assert_eq!(row_of("wlan0", h), "Link wlan0 WiFi -61 dBm", "{h}");
        }
        // A name that does not fit with the word loses it, and keeps the
        // rest.
        assert_eq!(
            row_of("wlp0s20f3", STATUS_CELL_LARGE_H),
            "Link wlp0s20f3 -61 dBm",
            "the word goes first"
        );
        assert_eq!(
            row_of("wlx00e04c123456", STATUS_CELL_SMALL_H),
            "Link wlx00e04c123 -61 dBm".replace("wlx00e04c123", "wlx00e04c123456")
        );
    }

    #[test]
    fn a_long_value_is_cut_at_the_padding() {
        let mut long = sample();
        long.info.dns = vec!["fd00:1234:5678:9abc:def0:1234:5678:9abc".into()];
        let h = STATUS_CELL_LARGE_H;
        let rect = rect_of(TALL, h);
        let out = tall(&long, h);
        let right = rect.right() - sizes_for(h).pad_x;
        let dns = text_boxes(&out)
            .into_iter()
            .find(|(s, _, _)| s.starts_with("fd00"))
            .expect("the address is drawn, cut");
        // The galley keeps the whole text; what it draws ends at the padding.
        assert!(dns.1.width() < 39.0 * 0.6 * sizes_for(h).value, "{:?}", dns.1);
        assert!(dns.1.right() <= right + 0.5, "{:?} past {right}", dns.1);
        for (text, r, _) in text_boxes(&out) {
            assert!(r.right() <= right + 0.5, "{text}: {r:?}");
        }
    }

    #[test]
    fn names_from_the_system_never_break_a_row() {
        let mut odd = sample();
        odd.info.dns = vec!["<b>&\"'".into(), "1.1.1.1\x01".into()];
        odd.info.iface = Some("e<0>\n".into());
        let d = drawn(&tall(&odd, STATUS_CELL_LARGE_H));
        assert!(d.iter().any(|r| r.ends_with("<b>&\"'")), "{d:?}");
        assert!(d.contains(&"Link e<0>? 1 Gbit/s".to_string()), "{d:?}");
        let r = rows(&odd, &Since::default());
        assert!(r.iter().any(|x| x.runs[0].text == "1.1.1.1?"));
    }

    #[test]
    fn no_text_overlaps_another_or_leaves_the_element() {
        let since = Since {
            job: Some(1),
            label: "since 14:02".into(),
            base: (0, 0),
        };
        let mut busy = sample();
        busy.total = (987_654_321_000, 123_456_789_000);
        busy.info.dns = vec!["192.168.100.100".into(), "9.9.9.9".into(), "1.1.1.1".into()];
        busy.info.ipv4 = Some((std::net::Ipv4Addr::new(192, 168, 100, 123), 24));
        busy.info.gateway = Some(std::net::Ipv4Addr::new(192, 168, 100, 254));
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
                    let b = text_boxes(&draw(snap, &since, size, h));
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
    fn network_info_draws_inside_its_rect_at_every_size() {
        let now = Instant::now();
        let mut hostile = sample();
        // Six DNS servers, one of them very long, and a 40-character interface.
        hostile.info.dns = (0..6)
            .map(|i| format!("fd00:1234:5678:9abc:def0:1234:5678:{i}"))
            .collect();
        hostile.info.iface = Some("e".repeat(40));
        hostile.info.link = Link::Wifi(Some(-61));
        let since = Since {
            job: Some(1),
            label: "since 14:02".into(),
            base: (0, 0),
        };
        testing::assert_inside(&NetworkInfo, &data(&hostile, &since), now);
        testing::assert_inside(&NetworkInfo, &data(&sample(), &since), now);
        for size in SIZES {
            for h in [STATUS_CELL_LARGE_H, STATUS_CELL_SMALL_H, 70.3] {
                let rect = rect_of(*size, h).translate(vec2(0.37, 0.61));
                let out = shapes(&NetworkInfo, rect, *size, &data(&hostile, &since), now);
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
        assert_eq!(label_in_row(STATUS_LARGE.label, 14.3), STATUS_LARGE.label, "ten rows");
        assert_eq!(label_in_row(STATUS_LARGE.label, 13.0), 14.5, "eleven rows");
        assert_eq!(label_in_row(STATUS_SMALL.label, 12.1), STATUS_SMALL.label);
    }

    #[test]
    fn it_has_content_when_the_graph_has() {
        let s = sample();
        let none = Since::default();
        assert!(NetworkInfo.has_content(&data(&s, &none)));
        assert!(!NetworkInfo.has_content(&Data::EMPTY));
        let nothing = Snapshot::default();
        assert!(!NetworkInfo.has_content(&data(&nothing, &none)));
        assert_eq!(
            NetworkInfo.has_content(&data(&nothing, &none)),
            super::super::network::Network.has_content(&data(&nothing, &none))
        );
        assert_eq!(NetworkInfo.refresh(), Some(Duration::from_secs(1)));
        assert_eq!(
            NetworkInfo.next_change(&data(&s, &none), rect_of(TALL, 76.0), Instant::now()),
            None
        );
        assert_eq!(NetworkInfo.sizes(), [TALL, WIDE]);
        assert_eq!(NetworkInfo.short_size(), None);
        assert_eq!(NetworkInfo.sources(), [Source::Network]);
    }
}
