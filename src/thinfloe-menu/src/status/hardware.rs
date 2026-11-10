//! The hardware elements (`Type=hardware`), one sampler and four tiles, chosen
//! by `Show=` in the file: `temperatures`, `cpu` (load and clock), `memory` (RAM
//! and zram) and `system` (fans, uptime and the machine's model). One
//! line per value, the label in Geo on the left, the value in Hack on the right;
//! a value the machine does not provide drops out, and a tile left with nothing
//! is not shown. Nothing pages: sensors that do not fit the temperature tile
//! drop the least important first. A hot temperature has its line in the warning
//! colour, a critical one in the alert colour. The data is a
//! `hardware::Snapshot` the menu copies from the sampler thread.

use super::style::{self, Anchor, Face, hex, mono_width};
use super::{Element, Kind, Room, Sources};
use crate::desktop::Entry;
use crate::hardware::{Heat, Snapshot, Temp, Usage};
use egui::Color32;
use std::cmp::Reverse;
use std::fmt::Write;
use std::time::{Duration, Instant};

pub const KIND: Kind = Kind {
    name: "hardware",
    options: &["Show"],
    build,
};

/// What a hardware tile shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    Temperatures,
    Cpu,
    Memory,
    System,
}

impl Show {
    pub const ALL: [Show; 4] = [Show::Temperatures, Show::Cpu, Show::Memory, Show::System];

    fn parse(text: &str) -> Option<Show> {
        match text {
            "temperatures" => Some(Show::Temperatures),
            "cpu" => Some(Show::Cpu),
            "memory" => Some(Show::Memory),
            "system" => Some(Show::System),
            _ => None,
        }
    }
}

fn build(entry: &Entry) -> Result<Box<dyn Element>, String> {
    let show = entry.get("Show").ok_or("no Show")?;
    let show =
        Show::parse(show).ok_or_else(|| format!("Show must be temperatures, cpu, memory or system, not '{show}'"))?;
    Ok(Box::new(Hardware { show }))
}

pub struct Hardware {
    pub show: Show,
}

/// One line: a label and, unless the label stands alone, a value.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub label: String,
    pub value: Option<String>,
    pub colour: Color32,
}

pub(super) fn line(label: impl Into<String>, value: impl Into<String>) -> Line {
    Line {
        label: label.into(),
        value: Some(value.into()),
        colour: style::foreground(),
    }
}

/// `3.1 / 7.7 GiB`, or in MiB when there is less than 1 GiB in all.
pub fn usage(u: Usage) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * MIB;
    let (used, total) = (u.used.min(u.total) as f64, u.total as f64);
    if total >= GIB {
        format!("{:.1} / {:.1} GiB", used / GIB, total / GIB)
    } else {
        format!("{:.0} / {:.0} MiB", used / MIB, total / MIB)
    }
}

/// `800 MHz`, `2.4 GHz`.
pub fn clock(mhz: u32) -> String {
    if mhz >= 1000 {
        format!("{:.1} GHz", f64::from(mhz) / 1000.0)
    } else {
        format!("{mhz} MHz")
    }
}

/// `12 m`, `4 h 12 m`, `3 d 4 h`: it changes once a minute at most.
pub fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} m")
    } else {
        format!("{m} m")
    }
}

fn heat_colour(heat: Heat) -> Color32 {
    match heat {
        Heat::Normal => style::foreground(),
        Heat::Hot => style::WARN,
        Heat::Critical => style::ALERT,
    }
}

