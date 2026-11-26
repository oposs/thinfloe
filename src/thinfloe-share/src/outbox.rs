//! The FloeDrop outbox: one folder per AirDrop receiver in range, batching the files put
//! there into one offer, and the notes shown about it.
//!
//! The logic here is free of AirDrop and of time sources: the daemon feeds it the
//! receivers it sees, the folders inotify reports and the outcome of each send, and
//! asks it when to wake next. It does the folder operations below the outbox root itself.

use crate::names;
use crate::osd::Tone;
use crate::state::{DevState, Device};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Quiet time after the last change in a folder before its entries are offered.
pub const BATCH_DELAY: Duration = Duration::from_secs(3);
/// A receiver not seen for this long is gone; its folder is removed if empty.
pub const GONE_AFTER: Duration = Duration::from_secs(60);
/// How soon a batch held by a file still being written is looked at again.
const HOLD_RETRY: Duration = BATCH_DELAY;
/// A batch held this long gets one note; it still waits for the file, without a limit.
pub const HOLD_NOTE_AFTER: Duration = Duration::from_secs(30);
/// The write check looks this deep below an entry, and at this many entries of a batch.
pub const WALK_MAX_DEPTH: usize = 32;
pub const WALK_MAX_ENTRIES: usize = 10_000;
/// Folder name when a receiver gives no usable name.
const DEVICE_FALLBACK: &str = "iPhone";

/// How an offer ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Delivered,
    Declined,
    Failed(String),
}

/// One send, named by the transport; the daemon resolves it with `Outbox::finished`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SendHandle(pub u64);

/// The way to the receivers: AirDrop in the daemon, a recorder in the tests.
pub trait Transport {
    /// Start one offer of `paths` to the receiver `service_id`; it must not block.
    fn send(&mut self, service_id: &str, paths: Vec<PathBuf>) -> SendHandle;
}

/// Whether entries are still being written.
pub trait Probe {
    fn busy(&mut self, entries: &[PathBuf]) -> bool;
}

/// The space left for a batch: the files of an offer take as much again in Owly's
/// spool inside the same tmpfs.
pub trait Space {
    fn free(&mut self) -> Option<u64>;
}

/// The real space: what the file system holding the outbox has free.
pub struct SystemSpace(pub PathBuf);

impl Space for SystemSpace {
    fn free(&mut self) -> Option<u64> {
        crate::inbox::free_bytes(&self.0).ok()
    }
}

/// Bytes of the plain files at or below `entries` (symlinks not followed), within the
/// walk bounds of the write check.
pub fn batch_bytes(entries: &[PathBuf]) -> u64 {
    fn walk(p: &Path, depth: usize, budget: &mut usize, sum: &mut u64) {
        if depth > WALK_MAX_DEPTH || *budget == 0 {
            return;
        }
        *budget -= 1;
        let Ok(md) = fs::symlink_metadata(p) else {
            return;
        };
        if md.is_file() {
            *sum = sum.saturating_add(md.len());
        } else if md.is_dir()
            && let Ok(rd) = fs::read_dir(p)
        {
            for e in rd.flatten() {
                walk(&e.path(), depth + 1, budget, sum);
            }
        }
    }
    let mut budget = WALK_MAX_ENTRIES;
    let mut sum = 0;
    for e in entries {
        walk(e, 0, &mut budget, &mut sum);
    }
    sum
}

/// OSD text when a batch of `bytes` does not fit in FloeDrop's folder.
pub fn room_note(device: &str, bytes: u64) -> (Tone, String) {
    let mb = bytes.div_ceil(1_000_000).max(1);
    (
        Tone::Warn,
        format!("Could not send to {device}: not enough room in FloeDrop for {mb} MB"),
    )
}

/// The real check: a file below an entry changed less than `BATCH_DELAY` ago, or a
/// process of this user has one open for writing (`/proc/*/fd`).
pub struct SystemProbe;

impl Probe for SystemProbe {
    fn busy(&mut self, entries: &[PathBuf]) -> bool {
        let now = SystemTime::now();
        if entries.iter().any(|e| recently_modified(e, now)) {
            return true;
        }
        let open = open_for_writing(Path::new("/proc"));
        // /proc names the real path: an outbox reached through a symlink must match too.
        entries
            .iter()
            .map(|e| fs::canonicalize(e).unwrap_or_else(|_| e.clone()))
            .any(|e| open.iter().any(|p| p.starts_with(&e)))
    }
}

/// True if `p` or anything below it (symlinks not followed) has an mtime in the last
/// `BATCH_DELAY` before `now`. An mtime in the future does not count: it would hold forever.
/// The walk stops at `WALK_MAX_DEPTH` levels and `WALK_MAX_ENTRIES` entries; what lies
/// beyond counts as not modified (a file there still open for writing holds the batch
/// through `/proc`).
pub fn recently_modified(p: &Path, now: SystemTime) -> bool {
    let mut budget = WALK_MAX_ENTRIES;
    modified_below(p, now, 0, &mut budget)
}

fn modified_below(p: &Path, now: SystemTime, depth: usize, budget: &mut usize) -> bool {
    if depth > WALK_MAX_DEPTH || *budget == 0 {
        return false;
    }
    *budget -= 1;
    let Ok(md) = fs::symlink_metadata(p) else {
        return false;
    };
    if let Ok(m) = md.modified()
        && let Ok(age) = now.duration_since(m)
        && age < BATCH_DELAY
    {
        return true;
    }
    if md.is_dir()
        && let Ok(rd) = fs::read_dir(p)
    {
        for e in rd.flatten() {
            if modified_below(&e.path(), now, depth + 1, budget) {
                return true;
            }
            if *budget == 0 {
                break;
            }
        }
    }
    false
}

/// Paths open for writing by any process whose `fd` directory below `proc` is readable.
pub fn open_for_writing(proc: &Path) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    let Ok(pids) = fs::read_dir(proc) else {
        return out;
    };
    for pid in pids.flatten() {
        let name = pid.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(fds) = fs::read_dir(pid.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(target) = fs::read_link(fd.path()) else {
                continue;
            };
            if !target.is_absolute() {
                continue; // socket:[..], pipe:[..], anon_inode:..
            }
            let info = pid.path().join("fdinfo").join(fd.file_name());
            let Ok(text) = fs::read_to_string(info) else {
                continue;
            };
            let flags = text
                .lines()
                .find_map(|l| l.strip_prefix("flags:"))
                .and_then(|v| u32::from_str_radix(v.trim(), 8).ok());
            if let Some(f) = flags
                && f & (libc::O_ACCMODE as u32) != libc::O_RDONLY as u32
            {
                out.insert(target);
            }
        }
    }
    out
}

