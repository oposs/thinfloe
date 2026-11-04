//! Where the status elements come from: one file per element in the
//! shipped directory `/etc/thinfloe/status/`, read like the tile files
//! (same bounded reader, same key=value lines, group `[Status Element]`):
//!
//! ```text
//! [Status Element]
//! Type=battery
//! Cell=0,0
//! Size=1x1
//! Refresh=change
//! ```
//!
//! `Type` names an element type (`KINDS`), `Cell` the column and row of its
//! top-left cell in the 4 x 3 grid, `Size` its width and height in cells
//! (default `1x1`), `Refresh` is `change` (the default) or a number of
//! seconds from 1 to 3600. `ShortSize` is its width and height on a short
//! screen (`ui::SHORT_SCREEN_H`), from the same top-left cell and inside the
//! grid, or `none`: the element is not on a short screen at all. Short forms
//! must not overlap each other. Without the key the element keeps `Size`.
//! Further keys are the type's own options
//! (`PageEvery` for the battery, `Show` for hardware); any other key refuses the file. The file
//! name orders the elements and names them in the log.
//!
//! There are no user files yet (owner, 2026-11-03). Elements are taken in
//! file-name order; on overlap the lower name wins and the other is refused.
//! Every refusal is a warning in the menu's log naming the file.

use super::{Area, Cells, Placed, Refresh, kind};
use crate::desktop::{self, Entry};
use crate::tiles::{SourceFile, read_entries};
use std::path::Path;
use std::time::Duration;

pub const SHIPPED_DIR: &str = "/etc/thinfloe/status";
pub const HEADER: &str = "[Status Element]";
pub const EXT: &str = ".conf";
/// At most this many files; the grid has 12 cells.
pub const MAX_FILES: usize = 16;
pub const MAX_REFRESH_SECS: u64 = 3600;

const COMMON_KEYS: &[&str] = &["Type", "Cell", "Size", "ShortSize", "Refresh"];

pub fn parse(text: &str) -> Result<Entry, String> {
    desktop::parse_group(text, HEADER)
}

/// A number of seconds from 1 to `max`, with or without a trailing `s`.
pub fn seconds(v: &str, max: u64) -> Option<Duration> {
    v.strip_suffix('s')
        .unwrap_or(v)
        .parse::<u64>()
        .ok()
        .filter(|s| (1..=max).contains(s) && v.starts_with(|c: char| c.is_ascii_digit()))
        .map(Duration::from_secs)
}

fn refresh(value: Option<&str>) -> Result<Refresh, String> {
    match value {
        None | Some("change") => Ok(Refresh::Change),
        Some(v) => seconds(v, MAX_REFRESH_SECS)
            .map(Refresh::Every)
            .ok_or_else(|| format!("Refresh must be 'change' or 1 to {MAX_REFRESH_SECS} seconds, not '{v}'")),
    }
}

/// `Cell=col,row` and `Size=WxH` (default 1x1) as one rectangle.
fn cells(entry: &Entry) -> Result<Cells, String> {
    let cell = entry.get("Cell").ok_or("no Cell")?;
    let (col, row) = cell
        .split_once(',')
        .ok_or_else(|| format!("Cell must be column,row, not '{cell}'"))?;
    let size = entry.get("Size").unwrap_or("1x1");
    let (w, h) = size
        .split_once('x')
        .ok_or_else(|| format!("Size must be WIDTHxHEIGHT, not '{size}'"))?;
    Cells::parse(&format!("{},{},{},{}", col.trim(), row.trim(), w.trim(), h.trim())).map_err(|_| {
        format!(
            "Cell {cell} with Size {size} is not inside the {} x {} grid",
            super::COLUMNS,
            super::ROWS
        )
    })
}

