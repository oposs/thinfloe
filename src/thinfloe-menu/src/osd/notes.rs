//! The notes the OSD shows for sticks: one when a stick appears, one when
//! the helper reports an ejection (the stick can be pulled) or a removal
//! without eject (files may be lost). The sources are the contract file
//! and the event file of the removable module; the helper replaces each
//! atomically, so the directory is watched, not the files.

use std::collections::BTreeSet;
use std::ffi::{CString, OsStr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::removable::{self, Event, Stick};

/// The longest name written in a note; longer names end in "...".
pub const NAME_MAX: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// A stick appeared (its first name).
    Connected(String),
    /// Ejected: it can be pulled.
    Removable(String),
    /// Pulled without eject.
    Unclean(String),
}

/// What the OSD has seen of the files. The first update is the
/// baseline: what is there at start is not news.
#[derive(Debug, Default)]
pub struct Notes {
    /// The disks seen so far in the contract file. A disk that goes is
    /// forgotten without a note: the event file tells how it went.
    known: BTreeSet<String>,
    last_seq: Option<u64>,
    primed: bool,
}

impl Notes {
    pub fn new() -> Notes {
        Notes::default()
    }

    /// The notes the sticks and the event call for, the most important
    /// last. An event note comes once per sequence number; skipped numbers
    /// give none (eject all writes several in a row, the newest is shown).
    pub fn update(&mut self, sticks: &[Stick], event: Option<&Event>) -> Vec<Note> {
        let mut out = Vec::new();
        let fresh = event.filter(|e| self.last_seq != Some(e.seq));
        if self.primed {
            for s in sticks.iter().filter(|s| !self.known.contains(&s.disk)) {
                out.push(Note::Connected(s.label().to_string()));
            }
            if let Some(e) = fresh {
                out.push(if e.clean {
                    Note::Removable(e.name.clone())
                } else {
                    Note::Unclean(e.name.clone())
                });
            }
        }
        self.known = sticks.iter().map(|s| s.disk.clone()).collect();
        if let Some(e) = event {
            self.last_seq = Some(e.seq);
        }
        self.primed = true;
        out
    }
}

fn short(name: &str) -> String {
    if name.chars().count() > NAME_MAX {
        let cut: String = name.chars().take(NAME_MAX).collect();
        format!("{cut}...")
    } else {
        name.to_string()
    }
}

/// The note as written, and whether it is a warning.
pub fn text(n: &Note) -> (String, bool) {
    match n {
        Note::Connected(name) => (format!("{} connected", short(name)), false),
        Note::Removable(name) => (format!("{} can be removed", short(name)), false),
        Note::Unclean(name) => (
            format!("{} removed without eject \u{2014} files may be lost", short(name)),
            true,
        ),
    }
}

/// The two files, read as the menu reads them.
pub fn read_files(contract: &Path, event: &Path) -> (Vec<Stick>, Option<Event>) {
    let ev = crate::bounded::read_bounded(event, 4096, false)
        .ok()
        .and_then(|b| removable::parse_event(&String::from_utf8_lossy(&b)));
    (removable::read(contract), ev)
}

/// An inotify watch on the directories of the two files, which says
/// whether either of them was written, created, moved in or deleted.
pub struct Watcher {
    fd: OwnedFd,
    names: Vec<std::ffi::OsString>,
}

