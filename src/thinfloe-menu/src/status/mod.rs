//! The status area below the tiles: a grid of the tiles' cell size, four
//! columns and up to three rows, holding elements. Each element draws itself
//! with egui into the rectangle of the cells it covers, with colours from the
//! theme and sizes from `brand.rs`; `layout::pack` places the elements the
//! user chose, in their order, first fit. Only the rows used are drawn; with
//! nothing to show the area is gone.
//!
//! To add an element:
//!
//! 1. Write a module here with a unit struct that implements `Element`. Its
//!    `draw` must be pure (same data and time, same picture) and stay inside
//!    its rectangle; the test `<id>_draws_inside_its_rect_at_every_size` beside
//!    it proves that for every size it offers.
//! 2. List it in `ELEMENTS`. That order is the default order of the area.
//! 3. If it needs data the menu does not sample yet, add a `Source`, a field
//!    of `Data`, and the sampler in `app.rs`, which runs only while
//!    `Area::needs` says so.

pub mod battery;
pub mod clock;
pub mod hardware;
pub mod layout;
pub mod network;
pub mod network_info;

use crate::brand::{self, StatusSizes};
use egui::{Pos2, Rect, Vec2, pos2, vec2};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

/// The grid has the tile grid's column count.
pub const COLUMNS: u8 = 4;
/// At most this many rows.
pub const ROWS: u8 = 3;

/// The size of an element in cells: width 1..=4, height 1..=3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub w: u8,
    pub h: u8,
}

impl Size {
    /// `WxH`, one digit each, inside the grid. No sign, no spaces.
    pub fn parse(text: &str) -> Option<Size> {
        let (w, h) = text.split_once('x')?;
        let one = |s: &str| -> Option<u8> {
            if s.len() == 1 && s.bytes().all(|b| b.is_ascii_digit()) {
                s.parse().ok()
            } else {
                None
            }
        };
        let (w, h) = (one(w)?, one(h)?);
        ((1..=COLUMNS).contains(&w) && (1..=ROWS).contains(&h)).then_some(Size { w, h })
    }
}

impl std::fmt::Display for Size {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.w, self.h)
    }
}

/// The cells an element covers: column and row of its top-left cell, then
/// its width and height in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cells {
    pub col: u8,
    pub row: u8,
    pub w: u8,
    pub h: u8,
}

impl Cells {
    pub fn overlaps(&self, o: &Cells) -> bool {
        self.col < o.col + o.w && o.col < self.col + self.w && self.row < o.row + o.h && o.row < self.row + self.h
    }

    /// The rows down to the element's last one.
    pub fn bottom(&self) -> usize {
        (self.row + self.h) as usize
    }

    pub fn size(&self) -> Size {
        Size { w: self.w, h: self.h }
    }

    /// Where the element goes in an area whose top-left cell starts at
    /// `origin`, with cells `cell` large and `gap` apart.
    pub fn rect(&self, origin: Pos2, cell: Vec2, gap: f32) -> Rect {
        let at = |n: u8, size: f32| n as f32 * (size + gap);
        let span = |n: u8, size: f32| n as f32 * size + (n as f32 - 1.0) * gap;
        Rect::from_min_size(
            pos2(origin.x + at(self.col, cell.x), origin.y + at(self.row, cell.y)),
            vec2(span(self.w, cell.x), span(self.h, cell.y)),
        )
    }
}

/// The sizes of text and spacing for cells `cell_h` points high: the set
/// for the full tile height, the set for the smallest, and in between a
/// value interpolated and rounded to half a point. A 2 x 1 element uses the
/// same sizes as a 1 x 1 one beside it.
pub fn sizes_for(cell_h: f32) -> StatusSizes {
    let (small, large) = (brand::STATUS_SMALL, brand::STATUS_LARGE);
    let (lo, hi) = (brand::STATUS_CELL_SMALL_H, brand::STATUS_CELL_LARGE_H);
    let h = if cell_h.is_finite() { cell_h } else { hi };
    let k = (h.clamp(lo, hi) - lo) / (hi - lo);
    let mix = |a: f32, b: f32| ((a + (b - a) * k) * 2.0).round() / 2.0;
    StatusSizes {
        label: mix(small.label, large.label),
        value: mix(small.value, large.value),
        axis: mix(small.axis, large.axis),
        row: mix(small.row, large.row),
        pad_x: mix(small.pad_x, large.pad_x),
        pad_y: mix(small.pad_y, large.pad_y),
        gap: mix(small.gap, large.gap),
    }
}

