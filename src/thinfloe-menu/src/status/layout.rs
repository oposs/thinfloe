//! The per-machine status layout file (`~/.config/thinfloe/status`) and the
//! packing of element sizes into the grid. Pure: no file access here.
//!
//! One element per line: `<id> <w>x<h>`, or `-<id> <w>x<h>` when hidden;
//! `#` starts a comment. Ids this image does not know are kept, so a newer
//! image finds them again after this one rewrote the file.

use super::{COLUMNS, Cells, ROWS, Size};
use std::collections::HashSet;

const MAX_BYTES: usize = 4096;
const MAX_LINES: usize = 64;
const HEADER: &str = "# thinfloe status area, written by the menu";

/// An element as `default_layout` and `complete` see it: id and default size.
pub type Descriptor = (&'static str, Size);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub size: Size,
    pub shown: bool,
    /// False for an id this image does not know: written back, never drawn.
    pub known: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Layout {
    pub choices: Vec<Choice>,
    /// One English sentence per line that was skipped or changed.
    pub notes: Vec<String>,
    /// The file was not UTF-8, too long or too big. `choices` is then empty;
    /// `complete` makes it the default list.
    pub refused: bool,
}

fn refuse(why: &str) -> Layout {
    Layout {
        choices: Vec::new(),
        notes: vec![format!("status layout file ignored: {why}")],
        refused: true,
    }
}

/// Parse the file. `known` returns the sizes an id offers (first = default),
/// or None for an unknown id. A missing or empty file gives no choices; call
/// `complete` to get the default list.
pub fn parse(text: &[u8], known: &dyn Fn(&str) -> Option<&'static [Size]>) -> Layout {
    if text.len() > MAX_BYTES {
        return refuse("it is over 4 KiB");
    }
    let Ok(text) = std::str::from_utf8(text) else {
        return refuse("it is not UTF-8 text");
    };
    if text.lines().count() > MAX_LINES {
        return refuse("it has more than 64 lines");
    }
    let mut out = Layout::default();
    let mut seen = HashSet::new();
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.split('#').next().unwrap_or("");
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        let (shown, id) = match parts[0].strip_prefix('-') {
            Some(id) => (false, id),
            None => (true, parts[0]),
        };
        let size = if parts.len() == 2 { Size::parse(parts[1]) } else { None };
        let (Some(size), false) = (size, id.is_empty()) else {
            out.notes.push(format!(
                "status line {n} skipped: expected '<id> <w>x<h>', got '{}'",
                line.trim()
            ));
            continue;
        };
        if !seen.insert(id.to_string()) {
            out.notes
                .push(format!("status line {n} skipped: '{id}' is already listed"));
            continue;
        }
        match known(id) {
            None => {
                out.notes
                    .push(format!("status line {n}: unknown element '{id}' kept but not shown"));
                out.choices.push(Choice {
                    id: id.into(),
                    size,
                    shown,
                    known: false,
                });
            }
            Some(offered) => {
                let size = if offered.contains(&size) {
                    size
                } else {
                    let d = offered.first().copied().unwrap_or(size);
                    out.notes
                        .push(format!("status line {n}: '{id}' does not come in {size}, using {d}"));
                    d
                };
                out.choices.push(Choice {
                    id: id.into(),
                    size,
                    shown,
                    known: true,
                });
            }
        }
    }
    out
}

fn choice_of((id, size): &Descriptor) -> Choice {
    Choice {
        id: (*id).into(),
        size: *size,
        shown: true,
        known: true,
    }
}

/// Every element, shown, at its default size.
pub fn default_layout(elements: &[Descriptor]) -> Layout {
    Layout {
        choices: elements.iter().map(choice_of).collect(),
        ..Layout::default()
    }
}

/// Append the elements the file does not name: shown, default size.
pub fn complete(layout: &mut Layout, elements: &[Descriptor]) {
    for e in elements {
        if !layout.choices.iter().any(|c| c.id == e.0) {
            layout.choices.push(choice_of(e));
        }
    }
}

pub fn to_text(layout: &Layout) -> String {
    let mut s = format!("{HEADER}\n");
    for c in &layout.choices {
        s += &format!("{}{} {}\n", if c.shown { "" } else { "-" }, c.id, c.size);
    }
    s
}

