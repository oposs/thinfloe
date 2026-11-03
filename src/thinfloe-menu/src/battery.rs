//! The battery file written by the root helper every few minutes. The helper
//! is trusted, the file is parsed as if it were not: bounded read, no
//! symlinks, malformed lines dropped, names made printable.

use std::path::Path;
use std::time::Duration;

pub const FILE: &str = "/run/thinfloe/peripherals/battery";
pub const MAX_BYTES: u64 = 16 * 1024;
pub const MAX_LINES: usize = 64;
pub const MAX_NAME: usize = 64;
/// A reachable device below this percent is drawn red and logged as a warning.
pub const LOW_BELOW: u8 = 10;
pub const CHARGING_MARK: &str = "+";
/// How long the tile shows one page of lines when not all fit.
pub const PAGE_EVERY: Duration = Duration::from_secs(4);

const NOT_REACHABLE: &str = "not reachable";

/// What a device is, as the reader learned it (the receiver's pairing info).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeviceKind {
    Keyboard,
    Mouse,
    Numpad,
    Presenter,
    Trackball,
    Touchpad,
    #[default]
    Unknown,
}

impl DeviceKind {
    /// The word of the file's fifth field; anything else is unknown.
    pub fn parse(word: &str) -> DeviceKind {
        match word {
            "keyboard" => DeviceKind::Keyboard,
            "mouse" => DeviceKind::Mouse,
            "numpad" => DeviceKind::Numpad,
            "presenter" => DeviceKind::Presenter,
            "trackball" => DeviceKind::Trackball,
            "touchpad" => DeviceKind::Touchpad,
            _ => DeviceKind::Unknown,
        }
    }

    pub fn label(self) -> Option<&'static str> {
        match self {
            DeviceKind::Keyboard => Some("Keyboard"),
            DeviceKind::Mouse => Some("Mouse"),
            DeviceKind::Numpad => Some("Numpad"),
            DeviceKind::Presenter => Some("Presenter"),
            DeviceKind::Trackball => Some("Trackball"),
            DeviceKind::Touchpad => Some("Touchpad"),
            DeviceKind::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub percent: Option<u8>,
    pub state: String,
    pub read_epoch: u64,
    pub kind: DeviceKind,
}

impl Device {
    pub fn reachable(&self) -> bool {
        self.state != NOT_REACHABLE
    }

    /// Below `LOW_BELOW` percent or critical, and reachable: a stale
    /// percent says nothing about now.
    pub fn low(&self) -> bool {
        self.reachable() && (self.state == "critical" || self.percent.is_some_and(|p| p < LOW_BELOW))
    }
}

/// Make a device name printable: NUL dropped, control, format and separator
/// characters shown as `?`, cut to `MAX_NAME` characters.
pub fn sanitise(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|&c| c != '\0')
        .map(|c| {
            let bad = c.is_control()
                || matches!(c, '\u{200B}'..='\u{200F}'
                    | '\u{202A}'..='\u{202E}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{2060}'..='\u{2064}'
                    | '\u{2066}'..='\u{2069}'
                    | '\u{FEFF}'
                    | '\u{061C}'
                    | '\u{00AD}'
                    | '\u{180E}'
                    | '\u{206A}'..='\u{206F}'
                    | '\u{FFF9}'..='\u{FFFB}'
                    | '\u{E0000}'..='\u{E007F}');
            if bad { '?' } else { c }
        })
        .collect();
    let cut: String = cleaned.trim().chars().take(MAX_NAME).collect();
    if cut.is_empty() {
        "unknown device".to_string()
    } else {
        cut
    }
}

fn parse_line(line: &str) -> Option<Device> {
    let f: Vec<&str> = line.split('\t').collect();
    // Four fields come from a reader of before the kind; the kind is unknown.
    if f.len() != 4 && f.len() != 5 {
        return None;
    }
    // `\r` at the end of the last field is tolerated.
    let epoch = if f.len() == 4 {
        f[3].strip_suffix('\r').unwrap_or(f[3])
    } else {
        f[3]
    };
    let kind = f.get(4).map_or(DeviceKind::Unknown, |k| {
        DeviceKind::parse(k.strip_suffix('\r').unwrap_or(k))
    });
    if epoch.is_empty() || !epoch.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let read_epoch = epoch.parse::<u64>().ok()?;
    let percent = if f[1] == "-" {
        None
    } else if (1..=3).contains(&f[1].len()) && f[1].bytes().all(|b| b.is_ascii_digit()) {
        let p: u8 = f[1].parse::<u16>().ok().filter(|&p| p <= 100)? as u8;
        Some(p)
    } else {
        return None;
    };
    Some(Device {
        name: sanitise(f[0]),
        percent,
        state: sanitise(f[2]),
        read_epoch,
        kind,
    })
}

