//! Volume and microphone mute through `pactl` (pipewire-pulse), on every
//! speaker (sink) and microphone (source) at once. The keys run on a
//! worker thread (`AudioWorker`) that owns the shared state (`sync`), so
//! a slow `pactl` never stalls the OSD; presses that arrive while it runs
//! are merged.

use std::collections::{BTreeSet, VecDeque};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::sync::{Mode, Set, Shared};

/// How long one `pactl` call may take.
const TIMEOUT: Duration = Duration::from_secs(3);
/// The first wait before `pactl subscribe` starts again; it doubles up to
/// `WATCH_CAP`.
const WATCH_BASE: Duration = Duration::from_secs(1);
/// The longest wait before `pactl subscribe` starts again; a run that
/// lasted this long counts as working, and the wait drops back.
const WATCH_CAP: Duration = Duration::from_secs(30);
/// The longest line of `pactl subscribe` kept; a longer one is dropped.
const WATCH_LINE: usize = 4096;
/// How long a restarted `pactl subscribe` must stay up, without a line,
/// to count as connected: one without a server ends at once.
const WATCH_UP: Duration = Duration::from_millis(500);
/// How long dropping a `Watcher` waits for its thread.
const WATCH_JOIN: Duration = Duration::from_secs(2);

/// The shared speaker level and mute state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub percent: u8,
    pub muted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sink,
    Source,
}

impl Kind {
    /// The word `pactl` uses: `set-sink-volume`, `get-default-source`.
    fn word(self) -> &'static str {
        match self {
            Kind::Sink => "sink",
            Kind::Source => "source",
        }
    }
}

/// One sink or source as `pactl -f json list` shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub kind: Kind,
    pub index: u32,
    pub name: String,
    /// The rounded mean of the channels.
    pub percent: u8,
    pub muted: bool,
}

/// The array `pactl -f json list sinks|sources` prints. A source that is
/// the monitor of a sink is left out: it is a copy of what the sink
/// plays, not a microphone. pactl 17 names that sink under
/// `monitor_source` in a source's entry (empty for a microphone); other
/// versions under `monitor_of_sink` (`n/a` for none); either way its
/// `device.class` property is `monitor`. An entry without a
/// name, index or channel volume is left out, not the whole list.
pub fn parse_devices(kind: Kind, json: &str) -> Result<Vec<Device>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("pactl printed no device list: {e}"))?;
    let list = v.as_array().ok_or("pactl printed no device list")?;
    Ok(list.iter().filter_map(|d| device(kind, d)).collect())
}

fn device(kind: Kind, d: &Value) -> Option<Device> {
    let monitor_of = |key: &str| {
        d.get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty() && s != "n/a")
    };
    let class = d.pointer("/properties/device.class").and_then(Value::as_str);
    if kind == Kind::Source
        && (monitor_of("monitor_of_sink") || monitor_of("monitor_source") || class == Some("monitor"))
    {
        return None;
    }
    let percents: Vec<f64> = d
        .get("volume")?
        .as_object()?
        .values()
        .filter_map(|c| {
            c.get("value_percent")?
                .as_str()?
                .trim()
                .strip_suffix('%')?
                .trim()
                .parse()
                .ok()
        })
        .filter(|p: &f64| p.is_finite() && *p >= 0.0)
        .collect();
    if percents.is_empty() {
        return None;
    }
    let mean = percents.iter().sum::<f64>() / percents.len() as f64;
    Some(Device {
        kind,
        index: u32::try_from(d.get("index")?.as_u64()?).ok()?,
        name: d.get("name")?.as_str()?.to_string(),
        percent: mean.round().min(f64::from(u8::MAX)) as u8,
        muted: d.get("mute")?.as_bool()?,
    })
}

/// A line of `pactl subscribe` about a sink, a source or the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaEvent {
    New(Kind, u32),
    Change(Kind, u32),
    Remove(Kind, u32),
    /// The server changed, as when the default sink or source moves.
    Server,
}

/// `Event 'new' on sink #57`, `Event 'change' on server #4294967295`;
/// lines about other facilities (`sink-input`, `client`, `card`, …) are
/// `None`.
pub fn parse_event(line: &str) -> Option<PaEvent> {
    let rest = line.trim().strip_prefix("Event '")?;
    let (what, rest) = rest.split_once("' on ")?;
    let mut words = rest.split_whitespace();
    let facility = words.next()?;
    if facility == "server" {
        return (what == "change").then_some(PaEvent::Server);
    }
    let kind = match facility {
        "sink" => Kind::Sink,
        "source" => Kind::Source,
        _ => return None,
    };
    let index = words.next()?.strip_prefix('#')?.parse().ok()?;
    match what {
        "new" => Some(PaEvent::New(kind, index)),
        "change" => Some(PaEvent::Change(kind, index)),
        "remove" => Some(PaEvent::Remove(kind, index)),
        _ => None,
    }
}

pub struct Pactl {
    cmd: Vec<String>,
    timeout: Duration,
}

impl Pactl {
    /// `TF_PACTL` split on whitespace, `pactl` when unset or empty.
    pub fn from_env() -> Pactl {
        Pactl {
            cmd: super::cmd_from_env("TF_PACTL", "pactl"),
            timeout: TIMEOUT,
        }
    }

    /// Every sink or every source but the monitors.
    pub fn list(&self, kind: Kind) -> Result<Vec<Device>, String> {
        let out = self.run(&["-f", "json", "list", &format!("{}s", kind.word())])?;
        parse_devices(kind, &out)
    }

    /// The default sink's or source's name.
    pub fn default_name(&self, kind: Kind) -> Result<String, String> {
        let out = self.run(&[&format!("get-default-{}", kind.word())])?;
        Ok(out.trim().to_string())
    }

    pub fn set_volume(&self, kind: Kind, name: &str, percent: u8) -> Result<(), String> {
        self.run(&[&format!("set-{}-volume", kind.word()), name, &format!("{percent}%")])
            .map(drop)
    }

    pub fn set_mute(&self, kind: Kind, name: &str, muted: bool) -> Result<(), String> {
        let m = if muted { "1" } else { "0" };
        self.run(&[&format!("set-{}-mute", kind.word()), name, m]).map(drop)
    }

    /// Whether the default source is muted: false when it is not listed
    /// (a monitor, or none).
    pub fn source_muted(&self) -> Result<bool, String> {
        let name = self.default_name(Kind::Source)?;
        let sources = self.list(Kind::Source)?;
        Ok(sources.iter().any(|d| d.name == name && d.muted))
    }

    /// The command line of `pactl subscribe`.
    pub fn subscribe_args(&self) -> Vec<String> {
        let mut argv = self.cmd.clone();
        argv.push("subscribe".into());
        argv
    }

    /// Run every set, one after the other. A failure (a device that went
    /// away) does not stop the other sets; the failures are returned, for
    /// the log.
    pub fn apply(&self, sets: &[Set]) -> Vec<String> {
        sets.iter()
            .filter_map(|s| {
                match s {
                    Set::Volume(k, name, p) => self.set_volume(*k, name, *p),
                    Set::Mute(k, name, m) => self.set_mute(*k, name, *m),
                }
                .err()
            })
            .collect()
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        super::run_tool(&self.cmd, &args, self.timeout)
    }
}

