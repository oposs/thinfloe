//! The network list (`Type=network-info`, shipped 1 x 2 right of the graph):
//! one fact per row, the label in Geo on the left, the value from one column:
//! the rates, the peak, the total and since when, the interface and link, the
//! IPv4 address, the gateway, DNS and the VPN. It draws from the same
//! `network::Snapshot` as the graph and is hidden when the graph is.
//!
//! Rows move closer together as far as `min_row`, then rows are left out in
//! the order `Row::drop`: further DNS servers (the last first), the peak, the
//! since, the total, DNS, the gateway, the link. The rates, the address and
//! the VPN stay. A WiFi link row too wide for the column leaves out the word
//! `WiFi`. A long value is cut at the padding. On the tightest rows of
//! the largest cell the labels are half a point smaller, so a `p` does not
//! reach the `k` under it (`style::label_in_row`).

use super::style::{self, DOWN, Piece, Run, Style, UP, arrow, label, label_width, value};
use super::{Element, Kind, Room, Sources};
use crate::desktop::Entry;
use crate::network::{self, Link, Since, Snapshot, Vpn};
use std::time::{Duration, Instant};

pub const KIND: Kind = Kind {
    name: "network-info",
    options: &[],
    build,
};

fn build(_: &Entry) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(NetworkInfo))
}

pub struct NetworkInfo;

/// Every label there can be; the value column starts after the widest, so it
/// does not move when a row is left out.
const LABELS: [&str; 10] = [
    "Down", "Up", "Peak", "Total", "Since", "Link", "IPv4", "Gateway", "DNS", "VPN",
];

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
    let fg = style::foreground();
    let i = &snap.info;
    let (rx, tx) = snap.current();
    let (prx, ptx) = snap.peak();
    let total = (
        snap.total.0.saturating_sub(since.base.0),
        snap.total.1.saturating_sub(since.base.1),
    );
    let since_runs = match since.label.strip_prefix("since ") {
        Some(t) if t.starts_with(|c: char| c.is_ascii_digit()) => vec![value(t, fg)],
        Some(t) => vec![label(t, style::MUTED)],
        None => vec![label(since.label.as_str(), style::MUTED)],
    };
    let link = match (&i.iface, i.link) {
        (None, _) => vec![label("no interface", style::WARN)],
        (Some(n), Link::Wired(Some(mbit))) => vec![value(format!("{n} {}", network::speed(mbit)), fg)],
        (Some(n), Link::Wifi(Some(dbm))) => vec![
            value(n.as_str(), fg),
            label("WiFi", style::MUTED),
            value(format!("{dbm} dBm"), fg),
        ],
        (Some(n), Link::Wifi(None)) => vec![value(n.as_str(), fg), label("WiFi", style::MUTED)],
        (Some(n), Link::Wired(None) | Link::Unknown) => vec![value(n.as_str(), fg)],
    };
    let ip = match i.ipv4 {
        Some((a, p)) => vec![value(format!("{a}/{p}"), fg)],
        None => vec![label("no IPv4 address", style::WARN)],
    };
    let gw = match (i.default_route, i.gateway) {
        (false, _) => vec![label("no default route", style::WARN)],
        (true, Some(g)) => vec![value(g.to_string(), fg)],
        (true, None) => vec![label("no gateway", style::MUTED)],
    };
    let vpn = match i.vpn {
        Vpn::Up => vec![value(network::VPN_IF, fg), label("up", style::GOOD)],
        Vpn::Down => vec![value(network::VPN_IF, fg), label("down", style::WARN)],
        Vpn::Absent => vec![label("no VPN", style::MUTED)],
    };
    let row = |label: &'static str, runs: Vec<Run>, drop: u8| Row { label, runs, drop };
    let mut out = vec![
        row(
            "Down",
            vec![arrow(DOWN, style::RX), value(network::bits(rx), style::RX)],
            0,
        ),
        row("Up", vec![arrow(UP, style::TX), value(network::bits(tx), style::TX)], 0),
        row("Peak", vec![value(network::bits(prx.max(ptx)), fg)], 7),
        row(
            "Total",
            vec![
                arrow(DOWN, style::RX),
                value(network::bytes(total.0), fg),
                arrow(UP, style::TX),
                value(network::bytes(total.1), fg),
            ],
            5,
        ),
        row("Since", since_runs, 6),
        row("Link", link, 2),
        row("IPv4", ip, 0),
        row("Gateway", gw, 3),
    ];
    if i.dns.is_empty() {
        out.push(row("DNS", vec![label("no DNS server", style::WARN)], 4));
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
pub fn min_row(st: &Style) -> f32 {
    st.value + 1.0
}

/// The rows that fit `room` and how high each is: as tall as a line when
/// there is room, as tight as `min_row` before any row is left out.
pub fn fit(mut rows: Vec<Row>, room: Room) -> (Vec<Row>, f32) {
    let st = room.style();
    let avail = (room.size.y - 2.0 * st.pad_y).max(0.0);
    let fits = (avail / min_row(&st)).floor() as usize;
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
    let row = (avail / rows.len().max(1) as f32).min(st.line).max(min_row(&st));
    (rows, row)
}

/// Where the values start.
pub fn value_x(st: &Style) -> f32 {
    let widest = LABELS.iter().map(|l| label_width(l, st.label)).fold(0.0, f32::max);
    st.pad_x + widest + st.gap
}

/// The SVG of `snap` in `room`.
pub fn svg(snap: &Snapshot, since: &Since, room: Room) -> String {
    let st = room.style();
    let (w, h) = (room.size.x, room.size.y);
    let x = value_x(&st);
    let (rows, row_h) = fit(rows(snap, since), room);
    let top = (h - rows.len() as f32 * row_h) / 2.0;
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\
         <rect width=\"{w}\" height=\"{h}\" fill=\"{}\"/>\
         <clipPath id=\"value\"><rect x=\"{x}\" width=\"{}\" height=\"{h}\"/></clipPath>",
        style::hex(style::BACKGROUND),
        (w - st.pad_x - x).max(0.0),
    );
    for (i, r) in rows.iter().enumerate() {
        let y = style::baseline(top + i as f32 * row_h, row_h, st.value);
        if !r.label.is_empty() {
            style::text(
                &mut s,
                st.pad_x,
                y,
                style::Face::Label,
                style::label_in_row(st.label, row_h),
                style::MUTED,
                style::Anchor::Start,
                r.label,
            );
        }
        s.push_str("<g clip-path=\"url(#value)\">");
        let mut piece = Piece { runs: r.runs.clone() };
        // A row too wide for the column leaves out the word `WiFi`.
        if piece.width(&st) > w - st.pad_x - x {
            piece
                .runs
                .retain(|run| !(run.face == style::Face::Label && run.text == "WiFi"));
        }
        style::draw_line(&mut s, x, y, &piece, &st);
        s.push_str("</g>");
    }
    s.push_str("</svg>");
    s
}