pub fn parse(text: &str) -> Vec<Device> {
    text.split('\n').filter_map(parse_line).take(MAX_LINES).collect()
}

pub fn read(path: &Path) -> Vec<Device> {
    match crate::bounded::read_bounded(path, MAX_BYTES, false) {
        Ok(bytes) => parse(&String::from_utf8_lossy(&bytes)),
        Err(_) => Vec::new(),
    }
}

/// How a tile line is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    /// Reachable and below `LOW_BELOW` percent or critical: red.
    Low,
    /// Not reachable: muted.
    Muted,
}

/// One device line of the tile: label left, percent right, then the
/// charging mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub label: String,
    /// `100 %`, or `–` when the device is not reachable or has no percent.
    pub percent: String,
    pub charging: bool,
    pub tone: Tone,
}

/// The label of each device, in file order: its kind, numbered when two
/// devices share it (`Keyboard 1`, `Keyboard 2`), or its name when the kind
/// is unknown.
pub fn labels(devices: &[Device]) -> Vec<String> {
    let mut seen = std::collections::HashMap::new();
    devices
        .iter()
        .map(|d| match d.kind.label() {
            None => d.name.clone(),
            Some(label) => {
                let total = devices.iter().filter(|o| o.kind == d.kind).count();
                if total == 1 {
                    label.to_string()
                } else {
                    let n = seen.entry(d.kind).or_insert(0);
                    *n += 1;
                    format!("{label} {n}")
                }
            }
        })
        .collect()
}

/// One line per device: known kinds first, in the order of `DeviceKind`,
/// otherwise in file order.
pub fn tile_lines(devices: &[Device]) -> Vec<Line> {
    let mut lines: Vec<(DeviceKind, Line)> = devices
        .iter()
        .zip(labels(devices))
        .map(|(d, label)| {
            let percent = match d.percent {
                Some(p) if d.reachable() => format!("{p} %"),
                _ => "–".to_string(),
            };
            let tone = if !d.reachable() {
                Tone::Muted
            } else if d.low() {
                Tone::Low
            } else {
                Tone::Normal
            };
            let line = Line {
                label,
                percent,
                charging: d.state == "charging",
                tone,
            };
            (d.kind, line)
        })
        .collect();
    lines.sort_by_key(|(kind, _)| *kind);
    lines.into_iter().map(|(_, l)| l).collect()
}

/// The lines `start..end` the tile shows, and how long until the next page
/// (`None` when all lines fit and nothing pages).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub start: usize,
    pub end: usize,
    pub next: Option<Duration>,
}

/// The page of `lines` lines, `per_page` to a page, shown `elapsed` after
/// the lines were set. Each page stays `PAGE_EVERY`; after the last comes
/// the first.
pub fn page(lines: usize, per_page: usize, elapsed: Duration) -> Page {
    let per_page = per_page.max(1);
    let pages = lines.div_ceil(per_page);
    if pages <= 1 {
        return Page {
            start: 0,
            end: lines,
            next: None,
        };
    }
    let every = PAGE_EVERY.as_millis();
    let ms = elapsed.as_millis();
    let index = ((ms / every) % pages as u128) as usize;
    let start = index * per_page;
    Page {
        start,
        end: (start + per_page).min(lines),
        next: Some(Duration::from_millis((every - ms % every) as u64)),
    }
}

fn phrase(d: &Device, label: &str, h: i8, m: i8) -> String {
    let p = d.percent.map_or_else(|| "–".to_string(), |p| format!("{p} %"));
    let who = if label == d.name {
        label.to_string()
    } else {
        format!("{label} ({})", d.name)
    };
    format!("{who}: {p}, {}, read {:02}:{:02}", d.state, h, m)
}

/// The log line of a device, `label` from `labels`.
pub fn log_text(d: &Device, label: &str) -> String {
    let (h, m, _) = crate::log::local_hms(d.read_epoch as i64);
    phrase(d, label, h, m)
}

/// Says when the file needs reading again: the first `tick` always, then
/// only when the file's stamp changed (or it appeared or vanished).
#[derive(Debug, Default)]
pub struct Watcher {
    seen: Option<Option<crate::addons::Stamp>>,
}