pub enum AudioRequest {
    /// Change the volume by this many percent.
    Step(i32),
    ToggleMute,
    ToggleMic,
    /// A `pactl subscribe` line.
    Device(PaEvent),
    /// The volume keys switch changed.
    Mode(Mode),
    /// Every device in line with the shared state again (after the
    /// sound server came back).
    Resync,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioEvent {
    /// The shared speaker state after a key.
    Sink(Level),
    /// The shared microphone mute after the mic key.
    Mic(bool),
    /// Whether the default source is muted now, for the mark.
    Mark(bool),
    Failed(String),
}

/// The requests not yet taken by the worker; `closed` ends it.
#[derive(Default)]
struct Queue {
    pending: VecDeque<AudioRequest>,
    closed: bool,
}

/// True for an event whose only effect is a fresh mark: a source or the
/// server changed. The OSD's own sets cause these, so they never set.
fn refreshes_mark(e: &PaEvent) -> bool {
    matches!(e, PaEvent::Change(Kind::Source, _) | PaEvent::Server)
}

/// Runs `pactl` on a thread of its own, one request at a time, and owns
/// the shared state. A `Step` that arrives while the previous `Step`
/// still waits is added to it, so a held key runs one round of sets per
/// call, not one per press; change events merge into one mark refresh;
/// toggles are kept one by one. Dropping the worker ends the thread after
/// its current call.
pub struct AudioWorker {
    shared: Arc<(Mutex<Queue>, Condvar)>,
}

impl AudioWorker {
    /// At start the worker brings every device in line (`Shared::start`) and
    /// sends one `Mark`.
    pub fn spawn(p: Pactl, mode: Mode, on_event: impl Fn(AudioEvent) + Send + 'static) -> AudioWorker {
        let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let s = shared.clone();
        std::thread::spawn(move || {
            let mut w = Worker { p, mode, sync: None };
            match w.start() {
                Ok(muted) => on_event(AudioEvent::Mark(muted)),
                Err(e) => {
                    on_event(AudioEvent::Failed(e));
                    on_event(AudioEvent::Mark(false));
                }
            }
            while let Some(r) = next(&s) {
                // What may change a source's mute sends a fresh mark once
                // its sets are done, read from the default source: the
                // mark follows that source's real state, never the shared
                // one (a monitor as the default has no mark).
                let remark = matches!(
                    r,
                    AudioRequest::ToggleMic
                        | AudioRequest::Mode(_)
                        | AudioRequest::Resync
                        | AudioRequest::Device(PaEvent::New(Kind::Source, _))
                );
                let (ev, sets) = w.handle(r);
                let failed = matches!(ev, Some(AudioEvent::Failed(_)));
                // The panel shows the new state at once; a stalled sound
                // server holds only the sets, not the panel.
                if let Some(ev) = ev {
                    on_event(ev);
                }
                w.apply(&sets);
                if remark && !failed {
                    on_event(w.mark());
                }
            }
        });
        AudioWorker { shared }
    }

    /// Queue `r` for the worker. Never blocks on `pactl`.
    pub fn request(&self, r: AudioRequest) {
        if matches!(r, AudioRequest::Device(PaEvent::Change(Kind::Sink, _))) {
            // Someone else's change of a sink is not copied anywhere.
            return;
        }
        let (m, cv) = &*self.shared;
        let mut q = m.lock().unwrap_or_else(|e| e.into_inner());
        match (q.pending.back_mut(), r) {
            (Some(AudioRequest::Step(a)), AudioRequest::Step(b)) => *a = a.saturating_add(b),
            (Some(AudioRequest::Device(a)), AudioRequest::Device(b)) if refreshes_mark(a) && refreshes_mark(&b) => {}
            (_, r) => q.pending.push_back(r),
        }
        cv.notify_one();
    }
}

impl Drop for AudioWorker {
    fn drop(&mut self) {
        let (m, cv) = &*self.shared;
        m.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        cv.notify_one();
    }
}

/// The worker thread's side: `pactl` and the shared state, which exists
/// once a start has listed the devices.
struct Worker {
    p: Pactl,
    mode: Mode,
    sync: Option<Shared>,
}

impl Worker {
    /// Read the defaults, take the shared state from them and bring every
    /// device in line. Whether the default source is muted, for the mark.
    fn start(&mut self) -> Result<bool, String> {
        let sinks = self.p.list(Kind::Sink)?;
        let sources = self.p.list(Kind::Source)?;
        // A default that cannot be read fails the start: taken for "no
        // default", it would set every device to 100 %.
        let named = |n: String| Some(n).filter(|n| !n.is_empty());
        let default_sink = named(self.p.default_name(Kind::Sink)?);
        let default_source = named(self.p.default_name(Kind::Source)?);
        let (sync, sets) = Shared::start(
            self.mode,
            &sinks,
            &sources,
            default_sink.as_deref(),
            default_source.as_deref(),
        );
        self.apply(&sets);
        self.sync = Some(sync);
        let source = default_source.unwrap_or_default();
        Ok(sources.iter().any(|d| d.name == source && d.muted))
    }

    /// Run `sets`; a failed set is logged, not reported as the key's
    /// failure.
    fn apply(&self, sets: &[Set]) {
        for e in self.p.apply(sets) {
            eprintln!("thinfloe-menu --osd: {e}");
        }
    }

    fn mark(&self) -> AudioEvent {
        self.p.source_muted().map_or_else(AudioEvent::Failed, AudioEvent::Mark)
    }

    /// What `r` does: the event to send, if any, and the sets to run
    /// after it.
    fn handle(&mut self, r: AudioRequest) -> (Option<AudioEvent>, Vec<Set>) {
        let done = match r {
            AudioRequest::Step(d) => self
                .with(Kind::Sink, |s, sinks| s.step(d, sinks))
                .map(|(s, sets)| (AudioEvent::Sink(s.level()), sets)),
            AudioRequest::ToggleMute => self
                .with(Kind::Sink, Shared::toggle_mute)
                .map(|(s, sets)| (AudioEvent::Sink(s.level()), sets)),
            AudioRequest::ToggleMic => self
                .with(Kind::Source, Shared::toggle_mic)
                .map(|(s, sets)| (AudioEvent::Mic(s.mic_muted()), sets)),
            AudioRequest::Device(PaEvent::New(kind, index)) => {
                return (self.on_new(kind, index).err().map(AudioEvent::Failed), Vec::new());
            }
            AudioRequest::Device(e) if refreshes_mark(&e) => return (Some(self.mark()), Vec::new()),
            AudioRequest::Device(_) => return (None, Vec::new()),
            AudioRequest::Mode(m) => {
                self.mode = m;
                return (
                    self.realign(|s, sinks, sources| s.set_mode(m, sinks, sources)),
                    Vec::new(),
                );
            }
            AudioRequest::Resync => {
                return (self.realign(|s, sinks, sources| s.reapply(sinks, sources)), Vec::new());
            }
        };
        match done {
            Ok((ev, sets)) => (Some(ev), sets),
            Err(e) => (Some(AudioEvent::Failed(e)), Vec::new()),
        }
    }

