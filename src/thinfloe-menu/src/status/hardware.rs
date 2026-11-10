//! The hardware elements, one sampler and four elements: `temperatures`,
//! `cpu` (load and clock), `memory` (RAM and zram) and `system` (fans, uptime
//! and the machine's model). One line per value, the label in Geo on the
//! left, the value in the monospace font on the right; a value the machine
//! does not provide drops out, and an element left with nothing is not shown.
//! Nothing pages: sensors that do not fit the temperature element drop the
//! least important first. A hot temperature has its line in the warning
//! colour, a critical one in the alert colour. The data is a
//! `hardware::Snapshot` the menu copies from the sampler thread.

use super::battery::{cut, per_page};
use super::{Data, Element, Size, Source, background, cell_height, sizes_for};
use crate::brand::role;
use crate::hardware::{self, Heat, Snapshot, Temp, Usage};
use egui::{Color32, FontId, Rect, pos2};
use shadcn_egui::Theme;
use std::cmp::Reverse;
use std::time::{Duration, Instant};

/// What an element shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    Temperatures,
    Cpu,
    Memory,
    System,
}

impl Show {
    pub const ALL: [Show; 4] = [Show::Temperatures, Show::Cpu, Show::Memory, Show::System];
}

pub struct Temperatures;
pub struct Cpu;
pub struct Memory;
pub struct System;

/// A colour by what it says; the theme gives the colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Warn,
    Alert,
}

/// One line: a label and its value.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub label: String,
    pub value: String,
    pub tone: Tone,
}

/// Names come from the machine: a control character would break a line.
fn clean(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { '?' } else { c }).collect()
}