impl Watcher {
    pub fn tick(&mut self, path: &Path) -> bool {
        let now = crate::addons::stamp(path);
        let changed = self.seen.as_ref() != Some(&now);
        self.seen = Some(now);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::fs;

    fn dev(name: &str, p: Option<u8>, state: &str) -> Device {
        Device {
            name: name.into(),
            percent: p,
            state: state.into(),
            read_epoch: 1,
            kind: DeviceKind::Unknown,
        }
    }

    fn kdev(name: &str, kind: DeviceKind, p: Option<u8>, state: &str) -> Device {
        Device {
            kind,
            ..dev(name, p, state)
        }
    }

    #[test]
    fn parse_takes_good_lines_only() {
        let text = "MX Mechanical\t100\tcharging\t1000\tkeyboard\n\
                    Mouse\t-\tnot reachable\t900\n\
                    a\t5\tfull\n\
                    a\t5\tfull\t1\tmouse\textra\n\
                    Odd\t5\tfull\t1\tlaser\n\
                    x\t101\tfull\t1\n\
                    x\t9a\tfull\t1\n\
                    x\t5\tfull\t-1\n\
                    \n\
                    Pad\x1b\t7\tdischarging\t5\r\n";
        assert_eq!(
            parse(text),
            vec![
                Device {
                    name: "MX Mechanical".into(),
                    percent: Some(100),
                    state: "charging".into(),
                    read_epoch: 1000,
                    kind: DeviceKind::Keyboard,
                },
                Device {
                    name: "Mouse".into(),
                    percent: None,
                    state: "not reachable".into(),
                    read_epoch: 900,
                    kind: DeviceKind::Unknown,
                },
                Device {
                    name: "Odd".into(),
                    percent: Some(5),
                    state: "full".into(),
                    read_epoch: 1,
                    kind: DeviceKind::Unknown,
                },
                Device {
                    name: "Pad?".into(),
                    percent: Some(7),
                    state: "discharging".into(),
                    read_epoch: 5,
                    kind: DeviceKind::Unknown,
                },
            ]
        );
    }

    #[test]
    fn names_are_sanitised_and_cut() {
        assert_eq!(sanitise("A\u{202e}B\u{2028}C\x1b"), "A?B?C?");
        for c in [
            '\u{061C}',
            '\u{00AD}',
            '\u{180E}',
            '\u{206A}',
            '\u{206F}',
            '\u{FFF9}',
            '\u{FFFB}',
            '\u{E0000}',
            '\u{E0041}',
            '\u{E007F}',
        ] {
            assert_eq!(sanitise(&format!("A{c}B")), "A?B", "U+{:04X}", c as u32);
        }
        assert_eq!(sanitise(&"W".repeat(200)).chars().count(), 64);
        assert_eq!(sanitise("  "), "unknown device");
    }

    #[test]
    fn read_is_bounded_and_refuses_odd_files() {
        let d = TestDir::new("battery", "read");
        assert!(read(&d.join("missing")).is_empty());

        let good = d.join("good");
        fs::write(&good, "Mouse\t50\tdischarging\t1\n").unwrap();
        assert_eq!(read(&good).len(), 1);
        let link = d.join("link");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        assert!(read(&link).is_empty());

        let fifo = d.join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(read(&fifo).is_empty());

        let big = d.join("big");
        fs::write(&big, vec![b'a'; MAX_BYTES as usize + 1]).unwrap();
        assert!(read(&big).is_empty());

        let bad = d.join("bad");
        fs::write(&bad, b"Mo\xffuse\t50\tfull\t1\n").unwrap();
        let v = read(&bad);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "Mo\u{FFFD}use");

        let many = d.join("many");
        fs::write(&many, "M\t5\tfull\t1\n".repeat(100)).unwrap();
        assert_eq!(read(&many).len(), 64);
    }

    #[test]
    fn every_kind_word_parses_and_has_its_label() {
        for (word, label) in [
            ("keyboard", Some("Keyboard")),
            ("mouse", Some("Mouse")),
            ("numpad", Some("Numpad")),
            ("presenter", Some("Presenter")),
            ("trackball", Some("Trackball")),
            ("touchpad", Some("Touchpad")),
            ("unknown", None),
            ("Keyboard", None),
            ("", None),
        ] {
            assert_eq!(DeviceKind::parse(word).label(), label, "{word:?}");
        }
    }

    #[test]
    fn a_line_names_the_kind_and_says_percent_and_charging() {
        let v = vec![
            kdev("MX Anywhere 3S for Business", DeviceKind::Mouse, Some(95), "charging"),
            kdev("MX Mechanical", DeviceKind::Keyboard, Some(100), "discharging"),
            dev("Pen", Some(5), "not reachable"),
            dev("Stylus", None, "unknown"),
        ];
        let lines = tile_lines(&v);
        let shown: Vec<(&str, &str, bool, Tone)> = lines
            .iter()
            .map(|l| (l.label.as_str(), l.percent.as_str(), l.charging, l.tone))
            .collect();
        // Known kinds first, keyboard before mouse; unknown ones by name.
        assert_eq!(
            shown,
            vec![
                ("Keyboard", "100 %", false, Tone::Normal),
                ("Mouse", "95 %", true, Tone::Normal),
                ("Pen", "–", false, Tone::Muted),
                ("Stylus", "–", false, Tone::Normal),
            ]
        );
    }

