//! The status area below the tiles: a grid of the tiles' cell size, four
//! columns and up to three rows, holding elements. Each element covers a
//! rectangle of cells, comes from a file of its own (see `config`), is drawn
//! as an SVG it builds itself and rendered in-process with resvg (`render`),
//! and says when its picture next changes on its own. An element moves up
//! until a shown element above it in its columns or the top stops it, so no
//! row stays blank: only the rows the shown elements then use are drawn. With
//! nothing to show the area is gone.
//!
//! A new element type is one module with an `Element` and a `Kind`, listed
//! in `KINDS`, plus a file in `/etc/thinfloe/status/`.

pub mod battery;
pub mod clock;
pub mod config;
pub mod hardware;
pub mod layout;
pub mod network;
pub mod network_info;
pub mod render;
pub mod style;

use crate::desktop::Entry;
use egui::{Pos2, Rect, Vec2, pos2, vec2};
use std::time::{Duration, Instant};

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
    /// `column,row,width,height`, inside the 4 x 3 grid, at least 1 x 1.
    pub fn parse(text: &str) -> Result<Cells, String> {
        let bad = || format!("Cells must be column,row,width,height, not '{text}'");
        let parts: Vec<&str> = text.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            return Err(bad());
        }
        let mut n = [0u8; 4];
        for (slot, p) in n.iter_mut().zip(&parts) {
            if p.is_empty() || p.len() > 2 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            *slot = p.parse().map_err(|_| bad())?;
        }
        let c = Cells {
            col: n[0],
            row: n[1],
            w: n[2],
            h: n[3],
        };
        if c.w == 0 || c.h == 0 {
            return Err(format!("Cells {text} cover no cell"));
        }
        if c.col as u16 + c.w as u16 > COLUMNS as u16 || c.row as u16 + c.h as u16 > ROWS as u16 {
            return Err(format!("Cells {text} reach outside the {COLUMNS} x {ROWS} grid"));
        }
        Ok(c)
    }

    pub fn overlaps(&self, o: &Cells) -> bool {
        self.col < o.col + o.w && o.col < self.col + self.w && self.row < o.row + o.h && o.row < self.row + self.h
    }

    /// The rows down to the element's last one.
    pub fn bottom(&self) -> usize {
        (self.row + self.h) as usize
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

    /// The room an element of these cells gets, cells `cell` large and
    /// `gap` apart.
    pub fn room(&self, cell: Vec2, gap: f32) -> Room {
        Room {
            size: self.rect(Pos2::ZERO, cell, gap).size(),
            cell,
        }
    }
}

/// What an element is drawn into: its rectangle's size and the size of
/// one cell, both in points. The cell height picks the sizes of text and
/// spacing (`style::Style::for_cell`), so a 3 x 2 element uses the same
/// sizes as a 1 x 1 one beside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Room {
    pub size: Vec2,
    pub cell: Vec2,
}

impl Room {
    /// The sizes for this room's cells.
    pub fn style(&self) -> style::Style {
        style::Style::for_cell(self.cell.y)
    }

    /// One cell `cell` large: the room of a 1 x 1 element.
    #[cfg(test)]
    pub fn one(cell: Vec2) -> Room {
        Room { size: cell, cell }
    }
}

/// When an element is drawn again besides the changes it reports itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// When its data source changes (the battery file, say).
    Change,
    /// Every so often (a traffic graph, every second).
    Every(Duration),
}

/// What the elements draw from: the menu's data sources, read by the app.
#[derive(Debug, Clone, Copy)]
pub struct Sources<'a> {
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
}

/// One element type's drawing. `room.size` is the element's rectangle in
/// points; the SVG is drawn into exactly that.
pub trait Element {
    /// Whether it has anything to show; an area of elements that have
    /// nothing is not drawn.
    fn visible(&self, data: &Sources) -> bool;
    /// The picture now, as SVG text. The same text means the same picture,
    /// so the menu renders it again only when the text changes.
    fn svg(&self, data: &Sources, room: Room, now: Instant) -> String;
    /// How long until the picture changes on its own (a page turns); `None`
    /// when only new data changes it.
    fn next_change(&self, data: &Sources, room: Room, now: Instant) -> Option<Duration>;
}

