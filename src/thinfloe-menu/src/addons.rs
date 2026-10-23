//! What thinfloe-addon (root, a Python tool) tells the menu: the status file
//! says which add-ons are installed, and the log file says what the tool did.
//! Both are written by root, but the log carries text that came from the
//! network (versions, HTTP errors, dpkg output), so everything read here is
//! bounded, and log text goes through the menu's own log path.

use crate::log::{self, Kind, LINE_BYTES, Line};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const STATUS_PATH: &str = "/run/thinfloe/addons/status";
pub const LOG_PATH: &str = "/run/thinfloe/addons/log";
/// The source column of the menu log for everything from the add-on tool.
pub const LOG_SOURCE: &str = "add-ons";

/// The status file is read up to this size and this many lines.
pub const STATUS_BYTES: usize = 64 * 1024;
pub const STATUS_LINES: usize = 64;
/// The most log lines one poll returns.
pub const POLL_LINES: usize = 200;
/// The longest id, version or message kept from the status file, in chars.
const FIELD_CHARS: usize = 200;
/// How much of the log one poll reads, and how long a line may grow before
/// it is cut without waiting for its newline.
const POLL_BYTES: usize = 256 * 1024;
const PARTIAL_LINE_BYTES: usize = 64 * 1024;
/// A rewritten log is searched for the last line seen within this much.
const RESYNC_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddonState {
    Absent,
    Installed,
    Pending,
    Failed,
    InstallFailed,
}