fn line(label: impl AsRef<str>, value: impl AsRef<str>) -> Line {
    Line {
        label: clean(label.as_ref()),
        value: clean(value.as_ref()),
        tone: Tone::Normal,
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

fn heat_tone(heat: Heat) -> Tone {
    match heat {
        Heat::Normal => Tone::Normal,
        Heat::Hot => Tone::Warn,
        Heat::Critical => Tone::Alert,
    }
}

fn colour(t: &Theme, tone: Tone) -> Color32 {
    match tone {
        Tone::Normal => t.palette().foreground,
        Tone::Warn => t.role(role::WARN),
        Tone::Alert => t.role(role::ALERT),
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
            tone: heat_tone(order[i].heat),
            ..line(&order[i].label, format!("{:.0} °C", order[i].celsius))
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
/// a cell: `Gigabyte Technology GB-BXi3-5010` was cut at the padding.
fn machine(snap: &Snapshot) -> Option<(&'static str, String)> {
    match (&snap.model, &snap.maker) {
        (Some(model), _) => Some((MODEL, model.clone())),
        (None, Some(maker)) => Some((MAKER, maker.clone())),
        (None, None) => None,
    }
}

/// The lines of an element in a room of `per` lines; empty when the machine
/// has none of its values. Only the temperatures can be more than the three
/// lines that fit the smallest cell.
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

/// How wide text is: a label in Geo and a value in the monospace font.
pub struct Measure<'a> {
    pub label: &'a dyn Fn(&str) -> f32,
    pub value: &'a dyn Fn(&str) -> f32,
}

/// The lines of an element in `per` lines with `free` points between the
/// paddings, less the gap between label and value: `lines`, with the fans of
/// the system element cut to as many as fit beside their label with the unit
/// still showing, and a long model cut at its end, because a value is
/// right-aligned and would run over its label. The first fans stay; a single
/// fan always stays.
pub fn lines_in(snap: &Snapshot, show: Show, per: usize, free: f32, w: &Measure) -> Vec<Line> {
    if show != Show::System {
        return lines(snap, show, per);
    }
    let fits = |n: usize| {
        let label = if n == 1 { "Fan" } else { "Fans" };
        (w.label)(label) + (w.value)(&fan_value(&snap.fans[..n])) <= free
    };
    let n = (1..=snap.fans.len())
        .rev()
        .find(|&n| fits(n))
        .unwrap_or(snap.fans.len().min(1));
    let cut_fans = Snapshot {
        fans: snap.fans[..n].to_vec(),
        ..snap.clone()
    };
    let mut out = lines(&cut_fans, show, per);
    let em = (w.value)("M").max(0.01);
    for l in out.iter_mut().filter(|l| l.label == MODEL || l.label == MAKER) {
        let chars = ((free - (w.label)(&l.label)) / em).max(0.0) as usize;
        l.value = l.value.chars().take(chars).collect();
    }
    out
}

fn show_in(snap: &Snapshot, show: Show) -> bool {
    !lines(snap, show, 1).is_empty()
}

/// Draw the lines of `show` in `rect` of `size` cells, centred: the label
/// at the left padding, cut where its value starts, the value right-aligned
/// at the right padding.
fn paint(ui: &egui::Ui, rect: Rect, size: Size, snap: &Snapshot, show: Show) {
    let t = Theme::current(ui.ctx());
    let p = ui.painter();
    let m = sizes_for(cell_height(rect, size));
    let label_font = crate::brand::geo(m.label);
    let value_font = FontId::monospace(m.value);
    let width = |text: &str, font: &FontId| {
        p.layout_no_wrap(text.to_string(), font.clone(), Color32::TRANSPARENT)
            .size()
            .x
    };
    let (label_w, value_w) = (|s: &str| width(s, &label_font), |s: &str| width(s, &value_font));
    let measure = Measure {
        label: &label_w,
        value: &value_w,
    };
    let (left, right) = (rect.left() + m.pad_x, rect.right() - m.pad_x);
    let lines = lines_in(snap, show, per_page(rect.height(), &m), right - left - m.gap, &measure);
    let top = rect.top() + (rect.height() - lines.len() as f32 * m.row) / 2.0;
    for (i, l) in lines.iter().enumerate() {
        let mid = top + (i as f32 + 0.5) * m.row;
        let c = colour(&t, l.tone);
        let v = cut(p, &l.value, value_font.clone(), c, right - left);
        let x = right - v.size().x;
        let label = cut(p, &l.label, label_font.clone(), c, x - m.gap - left);
        if v.size().x <= right - left + 0.01 {
            p.galley(pos2(x, mid - v.size().y / 2.0), v, c);
        }
        p.galley(pos2(left, mid - label.size().y / 2.0), label, c);
    }
}

/// The part of `Element` the four share.
fn draw(ui: &mut egui::Ui, rect: Rect, size: Size, data: &Data, show: Show) {
    background(ui, rect);
    if let Some(snap) = data.hardware {
        paint(ui, rect, size, snap, show);
    }
}

macro_rules! element {
    ($ty:ty, $show:expr, $id:literal, $name:literal, $sizes:expr) => {
        impl Element for $ty {
            fn id(&self) -> &'static str {
                $id
            }

            fn name(&self) -> &'static str {
                $name
            }

            fn sizes(&self) -> &'static [Size] {
                $sizes
            }

            fn short_size(&self) -> Option<Size> {
                None
            }

            fn sources(&self) -> &'static [Source] {
                &[Source::Hardware]
            }

            fn has_content(&self, data: &Data) -> bool {
                data.hardware.is_some_and(|snap| show_in(snap, $show))
            }

            fn draw(&self, ui: &mut egui::Ui, rect: Rect, size: Size, data: &Data, _now: Instant) {
                draw(ui, rect, size, data, $show);
            }

            /// Only a new reading changes the picture.
            fn next_change(&self, _data: &Data, _rect: Rect, _now: Instant) -> Option<Duration> {
                None
            }

            fn refresh(&self) -> Option<Duration> {
                Some(hardware::EVERY)
            }
        }
    };
}

const ONE: Size = Size { w: 1, h: 1 };
const TALL: Size = Size { w: 1, h: 2 };
const WIDE: Size = Size { w: 2, h: 1 };