    /// The shared state, started first if no start has worked yet (the
    /// sound server was not up), and the devices of `kind`; `f` changes
    /// the state. The state afterwards, and the sets still to run.
    fn with(
        &mut self,
        kind: Kind,
        f: impl FnOnce(&mut Shared, &[Device]) -> Vec<Set>,
    ) -> Result<(Shared, Vec<Set>), String> {
        if self.sync.is_none() {
            self.start()?;
        }
        let devices = self.p.list(kind)?;
        let sync = self.sync.as_mut().expect("started");
        let sets = f(sync, &devices);
        Ok((sync.clone(), sets))
    }

    fn on_new(&mut self, kind: Kind, index: u32) -> Result<(), String> {
        let Some(sync) = &self.sync else {
            // A start lines up the new device with all the others.
            return self.start().map(drop);
        };
        let devices = self.p.list(kind)?;
        if let Some(d) = devices.iter().find(|d| d.index == index) {
            self.apply(&sync.on_new(d));
        }
        Ok(())
    }

    /// Every device again, through `f`; a start does that by itself. When
    /// the devices cannot be listed, `f` still changes the state, with no
    /// devices: after a switch-off the next `Resync` or new device gets
    /// 100 % and unmuted, not the keys' state.
    fn realign(&mut self, f: impl FnOnce(&mut Shared, &[Device], &[Device]) -> Vec<Set>) -> Option<AudioEvent> {
        let Some(sync) = &mut self.sync else {
            return self.start().err().map(AudioEvent::Failed);
        };
        let lists = self
            .p
            .list(Kind::Sink)
            .and_then(|s| Ok((s, self.p.list(Kind::Source)?)));
        let (sinks, sources) = match lists {
            Ok(l) => l,
            Err(e) => {
                f(sync, &[], &[]);
                return Some(AudioEvent::Failed(e));
            }
        };
        let sets = f(sync, &sinks, &sources);
        self.apply(&sets);
        None
    }
}

/// What `watch` reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Watch {
    /// A line of `pactl subscribe` about a sink, a source or the server.
    Event(PaEvent),
    /// `pactl subscribe` ended and runs again: the sound server may have
    /// restarted, and events in between are lost.
    Restarted,
}

/// The wait before `pactl subscribe` starts again: `base`, doubled after
/// each short run up to `cap`, `base` again after a run of `cap` or more.
struct Backoff {
    base: Duration,
    cap: Duration,
    next: Duration,
    logged: BTreeSet<Duration>,
}

impl Backoff {
    fn new(base: Duration, cap: Duration) -> Backoff {
        Backoff {
            base,
            cap,
            next: base,
            logged: BTreeSet::new(),
        }
    }

    /// The wait after a run that lasted `ran_for`.
    fn after_run(&mut self, ran_for: Duration) -> Duration {
        if ran_for >= self.cap {
            self.next = self.base;
        }
        let d = self.next;
        self.next = (d * 2).min(self.cap);
        d
    }

    /// The log line for a wait of `d`, once per distinct wait.
    fn log_line(&mut self, d: Duration) -> Option<String> {
        self.logged.insert(d).then(|| {
            format!(
                "thinfloe-menu --osd: pactl subscribe ended; retrying in {} s",
                d.as_secs()
            )
        })
    }
}

/// The watch thread and its `pactl subscribe` child, shared with the
/// `Watcher` that stops them.
#[derive(Default)]
struct WatchState {
    stop: bool,
    /// The running child's pid, which is also its process group.
    child: Option<u32>,
    /// The thread has ended.
    done: bool,
}

type WatchShared = Arc<(Mutex<WatchState>, Condvar)>;

fn lock_watch(s: &(Mutex<WatchState>, Condvar)) -> MutexGuard<'_, WatchState> {
    s.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// `pactl subscribe` on a thread of its own. Dropping it kills the child's
/// whole process group (a shell's `tail -f` too), reaps it and ends the
/// thread; the drop does not wait out a back-off.
pub struct Watcher {
    shared: WatchShared,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let mut st = lock_watch(&self.shared);
        st.stop = true;
        if let Some(pid) = st.child {
            // SAFETY: the child is not reaped while `child` is set, so its
            // process group still exists and is ours.
            unsafe {
                libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
            }
        }
        self.shared.1.notify_all();
        // A grandchild that left the group could hold the pipe open; the
        // OSD does not hang on it.
        let (st, _) = self
            .shared
            .1
            .wait_timeout_while(st, WATCH_JOIN, |s| !s.done)
            .unwrap_or_else(|e| e.into_inner());
        let done = st.done;
        drop(st);
        if let (true, Some(t)) = (done, self.thread.take()) {
            let _ = t.join();
        }
    }
}

/// Run `pactl subscribe` and hand each line about a sink, a source or the
/// server to `on`. When it ends (no sound server, pipewire-pulse
/// restarted, no `pactl`), it is started again after a wait (`Backoff`,
/// logged once per distinct wait), then `on` gets `Watch::Restarted`.
/// Its stderr is logged once per distinct line, so a missing server
/// does not log every 30 s.
pub fn watch(p: &Pactl, on: impl Fn(Watch) + Send + 'static) -> Watcher {
    let argv = p.subscribe_args();
    let shared: WatchShared = Arc::default();
    let s = shared.clone();
    let thread = std::thread::spawn(move || {
        watch_loop(&argv, &s, Backoff::new(WATCH_BASE, WATCH_CAP), on);
        lock_watch(&s).done = true;
        s.1.notify_all();
    });
    Watcher {
        shared,
        thread: Some(thread),
    }
}

/// Messages of the watch, each logged once, at most `cap` of them: a
/// `pactl` that prints a new line on every run cannot fill the log.
struct OnceLog {
    seen: BTreeSet<String>,
    cap: usize,
    full: bool,
}

impl OnceLog {
    fn new(cap: usize) -> OnceLog {
        OnceLog {
            seen: BTreeSet::new(),
            cap,
            full: false,
        }
    }

    /// The line to log for `msg`, if any.
    fn line(&mut self, msg: String) -> Option<String> {
        if self.seen.contains(&msg) {
            return None;
        }
        if self.seen.len() >= self.cap {
            return (!std::mem::replace(&mut self.full, true))
                .then(|| "thinfloe-menu --osd: pactl subscribe: more messages are not logged".to_string());
        }
        self.seen.insert(msg.clone());
        Some(msg)
    }
}

/// Most distinct messages the watch logs.
const WATCH_LOG_CAP: usize = 32;