/// `ShortSize=` as the cells on a short screen: `WxH` from the same corner and
/// inside the grid, `none` for no cells at all, `cells` without the key.
fn short_cells(entry: &Entry, cells: Cells) -> Result<Option<Cells>, String> {
    let Some(short) = entry.get("ShortSize") else {
        return Ok(Some(cells));
    };
    if short == "none" {
        return Ok(None);
    }
    let bad = || format!("ShortSize must be WIDTHxHEIGHT of at least 1x1, or none, not '{short}'");
    let number = |n: &str| {
        n.parse::<u8>()
            .ok()
            .filter(|v| *v > 0 && n.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(bad)
    };
    let (w, h) = short.split_once('x').ok_or_else(bad)?;
    let (w, h) = (number(w.trim())?, number(h.trim())?);
    let form = Cells { w, h, ..cells };
    if form.col as u16 + w as u16 > super::COLUMNS as u16 || form.row as u16 + h as u16 > super::ROWS as u16 {
        return Err(format!(
            "ShortSize {short} at Cell {},{} is not inside the {} x {} grid",
            cells.col,
            cells.row,
            super::COLUMNS,
            super::ROWS
        ));
    }
    Ok(Some(form))
}

/// The line `--check` prints for an element.
pub fn check_line(p: &Placed) -> String {
    let c = p.cells;
    let short = match p.short {
        Some(s) if s == c => String::new(),
        Some(s) => format!(" short {}x{}", s.w, s.h),
        None => " short none".to_string(),
    };
    format!(
        "status {}: {} at {},{} size {}x{}{short}",
        p.name, p.kind, c.col, c.row, c.w, c.h
    )
}

/// The element a file describes, or why it is refused.
pub fn element(name: &str, entry: &Entry) -> Result<Placed, String> {
    let type_name = entry.get("Type").ok_or("no Type")?;
    let kind = kind(type_name).ok_or_else(|| format!("unknown type '{type_name}'"))?;
    if let Some(key) = entry
        .keys
        .keys()
        .find(|k| !COMMON_KEYS.contains(&k.as_str()) && !kind.options.contains(&k.as_str()))
    {
        return Err(format!("unknown key '{key}' for type {type_name}"));
    }
    let cells = cells(entry)?;
    let short = short_cells(entry, cells)?;
    let refresh = refresh(entry.get("Refresh"))?;
    let element = (kind.build)(entry)?;
    Ok(Placed {
        name: name.to_string(),
        kind: kind.name,
        cells,
        short,
        refresh,
        element,
    })
}

/// Build the area from the files, in file-name order.
pub fn assemble(files: &[SourceFile]) -> Area {
    let mut area = Area::default();
    let mut files: Vec<&SourceFile> = files.iter().collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));
    for f in files {
        let placed = match f.entry.clone().and_then(|e| element(&f.name, &e)) {
            Ok(p) => p,
            Err(why) => {
                area.notes.push(format!("status: {} refused: {why}", f.name));
                continue;
            }
        };
        // Either form overlapping is a refusal: both forms are drawn, one
        // on a short screen and one on any other.
        let short_hit = |o: &&Placed| matches!((o.short, placed.short), (Some(a), Some(b)) if a.overlaps(&b));
        if let Some(other) = area
            .elements
            .iter()
            .find(|o| o.cells.overlaps(&placed.cells) || short_hit(o))
        {
            area.notes
                .push(format!("status: {} refused: overlaps {}", f.name, other.name));
            continue;
        }
        area.elements.push(placed);
    }
    area.shipped_refused = !area.notes.is_empty();
    area
}