/// The height of one cell of an element `rect` of `size` cells: the gaps
/// between its rows are not part of any cell.
pub fn cell_height(rect: Rect, size: Size) -> f32 {
    (rect.height() - f32::from(size.h - 1) * crate::ui::TILE_GAP) / f32::from(size.h)
}

/// The data an element draws from, shared by several elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The battery file of keyboards and mice.
    Battery,
    /// The network sampler.
    Network,
    /// The hardware sampler.
    Hardware,
}

/// What the elements draw from: the menu's data sources, read by the app.
#[derive(Debug, Clone, Copy)]
pub struct Data<'a> {
    pub battery: &'a [crate::battery::Device],
    /// When `battery` was set.
    pub battery_since: Option<Instant>,
    /// The network sampler's last copy; `None` while no network element
    /// samples.
    pub network: Option<&'a crate::netstat::Snapshot>,
    /// Where the network element's session total counts from; `None`: the
    /// menu start.
    pub network_since: Option<&'a crate::netstat::Since>,
    /// The hardware sampler's last copy; `None` while no hardware element
    /// samples.
    pub hardware: Option<&'a crate::hardware::Snapshot>,
    /// The frame's time on the wall clock, read once per frame by the app;
    /// `None`: not known, and the clock shows nothing.
    pub wall: Option<SystemTime>,
    /// The machine's time zone, looked up once per frame by the app; `None`:
    /// UTC.
    pub zone: Option<&'a jiff::tz::TimeZone>,
}

impl Data<'_> {
    /// No source has anything.
    pub const EMPTY: Data<'static> = Data {
        battery: &[],
        battery_since: None,
        network: None,
        network_since: None,
        hardware: None,
        wall: None,
        zone: None,
    };
}

/// One kind of status element.
pub trait Element: Sync {
    /// Its name in the layout file.
    fn id(&self) -> &'static str;
    /// Its name in Settings.
    fn name(&self) -> &'static str;
    /// The sizes it draws well; the first is its default.
    fn sizes(&self) -> &'static [Size];
    /// Its one-row size on a short screen; `None`: it is not shown there.
    fn short_size(&self) -> Option<Size>;
    fn sources(&self) -> &'static [Source];
    /// Whether it has anything to show; one that has not takes no cells.
    fn has_content(&self, data: &Data) -> bool;
    /// Draw into `rect`, which covers `size` cells, and nowhere else.
    fn draw(&self, ui: &mut egui::Ui, rect: Rect, size: Size, data: &Data, now: Instant);
    /// How long until its picture changes on its own (a page turns, the
    /// minute changes); `None` when only new data changes it.
    fn next_change(&self, data: &Data, rect: Rect, now: Instant) -> Option<Duration>;
    /// How often it is drawn again besides; `None`: when its data changes.
    fn refresh(&self) -> Option<Duration>;
}

/// Every element there is, in the default order. One line per element.
pub static ELEMENTS: &[&dyn Element] = &[
    &battery::Battery,
    &network::Network,
    &network_info::NetworkInfo,
    &hardware::Temperatures,
    &hardware::Cpu,
    &hardware::Memory,
    &hardware::System,
    &clock::Clock,
];

pub fn element(id: &str) -> Option<&'static dyn Element> {
    ELEMENTS.iter().copied().find(|e| e.id() == id)
}

/// The sizes element `id` offers, for `layout::parse`.
pub fn offered(id: &str) -> Option<&'static [Size]> {
    element(id).map(|e| e.sizes())
}

/// Every element with its default size, for `layout::default_layout` and
/// `layout::complete`.
pub fn descriptors() -> Vec<layout::Descriptor> {
    ELEMENTS
        .iter()
        .filter_map(|e| e.sizes().first().map(|s| (e.id(), *s)))
        .collect()
}

