//! The Deskflow daemon, `thinfloe-menu --deskflow` (spec, Processes): the
//! TCP listener, TLS, pairing, the state file and the command socket, in
//! one poll loop. Exit 0 means there is nothing to serve (the switch is
//! off, SIGTERM, the compositor is gone); an error is written to the state
//! file's head and ends it with 1.

use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::net::{TcpListener, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustls::ServerConfig;

use super::capture::{CapEvent, Capture, OutputGeom, StripSpec};
use super::clip::{Clip, ClipEvent};
use super::clipsync::{ClipAct, ClipSync};
use super::cmd::{self, Cmd};
use super::conn::{Conn, Event};
use super::edge::{self, Rect, Tracker};
use super::faillog::{self, FailLog};
use super::forward::{self, Action, Active, Keyboard};
use super::ident::{self, Identity};
use super::pair::{self, Pending, PendingKind, Verdict};
use super::proto::Msg;
use super::statefile::{self, DevLine, DevState, Head, StateFile};
use super::tls;
use crate::config::DeskflowCfg;
use crate::deskflowcfg::Generated;
use crate::kanshi::Side;

/// The paths and the host name the daemon works with.
pub struct RunEnv {
    /// `generated/deskflow`, the daemon's only configuration.
    pub generated: PathBuf,
    pub identity: PathBuf,
    pub state: PathBuf,
    pub socket: PathBuf,
    pub hostname: String,
    /// The OSD key command without its key ([`osd_key_command`]).
    pub osd_key: Vec<String>,
}

/// The OSD key command of labwc's media key bindings.
const OSD_KEY_CMD: &str = "/usr/bin/thinfloe-menu --osd-key";

/// The command the microphone key runs while a device has the keyboard,
/// without its key: `TF_OSD_KEY_CMD` (`value`) when set, as
/// thinfloe-compose-labwc-config takes it, else the menu's `--osd-key`.
pub fn osd_key_command(value: Option<&str>) -> Vec<String> {
    let v = value.filter(|v| !v.trim().is_empty()).unwrap_or(OSD_KEY_CMD);
    v.split_whitespace().map(str::to_string).collect()
}

/// Starts `cmd key` in a process group of its own, stdin from /dev/null;
/// the caller reaps it.
/// At most this many OSD key commands run at once; a press beyond them is
/// dropped, so a hung command cannot pile up children.
const OSD_KEYS_MAX: usize = 4;

/// A media key the daemon runs itself: nothing when `enabled` (the volume
/// key switch) is off or [OSD_KEYS_MAX] commands are still running.
fn press_osd_key(enabled: bool, cmd: &[String], running: &mut Vec<std::process::Child>, key: &str) {
    if !enabled {
        return;
    }
    running.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
    if running.len() >= OSD_KEYS_MAX {
        eprintln!("thinfloe-menu --deskflow: the {key} key: {OSD_KEYS_MAX} commands still run");
        return;
    }
    match spawn_osd_key(cmd, key) {
        Ok(c) => running.push(c),
        Err(e) => eprintln!("thinfloe-menu --deskflow: the {key} key: {e}"),
    }
}

fn spawn_osd_key(cmd: &[String], key: &str) -> io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let (prog, args) = cmd
        .split_first()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "no OSD key command"))?;
    std::process::Command::new(prog)
        .args(args)
        .arg(key)
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .spawn()
}

/// The longest poll; the watchdog's heartbeat is stored once per turn.
const TURN: Duration = Duration::from_millis(250);
/// The watchdog ends the process when the loop has not turned for this long.
const STALL: Duration = Duration::from_secs(2);
/// The watchdog's exit status (EX_SOFTWARE).
const STALL_EXIT: i32 = 70;
/// At most this many connections may be in TLS, Hello or the verdict.
const MAX_HANDSHAKES: usize = 4;
/// At most this many connections are taken from the listener per turn.
const MAX_ACCEPTS: usize = 64;
/// A capture whose pointer lock is not active this long after it was
/// asked for is given up.
const LOCK_WAIT: Duration = Duration::from_millis(500);
/// The largest generated/deskflow read.
const CONFIG_MAX: u64 = 256 * 1024;

pub fn run(env: RunEnv) -> Result<(), String> {
    let _lock = lock(&env.state)?;
    let cfg = read_config(&env.generated).map_err(|e| fail(&env.state, e))?;
    if !cfg.deskflow.listen {
        return Ok(());
    }
    let id =
        ident::load_or_create(&env.identity, &env.hostname).map_err(|e| fail(&env.state, format!("identity: {e}")))?;
    let listener = listen(cfg.deskflow.port).map_err(|e| fail(&env.state, e))?;
    let sock = cmd::bind(&env.socket).map_err(|e| fail(&env.state, format!("command socket: {e}")))?;
    let wake = Wake::new().map_err(|e| fail(&env.state, e))?;
    wake.install_signals();
    // The menu starts the daemon with SIGHUP blocked, so a reload before
    // the handler was set waits for it instead of ending the daemon.
    unblock_sighup();
    let capture = Capture::connect().map_err(|e| fail(&env.state, e))?;
    let beat = Watchdog::start();
    let mut d = Daemon::new(env, cfg, &id, listener, sock, Some(capture));
    d.clip = match Clip::connect() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("thinfloe-menu --deskflow: no clipboard sharing: {e}");
            None
        }
    };
    d.serve(&wake, Some(&beat))
}

/// Unblock SIGHUP for this thread; a reload that waited arrives now.
fn unblock_sighup() {
    // SAFETY: a sigset on the stack, filled by sigemptyset and sigaddset.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGHUP);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
    }
}

/// Writes the error head and hands the text back.
fn fail(state: &Path, why: String) -> String {
    let s = StateFile {
        head: Head::Error(why.clone()),
        devices: Vec::new(),
    };
    if let Err(e) = statefile::write(state, &s) {
        eprintln!("thinfloe-menu --deskflow: {}: {e}", state.display());
    }
    why
}

/// One daemon per state file: an exclusive lock on `<state>.lock`, held
/// for the life of the process, comes before the command socket, whose
/// bind removes the socket of a running daemon.
fn lock(state: &Path) -> Result<File, String> {
    let mut path = state.as_os_str().to_owned();
    path.push(".lock");
    let path = PathBuf::from(path);
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    // SAFETY: flock on a valid descriptor owned by f.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let e = io::Error::last_os_error();
        if e.kind() == ErrorKind::WouldBlock {
            return Err("another deskflow daemon runs".into());
        }
        return Err(format!("{}: {e}", path.display()));
    }
    Ok(f)
}

/// The configuration in generated/deskflow; none there means the switch
/// is off.
fn read_config(path: &Path) -> Result<Generated, String> {
    let Some(bytes) = crate::atomic::read(path, CONFIG_MAX).map_err(|e| format!("generated/deskflow: {e}"))? else {
        return Ok(Generated::default());
    };
    let text = String::from_utf8(bytes).map_err(|_| "generated/deskflow: not UTF-8".to_string())?;
    let g = crate::deskflowcfg::parse(&text)?;
    crate::deskflowcfg::validate(&g.deskflow)?;
    Ok(g)
}

fn listen(port: u16) -> Result<TcpListener, String> {
    let l = TcpListener::bind(("0.0.0.0", port)).map_err(|e| match e.kind() {
        ErrorKind::AddrInUse => format!("port {port} in use"),
        _ => format!("port {port}: {e}"),
    })?;
    l.set_nonblocking(true).map_err(|e| format!("port {port}: {e}"))?;
    Ok(l)
}