impl Watcher {
    pub fn new(files: &[&Path]) -> Result<Watcher, String> {
        // SAFETY: plain syscall; the descriptor is owned right after.
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if raw < 0 {
            return Err(format!("inotify: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: raw is a fresh descriptor nobody else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
        let mut names = Vec::new();
        for f in files {
            let dir = f
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            dirs.insert(dir.to_path_buf());
            names.extend(f.file_name().map(OsStr::to_os_string));
        }
        let mask = libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_MOVED_FROM | libc::IN_CREATE | libc::IN_DELETE;
        for dir in dirs {
            let c = CString::new(dir.as_os_str().as_bytes()).map_err(|e| format!("{}: {e}", dir.display()))?;
            // SAFETY: c is a valid C string for the call.
            let wd = unsafe { libc::inotify_add_watch(fd.as_raw_fd(), c.as_ptr(), mask) };
            if wd < 0 {
                return Err(format!("{}: {}", dir.display(), std::io::Error::last_os_error()));
            }
        }
        Ok(Watcher { fd, names })
    }

    pub fn raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }

    /// Read what is waiting; true when one of the files was touched.
    pub fn drain(&self) -> bool {
        let mut hit = false;
        let mut buf = [0u8; 4096];
        loop {
            // SAFETY: buf is valid for buf.len() bytes.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                return hit;
            }
            hit |= self.touched(&buf[..n as usize]);
        }
    }

    /// Whether the inotify records in buf name one of the files.
    fn touched(&self, buf: &[u8]) -> bool {
        const HEAD: usize = 16; // wd, mask, cookie, len: four 32 bit words
        let mut hit = false;
        let mut at = 0;
        while at + HEAD <= buf.len() {
            let len = u32::from_ne_bytes(buf[at + 12..at + 16].try_into().expect("4 bytes")) as usize;
            let Some(raw) = buf.get(at + HEAD..at + HEAD + len) else {
                break;
            };
            let name = raw.split(|&b| b == 0).next().unwrap_or(&[]);
            hit |= self.names.iter().any(|n| n.as_bytes() == name);
            at += HEAD + len;
        }
        hit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::removable::Kind;

    fn stick(disk: &str, name: &str) -> Stick {
        Stick {
            disk: disk.into(),
            names: vec![name.into()],
            size_bytes: 1,
            model: "M".into(),
            kind: Kind::Stick,
        }
    }

    fn ev(seq: u64, clean: bool) -> Event {
        Event {
            seq,
            name: "MYSTICK".into(),
            clean,
        }
    }

    #[test]
    fn first_read_is_a_baseline() {
        let mut n = Notes::new();
        assert!(
            n.update(&[stick("sdb", "A"), stick("sdc", "B")], Some(&ev(5, true)))
                .is_empty()
        );
        assert!(
            n.update(&[stick("sdb", "A"), stick("sdc", "B")], Some(&ev(5, true)))
                .is_empty()
        );
    }

    #[test]
    fn first_read_of_a_missing_file_is_empty() {
        let mut n = Notes::new();
        assert!(n.update(&[], None).is_empty());
        assert_eq!(
            n.update(&[stick("sdb", "MYSTICK")], None),
            [Note::Connected("MYSTICK".into())]
        );
        // The first event after a start without a file is news.
        assert_eq!(n.update(&[stick("sdb", "MYSTICK")], Some(&ev(1, true))).len(), 1);
    }

    #[test]
    fn new_stick_is_connected() {
        let mut n = Notes::new();
        n.update(&[stick("sdb", "A")], None);
        assert_eq!(
            n.update(&[stick("sdb", "A"), stick("sdc", "B")], None),
            [Note::Connected("B".into())]
        );
        assert!(n.update(&[stick("sdb", "A"), stick("sdc", "B")], None).is_empty());
    }

    #[test]
    fn clean_event_is_removable() {
        let mut n = Notes::new();
        n.update(&[stick("sdb", "MYSTICK")], Some(&ev(1, true)));
        assert_eq!(n.update(&[], Some(&ev(2, true))), [Note::Removable("MYSTICK".into())]);
    }

    #[test]
    fn unclean_event_is_a_warning() {
        let mut n = Notes::new();
        n.update(&[stick("sdb", "MYSTICK")], None);
        assert_eq!(n.update(&[], Some(&ev(1, false))), [Note::Unclean("MYSTICK".into())]);
    }

    #[test]
    fn same_seq_twice_is_one_note() {
        let mut n = Notes::new();
        n.update(&[], None);
        assert_eq!(n.update(&[], Some(&ev(3, true))).len(), 1);
        assert!(n.update(&[], Some(&ev(3, true))).is_empty());
    }

    #[test]
    fn skipped_seqs_give_one_note_and_the_clean_disappearance_none() {
        let mut n = Notes::new();
        n.update(&[stick("sdb", "A"), stick("sdc", "B")], Some(&ev(1, true)));
        // Eject all wrote 2, 3, 4; the OSD saw only 4, and both disks went.
        assert_eq!(n.update(&[], Some(&ev(4, true))), [Note::Removable("MYSTICK".into())]);
        // The disappearance alone is no news.
        let mut m = Notes::new();
        m.update(&[stick("sdb", "A")], Some(&ev(1, true)));
        assert!(m.update(&[], Some(&ev(1, true))).is_empty());
    }

    #[test]
    fn a_stick_and_an_event_together_end_with_the_event() {
        let mut n = Notes::new();
        n.update(&[], None);
        let got = n.update(&[stick("sdb", "A")], Some(&ev(1, false)));
        assert_eq!(got, [Note::Connected("A".into()), Note::Unclean("MYSTICK".into())]);
    }

    #[test]
    fn text_says_what_happened() {
        assert_eq!(
            text(&Note::Connected("MYSTICK".into())),
            ("MYSTICK connected".to_string(), false)
        );
        assert_eq!(
            text(&Note::Removable("MYSTICK".into())),
            ("MYSTICK can be removed".to_string(), false)
        );
        assert_eq!(
            text(&Note::Unclean("MYSTICK".into())),
            (
                "MYSTICK removed without eject \u{2014} files may be lost".to_string(),
                true
            )
        );
        let long = text(&Note::Connected("ABCDEFGHIJKLMNOPQRSTUVWXYZ".into())).0;
        assert_eq!(long, "ABCDEFGHIJKLMNOP... connected");
    }

    #[test]
    fn the_watcher_sees_the_files_arrive_by_rename_and_ignores_others() {
        let d = crate::testdir::TestDir::new("osd-notes", "watch");
        let contract = d.join("removable");
        let event = d.join("removable.event");
        let w = Watcher::new(&[&contract, &event]).unwrap();
        assert!(!w.drain());
        std::fs::write(d.join("other"), "x").unwrap();
        assert!(!w.drain(), "another name in the directory");
        std::fs::write(d.join("removable.event.tmp"), "1\tA\tclean\n").unwrap();
        assert!(!w.drain(), "the temporary name");
        std::fs::rename(d.join("removable.event.tmp"), &event).unwrap();
        assert!(w.drain(), "moved in");
        std::fs::write(&contract, "").unwrap();
        assert!(w.drain(), "written");
        std::fs::remove_file(&contract).unwrap();
        assert!(w.drain(), "deleted");
    }

    #[test]
    fn read_files_reads_both() {
        let d = crate::testdir::TestDir::new("osd-notes", "read");
        let contract = d.join("removable");
        let event = d.join("removable.event");
        let (s, e) = read_files(&contract, &event);
        assert!(s.is_empty() && e.is_none());
        std::fs::write(&contract, "/dev/sdb1\tsdb\tA\t/m\tvfat\t5\tM\tstick\n").unwrap();
        std::fs::write(&event, "7\tA\tunclean\n").unwrap();
        let (s, e) = read_files(&contract, &event);
        assert_eq!(s.len(), 1);
        assert_eq!(
            e,
            Some(Event {
                seq: 7,
                name: "A".into(),
                clean: false
            })
        );
    }
}