/// The temperatures that fit `per` lines, in the sensor order with the CPU
/// first. The first CPU sensor always stays; the others are kept by heat, then
/// by that order, so a hot sensor is not the one dropped.
fn temperatures(snap: &Snapshot, per: usize) -> Vec<Line> {
    let is_cpu = |t: &Temp| t.label == "CPU" || t.label.starts_with("CPU ");
    let cpu = snap.temps.iter().filter(|t| is_cpu(t));
    let rest = snap.temps.iter().filter(|t| !is_cpu(t));
    let order: Vec<&Temp> = cpu.chain(rest).collect();
    let cpu_stays = order.first().is_some_and(|t| is_cpu(t));
    let heat = |t: &Temp| match t.heat {
        Heat::Normal => 0,
        Heat::Hot => 1,
        Heat::Critical => 2,
    };
    let mut kept: Vec<usize> = (0..order.len()).collect();
    kept.sort_by_key(|&i| (!(cpu_stays && i == 0), Reverse(heat(order[i])), i));
    kept.truncate(per);
    kept.sort_unstable();
    kept.into_iter()
        .map(|i| Line {
            label: order[i].label.clone(),
            value: Some(format!("{:.0} °C", order[i].celsius)),
            colour: heat_colour(order[i].heat),
        })
        .collect()
}

const MODEL: &str = "Model";
const MAKER: &str = "Maker";

/// The speeds and the unit, as one value.
fn fan_value(speeds: &[u32]) -> String {
    let list: Vec<String> = speeds.iter().map(u32::to_string).collect();
    format!("{} rpm", list.join(" "))
}

/// The label and value of the machine's line: its model, or its maker on a
/// machine that names no model. Both on one line did not fit the 220 points of
/// the tile: `Gigabyte Technology GB-BXi3-5010` was cut at the padding.
fn machine(snap: &Snapshot) -> Option<(&'static str, String)> {
    match (&snap.model, &snap.maker) {
        (Some(model), _) => Some((MODEL, model.clone())),
        (None, Some(maker)) => Some((MAKER, maker.clone())),
        (None, None) => None,
    }
}

/// The lines of a tile in a room of `per` lines; empty when the machine has
/// none of its values. Only the temperatures can be more than the three lines
/// that fit the smallest cell.
pub fn lines(snap: &Snapshot, show: Show, per: usize) -> Vec<Line> {
    match show {
        Show::Temperatures => temperatures(snap, per),
        Show::Cpu => {
            let load = snap.load.map(|l| line("CPU load", format!("{l} %")));
            let speed = snap.clock_mhz.map(|m| line("CPU clock", clock(m)));
            load.into_iter().chain(speed).collect()
        }
        Show::Memory => {
            let ram = snap.ram.map(|u| line("RAM", usage(u)));
            let zram = snap.zram.map(|u| line("zram", usage(u)));
            ram.into_iter().chain(zram).collect()
        }
        Show::System => {
            let fans = match snap.fans.len() {
                0 => None,
                1 => Some(line("Fan", fan_value(&snap.fans))),
                _ => Some(line("Fans", fan_value(&snap.fans))),
            };
            let up = snap.uptime.map(|s| line("Up", uptime(s)));
            let name = machine(snap).map(|(label, value)| line(label, value));
            fans.into_iter().chain(up).chain(name).collect()
        }
    }
}

/// The lines of a tile in `room`: `lines`, with the fans of the system tile cut
/// to as many as fit beside their label with the unit still showing, and a long
/// model cut at its end, because a value grows left from the padding and would
/// run over its label. The first fans stay; a single fan always stays, and a
/// long fan line never reaches the label.
pub fn lines_in(snap: &Snapshot, show: Show, per: usize, room: Room) -> Vec<Line> {
    if show != Show::System {
        return lines(snap, show, per);
    }
    let st = room.style();
    let free = room.size.x - 2.0 * st.pad_x - st.gap;
    let fits = |n: usize| {
        let value = fan_value(&snap.fans[..n]);
        let label = if n == 1 { "Fan" } else { "Fans" };
        style::label_width(label, st.title) + mono_width(&value, st.value) <= free
    };
    let n = (1..=snap.fans.len())
        .rev()
        .find(|&n| fits(n))
        .unwrap_or(snap.fans.len().min(1));
    let cut = Snapshot {
        fans: snap.fans[..n].to_vec(),
        ..snap.clone()
    };
    let mut out = lines(&cut, show, per);
    for l in out.iter_mut().filter(|l| l.label == MODEL || l.label == MAKER) {
        let room_left = free - style::label_width(&l.label, st.title);
        let chars = (room_left / mono_width("M", st.value)).max(0.0) as usize;
        if let Some(v) = &mut l.value {
            *v = v.chars().take(chars).collect();
        }
    }
    out
}