/// First fit, row by row. One result per item; `None` = left out. On a short
/// screen there is one row and an item uses its entry of `short_sizes`
/// (`None` there: left out).
pub fn pack(items: &[Size], short: bool, short_sizes: &[Option<Size>]) -> Vec<Option<Cells>> {
    let rows = if short { 1 } else { ROWS };
    let mut used = [[false; COLUMNS as usize]; ROWS as usize];
    items
        .iter()
        .enumerate()
        .map(|(i, &size)| {
            let size = if short {
                short_sizes.get(i).copied().flatten()?
            } else {
                size
            };
            for row in 0..rows {
                for col in 0..COLUMNS {
                    if col + size.w > COLUMNS || row + size.h > rows {
                        continue;
                    }
                    let cells = Cells {
                        col,
                        row,
                        w: size.w,
                        h: size.h,
                    };
                    let free = (row..row + size.h).all(|r| (col..col + size.w).all(|c| !used[r as usize][c as usize]));
                    if free {
                        for r in row..row + size.h {
                            for c in col..col + size.w {
                                used[r as usize][c as usize] = true;
                            }
                        }
                        return Some(cells);
                    }
                }
            }
            None
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn sz(w: u8, h: u8) -> Size {
        Size { w, h }
    }
    static ALL: [Size; 3] = [sz(1, 1), sz(2, 1), sz(3, 1)];
    fn known(id: &str) -> Option<&'static [Size]> {
        matches!(id, "battery" | "clock" | "net").then_some(&ALL[..])
    }
    fn p(t: &str) -> Layout {
        parse(t.as_bytes(), &known)
    }
    fn cells(v: &[Option<Cells>]) -> Vec<(u8, u8, u8, u8)> {
        v.iter()
            .map(|c| c.map_or((9, 9, 9, 9), |c| (c.col, c.row, c.w, c.h)))
            .collect()
    }
    const ELEMS: [Descriptor; 2] = [("battery", sz(1, 1)), ("clock", sz(1, 1))];

    #[test]
    fn the_default_list_packs_to_the_pr6_layout() {
        let items = [
            sz(1, 1),
            sz(2, 2),
            sz(1, 2),
            sz(1, 1),
            sz(1, 1),
            sz(1, 1),
            sz(1, 1),
            sz(1, 1),
        ];
        assert_eq!(
            cells(&pack(&items, false, &[])),
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
    }

    #[test]
    fn packing_is_first_fit_row_by_row() {
        let r = pack(&[sz(2, 1), sz(3, 1), sz(2, 1)], false, &[]);
        assert_eq!(cells(&r), [(0, 0, 2, 1), (0, 1, 3, 1), (2, 0, 2, 1)]);
    }

    #[test]
    fn an_item_that_fits_nowhere_is_left_out() {
        let r = pack(&[sz(4, 3), sz(1, 1)], false, &[]);
        assert_eq!(
            r[0],
            Some(Cells {
                col: 0,
                row: 0,
                w: 4,
                h: 3
            })
        );
        assert_eq!(r[1], None);
    }

    #[test]
    fn a_short_screen_has_one_row() {
        let shorts = [Some(sz(1, 1)), Some(sz(3, 1)), None];
        let r = pack(&[sz(1, 1), sz(2, 2), sz(1, 2)], true, &shorts);
        assert_eq!(
            r[0],
            Some(Cells {
                col: 0,
                row: 0,
                w: 1,
                h: 1
            })
        );
        assert_eq!(
            r[1],
            Some(Cells {
                col: 1,
                row: 0,
                w: 3,
                h: 1
            })
        );
        assert_eq!(r[2], None);
    }

    #[test]
    fn missing_or_empty_file_is_the_default() {
        for t in ["", "# nothing\n\n"] {
            let mut l = p(t);
            assert!(!l.refused && l.choices.is_empty());
            complete(&mut l, &ELEMS);
            assert_eq!(l.choices, default_layout(&ELEMS).choices);
        }
    }

    #[test]
    fn hidden_lines_keep_place_and_size() {
        let l = p("battery 1x1\n-clock 2x1\n");
        assert!(!l.choices[1].shown && l.choices[1].size == sz(2, 1));
        assert_eq!(p(&to_text(&l)), l);
    }

    #[test]
    fn layout_lines_tolerate_hand_editing() {
        let l = p("battery\t2x1  \r\n\r\n  clock 1x1 # note\r\n+1x1\nnet 1x1x1\nbattery +1x1\n");
        let ids: Vec<_> = l.choices.iter().map(|c| (c.id.as_str(), c.size)).collect();
        assert_eq!(ids, [("battery", sz(2, 1)), ("clock", sz(1, 1))]);
        assert_eq!(l.notes.len(), 3);
        assert!(l.notes[0].contains("line 4"));
    }

    #[test]
    fn unknown_lines_survive_a_rewrite() {
        let l = p("battery 1x1\nups 2x1\nclock 1x1\n");
        assert!(!l.choices[1].known);
        assert_eq!(to_text(&l), format!("{HEADER}\nbattery 1x1\nups 2x1\nclock 1x1\n"));
    }

    #[test]
    fn a_size_not_offered_falls_back_to_the_default() {
        let l = p("clock 4x3\n");
        assert_eq!(l.choices[0].size, sz(1, 1));
        assert_eq!(l.notes.len(), 1);
    }

    #[test]
    fn a_second_line_for_an_id_is_ignored() {
        let l = p("clock 2x1\nclock 1x1\n");
        assert_eq!(l.choices.len(), 1);
        assert_eq!(l.choices[0].size, sz(2, 1));
        assert_eq!(l.notes.len(), 1);
    }

    #[test]
    fn an_oversized_or_binary_file_is_refused() {
        let big = "#".repeat(4097);
        let many = "clock 1x1\n".repeat(65);
        for bytes in [big.as_bytes(), many.as_bytes(), &[0xff, 0xfe, b'\n'][..]] {
            let mut l = parse(bytes, &known);
            assert!(l.refused && l.notes.len() == 1);
            complete(&mut l, &ELEMS);
            assert_eq!(l.choices, default_layout(&ELEMS).choices);
        }
    }

    #[test]
    fn new_elements_are_appended_shown() {
        let mut l = p("-clock 2x1\n");
        complete(&mut l, &ELEMS);
        let c = &l.choices[1];
        assert_eq!((c.id.as_str(), c.size, c.shown), ("battery", sz(1, 1), true));
    }
}
