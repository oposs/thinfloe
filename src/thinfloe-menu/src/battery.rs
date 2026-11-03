//! The battery file written by the root helper every few minutes. The helper
//! is trusted, the file is parsed as if it were not: bounded read, no
//! symlinks, malformed lines dropped, names made printable.

use std::path::Path;

pub const FILE: &str = "/run/thinfloe/peripherals/battery";
pub const MAX_BYTES: u64 = 16 * 1024;
pub const MAX_LINES: usize = 64;
pub const MAX_NAME: usize = 64;
pub const SHORT_NAME: usize = 14;
pub const WARN_PERCENT: u8 = 15;
pub const CHARGING_MARK: &str = "⚡";

const NOT_REACHABLE: &str = "not reachable";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub percent: Option<u8>,
    pub state: String,
    pub read_epoch: u64,
}

impl Device {
    pub fn reachable(&self) -> bool {
        self.state != NOT_REACHABLE
    }

    pub fn low(&self) -> bool {
        self.reachable()
            && (self.state == "low" || self.state == "critical" || self.percent.is_some_and(|p| p <= WARN_PERCENT))
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
                    | '\u{FEFF}');
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
    if f.len() != 4 {
        return None;
    }
    // `\r` at the end of the last field is tolerated.
    let epoch = f[3].strip_suffix('\r').unwrap_or(f[3]);
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileText {
    pub name: String,
    pub entries: Vec<String>,
    pub warn: bool,
}

fn short(name: &str) -> String {
    if name.chars().count() > SHORT_NAME {
        let mut s: String = name.chars().take(SHORT_NAME).collect();
        s.push('…');
        s
    } else {
        name.to_string()
    }
}

pub fn tile_text(devices: &[Device]) -> TileText {
    let lowest = devices.iter().filter(|d| d.reachable()).filter_map(|d| d.percent).min();
    let entries = devices
        .iter()
        .map(|d| {
            let p = match d.percent {
                Some(p) if d.reachable() => p.to_string(),
                _ => "–".to_string(),
            };
            let mut e = format!("{} {}", short(&d.name), p);
            if d.state == "charging" {
                e.push(' ');
                e.push_str(CHARGING_MARK);
            }
            e
        })
        .collect();
    TileText {
        name: lowest.map_or_else(|| "–".to_string(), |p| format!("{p} %")),
        entries,
        warn: devices.iter().any(Device::low),
    }
}

fn phrase(d: &Device, h: i8, m: i8) -> String {
    let p = d.percent.map_or_else(|| "–".to_string(), |p| format!("{p} %"));
    format!("{}: {}, {}, read {:02}:{:02}", d.name, p, d.state, h, m)
}

pub fn log_text(d: &Device) -> String {
    let (h, m, _) = crate::log::local_hms(d.read_epoch as i64);
    phrase(d, h, m)
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
        }
    }

    #[test]
    fn parse_takes_good_lines_only() {
        let text = "MX Mechanical\t100\tcharging\t1000\n\
                    Mouse\t-\tnot reachable\t900\n\
                    a\t5\tfull\n\
                    a\t5\tfull\t1\textra\n\
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
                    read_epoch: 1000
                },
                Device {
                    name: "Mouse".into(),
                    percent: None,
                    state: "not reachable".into(),
                    read_epoch: 900
                },
                Device {
                    name: "Pad?".into(),
                    percent: Some(7),
                    state: "discharging".into(),
                    read_epoch: 5
                },
            ]
        );
    }

    #[test]
    fn names_are_sanitised_and_cut() {
        assert_eq!(sanitise("A\u{202e}B\u{2028}C\x1b"), "A?B?C?");
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
    fn tile_text_names_the_lowest_reachable_percent() {
        let v = vec![
            dev("MX Anywhere 3S Mouse", Some(95), "discharging"),
            dev("MX Mechanical", Some(100), "charging"),
            dev("Mouse", Some(5), "not reachable"),
        ];
        let t = tile_text(&v);
        assert_eq!(t.name, "95 %");
        assert_eq!(t.entries, vec!["MX Anywhere 3S… 95", "MX Mechanical 100 ⚡", "Mouse –"]);
        assert!(!t.warn);
        assert_eq!(tile_text(&[dev("Mouse", Some(5), "not reachable")]).name, "–");
        assert_eq!(tile_text(&[dev("Mouse", None, "discharging")]).name, "–");
    }

    #[test]
    fn warn_at_low_critical_or_15_percent() {
        let w = |p, s| tile_text(&[dev("M", p, s)]).warn;
        assert!(w(Some(15), "discharging"));
        assert!(!w(Some(16), "discharging"));
        assert!(w(Some(50), "low"));
        assert!(w(Some(50), "critical"));
        assert!(!w(Some(5), "not reachable"));
    }

    #[test]
    fn log_text_says_percent_state_and_time() {
        // The zone is not set here: another test changes TZ, so the clock
        // part is checked through the pure helper, and the public function
        // only for its shape.
        let d = Device {
            name: "MX Mechanical".into(),
            percent: Some(100),
            state: "charging".into(),
            read_epoch: 3660,
        };
        assert_eq!(phrase(&d, 1, 1), "MX Mechanical: 100 %, charging, read 01:01");
        let m = Device {
            name: "Mouse".into(),
            percent: None,
            state: "not reachable".into(),
            read_epoch: 3660,
        };
        assert_eq!(phrase(&m, 1, 1), "Mouse: –, not reachable, read 01:01");
        let t = log_text(&d);
        assert!(t.starts_with("MX Mechanical: 100 %, charging, read "), "{t}");
        assert_eq!(t.len(), "MX Mechanical: 100 %, charging, read 00:00".len());
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