element!(
    Temperatures,
    Show::Temperatures,
    "temperatures",
    "Temperatures",
    &[ONE, TALL]
);
element!(Cpu, Show::Cpu, "cpu", "CPU", &[ONE]);
element!(Memory, Show::Memory, "memory", "Memory", &[ONE]);
element!(System, Show::System, "system", "Fans, uptime, model", &[ONE, WIDE]);

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
    use super::super::testing::{self, CELL_HEIGHTS, flatten, rect_of, shapes, text_boxes, texts};
    use super::*;
    use crate::brand::STATUS_CELL_LARGE_H;
    use egui::{FontFamily, Shape};

    const FOUR: usize = 4;
    const THREE: usize = 3;

    fn data(snap: &Snapshot) -> Data<'_> {
        Data {
            hardware: Some(snap),
            ..Data::EMPTY
        }
    }

    fn element_of(show: Show) -> &'static dyn Element {
        match show {
            Show::Temperatures => &Temperatures,
            Show::Cpu => &Cpu,
            Show::Memory => &Memory,
            Show::System => &System,
        }
    }

    fn flat(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| format!("{} {}", l.label, l.value)).collect()
    }

    fn text(snap: &Snapshot, show: Show, per: usize) -> Vec<String> {
        flat(&lines(snap, show, per))
    }

    fn theme() -> Theme {
        crate::brand::theme()
    }

    /// Twelve sensors, eight fans and a model of 60 characters.
    fn crowded() -> Snapshot {
        Snapshot {
            temps: (0..12)
                .map(|i| Temp {
                    label: if i == 0 {
                        "CPU".into()
                    } else {
                        format!("Sensor number {i}")
                    },
                    celsius: 40.0 + i as f32 * 5.0,
                    heat: [Heat::Normal, Heat::Hot, Heat::Critical][i % 3],
                })
                .collect(),
            fans: vec![11111, 2222, 3333, 4444, 5555, 6666, 7777, 8888],
            model: Some("M".repeat(60)),
            ..sample()
        }
    }

    /// Text widths of the real fonts at the sizes of a cell `h` high.
    fn widths(h: f32, f: impl FnOnce(&Measure) -> Vec<Line>) -> Vec<Line> {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let m = sizes_for(h);
        let mut out = Vec::new();
        let mut f = Some(f);
        for _ in 0..3 {
            let mut o = ctx.run_ui(egui::RawInput::default(), |ui| {
                let p = ui.painter();
                let w = |s: &str, font: FontId| p.layout_no_wrap(s.to_string(), font, Color32::TRANSPARENT).size().x;
                let (l, v) = (
                    |s: &str| w(s, crate::brand::geo(m.label)),
                    |s: &str| w(s, FontId::monospace(m.value)),
                );
                if let Some(f) = f.take() {
                    out = f(&Measure { label: &l, value: &v });
                }
            });
            o.textures_delta.clear();
        }
        out
    }

    #[test]
    fn temperatures_draws_inside_its_rect_at_every_size() {
        for snap in [sample(), crowded(), Snapshot::default()] {
            testing::assert_inside(&Temperatures, &data(&snap), Instant::now());
        }
    }

    #[test]
    fn cpu_draws_inside_its_rect_at_every_size() {
        for snap in [sample(), crowded()] {
            testing::assert_inside(&Cpu, &data(&snap), Instant::now());
        }
    }

    #[test]
    fn memory_draws_inside_its_rect_at_every_size() {
        for snap in [sample(), crowded()] {
            testing::assert_inside(&Memory, &data(&snap), Instant::now());
        }
    }

    #[test]
    fn system_draws_inside_its_rect_at_every_size() {
        let hostile = Snapshot {
            model: Some("A&B\x01<x>\n\u{202e}".repeat(5)),
            ..crowded()
        };
        for snap in [sample(), crowded(), hostile] {
            testing::assert_inside(&System, &data(&snap), Instant::now());
        }
    }

    #[test]
    fn the_elements_are_what_the_spec_says() {
        let size = |w, h| Size { w, h };
        let want: [(Show, &str, &str, &[Size]); 4] = [
            (
                Show::Temperatures,
                "temperatures",
                "Temperatures",
                &[size(1, 1), size(1, 2)],
            ),
            (Show::Cpu, "cpu", "CPU", &[size(1, 1)]),
            (Show::Memory, "memory", "Memory", &[size(1, 1)]),
            (Show::System, "system", "Fans, uptime, model", &[size(1, 1), size(2, 1)]),
        ];
        for (show, id, name, sizes) in want {
            let e = element_of(show);
            assert_eq!((e.id(), e.name(), e.sizes()), (id, name, sizes));
            assert_eq!(e.short_size(), None, "{id}");
            assert_eq!(e.sources(), &[Source::Hardware], "{id}");
            assert_eq!(e.refresh(), Some(Duration::from_secs(5)), "{id}");
            assert!(crate::status::ELEMENTS.iter().any(|x| x.id() == id), "{id} is listed");
        }
    }

    #[test]
    fn each_element_shows_its_own_values() {
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
        assert_eq!(
            text(&calm, Show::Temperatures, FOUR),
            ["CPU 40 °C", "GPU 40 °C", "Disk 40 °C", "RAM 40 °C"]
        );
        assert_eq!(
            text(&calm, Show::Temperatures, THREE),
            ["CPU 40 °C", "GPU 40 °C", "Disk 40 °C"]
        );
        assert_eq!(text(&calm, Show::Temperatures, 1), ["CPU 40 °C"]);
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
        assert_eq!(text(&hot, Show::Temperatures, 1), ["CPU 40 °C"]);
        let late = Snapshot {
            temps: vec![t("Disk", 40.0, Heat::Normal), t("CPU", 40.0, Heat::Normal)],
            ..Snapshot::default()
        };
        assert_eq!(text(&late, Show::Temperatures, FOUR), ["CPU 40 °C", "Disk 40 °C"]);
        let none = Snapshot {
            temps: vec![t("Disk", 40.0, Heat::Normal), t("WiFi", 81.0, Heat::Hot)],
            ..Snapshot::default()
        };
        assert_eq!(text(&none, Show::Temperatures, 1), ["WiFi 81 °C"]);
    }

    #[test]
    fn the_temperature_element_drops_sensors_it_has_no_line_for() {
        // 12 sensors in one cell: the lines drawn are the kept ones, CPU first.
        let snap = crowded();
        let size = Size { w: 1, h: 1 };
        for h in CELL_HEIGHTS {
            let rect = rect_of(size, h);
            let drawn = text_boxes(&shapes(&Temperatures, rect, size, &data(&snap), Instant::now()));
            let labels: Vec<&str> = drawn
                .iter()
                .map(|(t, _, _)| t.as_str())
                .filter(|t| t.starts_with("CPU") || t.starts_with("Sensor"))
                .collect();
            assert!(!labels.is_empty() && labels.len() < 12, "{h}: {labels:?}");
            assert_eq!(labels[0], "CPU", "{h}");
        }
    }

    #[test]
    fn what_a_machine_lacks_drops_out_and_an_element_with_nothing_shows_nothing() {
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
        for show in [Show::Temperatures, Show::Cpu, Show::Memory] {
            assert!(!element_of(show).has_content(&data(&bare)), "{show:?}");
        }
        assert!(element_of(Show::System).has_content(&data(&bare)));
        // No placeholder text anywhere.
        let size = Size { w: 1, h: 1 };
        let drawn = texts(&shapes(
            &Cpu,
            rect_of(size, STATUS_CELL_LARGE_H),
            size,
            &data(&vm),
            Instant::now(),
        ));
        for (t, _) in &drawn {
            for gone in ["load", "°C", "–", "n/a", "zram", "Fan"] {
                assert!(!t.contains(gone), "{gone} in {t}");
            }
        }
    }

    #[test]
    fn has_content_needs_a_sampler_and_a_value() {
        for show in Show::ALL {
            let e = element_of(show);
            assert!(e.has_content(&data(&sample())), "{show:?}");
            assert!(!e.has_content(&data(&Snapshot::default())), "{show:?}");
            assert!(!e.has_content(&Data::EMPTY), "no sampler: nothing");
        }
    }

    #[test]
    fn the_system_element_has_one_line_for_the_fans_and_one_for_the_machine() {
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
            maker: Some("QEMU".into()),
            model: Some("QEMU".into()),
            ..Snapshot::default()
        };
        assert_eq!(text(&same, Show::System, THREE), ["Model QEMU"]);
        let only_maker = Snapshot {
            maker: Some("Acme".into()),
            ..Snapshot::default()
        };
        assert_eq!(text(&only_maker, Show::System, THREE), ["Maker Acme"]);
    }

    #[test]
    fn hot_and_critical_lines_use_the_warning_colours() {
        let t = theme();
        let (fg, warn, alert) = (t.palette().foreground, t.role(role::WARN), t.role(role::ALERT));
        assert_ne!(warn, alert);
        let size = Size { w: 1, h: 2 };
        let drawn = text_boxes(&shapes(
            &Temperatures,
            rect_of(size, STATUS_CELL_LARGE_H),
            size,
            &data(&sample()),
            Instant::now(),
        ));
        let colour = |name: &str| {
            drawn
                .iter()
                .find(|(s, _, _)| s == name)
                .unwrap_or_else(|| panic!("{name}"))
                .2
        };
        assert_eq!(colour("CPU"), fg);
        assert_eq!(colour("52 °C"), fg);
        assert_eq!((colour("WiFi"), colour("71 °C")), (warn, warn));
        assert_eq!((colour("GPU"), colour("97 °C")), (alert, alert));
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
        let later = t0 + Duration::from_secs(600);
        for show in Show::ALL {
            let e = element_of(show);
            let size = e.sizes()[0];
            let r = rect_of(size, STATUS_CELL_LARGE_H);
            assert_eq!(e.next_change(&data(&snap), r, t0), None, "{show:?}");
            let a = texts(&shapes(e, r, size, &data(&snap), t0));
            let b = texts(&shapes(e, r, size, &data(&snap), later));
            assert_eq!(a, b, "{show:?}");
        }
    }

    #[test]
    fn the_fans_that_fit_stay_with_their_unit_and_never_reach_the_label() {
        let mut kept = Vec::new();
        for n in 1..=6usize {
            let snap = Snapshot {
                fans: [12000, 1800, 900, 600, 450, 300][..n].to_vec(),
                ..sample()
            };
            for h in CELL_HEIGHTS {
                let size = Size { w: 1, h: 1 };
                let rect = rect_of(size, h);
                let m = sizes_for(h);
                let boxes = text_boxes(&shapes(&System, rect, size, &data(&snap), Instant::now()));
                let fan = boxes
                    .iter()
                    .find(|(t, _, _)| t.ends_with("rpm"))
                    .unwrap_or_else(|| panic!("the unit stays: {n} fans at {h}"));
                assert!(fan.1.right() <= rect.right() - m.pad_x + 0.5, "{n} fans at {h}");
                let label = boxes.iter().find(|(t, _, _)| t.starts_with("Fan")).unwrap();
                assert!(label.1.right() < fan.1.left(), "{n} fans at {h}: label meets value");
                if h == STATUS_CELL_LARGE_H {
                    kept.push(fan.0.split(' ').count() - 1);
                }
            }
        }
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
        let m = sizes_for(STATUS_CELL_LARGE_H);
        let free = 220.0 - 2.0 * m.pad_x - m.gap;
        let got = widths(STATUS_CELL_LARGE_H, |w| lines_in(&snap, Show::System, 3, free, w));
        let value = got
            .iter()
            .find(|l| l.value.ends_with("rpm"))
            .expect("a fan line")
            .value
            .clone();
        let n = value.split(' ').count() - 1;
        assert!(n >= 2 && n < snap.fans.len(), "this test needs a cut: {value}");
        assert_eq!(value, fan_value(&snap.fans[..n]), "the first fans, in order");
        let over = widths(STATUS_CELL_LARGE_H, |w| {
            vec![line(
                "x",
                format!("{}", (w.label)("Fans") + (w.value)(&fan_value(&snap.fans[..n + 1]))),
            )]
        });
        assert!(
            over[0].value.parse::<f32>().unwrap() > free,
            "{} fans would have fit too",
            n + 1
        );
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
        for h in CELL_HEIGHTS {
            let m = sizes_for(h);
            let free = 220.0 - 2.0 * m.pad_x - m.gap;
            let got = widths(h, |w| lines_in(&short, Show::System, 3, free, w));
            assert_eq!((got[0].label.as_str(), got[0].value.as_str()), ("Model", "NUC8i3BEK"));
            let got = widths(h, |w| lines_in(&long, Show::System, 3, free, w));
            let model = got.iter().find(|l| l.label == "Model").unwrap();
            assert!(
                model.value.starts_with("GB-BXi3") && model.value.len() < 33,
                "{}",
                model.value
            );
            let size = Size { w: 1, h: 1 };
            let rect = rect_of(size, h);
            let boxes = text_boxes(&shapes(&System, rect, size, &data(&long), Instant::now()));
            let label = boxes.iter().find(|(t, _, _)| t == "Model").unwrap();
            let value = boxes.iter().find(|(t, _, _)| t.starts_with("GB-")).unwrap();
            assert!(label.1.right() < value.1.left(), "at {h}");
            assert!(value.1.right() <= rect.right() - m.pad_x + 0.5, "at {h}");
        }
    }

    /// Wider than any 40 glyphs of the label font could be cut to.
    fn m_value_em(h: f32) -> f32 {
        sizes_for(h).label
    }

    #[test]
    fn a_long_sensor_name_is_cut_before_its_value() {
        let snap = Snapshot {
            temps: vec![Temp {
                label: "W".repeat(40),
                celsius: 100.0,
                heat: Heat::Normal,
            }],
            ..Snapshot::default()
        };
        let size = Size { w: 1, h: 1 };
        for h in CELL_HEIGHTS {
            let rect = rect_of(size, h);
            let boxes = text_boxes(&shapes(&Temperatures, rect, size, &data(&snap), Instant::now()));
            let value = boxes.iter().find(|(t, _, _)| t == "100 °C").expect("the value stays");
            let label = boxes
                .iter()
                .find(|(t, _, _)| t.starts_with('W'))
                .expect("the name stays");
            assert!(label.1.width() < 40.0 * m_value_em(h), "cut");
            assert!(label.1.right() < value.1.left(), "at {h}");
        }
    }

    #[test]
    fn labels_in_geo_values_in_the_monospace_font() {
        let size = Size { w: 1, h: 2 };
        let s = shapes(
            &Temperatures,
            rect_of(size, STATUS_CELL_LARGE_H),
            size,
            &data(&sample()),
            Instant::now(),
        );
        let mut seen = 0;
        for sh in flatten(&s) {
            if let Shape::Text(t) = sh {
                let family = t.galley.job.sections[0].format.font_id.family.clone();
                let text = t.galley.text();
                let want = if text.ends_with("°C") {
                    FontFamily::Monospace
                } else {
                    crate::brand::geo(10.0).family
                };
                assert_eq!(family, want, "{text}");
                seen += 1;
            }
        }
        assert_eq!(seen, 10, "five labels and five values");
    }

    #[test]
    fn the_degree_sign_has_a_glyph_in_the_monospace_font() {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let mut ok = false;
        for _ in 0..3 {
            let mut o = ctx.run_ui(egui::RawInput::default(), |ui| {
                ok = ui.fonts_mut(|f| f.has_glyph(&FontId::monospace(14.0), '°'));
            });
            o.textures_delta.clear();
        }
        assert!(ok);
    }
}