impl Element for NetworkInfo {
    fn visible(&self, data: &Sources) -> bool {
        data.network.is_some_and(Snapshot::visible)
    }

    fn svg(&self, data: &Sources, room: Room, _: Instant) -> String {
        let since = Since::default();
        match data.network {
            Some(snap) => svg(snap, data.network_since.unwrap_or(&since), room),
            None => String::new(),
        }
    }

    fn next_change(&self, _: &Sources, _: Room, _: Instant) -> Option<Duration> {
        // Its file's Refresh=1 draws it every second while the menu shows.
        None
    }
}

/// A network as it might be: for the tests and the pictures.
#[cfg(test)]
pub fn sample() -> Snapshot {
    use crate::network::Info;
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
    use super::*;
    use crate::network::Info;
    use crate::status::Cells;
    use crate::status::render::{self, GEO_FAMILY, MONO_FAMILY};
    use crate::status::style::{LARGE, SMALL, mono_width};
    use crate::ui::{TILE_GAP, TILE_H, TILE_H_MIN};
    use egui::vec2;

    fn shipped(cell_h: f32) -> Room {
        Cells::parse("3,0,1,2").unwrap().room(vec2(220.0, cell_h), TILE_GAP)
    }

    /// Each row as the words it draws, by baseline.
    fn drawn(svg: &str) -> Vec<String> {
        let doc = resvg::usvg::roxmltree::Document::parse(svg).expect("well-formed");
        let mut rows: Vec<(f32, Vec<String>)> = Vec::new();
        for n in doc.descendants() {
            let item = if n.has_tag_name("text") {
                n.attribute("y")
                    .and_then(|y| y.parse().ok())
                    .map(|y: f32| (n.text().unwrap_or("").to_string(), y))
            } else if let (Some(a), Some(t)) = (n.attribute("aria-label"), n.attribute("transform")) {
                let y = t
                    .strip_prefix("translate(")
                    .and_then(|t| t.strip_suffix(')'))
                    .and_then(|t| t.split_once(' '));
                y.and_then(|(_, y)| y.parse().ok()).map(|y: f32| (a.to_string(), y))
            } else {
                None
            };
            let Some((text, y)) = item else { continue };
            match rows.iter_mut().find(|(at, _)| (*at - y).abs() < 0.01) {
                Some((_, words)) => words.push(text),
                None => rows.push((y, vec![text])),
            }
        }
        rows.into_iter().map(|(_, w)| w.join(" ")).collect()
    }

    #[test]
    fn one_fact_a_row_label_left_value_in_a_column() {
        let room = shipped(TILE_H);
        assert_eq!(room.size, vec2(220.0, 155.0));
        let s = svg(&sample(), &Since::default(), room);
        assert_eq!(
            drawn(&s),
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
        // Labels in Geo at the padding, numbers in Hack from one column.
        let doc = resvg::usvg::roxmltree::Document::parse(&s).unwrap();
        let x = value_x(&LARGE);
        for n in doc.descendants().filter(|n| n.has_tag_name("text")) {
            let t = n.text().unwrap();
            let at: f32 = n.attribute("x").unwrap().parse().unwrap();
            if LABELS.contains(&t) {
                assert_eq!(n.attribute("font-family"), Some(GEO_FAMILY), "{t}");
                assert_eq!(at, LARGE.pad_x, "{t}");
            } else if t.contains("192.168") || t.contains("Gbit") {
                assert_eq!(n.attribute("font-family"), Some(MONO_FAMILY), "{t}");
                assert!((at - x).abs() < 0.01, "{t}: {at} vs {x}");
            }
        }
    }

    #[test]
    fn the_session_total_counts_from_its_start() {
        let since = Since {
            job: Some(3),
            label: "since 14:02".into(),
            base: (1_000_000_000, 600_000),
        };
        let d = drawn(&svg(&sample(), &since, shipped(TILE_H)));
        assert!(d.contains(&"Total ↓ 234 MB ↑ 98.1 MB".to_string()), "{d:?}");
        assert!(d.contains(&"Since 14:02".to_string()), "{d:?}");
    }

    #[test]
    fn what_is_missing_keeps_its_words_and_colours() {
        let mut bare = sample();
        bare.info = Info {
            iface: Some("wlan0".into()),
            default_route: false,
            link: Link::Wifi(Some(-61)),
            addressed: true,
            vpn: Vpn::Down,
            ..Info::default()
        };
        let s = svg(&bare, &Since::default(), shipped(TILE_H));
        let d = drawn(&s);
        for want in [
            "Link wlan0 WiFi -61 dBm",
            "IPv4 no IPv4 address",
            "Gateway no default route",
            "DNS no DNS server",
            "VPN wg0 down",
        ] {
            assert!(d.contains(&want.to_string()), "{want}: {d:?}");
        }
        let doc = resvg::usvg::roxmltree::Document::parse(&s).unwrap();
        let fill = |t: &str| {
            doc.descendants()
                .find(|n| n.text() == Some(t))
                .and_then(|n| n.attribute("fill"))
                .unwrap()
                .to_string()
        };
        let warn = style::hex(style::WARN);
        for t in ["no IPv4 address", "no default route", "no DNS server", "down"] {
            assert_eq!(fill(t), warn, "{t}");
        }
        let mut up = sample();
        up.info.default_route = true;
        up.info.gateway = None;
        up.info.vpn = Vpn::Absent;
        up.info.link = Link::Unknown;
        up.info.iface = None;
        let d = drawn(&svg(&up, &Since::default(), shipped(TILE_H)));
        for want in ["Link no interface", "Gateway no gateway", "VPN no VPN"] {
            assert!(d.contains(&want.to_string()), "{want}: {d:?}");
        }
    }

    fn labels_of(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.label).collect()
    }

    #[test]
    fn rows_tighten_first_then_further_dns_then_the_peak() {
        let mut many = sample();
        many.info.dns = vec!["192.168.1.1".into(), "9.9.9.9".into(), "1.1.1.1".into()];
        // 76 pt: 143 pt of room, rows of 13 at the tightest, so 11: of 12
        // rows only the third server goes.
        let (r, h) = fit(rows(&many, &Since::default()), shipped(TILE_H));
        assert_eq!(r.len(), 11, "{:?}", labels_of(&r));
        assert_eq!(h, 13.0);
        assert!(r.iter().any(|x| x.runs[0].text == "9.9.9.9"));
        assert!(!r.iter().any(|x| x.runs[0].text == "1.1.1.1"));
        assert!(labels_of(&r).contains(&"Peak"));
        // 64 pt: 121 pt, rows of 12, so 10: both further servers go, and the
        // peak stays.
        let (r, h) = fit(rows(&many, &Since::default()), shipped(TILE_H_MIN));
        assert_eq!(r.len(), 10);
        assert!((h - 12.1).abs() < 0.01);
        assert!(!r.iter().any(|x| x.runs[0].text == "9.9.9.9"));
        assert!(labels_of(&r).contains(&"Peak"));
        // A usual network loses nothing at either height.
        for h in [TILE_H, TILE_H_MIN] {
            assert_eq!(fit(rows(&sample(), &Since::default()), shipped(h)).0.len(), 10, "{h}");
        }
        // The row is no taller than a line, however much room there is.
        let tall = Room {
            size: vec2(220.0, 400.0),
            cell: vec2(220.0, 76.0),
        };
        assert_eq!(fit(rows(&sample(), &Since::default()), tall).1, LARGE.line);
        // Squeezed further the order goes on; rates, address and VPN stay.
        let mut seen = Vec::new();
        for h in (20..=150).rev() {
            let room = Room {
                size: vec2(220.0, h as f32),
                cell: vec2(220.0, 76.0),
            };
            let got: Vec<String> = labels_of(&fit(rows(&sample(), &Since::default()), room).0)
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
    fn a_wifi_row_that_does_not_fit_leaves_out_the_word_wifi() {
        let wifi = |name: &str| {
            let mut snap = sample();
            snap.info.iface = Some(name.into());
            snap.info.link = Link::Wifi(Some(-61));
            snap
        };
        let row_of = |name: &str, h: f32| {
            drawn(&svg(&wifi(name), &Since::default(), shipped(h)))
                .into_iter()
                .find(|r| r.starts_with("Link"))
                .unwrap()
        };
        // At 76 pt the long name does not fit with the word; at 64 pt the
        // smaller text does, so it keeps the word there.
        assert_eq!(row_of("wlp0s20f3", TILE_H), "Link wlp0s20f3 -61 dBm");
        assert_eq!(row_of("wlp0s20f3", TILE_H_MIN), "Link wlp0s20f3 WiFi -61 dBm");
        // A name that does not fit even at 64 pt loses the word there too.
        assert_eq!(row_of("wlx00e04c123456", TILE_H_MIN), "Link wlx00e04c123456 -61 dBm");
        for h in [TILE_H, TILE_H_MIN] {
            assert_eq!(row_of("wlan0", h), "Link wlan0 WiFi -61 dBm", "{h}");
        }
    }

    #[test]
    fn a_long_value_is_cut_at_the_padding() {
        let mut long = sample();
        long.info.dns = vec!["fd00:1234:5678:9abc:def0:1234:5678:9abc".into()];
        let room = shipped(TILE_H);
        let s = svg(&long, &Since::default(), room);
        let doc = resvg::usvg::roxmltree::Document::parse(&s).unwrap();
        let clip = doc
            .descendants()
            .find(|n| n.attribute("id") == Some("value"))
            .and_then(|c| c.first_element_child())
            .unwrap();
        let right = clip.attribute("x").unwrap().parse::<f32>().unwrap()
            + clip.attribute("width").unwrap().parse::<f32>().unwrap();
        assert!((right - (220.0 - LARGE.pad_x)).abs() < 0.01, "{right}");
        let img = render::render(&s, [220, 155]).unwrap();
        // Nothing drawn in the right padding.
        for x in (220 - LARGE.pad_x as usize + 1)..220 {
            assert!((0..155).all(|y| img.pixels[y * 220 + x] == style::BACKGROUND), "{x}");
        }
    }

    #[test]
    fn names_from_the_system_are_escaped_and_a_control_character_blanks_nothing() {
        let mut odd = sample();
        odd.info.dns = vec!["<b>&\"'".into(), "1.1.1.1\x01".into()];
        odd.info.iface = Some("e<0>".into());
        let s = svg(&odd, &Since::default(), shipped(TILE_H));
        assert!(s.contains("&lt;b&gt;&amp;&quot;&apos;"), "{s}");
        assert!(s.contains("e&lt;0&gt;"), "{s}");
        assert!(s.contains("1.1.1.1?"), "{s}");
        render::render(&s, [220, 155]).expect("renders");
    }

    fn ink(svg: &str) -> Vec<(String, resvg::usvg::Rect)> {
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
        fn walk(g: &resvg::usvg::Group, out: &mut Vec<resvg::usvg::Rect>) {
            for n in g.children() {
                match n {
                    resvg::usvg::Node::Group(c) => walk(c, out),
                    resvg::usvg::Node::Text(t) => out.push(t.flattened().abs_bounding_box()),
                    _ => {}
                }
            }
        }
        let mut boxes = Vec::new();
        walk(tree.root(), &mut boxes);
        assert_eq!(boxes.len(), bodies.len());
        bodies.into_iter().zip(boxes).collect()
    }

    #[test]
    fn no_text_overlaps_another_with_the_real_fonts() {
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
            for (h, st) in [(TILE_H, LARGE), (TILE_H_MIN, SMALL), (70.0, Style::for_cell(70.0))] {
                let room = shipped(h);
                let right = room.size.x - st.pad_x;
                let b = ink(&svg(snap, &since, room));
                for (i, (ta, a)) in b.iter().enumerate() {
                    assert!(
                        a.top() >= 0.0 && a.bottom() <= room.size.y && a.left() >= 0.0,
                        "{h}: {ta} {a:?}"
                    );
                    assert!(a.right() <= right + 0.5, "{h}: {ta} {a:?} past the padding");
                    for (tb, c) in &b[i + 1..] {
                        let hit = a.left() < c.right()
                            && c.left() < a.right()
                            && a.top() < c.bottom()
                            && c.top() < a.bottom();
                        assert!(!hit, "{h}: {ta} {a:?} overlaps {tb} {c:?}");
                    }
                }
            }
        }
        assert!(mono_width("192.168.100.123/24", LARGE.value) + value_x(&LARGE) <= 220.0 - LARGE.pad_x);
    }

    #[test]
    fn labels_shrink_only_on_the_tightest_rows_of_the_large_cell() {
        assert_eq!(style::label_in_row(LARGE.label, 14.3), LARGE.label, "ten rows");
        assert_eq!(style::label_in_row(LARGE.label, 13.0), 14.5, "eleven rows");
        assert_eq!(style::label_in_row(SMALL.label, 12.1), SMALL.label);
    }

    #[test]
    fn hidden_when_the_graph_is_hidden() {
        let src = |snap| Sources {
            battery: &[],
            battery_since: None,
            network: snap,
            network_since: None,
            hardware: None,
        };
        let s = sample();
        assert!(NetworkInfo.visible(&src(Some(&s))));
        assert!(!NetworkInfo.visible(&src(None)));
        let none = Snapshot::default();
        assert!(!NetworkInfo.visible(&src(Some(&none))));
        assert_eq!(
            NetworkInfo.visible(&src(Some(&none))),
            super::super::network::Network.visible(&src(Some(&none)))
        );
        assert_eq!(NetworkInfo.svg(&src(None), shipped(TILE_H), Instant::now()), "");
    }
}
