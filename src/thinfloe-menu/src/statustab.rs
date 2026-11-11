//! The Status tab of Settings: which status elements show, at which size and
//! in which order. Every change is written at once (menu.rs starts the
//! write); the placement follows from `status::layout::pack`, so the tab only
//! says where an element got no cells. Drawing is in statustab_ui.rs.

use crate::status::layout::Layout;
use crate::status::{self, Area, Data, Size};

/// What the person did on the Status tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusEvent {
    /// The tick box of an element: show or hide it.
    Shown(String, bool),
    /// The size select of an element.
    Size(String, Size),
    /// Move the element one row up.
    Up(String),
    /// Move the element one row down.
    Down(String),
    /// Remove the layout file: the shipped list counts again.
    Reset,
}

/// Why a ticked element is not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// The packing found no free cells for it.
    NoRoom,
    /// It has nothing to show (no battery device, no sensor, no network).
    Nothing,
    /// It has no size for a short screen.
    OffShortScreen,
}

impl Note {
    pub fn text(self) -> &'static str {
        match self {
            Note::NoRoom => "no room",
            Note::Nothing => "nothing to show on this machine",
            Note::OffShortScreen => "not on a small screen",
        }
    }
}

/// One element as its row shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: &'static str,
    pub name: &'static str,
    pub shown: bool,
    pub size: Size,
    /// What the size select offers: the element's `sizes()`.
    pub sizes: &'static [Size],
    /// The top row: Up does nothing.
    pub first: bool,
    /// The bottom row: Down does nothing.
    pub last: bool,
    pub note: Option<Note>,
}

/// One row per element this image knows, in the file's order. Lines of
/// unknown ids are not listed. The notes are for the screen as it is now
/// (`short`), with the data the main screen draws from.
pub fn rows(area: &Area, data: &Data, short: bool) -> Vec<Row> {
    let placed: Vec<&str> = area.shown(data, short).iter().map(|(e, _, _)| e.id()).collect();
    let planned: Vec<&str> = area.planned(short).iter().map(|(e, _, _)| e.id()).collect();
    let listed: Vec<_> = area
        .layout
        .choices
        .iter()
        .filter(|c| c.known)
        .filter_map(|c| status::element(&c.id).map(|e| (c, e)))
        .collect();
    let n = listed.len();
    listed
        .into_iter()
        .enumerate()
        .map(|(i, (c, e))| {
            // Placement first, from the packing that does not depend on the
            // data; "nothing to show" only once its sources have read.
            let read = e.sources().iter().all(|s| data.has_read(*s));
            let note = if !c.shown || placed.contains(&e.id()) {
                None
            } else if short && e.short_size().is_none() {
                Some(Note::OffShortScreen)
            } else if e.has_content(data) || !planned.contains(&e.id()) {
                // Left out by the planned packing, or by the row limit of
                // the shown one (`Area::shown`).
                Some(Note::NoRoom)
            } else if read {
                Some(Note::Nothing)
            } else {
                None
            };
            Row {
                id: e.id(),
                name: e.name(),
                shown: c.shown,
                size: c.size,
                sizes: e.sizes(),
                first: i == 0,
                last: i + 1 == n,
                note,
            }
        })
        .collect()
}

/// Where in `layout.choices` the listed row of `id` is.
fn listed(layout: &Layout, id: &str) -> Option<usize> {
    layout
        .choices
        .iter()
        .position(|c| c.known && c.id == id && status::element(id).is_some())
}

/// The listed row next to the one at `at`, going `up` or down; unknown
/// lines in between are passed over and keep their place.
fn neighbour(layout: &Layout, at: usize, up: bool) -> Option<usize> {
    let is_listed = |i: &usize| {
        let c = &layout.choices[*i];
        c.known && status::element(&c.id).is_some()
    };
    if up {
        (0..at).rev().find(is_listed)
    } else {
        (at + 1..layout.choices.len()).find(is_listed)
    }
}