struct Dev {
    service_id: String,
    /// Where the receiver answered; one phone found twice (an AWDL peer, then its mDNS
    /// instance) has two service IDs but one address.
    addr: String,
    name: String,
    /// What stays the same across addresses (a hash of the receiver record), if known.
    stable: Option<String>,
    folder: String,
    last_seen: Instant,
    present: bool,
    /// Last change reported in the folder that no offer has taken up yet.
    pending: Option<Instant>,
    /// Earliest next look at a held batch.
    hold_until: Option<Instant>,
    /// When the write check first held the current batch, and whether that was noted.
    held_since: Option<Instant>,
    hold_noted: bool,
    /// Offered entries that could not be deleted: never offered again while they exist.
    stuck: HashSet<PathBuf>,
    in_flight: Option<(SendHandle, Vec<PathBuf>)>,
    state: DevState,
    since: u64,
}

pub struct Outbox {
    root: PathBuf,
    devs: Vec<Dev>,
    probe: Box<dyn Probe>,
    space: Box<dyn Space>,
    notes: Vec<(Tone, String)>,
    /// Folders made since the last `take_new_folders`, for the daemon's inotify watches.
    new_folders: Vec<String>,
    /// Service IDs no device answers to any more (re-keyed, gone or replaced), for the
    /// daemon to forget their send targets.
    dropped: Vec<String>,
}

fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn is_real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false)
}

/// The entries an offer takes: top-level, not hidden, plain files and directories only
/// (a symlink or special file is never sent), and none in `stuck`. Sorted by name.
fn offerable(dir: &Path, stuck: &HashSet<PathBuf>) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| {
            e.file_type()
                .map(|t| t.is_file() || t.is_dir())
                .unwrap_or(false)
        })
        .map(|e| e.path())
        .filter(|p| !stuck.contains(p))
        .collect();
    v.sort();
    v
}

/// OSD text when a batch has waited `HOLD_NOTE_AFTER` for a file still open.
pub fn hold_note(device: &str) -> (Tone, String) {
    (
        Tone::Warn,
        format!("Waiting to send to {device}: a file in its folder is still open"),
    )
}

/// OSD text when an offered entry could not be deleted.
pub fn stuck_note(entry: &Path, device: &str) -> (Tone, String) {
    let name = entry
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (
        Tone::Warn,
        format!("Could not remove {name} from the folder of {device}; it is not sent again"),
    )
}

/// Make the folder of a device in range again when the user deleted it; records it in
/// `made`. Anything else of that name (a file the user put there) is left alone.
fn ensure_folder(root: &Path, folder: &str, made: &mut Vec<String>) {
    let p = root.join(folder);
    match fs::symlink_metadata(&p) {
        Ok(_) => return,
        Err(e) if e.kind() != io::ErrorKind::NotFound => return,
        Err(_) => {}
    }
    match fs::create_dir_all(root).and_then(|()| fs::create_dir(&p)) {
        Ok(()) => made.push(folder.to_string()),
        Err(e) => eprintln!("thinfloe-share: outbox folder {folder}: {e}"),
    }
}