/// Read the directory and build the area.
pub fn load(dir: &Path) -> Area {
    let read = read_entries(dir, MAX_FILES, "/etc/thinfloe/status", EXT, "status", parse);
    let mut area = assemble(&read.files);
    area.shipped_refused |= !read.notes.is_empty();
    let mut notes: Vec<String> = read.notes.iter().map(|n| format!("status: {n}")).collect();
    notes.append(&mut area.notes);
    area.notes = notes;
    area
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::fs;

    fn src(name: &str, text: &str) -> SourceFile {
        SourceFile {
            name: name.into(),
            entry: parse(text),
        }
    }

    fn file(t: &str, cell: &str, size: &str) -> String {
        format!("[Status Element]\nType={t}\nCell={cell}\nSize={size}\n")
    }

    fn names(a: &Area) -> Vec<(&str, Cells)> {
        a.elements.iter().map(|p| (p.name.as_str(), p.cells)).collect()
    }

    fn c(text: &str) -> Cells {
        Cells::parse(text).unwrap()
    }

    fn one(text: &str) -> Result<Placed, String> {
        element("10-x.conf", &parse(text)?)
    }

    #[test]
    fn the_shipped_files_are_battery_graph_list_four_hardware_tiles_and_the_clock() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../overlay/etc/thinfloe/status");
        let a = load(&dir);
        assert!(a.notes.is_empty(), "{:?}", a.notes);
        assert!(!a.shipped_refused);
        assert_eq!(a.elements.len(), 8);
        let p = &a.elements[0];
        assert_eq!(p.name, "10-battery.conf");
        assert_eq!(p.kind, "battery");
        assert_eq!(p.cells, c("0,0,1,1"));
        assert_eq!(p.refresh, Refresh::Change);
        let n = &a.elements[1];
        assert_eq!(n.name, "20-network.conf");
        assert_eq!(n.kind, "network");
        assert_eq!(n.cells, c("1,0,2,2"), "2 x 2 right of the battery");
        assert_eq!(n.short, Some(c("1,0,3,1")), "one row on a short screen");
        assert_eq!(p.short, Some(p.cells), "the battery keeps its size");
        assert_eq!(n.refresh, Refresh::Every(Duration::from_secs(1)));
        let l = &a.elements[2];
        assert_eq!(l.name, "25-network-info.conf");
        assert_eq!(l.kind, "network-info");
        assert_eq!(l.cells, c("3,0,1,2"), "1 x 2 right of the graph");
        assert_eq!(l.short, None, "not on a short screen");
        assert_eq!(l.refresh, Refresh::Every(Duration::from_secs(1)));
        // The hardware: four tiles, none on a short screen.
        for (i, name, cells) in [
            (3, "30-temperatures.conf", "0,1,1,1"),
            (4, "31-cpu.conf", "0,2,1,1"),
            (5, "32-memory.conf", "1,2,1,1"),
            (6, "33-system.conf", "2,2,1,1"),
        ] {
            let h = &a.elements[i];
            assert_eq!(h.name, name);
            assert_eq!(h.kind, "hardware", "{name}");
            assert_eq!(h.cells, c(cells), "{name}");
            assert_eq!(h.short, None, "{name}: not on a short screen");
            assert_eq!(h.refresh, Refresh::Every(Duration::from_secs(5)), "{name}");
        }
        // As test-session-packages.sh expects them from `--check`.
        assert_eq!(
            check_line(&a.elements[6]),
            "status 33-system.conf: hardware at 2,2 size 1x1 short none"
        );
        assert_eq!(
            check_line(&a.elements[7]),
            "status 40-clock.conf: clock at 3,2 size 1x1 short none"
        );
        assert_eq!(a.elements[7].refresh, Refresh::Change);
        assert_eq!(a.planned_rows(false), 3);
        assert_eq!(a.planned_rows(true), 1);
        for (i, x) in a.elements.iter().enumerate() {
            for y in &a.elements[i + 1..] {
                assert!(!x.cells.overlaps(&y.cells), "{} and {}", x.name, y.name);
            }
        }
    }

    #[test]
    fn short_size_is_a_form_from_the_same_corner_inside_the_grid() {
        let with =
            |cell: &str, size: &str, short: &str| one(&(file("network", cell, size) + &format!("ShortSize={short}\n")));
        let p = with("1,0", "3x2", "3x1").unwrap();
        assert_eq!((p.cells, p.short), (c("1,0,3,2"), Some(c("1,0,3,1"))));
        let p = one(&file("network", "1,0", "3x2")).unwrap();
        assert_eq!(p.short, Some(p.cells), "without the key: the same cells");
        // Wider than Size is fine: the graph goes 2x2 to 3x1.
        let p = with("1,0", "2x2", "3x1").unwrap();
        assert_eq!((p.cells, p.short), (c("1,0,2,2"), Some(c("1,0,3,1"))));
        // It only has to stay inside the grid.
        for (cell, size, short) in [("1,0", "2x2", "4x1"), ("0,2", "1x1", "1x2"), ("3,0", "1x1", "2x1")] {
            assert_eq!(
                with(cell, size, short).err().unwrap(),
                format!("ShortSize {short} at Cell {cell} is not inside the 4 x 3 grid")
            );
        }
        for bad in ["3", "3x", "0x1", "3x0", "-1x1", "axb", "300x1", "None", "none "] {
            let text = file("network", "1,0", "3x2") + &format!("ShortSize={bad}\n");
            // The reader trims values; what it leaves is judged.
            let shown = bad.trim();
            if shown == "none" {
                continue;
            }
            assert_eq!(
                one(&text).err().unwrap(),
                format!("ShortSize must be WIDTHxHEIGHT of at least 1x1, or none, not '{shown}'")
            );
        }
    }

    #[test]
    fn short_size_none_leaves_the_element_out_of_a_short_screen() {
        let p = one(&(file("battery", "0,1", "1x1") + "ShortSize=none\n")).unwrap();
        assert_eq!(p.cells, c("0,1,1,1"));
        assert_eq!(p.short, None);
        assert_eq!(p.cells_on(true), None);
        assert_eq!(p.cells_on(false), Some(p.cells));
        assert_eq!(check_line(&p), "status 10-x.conf: battery at 0,1 size 1x1 short none");
        let p = one(&(file("network", "1,0", "2x2") + "ShortSize=3x1\n")).unwrap();
        assert_eq!(check_line(&p), "status 10-x.conf: network at 1,0 size 2x2 short 3x1");
        let p = one(&file("battery", "0,0", "1x1")).unwrap();
        assert_eq!(check_line(&p), "status 10-x.conf: battery at 0,0 size 1x1");
    }

    #[test]
    fn short_forms_must_not_overlap_the_lower_name_wins() {
        let a = assemble(&[
            src("20-b.conf", &(file("battery", "3,0", "1x1"))),
            src("10-a.conf", &(file("network", "1,0", "2x2") + "ShortSize=3x1\n")),
            src("30-c.conf", &(file("battery", "3,1", "1x1") + "ShortSize=none\n")),
        ]);
        assert_eq!(
            names(&a),
            vec![("10-a.conf", c("1,0,2,2")), ("30-c.conf", c("3,1,1,1"))]
        );
        assert_eq!(a.notes, vec!["status: 20-b.conf refused: overlaps 10-a.conf"]);
        assert!(a.shipped_refused, "--check fails on it");
        // A form that is not there overlaps nothing.
        let a = assemble(&[
            src("10-a.conf", &(file("network", "1,0", "2x2") + "ShortSize=3x1\n")),
            src("20-b.conf", &(file("battery", "3,0", "1x2") + "ShortSize=none\n")),
        ]);
        assert!(a.notes.is_empty(), "{:?}", a.notes);
        assert_eq!(a.elements.len(), 2);
    }

    #[test]
    fn the_network_type_takes_no_option() {
        assert!(one(&file("network", "1,0", "3x1")).is_ok());
        assert_eq!(
            one(&(file("network", "1,0", "3x1") + "PageEvery=4\n")).err().unwrap(),
            "unknown key 'PageEvery' for type network"
        );
    }

    #[test]
    fn a_file_needs_type_and_cell_and_takes_no_stray_key() {
        let p = one(&file("battery", "1,2", "2x1")).unwrap();
        assert_eq!((p.kind, p.cells), ("battery", c("1,2,2,1")));
        let p = one("[Status Element]\nType=battery\nCell=3,1\n").unwrap();
        assert_eq!(p.cells, c("3,1,1,1"), "Size defaults to 1x1");
        assert!(one(&(file("battery", "0,0", "1x1") + "PageEvery=2\n")).is_ok());
        assert_eq!(one("[Status Element]\nCell=0,0\n").err().unwrap(), "no Type");
        assert_eq!(one("[Status Element]\nType=battery\n").err().unwrap(), "no Cell");
        assert_eq!(one(&file("radio", "0,0", "1x1")).err().unwrap(), "unknown type 'radio'");
        assert_eq!(
            one(&(file("battery", "0,0", "1x1") + "Colour=red\n")).err().unwrap(),
            "unknown key 'Colour' for type battery"
        );
        assert_eq!(
            one(&file("battery", "3,0", "2x1")).err().unwrap(),
            "Cell 3,0 with Size 2x1 is not inside the 4 x 3 grid"
        );
        assert_eq!(
            one(&file("battery", "0,0", "0x1")).err().unwrap(),
            "Cell 0,0 with Size 0x1 is not inside the 4 x 3 grid"
        );
        assert_eq!(
            one(&file("battery", "0", "1x1")).err().unwrap(),
            "Cell must be column,row, not '0'"
        );
        assert_eq!(
            one(&file("battery", "0,0", "1,1")).err().unwrap(),
            "Size must be WIDTHxHEIGHT, not '1,1'"
        );
        assert_eq!(
            one(&(file("battery", "0,0", "1x1") + "PageEvery=0\n")).err().unwrap(),
            "PageEvery must be 1 to 60 seconds, not '0'"
        );
        assert_eq!(
            one("Type=battery\n").err().unwrap(),
            "line 1 comes before [Status Element]"
        );
    }

    #[test]
    fn refresh_is_change_or_seconds() {
        let r = |v: &str| one(&(file("battery", "0,0", "1x1") + &format!("Refresh={v}\n"))).map(|p| p.refresh);
        assert_eq!(r("change"), Ok(Refresh::Change));
        assert_eq!(r("1"), Ok(Refresh::Every(Duration::from_secs(1))));
        assert_eq!(r("5s"), Ok(Refresh::Every(Duration::from_secs(5))));
        assert_eq!(r("3600"), Ok(Refresh::Every(Duration::from_secs(3600))));
        for bad in ["0", "3601", "-1", "+5", "1.5", "s", "often", ""] {
            assert_eq!(
                r(bad).err().unwrap(),
                format!("Refresh must be 'change' or 1 to 3600 seconds, not '{bad}'"),
                "{bad}"
            );
        }
    }

    #[test]
    fn on_overlap_the_lower_name_wins() {
        let a = assemble(&[
            src("30-free.conf", &file("battery", "2,0", "2x2")),
            src("20-network.conf", &file("battery", "1,0", "1x1")),
            src("10-battery.conf", &file("battery", "0,0", "2x1")),
        ]);
        assert_eq!(
            names(&a),
            vec![("10-battery.conf", c("0,0,2,1")), ("30-free.conf", c("2,0,2,2"))]
        );
        assert_eq!(
            a.notes,
            vec!["status: 20-network.conf refused: overlaps 10-battery.conf"]
        );
        assert!(a.shipped_refused);
    }

    #[test]
    fn a_bad_file_is_logged_by_name_never_a_crash() {
        let a = assemble(&[
            src("10-a.conf", "garbage"),
            src("20-b.conf", &file("battery", "0,0", "1x1")),
        ]);
        assert_eq!(names(&a).len(), 1);
        assert_eq!(
            a.notes,
            vec!["status: 10-a.conf refused: line 1 is not a desktop entry line"]
        );
    }

    #[test]
    fn a_directory_that_cannot_be_used_fails_the_check() {
        let d = TestDir::new("status", "refused");
        let not_dir = d.join("status");
        fs::write(&not_dir, "a file").unwrap();
        let a = load(&not_dir);
        assert_eq!(
            a.notes,
            vec!["status: /etc/thinfloe/status is not a directory, so it was ignored"]
        );
        assert!(a.shipped_refused, "{:?}", a.notes);
        // A missing directory means no element, which is no defect.
        assert!(!load(&d.join("none")).shipped_refused);
    }

    #[test]
    fn load_reads_conf_files_only_and_bounded() {
        let d = TestDir::new("status", "load");
        let dir = d.join("status");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("10-battery.conf"), file("battery", "0,0", "1x1")).unwrap();
        fs::write(dir.join("10-battery.conf~"), "junk").unwrap();
        fs::write(dir.join("README"), "junk").unwrap();
        assert_eq!(names(&load(&dir)).len(), 1);
        for i in 0..20 {
            fs::write(dir.join(format!("{:02}-x.conf", i + 20)), "junk").unwrap();
        }
        let a = load(&dir);
        assert_eq!(names(&a).len(), 1);
        assert_eq!(
            a.notes[0],
            "status: /etc/thinfloe/status has 21 status files; only the first 16 are shown"
        );
        let missing = load(&d.join("none"));
        assert!(missing.elements.is_empty() && missing.notes.is_empty());
    }
}