impl AddonState {
    fn parse(s: &str) -> Option<AddonState> {
        Some(match s {
            "absent" => AddonState::Absent,
            "installed" => AddonState::Installed,
            "pending" => AddonState::Pending,
            "failed" => AddonState::Failed,
            "install-failed" => AddonState::InstallFailed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonStatus {
    pub id: String,
    pub ticked: bool,
    pub state: AddonState,
    pub installed: Option<String>,
    pub pending: Option<String>,
    pub auto: bool,
    pub message: Option<String>,
}

impl AddonStatus {
    /// Whether the add-on has a copy on this machine, so its tile is drawn.
    /// A failed check keeps the installed copy; a failed install has none.
    pub fn present(&self) -> bool {
        self.installed.is_some()
            && matches!(
                self.state,
                AddonState::Installed | AddonState::Pending | AddonState::Failed
            )
    }
}

/// Printable characters only, cut to `FIELD_CHARS`.
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(FIELD_CHARS).collect()
}

fn optional(s: &str) -> Option<String> {
    let s = clean(s);
    (!s.is_empty() && s != "-").then_some(s)
}

fn flag(s: &str) -> Option<bool> {
    match s {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `id ticked state installed pending auto message`, tab separated.
fn parse_status_line(line: &str) -> Option<AddonStatus> {
    let mut f = line.splitn(7, '\t');
    let id = f.next()?;
    let ticked = flag(f.next()?)?;
    let state = AddonState::parse(f.next()?)?;
    let installed = optional(f.next()?);
    let pending = optional(f.next()?);
    let auto = flag(f.next()?)?;
    let message = optional(f.next()?);
    valid_id(id).then(|| AddonStatus {
        id: id.to_string(),
        ticked,
        state,
        installed,
        pending,
        auto,
        message,
    })
}

/// Read the status file. A missing or unreadable file gives no lines, and so
/// no add-on tile is drawn.
pub fn read_status(path: &Path) -> Vec<AddonStatus> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    if file.take(STATUS_BYTES as u64 + 1).read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if buf.len() > STATUS_BYTES {
        // The last line was cut by the limit.
        text.truncate(text.rfind('\n').map_or(0, |i| i + 1));
    }
    text.lines().take(STATUS_LINES).filter_map(parse_status_line).collect()
}

/// Follows the log file by offset, returning only lines not seen before.
#[derive(Debug)]
pub struct AddonLog {
    path: PathBuf,
    offset: u64,
    inode: u64,
    /// The rest of an over-long line is being dropped.
    skipping: bool,
    /// The last line returned, to find the place again in a rewritten file.
    last: Option<Vec<u8>>,
}

impl AddonLog {
    pub fn new(path: impl Into<PathBuf>) -> AddonLog {
        AddonLog {
            path: path.into(),
            offset: 0,
            inode: 0,
            skipping: false,
            last: None,
        }
    }

    /// The new lines, each as its time and its text cut to `LINE_BYTES`.
    /// The tool keeps the last 500 lines by rewriting the file; when the file
    /// is not the one read so far, the place is found again by the last line
    /// returned, and without it reading starts at 0.
    pub fn poll(&mut self) -> Vec<(SystemTime, String)> {
        let Ok(mut file) = File::open(&self.path) else {
            return Vec::new();
        };
        let Ok(meta) = file.metadata() else {
            return Vec::new();
        };
        if meta.ino() != self.inode || meta.len() < self.offset {
            self.inode = meta.ino();
            self.offset = self.resync(&mut file);
            self.skipping = false;
        }
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut buf = Vec::new();
        if (&mut file).take(POLL_BYTES as u64).read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut pos = 0;
        if self.skipping {
            match buf.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    pos = i + 1;
                    self.skipping = false;
                }
                None => pos = buf.len(),
            }
        }
        while pos < buf.len() && out.len() < POLL_LINES {
            let rest = &buf[pos..];
            let (line, used, cut_open) = match rest.iter().position(|&b| b == b'\n') {
                Some(i) => (&rest[..i], i + 1, false),
                None if rest.len() >= PARTIAL_LINE_BYTES => (rest, rest.len(), true),
                // The tool has not finished this line.
                None => break,
            };
            pos += used;
            self.skipping = cut_open;
            let cut = line.len() > LINE_BYTES;
            let line = &line[..line.len().min(LINE_BYTES)];
            self.last = Some(line.to_vec());
            out.push(split_line(line, cut));
        }
        self.offset += pos as u64;
        out
    }

    /// Where to read in a file that is not the one read so far.
    fn resync(&self, file: &mut File) -> u64 {
        let Some(last) = &self.last else { return 0 };
        let mut buf = Vec::new();
        if file.seek(SeekFrom::Start(0)).is_err() || file.take(RESYNC_BYTES).read_to_end(&mut buf).is_err() {
            return 0;
        }
        let mut end = 0;
        let mut found = 0;
        for line in buf.split_inclusive(|&b| b == b'\n') {
            end += line.len();
            let Some(body) = line.strip_suffix(b"\n") else { break };
            if &body[..body.len().min(LINE_BYTES)] == last.as_slice() {
                found = end;
            }
        }
        found as u64
    }
}

/// `epoch<TAB>text`. A line without an epoch is shown whole, stamped now.
fn split_line(line: &[u8], cut: bool) -> (SystemTime, String) {
    let tab = line.iter().position(|&b| b == b'\t');
    let (time, text) = match tab {
        Some(i) => match std::str::from_utf8(&line[..i])
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
        {
            Some(secs) => (UNIX_EPOCH + Duration::from_secs(secs), &line[i + 1..]),
            None => (SystemTime::now(), line),
        },
        None => (SystemTime::now(), line),
    };
    let mut text = String::from_utf8_lossy(text).into_owned();
    if cut {
        text.push('…');
    }
    (time, text)
}

/// A log line of the add-on tool as the menu log shows it: source
/// `add-ons`, escape sequences removed, a failure drawn as a warning.
pub fn log_line(time: SystemTime, text: &str) -> Line {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let lower = text.to_lowercase();
    let kind = if lower.contains("failed") || lower.contains("error") || lower.contains("cannot") {
        Kind::Warn
    } else {
        Kind::Event
    };
    Line {
        time: log::local_time_at(secs as i64),
        source: Some(LOG_SOURCE.to_string()),
        runs: log::parse(text.as_bytes()),
        kind,
    }
}

/// What a watcher saw of a file the last time, to notice a change without
/// reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stamp {
    ino: u64,
    mtime: i64,
    mtime_nsec: i64,
    len: u64,
}

pub fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some(Stamp {
        ino: m.ino(),
        mtime: m.mtime(),
        mtime_nsec: m.mtime_nsec(),
        len: m.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::fs;
    use std::io::Write;

    fn status_file(tag: &str, text: &str) -> (TestDir, PathBuf) {
        let d = TestDir::new("addons", tag);
        let p = d.join("status");
        fs::write(&p, text).unwrap();
        (d, p)
    }

    #[test]
    fn status_line_parses_every_state() {
        let text = "10-thinlinc\t1\tinstalled\t4.21.0\t-\t1\t-\n\
                    20-oxulnk\t1\tpending\t1.0\t1.1\t0\tupdate ready\n\
                    30-a\t0\tabsent\t-\t-\t1\t-\n\
                    40-b\t1\tfailed\t2.0\t-\t1\tHTTP 404\n\
                    50-c\t1\tinstall-failed\t-\t-\t0\tdpkg failed\n";
        let (_d, p) = status_file("states", text);
        let s = read_status(&p);
        assert_eq!(s.len(), 5);
        assert_eq!(
            s[0],
            AddonStatus {
                id: "10-thinlinc".into(),
                ticked: true,
                state: AddonState::Installed,
                installed: Some("4.21.0".into()),
                pending: None,
                auto: true,
                message: None,
            }
        );
        assert_eq!(s[1].state, AddonState::Pending);
        assert_eq!(s[1].pending.as_deref(), Some("1.1"));
        assert_eq!(s[1].message.as_deref(), Some("update ready"));
        assert!(!s[1].auto);
        assert_eq!(s[2].state, AddonState::Absent);
        assert!(!s[2].ticked);
        assert_eq!(s[3].state, AddonState::Failed);
        assert_eq!(s[4].state, AddonState::InstallFailed);
        assert!(read_status(&p.with_file_name("missing")).is_empty());
    }

    #[test]
    fn status_bounded() {
        let mut text = String::from(
            "bad line\n\
             x\t2\tinstalled\t-\t-\t0\t-\n\
             x\t1\tweird\t-\t-\t0\t-\n\
             bad id!\t1\tinstalled\t1\t-\t0\t-\n\
             short\t1\tinstalled\n",
        );
        for i in 0..100 {
            text.push_str(&format!("id{i}\t1\tinstalled\t1\t-\t0\t-\n"));
        }
        let (_d, p) = status_file("lines", &text);
        // Malformed lines are skipped; at most 64 lines are looked at.
        let s = read_status(&p);
        assert_eq!(s.len(), STATUS_LINES - 5);
        assert_eq!(s[0].id, "id0");

        let long = "v".repeat(10_000);
        let (_d, p) = status_file("long", &format!("a\t1\tinstalled\t{long}\t-\t0\t{long}\x1b[31m\n"));
        let s = read_status(&p);
        assert_eq!(s[0].installed.as_ref().unwrap().chars().count(), FIELD_CHARS);
        assert!(!s[0].message.as_ref().unwrap().contains('\x1b'));

        // Past 64 KiB the file is not read, and a cut line is dropped.
        let big = "a\t1\tinstalled\t1\t-\t0\t-\n".repeat(10_000);
        let (_d, p) = status_file("big", &big);
        let n = read_status(&p).len();
        assert_eq!(n, STATUS_LINES);
        let (_d, p) = status_file(
            "cut",
            &format!(
                "a\t1\tinstalled\t1\t-\t0\t-\nb\t1\tinstalled\t{}",
                "9".repeat(STATUS_BYTES)
            ),
        );
        assert_eq!(read_status(&p).len(), 1);
    }

    fn append(p: &Path, text: &str) {
        let mut f = fs::OpenOptions::new().create(true).append(true).open(p).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn texts(v: Vec<(SystemTime, String)>) -> Vec<String> {
        v.into_iter().map(|(_, t)| t).collect()
    }

    #[test]
    fn log_poll_reads_only_new_lines() {
        let d = TestDir::new("addons", "poll");
        let p = d.join("log");
        let mut log = AddonLog::new(&p);
        assert!(log.poll().is_empty());
        append(&p, "1000\tfirst\n1001\tsecond\n1002\tpartial");
        let got = log.poll();
        assert_eq!(got[0].0, UNIX_EPOCH + Duration::from_secs(1000));
        assert_eq!(texts(got), ["first", "second"]);
        assert!(log.poll().is_empty());
        append(&p, " line\n1003\tfourth\n");
        assert_eq!(texts(log.poll()), ["partial line", "fourth"]);
        assert!(log.poll().is_empty());
    }

    #[test]
    fn log_poll_is_bounded() {
        let d = TestDir::new("addons", "bound");
        let p = d.join("log");
        let mut text = String::new();
        for i in 0..450 {
            text.push_str(&format!("1\tline {i}\n"));
        }
        text.push_str(&format!("1\t{}\n1\tafter\n", "x".repeat(300_000)));
        append(&p, &text);
        let mut log = AddonLog::new(&p);
        assert_eq!(log.poll().len(), POLL_LINES);
        assert_eq!(log.poll().len(), POLL_LINES);
        let rest = log.poll();
        // 50 short lines, then the over-long one cut at 1 KiB.
        assert_eq!(rest.len(), 51);
        assert_eq!(rest[50].1.len(), LINE_BYTES - 2 + '…'.len_utf8());
        assert_eq!(texts(log.poll()), ["after"]);
    }

    #[test]
    fn log_poll_restarts_after_truncation() {
        let d = TestDir::new("addons", "trunc");
        let p = d.join("log");
        let mut log = AddonLog::new(&p);
        append(&p, "1\taaaaaaaa\n2\tbbbbbbbb\n");
        assert_eq!(log.poll().len(), 2);
        fs::write(&p, "3\tc\n").unwrap();
        assert_eq!(texts(log.poll()), ["c"]);
        append(&p, "4\td\n");
        assert_eq!(texts(log.poll()), ["d"]);
    }

    #[test]
    fn log_poll_does_not_repeat_lines_when_the_tool_trims_the_file() {
        let d = TestDir::new("addons", "trim");
        let p = d.join("log");
        let mut log = AddonLog::new(&p);
        append(&p, "1\ta\n2\tb\n3\tc\n");
        assert_eq!(log.poll().len(), 3);
        // The tool writes a new file with the last lines and a new one.
        let new = d.join("log.new");
        fs::write(&new, "2\tb\n3\tc\n4\td\n").unwrap();
        fs::rename(&new, &p).unwrap();
        assert_eq!(texts(log.poll()), ["d"]);
    }

    #[test]
    fn addon_log_lines_have_the_addons_source_and_no_escapes() {
        let l = log_line(
            UNIX_EPOCH + Duration::from_secs(86_400),
            "\x1b]0;title\x07ThinLinc: download failed",
        );
        assert_eq!(l.source.as_deref(), Some("add-ons"));
        assert_eq!(l.plain(), "ThinLinc: download failed");
        assert_eq!(l.kind, Kind::Warn);
        assert_eq!(log_line(UNIX_EPOCH, "checked").kind, Kind::Event);
    }

    #[test]
    fn a_changed_file_has_a_new_stamp() {
        let d = TestDir::new("addons", "stamp");
        let p = d.join("f");
        assert_eq!(stamp(&p), None);
        fs::write(&p, "a").unwrap();
        let a = stamp(&p);
        assert!(a.is_some());
        assert_eq!(a, stamp(&p));
        fs::write(&p, "ab").unwrap();
        assert_ne!(a, stamp(&p));
    }
}