/// Delete one offered entry without following a symlink; a missing one is fine.
fn delete(p: &Path) -> io::Result<()> {
    let r = match fs::symlink_metadata(p) {
        Ok(md) if md.is_dir() => fs::remove_dir_all(p),
        Ok(_) => fs::remove_file(p),
        Err(e) => Err(e),
    };
    match r {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// OSD text after an offer ended; `offered` must still exist (a full spool counts them).
pub fn sent_note(offered: &[PathBuf], device: &str, o: &Outcome) -> (Tone, String) {
    match o {
        Outcome::Delivered => {
            let text = if let [one] = offered {
                let name = one
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("{name} sent to {device}")
            } else {
                format!("{} files sent to {device}", offered.len())
            };
            (Tone::Info, text)
        }
        Outcome::Declined => (Tone::Warn, format!("{device} declined")),
        Outcome::Failed(why) if why.contains("os error 28") || why.contains("No space left") => {
            room_note(device, batch_bytes(offered))
        }
        Outcome::Failed(_) => (Tone::Warn, format!("Could not send to {device}")),
    }
}

impl Outbox {
    /// An outbox below `root` (made at the first device), with the real write check.
    pub fn new(root: PathBuf) -> Outbox {
        Outbox::with_probe(root, Box::new(SystemProbe))
    }

    pub fn with_probe(root: PathBuf, probe: Box<dyn Probe>) -> Outbox {
        Outbox {
            space: Box::new(SystemSpace(root.clone())),
            root,
            devs: Vec::new(),
            probe,
            notes: Vec::new(),
            new_folders: Vec::new(),
            dropped: Vec::new(),
        }
    }

    /// The same outbox with another source of the free space.
    #[cfg(test)]
    pub fn with_space(mut self, space: Box<dyn Space>) -> Outbox {
        self.space = space;
        self
    }

    /// A receiver answered `/Discover` at `addr` with the computer name `name` and,
    /// when it sent a receiver record, the record's hash `stable`: give it a folder, or
    /// the folder it, its address or its name had.
    pub fn seen(
        &mut self,
        service_id: &str,
        addr: &str,
        name: &str,
        stable: Option<&str>,
        now: Instant,
    ) {
        // The same service ID, or the same phone under a new one (found by the radio's
        // peer list first, by mDNS later): one device, one folder. A record alone is
        // not enough: two devices of one Apple account may send the same one, so the
        // name must agree too.
        let known = self
            .devs
            .iter()
            .position(|d| d.service_id == service_id)
            .or_else(|| {
                stable.and_then(|s| {
                    self.devs
                        .iter()
                        .position(|d| d.stable.as_deref() == Some(s) && d.name == name)
                })
            })
            .or_else(|| self.devs.iter().position(|d| d.addr == addr));
        if let Some(d) = known.map(|i| &mut self.devs[i]) {
            if d.service_id != service_id {
                // The radio's `awdl-` key wins: it does not change with the mDNS
                // instance, so the device does not flip between two keys.
                if d.service_id.starts_with("awdl-") && !service_id.starts_with("awdl-") {
                    self.dropped.push(service_id.to_string());
                } else {
                    self.dropped
                        .push(std::mem::replace(&mut d.service_id, service_id.to_string()));
                    d.addr = addr.to_string();
                }
            }
            if stable.is_some() {
                d.stable = stable.map(str::to_string);
            }
            d.name = name.to_string();
            d.last_seen = now;
            if !d.present {
                d.present = true;
                d.pending = Some(now);
            }
            ensure_folder(&self.root, &d.folder, &mut self.new_folders);
            return;
        }
        let wanted = names::safe(name, DEVICE_FALLBACK);
        // A gone device of the same name hands its folder over.
        if let Some(d) = self
            .devs
            .iter_mut()
            .find(|d| !d.present && d.in_flight.is_none() && d.folder == wanted)
        {
            self.dropped
                .push(std::mem::replace(&mut d.service_id, service_id.to_string()));
            d.addr = addr.to_string();
            d.stable = stable.map(str::to_string);
            d.name = name.to_string();
            d.last_seen = now;
            d.present = true;
            d.pending = Some(now);
            ensure_folder(&self.root, &d.folder, &mut self.new_folders);
            return;
        }
        if let Err(e) = fs::create_dir_all(&self.root) {
            eprintln!("thinfloe-share: outbox {}: {e}", self.root.display());
            return;
        }
        let owned: HashSet<&str> = self.devs.iter().map(|d| d.folder.as_str()).collect();
        // A folder of that name nobody owns (left from an earlier run) is taken over.
        let folder = if !owned.contains(wanted.as_str()) && is_real_dir(&self.root.join(&wanted)) {
            // A new run has a new inotify instance: the daemon watches it again.
            self.new_folders.push(wanted.clone());
            wanted
        } else {
            let mut taken: Vec<String> = fs::read_dir(&self.root)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            taken.extend(owned.iter().map(|s| s.to_string()));
            let f = names::unique_dir(&taken, &wanted);
            if let Err(e) = fs::create_dir(self.root.join(&f)) {
                eprintln!("thinfloe-share: outbox folder {f}: {e}");
                return;
            }
            self.new_folders.push(f.clone());
            f
        };
        self.devs.push(Dev {
            service_id: service_id.to_string(),
            addr: addr.to_string(),
            name: name.to_string(),
            stable: stable.map(str::to_string),
            folder,
            last_seen: now,
            present: true,
            pending: Some(now),
            hold_until: None,
            held_since: None,
            hold_noted: false,
            stuck: HashSet::new(),
            in_flight: None,
            state: DevState::Idle,
            since: epoch(),
        });
    }

    /// Something changed in the folder `folder` (inotify).
    pub fn changed(&mut self, folder: &str, now: Instant) {
        if let Some(d) = self.devs.iter_mut().find(|d| d.folder == folder) {
            d.pending = Some(now);
            d.hold_until = None;
        }
    }

    /// Start the offers that are due and drop gone devices; returns when to call again.
    pub fn tick(&mut self, now: Instant, t: &mut dyn Transport) -> Option<Instant> {
        let mut wake: Option<Instant> = None;
        let mut soon = |at: Instant| {
            wake = Some(wake.map_or(at, |w: Instant| w.min(at)));
        };
        let root = self.root.clone();
        let mut i = 0;
        while i < self.devs.len() {
            let d = &mut self.devs[i];
            if d.in_flight.is_some() {
                i += 1;
                continue;
            }
            if d.present && now.duration_since(d.last_seen) >= GONE_AFTER {
                d.present = false;
                self.dropped.push(d.service_id.clone());
            }
            if !d.present {
                let id = d.service_id.clone();
                // Only an empty folder goes; one with files waits for its device.
                match fs::remove_dir(root.join(&d.folder)) {
                    Ok(()) => {
                        self.devs.remove(i);
                        self.dropped.push(id);
                        continue;
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        self.devs.remove(i);
                        self.dropped.push(id);
                        continue;
                    }
                    Err(_) => {}
                }
                i += 1;
                continue;
            }
            soon(d.last_seen + GONE_AFTER);
            ensure_folder(&root, &d.folder, &mut self.new_folders);
            d.stuck.retain(|p| fs::symlink_metadata(p).is_ok());
            if let Some(changed) = d.pending {
                let due = (changed + BATCH_DELAY).max(d.hold_until.unwrap_or(now));
                if now < due {
                    soon(due);
                } else {
                    let entries = offerable(&root.join(&d.folder), &d.stuck);
                    if entries.is_empty() {
                        d.pending = None;
                        d.held_since = None;
                        d.hold_noted = false;
                    } else if self.probe.busy(&entries) {
                        let since = *d.held_since.get_or_insert(now);
                        if !d.hold_noted && now.duration_since(since) >= HOLD_NOTE_AFTER {
                            d.hold_noted = true;
                            self.notes.push(hold_note(&d.name));
                        }
                        d.hold_until = Some(now + HOLD_RETRY);
                        soon(now + HOLD_RETRY);
                    } else if let Some(free) = self.space.free()
                        && let bytes = batch_bytes(&entries)
                        && bytes > free
                    {
                        // Owly's archive of the batch needs as much again: it would fail
                        // on the way. The files stay for a later change.
                        d.pending = None;
                        d.held_since = None;
                        d.hold_noted = false;
                        d.state = DevState::Failed;
                        d.since = epoch();
                        self.notes.push(room_note(&d.name, bytes));
                    } else {
                        let h = t.send(&d.service_id, entries.clone());
                        d.in_flight = Some((h, entries));
                        d.pending = None;
                        d.hold_until = None;
                        d.held_since = None;
                        d.hold_noted = false;
                        d.state = DevState::Sending;
                        d.since = epoch();
                    }
                }
            }
            i += 1;
        }
        wake
    }

    /// A send ended: its offered entries are deleted whatever the outcome; one that cannot
    /// be deleted is noted and not offered again.
    pub fn finished(&mut self, id: SendHandle, o: Outcome, now: Instant) {
        let Some(d) = self
            .devs
            .iter_mut()
            .find(|d| d.in_flight.as_ref().is_some_and(|(h, _)| *h == id))
        else {
            return;
        };
        let (_, offered) = d.in_flight.take().expect("matched above");
        let note = sent_note(&offered, &d.name, &o);
        let mut stuck = Vec::new();
        for p in &offered {
            if let Err(e) = delete(p) {
                eprintln!("thinfloe-share: delete {}: {e}", p.display());
                d.stuck.insert(p.clone());
                stuck.push(stuck_note(p, &d.name));
            }
        }
        if let Outcome::Failed(why) = &o {
            eprintln!("thinfloe-share: send to {} failed: {why}", d.name);
        }
        d.state = match o {
            Outcome::Delivered => DevState::Idle,
            Outcome::Declined => DevState::Declined,
            Outcome::Failed(_) => DevState::Failed,
        };
        d.since = epoch();
        // An answer from the phone is a sign of life; a long transfer does not make it gone.
        if !matches!(o, Outcome::Failed(_)) {
            d.last_seen = d.last_seen.max(now);
        }
        self.notes.push(note);
        self.notes.extend(stuck);
    }

    /// Folders made since the last call (new devices and folders made again after the
    /// user deleted them): the daemon adds an inotify watch for each.
    pub fn take_new_folders(&mut self) -> Vec<String> {
        std::mem::take(&mut self.new_folders)
    }

    /// The service IDs that stopped naming a receiver since the last call.
    pub fn take_dropped(&mut self) -> Vec<String> {
        std::mem::take(&mut self.dropped)
    }

    /// The notes since the last call, for the OSD.
    pub fn take_notes(&mut self) -> Vec<(Tone, String)> {
        std::mem::take(&mut self.notes)
    }

    /// The receivers in range, for the state file.
    pub fn devices(&self) -> Vec<Device> {
        self.devs
            .iter()
            .filter(|d| d.present)
            .map(|d| Device {
                name: d.name.clone(),
                folder: d.folder.clone(),
                state: d.state,
                since: d.since,
            })
            .collect()
    }

    /// The daemon stops: empty device folders go, folders with files stay.
    pub fn shutdown(&mut self) {
        for d in self.devs.drain(..) {
            let _ = fs::remove_dir(self.root.join(&d.folder));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::os::unix::fs::symlink;
    use std::rc::Rc;

    struct Dir(PathBuf);
    impl Dir {
        fn new(name: &str) -> Dir {
            let p = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-tmp"))
                .join(format!("{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// One recorded offer: the service ID and the paths.
    type Call = (String, Vec<PathBuf>);

    #[derive(Default)]
    struct Fake {
        calls: Vec<Call>,
    }
    impl Transport for Fake {
        fn send(&mut self, service_id: &str, paths: Vec<PathBuf>) -> SendHandle {
            self.calls.push((service_id.to_string(), paths));
            SendHandle(self.calls.len() as u64)
        }
    }

    struct FakeProbe(Rc<Cell<bool>>);
    impl Probe for FakeProbe {
        fn busy(&mut self, _: &[PathBuf]) -> bool {
            self.0.get()
        }
    }

    struct T {
        _dir: Dir,
        root: PathBuf,
        ob: Outbox,
        tr: Fake,
        busy: Rc<Cell<bool>>,
        t0: Instant,
    }
    impl T {
        fn new(name: &str) -> T {
            let dir = Dir::new(name);
            let root = dir.0.join("outbox");
            let busy = Rc::new(Cell::new(false));
            let ob = Outbox::with_probe(root.clone(), Box::new(FakeProbe(busy.clone())));
            T {
                _dir: dir,
                root,
                ob,
                tr: Fake::default(),
                busy,
                t0: Instant::now(),
            }
        }
        fn at(&self, s: u64) -> Instant {
            self.t0 + Duration::from_secs(s)
        }
        /// Seen at an address of its own, derived from `id`.
        fn seen(&mut self, id: &str, name: &str, s: u64) {
            self.seen_at(id, &format!("addr-{id}"), name, s);
        }
        fn seen_at(&mut self, id: &str, addr: &str, name: &str, s: u64) {
            self.seen_with(id, addr, name, None, s);
        }
        fn seen_with(&mut self, id: &str, addr: &str, name: &str, stable: Option<&str>, s: u64) {
            let now = self.at(s);
            self.ob.seen(id, addr, name, stable, now);
        }
        fn folders(&self) -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(&self.root)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
        fn changed(&mut self, folder: &str, s: u64) {
            let now = self.at(s);
            self.ob.changed(folder, now);
        }
        fn tick(&mut self, s: u64) -> Option<Instant> {
            let now = self.at(s);
            self.ob.tick(now, &mut self.tr)
        }
        fn finished(&mut self, h: u64, o: Outcome, s: u64) {
            let now = self.at(s);
            self.ob.finished(SendHandle(h), o, now);
        }
        fn put(&self, folder: &str, name: &str) -> PathBuf {
            let p = self.root.join(folder).join(name);
            fs::write(&p, b"x").unwrap();
            p
        }
        fn calls(&self) -> Vec<Call> {
            self.tr.calls.clone()
        }
        fn entries(&self, folder: &str) -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(self.root.join(folder))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }

    const JANE: &str = "Jane's iPhone";

    #[test]
    fn an_awdl_peer_and_its_mdns_instance_share_one_folder() {
        let mut t = T::new("ob-rekey");
        // The radio's peer list first, then mDNS finds the same phone.
        t.seen_at("awdl-02:11:22:33:44:55", "fe80::11:22ff:fe33:4455", JANE, 0);
        t.tick(0);
        let early = t.put(JANE, "early.jpg");
        t.seen_at("Jane-mdns-instance", "fe80::11:22ff:fe33:4455", JANE, 1);
        let late = t.put(JANE, "late.jpg");
        t.changed(JANE, 1);
        t.tick(1);
        let mut dirs: Vec<String> = fs::read_dir(&t.root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        dirs.sort();
        assert_eq!(dirs, vec![JANE.to_string()], "one folder, never \"(2)\"");
        assert_eq!(t.entries(JANE), vec!["early.jpg", "late.jpg"]);
        assert_eq!(t.ob.devices().len(), 1);
        // The batch goes to the radio's key, files from before the mDNS find too.
        t.tick(4);
        assert_eq!(
            t.calls(),
            vec![("awdl-02:11:22:33:44:55".to_string(), vec![early, late])]
        );
    }

    #[test]
    fn the_awdl_key_wins_over_an_mdns_key() {
        let mut t = T::new("ob-awdl-wins");
        let awdl = "awdl-02:11:22:33:44:55";
        t.seen_at(awdl, "fe80::1", JANE, 0);
        t.seen_at("Jane-mdns", "fe80::1", JANE, 1);
        t.seen_at(awdl, "fe80::1", JANE, 2);
        t.seen_at("Jane-mdns", "fe80::1", JANE, 3);
        assert_eq!(t.ob.take_dropped(), vec!["Jane-mdns", "Jane-mdns"]);
        let a = t.put(JANE, "a.jpg");
        t.changed(JANE, 3);
        t.tick(6);
        assert_eq!(t.calls(), vec![(awdl.to_string(), vec![a])]);
    }

    #[test]
    fn one_record_is_one_phone_at_two_addresses() {
        let mut t = T::new("ob-record");
        t.seen_with("awdl-a", "fe80::1", JANE, Some("r1"), 0);
        t.seen_with("Jane-mdns", "fe80::99", JANE, Some("r1"), 1);
        assert_eq!(t.folders(), vec![JANE.to_string()]);
        assert_eq!(t.ob.devices().len(), 1);
        // The same record under another name is another device (one Apple account,
        // two devices).
        t.seen_with("Jane-ipad", "fe80::7", "Jane's iPad", Some("r1"), 2);
        assert_eq!(t.ob.devices().len(), 2);
    }

    #[test]
    fn two_phones_of_the_same_name_keep_two_folders() {
        let mut t = T::new("ob-same-name");
        t.seen_at("p1", "fe80::1", "iPhone", 0);
        t.seen_at("p2", "fe80::2", "iPhone", 0);
        t.seen_at("p1", "fe80::1", "iPhone", 1);
        t.seen_at("p2", "fe80::2", "iPhone", 1);
        assert_eq!(t.folders(), vec!["iPhone", "iPhone (2)"]);
        assert_eq!(t.ob.devices().len(), 2);
        assert!(t.ob.take_dropped().is_empty());
    }

    /// Jane is seen at 0 and her three files are offered at 3 (handle 1).
    fn jane_sends_three(name: &str) -> (T, Vec<PathBuf>) {
        let mut t = T::new(name);
        t.seen("id1", JANE, 0);
        let files = vec![
            t.put(JANE, "a.jpg"),
            t.put(JANE, "b.jpg"),
            t.put(JANE, "c.jpg"),
        ];
        t.changed(JANE, 0);
        t.tick(3);
        assert_eq!(t.calls().len(), 1);
        (t, files)
    }

    #[test]
    fn a_seen_device_gets_a_folder() {
        let mut t = T::new("ob-seen");
        t.seen("id1", JANE, 0);
        assert!(t.root.join(JANE).is_dir());
        let d = t.ob.devices();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].name, JANE);
        assert_eq!(d[0].folder, JANE);
        assert_eq!(d[0].state, DevState::Idle);
    }

    #[test]
    fn a_hostile_name_stays_one_folder() {
        let mut t = T::new("ob-hostile");
        t.seen("id1", "../etc", 0);
        t.seen("id2", "", 0);
        assert!(t.root.join(".._etc").is_dir() || t.root.join("_._etc").is_dir());
        assert!(t.root.join("iPhone").is_dir());
        assert_eq!(fs::read_dir(&t.root).unwrap().count(), 2);
    }

    #[test]
    fn folders_follow_the_service_id() {
        let mut t = T::new("ob-sid");
        t.seen("id1", "iPhone", 0);
        t.seen("id2", "iPhone", 0);
        assert!(t.root.join("iPhone (2)").is_dir());
        // A renamed device keeps its folder.
        t.seen("id1", "Renamed", 1);
        t.put("iPhone (2)", "a.jpg");
        t.changed("iPhone (2)", 1);
        t.put("iPhone", "b.jpg");
        t.changed("iPhone", 1);
        t.tick(4);
        let mut c = t.calls();
        c.sort();
        assert_eq!(c[0], ("id1".into(), vec![t.root.join("iPhone/b.jpg")]));
        assert_eq!(c[1], ("id2".into(), vec![t.root.join("iPhone (2)/a.jpg")]));
        let names: Vec<String> = t.ob.devices().into_iter().map(|d| d.name).collect();
        assert!(names.contains(&"Renamed".to_string()));
    }

    #[test]
    fn three_files_go_as_one_offer_after_3s() {
        let mut t = T::new("ob-three");
        t.seen("id1", JANE, 0);
        let files = vec![
            t.put(JANE, "a.jpg"),
            t.put(JANE, "b.jpg"),
            t.put(JANE, "c.jpg"),
        ];
        t.changed(JANE, 0);
        assert_eq!(t.tick(2), Some(t.at(3)));
        assert!(t.calls().is_empty());
        t.tick(3);
        assert_eq!(t.calls(), vec![("id1".to_string(), files)]);
        assert_eq!(t.ob.devices()[0].state, DevState::Sending);
    }

    #[test]
    fn a_folder_goes_whole_in_the_offer() {
        let mut t = T::new("ob-dir");
        t.seen("id1", JANE, 0);
        fs::create_dir(t.root.join(JANE).join("trip")).unwrap();
        t.put(JANE, "trip/x.jpg");
        t.changed(JANE, 0);
        t.tick(3);
        assert_eq!(t.calls()[0].1, vec![t.root.join(JANE).join("trip")]);
    }

    #[test]
    fn a_change_restarts_the_timer() {
        let mut t = T::new("ob-restart");
        t.seen("id1", JANE, 0);
        t.put(JANE, "a.jpg");
        t.changed(JANE, 0);
        t.put(JANE, "b.jpg");
        t.changed(JANE, 2);
        assert_eq!(t.tick(3), Some(t.at(5)));
        assert!(t.calls().is_empty());
        t.tick(5);
        assert_eq!(t.calls().len(), 1);
        assert_eq!(t.calls()[0].1.len(), 2);
    }

    #[test]
    fn a_file_still_growing_holds_the_batch() {
        let mut t = T::new("ob-growing");
        t.seen("id1", JANE, 0);
        t.put(JANE, "a.jpg");
        t.changed(JANE, 0);
        t.busy.set(true);
        let wake = t.tick(3).expect("a held batch wakes again");
        assert!(wake > t.at(3) && wake <= t.at(6));
        assert!(t.calls().is_empty());
        t.tick(6);
        assert!(t.calls().is_empty());
        t.busy.set(false);
        t.tick(9);
        assert_eq!(t.calls().len(), 1);
    }

    #[test]
    fn a_long_hold_is_noted_once() {
        let mut t = T::new("ob-long-hold");
        t.seen("id1", JANE, 0);
        t.put(JANE, "a.jpg");
        t.changed(JANE, 0);
        t.busy.set(true);
        for s in 3..=32 {
            t.tick(s);
            assert!(t.ob.take_notes().is_empty(), "no note at {s}");
        }
        t.tick(33);
        assert_eq!(
            t.ob.take_notes(),
            vec![(
                Tone::Warn,
                "Waiting to send to Jane's iPhone: a file in its folder is still open".to_string()
            )]
        );
        for s in 34..=63 {
            if s % 20 == 0 {
                t.seen("id1", JANE, s); // still in range
            }
            t.tick(s);
        }
        assert!(t.ob.take_notes().is_empty(), "noted once");
        assert!(t.calls().is_empty());
        t.busy.set(false);
        t.tick(66);
        assert_eq!(t.calls().len(), 1);
    }

    #[test]
    fn an_entry_that_cannot_be_deleted_is_not_offered_again() {
        use std::os::unix::fs::PermissionsExt;
        let mut t = T::new("ob-stuck");
        t.seen("id1", JANE, 0);
        let d = t.root.join(JANE).join("d");
        fs::create_dir_all(d.join("sub")).unwrap();
        t.put(JANE, "d/sub/f");
        fs::set_permissions(d.join("sub"), fs::Permissions::from_mode(0o500)).unwrap();
        t.changed(JANE, 0);
        t.tick(3);
        assert_eq!(t.calls(), vec![("id1".to_string(), vec![d.clone()])]);
        t.finished(1, Outcome::Delivered, 4);
        let notes = t.ob.take_notes();
        assert!(
            notes.contains(&(
                Tone::Warn,
                "Could not remove d from the folder of Jane's iPhone; it is not sent again"
                    .to_string()
            )),
            "{notes:?}"
        );
        let b = t.put(JANE, "b.jpg");
        t.changed(JANE, 5);
        t.tick(8);
        assert_eq!(t.calls().len(), 2);
        assert_eq!(t.calls()[1].1, vec![b]);
        fs::set_permissions(d.join("sub"), fs::Permissions::from_mode(0o755)).unwrap();
        // Once the entry is gone, a new one of that name is offered again.
        t.finished(2, Outcome::Delivered, 9);
        fs::remove_dir_all(&d).unwrap();
        t.tick(10);
        fs::create_dir(&d).unwrap();
        t.put(JANE, "d/g");
        t.changed(JANE, 11);
        t.tick(14);
        assert_eq!(t.calls()[2].1, vec![d]);
    }

    struct FakeSpace(Rc<Cell<Option<u64>>>);
    impl Space for FakeSpace {
        fn free(&mut self) -> Option<u64> {
            self.0.get()
        }
    }

    #[test]
    fn a_batch_that_does_not_fit_is_kept_and_noted() {
        let mut t = T::new("ob-room");
        let free = Rc::new(Cell::new(Some(10)));
        let busy = t.busy.clone();
        t.ob = Outbox::with_probe(t.root.clone(), Box::new(FakeProbe(busy)))
            .with_space(Box::new(FakeSpace(free.clone())));
        t.seen("id1", JANE, 0);
        for n in ["a", "b", "c"] {
            t.put(JANE, n);
        }
        fs::write(t.root.join(JANE).join("big"), [0u8; 20]).unwrap();
        t.changed(JANE, 0);
        t.tick(3);
        assert!(t.calls().is_empty());
        assert_eq!(
            t.ob.take_notes(),
            vec![(
                Tone::Warn,
                "Could not send to Jane's iPhone: not enough room in FloeDrop for 1 MB".to_string()
            )]
        );
        assert_eq!(t.entries(JANE), vec!["a", "b", "big", "c"]);
        assert_eq!(t.ob.devices()[0].state, DevState::Failed);
        t.tick(10);
        assert!(t.calls().is_empty(), "not tried again without a change");
        free.set(Some(1 << 30));
        t.changed(JANE, 11);
        t.tick(14);
        assert_eq!(t.calls().len(), 1);
        assert_eq!(t.calls()[0].1.len(), 4);
    }

    #[test]
    fn a_full_spool_says_why() {
        let d = Dir::new("ob-full-spool");
        let a = d.0.join("a");
        fs::write(&a, vec![0u8; 2_500_000]).unwrap();
        assert_eq!(
            sent_note(
                std::slice::from_ref(&a),
                JANE,
                &Outcome::Failed("No space left on device (os error 28)".into())
            ),
            room_note(JANE, 2_500_000)
        );
        assert_eq!(room_note(JANE, 2_500_000).1, format!("Could not send to {JANE}: not enough room in FloeDrop for 3 MB"));
        assert_eq!(
            sent_note(&[a], JANE, &Outcome::Failed("link lost".into())),
            (Tone::Warn, format!("Could not send to {JANE}"))
        );
    }

    #[test]
    fn room_note_rounds_up() {
        assert!(room_note("X", 1_000_001).1.ends_with("for 2 MB"));
        assert!(room_note("X", 1_000_000).1.ends_with("for 1 MB"));
        assert!(room_note("X", 1).1.ends_with("for 1 MB"));
    }

    #[test]
    fn batch_bytes_counts_files_below_and_skips_symlinks() {
        let d = Dir::new("ob-bytes");
        fs::create_dir_all(d.0.join("t/u")).unwrap();
        fs::write(d.0.join("t/u/x"), [0u8; 7]).unwrap();
        fs::write(d.0.join("y"), [0u8; 5]).unwrap();
        symlink(d.0.join("y"), d.0.join("t/l")).unwrap();
        assert_eq!(batch_bytes(&[d.0.join("t"), d.0.join("y")]), 12);
    }

    #[test]
    fn a_deep_tree_stops_the_walk() {
        let dir = Dir::new("ob-deep");
        let old = SystemTime::now() - Duration::from_secs(10);
        let chain = |top: &Path, depth: usize| {
            let mut p = top.to_path_buf();
            let mut dirs = vec![p.clone()];
            for _ in 0..depth {
                p = p.join("d");
                dirs.push(p.clone());
            }
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("f"), b"x").unwrap();
            for d in dirs {
                fs::File::open(&d).unwrap().set_modified(old).unwrap();
            }
        };
        let deep = dir.0.join("deep");
        chain(&deep, 40);
        assert!(!recently_modified(&deep, SystemTime::now()));
        let shallow = dir.0.join("shallow");
        chain(&shallow, 5);
        assert!(recently_modified(&shallow, SystemTime::now()));
    }

    #[test]
    fn busy_follows_a_symlinked_root() {
        let d = Dir::new("ob-link-root");
        let real = d.0.join("real");
        fs::create_dir(&real).unwrap();
        let x = real.join("x");
        fs::write(&x, b"x").unwrap();
        let old = SystemTime::now() - Duration::from_secs(10);
        fs::File::open(&x).unwrap().set_modified(old).unwrap();
        let link = d.0.join("link");
        symlink(&real, &link).unwrap();
        let through = link.join("x");
        assert!(!SystemProbe.busy(std::slice::from_ref(&through)));
        let w = fs::OpenOptions::new().append(true).open(&x).unwrap();
        assert!(SystemProbe.busy(std::slice::from_ref(&through)));
        drop(w);
    }

    #[test]
    fn a_recently_written_file_is_busy() {
        let d = Dir::new("ob-mtime");
        let dir = d.0.join("trip");
        fs::create_dir(&dir).unwrap();
        let f = dir.join("x.jpg");
        fs::write(&f, b"x").unwrap();
        let old = SystemTime::now() - Duration::from_secs(10);
        let set_old = |p: &Path| {
            fs::File::open(p).unwrap().set_modified(old).unwrap();
        };
        set_old(&dir);
        // The file below the folder is fresh: the folder is busy.
        assert!(recently_modified(&dir, SystemTime::now()));
        set_old(&f);
        assert!(!recently_modified(&dir, SystemTime::now()));
        assert!(!SystemProbe.busy(std::slice::from_ref(&dir)));
        // An mtime in the future does not hold the batch forever.
        fs::File::open(&f)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(3600))
            .unwrap();
        assert!(!recently_modified(&f, SystemTime::now()));
    }

    #[test]
    fn a_file_open_for_writing_is_busy() {
        let d = Dir::new("ob-open");
        let f = d.0.join("x.jpg");
        fs::write(&f, b"x").unwrap();
        let old = SystemTime::now() - Duration::from_secs(10);
        fs::File::open(&f).unwrap().set_modified(old).unwrap();
        let ro = fs::File::open(&f).unwrap();
        assert!(!open_for_writing(Path::new("/proc")).contains(&f));
        assert!(!SystemProbe.busy(std::slice::from_ref(&f)));
        let w = fs::OpenOptions::new().append(true).open(&f).unwrap();
        assert!(open_for_writing(Path::new("/proc")).contains(&f));
        assert!(SystemProbe.busy(std::slice::from_ref(&f)));
        assert!(SystemProbe.busy(std::slice::from_ref(&d.0)));
        drop(w);
        drop(ro);
    }

    #[test]
    fn hidden_entries_are_left_out() {
        let mut t = T::new("ob-hidden");
        t.seen("id1", JANE, 0);
        t.put(JANE, ".DS_Store");
        t.changed(JANE, 0);
        t.tick(3);
        assert!(t.calls().is_empty());
        let a = t.put(JANE, "a.jpg");
        t.changed(JANE, 4);
        t.tick(7);
        assert_eq!(t.calls()[0].1, vec![a]);
    }

    #[test]
    fn a_symlink_is_never_sent() {
        let mut t = T::new("ob-symlink");
        t.seen("id1", JANE, 0);
        let secret = t._dir.0.join("secret");
        fs::write(&secret, b"s").unwrap();
        symlink(&secret, t.root.join(JANE).join("link")).unwrap();
        symlink(t._dir.0.clone(), t.root.join(JANE).join("dirlink")).unwrap();
        let a = t.put(JANE, "a.jpg");
        t.changed(JANE, 0);
        t.tick(3);
        assert_eq!(t.calls()[0].1, vec![a]);
        t.finished(1, Outcome::Delivered, 4);
        // Left where it is, and its target untouched.
        assert_eq!(t.entries(JANE), vec!["dirlink", "link"]);
        assert!(secret.exists());
    }

    #[test]
    fn delivered_entries_are_deleted() {
        let (mut t, _) = jane_sends_three("ob-delivered");
        t.finished(1, Outcome::Delivered, 5);
        assert!(t.entries(JANE).is_empty());
        assert_eq!(
            t.ob.take_notes(),
            vec![(Tone::Info, "3 files sent to Jane's iPhone".to_string())]
        );
        assert!(t.ob.take_notes().is_empty());
        assert_eq!(t.ob.devices()[0].state, DevState::Idle);
        // No second offer without a change.
        t.tick(20);
        assert_eq!(t.calls().len(), 1);
    }

    #[test]
    fn one_file_names_itself_in_the_note() {
        let mut t = T::new("ob-one");
        t.seen("id1", JANE, 0);
        t.put(JANE, "IMG_0042.HEIC");
        t.changed(JANE, 0);
        t.tick(3);
        t.finished(1, Outcome::Delivered, 4);
        assert_eq!(
            t.ob.take_notes(),
            vec![(
                Tone::Info,
                "IMG_0042.HEIC sent to Jane's iPhone".to_string()
            )]
        );
    }

    #[test]
    fn declined_entries_are_deleted_too() {
        let (mut t, _) = jane_sends_three("ob-declined");
        fs::create_dir(t.root.join(JANE).join("never-offered-dir")).ok();
        t.finished(1, Outcome::Declined, 5);
        assert_eq!(t.entries(JANE), vec!["never-offered-dir"]);
        assert_eq!(
            t.ob.take_notes(),
            vec![(Tone::Warn, "Jane's iPhone declined".to_string())]
        );
        assert_eq!(t.ob.devices()[0].state, DevState::Declined);
    }

    #[test]
    fn failed_entries_are_deleted_too() {
        let (mut t, _) = jane_sends_three("ob-failed");
        t.finished(1, Outcome::Failed("link lost".into()), 5);
        assert!(t.entries(JANE).is_empty());
        assert_eq!(
            t.ob.take_notes(),
            vec![(Tone::Warn, "Could not send to Jane's iPhone".to_string())]
        );
        assert_eq!(t.ob.devices()[0].state, DevState::Failed);
    }

    #[test]
    fn a_new_file_during_a_send_waits_for_the_next_batch() {
        let (mut t, _) = jane_sends_three("ob-during");
        let d = t.put(JANE, "d.jpg");
        t.changed(JANE, 4);
        t.tick(8);
        assert_eq!(t.calls().len(), 1, "one send in flight per device");
        t.finished(1, Outcome::Delivered, 9);
        assert_eq!(t.entries(JANE), vec!["d.jpg"]);
        t.tick(9);
        assert_eq!(t.calls().len(), 2);
        assert_eq!(t.calls()[1].1, vec![d]);
    }

    #[test]
    fn a_gone_device_loses_its_empty_folder_after_60s() {
        let mut t = T::new("ob-gone");
        t.seen("id1", JANE, 0);
        assert_eq!(t.tick(30), Some(t.at(60)));
        t.seen("id1", JANE, 30);
        t.tick(60);
        assert!(t.root.join(JANE).is_dir(), "seen again at 30");
        t.tick(89);
        assert!(t.root.join(JANE).is_dir());
        t.tick(90);
        assert!(!t.root.join(JANE).exists());
        assert!(t.ob.devices().is_empty());
    }

    #[test]
    fn a_rekey_drops_the_old_service_id() {
        let mut t = T::new("ob-drop-rekey");
        t.seen_at("Jane-mdns-instance", "fe80::1", JANE, 0);
        assert!(t.ob.take_dropped().is_empty());
        t.seen_at("awdl-02:11:22:33:44:55", "fe80::1", JANE, 1);
        assert_eq!(t.ob.take_dropped(), vec!["Jane-mdns-instance"]);
    }

    #[test]
    fn a_gone_device_drops_its_service_id() {
        let mut t = T::new("ob-drop-gone");
        t.seen("id1", JANE, 0);
        t.tick(59);
        assert!(t.ob.take_dropped().is_empty());
        t.tick(60);
        assert_eq!(t.ob.take_dropped(), vec!["id1", "id1"]);
    }

    #[test]
    fn a_gone_device_keeps_a_folder_with_files() {
        let mut t = T::new("ob-keep");
        t.seen("id1", JANE, 0);
        t.tick(3);
        t.put(JANE, "a.jpg");
        t.tick(60);
        assert_eq!(t.entries(JANE), vec!["a.jpg"]);
        assert!(t.ob.devices().is_empty(), "not in range");
        // A file for a device out of range waits.
        t.changed(JANE, 61);
        t.tick(70);
        assert!(t.calls().is_empty());
    }

    #[test]
    fn a_returning_name_reuses_its_folder() {
        let mut t = T::new("ob-return");
        t.seen("id1", JANE, 0);
        t.tick(3);
        let a = t.put(JANE, "a.jpg");
        t.tick(60);
        t.changed(JANE, 61);
        t.seen("id9", JANE, 70);
        assert_eq!(fs::read_dir(&t.root).unwrap().count(), 1, "no Jane (2)");
        assert_eq!(t.ob.devices()[0].folder, JANE);
        // Back in range: the 3 s rule applies from the return.
        t.tick(72);
        assert!(t.calls().is_empty());
        t.tick(73);
        assert_eq!(t.calls(), vec![("id9".to_string(), vec![a])]);
    }

    #[test]
    fn a_folder_left_from_an_earlier_run_is_taken_over() {
        let mut t = T::new("ob-left");
        fs::create_dir_all(t.root.join(JANE)).unwrap();
        let a = t.put(JANE, "a.jpg");
        t.seen("id1", JANE, 0);
        assert_eq!(t.ob.devices()[0].folder, JANE);
        t.tick(3);
        assert_eq!(t.calls(), vec![("id1".to_string(), vec![a])]);
    }

    #[test]
    fn a_taken_over_folder_is_listed_for_watching() {
        // A new run (new inotify instance) must watch a folder an earlier run left.
        let mut t = T::new("ob-left-watch");
        fs::create_dir_all(t.root.join(JANE)).unwrap();
        t.put(JANE, "a.jpg");
        t.seen("id1", JANE, 0);
        assert_eq!(t.ob.take_new_folders(), vec![JANE.to_string()]);
    }

    #[test]
    fn a_deleted_folder_comes_back() {
        let mut t = T::new("ob-deleted");
        t.seen("id1", JANE, 0);
        assert_eq!(t.ob.take_new_folders(), vec![JANE.to_string()]);
        assert!(t.ob.take_new_folders().is_empty());
        // Deleted in the remote file manager while the phone stays in range.
        fs::remove_dir(t.root.join(JANE)).unwrap();
        t.seen("id1", JANE, 5);
        assert!(t.root.join(JANE).is_dir());
        assert_eq!(t.ob.take_new_folders(), vec![JANE.to_string()]);
        fs::remove_dir(t.root.join(JANE)).unwrap();
        t.tick(6);
        assert!(t.root.join(JANE).is_dir(), "tick makes it again too");
        assert_eq!(t.ob.take_new_folders(), vec![JANE.to_string()]);
        t.seen("id1", JANE, 7);
        assert!(t.ob.take_new_folders().is_empty(), "nothing made twice");
        let a = t.put(JANE, "a.jpg");
        t.changed(JANE, 7);
        t.tick(10);
        assert_eq!(t.calls(), vec![("id1".to_string(), vec![a])]);
    }

    #[test]
    fn a_handed_over_folder_is_made_again() {
        let mut t = T::new("ob-handover");
        t.seen("id1", JANE, 0);
        t.tick(3);
        t.put(JANE, "a.jpg");
        t.tick(60);
        // Gone with files; the folder vanishes before the next tick sees it.
        fs::remove_dir_all(t.root.join(JANE)).unwrap();
        t.ob.take_new_folders();
        t.seen("id9", JANE, 61);
        assert!(t.root.join(JANE).is_dir());
        assert_eq!(t.ob.take_new_folders(), vec![JANE.to_string()]);
    }

    #[test]
    fn shutdown_keeps_folders_with_files() {
        let mut t = T::new("ob-shutdown");
        t.seen("id1", JANE, 0);
        t.seen("id2", "Bob's iPhone", 0);
        t.put(JANE, "a.jpg");
        t.ob.shutdown();
        assert_eq!(t.entries(JANE), vec!["a.jpg"]);
        assert!(!t.root.join("Bob's iPhone").exists());
        assert!(t.ob.devices().is_empty());
    }
}