    #[test]
    fn two_devices_of_one_kind_are_numbered_in_file_order() {
        let v = vec![
            kdev("B Keys", DeviceKind::Keyboard, Some(50), "discharging"),
            kdev("M", DeviceKind::Mouse, Some(50), "discharging"),
            kdev("A Keys", DeviceKind::Keyboard, Some(60), "discharging"),
            dev("Same", Some(1), "not reachable"),
            dev("Same", Some(2), "not reachable"),
        ];
        assert_eq!(labels(&v), vec!["Keyboard 1", "Mouse", "Keyboard 2", "Same", "Same"]);
        let order: Vec<String> = tile_lines(&v).into_iter().map(|l| l.label).collect();
        assert_eq!(order, vec!["Keyboard 1", "Keyboard 2", "Mouse", "Same", "Same"]);
    }

    #[test]
    fn red_below_10_percent_or_at_critical_when_reachable() {
        let tone = |p, s| tile_lines(&[dev("M", p, s)])[0].tone;
        assert_eq!(tone(Some(9), "discharging"), Tone::Low);
        assert_eq!(tone(Some(0), "discharging"), Tone::Low);
        assert_eq!(tone(Some(10), "discharging"), Tone::Normal);
        assert_eq!(tone(Some(15), "low"), Tone::Normal);
        assert_eq!(tone(Some(50), "critical"), Tone::Low);
        assert_eq!(tone(None, "critical"), Tone::Low);
        assert_eq!(tone(Some(5), "not reachable"), Tone::Muted);
        assert!(dev("M", Some(9), "charging").low());
        assert!(!dev("M", Some(9), "not reachable").low());
    }

    #[test]
    fn all_lines_on_one_page_means_no_paging() {
        for elapsed in [0, 3, 4, 100] {
            assert_eq!(
                page(3, 3, Duration::from_secs(elapsed)),
                Page {
                    start: 0,
                    end: 3,
                    next: None
                }
            );
        }
        assert_eq!(page(0, 3, Duration::ZERO).next, None);
    }

    #[test]
    fn pages_switch_every_page_interval() {
        let at = |ms| page(5, 2, Duration::from_millis(ms));
        assert_eq!((at(0).start, at(0).end), (0, 2));
        assert_eq!(at(0).next, Some(PAGE_EVERY));
        assert_eq!(at(1500).next, Some(PAGE_EVERY - Duration::from_millis(1500)));
        assert_eq!((at(3999).start, at(3999).end), (0, 2));
        assert_eq!((at(4000).start, at(4000).end), (2, 4));
        assert_eq!((at(8000).start, at(8000).end), (4, 5));
        // Back to the first page after the last.
        assert_eq!((at(12_000).start, at(12_000).end), (0, 2));
        // At least one line a page, however small the tile.
        assert_eq!(
            (
                page(2, 0, Duration::from_secs(4)).start,
                page(2, 0, Duration::from_secs(4)).end
            ),
            (1, 2)
        );
    }

    #[test]
    fn log_text_says_kind_name_percent_state_and_time() {
        let d = kdev("MX Mechanical", DeviceKind::Keyboard, Some(100), "charging");
        let d = Device { read_epoch: 3660, ..d };
        assert_eq!(
            phrase(&d, "Keyboard", 13, 47),
            "Keyboard (MX Mechanical): 100 %, charging, read 13:47"
        );
        let m = Device {
            read_epoch: 3660,
            ..dev("Mouse", None, "not reachable")
        };
        assert_eq!(phrase(&m, "Mouse", 1, 1), "Mouse: –, not reachable, read 01:01");
        let t = log_text(&d, "Keyboard");
        assert!(t.starts_with("Keyboard (MX Mechanical): 100 %, charging, read "), "{t}");
        assert_eq!(t.len(), "Keyboard (MX Mechanical): 100 %, charging, read 00:00".len());
    }

    #[test]
    fn watcher_fires_first_then_on_change() {
        let d = TestDir::new("battery", "watch");
        let p = d.join("battery");
        fs::write(&p, "a\t1\tfull\t1\n").unwrap();
        let mut w = Watcher::default();
        assert!(w.tick(&p));
        assert!(!w.tick(&p));
        fs::write(&p, "b\t22\tfull\t2\n").unwrap();
        assert!(w.tick(&p));
        fs::remove_file(&p).unwrap();
        assert!(w.tick(&p));
        assert!(!w.tick(&p));
    }
}
