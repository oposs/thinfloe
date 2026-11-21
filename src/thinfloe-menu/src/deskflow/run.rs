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

use super::capture::Capture;
use super::cmd::{self, Cmd};
use super::conn::{Conn, Event};
use super::ident::{self, Identity};
use super::pair::{self, Pending, PendingKind, Verdict};
use super::statefile::{self, DevLine, DevState, Head, StateFile};
use super::tls;
use crate::config::DeskflowCfg;

/// The paths and the host name the daemon works with.
pub struct RunEnv {
    /// `generated/deskflow`, the daemon's only configuration.
    pub generated: PathBuf,
    pub identity: PathBuf,
    pub state: PathBuf,
    pub socket: PathBuf,
    pub hostname: String,
}

/// The longest poll; the watchdog's heartbeat is stored once per turn.
const TURN: Duration = Duration::from_millis(250);
/// The watchdog ends the process when the loop has not turned for this long.
const STALL: Duration = Duration::from_secs(2);
/// The watchdog's exit status (EX_SOFTWARE).
const STALL_EXIT: i32 = 70;
/// At most this many connections may be in TLS, Hello or the verdict.
const MAX_HANDSHAKES: usize = 4;
/// The largest generated/deskflow read.
const CONFIG_MAX: u64 = 256 * 1024;

