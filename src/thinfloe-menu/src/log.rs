//! The log under the grid: what the menu did, and what the programs it
//! started wrote. Tiles come from files the user writes, so their output is
//! untrusted: every line is cut at `LINE_BYTES`, and the log, the queue
//! between the reader threads and the event loop, and the splitter's buffer
//! all have a fixed ceiling.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

/// Lines the log keeps, and the most the queue holds before the oldest go.
pub const LOG_LINES: usize = 500;
/// The longest line kept, in bytes of program output. A longer one is cut
/// and ends in "…"; the rest of it up to its newline is dropped.
pub const LINE_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Something the menu did: a start, a clean exit, a save.
    Event,
    /// A line a program wrote.
    Output,
    /// A failure, or a warning about the tile files.
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Local wall-clock time, HH:MM:SS.
    pub time: String,
    /// The tile name for program output, `None` for the menu's own lines.
    pub source: Option<String>,
    pub runs: Vec<Run>,
    pub kind: Kind,
}

#[derive(Debug, Default)]
pub struct Log {
    lines: VecDeque<Line>,
}

impl Log {
    /// Add a line, dropping the oldest past `LOG_LINES`. Every line also goes
    /// to stderr, which thinfloe-session sends to the session log: before the
    /// log existed, program output went there directly.
    pub fn push(&mut self, line: Line) {
        match &line.source {
            Some(s) => eprintln!("thinfloe-menu: {s} | {}", line.plain()),
            None => eprintln!("thinfloe-menu: {}", line.plain()),
        }
        if self.lines.len() == LOG_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    /// A line of the menu's own, stamped now.
    pub fn note(&mut self, kind: Kind, text: impl Into<String>) {
        self.push(Line {
            time: local_time(),
            source: None,
            runs: vec![Run::plain(text)],
            kind,
        });
    }

    pub fn lines(&self) -> impl ExactSizeIterator<Item = &Line> {
        self.lines.iter()
    }

    pub fn last(&self) -> Option<&Line> {
        self.lines.back()
    }
}

/// The local time of day, HH:MM:SS.
pub fn local_time() -> String {
    // SAFETY: time(NULL) has no preconditions; localtime_r writes only into
    // the struct passed to it, and a failure leaves it zeroed.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

/// The most colour runs one line keeps. Past it, colour changes are ignored
/// and the text joins the last run: a program cannot make a line cost more
/// than this many sections to draw.
pub const MAX_RUNS: usize = 64;

/// A foreground colour a program asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ansi {
    /// 0-7 normal, 8-15 bright.
    Basic(u8),
    /// The 256-colour palette.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// A piece of a line in one colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    /// `None` draws in the line's own colour.
    pub fg: Option<Ansi>,
    pub bold: bool,
}

impl Run {
    pub fn plain(text: impl Into<String>) -> Run {
        Run {
            text: text.into(),
            fg: None,
            bold: false,
        }
    }
}

/// The text of runs without their colours.
pub fn plain(runs: &[Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

impl Line {
    pub fn plain(&self) -> String {
        plain(&self.runs)
    }
}

/// Apply one SGR parameter list (the part between `ESC [` and `m`).
/// Only the foreground and bold are kept; everything else is read and
/// ignored, including background colours and their arguments.
fn sgr(params: &str, mut fg: Option<Ansi>, mut bold: bool) -> (Option<Ansi>, bool) {
    let p: Vec<Option<u16>> = params
        .split(';')
        .map(|x| if x.is_empty() { Some(0) } else { x.parse().ok() })
        .collect();
    let byte = |i: usize| p.get(i).copied().flatten().and_then(|n| u8::try_from(n).ok());
    let mut i = 0;
    while i < p.len() {
        match p[i] {
            Some(0) => {
                fg = None;
                bold = false;
            }
            Some(1) => bold = true,
            Some(22) => bold = false,
            Some(n @ 30..=37) => fg = Some(Ansi::Basic((n - 30) as u8)),
            Some(n @ 90..=97) => fg = Some(Ansi::Basic((n - 90 + 8) as u8)),
            Some(39) => fg = None,
            Some(c @ (38 | 48)) => match p.get(i + 1).copied().flatten() {
                Some(5) => {
                    if c == 38
                        && let Some(n) = byte(i + 2)
                    {
                        fg = Some(Ansi::Indexed(n));
                    }
                    i += 2;
                }
                Some(2) => {
                    if c == 38
                        && let (Some(r), Some(g), Some(b)) = (byte(i + 2), byte(i + 3), byte(i + 4))
                    {
                        fg = Some(Ansi::Rgb(r, g, b));
                    }
                    i += 4;
                }
                _ => {}
            },
            _ => {}
        }
        i += 1;
    }
    (fg, bold)
}

/// One line of program output as coloured runs: invalid UTF-8 replaced, a
/// tab as a space, SGR colours kept, every other escape sequence and
/// control character removed. A sequence cut off by the end of the line is
/// removed too. Never returns an empty list.
pub fn parse(bytes: &[u8]) -> Vec<Run> {
    let s = String::from_utf8_lossy(bytes);
    let mut chars = s.chars().peekable();
    let mut runs: Vec<Run> = Vec::new();
    let mut cur = Run::plain("");
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    let mut params = String::new();
                    let mut end = None;
                    for c in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            end = Some(c);
                            break;
                        }
                        // Bounded like the line: a parameter list longer
                        // than this is no colour anyone meant.
                        if params.len() < 64 {
                            params.push(c);
                        }
                    }
                    if end == Some('m') {
                        let (fg, bold) = sgr(&params, cur.fg, cur.bold);
                        if (fg, bold) != (cur.fg, cur.bold) {
                            if cur.text.is_empty() {
                                cur.fg = fg;
                                cur.bold = bold;
                            } else if runs.len() + 1 < MAX_RUNS {
                                let next = Run {
                                    text: String::new(),
                                    fg,
                                    bold,
                                };
                                runs.push(std::mem::replace(&mut cur, next));
                            }
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                // ESC ( B and the like: one intermediate, one final byte.
                Some(c) if ('\x20'..='\x2f').contains(&c) => {
                    chars.next();
                }
                _ => {}
            },
            '\t' => cur.text.push(' '),
            c if c.is_control() => {}
            c => cur.text.push(c),
        }
    }
    if !cur.text.is_empty() || runs.is_empty() {
        runs.push(cur);
    }
    runs
}