/// Where the layout file lives below the home.
pub const FILE: &str = ".config/thinfloe/status";

/// Read the layout file and add the elements it does not name. A missing
/// file is the default list; one that cannot be read, or that `layout::parse`
/// refuses, is the default list with one note, and `refused` set. At most
/// one byte past the limit is read, so a huge file costs nothing.
pub fn read_layout(path: &Path) -> layout::Layout {
    use std::io::Read;
    let read = std::fs::File::open(path).and_then(|f| {
        let mut bytes = Vec::new();
        f.take(layout::MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    });
    let mut l = match read {
        Ok(bytes) => layout::parse(&bytes, &offered),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => layout::Layout::default(),
        Err(e) => layout::refuse(&format!("it cannot be read: {e}")),
    };
    layout::complete(&mut l, &descriptors());
    l
}

/// The status area: the user's choice of elements, sizes and order.
#[derive(Debug, Clone, Default)]
pub struct Area {
    pub layout: layout::Layout,
}

impl Area {
    /// The shipped layout: every element, shown, at its default size.
    pub fn shipped() -> Area {
        let mut layout = layout::parse(b"", &offered);
        layout::complete(&mut layout, &descriptors());
        Area { layout }
    }

    /// The shown choices this image knows, in order, with their size.
    fn chosen(&self) -> impl Iterator<Item = (&'static dyn Element, Size)> + '_ {
        self.layout
            .choices
            .iter()
            .filter(|c| c.shown && c.known)
            .filter_map(|c| element(&c.id).map(|e| (e, c.size)))
    }

    fn packed(items: Vec<(&'static dyn Element, Size)>, short: bool) -> Vec<(&'static dyn Element, Size, Cells)> {
        let sizes: Vec<Size> = items.iter().map(|(_, s)| *s).collect();
        let shorts: Vec<Option<Size>> = items.iter().map(|(e, _)| e.short_size()).collect();
        let cells = layout::pack(&sizes, short, &shorts);
        items
            .into_iter()
            .zip(cells)
            .filter_map(|((e, _), c)| c.map(|c| (e, c.size(), c)))
            .collect()
    }

    /// The elements drawn now: the shown choices with something to show,
    /// packed, each with the size and cells it takes on this screen.
    pub fn shown(&self, data: &Data, short: bool) -> Vec<(&'static dyn Element, Size, Cells)> {
        let items = self.chosen().filter(|(e, _)| e.has_content(data)).collect();
        Self::packed(items, short)
    }

    /// The rows the shown choices take when all have something to show: the
    /// tiles and the top of the area are placed by this, so they stay where
    /// they are when an element appears or goes. 0: no area.
    pub fn planned_rows(&self, short: bool) -> usize {
        Self::packed(self.chosen().collect(), short)
            .iter()
            .map(|(_, _, c)| c.bottom())
            .max()
            .unwrap_or(0)
    }

    /// The rows the elements drawn now use; 0 hides the area.
    pub fn rows(&self, data: &Data, short: bool) -> usize {
        self.shown(data, short)
            .iter()
            .map(|(_, _, c)| c.bottom())
            .max()
            .unwrap_or(0)
    }

    /// The soonest an element drawn now wants a frame, in an area whose
    /// top-left cell is at `origin`: its refresh interval or its own next
    /// change. `None`: only new data redraws the area.
    pub fn wake(&self, data: &Data, origin: Pos2, cell: Vec2, gap: f32, now: Instant, short: bool) -> Option<Duration> {
        self.shown(data, short)
            .into_iter()
            .flat_map(|(e, _, c)| [e.refresh(), e.next_change(data, c.rect(origin, cell, gap), now)])
            .flatten()
            .min()
    }

    /// Some shown choice draws from `source`, whether or not it has anything
    /// yet: its source must run to find out.
    pub fn needs(&self, source: Source) -> bool {
        self.chosen().any(|(e, _)| e.sources().contains(&source))
    }

    /// Some shown choice that draws from `source` gets cells on this screen,
    /// whether or not it has anything yet. One left out for want of room, or
    /// without a size for a short screen, does not count.
    pub fn places(&self, source: Source, short: bool) -> bool {
        Self::packed(self.chosen().collect(), short)
            .iter()
            .any(|(e, _, _)| e.sources().contains(&source))
    }
}

/// Fill `rect` with the tiles' colour, as every element's background.
pub fn background(ui: &egui::Ui, rect: Rect) {
    let card = shadcn_egui::Theme::current(ui.ctx()).palette().card;
    ui.painter().rect_filled(rect, 0.0, card);
}

#[cfg(test)]
pub(crate) mod testing {
    //! Draw one element headless and collect what it drew.
    use super::*;

    /// A 1 x 1 cell of the full tile height, and of the smallest.
    pub const CELL_HEIGHTS: [f32; 2] = [brand::STATUS_CELL_LARGE_H, brand::STATUS_CELL_SMALL_H];
    pub const CELL_W: f32 = 220.0;
    pub const GAP: f32 = 3.0;

    /// The rectangle of an element of `size` at the top left of an area at
    /// (40, 30), cells `cell_h` high.
    pub fn rect_of(size: Size, cell_h: f32) -> Rect {
        Cells {
            col: 0,
            row: 0,
            w: size.w,
            h: size.h,
        }
        .rect(pos2(40.0, 30.0), vec2(CELL_W, cell_h), GAP)
    }

    /// The shapes `e` draws into `rect` in one frame, the fonts settled.
    pub fn shapes(e: &dyn Element, rect: Rect, size: Size, data: &Data, now: Instant) -> Vec<egui::Shape> {
        let ctx = egui::Context::default();
        brand::apply(&ctx);
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 600.0))),
            ..Default::default()
        };
        let mut out = None;
        for _ in 0..3 {
            let mut o = ctx.run_ui(input(), |ui| {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                child.set_clip_rect(rect);
                e.draw(&mut child, rect, size, data, now);
            });
            o.textures_delta.clear();
            out = Some(o);
        }
        out.map(|o| o.shapes.into_iter().map(|c| c.shape).collect())
            .unwrap_or_default()
    }

    /// The text of every text shape, in order, with its colour.
    pub fn texts(shapes: &[egui::Shape]) -> Vec<(String, egui::Color32)> {
        fn walk(s: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
            match s {
                egui::Shape::Text(t) => out.push((t.galley.text().to_string(), t.fallback_color)),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        shapes.iter().for_each(|s| walk(s, &mut out));
        out
    }

    fn flat<'a>(shapes: &'a [egui::Shape], out: &mut Vec<&'a egui::Shape>) {
        for s in shapes {
            match s {
                egui::Shape::Vec(v) => flat(v, out),
                other => out.push(other),
            }
        }
    }

    /// The shapes with `Shape::Vec` groups opened, in drawing order.
    pub fn flatten(shapes: &[egui::Shape]) -> Vec<&egui::Shape> {
        let mut out = Vec::new();
        flat(shapes, &mut out);
        out
    }

    /// Every polyline (`Shape::line`): its points, width and colour.
    pub fn paths(shapes: &[egui::Shape]) -> Vec<(Vec<Pos2>, f32, egui::Color32)> {
        flatten(shapes)
            .into_iter()
            .filter_map(|s| match s {
                egui::Shape::Path(p) => match p.stroke.color {
                    egui::epaint::ColorMode::Solid(c) => Some((p.points.clone(), p.stroke.width, c)),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    /// Every line segment: its end points and stroke.
    pub fn segments(shapes: &[egui::Shape]) -> Vec<([Pos2; 2], egui::Stroke)> {
        flatten(shapes)
            .into_iter()
            .filter_map(|s| match s {
                egui::Shape::LineSegment { points, stroke } => Some((*points, *stroke)),
                _ => None,
            })
            .collect()
    }

    /// Every filled rectangle: its rectangle and fill.
    pub fn rects(shapes: &[egui::Shape]) -> Vec<(Rect, egui::Color32)> {
        flatten(shapes)
            .into_iter()
            .filter_map(|s| match s {
                egui::Shape::Rect(r) => Some((r.rect, r.fill)),
                _ => None,
            })
            .collect()
    }

    /// Every text: its string, where its ink lies and its colour.
    pub fn text_boxes(shapes: &[egui::Shape]) -> Vec<(String, Rect, egui::Color32)> {
        flatten(shapes)
            .into_iter()
            .filter_map(|s| match s {
                egui::Shape::Text(t) => Some((t.galley.text().to_string(), s.visual_bounding_rect(), t.fallback_color)),
                _ => None,
            })
            .collect()
    }

    /// Every shape `e` draws at each size it offers, at both cell heights,
    /// lies inside its rectangle.
    pub fn assert_inside(e: &dyn Element, data: &Data, now: Instant) {
        for &size in e.sizes() {
            for h in CELL_HEIGHTS {
                let rect = rect_of(size, h);
                let shapes = shapes(e, rect, size, data, now);
                assert!(!shapes.is_empty(), "{} {size} at {h}: draws nothing", e.id());
                for s in &shapes {
                    let b = s.visual_bounding_rect();
                    assert!(
                        rect.expand(0.01).contains_rect(b),
                        "{} {size} at {h}: {b:?} outside {rect:?}: {s:?}",
                        e.id()
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(col: u8, row: u8, w: u8, h: u8) -> Cells {
        Cells { col, row, w, h }
    }

    #[test]
    fn overlap_is_a_shared_cell() {
        let a = cells(0, 0, 2, 1);
        assert!(a.overlaps(&cells(1, 0, 1, 1)));
        assert!(a.overlaps(&cells(0, 0, 4, 3)));
        assert!(!a.overlaps(&cells(2, 0, 2, 1)));
        assert!(!a.overlaps(&cells(0, 1, 2, 2)));
    }

    #[test]
    fn an_element_spans_its_cells_and_the_gaps_between() {
        let cell = vec2(220.0, 76.0);
        let r = cells(0, 0, 1, 1).rect(pos2(10.0, 20.0), cell, 3.0);
        assert_eq!(r, Rect::from_min_size(pos2(10.0, 20.0), cell));
        let r = cells(1, 1, 3, 2).rect(pos2(0.0, 0.0), cell, 3.0);
        assert_eq!(r.min, pos2(223.0, 79.0));
        assert_eq!(r.size(), vec2(3.0 * 220.0 + 6.0, 2.0 * 76.0 + 3.0));
        // At a smaller cell (the scaled-down or squeezed grid) it follows.
        let r = cells(3, 2, 1, 1).rect(pos2(0.0, 0.0), vec2(100.0, 64.0), 3.0);
        assert_eq!(r.max, pos2(4.0 * 100.0 + 9.0, 3.0 * 64.0 + 6.0));
    }

    #[test]
    fn the_sizes_follow_the_cell_height() {
        assert_eq!(sizes_for(76.0), brand::STATUS_LARGE);
        assert_eq!(sizes_for(64.0), brand::STATUS_SMALL);
        assert_eq!(sizes_for(200.0), brand::STATUS_LARGE);
        assert_eq!(sizes_for(f32::NAN), brand::STATUS_LARGE);
        let mid = sizes_for(70.0);
        assert!(mid.label > brand::STATUS_SMALL.label && mid.label < brand::STATUS_LARGE.label);
        assert_eq!(mid.label * 2.0, (mid.label * 2.0).round(), "half points");
    }

    #[test]
    fn every_element_has_a_unique_id_and_offers_a_size() {
        let mut ids: Vec<_> = ELEMENTS.iter().map(|e| e.id()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ELEMENTS.len());
        for e in ELEMENTS {
            assert!(!e.sizes().is_empty(), "{}", e.id());
            assert!(e.short_size().is_none_or(|s| s.h == 1), "{}", e.id());
            assert!(element(e.id()).is_some());
        }
        assert!(element("radio").is_none());
    }

    #[test]
    fn the_default_list_is_every_element_in_order() {
        let ids: Vec<String> = Area::shipped().layout.choices.into_iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                "battery",
                "network",
                "network-info",
                "temperatures",
                "cpu",
                "memory",
                "system",
                "clock"
            ]
        );
    }

    fn area(text: &str) -> Area {
        let mut layout = layout::parse(text.as_bytes(), &offered);
        layout::complete(&mut layout, &descriptors());
        Area { layout }
    }

    fn device() -> Vec<crate::battery::Device> {
        vec![crate::battery::Device {
            name: "K".into(),
            percent: Some(50),
            state: "discharging".into(),
            read_epoch: 1,
            kind: crate::battery::DeviceKind::Keyboard,
        }]
    }

    fn ids(v: &[(&'static dyn Element, Size, Cells)]) -> Vec<(&'static str, Cells)> {
        v.iter().map(|(e, _, c)| (e.id(), *c)).collect()
    }

    #[test]
    fn an_element_without_content_takes_no_cells_but_keeps_its_planned_row() {
        let a = Area::shipped();
        let devices = device();
        let with = Data {
            battery: &devices,
            ..Data::EMPTY
        };
        assert_eq!(
            ids(&a.shown(&with, false)),
            [("battery", cells(0, 0, 1, 1)), ("clock", cells(1, 0, 1, 1))]
        );
        // No device and no network: the clock moves into the first cell. The
        // planned rows are those with everything shown.
        assert_eq!(ids(&a.shown(&Data::EMPTY, false)), [("clock", cells(0, 0, 1, 1))]);
        assert_eq!(a.rows(&Data::EMPTY, false), 1);
        assert_eq!(a.planned_rows(false), 3);
        // A network with a route brings the graph and the list in, and the
        // clock goes down to the second row.
        let net = crate::status::network::sample(10);
        let with_net = Data {
            battery: &devices,
            network: Some(&net),
            ..Data::EMPTY
        };
        assert_eq!(
            ids(&a.shown(&with_net, false)),
            [
                ("battery", cells(0, 0, 1, 1)),
                ("network", cells(1, 0, 2, 2)),
                ("network-info", cells(3, 0, 1, 2)),
                ("clock", cells(0, 1, 1, 1)),
            ]
        );
        assert_eq!(a.rows(&with_net, false), 2);
        // A hidden element takes no part, planned or drawn.
        let hidden = area(
            "-battery 1x1\n-network 2x2\n-network-info 1x2\n-temperatures 1x1\n-cpu 1x1\n-memory 1x1\n-system 1x1\n-clock 1x1\n",
        );
        assert_eq!(hidden.planned_rows(false), 0);
        assert_eq!(hidden.rows(&with, false), 0);
        assert!(!hidden.needs(Source::Battery) && !hidden.needs(Source::Network));
        assert!(a.needs(Source::Battery) && a.needs(Source::Network));
        assert!(a.needs(Source::Hardware), "the hardware elements are in the list too");
        // An empty area: nothing.
        assert_eq!(Area::default().planned_rows(false), 0);
    }

    #[test]
    fn the_order_and_sizes_of_the_layout_are_kept() {
        let devices = device();
        let with = Data {
            battery: &devices,
            ..Data::EMPTY
        };
        let a = area("clock 1x1\nbattery 2x1\n");
        assert_eq!(
            ids(&a.shown(&with, false)),
            [("clock", cells(0, 0, 1, 1)), ("battery", cells(1, 0, 2, 1))]
        );
    }

    #[test]
    fn a_short_screen_takes_each_short_size_or_leaves_the_element_out() {
        let devices = device();
        let with = Data {
            battery: &devices,
            ..Data::EMPTY
        };
        let a = area("battery 2x1\nclock 1x1\n");
        // The battery's short size is 1x1; the clock has none.
        assert_eq!(ids(&a.shown(&with, true)), [("battery", cells(0, 0, 1, 1))]);
        assert_eq!(a.planned_rows(true), 1);
        let clock_only = area("-battery 1x1\n-network 2x2\n-network-info 1x2\nclock 1x1\n");
        assert_eq!(clock_only.planned_rows(true), 0);
        assert_eq!(clock_only.rows(&with, true), 0);
    }

    #[test]
    fn a_short_screen_gives_the_chosen_network_its_short_size_and_leaves_the_list_out() {
        let net = crate::status::network::sample(10);
        let with = Data {
            network: Some(&net),
            ..Data::EMPTY
        };
        let a = area("network 2x2\nnetwork-info 1x2\n-battery 1x1\n-clock 1x1\n");
        assert_eq!(
            ids(&a.shown(&with, false)),
            [("network", cells(0, 0, 2, 2)), ("network-info", cells(2, 0, 1, 2))]
        );
        // The graph takes its short size, 3x1; the list has none.
        let short = a.shown(&with, true);
        assert_eq!(ids(&short), [("network", cells(0, 0, 3, 1))]);
        assert_eq!(short[0].1, Size { w: 3, h: 1 });
        assert_eq!(a.planned_rows(true), 1);
        assert_eq!(a.rows(&with, true), 1);
        // The list alone has nowhere to go on a short screen.
        let list = area("-network 2x2\nnetwork-info 1x2\n-battery 1x1\n-clock 1x1\n");
        assert_eq!(list.planned_rows(true), 0);
        assert_eq!(list.rows(&with, true), 0);
    }

    #[test]
    fn the_soonest_element_drawn_sets_the_wake() {
        let t0 = Instant::now();
        let devices: Vec<_> = (0..6)
            .map(|i| crate::battery::Device {
                name: format!("D{i}"),
                ..device()[0].clone()
            })
            .collect();
        let paging = Data {
            battery: &devices,
            battery_since: Some(t0),
            ..Data::EMPTY
        };
        let at = t0 + Duration::from_millis(1500);
        let cell = vec2(220.0, 76.0);
        let battery_only = area("battery 1x1\n-clock 1x1\n");
        assert_eq!(
            battery_only.wake(&paging, Pos2::ZERO, cell, 3.0, at, false),
            Some(Duration::from_millis(2500))
        );
        // A battery that fits does not page: no wake from it.
        assert_eq!(battery_only.wake(&Data::EMPTY, Pos2::ZERO, cell, 3.0, at, false), None);
        // The clock wakes within the minute.
        let timed = Data {
            wall: Some(SystemTime::now()),
            ..Data::EMPTY
        };
        let both = Area::shipped().wake(&timed, Pos2::ZERO, cell, 3.0, at, false);
        assert!(both.is_some_and(|d| d <= Duration::from_secs(60)), "{both:?}");
    }

    #[test]
    fn the_default_list_built_from_the_elements_packs_to_the_pr6_layout() {
        let ids: Vec<&str> = ELEMENTS.iter().map(|e| e.id()).collect();
        assert_eq!(
            ids,
            [
                "battery",
                "network",
                "network-info",
                "temperatures",
                "cpu",
                "memory",
                "system",
                "clock"
            ]
        );
        let sizes: Vec<Size> = ELEMENTS.iter().filter_map(|e| e.sizes().first().copied()).collect();
        let shorts: Vec<Option<Size>> = ELEMENTS.iter().map(|e| e.short_size()).collect();
        let got: Vec<(u8, u8, u8, u8)> = layout::pack(&sizes, false, &shorts)
            .iter()
            .map(|c| {
                let c = c.expect("every element has a place");
                (c.col, c.row, c.w, c.h)
            })
            .collect();
        assert_eq!(
            got,
            [
                (0, 0, 1, 1),
                (1, 0, 2, 2),
                (3, 0, 1, 2),
                (0, 1, 1, 1),
                (0, 2, 1, 1),
                (1, 2, 1, 1),
                (2, 2, 1, 1),
                (3, 2, 1, 1)
            ]
        );
        assert_eq!(Area::shipped().planned_rows(false), 3);
    }

    #[test]
    fn a_source_is_placed_only_where_an_element_of_it_gets_cells() {
        let shipped = Area::shipped();
        assert!(shipped.places(Source::Hardware, false));
        // On a short screen the hardware elements have no size.
        assert!(!shipped.places(Source::Hardware, true));
        assert!(shipped.places(Source::Network, true), "the graph has a short size");
        let hidden = area("-temperatures 1x1\n-cpu 1x1\n-memory 1x1\n-system 1x1\n");
        assert!(!hidden.places(Source::Hardware, false));
        assert!(!hidden.needs(Source::Hardware));
        assert!(!Area::default().places(Source::Hardware, false));
    }
}