pub fn run(env: RunEnv) -> Result<(), String> {
    let _lock = lock(&env.state)?;
    let cfg = read_config(&env.generated).map_err(|e| fail(&env.state, e))?;
    if !cfg.listen {
        return Ok(());
    }
    let id =
        ident::load_or_create(&env.identity, &env.hostname).map_err(|e| fail(&env.state, format!("identity: {e}")))?;
    let listener = listen(cfg.port).map_err(|e| fail(&env.state, e))?;
    let sock = cmd::bind(&env.socket).map_err(|e| fail(&env.state, format!("command socket: {e}")))?;
    let wake = Wake::new().map_err(|e| fail(&env.state, e))?;
    wake.install_signals();
    let capture = Capture::connect().map_err(|e| fail(&env.state, e))?;
    let beat = Watchdog::start();
    let mut d = Daemon::new(env, cfg, &id, listener, sock, Some(capture));
    d.serve(&wake, Some(&beat))
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
fn read_config(path: &Path) -> Result<DeskflowCfg, String> {
    let Some(bytes) = crate::atomic::read(path, CONFIG_MAX).map_err(|e| format!("generated/deskflow: {e}"))? else {
        return Ok(DeskflowCfg::default());
    };
    let text = String::from_utf8(bytes).map_err(|_| "generated/deskflow: not UTF-8".to_string())?;
    let cfg = crate::deskflowcfg::parse(&text)?;
    crate::deskflowcfg::validate(&cfg)?;
    Ok(cfg)
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

struct Daemon {
    env: RunEnv,
    cfg: DeskflowCfg,
    tls: Arc<ServerConfig>,
    fingerprint: String,
    listener: TcpListener,
    sock: UnixDatagram,
    capture: Option<Capture>,
    conns: Vec<Conn>,
    pending: Pending,
    /// The last DINF size of each (name, fingerprint).
    screens: HashMap<(String, String), (u32, u32)>,
    /// Each line's state and when it took it.
    since: HashMap<(String, String), (DevState, i64)>,
    /// The text last written to the state file.
    written: Option<String>,
}

impl Daemon {
    fn new(
        env: RunEnv,
        cfg: DeskflowCfg,
        id: &Identity,
        listener: TcpListener,
        sock: UnixDatagram,
        capture: Option<Capture>,
    ) -> Daemon {
        Daemon {
            env,
            cfg,
            tls: tls::server_config(id),
            fingerprint: ident::fingerprint(&id.cert_der),
            listener,
            sock,
            capture,
            conns: Vec::new(),
            pending: Pending::default(),
            screens: HashMap::new(),
            since: HashMap::new(),
            written: None,
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
        // Every connection ends with the daemon; a failure has written its
        // head already.
        self.conns.clear();
        if result.is_ok() {
            self.write_state();
        }
        result
    }

    fn turn(&mut self, wake: &Wake) -> Next {
        if let Some(c) = self.capture.as_mut()
            && c.flush().is_err()
        {
            return Next::Exit(Ok(()));
        }
        const IN: libc::c_short = libc::POLLIN;
        let mut fds = vec![
            pollfd(wake.rd.as_raw_fd(), IN),
            pollfd(self.sock.as_raw_fd(), IN),
            pollfd(self.listener.as_raw_fd(), IN),
            pollfd(self.capture.as_ref().map_or(-1, |c| c.fd().as_raw_fd()), IN),
        ];
        let fixed = fds.len();
        for c in &self.conns {
            let ev = if c.wants_write() { IN | libc::POLLOUT } else { IN };
            fds.push(pollfd(c.fd(), ev));
        }
        // SAFETY: fds is a valid array of its length for the call.
        let r = unsafe {
            libc::poll(
                fds.as_mut_ptr(),
                fds.len() as libc::nfds_t,
                TURN.as_millis() as libc::c_int,
            )
        };
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == ErrorKind::Interrupted {
                return Next::Go;
            }
            return Next::Exit(Err(fail(&self.env.state, format!("poll: {e}"))));
        }
        let ready = |i: usize| fds[i].revents != 0;
        if ready(0) {
            for sig in wake.take() {
                let next = match sig {
                    libc::SIGTERM => Next::Exit(Ok(())),
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
        if ready(3)
            && let Some(c) = self.capture.as_mut()
        {
            // Wayland gone: labwc has ended, and the session with it.
            if fds[3].revents & (libc::POLLERR | libc::POLLHUP) != 0 || c.dispatch().is_err() {
                return Next::Exit(Ok(()));
            }
            // The edge strips come with the forwarding (Task 10).
        }
        let now = Instant::now();
        let known = self.conns.len();
        for i in 0..known {
            let revents = fds[fixed + i].revents;
            if revents == 0 {
                continue;
            }
            let events = self.conns[i].on_readable(now);
            self.on_events(i, events);
        }
        if ready(2) {
            self.accept(now);
        }
        for c in &mut self.conns {
            c.tick(now);
            c.flush();
        }
        self.conns.retain(|c| c.closing().is_none());
        self.write_state();
        Next::Go
    }

    fn accept(&mut self, now: Instant) {
        loop {
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
            match Conn::new(stream, self.tls.clone(), now) {
                Ok(c) => self.conns.push(c),
                Err(e) => eprintln!("thinfloe-menu --deskflow: {e}"),
            }
        }
    }

    fn on_events(&mut self, i: usize, events: Vec<Event>) {
        for ev in events {
            match ev {
                Event::Named(name) => self.judge(i, &name),
                Event::Screen { w, h } => {
                    if let Some(fp) = self.conns[i].fingerprint() {
                        let key = (name_of(&self.conns[i]), fp.to_string());
                        self.screens.insert(key, (w.max(0) as u32, h.max(0) as u32));
                    }
                }
            }
        }
    }

    /// The verdict on a named connection, given in the same pass.
    fn judge(&mut self, i: usize, name: &str) {
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
                    self.conns[i].refuse_busy(Instant::now());
                    return;
                }
            }
            Verdict::Pending => self.pending.note(name, &fp, PendingKind::New, epoch_now() as u64),
            Verdict::FingerprintChanged => {
                self.pending
                    .note(name, &fp, PendingKind::FingerprintChanged, epoch_now() as u64)
            }
        }
        self.conns[i].set_verdict(v, Instant::now());
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
        let cfg = match read_config(&self.env.generated) {
            Ok(c) => c,
            Err(e) => return Next::Exit(Err(fail(&self.env.state, e))),
        };
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
        self.cfg = cfg;
        Next::Go
    }

    /// The state file as it is now.
    fn state(&mut self) -> StateFile {
        let now = epoch_now();
        let mut lines: Vec<(String, String, DevState, Vec<String>)> = Vec::new();
        for d in &self.cfg.devices {
            let on = self.conns.iter().any(|c| {
                c.trusted()
                    && c.screen().is_some()
                    && c.name() == Some(&d.name)
                    && c.fingerprint() == Some(&d.fingerprint)
            });
            let state = if on { DevState::Connected } else { DevState::Offline };
            lines.push((d.name.clone(), d.fingerprint.clone(), state, Vec::new()));
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
        }
    }

    fn write_cfg(path: &Path, listen: bool, port: u16, devices: &[(&str, &str)]) {
        let mut t = format!("[deskflow]\nlisten = {listen}\nport = {port}\n");
        for (i, (name, fp)) in devices.iter().enumerate() {
            t.push_str(&format!(
                "\n[[deskflow.device]]\nname = \"{name}\"\nfingerprint = \"{fp}\"\noutput = \"DP-{i}\"\nside = \"right-of\"\n"
            ));
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
            [Msg::Ciak, Msg::Crop, Msg::Dsop(vec![])]
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

        // SIGTERM: exit 0, and the device is offline.
        r.wake.send(libc::SIGTERM);
        assert_eq!(r.done.join().unwrap(), Ok(()));
        let st = statefile::parse(&std::fs::read_to_string(&state).unwrap());
        assert_eq!(line(&st, "laptop", &fp1).unwrap().state, DevState::Offline);
        drop(c);
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