/// The SVG of `lines` in `room`: the same box and line placing as the
/// battery. A label is clipped where its value starts, or at the padding.
pub fn svg(lines: &[Line], room: Room) -> String {
    let st = room.style();
    let (w, h) = (room.size.x, room.size.y);
    let top = (h - lines.len() as f32 * st.row) / 2.0;
    let right = w - st.pad_x;
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\
         <rect width=\"{w}\" height=\"{h}\" fill=\"{}\"/>",
        hex(style::BACKGROUND)
    );
    for (i, l) in lines.iter().enumerate() {
        let y = style::baseline(top + i as f32 * st.row, st.row, st.title);
        let clip_right = match &l.value {
            Some(v) => right - mono_width(v, st.value) - st.gap,
            None => right,
        };
        let _ = write!(
            s,
            "<clipPath id=\"l{i}\"><rect width=\"{}\" height=\"{h}\"/></clipPath><g clip-path=\"url(#l{i})\">",
            clip_right.max(0.0)
        );
        style::text(
            &mut s,
            st.pad_x,
            y,
            Face::Label,
            st.title,
            l.colour,
            Anchor::Start,
            &l.label,
        );
        s.push_str("</g>");
        if let Some(v) = &l.value {
            style::text(&mut s, right, y, Face::Value, st.value, l.colour, Anchor::End, v);
        }
    }
    s.push_str("</svg>");
    s
}

impl Element for Hardware {
    fn visible(&self, data: &Sources) -> bool {
        data.hardware.is_some_and(|snap| !lines(snap, self.show, 1).is_empty())
    }

    fn svg(&self, data: &Sources, room: Room, _now: Instant) -> String {
        let Some(snap) = data.hardware else {
            return String::new();
        };
        let per = super::battery::per_page(room.size.y, &room.style());
        svg(&lines_in(snap, self.show, per, room), room)
    }

    /// Only a new reading changes the picture.
    fn next_change(&self, _data: &Sources, _room: Room, _now: Instant) -> Option<Duration> {
        None
    }
}