/// An element type: its name in the `Type=` key, the further keys its file
/// may set, and how to make one from the file.
pub struct Kind {
    pub name: &'static str,
    pub options: &'static [&'static str],
    pub build: fn(&Entry) -> Result<Box<dyn Element>, String>,
}

/// Every element type there is. One line per type.
pub const KINDS: &[Kind] = &[
    battery::KIND,
    network::KIND,
    network_info::KIND,
    hardware::KIND,
    clock::KIND,
];

pub fn kind(name: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.name == name)
}

/// Each of `cells` moved up until a cell above it in one of its columns, or
/// the top of the area, stops it, the highest first. Rectangles that did not
/// overlap still do not, and none moves down.
fn moved_up(cells: &[Cells]) -> Vec<Cells> {
    let mut order: Vec<usize> = (0..cells.len()).collect();
    order.sort_by_key(|&i| cells[i].row);
    let mut out = cells.to_vec();
    for (done, &i) in order.iter().enumerate() {
        let c = cells[i];
        let top = order[..done]
            .iter()
            .map(|&j| out[j])
            .filter(|o| o.col < c.col + c.w && c.col < o.col + o.w)
            .map(|o| o.bottom())
            .max()
            .unwrap_or(0);
        out[i].row = c.row.min(top as u8);
    }
    out
}

/// An element from its file: the file name names it.
pub struct Placed {
    pub name: String,
    pub kind: &'static str,
    pub cells: Cells,
    /// Its cells on a short screen (`ShortSize=`), `cells` without that key;
    /// `None` (`ShortSize=none`): it is not on a short screen at all.
    pub short: Option<Cells>,
    pub refresh: Refresh,
    pub element: Box<dyn Element>,
}

impl Placed {
    /// The cells it covers on a short screen, or on any other; `None`: it
    /// takes no part there, no row, no picture and no wake.
    pub fn cells_on(&self, short: bool) -> Option<Cells> {
        if short { self.short } else { Some(self.cells) }
    }
}

/// The elements in file-name order, none overlapping, and what was
/// refused and why.
#[derive(Default)]
pub struct Area {
    pub elements: Vec<Placed>,
    pub notes: Vec<String>,
    /// A file or the directory was refused (`--check` fails on it). A
    /// missing directory is not refused: it means no elements.
    pub shipped_refused: bool,
}

