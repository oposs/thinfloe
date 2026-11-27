//! The sticks and cards the automount helper keeps mounted, read from its
//! contract file `/run/thinfloe/removable`: one line per mounted partition,
//! `device<TAB>disk<TAB>name<TAB>mountpoint<TAB>fstype<TAB>size_bytes<TAB>model<TAB>kind<TAB>mount`,
//! mount a number per mount, counted per boot (0 when missing or not a number).
//! The helper is trusted, the file is parsed as if it were not: bounded read,
//! malformed lines dropped, names made printable, the disk name restricted to
//! what may be handed to the helper again. Unknown trailing fields are
//! ignored.

use std::path::Path;

pub const FILE: &str = "/run/thinfloe/removable";
/// One line per ejection or removal: `seq<TAB>name<TAB>clean|unclean[<TAB>count]`,
/// the count (2 or more) only for an "eject all" batch.
pub const EVENT_FILE: &str = "/run/thinfloe/removable.event";
pub const MAX_BYTES: u64 = 64 * 1024;
pub const MAX_LINES: usize = 256;
/// The most sticks the menu lists.
pub const MAX_STICKS: usize = 16;
/// The longest disk name taken over (`nvme0n1`, `mmcblk0`, `sda`).
const MAX_DISK: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Stick,
    Card,
}

/// One physical disk and the names of its mounted partitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stick {
    pub disk: String,
    /// The partitions' names, in file order.
    pub names: Vec<String>,
    /// The partitions' mount numbers, parallel to `names` (0 for a line
    /// without one): a card swapped for one of the same name differs here.
    pub mounts: Vec<u64>,
    pub size_bytes: u64,
    pub model: String,
    pub kind: Kind,
}

impl Stick {
    /// What the menu calls the stick: its first name, else its model, else
    /// the disk.
    pub fn label(&self) -> &str {
        [self.names.first(), Some(&self.model)]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .find(|s| !s.is_empty())
            .unwrap_or(&self.disk)
    }
}

/// A disk name safe to pass on as an argument: ASCII letters, digits, `_`
/// and `-`, not starting with `-`.
fn valid_disk(disk: &str) -> bool {
    !disk.is_empty()
        && disk.len() <= MAX_DISK
        && !disk.starts_with('-')
        && disk
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Printable text, empty when there is nothing: unlike a battery device, a
/// stick without a model has no placeholder.
fn clean(text: &str) -> String {
    if text.trim().is_empty() {
        String::new()
    } else {
        crate::battery::sanitise(text)
    }
}

struct Line {
    disk: String,
    name: String,
    size: u64,
    model: String,
    kind: Kind,
    mount: u64,
}

fn parse_line(line: &str) -> Option<Line> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 8 || !valid_disk(f[1]) {
        return None;
    }
    let size = f[5];
    if size.is_empty() || !size.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Line {
        disk: f[1].to_string(),
        name: clean(f[2]),
        size: size.parse().ok()?,
        model: clean(f[6]),
        kind: if f[7] == "card" { Kind::Card } else { Kind::Stick },
        mount: f
            .get(8)
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
    })
}

/// The sticks the lines name, one per disk, in the order of the first line
/// of each.
pub fn parse(text: &str) -> Vec<Stick> {
    let mut out: Vec<Stick> = Vec::new();
    for l in text.split('\n').take(MAX_LINES).filter_map(parse_line) {
        match out.iter().position(|s| s.disk == l.disk) {
            Some(i) => {
                out[i].names.push(l.name);
                out[i].mounts.push(l.mount);
            }
            None if out.len() < MAX_STICKS => out.push(Stick {
                disk: l.disk,
                names: vec![l.name],
                mounts: vec![l.mount],
                size_bytes: l.size,
                model: l.model,
                kind: l.kind,
            }),
            None => {}
        }
    }
    out
}

pub fn read(path: &Path) -> Vec<Stick> {
    match crate::bounded::read_bounded(path, MAX_BYTES, false) {
        Ok(bytes) => parse(&String::from_utf8_lossy(&bytes)),
        Err(_) => Vec::new(),
    }
}

/// What the helper last told: `seq<TAB>name<TAB>clean|unclean[<TAB>count]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub seq: u64,
    pub name: String,
    pub clean: bool,
    /// How many sticks the event is about: 1 unless the line carries a
    /// count of 2 or more (an "eject all" batch, `name` then joins their names).
    pub count: u32,
    /// How many of them are cards: the field after the count, 0 when it is
    /// missing (an older helper) or more than the count.
    pub cards: u32,
}