fn watch_loop(argv: &[String], s: &(Mutex<WatchState>, Condvar), mut backoff: Backoff, on: impl Fn(Watch)) {
    let mut logged = OnceLog::new(WATCH_LOG_CAP);
    let mut log_once = |msg: String| {
        if let Some(line) = logged.line(msg) {
            eprintln!("{line}");
        }
    };
    let mut first = true;
    loop {
        let started = Instant::now();
        let spawned = {
            let mut st = lock_watch(s);
            if st.stop {
                return;
            }
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .process_group(0);
            // SAFETY: prctl and getppid are async-signal-safe. The child
            // gets SIGTERM when this thread ends, as when the OSD is
            // killed with SIGKILL or crashes; a parent already gone before
            // the prctl is caught by the getppid check.
            let parent = std::process::id() as libc::pid_t;
            unsafe {
                cmd.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong, 0, 0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() != parent {
                        libc::_exit(1);
                    }
                    Ok(())
                });
            }
            let c = cmd.spawn();
            if let Ok(c) = &c {
                st.child = Some(c.id());
            }
            c
        };
        match spawned {
            Ok(child) => {
                // A restart counts once it reached the server: its first
                // line, or `WATCH_UP` without its end. Sent at the spawn,
                // a server that came up just after this run failed to
                // connect would get no Resync.
                let up = (!first).then(|| started + WATCH_UP);
                let stderr = pump_watch(child, s, up, &on);
                for line in stderr.lines().map(str::trim).filter(|l| !l.is_empty()) {
                    log_once(format!("thinfloe-menu --osd: pactl subscribe: {line}"));
                }
            }
            Err(e) => log_once(format!("thinfloe-menu --osd: {} subscribe: {e}", argv[0])),
        }
        first = false;
        let wait = backoff.after_run(started.elapsed());
        let st = lock_watch(s);
        if st.stop {
            return;
        }
        if let Some(msg) = backoff.log_line(wait) {
            eprintln!("{msg}");
        }
        let (st, _) =
            s.1.wait_timeout_while(st, wait, |st| !st.stop)
                .unwrap_or_else(|e| e.into_inner());
        if st.stop {
            return;
        }
    }
}

/// Hand each event line of `child` to `on` until its stdout ends, then
/// reap it. With `up`, `on` gets `Watch::Restarted` first: at the first
/// line, or at `up` if the child is still running. What it wrote to
/// stderr, at most `WATCH_LINE` bytes.
fn pump_watch(mut child: Child, s: &(Mutex<WatchState>, Condvar), up: Option<Instant>, on: &impl Fn(Watch)) -> String {
    let err = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                match e.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let room = WATCH_LINE.saturating_sub(kept.len());
                        kept.extend_from_slice(&buf[..n.min(room)]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            String::from_utf8_lossy(&kept).into_owned()
        })
    });
    if let Some(out) = child.stdout.take() {
        for_each_line(
            out,
            WATCH_LINE,
            up,
            || on(Watch::Restarted),
            |line| {
                if let Some(e) = parse_event(line) {
                    on(Watch::Event(e));
                }
            },
        );
    }
    // Not killed from here on: the pid may go once the child is reaped.
    lock_watch(s).child = None;
    let _ = child.wait();
    err.and_then(|t| t.join().ok()).unwrap_or_default()
}