/// The write end of the signal pipe, for the handler.
static SIGNAL_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(sig: libc::c_int) {
    let fd = SIGNAL_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        // SAFETY: write and errno are async-signal-safe; errno is put back
        // so the interrupted code does not see the handler's.
        unsafe {
            let errno = *libc::__errno_location();
            let b = sig as u8;
            libc::write(fd, (&b as *const u8).cast::<c_void>(), 1);
            *libc::__errno_location() = errno;
        }
    }
}

/// A self-pipe: one byte per signal (its number), read by the loop.
struct Wake {
    rd: OwnedFd,
    wr: OwnedFd,
}

impl Wake {
    fn new() -> Result<Wake, String> {
        let mut fds = [0 as RawFd; 2];
        // SAFETY: pipe2 fills two descriptors on success, owned from here.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } != 0 {
            return Err(format!("pipe: {}", io::Error::last_os_error()));
        }
        // SAFETY: both are fresh descriptors nobody else owns.
        Ok(unsafe {
            Wake {
                rd: OwnedFd::from_raw_fd(fds[0]),
                wr: OwnedFd::from_raw_fd(fds[1]),
            }
        })
    }

    /// SIGTERM and SIGHUP write to this pipe.
    fn install_signals(&self) {
        SIGNAL_FD.store(self.wr.as_raw_fd(), Ordering::Relaxed);
        for sig in [libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: a zeroed sigaction with a handler that only writes
            // to a pipe.
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = on_signal as *const () as libc::sighandler_t;
                sa.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut sa.sa_mask);
                libc::sigaction(sig, &sa, std::ptr::null_mut());
            }
        }
    }

    /// Delivers a signal number as the handler would (tests).
    #[cfg(test)]
    fn send(&self, sig: libc::c_int) {
        let b = sig as u8;
        // SAFETY: a one-byte write to our own pipe.
        unsafe { libc::write(self.wr.as_raw_fd(), (&b as *const u8).cast::<c_void>(), 1) };
    }

    /// The signals that arrived since the last call.
    fn take(&self) -> Vec<libc::c_int> {
        let mut buf = [0u8; 64];
        let mut sigs = Vec::new();
        loop {
            // SAFETY: a read into a buffer of its length.
            let n = unsafe { libc::read(self.rd.as_raw_fd(), buf.as_mut_ptr().cast::<c_void>(), buf.len()) };
            if n <= 0 {
                return sigs;
            }
            sigs.extend(buf[..n as usize].iter().map(|&b| b as libc::c_int));
        }
    }
}

impl Drop for Wake {
    fn drop(&mut self) {
        let _ = SIGNAL_FD.compare_exchange(self.wr.as_raw_fd(), -1, Ordering::Relaxed, Ordering::Relaxed);
    }
}

/// A thread that ends the process when the loop stalls: the compositor then
/// drops the pointer lock and the keyboard focus (spec, Release always
/// wins).
struct Watchdog {
    start: Instant,
    beat: Arc<AtomicU64>,
}

impl Watchdog {
    fn start() -> Watchdog {
        let start = Instant::now();
        let beat = Arc::new(AtomicU64::new(0));
        let seen = beat.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let now = start.elapsed().as_millis() as u64;
                if now.saturating_sub(seen.load(Ordering::Relaxed)) > STALL.as_millis() as u64 {
                    eprintln!("thinfloe-menu --deskflow: the event loop stalled; exiting");
                    // SAFETY: ends the process at once, without unwinding.
                    unsafe { libc::_exit(STALL_EXIT) };
                }
            }
        });
        Watchdog { start, beat }
    }

    fn beat(&self) {
        self.beat
            .store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
}

fn epoch_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// What a turn decides.
enum Next {
    Go,
    Exit(Result<(), String>),
}

/// A strip shown for a connected device, with what the capture needs of
/// it. Its id is the connection's.
struct Strip {
    spec: StripSpec,
    /// The output's logical rectangle.
    out: Rect,
    side: Side,
    /// The device's screen, from its DINF.
    dev: (i16, i16),
}

/// The pointer on a strip: where, and the push into the edge so far.
struct Hover {
    strip: u32,
    at: (f64, f64),
    push: edge::Resistance,
}

/// Who has the keyboard and the pointer.
enum Mode {
    /// ThinFloe.
    Idle,
    /// A capture of this strip waits for its pointer lock.
    Locking {
        strip: u32,
        since: Instant,
    },
    Active(Active),
}

struct Daemon {
    env: RunEnv,
    cfg: DeskflowCfg,
    /// generated/deskflow's `mic_mute_local`.
    mic_mute_local: bool,
    /// The OSD key commands started and not yet reaped.
    osd_keys: Vec<std::process::Child>,
    tls: Arc<ServerConfig>,
    fingerprint: String,
    listener: TcpListener,
    sock: UnixDatagram,
    capture: Option<Capture>,
    /// The clipboard bridge; none when the compositor has no data control
    /// or the bridge failed.
    clip: Option<Clip>,
    sync: ClipSync,
    conns: Vec<Conn>,
    pending: Pending,
    /// The last DINF size of each (name, fingerprint).
    screens: HashMap<(String, String), (u32, u32)>,
    /// Each line's state and when it took it.
    since: HashMap<(String, String), (DevState, i64)>,
    /// The text last written to the state file.
    written: Option<String>,
    /// The id the next connection gets.
    next_id: usize,
    /// Failures before pairing logged lately.
    failed: FailLog,
    /// The compositor's outputs, as of the last `OutputsChanged`.
    outputs: Vec<OutputGeom>,
    strips: Vec<Strip>,
    /// Connected devices without a strip: their output is missing, or
    /// their edge touches another output.
    offline: Vec<usize>,
    busy: Vec<usize>,
    hover: Option<Hover>,
    mode: Mode,
    kb: Keyboard,
    /// The `Unlocked` events that our own `end_capture` calls queued.
    own_unlocks: usize,
}

impl Daemon {
    fn new(
        env: RunEnv,
        cfg: Generated,
        id: &Identity,
        listener: TcpListener,
        sock: UnixDatagram,
        capture: Option<Capture>,
    ) -> Daemon {
        Daemon {
            env,
            cfg: cfg.deskflow,
            mic_mute_local: cfg.mic_mute_local,
            osd_keys: Vec::new(),
            tls: tls::server_config(id),
            fingerprint: ident::fingerprint(&id.cert_der),
            listener,
            sock,
            outputs: capture.as_ref().map(Capture::outputs).unwrap_or_default(),
            capture,
            clip: None,
            sync: ClipSync::default(),
            conns: Vec::new(),
            pending: Pending::default(),
            screens: HashMap::new(),
            since: HashMap::new(),
            written: None,
            next_id: 1,
            failed: FailLog::default(),
            strips: Vec::new(),
            offline: Vec::new(),
            busy: Vec::new(),
            hover: None,
            mode: Mode::Idle,
            kb: Keyboard::default(),
            own_unlocks: 0,
        }
    }

    /// The loop, until SIGTERM, the switch going off, the compositor
    /// going away or an error.
    fn serve(&mut self, wake: &Wake, beat: Option<&Watchdog>) -> Result<(), String> {
        self.write_state();
        let result = loop {
            if let Some(b) = beat {
                b.beat();
            }
            if let Next::Exit(r) = self.turn(wake) {
                break r;
            }
        };
        // Every connection ends with the daemon, the device that has the
        // pointer first gets its keys and buttons up and COUT, then each a
        // CBYE; a failure has written its head already.
        self.release("the daemon ends");
        for c in &mut self.conns {
            c.send(Msg::Cbye);
            c.close("the daemon ends");
            c.flush();
        }
        if let Some(c) = self.capture.as_mut() {
            let _ = c.flush();
        }
        self.conns.clear();
        if result.is_ok() {
            self.write_state();
        }
        result
    }