impl Area {
    /// The elements with something to show on this screen, in file-name
    /// order, with the cells they take: their own, moved up (`moved_up`).
    pub fn shown<'a>(&'a self, data: &'a Sources, short: bool) -> impl Iterator<Item = (&'a Placed, Cells)> + 'a {
        let shown: Vec<(&Placed, Cells)> = self
            .elements
            .iter()
            .filter_map(|p| p.cells_on(short).map(|c| (p, c)))
            .filter(|(p, _)| p.element.visible(data))
            .collect();
        let cells: Vec<Cells> = shown.iter().map(|(_, c)| *c).collect();
        let moved = moved_up(&cells);
        shown.into_iter().map(|(p, _)| p).zip(moved)
    }

    /// An element of this kind takes part on this screen, whether or not it
    /// has anything to show yet: its data source must run to find out.
    pub fn has_on(&self, kind: &str, short: bool) -> bool {
        self.elements
            .iter()
            .any(|p| p.kind == kind && p.cells_on(short).is_some())
    }

    /// The rows the shown elements use; 0 hides the area.
    pub fn rows(&self, data: &Sources, short: bool) -> usize {
        self.shown(data, short).map(|(_, c)| c.bottom()).max().unwrap_or(0)
    }

    /// The rows the elements take on this screen when all show, whatever they
    /// have to show now: the tiles and the top of the area are placed by this,
    /// so they stay where they are when an element appears or goes (the
    /// elements inside the area close up). 0: no element at all.
    pub fn planned_rows(&self, short: bool) -> usize {
        self.elements
            .iter()
            .filter_map(|p| p.cells_on(short))
            .map(|c| c.bottom())
            .max()
            .unwrap_or(0)
    }

    /// The soonest a shown element wants a frame: its refresh interval or
    /// its own next change. `None`: only new data redraws the area.
    pub fn wake(&self, data: &Sources, cell: Vec2, gap: f32, now: Instant, short: bool) -> Option<Duration> {
        self.shown(data, short)
            .flat_map(|(p, cells)| {
                let room = cells.room(cell, gap);
                let every = match p.refresh {
                    Refresh::Every(d) => Some(d),
                    Refresh::Change => None,
                };
                [every, p.element.next_change(data, room, now)]
            })
            .flatten()
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(col: u8, row: u8, w: u8, h: u8) -> Cells {
        Cells { col, row, w, h }
    }

    #[test]
    fn cells_parse_inside_the_grid_only() {
        assert_eq!(Cells::parse("0,0,1,1"), Ok(cells(0, 0, 1, 1)));
        assert_eq!(Cells::parse(" 1, 2 ,3,1 "), Ok(cells(1, 2, 3, 1)));
        assert_eq!(Cells::parse("0,0,4,3"), Ok(cells(0, 0, 4, 3)));
        for bad in [
            "",
            "0,0,1",
            "0,0,1,1,1",
            "a,0,1,1",
            "-1,0,1,1",
            "0,0,1,+1",
            "0,0,100,1",
            "0;0;1;1",
        ] {
            assert_eq!(
                Cells::parse(bad),
                Err(format!("Cells must be column,row,width,height, not '{bad}'"))
            );
        }
        assert_eq!(Cells::parse("0,0,0,1"), Err("Cells 0,0,0,1 cover no cell".into()));
        assert_eq!(
            Cells::parse("3,0,2,1"),
            Err("Cells 3,0,2,1 reach outside the 4 x 3 grid".into())
        );
        assert_eq!(
            Cells::parse("0,2,1,2"),
            Err("Cells 0,2,1,2 reach outside the 4 x 3 grid".into())
        );
        assert!(Cells::parse("99,99,99,99").is_err());
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
        // Its room: that size, and the cell it was cut from.
        let room = cells(1, 0, 3, 2).room(cell, 3.0);
        assert_eq!(room.size, vec2(666.0, 155.0));
        assert_eq!(room.cell, cell);
        assert_eq!(room.style(), style::LARGE);
        assert_eq!(cells(0, 0, 1, 1).room(vec2(220.0, 64.0), 3.0).style(), style::SMALL);
        assert_eq!(Room::one(cell), cells(0, 0, 1, 1).room(cell, 3.0));
    }

    struct Fixed {
        shows: bool,
        next: Option<Duration>,
    }

    impl Element for Fixed {
        fn visible(&self, _: &Sources) -> bool {
            self.shows
        }
        fn svg(&self, _: &Sources, _: Room, _: Instant) -> String {
            String::new()
        }
        fn next_change(&self, _: &Sources, _: Room, _: Instant) -> Option<Duration> {
            self.next
        }
    }

    fn placed(name: &str, c: Cells, refresh: Refresh, shows: bool, next: Option<Duration>) -> Placed {
        Placed {
            name: name.into(),
            kind: "fixed",
            cells: c,
            short: Some(c),
            refresh,
            element: Box::new(Fixed { shows, next }),
        }
    }

    const DATA: Sources = Sources {
        battery: &[],
        battery_since: None,
        network: None,
        network_since: None,
        hardware: None,
    };

    #[test]
    fn an_element_is_on_a_screen_by_its_own_form() {
        let mut area = Area::default();
        assert!(!area.has_on("fixed", false), "no such element");
        let mut p = placed("a", cells(0, 0, 1, 1), Refresh::Change, false, None);
        p.short = None;
        area.elements.push(p);
        assert!(area.has_on("fixed", false));
        assert!(!area.has_on("fixed", true), "ShortSize=none");
        assert!(!area.has_on("other", false));
        area.elements[0].short = Some(cells(0, 0, 1, 1));
        assert!(area.has_on("fixed", true), "it has a short form");
    }

    #[test]
    fn a_short_screen_counts_the_short_form_of_an_element() {
        let mut area = Area::default();
        let mut p = placed("a", cells(1, 0, 3, 2), Refresh::Change, true, None);
        p.short = Some(cells(1, 0, 3, 1));
        area.elements.push(p);
        assert_eq!(area.rows(&DATA, false), 2);
        assert_eq!(area.rows(&DATA, true), 1);
        assert_eq!(area.elements[0].cells_on(true), Some(cells(1, 0, 3, 1)));
        assert_eq!(area.elements[0].cells_on(false), Some(cells(1, 0, 3, 2)));
    }

    #[test]
    fn an_element_without_a_short_form_takes_no_part_on_a_short_screen() {
        let s = Duration::from_secs;
        let now = Instant::now();
        let cell = vec2(220.0, 76.0);
        let mut area = Area::default();
        area.elements
            .push(placed("a", cells(0, 0, 1, 1), Refresh::Change, true, Some(s(9))));
        let mut gone = placed("b", cells(0, 1, 1, 1), Refresh::Every(s(5)), true, Some(s(2)));
        gone.short = None;
        area.elements.push(gone);
        assert_eq!(area.elements[1].cells_on(true), None);
        assert_eq!(area.elements[1].cells_on(false), Some(cells(0, 1, 1, 1)));
        // Not counted: its row is not there.
        assert_eq!(area.rows(&DATA, true), 1);
        assert_eq!(area.rows(&DATA, false), 2);
        // Not painted.
        let names = |short| {
            area.shown(&DATA, short)
                .map(|(p, _)| p.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(true), ["a"]);
        assert_eq!(names(false), ["a", "b"]);
        // No wake of its own.
        assert_eq!(area.wake(&DATA, cell, 3.0, now, true), Some(s(9)));
        assert_eq!(area.wake(&DATA, cell, 3.0, now, false), Some(s(2)));
    }

    #[test]
    fn rows_are_those_the_shown_elements_use() {
        let mut area = Area::default();
        assert_eq!(area.rows(&DATA, false), 0, "no element: no area");
        area.elements
            .push(placed("a", cells(0, 0, 1, 1), Refresh::Change, true, None));
        assert_eq!(area.rows(&DATA, false), 1);
        area.elements
            .push(placed("b", cells(1, 0, 3, 3), Refresh::Change, false, None));
        assert_eq!(area.rows(&DATA, false), 1, "a hidden element takes no row");
        area.elements
            .push(placed("c", cells(0, 1, 2, 1), Refresh::Change, true, None));
        assert_eq!(area.rows(&DATA, false), 2);
        let empty = Area {
            elements: vec![placed("a", cells(0, 0, 1, 1), Refresh::Change, false, None)],
            ..Area::default()
        };
        assert_eq!(empty.rows(&DATA, false), 0, "nothing to show: no area");
    }

    /// The shipped layout at 900 points and above, with which elements show.
    fn shipped(battery: bool, network: bool, temperatures: bool, cpu: bool, memory: bool, system: bool) -> Area {
        Area {
            elements: vec![
                placed("10-battery", cells(0, 0, 1, 1), Refresh::Change, battery, None),
                placed("20-network", cells(1, 0, 2, 2), Refresh::Change, network, None),
                placed("25-info", cells(3, 0, 1, 2), Refresh::Change, network, None),
                placed(
                    "30-temperatures",
                    cells(0, 1, 1, 1),
                    Refresh::Change,
                    temperatures,
                    None,
                ),
                placed("31-cpu", cells(0, 2, 1, 1), Refresh::Change, cpu, None),
                placed("32-memory", cells(1, 2, 1, 1), Refresh::Change, memory, None),
                placed("33-system", cells(2, 2, 1, 1), Refresh::Change, system, None),
                placed("40-clock", cells(3, 2, 1, 1), Refresh::Change, true, None),
            ],
            ..Area::default()
        }
    }

    fn where_is(area: &Area, name: &str) -> Option<Cells> {
        area.shown(&DATA, false).find(|(p, _)| p.name == name).map(|(_, c)| c)
    }

    #[test]
    fn an_element_with_nothing_shown_above_it_moves_up() {
        // All there: the cells of the files.
        let all = shipped(true, true, true, true, true, true);
        assert_eq!(where_is(&all, "30-temperatures"), Some(cells(0, 1, 1, 1)));
        assert_eq!(where_is(&all, "31-cpu"), Some(cells(0, 2, 1, 1)));
        assert_eq!(where_is(&all, "32-memory"), Some(cells(1, 2, 1, 1)));
        assert_eq!(where_is(&all, "33-system"), Some(cells(2, 2, 1, 1)));
        assert_eq!(where_is(&all, "40-clock"), Some(cells(3, 2, 1, 1)));
        assert_eq!(all.rows(&DATA, false), 3);
        // No battery: its column closes up; the network elements and what is
        // under them stay.
        let no_battery = shipped(false, true, true, true, true, true);
        assert_eq!(where_is(&no_battery, "30-temperatures"), Some(cells(0, 0, 1, 1)));
        assert_eq!(where_is(&no_battery, "31-cpu"), Some(cells(0, 1, 1, 1)));
        assert_eq!(where_is(&no_battery, "20-network"), Some(cells(1, 0, 2, 2)));
        assert_eq!(where_is(&no_battery, "25-info"), Some(cells(3, 0, 1, 2)));
        assert_eq!(where_is(&no_battery, "32-memory"), Some(cells(1, 2, 1, 1)));
        assert_eq!(where_is(&no_battery, "33-system"), Some(cells(2, 2, 1, 1)));
        assert_eq!(no_battery.rows(&DATA, false), 3);
        // No network: memory and system stand in the top row.
        let no_network = shipped(true, false, true, true, true, true);
        assert_eq!(where_is(&no_network, "32-memory"), Some(cells(1, 0, 1, 1)));
        assert_eq!(where_is(&no_network, "33-system"), Some(cells(2, 0, 1, 1)));
        assert_eq!(where_is(&no_network, "40-clock"), Some(cells(3, 0, 1, 1)));
        assert_eq!(where_is(&no_network, "31-cpu"), Some(cells(0, 2, 1, 1)));
        assert_eq!(no_network.rows(&DATA, false), 3);
        // No battery, no network, no sensor (a VM): one row, none blank above.
        let vm = shipped(false, false, false, true, true, true);
        assert_eq!(where_is(&vm, "31-cpu"), Some(cells(0, 0, 1, 1)));
        assert_eq!(where_is(&vm, "32-memory"), Some(cells(1, 0, 1, 1)));
        assert_eq!(where_is(&vm, "33-system"), Some(cells(2, 0, 1, 1)));
        assert_eq!(vm.rows(&DATA, false), 1);
        // A hardware tile with nothing leaves no row of its own; the clock
        // under the list keeps the third.
        let network_only = shipped(false, true, false, false, false, false);
        assert_eq!(where_is(&network_only, "40-clock"), Some(cells(3, 2, 1, 1)));
        assert_eq!(network_only.rows(&DATA, false), 3);
        // Something above it in another column does not hold it.
        let mut beside = Area::default();
        beside
            .elements
            .push(placed("a", cells(1, 0, 1, 1), Refresh::Change, true, None));
        beside
            .elements
            .push(placed("b", cells(0, 1, 1, 1), Refresh::Change, true, None));
        assert_eq!(where_is(&beside, "b"), Some(cells(0, 0, 1, 1)));
        assert_eq!(beside.rows(&DATA, false), 1);
    }

    #[test]
    fn no_shown_combination_leaves_a_blank_row_or_an_overlap() {
        for bits in 0..64u8 {
            let on = |n: u8| bits & (1 << n) != 0;
            let area = shipped(on(0), on(1), on(2), on(3), on(4), on(5));
            let got: Vec<Cells> = area.shown(&DATA, false).map(|(_, c)| c).collect();
            for (i, a) in got.iter().enumerate() {
                assert!(got[i + 1..].iter().all(|b| !a.overlaps(b)), "{bits:#b}: {got:?}");
            }
            for row in 0..area.rows(&DATA, false) as u8 {
                assert!(
                    got.iter().any(|c| c.row <= row && (row as usize) < c.bottom()),
                    "{bits:#b}: row {row} blank in {got:?}"
                );
            }
        }
    }

    #[test]
    fn a_wide_element_stops_at_the_lowest_element_above_any_of_its_columns() {
        let area = Area {
            elements: vec![
                placed("gone", cells(0, 0, 1, 1), Refresh::Change, false, None),
                placed("a", cells(1, 0, 1, 2), Refresh::Change, true, None),
                placed("b", cells(0, 1, 1, 1), Refresh::Change, true, None),
                placed("c", cells(0, 2, 2, 1), Refresh::Change, true, None),
            ],
            ..Area::default()
        };
        assert_eq!(where_is(&area, "b"), Some(cells(0, 0, 1, 1)));
        assert_eq!(where_is(&area, "c"), Some(cells(0, 2, 2, 1)), "a reaches down to row 1");
        assert_eq!(area.rows(&DATA, false), 3);
        // A column of three with the top one gone closes up.
        let column = Area {
            elements: vec![
                placed("gone", cells(0, 0, 1, 1), Refresh::Change, false, None),
                placed("b", cells(0, 1, 1, 1), Refresh::Change, true, None),
                placed("c", cells(0, 2, 1, 1), Refresh::Change, true, None),
            ],
            ..Area::default()
        };
        assert_eq!(where_is(&column, "c"), Some(cells(0, 1, 1, 1)));
        assert_eq!(column.rows(&DATA, false), 2);
    }

    #[test]
    fn planned_rows_count_every_element_on_this_screen_shown_or_not() {
        assert_eq!(Area::default().planned_rows(false), 0, "no element");
        let mut area = Area {
            elements: vec![
                placed("a", cells(0, 0, 1, 1), Refresh::Change, false, None),
                placed("b", cells(1, 0, 2, 2), Refresh::Change, false, None),
            ],
            ..Area::default()
        };
        area.elements[1].short = Some(cells(1, 0, 3, 1));
        assert_eq!(area.planned_rows(false), 2, "nothing shows and it still counts");
        assert_eq!(area.planned_rows(true), 1, "the short form");
        let mut low = placed("c", cells(0, 2, 1, 1), Refresh::Change, true, None);
        low.short = None;
        area.elements.push(low);
        assert_eq!(area.planned_rows(false), 3);
        assert_eq!(area.planned_rows(true), 1, "ShortSize=none takes no row");
        for p in &mut area.elements {
            p.short = None;
        }
        assert_eq!(area.planned_rows(true), 0, "none on a short screen");
    }

    #[test]
    fn the_soonest_shown_element_sets_the_wake() {
        let now = Instant::now();
        let cell = vec2(220.0, 76.0);
        let s = Duration::from_secs;
        let area = Area {
            elements: vec![
                placed("a", cells(0, 0, 1, 1), Refresh::Change, true, Some(s(4))),
                placed("b", cells(1, 0, 2, 1), Refresh::Every(s(1)), false, None),
                placed("c", cells(3, 0, 1, 1), Refresh::Every(s(3)), true, None),
            ],
            ..Area::default()
        };
        assert_eq!(area.wake(&DATA, cell, 3.0, now, false), Some(s(3)));
        let calm = Area {
            elements: vec![placed("a", cells(0, 0, 1, 1), Refresh::Change, true, None)],
            ..Area::default()
        };
        assert_eq!(calm.wake(&DATA, cell, 3.0, now, false), None);
    }

    #[test]
    fn every_kind_has_a_unique_name() {
        let mut names: Vec<_> = KINDS.iter().map(|k| k.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), KINDS.len());
        assert!(kind("battery").is_some());
        assert!(kind("network").is_some());
        assert!(kind("network-info").is_some());
        assert!(kind("hardware").is_some());
        assert!(kind("clock").is_some());
        assert!(kind("radio").is_none());
    }
}