/// Splits a byte stream into lines without ever holding more than
/// `LINE_BYTES` of it.
#[derive(Debug, Default)]
pub struct Splitter {
    buf: Vec<u8>,
    /// The current line was already cut; drop the rest of it.
    skipping: bool,
}

impl Splitter {
    pub fn feed(&mut self, mut chunk: &[u8], out: &mut impl FnMut(Vec<Run>)) {
        while !chunk.is_empty() {
            let nl = chunk.iter().position(|&b| b == b'\n');
            let (part, rest, ends) = match nl {
                Some(i) => (&chunk[..i], &chunk[i + 1..], true),
                None => (chunk, &chunk[chunk.len()..], false),
            };
            if !self.skipping {
                let room = LINE_BYTES - self.buf.len();
                if part.len() > room {
                    self.buf.extend_from_slice(&part[..room]);
                    let mut runs = parse(&self.buf);
                    // parse never returns an empty list.
                    if let Some(last) = runs.last_mut() {
                        last.text.push('…');
                    }
                    out(runs);
                    self.buf.clear();
                    self.skipping = true;
                } else {
                    self.buf.extend_from_slice(part);
                }
            }
            if ends {
                if !self.skipping {
                    out(parse(&self.buf));
                }
                self.buf.clear();
                self.skipping = false;
            }
            chunk = rest;
        }
    }

    /// The stream ended: a last line without a newline is still a line.
    pub fn finish(&mut self, out: &mut impl FnMut(Vec<Run>)) {
        if !self.skipping && !self.buf.is_empty() {
            out(parse(&self.buf));
        }
        self.buf.clear();
        self.skipping = false;
    }
}