    fn turn(&mut self, wake: &Wake) -> Next {
        self.reap();
        if let Some(c) = self.capture.as_mut()
            && c.flush().is_err()
        {
            return Next::Exit(Ok(()));
        }
        if let Some(e) = self.clip.as_mut().and_then(|c| c.flush().err()) {
            self.drop_clip(&e);
        }
        const IN: libc::c_short = libc::POLLIN;
        let mut fds = vec![
            pollfd(wake.rd.as_raw_fd(), IN),
            pollfd(self.sock.as_raw_fd(), IN),
            pollfd(self.listener.as_raw_fd(), IN),
            pollfd(self.capture.as_ref().map_or(-1, |c| c.fd().as_raw_fd()), IN),
            pollfd(self.clip.as_ref().map_or(-1, |c| c.fd().as_raw_fd()), IN),
        ];
        let fixed = fds.len();
        for c in &self.conns {
            let ev = if c.wants_write() { IN | libc::POLLOUT } else { IN };
            fds.push(pollfd(c.fd(), ev));
        }
        let pipes = fds.len();
        if let Some(c) = &self.clip {
            fds.extend(c.pipe_fds().into_iter().map(|(fd, ev)| pollfd(fd, ev)));
        }
        let timeout = self.poll_timeout(Instant::now());
        // SAFETY: fds is a valid array of its length for the call.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == ErrorKind::Interrupted {
                return Next::Go;
            }
            return Next::Exit(Err(fail(&self.env.state, format!("poll: {e}"))));
        }
        let ready = |i: usize| fds[i].revents != 0;
        let now = Instant::now();
        if ready(0) {
            for sig in wake.take() {
                let next = match sig {
                    libc::SIGTERM => {
                        self.release("SIGTERM");
                        Next::Exit(Ok(()))
                    }
                    libc::SIGHUP => self.reload(),
                    _ => Next::Go,
                };
                if let Next::Exit(_) = next {
                    return next;
                }
            }
        }
        if ready(1)
            && let Next::Exit(r) = self.commands()
        {
            return Next::Exit(r);
        }
        if ready(3) && self.capture.is_some() {
            // Wayland gone: labwc has ended, and the session with it.
            if fds[3].revents & (libc::POLLERR | libc::POLLHUP) != 0 {
                return Next::Exit(Ok(()));
            }
            match self.capture.as_mut().map(Capture::dispatch) {
                Some(Ok(events)) => self.on_capture(events, now),
                _ => return Next::Exit(Ok(())),
            }
        }
        let known = self.conns.len();
        for i in 0..known {
            let revents = fds[fixed + i].revents;
            if revents == 0 {
                continue;
            }
            let events = self.conns[i].on_readable(now);
            self.on_events(i, events, now);
        }
        self.on_clip(ready(4).then_some(fds[4].revents), &fds[pipes..], now);
        if ready(2) {
            self.accept(now);
        }
        self.timers(now);
        for c in &mut self.conns {
            c.tick(now);
        }
        self.check_device();
        self.update_strips();
        for c in &mut self.conns {
            c.flush();
        }
        // A connection that ends before its client is named shows nowhere
        // else: not in the state file, not in Settings.
        for c in &self.conns {
            if let Some(why) = c.closing()
                && c.name().is_none()
                && self.failed.should_log(c.peer(), why, now)
            {
                eprintln!("thinfloe-menu --deskflow: {}", faillog::line(c.peer(), why));
            }
        }
        self.conns.retain(|c| c.closing().is_none());
        self.write_state();
        Next::Go
    }

    /// The poll timeout: a turn, or less when a key repeat or the lock
    /// deadline is due sooner.
    fn poll_timeout(&self, now: Instant) -> libc::c_int {
        let due = match &self.mode {
            Mode::Idle => None,
            Mode::Locking { since, .. } => Some(*since + LOCK_WAIT),
            Mode::Active(a) => a.deadline(),
        };
        let due = match (due, self.clip.as_ref().and_then(Clip::deadline)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let wait = due.map_or(TURN, |d| d.saturating_duration_since(now).min(TURN));
        // Rounded up, so the loop does not spin before the deadline.
        wait.as_micros().div_ceil(1000) as libc::c_int
    }

    /// The lock deadline and the key repeat.
    fn timers(&mut self, now: Instant) {
        match &mut self.mode {
            Mode::Idle => {}
            Mode::Locking { since, .. } => {
                if now.saturating_duration_since(*since) >= LOCK_WAIT {
                    self.release("no pointer lock within 500 ms");
                }
            }
            Mode::Active(a) => {
                let acts = forward::tick(a, &self.kb, now);
                self.forward(acts);
            }
        }
    }

    /// The events of the Wayland connection. A changed output layout is
    /// taken first, so a capture whose output went is released for that
    /// reason.
    fn on_capture(&mut self, events: Vec<CapEvent>, now: Instant) {
        if events.iter().any(|e| matches!(e, CapEvent::OutputsChanged)) {
            if let Some(c) = &self.capture {
                self.outputs = c.outputs();
            }
            self.update_strips();
        }
        for ev in &events {
            self.on_cap_event(ev, now);
        }
    }

    fn on_cap_event(&mut self, ev: &CapEvent, now: Instant) {
        match *ev {
            CapEvent::Enter { strip, x, y } => {
                self.hover = Some(Hover {
                    strip,
                    at: (x, y),
                    push: edge::Resistance::new(self.cfg.edge_resistance.px()),
                });
            }
            CapEvent::Motion { x, y } => {
                if let Some(h) = self.hover.as_mut() {
                    h.at = (x, y);
                }
            }
            CapEvent::Leave { strip } => {
                if self.hover.as_ref().is_some_and(|h| h.strip == strip) {
                    self.hover = None;
                }
            }
            // The one our own end_capture reported.
            CapEvent::Unlocked if self.own_unlocks > 0 => {
                self.own_unlocks -= 1;
                return;
            }
            _ => {}
        }
        match &mut self.mode {
            Mode::Active(a) => {
                let acts = forward::on_cap_event(a, ev, &mut self.kb, now);
                self.forward(acts);
            }
            Mode::Locking { .. } => {
                self.kb.update(ev);
                match ev {
                    CapEvent::Locked => self.enter_device(),
                    CapEvent::Unlocked => self.release("unlocked"),
                    _ => {}
                }
            }
            Mode::Idle => {
                self.kb.update(ev);
                match *ev {
                    CapEvent::Relative {
                        dx_unaccel, dy_unaccel, ..
                    } if dx_unaccel.is_finite() && dy_unaccel.is_finite() => {
                        self.push_edge(dx_unaccel, dy_unaccel, now)
                    }
                    _ => {}
                }
            }
        }
    }

    /// Motion on a strip pushes into its edge; enough of it begins the
    /// capture.
    fn push_edge(&mut self, dx: f64, dy: f64, now: Instant) {
        let Some(h) = self.hover.as_mut() else {
            return;
        };
        let Some(s) = self.strips.iter().find(|s| s.spec.id == h.strip) else {
            return;
        };
        if !h.push.push(s.side, dx, dy, now) {
            return;
        }
        h.push.reset();
        let strip = h.strip;
        if let Some(c) = self.capture.as_mut() {
            c.begin_capture(strip);
        }
        self.mode = Mode::Locking { strip, since: now };
    }

    /// The lock is active: CINN where the pointer left the edge.
    fn enter_device(&mut self) {
        let Mode::Locking { strip, .. } = self.mode else {
            return;
        };
        let Some((out, side, dev, rect)) = self
            .strips
            .iter()
            .find(|s| s.spec.id == strip)
            .map(|s| (s.out, s.side, s.dev, s.spec.rect))
        else {
            return self.release("the strip went");
        };
        let middle = (
            f64::from(rect.x) + f64::from(rect.w) / 2.0,
            f64::from(rect.y) + f64::from(rect.h) / 2.0,
        );
        let at = self
            .hover
            .as_ref()
            .filter(|h| h.strip == strip)
            .map_or(middle, |h| h.at);
        let entry = edge::entry_point(out, side, at, dev);
        let device = strip as usize;
        let mods = self.kb.mask();
        let Some(i) = self.conns.iter().position(|c| c.id() == device && c.trusted()) else {
            return self.release("connection closed");
        };
        let shares = self.shares_clipboard(&self.conns[i]);
        self.conns[i].enter(entry.0, entry.1, mods);
        self.mode = Mode::Active(Active::new(device, Tracker::new(side, entry, dev), entry));
        let acts = self.sync.on_enter(device, shares);
        self.clip_acts(acts);
    }

    /// The device's Clipboard switch; off for a connection without one.
    fn shares_clipboard(&self, c: &Conn) -> bool {
        self.cfg
            .devices
            .iter()
            .find(|d| c.name() == Some(&d.name) && c.fingerprint() == Some(&d.fingerprint))
            .is_some_and(|d| d.clipboard)
    }

    /// The trusted connections and their Clipboard switch, for ClipSync.
    fn clip_live(&self) -> Vec<(usize, bool)> {
        self.conns
            .iter()
            .filter(|c| c.trusted())
            .map(|c| (c.id(), self.shares_clipboard(c)))
            .collect()
    }

    /// The bridge's descriptor and pipes, as poll left them. An error
    /// drops the bridge; the daemon serves on without a clipboard.
    fn on_clip(&mut self, revents: Option<libc::c_short>, pipes: &[libc::pollfd], now: Instant) {
        let Some(clip) = self.clip.as_mut() else {
            return;
        };
        let mut events = Vec::new();
        if let Some(r) = revents {
            if r & (libc::POLLERR | libc::POLLHUP) != 0 {
                return self.drop_clip("the compositor closed the connection");
            }
            match clip.dispatch(now) {
                Ok(ev) => events = ev,
                Err(e) => return self.drop_clip(&e),
            }
        }
        for p in pipes.iter().filter(|p| p.revents != 0) {
            events.extend(clip.on_pipe_ready(p.fd, now));
        }
        events.extend(clip.tick(now));
        let active = match &self.mode {
            Mode::Active(a) => Some(a.device),
            _ => None,
        };
        for ev in events {
            let acts = match ev {
                ClipEvent::Changed(text) => self.sync.on_local(text, active, &self.clip_live()),
                ClipEvent::Cleared => self.sync.on_cleared(&self.clip_live()),
            };
            self.clip_acts(acts);
        }
    }

    fn drop_clip(&mut self, why: &str) {
        eprintln!("thinfloe-menu --deskflow: clipboard sharing ends: {why}");
        self.clip = None;
    }

    fn clip_acts(&mut self, acts: Vec<ClipAct>) {
        for act in acts {
            match act {
                ClipAct::Send { conn, text } => {
                    if let Some(c) = self.conns.iter_mut().find(|c| c.id() == conn) {
                        c.clipboard(&text);
                    }
                }
                ClipAct::Grab { conn } => {
                    if let Some(c) = self.conns.iter_mut().find(|c| c.id() == conn) {
                        c.grab();
                    }
                }
                ClipAct::SetLocal(text) => {
                    if let Some(c) = self.clip.as_mut() {
                        c.set_text(text);
                    }
                }
            }
        }
    }

    fn forward(&mut self, acts: Vec<Action>) {
        for act in acts {
            match act {
                Action::Send(m) => {
                    let Mode::Active(a) = &self.mode else {
                        return;
                    };
                    let id = a.device;
                    if let Some(c) = self.conns.iter_mut().find(|c| c.id() == id) {
                        c.send(m);
                    }
                }
                Action::Release(why) => self.release(why),
                Action::LocalMicMute => self.local_mic_mute(),
            }
        }
    }

    /// The microphone key while a device has the keyboard: labwc's binding
    /// cannot act, so the daemon runs it, when the volume keys are on. It
    /// does not wait; `reap` collects the command.
    fn local_mic_mute(&mut self) {
        press_osd_key(self.mic_mute_local, &self.env.osd_key, &mut self.osd_keys, "mic-mute");
    }

    /// Collects the OSD key commands that ended.
    fn reap(&mut self) {
        self.osd_keys.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
    }

    /// Gives keyboard and pointer back to ThinFloe at once: the device gets
    /// a key up for every key and button sent down and COUT, the lock and
    /// the keyboard focus go.
    fn release(&mut self, reason: &str) {
        match std::mem::replace(&mut self.mode, Mode::Idle) {
            Mode::Idle => return,
            Mode::Locking { .. } => {}
            Mode::Active(mut a) => {
                let msgs = forward::release(&mut a, &self.kb);
                if let Some(c) = self.conns.iter_mut().find(|c| c.id() == a.device) {
                    for m in msgs {
                        c.send(m);
                    }
                }
            }
        }
        if let Some(c) = self.capture.as_mut()
            && c.end_capture()
        {
            self.own_unlocks += 1;
        }
        if let Some(h) = self.hover.as_mut() {
            h.push.reset();
        }
        eprintln!("thinfloe-deskflow: released ({reason})");
    }

    /// The device of a capture must still be connected.
    fn check_device(&mut self) {
        let id = match &self.mode {
            Mode::Idle => return,
            Mode::Locking { strip, .. } => *strip as usize,
            Mode::Active(a) => a.device,
        };
        if !self.conns.iter().any(|c| c.id() == id && c.trusted()) {
            self.release("connection closed");
        }
    }

    /// The strips the devices need now: one per connected device whose
    /// output exists and whose edge is free. Also the ids of the connected
    /// devices without an output and of those whose edge is not free.
    fn wanted_strips(&self) -> (Vec<Strip>, Vec<usize>, Vec<usize>) {
        let rect = |o: &OutputGeom| Rect {
            x: o.x,
            y: o.y,
            w: o.w,
            h: o.h,
        };
        let (mut strips, mut offline, mut busy) = (Vec::new(), Vec::new(), Vec::new());
        for c in self.conns.iter().filter(|c| c.trusted()) {
            let Some((w, h)) = c.screen().filter(|(w, h)| *w > 0 && *h > 0) else {
                continue;
            };
            let Some(d) = self
                .cfg
                .devices
                .iter()
                .find(|d| c.name() == Some(&d.name) && c.fingerprint() == Some(&d.fingerprint))
            else {
                continue;
            };
            let Some(o) = self.outputs.iter().find(|o| o.matches(&d.output)) else {
                offline.push(c.id());
                continue;
            };
            let others: Vec<Rect> = self.outputs.iter().filter(|p| p.name != o.name).map(rect).collect();
            if !edge::edge_free(rect(o), d.side, &others) {
                busy.push(c.id());
                continue;
            }
            strips.push(Strip {
                spec: StripSpec {
                    id: c.id() as u32,
                    output: o.name.clone(),
                    rect: edge::strip_rect(rect(o), d.side),
                },
                out: rect(o),
                side: d.side,
                dev: (w, h),
            });
        }
        (strips, offline, busy)
    }

    /// Shows the strips the devices need; a capture whose strip goes or
    /// moves is released first.
    fn update_strips(&mut self) {
        let (strips, offline, busy) = self.wanted_strips();
        let spec = |all: &[Strip], id: u32| all.iter().find(|s| s.spec.id == id).map(|s| s.spec.clone());
        let captured = match &self.mode {
            Mode::Idle => None,
            Mode::Locking { strip, .. } => Some(*strip),
            Mode::Active(a) => Some(a.device as u32),
        };
        if let Some(id) = captured {
            let now = spec(&strips, id);
            if now != spec(&self.strips, id) {
                let why = match now {
                    Some(_) => "the output changed",
                    None if offline.contains(&(id as usize)) => "output gone",
                    None if busy.contains(&(id as usize)) => "the edge is not free",
                    None => "the device went",
                };
                self.release(why);
            }
        }
        if let Some(h) = &self.hover
            && (spec(&strips, h.strip).is_none() || spec(&strips, h.strip) != spec(&self.strips, h.strip))
        {
            self.hover = None;
        }
        let specs: Vec<StripSpec> = strips.iter().map(|s| s.spec.clone()).collect();
        if let Some(c) = self.capture.as_mut() {
            // A strip the compositor closed counts as missing.
            let closed = c.take_closed(Instant::now());
            if closed || self.strips.iter().map(|s| &s.spec).ne(specs.iter()) {
                c.set_strips(&specs);
            }
        }
        (self.strips, self.offline, self.busy) = (strips, offline, busy);
    }

    fn accept(&mut self, now: Instant) {
        for _ in 0..MAX_ACCEPTS {
            let stream: TcpStream = match self.listener.accept() {
                Ok((s, _)) => s,
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    eprintln!("thinfloe-menu --deskflow: accept: {e}");
                    return;
                }
            };
            if self.conns.iter().filter(|c| c.in_handshake()).count() >= MAX_HANDSHAKES {
                drop(stream);
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            match Conn::new(id, stream, self.tls.clone(), now) {
                Ok(c) => self.conns.push(c),
                Err(e) => eprintln!("thinfloe-menu --deskflow: {e}"),
            }
        }
    }

    fn on_events(&mut self, i: usize, events: Vec<Event>, now: Instant) {
        for ev in events {
            match ev {
                Event::Named(name) => self.judge(i, &name, now),
                Event::Screen { w, h } => {
                    if let Some(fp) = self.conns[i].fingerprint() {
                        let key = (name_of(&self.conns[i]), fp.to_string());
                        self.screens.insert(key, (w.max(0) as u32, h.max(0) as u32));
                    }
                }
                Event::Clipboard(text) => {
                    let acts = self.sync.on_remote(self.conns[i].id(), text);
                    self.clip_acts(acts);
                }
                Event::Grabbed => {
                    let acts = self.sync.on_grabbed(self.conns[i].id(), &self.clip_live());
                    self.clip_acts(acts);
                }
            }
        }
    }

    /// The verdict on a named connection, given in the same pass.
    fn judge(&mut self, i: usize, name: &str, now: Instant) {
        let fp = self.conns[i].fingerprint().unwrap_or_default().to_string();
        let v = pair::judge(&self.cfg.devices, name, &fp);
        match v {
            Verdict::Trusted(_) => {
                let busy = self
                    .conns
                    .iter()
                    .enumerate()
                    .any(|(j, c)| j != i && c.trusted() && c.name() == Some(name));
                if busy {
                    self.conns[i].refuse_busy(now);
                    return;
                }
            }
            Verdict::Pending => self.pending.note(name, &fp, PendingKind::New, epoch_now() as u64),
            Verdict::FingerprintChanged => {
                self.pending
                    .note(name, &fp, PendingKind::FingerprintChanged, epoch_now() as u64)
            }
        }
        self.conns[i].set_verdict(v, now);
        if let Verdict::Trusted(_) = v {
            let shares = self.shares_clipboard(&self.conns[i]);
            self.conns[i].set_clipboard(shares);
        }
    }

    fn commands(&mut self) -> Next {
        let mut buf = [0u8; cmd::DATAGRAM_MAX + 1];
        loop {
            let n = match self.sock.recv(&mut buf) {
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Next::Go,
            };
            match cmd::decode(&buf[..n]) {
                Some(Cmd::Reload) => {
                    if let Next::Exit(r) = self.reload() {
                        return Next::Exit(r);
                    }
                }
                Some(Cmd::Reject { name, fingerprint }) => self.pending.reject(&name, &fingerprint),
                None => eprintln!("thinfloe-menu --deskflow: a command it does not know"),
            }
        }
    }

    /// Reads generated/deskflow again.
    fn reload(&mut self) -> Next {
        let Generated {
            deskflow: cfg,
            mic_mute_local,
        } = match read_config(&self.env.generated) {
            Ok(c) => c,
            Err(e) => return Next::Exit(Err(fail(&self.env.state, e))),
        };
        self.mic_mute_local = mic_mute_local;
        if !cfg.listen {
            return Next::Exit(Ok(()));
        }
        if cfg.port != self.cfg.port {
            match listen(cfg.port) {
                Ok(l) => self.listener = l,
                Err(e) => return Next::Exit(Err(fail(&self.env.state, e))),
            }
        }
        for c in &mut self.conns {
            if !c.trusted() {
                continue;
            }
            let (name, fp) = (c.name().unwrap_or_default(), c.fingerprint().unwrap_or_default());
            if !cfg.devices.iter().any(|d| d.name == name && d.fingerprint == fp) {
                c.close("the device was forgotten");
            }
        }
        self.pending.forget_trusted(&cfg.devices);
        let before = self.clip_live();
        self.cfg = cfg;
        for (conn, shares) in self.clip_live() {
            if !before.contains(&(conn, shares)) {
                self.sync.forget(conn);
            }
        }
        for i in 0..self.conns.len() {
            if self.conns[i].trusted() {
                let shares = self.shares_clipboard(&self.conns[i]);
                self.conns[i].set_clipboard(shares);
            }
        }
        Next::Go
    }

    /// The state file as it is now.
    fn state(&mut self) -> StateFile {
        let now = epoch_now();
        let mut lines: Vec<(String, String, DevState, Vec<String>)> = Vec::new();
        let active = match &self.mode {
            Mode::Active(a) => Some(a.device),
            _ => None,
        };
        for d in &self.cfg.devices {
            let on = self.conns.iter().find(|c| {
                c.trusted()
                    && c.screen().is_some()
                    && c.name() == Some(&d.name)
                    && c.fingerprint() == Some(&d.fingerprint)
            });
            let (state, flags) = match on.map(Conn::id) {
                None => (DevState::Offline, Vec::new()),
                Some(id) if active == Some(id) => (DevState::Active, Vec::new()),
                Some(id) if self.offline.contains(&id) => (DevState::Offline, Vec::new()),
                Some(id) if self.busy.contains(&id) => (DevState::Connected, vec!["edge-busy".to_string()]),
                Some(_) => (DevState::Connected, Vec::new()),
            };
            lines.push((d.name.clone(), d.fingerprint.clone(), state, flags));
        }
        for e in self.pending.entries() {
            let flags = match e.kind {
                PendingKind::FingerprintChanged => vec!["fp-changed".to_string()],
                PendingKind::New => Vec::new(),
            };
            lines.push((e.name.clone(), e.fingerprint.clone(), DevState::Pending, flags));
        }
        for (n, f) in self.pending.rejected() {
            if !self.cfg.devices.iter().any(|d| &d.name == n && &d.fingerprint == f) {
                lines.push((n.clone(), f.clone(), DevState::Rejected, Vec::new()));
            }
        }
        let mut since = HashMap::new();
        let devices = lines
            .into_iter()
            .map(|(name, fingerprint, state, flags)| {
                let key = (name.clone(), fingerprint.clone());
                let t = match self.since.get(&key) {
                    Some(&(s, t)) if s == state => t,
                    _ => now,
                };
                since.insert(key.clone(), (state, t));
                let (width, height) = self.screens.get(&key).copied().unwrap_or((0, 0));
                DevLine {
                    name,
                    fingerprint,
                    state,
                    since: t,
                    width,
                    height,
                    flags,
                }
            })
            .collect();
        self.since = since;
        let listen = self
            .listener
            .local_addr()
            .map_or_else(|_| format!("0.0.0.0:{}", self.cfg.port), |a| a.to_string());
        StateFile {
            head: Head::SelfInfo {
                fingerprint: self.fingerprint.clone(),
                listen,
            },
            devices,
        }
    }

    /// Writes the state file when a line changed.
    fn write_state(&mut self) {
        let text = statefile::render(&self.state());
        if self.written.as_deref() == Some(text.as_str()) {
            return;
        }
        if let Err(e) = crate::atomic::write(&self.env.state, text.as_bytes(), 0o600) {
            eprintln!("thinfloe-menu --deskflow: {}: {e}", self.env.state.display());
        }
        self.written = Some(text);
    }
}