/// Change `layout` for `event`; false when nothing changed (an id that is not
/// listed, a size the element does not offer, Up on the top row, Down on the
/// bottom one). `Reset` is not a change of the list: menu.rs handles it.
pub fn apply(layout: &mut Layout, event: &StatusEvent) -> bool {
    match event {
        StatusEvent::Shown(id, on) => {
            let Some(i) = listed(layout, id) else { return false };
            let c = &mut layout.choices[i];
            let changed = c.shown != *on;
            c.shown = *on;
            changed
        }
        StatusEvent::Size(id, size) => {
            let Some(i) = listed(layout, id) else { return false };
            let offered = status::offered(id).is_some_and(|s| s.contains(size));
            let c = &mut layout.choices[i];
            let changed = offered && c.size != *size;
            if offered {
                c.size = *size;
            }
            changed
        }
        StatusEvent::Up(id) | StatusEvent::Down(id) => {
            let up = matches!(event, StatusEvent::Up(_));
            let Some(i) = listed(layout, id) else { return false };
            let Some(j) = neighbour(layout, i, up) else {
                return false;
            };
            layout.choices.swap(i, j);
            true
        }
        StatusEvent::Reset => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::layout;

    const fn sz(w: u8, h: u8) -> Size {
        Size { w, h }
    }

    fn area(text: &str) -> Area {
        let mut layout = layout::parse(text.as_bytes(), &status::offered);
        layout::complete(&mut layout, &status::descriptors());
        Area { layout }
    }

    fn ids(layout: &Layout) -> Vec<(String, bool, Size)> {
        layout.choices.iter().map(|c| (c.id.clone(), c.shown, c.size)).collect()
    }

    fn row_ids(rows: &[Row]) -> Vec<&'static str> {
        rows.iter().map(|r| r.id).collect()
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

    #[test]
    fn a_row_per_known_element_in_the_files_order_and_no_unknown_line() {
        let a = area("clock 1x1\nups 2x1\n-battery 2x1\n");
        let rows = rows(&a, &Data::EMPTY, false);
        let mut want = vec!["clock", "battery"];
        // The elements the file does not name follow, in their default order.
        want.extend(["network", "network-info", "temperatures", "cpu", "memory", "system"]);
        assert_eq!(row_ids(&rows), want);
        assert_eq!(rows.len(), status::ELEMENTS.len());
        let battery = &rows[1];
        assert_eq!((battery.shown, battery.size), (false, sz(2, 1)));
        assert_eq!(battery.name, status::element("battery").unwrap().name());
        assert_eq!(battery.sizes, status::element("battery").unwrap().sizes());
        assert!(rows[0].first && !rows[0].last);
        assert!(rows.last().unwrap().last && !rows[1].first && !rows[1].last);
    }

    #[test]
    fn a_tick_hides_and_shows_in_place() {
        let mut l = area("clock 1x1\nbattery 2x1\n").layout;
        let before = ids(&l);
        assert!(apply(&mut l, &StatusEvent::Shown("battery".into(), false)));
        assert_eq!(ids(&l)[1], ("battery".into(), false, sz(2, 1)));
        assert_eq!(ids(&l)[0], before[0]);
        assert_eq!(ids(&l)[2..], before[2..]);
        assert!(apply(&mut l, &StatusEvent::Shown("battery".into(), true)));
        assert_eq!(ids(&l), before);
        // The same state again is no change.
        assert!(!apply(&mut l, &StatusEvent::Shown("battery".into(), true)));
    }

    #[test]
    fn a_size_is_taken_only_when_the_element_offers_it() {
        let mut l = area("").layout;
        let offered = status::element("battery").unwrap().sizes();
        let other = *offered.last().unwrap();
        assert!(offered.len() > 1, "the battery comes in more than one size");
        assert!(apply(&mut l, &StatusEvent::Size("battery".into(), other)));
        assert_eq!(l.choices[0].size, other);
        let not_offered = sz(4, 3);
        assert!(!offered.contains(&not_offered));
        assert!(!apply(&mut l, &StatusEvent::Size("battery".into(), not_offered)));
        assert_eq!(l.choices[0].size, other);
        // An id that is not listed changes nothing.
        assert!(!apply(&mut l, &StatusEvent::Size("radio".into(), sz(1, 1))));
        assert!(!apply(&mut l, &StatusEvent::Shown("radio".into(), false)));
    }

    #[test]
    fn up_on_the_first_row_and_down_on_the_last_do_nothing() {
        let mut l = area("").layout;
        let before = ids(&l);
        assert!(!apply(&mut l, &StatusEvent::Up("battery".into())));
        assert!(!apply(&mut l, &StatusEvent::Down("clock".into())));
        assert_eq!(ids(&l), before);
        assert!(apply(&mut l, &StatusEvent::Down("battery".into())));
        assert_eq!(l.choices[0].id, "network");
        assert_eq!(l.choices[1].id, "battery");
        assert!(apply(&mut l, &StatusEvent::Up("battery".into())));
        assert_eq!(ids(&l), before);
    }

    #[test]
    fn unknown_lines_are_never_listed_and_survive_every_event() {
        let text = "ups 2x1\nclock 1x1\nradio 1x1\nbattery 1x1\n";
        let mut l = area(text).layout;
        let unknown = |l: &Layout| -> Vec<(usize, String)> {
            l.choices
                .iter()
                .enumerate()
                .filter(|(_, c)| !c.known)
                .map(|(i, c)| (i, c.id.clone()))
                .collect()
        };
        let before = unknown(&l);
        assert_eq!(before, [(0, "ups".to_string()), (2, "radio".to_string())]);
        let events = [
            StatusEvent::Shown("clock".into(), false),
            StatusEvent::Size(
                "battery".into(),
                *status::element("battery").unwrap().sizes().last().unwrap(),
            ),
            // Up and Down swap with the next listed row; the unknown line
            // between them stays where it is.
            StatusEvent::Up("battery".into()),
            StatusEvent::Down("battery".into()),
            StatusEvent::Up("clock".into()),
            StatusEvent::Up("ups".into()),
            StatusEvent::Shown("ups".into(), false),
        ];
        for e in &events {
            apply(&mut l, e);
            assert_eq!(unknown(&l), before, "after {e:?}");
            assert!(!row_ids(&rows(&Area { layout: l.clone() }, &Data::EMPTY, false)).contains(&"ups"));
        }
        // Up on battery put it before clock, across the unknown line.
        let mut l = area(text).layout;
        assert!(apply(&mut l, &StatusEvent::Up("battery".into())));
        let order: Vec<&str> = l.choices.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(order[..4], ["ups", "battery", "radio", "clock"]);
        // The top listed row has an unknown line above it: still the top.
        assert!(!apply(&mut l, &StatusEvent::Up("battery".into())));
        assert!(layout::to_text(&l).contains("ups 2x1\n"));
    }

    #[test]
    fn a_ticked_element_left_out_says_why() {
        let devices = device();
        let net = crate::status::network::sample(10);
        let hw = crate::status::hardware::sample();
        let data = Data {
            battery: &devices,
            network: Some(&net),
            hardware: Some(&hw),
            wall: Some(std::time::UNIX_EPOCH),
            ..Data::EMPTY
        };
        // The first six fill all twelve cells; system and clock get none.
        let a = area(
            "battery 2x1\nnetwork 2x2\nnetwork-info 1x2\ntemperatures 1x2\ncpu 1x1\nmemory 1x1\nsystem 2x1\nclock 1x1\n",
        );
        let note = |rows: &[Row], id: &str| rows.iter().find(|r| r.id == id).unwrap().note;
        let full = rows(&a, &data, false);
        for id in ["battery", "network", "network-info", "temperatures", "cpu", "memory"] {
            assert_eq!(note(&full, id), None, "{id}");
        }
        assert_eq!(note(&full, "system"), Some(Note::NoRoom));
        assert_eq!(note(&full, "clock"), Some(Note::NoRoom));
        // Without a network the graph and the list say so, and their cells
        // go to the elements behind them.
        let none = crate::netstat::Snapshot::default();
        let no_net = Data {
            network: Some(&none),
            ..data
        };
        let rows_no_net = rows(&a, &no_net, false);
        assert_eq!(note(&rows_no_net, "network"), Some(Note::Nothing));
        assert_eq!(note(&rows_no_net, "network-info"), Some(Note::Nothing));
        assert_eq!(note(&rows_no_net, "system"), None);
        assert_eq!(note(&rows_no_net, "clock"), None);
        // A hidden element has no note, though it would have no room either.
        let hidden = area(
            "battery 2x1\nnetwork 2x2\nnetwork-info 1x2\ntemperatures 1x2\ncpu 1x1\nmemory 1x1\n-system 2x1\n-clock 1x1\n",
        );
        assert_eq!(note(&rows(&hidden, &data, false), "system"), None);
        assert_eq!(Note::NoRoom.text(), "no room");
        assert_eq!(Note::Nothing.text(), "nothing to show on this machine");
    }

    #[test]
    fn on_a_short_screen_an_element_without_a_short_size_says_so() {
        let devices = device();
        let net = crate::status::network::sample(10);
        let data = Data {
            battery: &devices,
            network: Some(&net),
            ..Data::EMPTY
        };
        let a = area("");
        let note = |rows: &[Row], id: &str| rows.iter().find(|r| r.id == id).unwrap().note;
        // The hardware sampler rests on a short screen and has read nothing:
        // that is no reason to say "nothing to show".
        for id in ["network-info", "temperatures", "cpu", "memory", "system", "clock"] {
            assert_eq!(note(&rows(&a, &data, false), id), None, "{id} tall");
            assert_eq!(
                note(&rows(&a, &data, true), id),
                Some(Note::OffShortScreen),
                "{id} short"
            );
        }
        assert_eq!(note(&rows(&a, &data, true), "network"), None);
        assert_eq!(note(&rows(&a, &data, true), "battery"), None);
        assert_eq!(Note::OffShortScreen.text(), "not on a small screen");
    }

    #[test]
    fn nothing_to_show_only_once_the_source_has_read() {
        let a = area("");
        let note = |rows: &[Row], id: &str| rows.iter().find(|r| r.id == id).unwrap().note;
        // No source has read yet: no element says it has nothing.
        for short in [false, true] {
            for r in rows(&a, &Data::EMPTY, short) {
                assert_ne!(r.note, Some(Note::Nothing), "{} short {short}", r.id);
            }
        }
        // Every source read and found nothing.
        let net = crate::netstat::Snapshot::default();
        let hw = crate::hardware::Snapshot::default();
        let read = Data {
            battery_since: Some(std::time::Instant::now()),
            network: Some(&net),
            hardware: Some(&hw),
            ..Data::EMPTY
        };
        let tall = rows(&a, &read, false);
        for id in [
            "battery",
            "network",
            "network-info",
            "temperatures",
            "cpu",
            "memory",
            "system",
        ] {
            assert_eq!(note(&tall, id), Some(Note::Nothing), "{id}");
        }
        assert_eq!(note(&tall, "clock"), None);
        let short = rows(&a, &read, true);
        assert_eq!(note(&short, "battery"), Some(Note::Nothing));
        assert_eq!(note(&short, "network"), Some(Note::Nothing));
        assert_eq!(note(&short, "temperatures"), Some(Note::OffShortScreen));
    }

    #[test]
    fn no_room_comes_from_the_planned_packing_and_the_row_limit() {
        let note = |rows: &[Row], id: &str| rows.iter().find(|r| r.id == id).unwrap().note;
        // The planned packing gives system no cell; with no hardware read
        // yet it still says "no room", not nothing.
        let crowded = area(
            "battery 2x1\nnetwork 2x2\nnetwork-info 1x2\ntemperatures 1x2\ncpu 1x1\nmemory 1x1\nsystem 2x1\nclock 1x1\n",
        );
        assert_eq!(note(&rows(&crowded, &Data::EMPTY, false), "system"), Some(Note::NoRoom));
        // The final review's layout: with the battery read and empty, the
        // list would need a third row, which was never reserved.
        let a = area(
            "clock 1x1\nbattery 1x1\nnetwork 2x2\nnetwork-info 2x1\n-temperatures 1x1\n-cpu 1x1\n-memory 1x1\n-system 1x1\n",
        );
        let net = crate::status::network::sample(10);
        let data = Data {
            battery_since: Some(std::time::Instant::now()),
            network: Some(&net),
            wall: Some(std::time::UNIX_EPOCH),
            ..Data::EMPTY
        };
        let r = rows(&a, &data, false);
        assert_eq!(note(&r, "battery"), Some(Note::Nothing));
        assert_eq!(note(&r, "network"), None);
        assert_eq!(note(&r, "network-info"), Some(Note::NoRoom));
        assert_eq!(note(&r, "clock"), None);
    }
}