/// A machine as the owner might have one: for the tests and the pictures.
#[cfg(test)]
pub fn sample() -> Snapshot {
    let t = |label: &str, celsius, heat| Temp {
        label: label.into(),
        celsius,
        heat,
    };
    const GIB: u64 = 1024 * 1024 * 1024;
    Snapshot {
        temps: vec![
            t("CPU", 52.0, Heat::Normal),
            t("GPU", 97.0, Heat::Critical),
            t("Disk", 38.9, Heat::Normal),
            t("WiFi", 71.0, Heat::Hot),
            t("Board", 31.0, Heat::Normal),
        ],
        load: Some(12),
        clock_mhz: Some(2401),
        ram: Some(Usage {
            used: 3 * GIB + GIB / 8,
            total: 8 * GIB,
        }),
        zram: Some(Usage {
            used: GIB / 2,
            total: 2 * GIB,
        }),
        fans: vec![2400],
        uptime: Some(3 * 86_400 + 4 * 3600 + 120),
        maker: Some("Lenovo".into()),
        model: Some("ThinkCentre M75q Gen 2".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::Cells;
    use crate::status::render::{self, GEO_FAMILY, MONO_FAMILY};
    use crate::status::style::{LARGE, Style};
    use crate::ui::{TILE_GAP, TILE_H, TILE_H_MIN};
    use egui::vec2;

    fn room(h: f32) -> Room {
        Cells::parse("0,1,1,1").unwrap().room(vec2(220.0, h), TILE_GAP)
    }

    /// The room of a tile as shipped, at cells `h` high.
    fn room_of(show: Show, h: f32) -> Room {
        let cells = if show == Show::System { "2,2,1,1" } else { "0,1,1,1" };
        Cells::parse(cells).unwrap().room(vec2(220.0, h), TILE_GAP)
    }

    fn sources(snap: &Snapshot) -> Sources<'_> {
        Sources {
            battery: &[],
            battery_since: None,
            network: None,
            network_since: None,
            hardware: Some(snap),
        }
    }

    fn flat(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| match &l.value {
                Some(v) => format!("{} {v}", l.label),
                None => l.label.clone(),
            })
            .collect()
    }

    fn text(snap: &Snapshot, show: Show, per: usize) -> Vec<String> {
        flat(&lines(snap, show, per))
    }

    const FOUR: usize = 4;
    const THREE: usize = 3;

    #[test]
    fn a_cell_holds_four_lines_at_76_and_three_at_64() {
        let per = |h: f32| super::super::battery::per_page(h, &Style::for_cell(h));
        assert_eq!(per(TILE_H), FOUR);
        assert_eq!(per(TILE_H_MIN), THREE);
        assert_eq!(per(70.0), THREE);
    }

    #[test]
    fn each_tile_shows_its_own_values() {
        let s = sample();
        assert_eq!(
            text(&s, Show::Temperatures, FOUR),
            ["CPU 52 °C", "GPU 97 °C", "Disk 39 °C", "WiFi 71 °C"]
        );
        assert_eq!(text(&s, Show::Cpu, FOUR), ["CPU load 12 %", "CPU clock 2.4 GHz"]);
        assert_eq!(
            text(&s, Show::Memory, FOUR),
            ["RAM 3.1 / 8.0 GiB", "zram 0.5 / 2.0 GiB"]
        );
        assert_eq!(
            text(&s, Show::System, FOUR),
            ["Fan 2400 rpm", "Up 3 d 4 h", "Model ThinkCentre M75q Gen 2"]
        );
    }

    #[test]
    fn sensors_that_do_not_fit_drop_the_least_important_and_the_cpu_stays() {
        let t = |label: &str, celsius, heat| Temp {
            label: label.into(),
            celsius,
            heat,
        };
        let calm = Snapshot {
            temps: ["CPU", "GPU", "Disk", "RAM", "WiFi", "Board"]
                .iter()
                .map(|n| t(n, 40.0, Heat::Normal))
                .collect(),
            ..Snapshot::default()
        };
        // All normal: the sensor order decides.
        assert_eq!(
            text(&calm, Show::Temperatures, FOUR),
            ["CPU 40 °C", "GPU 40 °C", "Disk 40 °C", "RAM 40 °C"]
        );
        assert_eq!(
            text(&calm, Show::Temperatures, THREE),
            ["CPU 40 °C", "GPU 40 °C", "Disk 40 °C"]
        );
        assert_eq!(text(&calm, Show::Temperatures, 1), ["CPU 40 °C"]);
        // A hot sensor is not the one dropped, and keeps its place in the order.
        let hot = Snapshot {
            temps: vec![
                t("CPU", 40.0, Heat::Normal),
                t("GPU", 40.0, Heat::Normal),
                t("Disk", 40.0, Heat::Normal),
                t("WiFi", 81.0, Heat::Hot),
                t("Board", 99.0, Heat::Critical),
            ],
            ..Snapshot::default()
        };
        assert_eq!(
            text(&hot, Show::Temperatures, THREE),
            ["CPU 40 °C", "WiFi 81 °C", "Board 99 °C"]
        );
        assert_eq!(
            text(&hot, Show::Temperatures, FOUR),
            ["CPU 40 °C", "GPU 40 °C", "WiFi 81 °C", "Board 99 °C"]
        );
        // The CPU stays even when all the others are hot, and with one line.
        assert_eq!(text(&hot, Show::Temperatures, 1), ["CPU 40 °C"]);
        // A CPU sensor found later in the list still comes first.
        let late = Snapshot {
            temps: vec![t("Disk", 40.0, Heat::Normal), t("CPU", 40.0, Heat::Normal)],
            ..Snapshot::default()
        };
        assert_eq!(text(&late, Show::Temperatures, FOUR), ["CPU 40 °C", "Disk 40 °C"]);
        // No CPU sensor: the hottest wins the only line.
        let none = Snapshot {
            temps: vec![t("Disk", 40.0, Heat::Normal), t("WiFi", 81.0, Heat::Hot)],
            ..Snapshot::default()
        };
        assert_eq!(text(&none, Show::Temperatures, 1), ["WiFi 81 °C"]);
    }

    #[test]
    fn what_a_machine_lacks_drops_out_and_a_tile_with_nothing_shows_nothing() {
        // A VM: a clock, memory, uptime and a name; no sensor at all.
        let vm = Snapshot {
            temps: vec![],
            load: None,
            zram: None,
            fans: vec![],
            ..sample()
        };
        assert!(lines(&vm, Show::Temperatures, FOUR).is_empty());
        assert_eq!(text(&vm, Show::Cpu, FOUR), ["CPU clock 2.4 GHz"]);
        assert_eq!(text(&vm, Show::Memory, FOUR), ["RAM 3.1 / 8.0 GiB"]);
        assert_eq!(
            text(&vm, Show::System, FOUR),
            ["Up 3 d 4 h", "Model ThinkCentre M75q Gen 2"]
        );
        let bare = Snapshot {
            uptime: Some(59),
            ..Snapshot::default()
        };
        assert_eq!(text(&bare, Show::System, FOUR), ["Up 0 m"]);
        let tile = |show| Hardware { show };
        for show in [Show::Temperatures, Show::Cpu, Show::Memory] {
            assert!(!tile(show).visible(&sources(&bare)), "{show:?}");
        }
        assert!(tile(Show::System).visible(&sources(&bare)));
        // No placeholder text anywhere.
        let s = tile(Show::Cpu).svg(&sources(&vm), room(TILE_H), Instant::now());
        for gone in ["load", "°C", "–", "n/a", "zram", "Fan"] {
            assert!(!s.contains(gone), "{gone} in {s}");
        }
    }

    #[test]
    fn the_system_tile_has_one_line_for_the_fans_and_one_for_the_machine() {
        let many = Snapshot {
            fans: vec![12000, 1800, 900, 600],
            maker: Some("Gigabyte Technology".into()),
            model: Some("GB-BXi3-5010 Some Long Board Name".into()),
            ..sample()
        };
        assert_eq!(
            text(&many, Show::System, THREE),
            [
                "Fans 12000 1800 900 600 rpm",
                "Up 3 d 4 h",
                "Model GB-BXi3-5010 Some Long Board Name"
            ]
        );
        let same = Snapshot {
            fans: vec![],
            uptime: None,
            maker: Some("QEMU".into()),
            model: Some("QEMU".into()),
            ..Snapshot::default()
        };
        assert_eq!(text(&same, Show::System, THREE), ["Model QEMU"]);
        let only = Snapshot {
            model: Some("X1".into()),
            ..Snapshot::default()
        };
        assert_eq!(text(&only, Show::System, THREE), ["Model X1"]);
        let only_maker = Snapshot {
            maker: Some("Acme".into()),
            ..Snapshot::default()
        };
        assert_eq!(text(&only_maker, Show::System, THREE), ["Maker Acme"]);
    }

    #[test]
    fn hot_and_critical_lines_use_the_warning_colours() {
        let l = lines(&sample(), Show::Temperatures, FOUR);
        let colour = |name: &str| l.iter().find(|l| l.label == name).unwrap().colour;
        assert_eq!(colour("CPU"), style::foreground());
        assert_eq!(colour("WiFi"), style::WARN);
        assert_eq!(colour("GPU"), style::ALERT);
        let s = svg(&l, room(TILE_H));
        assert!(s.contains(&hex(style::ALERT)) && s.contains(&hex(style::WARN)), "{s}");
    }

    #[test]
    fn values_read_short() {
        const GIB: u64 = 1024 * 1024 * 1024;
        assert_eq!(usage(Usage { used: 0, total: GIB }), "0.0 / 1.0 GiB");
        assert_eq!(
            usage(Usage {
                used: 512 * 1024 * 1024,
                total: 900 * 1024 * 1024
            }),
            "512 / 900 MiB"
        );
        assert_eq!(
            usage(Usage {
                used: 9 * GIB,
                total: 8 * GIB
            }),
            "8.0 / 8.0 GiB",
            "never over the total"
        );
        assert_eq!(clock(800), "800 MHz");
        assert_eq!(clock(2401), "2.4 GHz");
        assert_eq!(uptime(0), "0 m");
        assert_eq!(uptime(61), "1 m");
        assert_eq!(uptime(3661), "1 h 1 m");
        assert_eq!(uptime(90_000), "1 d 1 h");
    }

    #[test]
    fn nothing_turns_a_page() {
        let t0 = Instant::now();
        let snap = sample();
        let d = sources(&snap);
        for show in Show::ALL {
            let e = Hardware { show };
            let r = room_of(show, TILE_H);
            assert_eq!(e.next_change(&d, r, t0), None, "{show:?}");
            let later = t0 + Duration::from_secs(600);
            assert_eq!(e.svg(&d, r, t0), e.svg(&d, r, later), "{show:?}");
        }
    }

    #[test]
    fn shown_only_with_something_to_show() {
        let now = Instant::now();
        let none = Sources {
            battery: &[],
            battery_since: None,
            network: None,
            network_since: None,
            hardware: None,
        };
        for show in Show::ALL {
            let e = Hardware { show };
            assert!(e.visible(&sources(&sample())), "{show:?}");
            assert!(!e.visible(&sources(&Snapshot::default())), "{show:?}");
            assert!(!e.visible(&none), "no sampler: nothing");
            assert_eq!(e.svg(&none, room(TILE_H), now), "");
        }
    }

    #[test]
    fn show_comes_from_the_file() {
        let b = |text: &str| {
            let e = crate::desktop::parse_group(&format!("[G]\n{text}\n"), "[G]").unwrap();
            build(&e).map(|_| ())
        };
        for good in ["temperatures", "cpu", "memory", "system"] {
            assert_eq!(b(&format!("Show={good}")), Ok(()));
        }
        assert_eq!(b("Other=1"), Err("no Show".to_string()));
        assert_eq!(
            b("Show=fans"),
            Err("Show must be temperatures, cpu, memory or system, not 'fans'".to_string())
        );
    }

    #[test]
    fn names_from_the_system_are_escaped_and_a_control_character_blanks_nothing() {
        let odd = Snapshot {
            temps: vec![Temp {
                label: "<b>&\"'".into(),
                celsius: 40.0,
                heat: Heat::Normal,
            }],
            model: Some("A&B\x01<x>".into()),
            ..Snapshot::default()
        };
        let s = svg(&lines(&odd, Show::Temperatures, FOUR), room(TILE_H));
        assert!(s.contains("&lt;b&gt;&amp;&quot;&apos;"), "{s}");
        render::render(&s, [220, 76]).expect("renders");
        let s = svg(&lines(&odd, Show::System, FOUR), room(TILE_H));
        assert!(s.contains("A&amp;B?&lt;x&gt;"), "{s}");
        render::render(&s, [220, 76]).expect("renders");
    }

    /// The ink of every text, with the real fonts. Clips are not taken into
    /// account: a clipped label counts at its full width.
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

    /// Every ink box lies inside the room and none touches another.
    fn assert_clean(b: &[(String, resvg::usvg::Rect)], r: Room, what: &str) {
        for (i, (ta, a)) in b.iter().enumerate() {
            assert!(
                a.left() >= 0.0 && a.top() >= 0.0 && a.right() <= r.size.x && a.bottom() <= r.size.y,
                "{what}: {ta} {a:?}"
            );
            for (tb, c) in &b[i + 1..] {
                let hit = a.left() < c.right() && c.left() < a.right() && a.top() < c.bottom() && c.top() < a.bottom();
                assert!(!hit, "{what}: {ta} {a:?} overlaps {tb} {c:?}");
            }
        }
    }

    #[test]
    fn no_text_overlaps_another_or_leaves_the_element_with_the_real_fonts() {
        let busy = Snapshot {
            fans: vec![12000, 1800, 900, 600],
            ..sample()
        };
        for snap in [sample(), busy] {
            for show in Show::ALL {
                for h in [TILE_H, TILE_H_MIN, 70.0] {
                    let r = room_of(show, h);
                    let e = Hardware { show };
                    let s = e.svg(&sources(&snap), r, Instant::now());
                    let b = ink(&s);
                    assert!(!b.is_empty(), "{show:?} at {h}");
                    assert_clean(&b, r, &format!("{show:?} at {h}"));
                }
            }
        }
    }

    #[test]
    fn the_system_tile_is_one_cell_wide_and_keeps_the_fans_that_fit_with_their_unit() {
        let mut kept = Vec::new();
        for n in 1..=6usize {
            let snap = Snapshot {
                fans: [12000, 1800, 900, 600, 450, 300][..n].to_vec(),
                ..sample()
            };
            for h in [TILE_H, TILE_H_MIN] {
                let r = room_of(Show::System, h);
                assert_eq!(r.size.x, 220.0, "one cell wide");
                let s = Hardware { show: Show::System }.svg(&sources(&snap), r, Instant::now());
                let b = ink(&s);
                assert_clean(&b, r, &format!("{n} fans at {h}"));
                let fans = b
                    .iter()
                    .map(|(t, _)| t.as_str())
                    .find(|t| *t == "rpm" || t.ends_with("rpm"));
                let right = b.iter().map(|(_, a)| a.right()).fold(0.0, f32::max);
                assert!(
                    right <= r.size.x - room_of(Show::System, h).style().pad_x + 0.5,
                    "{n} fans at {h}: {right}"
                );
                assert!(fans.is_some(), "the unit stays: {n} fans at {h}: {s}");
                let shown = s.matches("rpm").count();
                assert_eq!(shown, 1, "{n} fans at {h}");
                if h == TILE_H {
                    kept.push(
                        b.iter()
                            .filter(|(t, _)| t.ends_with("rpm"))
                            .map(|(t, _)| t.split(' ').count() - 1)
                            .sum::<usize>(),
                    );
                }
            }
        }
        // One fan in full, never fewer than one, never more than were there, and
        // the count only grows with the fans.
        assert_eq!(kept[0], 1);
        assert!(kept.windows(2).all(|w| w[0] <= w[1] && w[1] <= w[0] + 1), "{kept:?}");
        assert!(*kept.last().unwrap() >= 2, "two short speeds fit one cell: {kept:?}");
    }

    #[test]
    fn the_first_fans_stay_and_one_more_would_not_fit() {
        let snap = Snapshot {
            fans: vec![1111, 2222, 3333, 4444, 5555, 6666, 7777, 8888],
            ..sample()
        };
        let r = room_of(Show::System, TILE_H);
        let st = r.style();
        let free = r.size.x - 2.0 * st.pad_x - st.gap;
        let width = |n: usize| style::label_width("Fans", st.title) + mono_width(&fan_value(&snap.fans[..n]), st.value);
        let got = lines_in(&snap, Show::System, 3, r);
        let value = got
            .iter()
            .find_map(|l| l.value.clone().filter(|v| v.ends_with("rpm")))
            .expect("a fan line");
        let n = value.split(' ').count() - 1;
        assert!(n >= 2 && n < snap.fans.len(), "this test needs a cut: {value}");
        assert_eq!(value, fan_value(&snap.fans[..n]), "the first fans, in order");
        assert!(width(n) <= free, "{n} fans do not fit");
        assert!(width(n + 1) > free, "{} fans would have fit too", n + 1);
    }

    #[test]
    fn a_model_has_its_label_on_the_left_and_a_long_one_stops_short_of_it() {
        let short = Snapshot {
            model: Some("NUC8i3BEK".into()),
            ..Snapshot::default()
        };
        let long = Snapshot {
            model: Some("GB-BXi3-5010 Some Long Board Name".into()),
            ..sample()
        };
        for h in [TILE_H, TILE_H_MIN] {
            let r = room_of(Show::System, h);
            let st = r.style();
            let got = lines_in(&short, Show::System, 3, r);
            assert_eq!(got[0].label, "Model");
            assert_eq!(got[0].value.as_deref(), Some("NUC8i3BEK"));
            let got = lines_in(&long, Show::System, 3, r);
            let model = got.iter().find(|l| l.label == "Model").unwrap();
            let v = model.value.as_deref().unwrap();
            assert!(v.starts_with("GB-BXi3") && v.len() < 33, "{v}");
            let free = r.size.x - 2.0 * st.pad_x - st.gap;
            assert!(
                style::label_width("Model", st.title) + mono_width(v, st.value) <= free,
                "{v} at {h}"
            );
            let s = Hardware { show: Show::System }.svg(&sources(&long), r, Instant::now());
            assert_clean(&ink(&s), r, &format!("a long model at {h}"));
        }
    }

    #[test]
    fn a_long_machine_name_is_cut_at_the_padding_in_the_one_cell_system_tile() {
        let snap = Snapshot {
            maker: Some("Gigabyte Technology".into()),
            model: Some("GB-BXi3-5010 Some Long Board Name".into()),
            ..sample()
        };
        for h in [TILE_H, TILE_H_MIN] {
            let r = room_of(Show::System, h);
            let st = r.style();
            let s = Hardware { show: Show::System }.svg(&sources(&snap), r, Instant::now());
            let img = render::render(&s, [r.size.x as usize, r.size.y as usize]).unwrap();
            let w = r.size.x as usize;
            let from = (w as f32 - st.pad_x).ceil() as usize + 1;
            let last = img.pixels.len() / w;
            let blank = (from..w).all(|x| (0..last).all(|y| img.pixels[y * w + x] == style::BACKGROUND));
            assert!(blank, "ink right of the padding at {h}");
            let inked = (0..w).any(|x| (0..last).any(|y| img.pixels[y * w + x] != style::BACKGROUND));
            assert!(inked);
        }
    }

    #[test]
    fn a_long_sensor_name_is_cut_before_its_value() {
        let r = room(TILE_H);
        let long = vec![Line {
            label: "W".repeat(40),
            value: Some("100 °C".into()),
            colour: style::foreground(),
        }];
        let s = svg(&long, r);
        let img = render::render(&s, [220, 76]).unwrap();
        let gap_from = 220.0 - LARGE.pad_x - mono_width("100 °C", LARGE.value) - LARGE.gap;
        // Blank in the gap between the cut name and the value.
        let blank = (gap_from.ceil() as usize + 1..(gap_from + LARGE.gap).floor() as usize)
            .all(|x| (0..76).all(|y| img.pixels[y * 220 + x] == style::BACKGROUND));
        assert!(blank, "{s}");
    }

    #[test]
    fn labels_in_geo_values_in_hack_and_the_degree_sign_has_a_glyph() {
        let r = room(TILE_H);
        let s = svg(&lines(&sample(), Show::Temperatures, FOUR), r);
        let doc = resvg::usvg::roxmltree::Document::parse(&s).unwrap();
        for n in doc.descendants().filter(|n| n.has_tag_name("text")) {
            let want = if n.attribute("text-anchor") == Some("end") {
                MONO_FAMILY
            } else {
                GEO_FAMILY
            };
            assert_eq!(n.attribute("font-family"), Some(want), "{:?}", n.text());
        }
        let glyph = |c: char| {
            let svg = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\">\
                 <text x=\"2\" y=\"15\" font-family=\"{MONO_FAMILY}\" font-size=\"14\" fill=\"#fff\">{c}</text></svg>"
            );
            render::render(&svg, [20, 20]).unwrap().pixels
        };
        assert_ne!(glyph('°'), glyph('\u{e000}'));
    }
}