fn name_of(c: &Conn) -> String {
    c.name().unwrap_or_default().to_string()
}

fn pollfd(fd: RawFd, events: libc::c_short) -> libc::pollfd {
    libc::pollfd { fd, events, revents: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deskflow::proto::{self, Msg};
    use crate::deskflow::tls::testclient;
    use crate::testdir::TestDir;
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::thread::JoinHandle;

    type Client = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

    fn test_env(d: &TestDir, s: &TestDir) -> RunEnv {
        RunEnv {
            generated: d.join("generated/deskflow"),
            identity: d.join("deskflow/identity.pem"),
            state: d.join("state"),
            socket: s.socket("s"),
            hostname: "test".into(),
            osd_key: osd_key_command(None),
        }
    }

    #[test]
    fn the_osd_key_command_follows_tf_osd_key_cmd() {
        assert_eq!(osd_key_command(None), ["/usr/bin/thinfloe-menu", "--osd-key"]);
        assert_eq!(osd_key_command(Some(" ")), ["/usr/bin/thinfloe-menu", "--osd-key"]);
        assert_eq!(osd_key_command(Some("sh /tmp/rec.sh")), ["sh", "/tmp/rec.sh"]);
    }

    #[test]
    fn the_osd_key_runs_with_its_key_in_a_group_of_its_own() {
        let d = TestDir::new("deskflow-run", "osd-key");
        let out = d.join("key");
        let cmd = vec![
            "sh".to_string(),
            "-c".into(),
            "printf %s \"$1\" >\"$0\"; exec sleep 1".into(),
            out.to_string_lossy().into_owned(),
        ];
        let mut c = spawn_osd_key(&cmd, "mic-mute").unwrap();
        let pid = c.id() as libc::pid_t;
        // SAFETY: getpgid takes no pointers. The child sleeps, and is not
        // reaped before the wait below, so its group is there to read.
        let pgid = unsafe { libc::getpgid(pid) };
        assert!(c.wait().unwrap().success());
        assert_eq!(pgid, pid);
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "mic-mute");
        assert!(spawn_osd_key(&[], "mic-mute").is_err());
    }

    #[test]
    fn the_microphone_key_runs_nothing_with_the_volume_keys_off() {
        let d = TestDir::new("deskflow-run", "osd-off");
        let out = d.join("key");
        let cmd = vec![
            "sh".to_string(),
            "-c".into(),
            "printf %s \"$1\" >\"$0\"".into(),
            out.to_string_lossy().into_owned(),
        ];
        let mut running = Vec::new();
        press_osd_key(false, &cmd, &mut running, "mic-mute");
        assert!(running.is_empty());
        press_osd_key(true, &cmd, &mut running, "mic-mute");
        assert_eq!(running.len(), 1);
        assert!(running[0].wait().unwrap().success());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "mic-mute");
    }

    #[test]
    fn at_most_four_osd_key_commands_run_at_once() {
        let cmd = vec!["sh".to_string(), "-c".into(), "exec sleep 5".into()];
        let mut running = Vec::new();
        for _ in 0..6 {
            press_osd_key(true, &cmd, &mut running, "mic-mute");
        }
        assert_eq!(running.len(), OSD_KEYS_MAX);
        for c in &mut running {
            c.kill().unwrap();
            c.wait().unwrap();
        }
        // The reaped ones make room again.
        press_osd_key(true, &cmd, &mut running, "mic-mute");
        assert_eq!(running.len(), 1);
        running[0].kill().unwrap();
        running[0].wait().unwrap();
    }

    fn write_cfg(path: &Path, listen: bool, port: u16, devices: &[(&str, &str)]) {
        let devices: Vec<_> = devices.iter().map(|&(n, f)| (n, f, true)).collect();
        write_cfg_clip(path, listen, port, &devices);
    }

    /// As write_cfg, with each device's Clipboard switch.
    fn write_cfg_clip(path: &Path, listen: bool, port: u16, devices: &[(&str, &str, bool)]) {
        let mut t = format!("[deskflow]\nlisten = {listen}\nport = {port}\n");
        for (i, (name, fp, clipboard)) in devices.iter().enumerate() {
            t.push_str(&format!(
                "\n[[deskflow.device]]\nname = \"{name}\"\nfingerprint = \"{fp}\"\noutput = \"DP-{i}\"\nside = \"right-of\"\n"
            ));
            if !clipboard {
                t.push_str("clipboard = false\n");
            }
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, t).unwrap();
    }

    struct Running {
        port: u16,
        wake: Arc<Wake>,
        done: JoinHandle<Result<(), String>>,
    }

    /// The daemon on 127.0.0.1 and a free port, without Wayland, in a
    /// thread of its own.
    fn start(env: RunEnv) -> Running {
        let wake = Arc::new(Wake::new().unwrap());
        let w = wake.clone();
        let (tx, rx) = mpsc::channel();
        let done = std::thread::spawn(move || {
            let cfg = read_config(&env.generated)?;
            let id = ident::load_or_create(&env.identity, &env.hostname)?;
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            // The port goes out once the command socket is bound: a test
            // that sends a command right after start finds it.
            let sock = cmd::bind(&env.socket).unwrap();
            tx.send(listener.local_addr().unwrap().port()).unwrap();
            let mut d = Daemon::new(env, cfg, &id, listener, sock, None);
            // The monitor of the first attached device (write_cfg).
            d.outputs = vec![OutputGeom {
                name: "DP-0".into(),
                description: String::new(),
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            }];
            d.serve(&w, None)
        });
        let port = rx.recv().unwrap();
        Running { port, wake, done }
    }

    fn cert(name: &str) -> rcgen::CertifiedKey<rcgen::KeyPair> {
        rcgen::generate_simple_self_signed(vec![name.to_string()]).unwrap()
    }

    fn connect(port: u16, cert: &rcgen::CertifiedKey<rcgen::KeyPair>) -> Client {
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let conn = rustls::ClientConnection::new(testclient::config(cert), testclient::server_name()).unwrap();
        rustls::StreamOwned::new(conn, tcp)
    }

    fn read_payload(c: &mut Client) -> io::Result<Vec<u8>> {
        let mut len = [0u8; 4];
        c.read_exact(&mut len)?;
        let mut p = vec![0u8; u32::from_be_bytes(len) as usize];
        c.read_exact(&mut p)?;
        Ok(p)
    }

    fn read_msg(c: &mut Client) -> Msg {
        proto::decode(&read_payload(c).unwrap()).unwrap()
    }

    fn send(c: &mut Client, m: &Msg) {
        c.write_all(&proto::frame(&proto::encode(m, 8))).unwrap();
        c.flush().unwrap();
    }

    /// Hello in, HelloBack out: the daemon's answer.
    fn hello(c: &mut Client, name: &str) -> Msg {
        let p = read_payload(c).unwrap();
        assert!(p.starts_with(b"Barrier"), "{p:?}");
        send(
            c,
            &Msg::HelloBack {
                major: 1,
                minor: 8,
                name: name.as_bytes().to_vec(),
            },
        );
        read_msg(c)
    }

    fn line<'a>(s: &'a StateFile, name: &str, fp: &str) -> Option<&'a DevLine> {
        s.devices.iter().find(|d| d.name == name && d.fingerprint == fp)
    }

    /// The state file once it satisfies `ok`, within 5 s.
    fn wait_state(path: &Path, ok: impl Fn(&StateFile) -> bool) -> StateFile {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let s = statefile::parse(&std::fs::read_to_string(path).unwrap_or_default());
            if ok(&s) || Instant::now() > end {
                return s;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn fp_of(c: &rcgen::CertifiedKey<rcgen::KeyPair>) -> String {
        ident::fingerprint(c.cert.der())
    }

    #[test]
    fn pairing_over_loopback() {
        let (d, s) = (TestDir::new("deskflow-run", "pair"), TestDir::sockets("dfp"));
        let env = test_env(&d, &s);
        let (state, gen_path, socket) = (env.state.clone(), env.generated.clone(), env.socket.clone());
        write_cfg(&gen_path, true, 24800, &[]);
        let r = start(env);
        let (laptop, other) = (cert("laptop"), cert("laptop"));
        let (fp1, fp2) = (fp_of(&laptop), fp_of(&other));

        // Unknown: EUNK, and pending in the state file.
        let mut c = connect(r.port, &laptop);
        assert_eq!(hello(&mut c, "laptop"), Msg::Eunk);
        let st = wait_state(&state, |s| line(s, "laptop", &fp1).is_some());
        assert_eq!(line(&st, "laptop", &fp1).unwrap().state, DevState::Pending);
        match &st.head {
            Head::SelfInfo { fingerprint, listen } => {
                assert!(fingerprint.starts_with("v2:sha256:"));
                assert_eq!(listen, &format!("127.0.0.1:{}", r.port));
            }
            h => panic!("{h:?}"),
        }

        // Attached: QINF, then DINF gets CIAK, CROP and DSOP.
        write_cfg(&gen_path, true, 24800, &[("laptop", &fp1)]);
        cmd::send(&socket, &Cmd::Reload).unwrap();
        wait_state(&state, |s| {
            line(s, "laptop", &fp1).is_some_and(|l| l.state == DevState::Offline)
        });
        let mut c = connect(r.port, &laptop);
        assert_eq!(hello(&mut c, "laptop"), Msg::Qinf);
        let dinf = Msg::Dinf {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
            warp: 0,
            mx: 0,
            my: 0,
        };
        send(&mut c, &dinf);
        assert_eq!(
            [read_msg(&mut c), read_msg(&mut c), read_msg(&mut c)],
            [
                Msg::Ciak,
                Msg::Crop,
                Msg::Dsop(crate::deskflow::clipdata::options(true))
            ]
        );
        let st = wait_state(&state, |s| {
            line(s, "laptop", &fp1).is_some_and(|l| l.state == DevState::Connected)
        });
        let l = line(&st, "laptop", &fp1).unwrap();
        assert_eq!((l.state, l.width, l.height), (DevState::Connected, 1920, 1080));

        // The same device again while it is connected: EBSY.
        let mut again = connect(r.port, &laptop);
        assert_eq!(hello(&mut again, "laptop"), Msg::Ebsy);

        // Its name with another certificate: EUNK, pending, fp-changed.
        let mut c2 = connect(r.port, &other);
        assert_eq!(hello(&mut c2, "laptop"), Msg::Eunk);
        let st = wait_state(&state, |s| line(s, "laptop", &fp2).is_some());
        let l = line(&st, "laptop", &fp2).unwrap();
        assert_eq!(
            (l.state, l.flags.clone()),
            (DevState::Pending, vec!["fp-changed".to_string()])
        );
        assert_eq!(line(&st, "laptop", &fp1).unwrap().state, DevState::Connected);

        // Rejected: shown as such and refused at once, never pending again.
        cmd::send(
            &socket,
            &Cmd::Reject {
                name: "laptop".into(),
                fingerprint: fp2.clone(),
            },
        )
        .unwrap();
        wait_state(&state, |s| {
            line(s, "laptop", &fp2).is_some_and(|l| l.state == DevState::Rejected)
        });
        let mut c3 = connect(r.port, &other);
        assert_eq!(hello(&mut c3, "laptop"), Msg::Eunk);
        let st = wait_state(&state, |_| false);
        assert_eq!(line(&st, "laptop", &fp2).unwrap().state, DevState::Rejected);
        assert_eq!(st.devices.len(), 2, "{st:?}");

        // SIGTERM: CBYE to the client, exit 0, and the device is offline.
        r.wake.send(libc::SIGTERM);
        assert_eq!(r.done.join().unwrap(), Ok(()));
        let goodbye = std::iter::from_fn(|| read_payload(&mut c).ok())
            .map(|p| proto::decode(&p).unwrap())
            .find(|m| *m != Msg::Calv);
        assert_eq!(goodbye, Some(Msg::Cbye));
        let st = statefile::parse(&std::fs::read_to_string(&state).unwrap());
        assert_eq!(line(&st, "laptop", &fp1).unwrap().state, DevState::Offline);
    }

    /// The next message that is not a keep-alive.
    fn next_msg(c: &mut Client) -> Msg {
        std::iter::repeat_with(|| read_msg(c))
            .find(|m| *m != Msg::Calv)
            .unwrap()
    }

    /// The messages other than keep-alives within `wait`.
    fn quiet_for(c: &mut Client, wait: Duration) -> Vec<Msg> {
        c.sock.set_read_timeout(Some(wait)).unwrap();
        std::iter::from_fn(|| read_payload(c).ok())
            .map(|p| proto::decode(&p).unwrap())
            .filter(|m| *m != Msg::Calv)
            .collect()
    }

    fn dinf() -> Msg {
        Msg::Dinf {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
            warp: 0,
            mx: 0,
            my: 0,
        }
    }

    #[test]
    fn the_clipboard_switch_reaches_the_device_on_reload() {
        let (d, s) = (TestDir::new("deskflow-run", "clipsw"), TestDir::sockets("dfc"));
        let env = test_env(&d, &s);
        let (state, gen_path, socket) = (env.state.clone(), env.generated.clone(), env.socket.clone());
        let laptop = cert("laptop");
        let fp = fp_of(&laptop);
        write_cfg_clip(&gen_path, true, 24800, &[("laptop", &fp, false)]);
        let r = start(env);
        let mut c = connect(r.port, &laptop);
        assert_eq!(hello(&mut c, "laptop"), Msg::Qinf);
        send(&mut c, &dinf());
        assert_eq!(
            [read_msg(&mut c), read_msg(&mut c), read_msg(&mut c)],
            [
                Msg::Ciak,
                Msg::Crop,
                Msg::Dsop(crate::deskflow::clipdata::options(false))
            ]
        );
        wait_state(&state, |s| {
            line(s, "laptop", &fp).is_some_and(|l| l.state == DevState::Connected)
        });
        for on in [true, false] {
            write_cfg_clip(&gen_path, true, 24800, &[("laptop", &fp, on)]);
            cmd::send(&socket, &Cmd::Reload).unwrap();
            assert_eq!(next_msg(&mut c), Msg::Dsop(crate::deskflow::clipdata::options(on)));
        }
        r.wake.send(libc::SIGTERM);
        assert_eq!(r.done.join().unwrap(), Ok(()));
    }

    #[test]
    fn a_grab_from_one_device_reaches_the_others_that_share() {
        let (d, s) = (TestDir::new("deskflow-run", "clipgrab"), TestDir::sockets("dfg"));
        let env = test_env(&d, &s);
        let gen_path = env.generated.clone();
        let (a, b, off) = (cert("a"), cert("b"), cert("off"));
        let (fa, fb, foff) = (fp_of(&a), fp_of(&b), fp_of(&off));
        write_cfg_clip(
            &gen_path,
            true,
            24800,
            &[("a", &fa, true), ("b", &fb, true), ("off", &foff, false)],
        );
        let r = start(env);
        let mut ca = connect(r.port, &a);
        assert_eq!(hello(&mut ca, "a"), Msg::Qinf);
        let mut cb = connect(r.port, &b);
        assert_eq!(hello(&mut cb, "b"), Msg::Qinf);
        let mut coff = connect(r.port, &off);
        assert_eq!(hello(&mut coff, "off"), Msg::Qinf);
        send(&mut ca, &Msg::Cclp { id: 0, seq: 1 });
        assert!(matches!(next_msg(&mut cb), Msg::Cclp { id: 0, .. }));
        // The device with the switch off hears nothing of it, and its own
        // grab reaches nobody.
        send(&mut coff, &Msg::Cclp { id: 0, seq: 1 });
        assert_eq!(quiet_for(&mut coff, Duration::from_millis(300)), []);
        assert_eq!(quiet_for(&mut ca, Duration::from_millis(300)), []);
        r.wake.send(libc::SIGTERM);
        assert_eq!(r.done.join().unwrap(), Ok(()));
    }

    #[test]
    fn a_reload_with_another_port_listens_there() {
        let (d, s) = (TestDir::new("deskflow-run", "port2"), TestDir::sockets("dfr"));
        let env = test_env(&d, &s);
        let (state, gen_path, socket) = (env.state.clone(), env.generated.clone(), env.socket.clone());
        write_cfg(&gen_path, true, 24800, &[]);
        let r = start(env);
        let port = TcpListener::bind("0.0.0.0:0").unwrap().local_addr().unwrap().port();
        write_cfg(&gen_path, true, port, &[]);
        cmd::send(&socket, &Cmd::Reload).unwrap();
        let want = format!("0.0.0.0:{port}");
        let st = wait_state(
            &state,
            |s| matches!(&s.head, Head::SelfInfo { listen, .. } if *listen == want),
        );
        assert!(
            matches!(&st.head, Head::SelfInfo { listen, .. } if *listen == want),
            "{st:?}"
        );
        let mut c = connect(port, &cert("moved"));
        assert!(read_payload(&mut c).unwrap().starts_with(b"Barrier"));
        r.wake.send(libc::SIGTERM);
        assert_eq!(r.done.join().unwrap(), Ok(()));
    }

    #[test]
    fn an_oversized_frame_and_a_fifth_handshake_are_dropped() {
        let (d, s) = (TestDir::new("deskflow-run", "limits"), TestDir::sockets("dfl"));
        let env = test_env(&d, &s);
        write_cfg(&env.generated, true, 24800, &[]);
        let r = start(env);
        let mut c = connect(r.port, &cert("big"));
        read_payload(&mut c).unwrap();
        c.write_all(&((proto::MAX_MSG + 1) as u32).to_be_bytes()).unwrap();
        c.flush().unwrap();
        let mut b = [0u8; 16];
        assert!(matches!(c.read(&mut b), Ok(0) | Err(_)), "the daemon closes it");

        let idle: Vec<TcpStream> = (0..MAX_HANDSHAKES)
            .map(|_| TcpStream::connect(("127.0.0.1", r.port)).unwrap())
            .collect();
        let mut fifth = TcpStream::connect(("127.0.0.1", r.port)).unwrap();
        fifth.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let t = Instant::now();
        assert_eq!(fifth.read(&mut b).unwrap_or(0), 0);
        assert!(t.elapsed() < Duration::from_secs(4), "closed at once");
        drop(idle);
        r.wake.send(libc::SIGTERM);
        assert_eq!(r.done.join().unwrap(), Ok(()));
    }

    #[test]
    fn switching_off_ends_the_daemon_and_frees_the_port() {
        let (d, s) = (TestDir::new("deskflow-run", "off"), TestDir::sockets("dfo"));
        let env = test_env(&d, &s);
        let (gen_path, socket) = (env.generated.clone(), env.socket.clone());
        write_cfg(&gen_path, true, 24800, &[]);
        let r = start(env);
        write_cfg(&gen_path, false, 24800, &[]);
        cmd::send(&socket, &Cmd::Reload).unwrap();
        assert_eq!(r.done.join().unwrap(), Ok(()));
        TcpListener::bind(("127.0.0.1", r.port)).unwrap();
    }

    #[test]
    fn a_second_daemon_stops_before_touching_anything() {
        let (d, s) = (TestDir::new("deskflow-run", "twice"), TestDir::sockets("dft"));
        let env = test_env(&d, &s);
        let _held = lock(&env.state).unwrap();
        write_cfg(&env.generated, true, 24800, &[]);
        let state = env.state.clone();
        assert_eq!(run(env), Err("another deskflow daemon runs".to_string()));
        assert!(!state.exists());
    }

    #[test]
    fn no_generated_file_means_off() {
        let (d, s) = (TestDir::new("deskflow-run", "absent"), TestDir::sockets("dfa"));
        let env = test_env(&d, &s);
        let state = env.state.clone();
        assert_eq!(run(env), Ok(()));
        assert!(!state.exists());
    }

    fn error_head(state: &Path) -> String {
        match statefile::parse(&std::fs::read_to_string(state).unwrap()).head {
            Head::Error(e) => e,
            h => panic!("{h:?}"),
        }
    }

    #[test]
    fn a_bad_config_is_an_error_head() {
        let (d, s) = (TestDir::new("deskflow-run", "bad"), TestDir::sockets("dfb"));
        let env = test_env(&d, &s);
        std::fs::create_dir_all(env.generated.parent().unwrap()).unwrap();
        std::fs::write(&env.generated, "[deskflow]\nlisten = 7\n").unwrap();
        let state = env.state.clone();
        let e = run(env).unwrap_err();
        assert!(e.starts_with("generated/deskflow:"), "{e}");
        assert_eq!(error_head(&state), e);

        let (d, s) = (TestDir::new("deskflow-run", "invalid"), TestDir::sockets("dfv"));
        let env2 = test_env(&d, &s);
        write_cfg(&env2.generated, true, 0, &[]);
        let state = env2.state.clone();
        let e = run(env2).unwrap_err();
        assert!(e.contains("port"), "{e}");
        assert_eq!(error_head(&state), e);
    }

    #[test]
    fn a_damaged_identity_is_kept_and_reported() {
        let (d, s) = (TestDir::new("deskflow-run", "ident"), TestDir::sockets("dfi"));
        let env = test_env(&d, &s);
        write_cfg(&env.generated, true, 24800, &[]);
        std::fs::create_dir_all(env.identity.parent().unwrap()).unwrap();
        std::fs::write(&env.identity, "junk\n").unwrap();
        let (state, identity) = (env.state.clone(), env.identity.clone());
        let e = run(env).unwrap_err();
        assert!(e.starts_with("identity: "), "{e}");
        assert_eq!(error_head(&state), e);
        assert_eq!(std::fs::read_to_string(identity).unwrap(), "junk\n");
    }

    #[test]
    fn a_port_in_use_is_an_error_head() {
        let (d, s) = (TestDir::new("deskflow-run", "port"), TestDir::sockets("dfu"));
        let env = test_env(&d, &s);
        let taken = TcpListener::bind("0.0.0.0:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        write_cfg(&env.generated, true, port, &[]);
        let state = env.state.clone();
        assert_eq!(run(env), Err(format!("port {port} in use")));
        assert_eq!(error_head(&state), format!("port {port} in use"));
    }
}