pub fn parse_event(text: &str) -> Option<Event> {
    let line = text.lines().next()?;
    let mut f = line.split('\t');
    let seq = f
        .next()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))?;
    let name = f.next()?;
    let clean = match f.next()? {
        "clean" => true,
        "unclean" => false,
        _ => return None,
    };
    let count = f
        .next()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(1);
    let cards = f
        .next()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|&n| n <= count)
        .unwrap_or(0);
    Some(Event {
        seq: seq.parse().ok()?,
        name: crate::battery::sanitise(name),
        clean,
        count,
        cards,
    })
}

/// How many sticks and how many cards the kinds are.
pub fn tally(kinds: impl IntoIterator<Item = Kind>) -> (usize, usize) {
    kinds.into_iter().fold((0, 0), |(s, c), k| match k {
        Kind::Stick => (s + 1, c),
        Kind::Card => (s, c + 1),
    })
}

/// Each kind with its count: "2 sticks", "1 stick and 1 card", "2 cards".
pub fn counted(sticks: usize, cards: usize) -> String {
    let one = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    match (sticks, cards) {
        (s, 0) => one(s, "stick"),
        (0, c) => one(c, "card"),
        (s, c) => format!("{} and {}", one(s, "stick"), one(c, "card")),
    }
}

/// Each kind without a count: "the stick", "the sticks and the card".
/// Nothing known reads as "the sticks".
pub fn the_kinds(sticks: usize, cards: usize) -> String {
    let the = |n: usize, word: &str| format!("the {word}{}", if n == 1 { "" } else { "s" });
    match (sticks, cards) {
        (0, 0) => "the sticks".to_string(),
        (s, 0) => the(s, "stick"),
        (0, c) => the(c, "card"),
        (s, c) => format!("{} and {}", the(s, "stick"), the(c, "card")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    fn line(dev: &str, disk: &str, name: &str, kind: &str) -> String {
        format!("/dev/{dev}\t{disk}\t{name}\t/media/tfuser/{name}\tvfat\t16000000000\tCruzer\t{kind}\n")
    }

    #[test]
    fn parse_groups_by_disk() {
        let text = line("sdb1", "sdb", "ONE", "stick")
            + &line("sdb2", "sdb", "TWO", "stick")
            + &line("sdc1", "sdc", "SD", "card");
        let s = parse(&text);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].disk, "sdb");
        assert_eq!(s[0].names, ["ONE", "TWO"]);
        assert_eq!(
            (s[0].size_bytes, s[0].model.as_str(), s[0].kind),
            (16_000_000_000, "Cruzer", Kind::Stick)
        );
        assert_eq!((s[1].disk.as_str(), s[1].kind), ("sdc", Kind::Card));
        assert_eq!(s[0].label(), "ONE");
    }

    #[test]
    fn mount_field_is_optional_and_garbage_is_zero() {
        let s = parse("/dev/sdb1\tsdb\tA\t/m\tvfat\t5\tM\tstick\t7\n/dev/sdb2\tsdb\tB\t/m2\tvfat\t5\tM\tstick\n");
        assert_eq!(s[0].mounts, [7, 0]);
        assert_eq!(parse("/dev/sdb1\tsdb\tA\t/m\tvfat\t5\tM\tstick\tx\n")[0].mounts, [0]);
    }

    #[test]
    fn unknown_trailing_fields_are_ignored() {
        let text = "/dev/sdb1\tsdb\tA\t/m\tvfat\t5\tM\tstick\textra\tmore\n";
        assert_eq!(parse(text).len(), 1);
    }

    #[test]
    fn removable_parse_tolerates_garbage() {
        assert!(parse("").is_empty());
        assert!(read(Path::new("/nonexistent/removable")).is_empty());
        assert!(parse("/dev/sdb1\tsdb\tA\n").is_empty(), "three fields");
        assert!(
            parse("/dev/sdb1\tsdb\tA\t/m\tvfat\tmany\tM\tstick\n").is_empty(),
            "bad size"
        );
        assert!(
            parse("/dev/x\t-rf\tA\t/m\tvfat\t5\tM\tstick\n").is_empty(),
            "option as disk"
        );
        assert!(
            parse("/dev/x\ta b\tA\t/m\tvfat\t5\tM\tstick\n").is_empty(),
            "space in disk"
        );
        let d = TestDir::new("removable", "binary");
        let p = d.join("removable");
        std::fs::write(&p, [0xff, 0xfe, b'\t', 0, b'\n', 0xc3]).unwrap();
        assert!(read(&p).is_empty());
        let many: String = (0..10_000)
            .map(|i| line(&format!("sd{i}1"), &format!("sd{i}"), "N", "stick"))
            .collect();
        let s = parse(&many);
        assert!(s.len() <= MAX_STICKS && !s.is_empty());
        // Over MAX_BYTES the file is not read at all.
        assert!(many.len() as u64 > MAX_BYTES);
        std::fs::write(&p, &many).unwrap();
        assert!(read(&p).is_empty());
        // Under MAX_BYTES, with more lines than MAX_LINES: the line and the
        // stick caps apply through read().
        let short: String = (0..300)
            .map(|i| format!("/dev/x{i}\tx{i}\tN\t/m\tvfat\t1\tM\tstick\n"))
            .collect();
        assert!((short.len() as u64) < MAX_BYTES && short.lines().count() > MAX_LINES);
        std::fs::write(&p, &short).unwrap();
        let s = read(&p);
        assert_eq!(s.len(), MAX_STICKS);
        assert_eq!(s.last().map(|s| s.disk.as_str()), Some("x15"));
        // A garbage line between good ones does not hide them.
        let mixed = line("sdb1", "sdb", "A", "stick") + "garbage\n" + &line("sdc1", "sdc", "B", "card");
        assert_eq!(parse(&mixed).len(), 2);
    }

    #[test]
    fn names_are_made_printable_and_an_empty_label_falls_back() {
        let s = parse("/dev/sdb1\tsdb\tA\u{202e}B\t/m\tvfat\t5\t\tstick\n");
        assert_eq!(s[0].names, ["A?B"]);
        assert_eq!(s[0].model, "");
        let s = parse("/dev/sdb1\tsdb\t\t/m\tvfat\t5\tCruzer\tstick\n");
        assert_eq!(s[0].label(), "Cruzer");
        let s = parse("/dev/sdb1\tsdb\t\t/m\tvfat\t5\t\tstick\n");
        assert_eq!(s[0].label(), "sdb");
    }

    #[test]
    fn event_line() {
        assert_eq!(
            parse_event("7\tMYSTICK\tclean\n"),
            Some(Event {
                seq: 7,
                name: "MYSTICK".into(),
                clean: true,
                count: 1,
                cards: 0
            })
        );
        assert_eq!(parse_event("8\tX\tunclean").map(|e| e.clean), Some(false));
        for bad in ["", "x\tA\tclean", "1\tA", "1\tA\tmaybe", "\tA\tclean"] {
            assert_eq!(parse_event(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn event_cards_are_optional_and_within_the_count() {
        let c = |t: &str| parse_event(t).map(|e| (e.count, e.cards));
        assert_eq!(c("7\tA, B\tclean\t2\t1\n"), Some((2, 1)));
        assert_eq!(c("7\tA, B\tclean\t2\t2\n"), Some((2, 2)));
        // A helper before the field: no card known.
        assert_eq!(c("7\tA, B\tclean\t2\n"), Some((2, 0)));
        assert_eq!(c("7\tA\tclean\n"), Some((1, 0)));
        for bad in ["7\tA, B\tclean\t2\t3", "7\tA, B\tclean\t2\tx", "7\tA, B\tclean\t2\t"] {
            assert_eq!(c(bad), Some((2, 0)), "{bad:?}");
        }
    }

    #[test]
    fn kinds_are_named_with_their_count() {
        assert_eq!(counted(2, 0), "2 sticks");
        assert_eq!(counted(0, 2), "2 cards");
        assert_eq!(counted(1, 1), "1 stick and 1 card");
        assert_eq!(counted(2, 1), "2 sticks and 1 card");
        assert_eq!(counted(1, 0), "1 stick");
        assert_eq!(counted(0, 1), "1 card");
    }

    #[test]
    fn kinds_are_named_without_a_count() {
        assert_eq!(the_kinds(1, 0), "the stick");
        assert_eq!(the_kinds(0, 1), "the card");
        assert_eq!(the_kinds(3, 0), "the sticks");
        assert_eq!(the_kinds(0, 2), "the cards");
        assert_eq!(the_kinds(1, 1), "the stick and the card");
        assert_eq!(the_kinds(2, 1), "the sticks and the card");
        // Nothing known: the sticks, as before.
        assert_eq!(the_kinds(0, 0), "the sticks");
    }

    #[test]
    fn tally_counts_each_kind() {
        assert_eq!(tally([Kind::Stick, Kind::Card, Kind::Stick]), (2, 1));
        assert_eq!(tally([]), (0, 0));
    }

    #[test]
    fn event_count_is_optional_and_bounded() {
        let c = |t: &str| parse_event(t).map(|e| e.count);
        assert_eq!(c("7\tA, B\tclean\t2\n"), Some(2));
        assert_eq!(c("7\tA\tclean\n"), Some(1));
        for one in [
            "7\tA\tclean\t",
            "7\tA\tclean\t0",
            "7\tA\tclean\tx",
            "7\tA\tclean\t99999999999",
        ] {
            assert_eq!(c(one), Some(1), "{one:?}");
        }
        assert_eq!(c("7\tA\tclean\t3\tmore"), Some(3), "trailing fields ignored");
    }
}