/// Call `f` with each line of `r` until it ends; a line longer than `max`
/// bytes is dropped, not kept in memory. With `up`, `on_up` runs once,
/// before the first line or at `up` if `r` has neither ended nor sent a
/// line by then.
fn for_each_line<R: Read + AsRawFd>(
    mut r: R,
    max: usize,
    mut up: Option<Instant>,
    mut on_up: impl FnMut(),
    mut f: impl FnMut(&str),
) {
    let mut line: Vec<u8> = Vec::new();
    let mut too_long = false;
    let mut buf = [0u8; 4096];
    loop {
        if let Some(t) = up {
            let ms = t
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(i32::MAX as u128) as libc::c_int;
            let mut pfd = libc::pollfd {
                fd: r.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd for the duration of the call.
            match unsafe { libc::poll(&mut pfd, 1, ms) } {
                0 => {
                    up = None;
                    on_up();
                }
                n if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => continue,
                _ => {}
            }
        }
        let n = match r.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        for &b in &buf[..n] {
            if b == b'\n' {
                if up.take().is_some() {
                    on_up();
                }
                if !too_long {
                    f(&String::from_utf8_lossy(&line));
                }
                line.clear();
                too_long = false;
            } else if line.len() < max {
                line.push(b);
            } else {
                too_long = true;
            }
        }
    }
}

/// The next request, waiting for one; `None` once the worker is dropped.
fn next(shared: &(Mutex<Queue>, Condvar)) -> Option<AudioRequest> {
    let (m, cv) = shared;
    let mut q = m.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if q.closed {
            return None;
        }
        if let Some(r) = q.pending.pop_front() {
            return Some(r);
        }
        q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::testdir::TestDir;
    use std::path::Path;
    use std::time::Instant;

    const SINKS: &str = include_str!("../../tests/fixtures/pactl-sinks.json");
    const SOURCES: &str = include_str!("../../tests/fixtures/pactl-sources.json");
    const SUBSCRIBE: &str = include_str!("../../tests/fixtures/pactl-subscribe.txt");

    /// A device list in the shape `pactl -f json list` prints, with two
    /// channels at `percent` each.
    pub(in crate::osd) fn json(devs: &[(u32, &str, u8, bool)]) -> String {
        let items: Vec<String> = devs
            .iter()
            .map(|(i, n, p, m)| {
                format!(
                    r#"{{"index":{i},"name":"{n}","mute":{m},"volume":{{"front-left":{{"value":1,"value_percent":"{p}%","db":"0 dB"}},"front-right":{{"value":1,"value_percent":"{p}%","db":"0 dB"}}}},"monitor_source":""}}"#
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    /// A fake `pactl` in `d`: logs its arguments to `log`, answers
    /// `-f json list sinks|sources` from `sinks.json`/`sources.json` and
    /// `get-default-*` from `default-sink`/`default-source`, then runs
    /// `body`.
    pub(in crate::osd) fn fake(
        d: &Path,
        sinks: &str,
        sources: &str,
        default_sink: &str,
        default_source: &str,
        body: &str,
    ) -> Pactl {
        std::fs::write(d.join("sinks.json"), sinks).unwrap();
        std::fs::write(d.join("sources.json"), sources).unwrap();
        std::fs::write(d.join("default-sink"), format!("{default_sink}\n")).unwrap();
        std::fs::write(d.join("default-source"), format!("{default_source}\n")).unwrap();
        let dir = d.display();
        let script = d.join("pactl");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{dir}/log'\n{body}\ncase \"$*\" in\n\
                 '-f json list sinks') cat '{dir}/sinks.json';;\n\
                 '-f json list sources') cat '{dir}/sources.json';;\n\
                 get-default-sink) cat '{dir}/default-sink';;\n\
                 get-default-source) cat '{dir}/default-source';;\n\
                 esac\n"
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Pactl {
            cmd: vec![script.to_string_lossy().into_owned()],
            timeout: TIMEOUT,
        }
    }

    fn log(d: &Path) -> Vec<String> {
        std::fs::read_to_string(d.join("log"))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    pub(in crate::osd) fn calls(d: &Path, prefix: &str) -> Vec<String> {
        log(d).into_iter().filter(|l| l.starts_with(prefix)).collect()
    }

    pub(in crate::osd) fn wait_for(what: &str, cond: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        while !cond() {
            assert!(Instant::now() < until, "{what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    type Events = Arc<Mutex<Vec<AudioEvent>>>;

    fn spawn(p: Pactl, mode: Mode) -> (AudioWorker, Events) {
        let events: Events = Arc::new(Mutex::new(Vec::new()));
        let e = events.clone();
        let worker = AudioWorker::spawn(p, mode, move |ev| e.lock().unwrap().push(ev));
        (worker, events)
    }

    fn count(events: &Events, f: impl Fn(&AudioEvent) -> bool) -> usize {
        events.lock().unwrap().iter().filter(|e| f(e)).count()
    }

    #[test]
    fn parse_devices_reads_name_index_percent_mute() {
        // Taken from pactl 17 (pulseaudio-utils, Ubuntu 26.04) on
        // pipewire-pulse 1.6.2 with two null sinks.
        assert_eq!(
            parse_devices(Kind::Sink, SINKS),
            Ok(vec![
                Device {
                    kind: Kind::Sink,
                    index: 51,
                    name: "tf_null_a".into(),
                    percent: 46,
                    muted: false
                },
                Device {
                    kind: Kind::Sink,
                    index: 58,
                    name: "tf_null_b".into(),
                    percent: 100,
                    muted: true
                },
            ])
        );
        assert_eq!(parse_devices(Kind::Sink, "[]"), Ok(vec![]));
        assert!(parse_devices(Kind::Sink, "").is_err());
        assert!(parse_devices(Kind::Sink, "{}").is_err());
        // An entry without a name or a volume is left out, not the list.
        let odd = r#"[{"index":1,"mute":false},{"index":2,"name":"x","mute":false,"volume":{}},
                     {"index":3,"name":"y","mute":true,"volume":{"mono":{"value_percent":"30%"}}}]"#;
        assert_eq!(
            parse_devices(Kind::Sink, odd),
            Ok(vec![Device {
                kind: Kind::Sink,
                index: 3,
                name: "y".into(),
                percent: 30,
                muted: true
            }])
        );
    }

    #[test]
    fn parse_devices_skips_monitor_sources() {
        // pactl 17 prints a monitor's sink under "monitor_source" (empty
        // for a real microphone); the fixture has two monitors and tf_mic.
        let got = parse_devices(Kind::Source, SOURCES).unwrap();
        assert_eq!(got.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["tf_mic"]);
        assert_eq!((got[0].index, got[0].percent, got[0].muted), (66, 100, false));
        // Other pactl versions name it "monitor_of_sink", "n/a" for none.
        let v = r#""volume":{"mono":{"value_percent":"100%"}}"#;
        let list = format!(
            r#"[{{"index":1,"name":"alsa_output.x.monitor","mute":false,{v},"monitor_of_sink":"alsa_output.x"}},
                {{"index":2,"name":"mic","mute":false,{v},"monitor_of_sink":"n/a"}},
                {{"index":3,"name":"cam","mute":true,{v},"monitor_of_sink":null}},
                {{"index":4,"name":"usb","mute":false,{v}}}]"#
        );
        let got = parse_devices(Kind::Source, &list).unwrap();
        assert_eq!(
            got.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["mic", "cam", "usb"]
        );
        // A source marked only by its properties is a monitor too.
        let marked = format!(
            r#"[{{"index":5,"name":"m","mute":false,{v},"properties":{{"device.class":"monitor"}}}},
                                 {{"index":6,"name":"k","mute":false,{v},"properties":{{"device.class":"sound"}}}}]"#
        );
        let got = parse_devices(Kind::Source, &marked).unwrap();
        assert_eq!(got.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["k"]);
        // A sink's "monitor_source" is its own monitor, not a reason to skip.
        assert_eq!(parse_devices(Kind::Sink, SINKS).unwrap().len(), 2);
    }

    #[test]
    fn parse_event_reads_new_change_remove_server() {
        use PaEvent::*;
        assert_eq!(parse_event("Event 'new' on sink #57"), Some(New(Kind::Sink, 57)));
        assert_eq!(
            parse_event("Event 'change' on source #12"),
            Some(Change(Kind::Source, 12))
        );
        assert_eq!(
            parse_event("Event 'remove' on sink #57\n"),
            Some(Remove(Kind::Sink, 57))
        );
        assert_eq!(parse_event("Event 'change' on server #4294967295"), Some(Server));
        assert_eq!(parse_event("Event 'change' on server"), Some(Server));
        for none in [
            "Event 'change' on sink-input #3",
            "Event 'new' on source-output #3",
            "Event 'new' on client #78",
            "Event 'new' on card #1",
            "Event 'new' on module #536870919",
            "Event 'new' on sink #x",
            "Event 'move' on sink #1",
            "",
            "Got SIGINT, exiting.",
        ] {
            assert_eq!(parse_event(none), None, "{none:?}");
        }
        // The lines a real `pactl subscribe` printed while null sinks
        // came and went.
        let got: Vec<PaEvent> = SUBSCRIBE.lines().filter_map(parse_event).collect();
        assert_eq!(got[0], Change(Kind::Sink, 51));
        assert!(got.contains(&New(Kind::Source, 81)));
        assert!(got.contains(&Remove(Kind::Source, 51)));
        assert_eq!(got.iter().filter(|e| **e == Server).count(), 4);
    }

    #[test]
    fn pactl_runs_the_calls_the_brief_names() {
        let d = TestDir::new("osd-audio", "argv");
        let p = fake(&d, "[]", "[]", "  alsa_output.a \n", "alsa_input.b", "");
        assert_eq!(p.default_name(Kind::Sink), Ok("alsa_output.a".to_string()));
        assert_eq!(p.default_name(Kind::Source), Ok("alsa_input.b".to_string()));
        p.set_volume(Kind::Sink, "s", 45).unwrap();
        p.set_volume(Kind::Source, "m", 100).unwrap();
        p.set_mute(Kind::Sink, "s", true).unwrap();
        p.set_mute(Kind::Source, "m", false).unwrap();
        assert_eq!(p.list(Kind::Sink), Ok(vec![]));
        assert_eq!(
            log(&d),
            [
                "get-default-sink",
                "get-default-source",
                "set-sink-volume s 45%",
                "set-source-volume m 100%",
                "set-sink-mute s 1",
                "set-source-mute m 0",
                "-f json list sinks",
            ]
        );
        assert_eq!(p.subscribe_args().last().map(String::as_str), Some("subscribe"));
        assert_eq!(p.subscribe_args()[0], p.cmd[0]);
    }

    #[test]
    fn a_failing_pactl_is_an_error_not_a_level() {
        let d = TestDir::new("osd-audio", "fail");
        let p = fake(
            &d,
            "[]",
            "[]",
            "",
            "",
            "echo 'Connection failure: Connection refused' >&2; exit 1",
        );
        assert!(p.list(Kind::Sink).is_err());
        assert!(p.default_name(Kind::Source).is_err());
        assert!(p.set_volume(Kind::Sink, "a", 5).is_err());
        // Exit 0 with no list is no list either.
        let p = fake(&d, "", "", "", "", "");
        assert!(p.list(Kind::Sink).is_err());

        let p = fake(&d, "[]", "[]", "", "", "exit 1");
        let (worker, events) = spawn(p, Mode::Keys);
        worker.request(AudioRequest::Step(5));
        wait_for("two Failed events", || {
            count(&events, |e| matches!(e, AudioEvent::Failed(_))) == 2
        });
        // No level from a failing pactl; the start's mark is off.
        assert_eq!(count(&events, |e| matches!(e, AudioEvent::Sink(_))), 0);
        assert_eq!(count(&events, |e| *e == AudioEvent::Mark(true)), 0);
    }

    #[test]
    fn the_worker_starts_aligned_and_sends_the_mark() {
        let d = TestDir::new("osd-audio", "start");
        let sinks = json(&[(1, "a", 40, false), (2, "b", 100, false)]);
        let sources = json(&[(3, "mic", 100, true), (4, "cam", 100, false)]);
        let p = fake(&d, &sinks, &sources, "a", "mic", "");
        let (_worker, events) = spawn(p, Mode::Keys);
        wait_for("the start's mark", || !events.lock().unwrap().is_empty());
        assert_eq!(*events.lock().unwrap(), [AudioEvent::Mark(true)]);
        assert_eq!(calls(&d, "set-"), ["set-sink-volume b 40%", "set-source-mute cam 1"]);

        let d = TestDir::new("osd-audio", "startoff");
        let p = fake(&d, &sinks, &sources, "a", "mic", "");
        let (_worker, events) = spawn(p, Mode::Off);
        wait_for("the start's mark", || !events.lock().unwrap().is_empty());
        assert_eq!(
            calls(&d, "set-"),
            ["set-sink-volume a 100%", "set-source-mute mic 0"],
            "Off: full and unmuted"
        );
    }

    #[test]
    fn a_failing_set_on_one_device_still_sets_the_others() {
        let d = TestDir::new("osd-audio", "gone");
        let sinks = json(&[(1, "gone", 40, false), (2, "kept", 40, false)]);
        let p = fake(
            &d,
            &sinks,
            "[]",
            "gone",
            "",
            "[ \"$*\" = 'set-sink-volume gone 45%' ] && { echo 'Failure: No such entity' >&2; exit 1; }",
        );
        let (worker, events) = spawn(p, Mode::Keys);
        worker.request(AudioRequest::Step(5));
        wait_for("a Sink event", || {
            count(&events, |e| matches!(e, AudioEvent::Sink(_))) == 1
        });
        // The sets run after the event.
        wait_for("both sets", || calls(&d, "set-").len() == 2);
        assert_eq!(
            calls(&d, "set-"),
            ["set-sink-volume gone 45%", "set-sink-volume kept 45%"]
        );
        assert!(events.lock().unwrap().contains(&AudioEvent::Sink(Level {
            percent: 45,
            muted: false
        })));
        assert_eq!(count(&events, |e| matches!(e, AudioEvent::Failed(_))), 0);
    }

    #[test]
    fn change_events_only_refresh_the_mark() {
        let d = TestDir::new("osd-audio", "change");
        let sinks = json(&[(1, "a", 40, false)]);
        let sources = json(&[(2, "mic", 100, false)]);
        // The start waits, so the burst is queued before the worker
        // takes the first of it.
        let p = fake(
            &d,
            &sinks,
            &sources,
            "a",
            "mic",
            "[ \"$1\" = get-default-sink ] && sleep 0.5",
        );
        let (worker, events) = spawn(p, Mode::Keys);
        worker.request(AudioRequest::Device(PaEvent::Change(Kind::Sink, 1)));
        worker.request(AudioRequest::Device(PaEvent::Change(Kind::Source, 2)));
        worker.request(AudioRequest::Device(PaEvent::Server));
        wait_for("the start's mark and one refresh", || {
            count(&events, |e| matches!(e, AudioEvent::Mark(_))) >= 2
        });
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(count(&events, |e| matches!(e, AudioEvent::Mark(_))), 2);
        assert_eq!(calls(&d, "set-"), Vec::<String>::new(), "no feedback loop");
        // One for the start, one for the merged burst.
        assert_eq!(calls(&d, "get-default-source").len(), 2);
        assert_eq!(calls(&d, "-f json list sources").len(), 2);
        // A sink change alone does nothing.
        worker.request(AudioRequest::Device(PaEvent::Change(Kind::Sink, 1)));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(calls(&d, "get-default-source").len(), 2);
    }

    #[test]
    fn a_new_device_gets_the_shared_state() {
        let d = TestDir::new("osd-audio", "new");
        let sources = json(&[(2, "cam", 100, false)]);
        let p = fake(&d, &json(&[(1, "a", 30, false)]), &sources, "a", "cam", "");
        let (worker, events) = spawn(p, Mode::Keys);
        worker.request(AudioRequest::ToggleMic);
        wait_for("a Mic event", || {
            events.lock().unwrap().contains(&AudioEvent::Mic(true))
        });
        // A headset arrives as the new default at 100 %.
        std::fs::write(
            d.join("sinks.json"),
            json(&[(1, "a", 30, false), (7, "headset", 100, false)]),
        )
        .unwrap();
        std::fs::write(
            d.join("sources.json"),
            json(&[(2, "cam", 100, true), (8, "mic", 100, false)]),
        )
        .unwrap();
        std::fs::write(d.join("default-sink"), "headset\n").unwrap();
        worker.request(AudioRequest::Device(PaEvent::New(Kind::Sink, 7)));
        worker.request(AudioRequest::Device(PaEvent::New(Kind::Source, 8)));
        wait_for("both new devices set", || calls(&d, "set-").len() == 3);
        assert_eq!(
            calls(&d, "set-"),
            [
                "set-source-mute cam 1",
                "set-sink-volume headset 30%",
                "set-source-mute mic 1"
            ]
        );
        // An index that is gone by the time it is listed sets nothing.
        worker.request(AudioRequest::Device(PaEvent::New(Kind::Sink, 99)));
        worker.request(AudioRequest::Step(5));
        wait_for("a Sink event", || {
            count(&events, |e| matches!(e, AudioEvent::Sink(_))) == 1
        });
        assert!(events.lock().unwrap().contains(&AudioEvent::Sink(Level {
            percent: 35,
            muted: false
        })));
    }

    #[test]
    fn mode_off_resets_every_device() {
        let d = TestDir::new("osd-audio", "mode");
        let p = fake(
            &d,
            &json(&[(1, "a", 40, true)]),
            &json(&[(2, "mic", 60, true)]),
            "a",
            "mic",
            "",
        );
        let (worker, events) = spawn(p, Mode::Keys);
        wait_for("the start's mark", || !events.lock().unwrap().is_empty());
        assert_eq!(calls(&d, "set-"), Vec::<String>::new());
        worker.request(AudioRequest::Mode(Mode::Off));
        wait_for("four sets", || calls(&d, "set-").len() == 4);
        assert_eq!(
            calls(&d, "set-"),
            [
                "set-sink-volume a 100%",
                "set-sink-mute a 0",
                "set-source-volume mic 100%",
                "set-source-mute mic 0"
            ]
        );
    }

    #[test]
    fn a_switch_off_whose_list_fails_still_turns_the_state_off() {
        let d = TestDir::new("osd-audio", "offfail");
        // While `nolist` exists, every device list times out.
        let p = fake(
            &d,
            &json(&[(1, "a", 40, true)]),
            &json(&[(2, "mic", 60, true)]),
            "a",
            "mic",
            &format!(
                "[ -e '{}/nolist' ] && [ \"$1\" = -f ] && {{ echo 'Connection failure: Timeout' >&2; exit 1; }}",
                d.display()
            ),
        );
        let (worker, events) = spawn(p, Mode::Keys);
        wait_for("the start's mark", || !events.lock().unwrap().is_empty());
        std::fs::write(d.join("nolist"), "").unwrap();
        worker.request(AudioRequest::Mode(Mode::Off));
        wait_for("the failed list", || {
            count(&events, |e| matches!(e, AudioEvent::Failed(_))) == 1
        });
        assert_eq!(calls(&d, "set-"), Vec::<String>::new());
        // The sound server is back: the shared state is off, so every
        // device goes to 100 % and unmuted, the microphone too.
        std::fs::remove_file(d.join("nolist")).unwrap();
        worker.request(AudioRequest::Resync);
        wait_for("four sets", || calls(&d, "set-").len() == 4);
        assert_eq!(
            calls(&d, "set-"),
            [
                "set-sink-volume a 100%",
                "set-sink-mute a 0",
                "set-source-volume mic 100%",
                "set-source-mute mic 0"
            ]
        );
    }

    #[test]
    fn steps_merge_while_pactl_runs() {
        let d = TestDir::new("osd-audio", "merge");
        let p = fake(
            &d,
            &json(&[(1, "a", 50, false)]),
            "[]",
            "a",
            "",
            "[ \"$1\" = set-sink-volume ] && sleep 0.3",
        );
        let (worker, events) = spawn(p, Mode::Keys);
        for _ in 0..6 {
            worker.request(AudioRequest::Step(5));
        }
        let at_80 = AudioEvent::Sink(Level {
            percent: 80,
            muted: false,
        });
        wait_for("the steps add up to 80 %", || events.lock().unwrap().contains(&at_80));
        // A third run would start at once; give it room to show.
        std::thread::sleep(Duration::from_millis(400));
        let sets = calls(&d, "set-sink-volume");
        assert!(sets.len() <= 2, "{sets:?}");
        assert_eq!(sets.last().map(String::as_str), Some("set-sink-volume a 80%"));
        assert_eq!(
            count(&events, |e| matches!(e, AudioEvent::Sink(_))),
            sets.len(),
            "an event per run"
        );
    }

    #[test]
    fn toggles_are_not_merged() {
        let d = TestDir::new("osd-audio", "toggles");
        let p = fake(
            &d,
            &json(&[(1, "a", 50, false)]),
            "[]",
            "a",
            "",
            "[ \"$1\" = set-sink-mute ] && sleep 0.2",
        );
        let (worker, events) = spawn(p, Mode::Keys);
        for _ in 0..3 {
            worker.request(AudioRequest::ToggleMute);
        }
        wait_for("three toggles", || {
            count(&events, |e| matches!(e, AudioEvent::Sink(_))) == 3
        });
        let muted: Vec<bool> = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AudioEvent::Sink(l) => Some(l.muted),
                _ => None,
            })
            .collect();
        assert_eq!(muted, [true, false, true]);
    }

    #[test]
    fn mic_toggle_sets_every_source_but_no_monitor() {
        let d = TestDir::new("osd-audio", "mic");
        let p = fake(&d, SINKS, SOURCES, "tf_null_a", "tf_null_a.monitor", "");
        let (worker, events) = spawn(p, Mode::Keys);
        worker.request(AudioRequest::ToggleMic);
        wait_for("a Mic event", || {
            events.lock().unwrap().contains(&AudioEvent::Mic(true))
        });
        wait_for("the source set", || !calls(&d, "set-source").is_empty());
        // Room for a wrong set on a monitor to show.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            calls(&d, "set-source"),
            ["set-source-mute tf_mic 1"],
            "the monitors are left alone"
        );
        // The default source here is a monitor: no mark.
        assert!(!events.lock().unwrap().contains(&AudioEvent::Mark(true)));
    }

    #[test]
    fn the_mic_key_sends_the_mark_after_its_sets() {
        let d = TestDir::new("osd-audio", "remark");
        // The fake's set-source-mute mutes the list, as pipewire-pulse does.
        let p = fake(
            &d,
            "[]",
            &json(&[(3, "mic", 100, false), (4, "cam", 100, false)]),
            "",
            "mic",
            &format!(
                "[ \"$1\" = set-source-mute ] && sed -i 's/\"mute\":false/\"mute\":true/' '{}/sources.json'",
                d.display()
            ),
        );
        let (worker, events) = spawn(p, Mode::Keys);
        wait_for("the start's mark", || !events.lock().unwrap().is_empty());
        worker.request(AudioRequest::ToggleMic);
        wait_for("the mark after the sets", || events.lock().unwrap().len() == 3);
        // The panel first; the mark from the default source's real state,
        // read after both sets.
        assert_eq!(
            *events.lock().unwrap(),
            [AudioEvent::Mark(false), AudioEvent::Mic(true), AudioEvent::Mark(true)]
        );
        assert_eq!(calls(&d, "set-"), ["set-source-mute mic 1", "set-source-mute cam 1"]);
    }

    /// `/bin/sh -c script`, so no test writes a script and executes it.
    fn sh_pactl(script: &str) -> Pactl {
        Pactl {
            cmd: ["/bin/sh", "-c", script, "pactl"].map(String::from).to_vec(),
            timeout: TIMEOUT,
        }
    }

    type Seen = Arc<Mutex<Vec<(Instant, Watch)>>>;

    fn watch_seen(p: &Pactl) -> (Watcher, Seen) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let w = watch(p, move |e| s.lock().unwrap().push((Instant::now(), e)));
        (w, seen)
    }

    #[test]
    fn the_watch_backs_off_and_resyncs() {
        // One event, a line about another facility, then the end. A
        // restart that prints an event line has reached the server.
        let p = sh_pactl(
            "[ \"$1\" = subscribe ] || exit 9; echo \"Event 'new' on source #3\"; echo \"Event 'new' on client #9\"",
        );
        let (w, seen) = watch_seen(&p);
        wait_for("an event, a restart, an event", || seen.lock().unwrap().len() >= 3);
        drop(w);
        let got: Vec<Watch> = seen.lock().unwrap().iter().map(|(_, e)| *e).collect();
        let new = Watch::Event(PaEvent::New(Kind::Source, 3));
        assert_eq!(got[..3], [new, Watch::Restarted, new]);
        // No sound server: pactl ends at once. Starts 1 s, then 2 s apart,
        // and no Restarted: none of them reached a server.
        let d = TestDir::new("osd-audio", "gaps");
        let p = sh_pactl(&format!(
            "date +%s.%N >> '{}/starts'; echo 'Connection failure: Connection refused' >&2; exit 1",
            d.display()
        ));
        let (w, seen) = watch_seen(&p);
        let starts = || -> Vec<f64> {
            std::fs::read_to_string(d.join("starts"))
                .unwrap_or_default()
                .lines()
                .filter_map(|l| l.parse().ok())
                .collect()
        };
        wait_for("three starts", || starts().len() >= 3);
        drop(w);
        let t = starts();
        let gaps = [t[1] - t[0], t[2] - t[1]];
        for (gap, want) in gaps.iter().zip([1.0, 2.0]) {
            assert!((gap - want).abs() <= 0.2, "gaps {gaps:?}");
        }
        assert!(seen.lock().unwrap().is_empty(), "no Restarted without a server");
    }

    #[test]
    fn a_restart_counts_once_pactl_stays_up() {
        let d = TestDir::new("osd-audio", "stayup");
        // Two runs without a server, then one that waits for events.
        let p = sh_pactl(&format!(
            "n=$(cat '{0}/n' 2>/dev/null || echo 0); echo $((n + 1)) > '{0}/n'; \
             [ $n -lt 2 ] && exit 1; exec sleep 30",
            d.display()
        ));
        let t0 = Instant::now();
        let (w, seen) = watch_seen(&p);
        wait_for("a restart", || !seen.lock().unwrap().is_empty());
        let at = seen.lock().unwrap()[0].0 - t0;
        // 1 s + 2 s of back-off, then 0.5 s up.
        assert!(at.as_millis().abs_diff(3500) <= 300, "Restarted after {at:?}");
        std::thread::sleep(Duration::from_millis(300));
        drop(w);
        let got: Vec<Watch> = seen.lock().unwrap().iter().map(|(_, e)| *e).collect();
        assert_eq!(got, [Watch::Restarted]);
    }

    #[test]
    fn the_watch_log_stops_after_32_messages() {
        let mut l = OnceLog::new(32);
        let mut out = Vec::new();
        for i in 0..40 {
            out.extend(l.line(format!("m{i}")));
        }
        out.extend(l.line("m0".into()));
        assert_eq!(out.len(), 33, "{out:?}");
        assert_eq!(out[31], "m31");
        assert_eq!(
            out[32],
            "thinfloe-menu --osd: pactl subscribe: more messages are not logged"
        );
    }

    #[test]
    fn the_back_off_doubles_to_30_s_and_drops_after_a_long_run() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(30));
        let short = Duration::from_millis(10);
        let secs: Vec<u64> = (0..7).map(|_| b.after_run(short).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30]);
        // A run that lasted 30 s worked: the next gap is 1 s again.
        assert_eq!(b.after_run(Duration::from_secs(30)).as_secs(), 1);
        assert_eq!(b.after_run(short).as_secs(), 2);
        // Only a new gap is logged.
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(30));
        let logged: Vec<Option<String>> = [short, short, Duration::from_secs(31), short]
            .into_iter()
            .map(|r| {
                let d = b.after_run(r);
                b.log_line(d)
            })
            .collect();
        assert_eq!(
            logged,
            [
                Some("thinfloe-menu --osd: pactl subscribe ended; retrying in 1 s".to_string()),
                Some("thinfloe-menu --osd: pactl subscribe ended; retrying in 2 s".to_string()),
                None,
                None,
            ]
        );
    }

    #[test]
    fn dropping_the_watch_ends_its_pactl() {
        let d = TestDir::new("osd-audio", "watchend");
        // A pactl with a child of its own, as `tail -f` under a shell.
        let p = sh_pactl(&format!(
            "sleep 1000 & echo $! > '{0}/child'; echo $$ > '{0}/pid'; wait",
            d.display()
        ));
        let (w, seen) = watch_seen(&p);
        let read = |f: &str| std::fs::read_to_string(d.join(f)).ok().filter(|s| s.ends_with('\n'));
        wait_for("both pids", || read("pid").is_some() && read("child").is_some());
        let t0 = Instant::now();
        drop(w);
        assert!(
            t0.elapsed() < Duration::from_secs(2),
            "the drop does not wait out a back-off"
        );
        for f in ["pid", "child"] {
            let pid = read(f).unwrap();
            let gone = || {
                std::fs::read_to_string(format!("/proc/{}/stat", pid.trim()))
                    .map_or(true, |s| s.split_whitespace().nth(2) == Some("Z"))
            };
            wait_for(&format!("{f} {} ends", pid.trim()), gone);
        }
        assert!(seen.lock().unwrap().is_empty(), "no restart after the drop");
    }

    #[test]
    fn a_failed_default_read_fails_the_start_and_is_retried() {
        let d = TestDir::new("osd-audio", "nodefault");
        // A timeout or an error is not "no default": the sink at 40 % must
        // not be taken for a start without one and set to 100 %.
        let p = fake(
            &d,
            &json(&[(1, "a", 40, false)]),
            "[]",
            "a",
            "",
            "[ \"$1\" = get-default-sink ] && { echo 'Connection failure: Timeout' >&2; exit 1; }",
        );
        let (worker, events) = spawn(p, Mode::Off);
        wait_for("the start's failure", || {
            count(&events, |e| matches!(e, AudioEvent::Failed(_))) == 1
        });
        worker.request(AudioRequest::Step(5));
        wait_for("the retried start fails too", || {
            count(&events, |e| matches!(e, AudioEvent::Failed(_))) == 2
        });
        assert_eq!(calls(&d, "get-default-sink").len(), 2, "the start is retried");
        assert_eq!(calls(&d, "set-"), Vec::<String>::new());
        assert_eq!(count(&events, |e| matches!(e, AudioEvent::Sink(_))), 0);
        // An empty default is "no default": full and unmuted.
        let d = TestDir::new("osd-audio", "emptydef");
        let p = fake(&d, &json(&[(1, "a", 40, false)]), "[]", "", "", "");
        let (_worker, events) = spawn(p, Mode::Keys);
        wait_for("the start's mark", || !events.lock().unwrap().is_empty());
        assert_eq!(calls(&d, "set-"), ["set-sink-volume a 100%"]);
    }

    #[test]
    fn the_key_event_comes_before_the_sets() {
        let d = TestDir::new("osd-audio", "latency");
        // A stalled pipewire-pulse: every set takes 1 s.
        let p = fake(
            &d,
            &json(&[(1, "a", 40, false), (2, "b", 40, false)]),
            "[]",
            "a",
            "",
            "case \"$1\" in set-*) sleep 1;; esac",
        );
        let (worker, events) = spawn(p, Mode::Keys);
        wait_for("the start's mark", || !events.lock().unwrap().is_empty());
        let t0 = Instant::now();
        worker.request(AudioRequest::Step(5));
        wait_for("a Sink event", || {
            count(&events, |e| matches!(e, AudioEvent::Sink(_))) == 1
        });
        assert!(t0.elapsed() < Duration::from_millis(900), "{:?}", t0.elapsed());
        // The sets still run, both of them.
        wait_for("both sets", || calls(&d, "set-sink-volume").len() == 2);
    }
}