/// Lines on their way from the reader threads to the event loop. The loop
/// may be slower than a program that floods its output, so the queue drops
/// its oldest lines rather than grow.
#[derive(Debug, Default)]
pub struct Feed {
    queue: Mutex<VecDeque<Line>>,
    ready: Condvar,
}

impl Feed {
    pub fn push(&self, line: Line) {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() == LOG_LINES {
            q.pop_front();
        }
        q.push_back(line);
        self.ready.notify_one();
    }

    pub fn drain(&self) -> Vec<Line> {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.drain(..).collect()
    }

    /// Block until at least one line is queued.
    pub fn wait(&self) {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        while q.is_empty() {
            q = self.ready.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(chunks: &[&[u8]]) -> Vec<String> {
        let mut s = Splitter::default();
        let mut out = Vec::new();
        for c in chunks {
            s.feed(c, &mut |r| out.push(plain(&r)));
        }
        s.finish(&mut |r| out.push(plain(&r)));
        out
    }

    fn run(text: &str, fg: Option<Ansi>, bold: bool) -> Run {
        Run {
            text: text.into(),
            fg,
            bold,
        }
    }

    #[test]
    fn lines_split_across_chunks_are_joined() {
        assert_eq!(split(&[b"one\ntw", b"o\nthree"]), ["one", "two", "three"]);
        assert_eq!(split(&[b"\n\n"]), ["", ""]);
    }

    #[test]
    fn a_long_line_is_cut_and_its_rest_dropped() {
        let long = vec![b'x'; LINE_BYTES * 3];
        let out = split(&[&long[..1000], &long[1000..], b"\nnext\n"]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], format!("{}…", "x".repeat(LINE_BYTES)));
        assert_eq!(out[1], "next");
    }

    #[test]
    fn a_line_of_exactly_the_limit_is_not_cut() {
        let mut exact = vec![b'y'; LINE_BYTES];
        exact.push(b'\n');
        assert_eq!(split(&[&exact]), ["y".repeat(LINE_BYTES)]);
    }

    #[test]
    fn an_endless_line_holds_at_most_the_limit() {
        let mut s = Splitter::default();
        let mut n = 0;
        let chunk = vec![b'z'; 4096];
        for _ in 0..1000 {
            s.feed(&chunk, &mut |_| n += 1);
            assert!(s.buf.len() <= LINE_BYTES);
        }
        assert_eq!(n, 1, "cut once, the rest dropped");
    }

    #[test]
    fn control_characters_are_removed() {
        assert_eq!(plain(&parse(b"a\tb\r")), "a b");
        assert_eq!(plain(&parse(b"bad \xff byte")), "bad \u{fffd} byte");
    }

    #[test]
    fn sgr_colours_become_runs() {
        // As oxulnk-desktop writes a log line.
        assert_eq!(
            parse(b"\x1b[2m2026-10-08\x1b[0m \x1b[33m WARN\x1b[0m \x1b[2moxulnk\x1b[0m: slow"),
            [
                run("2026-10-08 ", None, false),
                run(" WARN", Some(Ansi::Basic(3)), false),
                run(" oxulnk: slow", None, false),
            ]
        );
    }

    #[test]
    fn bold_bright_256_and_rgb() {
        assert_eq!(parse(b"\x1b[1;31mA"), [run("A", Some(Ansi::Basic(1)), true)]);
        assert_eq!(parse(b"\x1b[92mB"), [run("B", Some(Ansi::Basic(10)), false)]);
        assert_eq!(parse(b"\x1b[38;5;208mC"), [run("C", Some(Ansi::Indexed(208)), false)]);
        assert_eq!(parse(b"\x1b[38;2;1;2;3mD"), [run("D", Some(Ansi::Rgb(1, 2, 3)), false)]);
        assert_eq!(
            parse(b"\x1b[31mE\x1b[39mF"),
            [run("E", Some(Ansi::Basic(1)), false), run("F", None, false)]
        );
        assert_eq!(
            parse(b"\x1b[1mG\x1b[22mH"),
            [run("G", None, true), run("H", None, false)]
        );
        // A background colour is read and ignored, not taken for a foreground.
        assert_eq!(parse(b"\x1b[48;5;1;32mI"), [run("I", Some(Ansi::Basic(2)), false)]);
        assert_eq!(parse(b"\x1b[38;5;999mJ"), [run("J", None, false)], "out of range");
    }

    #[test]
    fn other_escapes_are_removed_whole() {
        assert_eq!(plain(&parse(b"\x1b]0;title\x07after")), "after");
        assert_eq!(plain(&parse(b"\x1b]0;title\x1b\\after")), "after");
        assert_eq!(plain(&parse(b"a\x1b[2Kb\x1b[10;20Hc")), "abc");
        assert_eq!(plain(&parse(b"\x1b(Bx\x1b=y")), "xy");
    }

    #[test]
    fn a_sequence_cut_at_the_end_is_removed() {
        assert_eq!(parse(b"ok\x1b[38;5"), [run("ok", None, false)]);
        assert_eq!(plain(&parse(b"ok\x1b]0;unterminated")), "ok");
        assert_eq!(plain(&parse(b"ok\x1b")), "ok");
    }

    #[test]
    fn escapes_only_give_an_empty_line() {
        assert_eq!(parse(b"\x1b[31m\x1b[0m"), [run("", None, false)]);
        assert_eq!(parse(b""), [run("", None, false)]);
    }

    #[test]
    fn colour_changes_past_the_limit_join_the_last_run() {
        let mut input = Vec::new();
        for i in 0..200 {
            input.extend_from_slice(format!("\x1b[3{}m{}", i % 8, i % 10).as_bytes());
        }
        let runs = parse(&input);
        assert_eq!(runs.len(), MAX_RUNS);
        assert_eq!(plain(&runs).len(), 200, "no text lost");
    }

    #[test]
    fn a_cut_line_ends_in_an_ellipsis_after_its_colours() {
        let mut input = b"\x1b[31m".to_vec();
        input.extend(std::iter::repeat_n(b'x', LINE_BYTES + 10));
        input.push(b'\n');
        let mut got = Vec::new();
        let mut s = Splitter::default();
        s.feed(&input, &mut |r| got.push(r));
        assert_eq!(got.len(), 1);
        let last = got[0].last().unwrap();
        assert!(last.text.ends_with('…'));
        assert_eq!(last.fg, Some(Ansi::Basic(1)));
    }

    fn line(text: &str) -> Line {
        Line {
            time: "12:00:00".into(),
            source: Some("T".into()),
            runs: vec![Run::plain(text)],
            kind: Kind::Output,
        }
    }

    #[test]
    fn the_log_keeps_the_newest_lines() {
        let mut log = Log::default();
        for i in 0..LOG_LINES + 10 {
            log.push(line(&i.to_string()));
        }
        assert_eq!(log.lines().len(), LOG_LINES);
        assert_eq!(log.lines().next().unwrap().plain(), "10");
        assert_eq!(log.last().unwrap().plain(), (LOG_LINES + 9).to_string());
    }

    #[test]
    fn the_feed_drops_its_oldest_lines_when_full() {
        let feed = Feed::default();
        for i in 0..LOG_LINES * 3 {
            feed.push(line(&i.to_string()));
        }
        let got = feed.drain();
        assert_eq!(got.len(), LOG_LINES);
        assert_eq!(got[0].plain(), (LOG_LINES * 2).to_string());
        assert!(feed.drain().is_empty());
    }

    #[test]
    fn local_time_is_hh_mm_ss() {
        let t = local_time();
        assert_eq!(t.len(), 8, "{t}");
        assert!(
            t.chars()
                .enumerate()
                .all(|(i, c)| if i == 2 || i == 5 { c == ':' } else { c.is_ascii_digit() })
        );
    }
}
