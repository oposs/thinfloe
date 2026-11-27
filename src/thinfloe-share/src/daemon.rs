//! `thinfloe-share` with no arguments: the FloeDrop daemon, run as tfuser by the menu.
//!
//! It starts the root helper (`sudo -n thinfloe-share --radio`) with a pipe on its
//! stdin and reads its one status line. On `up` it receives on the lease's
//! `awdl0` (port 8771, files staged in `.incoming/` and moved to
//! `inbox/<sender>/`), announces itself over mDNS and, unless the helper says
//! `no-ble`, browses for receivers and sends what lands in `outbox/<device>/`.
//! The state file `$XDG_RUNTIME_DIR/thinfloe-share.state` tells the menu how it
//! goes. Closing the helper's stdin gives the radio back.

use std::collections::HashMap;
use std::io;
use std::net::{Ipv6Addr, SocketAddrV6};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc as smpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask, Watches};
use owly_airdrop::driver::mdns::{Mdns, Registration};
use owly_airdrop::driver::receiver::{Options, Receiver, ReceiverEvent};
use owly_airdrop::driver::sender::{Payload, Sender};
use owly_airdrop::driver::tls::Identity;
use owly_airdrop::proto::mdns::{self, BrowseEvent, RECEIVER_FLAGS, SUPPORTS_DVZIP, Service};
use owly_airdrop::proto::messages::Offer;
use owly_airdrop::proto::receiver::Config;
use owly_airdrop::proto::sender::{Config as SenderConfig, Outcome as OwlyOutcome};
use owlyd::api::{self, LinkInfo, RadioProxy};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{mpsc, watch};

use crate::incoming::{self, Incoming, ask_total};
use crate::outbox::{Outbox, Outcome, SendHandle, Transport};
use crate::state::{Device, Head};
use crate::{config, inbox, osd, state};

/// Where we receive (Owly's NET-4 port).
const RECEIVE_PORT: u16 = 8771;
/// Apple receivers listen here; an AWDL peer is tried here (DISC-5).
const AIRDROP_PORT: u16 = 8770;
const DEFAULT_ROOT: &str = "/media/tfuser/FloeDrop";
const DEFAULT_RADIO: &str = "sudo -n /usr/bin/thinfloe-share --radio";
/// How long the helper gets to give the radio back after its stdin closes.
const HELPER_GRACE: Duration = Duration::from_secs(5);
/// A receiver that answered is asked again this often, so it stays in range
/// (the outbox drops a device not seen for 60 s).
const REDISCOVER: Duration = Duration::from_secs(20);
/// A receiver that did not answer yet is tried again this often.
const RETRY: Duration = Duration::from_secs(2);
/// The longest one `/Discover` may hold up the others.
const DISCOVER_WAIT: Duration = Duration::from_secs(3);
/// The longest the owlyd lookup at radio start may take (zbus waits 25 s).
const OWLYD_WAIT: Duration = Duration::from_secs(5);
/// The outbox thread looks at its folders at least this often.
const OUTBOX_MAX_SLEEP: Duration = Duration::from_secs(5);
/// An accepted offer with no event this long after the accept is looked at: Owly ends a
/// connection that brings no `/Upload` within its idle time (60 s) without an event.
const UPLOAD_START_WAIT: Duration = Duration::from_secs(75);
/// While an upload is under way the entry is looked at again this often.
const UPLOAD_RECHECK: Duration = Duration::from_secs(15);
/// An upload is under way when something in `.incoming/` changed this recently (Owly
/// fails an upload that brings less than 32 KiB in 30 s).
const UPLOAD_QUIET: Duration = Duration::from_secs(60);
/// How long the mDNS goodbye gets to go out before the radio is given back.
const GOODBYE_WAIT: Duration = Duration::from_millis(200);
/// State text when the helper ends without saying why.
const HELPER_GONE: &str = "The radio helper stopped without a reason.";
/// State text after SIGTERM: the menu stopped FloeDrop.
const STOPPED: &str = "FloeDrop is stopped.";

fn log(msg: impl std::fmt::Display) {
    eprintln!("thinfloe-share: {msg}");
}

/// A line of the helper's stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperLine {
    Up {
        iface: String,
        mode: String,
        channel: u32,
        flags: Vec<String>,
        /// The Wi-Fi card under the link and its driver; empty from a helper that does not say.
        station: String,
        driver: String,
    },
    Error(String),
}

/// `up<TAB>iface<TAB>mode<TAB>channel<TAB>flags<TAB>station<TAB>driver` or
/// `error<TAB>text`; anything else is None. The last two fields of an up line may be missing.
pub fn parse_helper_line(line: &str) -> Option<HelperLine> {
    let f: Vec<&str> = line.trim_end_matches(['\r', '\n']).split('\t').collect();
    match f[0] {
        "up" if f.len() >= 4 && !f[1].is_empty() => Some(HelperLine::Up {
            iface: f[1].to_string(),
            mode: f[2].to_string(),
            channel: f[3].parse().unwrap_or(0),
            flags: f
                .get(4)
                .map(|s| {
                    s.split(',')
                        .filter(|x| !x.is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
            station: f.get(5).copied().unwrap_or("").to_string(),
            driver: f.get(6).copied().unwrap_or("").to_string(),
        }),
        "error" => {
            let text = f[1..].join(" ");
            let text = text.trim();
            Some(HelperLine::Error(if text.is_empty() {
                HELPER_GONE.to_string()
            } else {
                text.to_string()
            }))
        }
        _ => None,
    }
}

/// The card under the link with its driver ("wlx1 (rtw88_8822bu)"), the card alone when the
/// driver is unknown, the link when the helper named no card.
pub fn card_label(iface: &str, station: &str, driver: &str) -> String {
    match (station, driver) {
        ("", _) => iface.to_string(),
        (s, "") => s.to_string(),
        (s, d) => format!("{s} ({d})"),
    }
}

/// The error when the receiver cannot start on the radio the helper gave.
pub fn start_failed_text(iface: &str, station: &str, driver: &str, why: &str) -> String {
    format!(
        "FloeDrop cannot start on {}: {why}",
        card_label(iface, station, driver)
    )
}

/// The log's ready text: the card under the link and its driver when the helper named them,
/// else the link.
pub fn ready_text(iface: &str, station: &str, driver: &str, mode: &str, channel: u32) -> String {
    let card = if station.is_empty() { iface } else { station };
    let mut how = String::new();
    if !driver.is_empty() {
        how.push_str(driver);
        how.push_str(", ");
    }
    how.push_str(mode);
    // 0: the helper could not read the channel.
    if channel != 0 {
        how.push_str(&format!(", channel {channel}"));
    }
    format!("ready on {card} ({how})")
}

/// What to say to an `/Ask`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Accept,
    /// Declined, with the note to show, if any.
    Decline(Option<(osd::Tone, String)>),
}

/// Files are taken iff they fit in `free` bytes; links are never taken.
pub fn answer(offer: &Offer, free: u64, sender: &str) -> Answer {
    match offer {
        Offer::Urls(_) => Answer::Decline(None),
        Offer::Files(_) if inbox::decide(ask_total(offer), free) => Answer::Accept,
        Offer::Files(_) => Answer::Decline(Some((osd::Tone::Warn, inbox::refused_note(sender)))),
    }
}

/// The IPv6 link-local address (EUI-64) of a MAC address such as `aa:bb:cc:dd:ee:ff`.
pub fn link_local(mac: &str) -> Option<Ipv6Addr> {
    let b: Vec<u8> = mac
        .split(':')
        .map(|x| u8::from_str_radix(x, 16))
        .collect::<Result<_, _>>()
        .ok()?;
    let [a, b1, c, d, e, f] = <[u8; 6]>::try_from(b).ok()?;
    Some(Ipv6Addr::new(
        0xfe80,
        0,
        0,
        0,
        u16::from_be_bytes([a ^ 0x02, b1]),
        u16::from_be_bytes([c, 0xff]),
        u16::from_be_bytes([0xfe, d]),
        u16::from_be_bytes([e, f]),
    ))
}

/// The link-local address and index of `iface` from `/proc/net/if_inet6`'s text.
pub fn if_inet6_link_local(text: &str, iface: &str) -> Option<(Ipv6Addr, u32)> {
    text.lines().find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 6 || f[5] != iface || f[3] != "20" || f[0].len() != 32 {
            return None;
        }
        let n = u128::from_str_radix(f[0], 16).ok()?;
        let index = u32::from_str_radix(f[1], 16).ok()?;
        Some((Ipv6Addr::from(n), index))
    })
}

/// Unblock SIGHUP on this thread, where the runtime polls its handler.
fn unblock_hangup() {
    // SAFETY: a zeroed sigset_t that sigemptyset initialises; no other memory.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGHUP);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
    }
}

/// The name phones show: the configured one, or `default_name` of the
/// host name.
pub fn share_name(configured: &str, host: &str) -> String {
    let c = configured.trim();
    if c.is_empty() {
        default_name(host)
    } else {
        c.to_string()
    }
}

/// `FloeDrop <id>`: the host name without its `thinfloe-` prefix, or the
/// whole host name when it has none. A space, not a dash: the phone wraps
/// the name there.
pub fn default_name(host: &str) -> String {
    let id = host
        .strip_prefix("thinfloe-")
        .filter(|s| !s.is_empty())
        .unwrap_or(host);
    format!("FloeDrop {id}")
}

fn host_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: buf is valid for buf.len() bytes; gethostname NUL-terminates within it on success.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return "ThinFloe".into();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let h = String::from_utf8_lossy(&buf[..end]).into_owned();
    if h.is_empty() { "ThinFloe".into() } else { h }
}

fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("TF_SHARE_CONFIG") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    Path::new(&home).join(".config/thinfloe/generated/share")
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    // SAFETY: getrandom writes at most b.len() bytes into the buffer.
    let n = unsafe { libc::getrandom(b.as_mut_ptr().cast(), b.len(), 0) };
    if n != b.len() as isize {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            ^ u128::from(std::process::id());
        for (i, x) in b.iter_mut().enumerate() {
            *x = (t >> ((i % 16) * 8)) as u8;
        }
    }
    b
}

/// Seconds since the epoch.
fn epoch_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The FloeDrop folder, the tmpfs.
fn share_root() -> PathBuf {
    std::env::var_os("TF_SHARE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ROOT))
}

/// True when `path` lies on a tmpfs.
pub fn is_tmpfs(path: &Path) -> std::io::Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` a valid out-pointer.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // f_type's type differs between architectures.
    #[allow(clippy::unnecessary_cast)]
    Ok(st.f_type as i64 == libc::TMPFS_MAGIC as i64)
}

/// The real folder must be the tmpfs; a test root below target/ need not be, unless the
/// test asks with `TF_SHARE_REQUIRE_TMPFS=1`.
fn root_must_be_tmpfs() -> bool {
    std::env::var_os("TF_SHARE_ROOT").is_none()
        || std::env::var_os("TF_SHARE_REQUIRE_TMPFS").is_some_and(|v| v == "1")
}

/// The error when FloeDrop's folder is not its tmpfs.
pub fn not_mounted(root: &Path) -> String {
    format!("The FloeDrop folder {} is not mounted.", root.display())
}

/// The ready head's flags: `no-peers` added when owlyd's link was not used.
fn ready_flags(mut flags: Vec<String>, peers: bool) -> Vec<String> {
    if !peers {
        flags.push("no-peers".to_string());
    }
    flags
}

/// The send spool below `root`: a dot folder, so neither the inbox nor the outbox
/// (both one level lower) ever lists it.
fn spool_dir(root: &Path) -> PathBuf {
    root.join(".spool")
}

/// The folders the daemon uses.
#[derive(Debug, Clone)]
struct Paths {
    root: PathBuf,
    inbox: PathBuf,
    outbox: PathBuf,
    /// Owly's destination: finished uploads land here before they are placed.
    incoming: PathBuf,
    /// `TMPDIR` of the daemon: Owly spools the archive of a send here, inside the
    /// tmpfs and its cap.
    spool: PathBuf,
    identity: PathBuf,
}

impl Paths {
    fn new(runtime_dir: &Path) -> Paths {
        let root = share_root();
        Paths {
            inbox: root.join("inbox"),
            outbox: root.join("outbox"),
            incoming: root.join(".incoming"),
            spool: spool_dir(&root),
            identity: runtime_dir.join("thinfloe-share"),
            root,
        }
    }

    /// `inbox/` and an empty `.incoming/` (what a killed run left there goes).
    fn prepare(&self) {
        if let Err(e) = std::fs::create_dir_all(&self.inbox) {
            log(format!("{}: {e}", self.inbox.display()));
        }
        if let Err(e) = inbox::discard(&self.incoming) {
            log(format!("{}: {e}", self.incoming.display()));
        }
        if let Err(e) = std::fs::create_dir_all(&self.incoming) {
            log(format!("{}: {e}", self.incoming.display()));
        }
        if let Err(e) = inbox::discard(&self.spool) {
            log(format!("{}: {e}", self.spool.display()));
        }
        if let Err(e) = std::fs::DirBuilder::new().mode(0o700).create(&self.spool) {
            log(format!("{}: {e}", self.spool.display()));
        }
    }
}

struct StateFile {
    path: PathBuf,
    head: Head,
    devices: Vec<Device>,
    incoming: Vec<Incoming>,
}

impl StateFile {
    fn write(&self) {
        if let Err(e) = state::write(&self.path, &self.head, &self.devices, &self.incoming) {
            log(format!("{}: {e}", self.path.display()));
        }
    }
}

/// The menu's pid, set by the menu when it starts the daemon.
pub const PARENT_ENV: &str = "TF_SHARE_PARENT";

/// The pid the daemon's parent must have: the menu's from `PARENT_ENV`, else the
/// parent at start.
fn expected_parent(env: Option<&str>, at_start: libc::pid_t) -> libc::pid_t {
    env.and_then(|p| p.trim().parse::<libc::pid_t>().ok())
        .filter(|&p| p > 0)
        .unwrap_or(at_start)
}

/// True when the daemon has lost its parent: re-parented to init (or a subreaper),
/// or never the child of the menu that named itself.
fn parent_gone(expected: libc::pid_t, now: libc::pid_t) -> bool {
    now == 1 || now != expected
}

/// `thinfloe-share` with no arguments; the exit code.
pub fn main() -> i32 {
    let cfg_path = config_path();
    let cfg = config::load(&cfg_path);
    if !cfg.enabled {
        return 0;
    }
    // The menu starts the daemon; it must not outlive it. A parent that died before
    // the prctl sends nothing, so the parent is checked after it: the menu's pid from
    // PARENT_ENV, else the parent at start.
    // SAFETY: getppid and prctl take no pointers and have no memory effects.
    let at_start = unsafe { libc::getppid() };
    let parent = expected_parent(std::env::var(PARENT_ENV).ok().as_deref(), at_start);
    unsafe {
        libc::prctl(
            libc::PR_SET_PDEATHSIG,
            libc::SIGTERM as libc::c_ulong,
            0,
            0,
            0,
        )
    };
    if parent_gone(parent, unsafe { libc::getppid() }) {
        log("the menu that started it is gone");
        return 0;
    }
    let Some(state_path) = state::path() else {
        log("XDG_RUNTIME_DIR is not set");
        return 1;
    };
    // Owly spools a send's archive with tempfile::tempfile(), in TMPDIR: in /tmp that is
    // RAM outside the FloeDrop cap, a second copy of the batch. In the tmpfs a batch that
    // does not fit twice fails like any failed send.
    // SAFETY: no other thread runs yet; the tokio runtime is built below.
    unsafe { std::env::set_var("TMPDIR", spool_dir(&share_root())) };
    let runtime_dir = state_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log(e);
            return 1;
        }
    };
    rt.block_on(run(cfg_path, cfg.name, state_path, runtime_dir))
}

enum Exit {
    Term,
    Error(String),
}

fn helper_command() -> Result<tokio::process::Command, String> {
    let line = std::env::var("TF_SHARE_RADIO").unwrap_or_else(|_| DEFAULT_RADIO.to_string());
    let mut words = line.split_whitespace();
    let program = words.next().ok_or("TF_SHARE_RADIO is empty")?;
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(words)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());
    Ok(cmd)
}

async fn run(
    cfg_path: PathBuf,
    configured: String,
    state_path: PathBuf,
    runtime_dir: PathBuf,
) -> i32 {
    let mut st = StateFile {
        path: state_path,
        head: Head::Starting,
        devices: Vec::new(),
        incoming: Vec::new(),
    };
    st.write();
    let paths = Paths::new(&runtime_dir);
    // Without its tmpfs FloeDrop would write into the root file system, which is RAM
    // without the cap; nothing below the folder is made before this check.
    if root_must_be_tmpfs() && !matches!(is_tmpfs(&paths.root), Ok(true)) {
        let text = not_mounted(&paths.root);
        log(&text);
        st.head = Head::Error(text);
        st.write();
        return 1;
    }
    paths.prepare();
    let mut name = share_name(&configured, &host_name());

    let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
        signal(SignalKind::hangup()),
    ) else {
        st.head = Head::Error("Cannot install signal handlers.".into());
        st.write();
        return 1;
    };
    // The menu starts the daemon with SIGHUP blocked, so a reload before the
    // handler above was set waits instead of ending it; it arrives now.
    unblock_hangup();

    let mut child = match helper_command().and_then(|mut c| c.spawn().map_err(|e| e.to_string())) {
        Ok(c) => c,
        Err(e) => {
            log(format!("cannot start the radio helper: {e}"));
            st.head = Head::Error(format!("Cannot start the radio helper: {e}"));
            st.write();
            return 1;
        }
    };
    let mut stdin = child.stdin.take();
    let mut lines = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
    let mut airdrop: Option<Airdrop> = None;

    let exit = 'main: loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(l)) => match parse_helper_line(&l) {
                    Some(HelperLine::Up { iface, mode, channel, flags, station, driver }) if airdrop.is_none() => {
                        let no_ble = flags.iter().any(|f| f == "no-ble");
                        // Signals and the helper's lines still count while the radio starts.
                        let started = {
                            let start = Airdrop::start(&iface, &name, no_ble, &paths);
                            tokio::pin!(start);
                            loop {
                                tokio::select! {
                                    r = &mut start => break r,
                                    _ = term.recv() => break 'main Exit::Term,
                                    _ = int.recv() => break 'main Exit::Term,
                                    line = lines.next_line() => match line {
                                        Ok(Some(l)) => match parse_helper_line(&l) {
                                            Some(HelperLine::Error(text)) => break 'main Exit::Error(text),
                                            _ => log(format!("the helper said {l:?}, ignored")),
                                        },
                                        Ok(None) | Err(_) => break 'main Exit::Error(HELPER_GONE.into()),
                                    },
                                }
                            }
                        };
                        match started {
                            Ok(a) => {
                                log(format!("{} as {name:?}", ready_text(&iface, &station, &driver, &mode, channel)));
                                let flags = ready_flags(flags, a.peers);
                                airdrop = Some(a);
                                st.head = Head::Ready { iface, mode, channel, flags, station, driver };
                                st.write();
                            }
                            Err(e) => break 'main Exit::Error(start_failed_text(&iface, &station, &driver, &e)),
                        }
                    }
                    Some(HelperLine::Up { .. }) => log("a second up line from the helper, ignored"),
                    Some(HelperLine::Error(text)) => break Exit::Error(text),
                    None => log(format!("the helper said {l:?}, ignored")),
                },
                Ok(None) | Err(_) => break Exit::Error(HELPER_GONE.into()),
            },
            _ = term.recv() => break Exit::Term,
            _ = int.recv() => break Exit::Term,
            _ = hup.recv() => {
                let cfg = config::load(&cfg_path);
                if !cfg.enabled {
                    break Exit::Term;
                }
                let new = share_name(&cfg.name, &host_name());
                if new != name {
                    log(format!("the name is now {new:?}"));
                    name = new;
                    if let Some(a) = airdrop.as_mut()
                        && let Err(e) = a.rename(&name).await
                    {
                        break Exit::Error(format!("FloeDrop cannot take the new name: {e}"));
                    }
                }
            }
            changed = next_step(&mut airdrop) => {
                if changed
                    && let Some(a) = &airdrop
                {
                    let (devices, incoming) = a.lists();
                    if devices != st.devices || incoming != st.incoming {
                        st.devices = devices;
                        st.incoming = incoming;
                        st.write();
                    }
                }
            }
        }
    };

    let code = match &exit {
        Exit::Term => 0,
        Exit::Error(text) => {
            log(text);
            1
        }
    };
    st.head = Head::Error(match exit {
        Exit::Term => STOPPED.to_string(),
        Exit::Error(t) => t,
    });
    st.devices.clear();
    st.incoming.clear();
    st.write();
    // The goodbye first, while the link is still up: sent after the radio went, it never
    // reaches the phones, which then list the next run's receiver beside the old one.
    if let Some(a) = airdrop.take() {
        a.shutdown().await;
    }
    // EOF on its stdin: the helper releases the lease and exits.
    drop(stdin.take());
    match tokio::time::timeout(HELPER_GRACE, child.wait()).await {
        Ok(_) => {}
        Err(_) => {
            log("the radio helper did not exit; stopping it");
            let _ = child.start_kill();
        }
    }
    code
}

/// An accepted request until its end.
#[derive(Debug, Clone)]
struct Pending {
    /// What the state file and the log show.
    shown: Incoming,
    /// The sender's name as the phone gave it: the inbox folder is made from it.
    sender: String,
    /// When to look whether it still runs.
    check_at: tokio::time::Instant,
}

/// The transfers under way, oldest first, then by sender.
fn incoming_list<'a>(pending: impl Iterator<Item = &'a Incoming>) -> Vec<Incoming> {
    let mut v: Vec<Incoming> = pending.cloned().collect();
    v.sort_by(|a, b| (a.since, &a.sender).cmp(&(b.since, &b.sender)));
    v
}

async fn next_step(a: &mut Option<Airdrop>) -> bool {
    match a {
        Some(a) => a.step().await,
        None => std::future::pending().await,
    }
}

/// The link the daemon works on.
#[derive(Debug, Clone, Copy)]
struct Iface {
    index: u32,
    addr: Ipv6Addr,
}

/// owlyd's view of the lease, read as tfuser (Radio1 and Properties are open to every user).
async fn owlyd_link(iface: &str) -> Result<(RadioProxy<'static>, LinkInfo), String> {
    let conn = api::connect(None)
        .await
        .map_err(|e| format!("owlyd cannot be reached ({e})"))?;
    let radio = api::radio(&conn).await.map_err(|e| e.to_string())?;
    let link = radio.link().await.map_err(|e| e.to_string())?;
    let link = LinkInfo::from_dict(&link).ok_or("the radio is not up")?;
    if link.interface != iface {
        return Err(format!(
            "the radio is up on {}, not {iface}",
            link.interface
        ));
    }
    Ok((radio, link))
}

/// What the main loop is told by the outbox thread.
enum FromOutbox {
    /// An offer to send: the target is taken in the outbox thread when the offer starts,
    /// so a re-key that comes before the main loop reads it cannot lose it.
    Send {
        handle: SendHandle,
        target: Option<Target>,
        paths: Vec<PathBuf>,
    },
    Devices(Vec<Device>),
}

/// What the outbox thread is told.
enum Cmd {
    Seen {
        key: String,
        addr: String,
        name: String,
        stable: Option<String>,
        target: Target,
    },
    Changed(String),
    Finished(SendHandle, Outcome),
    Shutdown,
}

/// A receiver that answered `/Discover`: where to send, its flags, and whether they came
/// from its TXT record (mDNS) or are the guess for a radio peer (UP-1b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    addr: SocketAddrV6,
    flags: u32,
    txt: bool,
}

/// The send targets by service ID. Only the outbox thread has them; each offer carries
/// its target to the main loop.
#[derive(Default)]
struct Targets(HashMap<String, Target>);

impl Targets {
    /// `t` for the device that answers to `key` now. Guessed flags never replace flags
    /// from a TXT record: a phone found by mDNS and by the radio keeps its mDNS flags
    /// under its `awdl-` key, and only the address follows the newest answer.
    fn record(&mut self, key: &str, t: Target) {
        match self.0.get_mut(key) {
            Some(old) if old.txt && !t.txt => old.addr = t.addr,
            _ => {
                self.0.insert(key.to_string(), t);
            }
        }
    }

    /// Forgets the service IDs in `dropped` that no device answers to (`known`).
    fn forget(&mut self, dropped: &[String], known: impl Fn(&str) -> bool) {
        for id in dropped {
            if !known(id) {
                self.0.remove(id);
            }
        }
    }

    fn get(&self, key: &str) -> Option<Target> {
        self.0.get(key).copied()
    }
}

/// What a receiver's record says across addresses: 16 hex digits of a hash of it, or
/// `None` without one.
pub fn stable_id(record: Option<&[u8]>) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let r = record.filter(|r| !r.is_empty())?;
    let mut h = std::hash::DefaultHasher::new();
    r.hash(&mut h);
    Some(format!("{:016x}", h.finish()))
}

type WatchMap = Arc<Mutex<HashMap<WatchDescriptor, String>>>;

struct ChanTransport {
    next: u64,
    out: mpsc::UnboundedSender<FromOutbox>,
    targets: Targets,
}

impl Transport for ChanTransport {
    fn send(&mut self, service_id: &str, paths: Vec<PathBuf>) -> SendHandle {
        self.next += 1;
        let handle = SendHandle(self.next);
        let _ = self.out.send(FromOutbox::Send {
            handle,
            target: self.targets.get(service_id),
            paths,
        });
        handle
    }
}

fn watch_mask() -> WatchMask {
    WatchMask::CLOSE_WRITE
        | WatchMask::MOVED_TO
        | WatchMask::CREATE
        | WatchMask::DELETE
        | WatchMask::MOVED_FROM
        | WatchMask::ONLYDIR
        | WatchMask::DONT_FOLLOW
}

/// The Outbox lives on a thread of its own: its write check scans /proc,
/// which must not stall the receiver.
fn outbox_thread(
    root: PathBuf,
    cmds: smpsc::Receiver<Cmd>,
    out: mpsc::UnboundedSender<FromOutbox>,
    mut watches: Watches,
    wmap: WatchMap,
) {
    let mut ob = Outbox::new(root.clone());
    let mut tr = ChanTransport {
        next: 0,
        out: out.clone(),
        targets: Targets::default(),
    };
    let mut last: Option<Vec<Device>> = None;
    let mut wake = Instant::now() + OUTBOX_MAX_SLEEP;
    loop {
        let wait = wake.saturating_duration_since(Instant::now());
        match cmds.recv_timeout(wait) {
            Ok(Cmd::Seen {
                key,
                addr,
                name,
                stable,
                target,
            }) => on_seen(
                &mut ob,
                &mut tr,
                &key,
                &addr,
                &name,
                stable.as_deref(),
                target,
            ),
            Ok(Cmd::Changed(folder)) => ob.changed(&folder, Instant::now()),
            Ok(Cmd::Finished(h, o)) => ob.finished(h, o, Instant::now()),
            Ok(Cmd::Shutdown) | Err(smpsc::RecvTimeoutError::Disconnected) => {
                ob.shutdown();
                return;
            }
            Err(smpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        let next = settle(&mut ob, &mut tr, now);
        wake = next.map_or(now + OUTBOX_MAX_SLEEP, |n| n.min(now + OUTBOX_MAX_SLEEP));
        for folder in ob.take_new_folders() {
            match watches.add(root.join(&folder), watch_mask()) {
                Ok(wd) => {
                    wmap.lock().unwrap().insert(wd, folder);
                }
                Err(e) => log(format!("cannot watch outbox/{folder}: {e}")),
            }
        }
        for (tone, text) in ob.take_notes() {
            osd::note(tone, &text);
        }
        let devices = ob.devices();
        if last.as_ref() != Some(&devices) {
            let _ = out.send(FromOutbox::Devices(devices.clone()));
            last = Some(devices);
        }
    }
}

/// A receiver answered: its device, and the target under the key the device answers to.
fn on_seen(
    ob: &mut Outbox,
    tr: &mut ChanTransport,
    key: &str,
    addr: &str,
    name: &str,
    stable: Option<&str>,
    target: Target,
) {
    if let Some(kept) = ob.seen(key, addr, name, stable, Instant::now()) {
        tr.targets.record(&kept, target);
    }
}

/// The due offers (each with its target), then the targets of dropped keys forgotten.
fn settle(ob: &mut Outbox, tr: &mut ChanTransport, now: Instant) -> Option<Instant> {
    let next = ob.tick(now, tr);
    let dropped = ob.take_dropped();
    tr.targets.forget(&dropped, |k| ob.knows(k));
    next
}

/// Finds receivers: mDNS on the link and `/Discover` at the radio's AWDL
/// peers, again every [`REDISCOVER`] for those that answered.
async fn discovery(
    iface: Iface,
    own_id: String,
    radio: Option<RadioProxy<'static>>,
    sender: watch::Receiver<Arc<Sender>>,
    cmds: smpsc::Sender<Cmd>,
) {
    struct Cand {
        key: String,
        port: u16,
        flags: u32,
        from_peers: bool,
        next_try: tokio::time::Instant,
    }
    let mut browse = match Mdns::new(iface.index).browse().await {
        Ok(b) => Some(b),
        Err(e) => {
            log(format!("not browsing mDNS: {e}"));
            None
        }
    };
    let mut cands: HashMap<Ipv6Addr, Cand> = HashMap::new();
    loop {
        let event = async {
            match browse.as_mut() {
                Some(b) => b.events.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            ev = event => match ev {
                Some(BrowseEvent::Found(f)) if f.instance != own_id => {
                    let c = cands.entry(f.addr).or_insert(Cand {
                        key: f.instance.clone(),
                        port: f.port,
                        flags: f.flags,
                        from_peers: false,
                        next_try: tokio::time::Instant::now(),
                    });
                    c.key = f.instance;
                    c.port = f.port;
                    c.flags = f.flags;
                    c.from_peers = false;
                    c.next_try = tokio::time::Instant::now();
                }
                Some(BrowseEvent::Lost(instance)) => cands.retain(|_, c| c.key != instance),
                Some(_) => {}
                None => browse = None,
            },
            () = tokio::time::sleep(RETRY) => {}
        }
        if let Some(radio) = &radio {
            let peers = radio.peers().await.unwrap_or_default();
            let addrs: Vec<(String, Ipv6Addr)> = peers
                .iter()
                .filter_map(|p| {
                    let mac = p.get("mac")?.downcast_ref::<String>().ok()?;
                    Some((mac.clone(), link_local(&mac)?))
                })
                .collect();
            cands.retain(|a, c| !c.from_peers || addrs.iter().any(|(_, x)| x == a));
            for (mac, addr) in addrs {
                cands.entry(addr).or_insert(Cand {
                    key: format!("awdl-{mac}"),
                    port: AIRDROP_PORT,
                    // UP-1b: no TXT to say, and Apple receivers take DVZip.
                    flags: RECEIVER_FLAGS | SUPPORTS_DVZIP,
                    from_peers: true,
                    next_try: tokio::time::Instant::now(),
                });
            }
        }
        let s = sender.borrow().clone();
        // Every due receiver is asked at once: one that does not answer costs
        // DISCOVER_WAIT once, not once per receiver behind it.
        let now = tokio::time::Instant::now();
        let due: Vec<(Ipv6Addr, SocketAddrV6)> = cands
            .iter()
            .filter(|(_, c)| c.next_try <= now)
            .map(|(addr, c)| (*addr, SocketAddrV6::new(*addr, c.port, 0, iface.index)))
            .collect();
        let answers = futures_util::future::join_all(due.into_iter().map(|(addr, sock)| {
            let s = &s;
            async move {
                (
                    addr,
                    sock,
                    tokio::time::timeout(DISCOVER_WAIT, s.discover(sock)).await,
                )
            }
        }))
        .await;
        for (addr, sock, answer) in answers {
            let Some(c) = cands.get_mut(&addr) else {
                continue;
            };
            match answer {
                Ok(Ok(Some(r))) => {
                    let _ = cmds.send(Cmd::Seen {
                        key: c.key.clone(),
                        addr: addr.to_string(),
                        name: r.computer_name,
                        stable: stable_id(r.record.as_deref()),
                        target: Target {
                            addr: sock,
                            flags: c.flags,
                            txt: !c.from_peers,
                        },
                    });
                    c.next_try = tokio::time::Instant::now() + REDISCOVER;
                }
                _ => c.next_try = tokio::time::Instant::now() + RETRY,
            }
        }
    }
}

/// The sending half: present unless the helper said `no-ble`.
struct Sending {
    sender: watch::Sender<Arc<Sender>>,
    cmds: smpsc::Sender<Cmd>,
    thread: Option<std::thread::JoinHandle<()>>,
    from_outbox: mpsc::UnboundedReceiver<FromOutbox>,
    events: inotify::EventStream<Vec<u8>>,
    wmap: WatchMap,
    discovery: tokio::task::JoinHandle<()>,
}

/// The AirDrop side once the radio is up.
struct Airdrop {
    iface: Iface,
    identity: Identity,
    paths: Paths,
    service_id: String,
    /// Bumped per receiver bind; request ids are per receiver.
    generation: u64,
    receiver: tokio::task::JoinHandle<()>,
    events_tx: mpsc::Sender<(u64, ReceiverEvent)>,
    events: mpsc::Receiver<(u64, ReceiverEvent)>,
    /// Accepted requests not ended yet, by (generation, request id).
    pending: HashMap<(u64, u64), Pending>,
    /// The receivers in range as the outbox last listed them.
    devices: Vec<Device>,
    _mdns: Registration,
    sending: Option<Sending>,
    /// Whether owlyd's link is used; without it there is no AWDL peer list, only mDNS.
    pub peers: bool,
}

enum Step {
    /// A pending transfer's check time came.
    Check,
    Receiver(Option<Box<(u64, ReceiverEvent)>>),
    Outbox(Option<FromOutbox>),
    Inotify(Option<io::Result<inotify::Event<std::ffi::OsString>>>),
}

impl Airdrop {
    async fn start(
        iface_name: &str,
        name: &str,
        no_ble: bool,
        paths: &Paths,
    ) -> Result<Airdrop, String> {
        let link = tokio::time::timeout(OWLYD_WAIT, owlyd_link(iface_name))
            .await
            .unwrap_or_else(|_| {
                Err(format!(
                    "owlyd did not answer within {} s",
                    OWLYD_WAIT.as_secs()
                ))
            });
        let (radio, iface, announce) = match link {
            Ok((radio, link)) => {
                let addr: Ipv6Addr = link
                    .address
                    .parse()
                    .map_err(|_| format!("owlyd gave the address {:?}", link.address))?;
                let announce = link.announce.map(|a| mdns::Burst {
                    count: a.count,
                    interval: a.interval,
                    pause: a.pause,
                });
                (
                    Some(radio),
                    Iface {
                        index: link.index,
                        addr,
                    },
                    announce,
                )
            }
            Err(e) => {
                log(format!(
                    "not using owlyd's link ({e}); reading {iface_name}'s own address"
                ));
                let text = std::fs::read_to_string("/proc/net/if_inet6").unwrap_or_default();
                let (addr, index) = if_inet6_link_local(&text, iface_name)
                    .ok_or_else(|| format!("{iface_name} has no IPv6 link-local address"))?;
                (None, Iface { index, addr }, None)
            }
        };
        let peers = radio.is_some();
        let identity = Identity::load_or_generate(&paths.identity).map_err(|e| e.to_string())?;
        let (events_tx, events) = mpsc::channel(64);
        let receiver = bind(iface, &identity, name, paths, 0, events_tx.clone()).await?;
        let service_id = service_id_at(&paths.identity.join("service-id"));
        let service = Service {
            instance: service_id.clone(),
            host: format!("floedrop-{service_id}"),
            port: RECEIVE_PORT,
            addr: iface.addr,
            flags: RECEIVER_FLAGS,
        };
        if let Some(b) = &announce {
            log(format!(
                "announcing {} times {} ms apart, every {} s, as the radio asks",
                b.count,
                b.interval.as_millis(),
                b.pause.as_secs()
            ));
        }
        let mdns = Mdns::new(iface.index)
            .with_burst(announce)
            .register(service)
            .await
            .map_err(|e| format!("mDNS: {e}"))?;
        let sending = if no_ble {
            log("no Bluetooth: receiving only");
            None
        } else {
            Some(Self::start_sending(iface, name, &service_id, radio, paths)?)
        };
        Ok(Airdrop {
            iface,
            identity,
            paths: paths.clone(),
            service_id,
            generation: 0,
            receiver,
            events_tx,
            events,
            pending: HashMap::new(),
            devices: Vec::new(),
            _mdns: mdns,
            sending,
            peers,
        })
    }

    fn start_sending(
        iface: Iface,
        name: &str,
        service_id: &str,
        radio: Option<RadioProxy<'static>>,
        paths: &Paths,
    ) -> Result<Sending, String> {
        std::fs::create_dir_all(&paths.outbox)
            .map_err(|e| format!("{}: {e}", paths.outbox.display()))?;
        let inotify = Inotify::init().map_err(|e| format!("inotify: {e}"))?;
        let watches = inotify.watches();
        let events = inotify
            .into_event_stream(vec![0u8; 4096])
            .map_err(|e| format!("inotify: {e}"))?;
        let sender = Sender::new(SenderConfig::new(name, service_id)).map_err(|e| e.to_string())?;
        let (sender_tx, sender_rx) = watch::channel(Arc::new(sender));
        let (cmds, cmds_rx) = smpsc::channel();
        let (out_tx, from_outbox) = mpsc::unbounded_channel();
        let wmap: WatchMap = Arc::default();
        let thread = std::thread::Builder::new()
            .name("outbox".into())
            .spawn({
                let root = paths.outbox.clone();
                let wmap = wmap.clone();
                move || outbox_thread(root, cmds_rx, out_tx, watches, wmap)
            })
            .map_err(|e| e.to_string())?;
        let discovery = tokio::spawn(discovery(
            iface,
            service_id.to_string(),
            radio,
            sender_rx,
            cmds.clone(),
        ));
        Ok(Sending {
            sender: sender_tx,
            cmds,
            thread: Some(thread),
            from_outbox,
            events,
            wmap,
            discovery,
        })
    }

    /// A new name: a new receiver (its `/Discover` answer) and sender.
    async fn rename(&mut self, name: &str) -> Result<(), String> {
        self.receiver.abort();
        let _ = (&mut self.receiver).await;
        // The aborted receiver sends no end for its transfers.
        for p in drop_generation(&mut self.pending, self.generation) {
            log(p.shown.cut_line());
        }
        self.generation += 1;
        self.receiver = bind(
            self.iface,
            &self.identity,
            name,
            &self.paths,
            self.generation,
            self.events_tx.clone(),
        )
        .await?;
        if let Some(s) = &self.sending {
            let sender = Sender::new(SenderConfig::new(name, self.service_id.as_str()))
                .map_err(|e| e.to_string())?;
            s.sender.send_replace(Arc::new(sender));
        }
        Ok(())
    }

    /// The state file's lists: receivers in range and transfers under way.
    fn lists(&self) -> (Vec<Device>, Vec<Incoming>) {
        (
            self.devices.clone(),
            incoming_list(self.pending.values().map(|p| &p.shown)),
        )
    }

    /// One event; true when the lists may have changed.
    async fn step(&mut self) -> bool {
        let check = next_check(&self.pending);
        let step = match self.sending.as_mut() {
            Some(s) => tokio::select! {
                e = self.events.recv() => Step::Receiver(e.map(Box::new)),
                m = s.from_outbox.recv() => Step::Outbox(m),
                i = s.events.next() => Step::Inotify(i),
                () = check => Step::Check,
            },
            None => tokio::select! {
                e = self.events.recv() => Step::Receiver(e.map(Box::new)),
                () = check => Step::Check,
            },
        };
        match step {
            Step::Check => {
                let uploading = staging_active(
                    &self.paths.incoming,
                    std::time::SystemTime::now(),
                    UPLOAD_QUIET,
                );
                let ended = expire(&mut self.pending, tokio::time::Instant::now(), uploading);
                for p in &ended {
                    log(p.shown.ended_line());
                }
                return !ended.is_empty();
            }
            Step::Receiver(Some(b)) => {
                let (generation, ev) = *b;
                return self.on_receiver(generation, ev);
            }
            // The sender half of the channel is ours: it never closes.
            Step::Receiver(None) => std::future::pending().await,
            Step::Outbox(Some(FromOutbox::Devices(d))) => {
                self.devices = d;
                return true;
            }
            Step::Outbox(Some(FromOutbox::Send {
                handle,
                target,
                paths,
            })) => self.send(handle, target, paths),
            Step::Outbox(None) | Step::Inotify(None) => {
                // Only a panic ends them; receiving goes on without sending.
                log("the outbox stopped; receiving only");
                if let Some(s) = self.sending.take() {
                    s.discovery.abort();
                }
            }
            Step::Inotify(Some(Ok(ev))) => self.on_inotify(ev),
            Step::Inotify(Some(Err(e))) => log(format!("inotify: {e}")),
        }
        false
    }

    fn on_inotify(&mut self, ev: inotify::Event<std::ffi::OsString>) {
        let Some(s) = &self.sending else { return };
        let mut map = s.wmap.lock().unwrap();
        if ev.mask.contains(EventMask::IGNORED) {
            map.remove(&ev.wd);
            return;
        }
        if let Some(folder) = map.get(&ev.wd) {
            let _ = s.cmds.send(Cmd::Changed(folder.clone()));
        }
    }

    fn send(&mut self, handle: SendHandle, target: Option<Target>, paths: Vec<PathBuf>) {
        let Some(s) = &self.sending else { return };
        let cmds = s.cmds.clone();
        let Some(target) = target else {
            let _ = cmds.send(Cmd::Finished(
                handle,
                Outcome::Failed("not in range".into()),
            ));
            return;
        };
        let sender = s.sender.borrow().clone();
        tokio::spawn(async move {
            let payload = Payload::Files(paths);
            let outcome = match sender.send(target.addr, &payload, target.flags, None).await {
                Ok(OwlyOutcome::Delivered) => Outcome::Delivered,
                Ok(OwlyOutcome::Declined) => Outcome::Declined,
                Ok(OwlyOutcome::Failed(why)) => Outcome::Failed(why),
                Err(e) => Outcome::Failed(e.to_string()),
            };
            let _ = cmds.send(Cmd::Finished(handle, outcome));
        });
    }

    /// An accepted request, forgotten here.
    fn take(&mut self, generation: u64, id: u64) -> Option<Pending> {
        self.pending.remove(&(generation, id))
    }

    /// One receiver event; true when the transfers under way may have changed.
    fn on_receiver(&mut self, generation: u64, ev: ReceiverEvent) -> bool {
        match ev {
            ReceiverEvent::Request(r) => {
                let sender = r.ask.sender_computer_name.clone();
                let free = inbox::free_bytes(&self.paths.root).unwrap_or_else(|e| {
                    log(format!("{}: {e}", self.paths.root.display()));
                    0
                });
                let offer = incoming::from_offer(&sender, &r.ask.offer, epoch_now());
                if matches!(r.ask.offer, Offer::Urls(_)) {
                    log(incoming::links_line(&offer.sender));
                } else {
                    log(offer.offer_line());
                }
                match answer(&r.ask.offer, free, &inbox::shown_sender(&sender)) {
                    Answer::Accept => {
                        log(offer.accepted_line());
                        let pending = Pending {
                            shown: offer,
                            sender,
                            check_at: tokio::time::Instant::now() + UPLOAD_START_WAIT,
                        };
                        self.pending.insert((generation, r.id), pending);
                        r.accept();
                    }
                    Answer::Decline(note) => {
                        if !matches!(r.ask.offer, Offer::Urls(_)) {
                            log(offer.declined_line(free));
                        }
                        if let Some((tone, text)) = note {
                            osd::note(tone, &text);
                        }
                        r.decline();
                    }
                }
            }
            ReceiverEvent::Received { id, paths } => {
                let done = self.take(generation, id);
                let line = done
                    .as_ref()
                    .map_or_else(incoming::unknown_received_line, |p| p.shown.received_line());
                let sender = done.map_or_else(String::new, |p| p.sender);
                let placed = inbox::place(&paths, &self.paths.incoming, &self.paths.inbox, &sender);
                let named = inbox::shown_sender(&sender);
                log(line);
                match placed.error {
                    None if placed.files.is_empty() => {}
                    None => osd::note(
                        osd::Tone::Info,
                        &inbox::received_note(&placed.files, &named),
                    ),
                    Some(e) => {
                        log(format!(
                            "placing files from {:?}: {e}",
                            incoming::shown(&sender)
                        ));
                        // The entries moved are gone from staging; the rest is dropped.
                        for p in &paths {
                            remove_entry(p);
                        }
                        let text = if placed.files.is_empty() {
                            inbox::failed_note(&named)
                        } else {
                            inbox::partial_note(placed.files.len(), placed.total, &named)
                        };
                        osd::note(osd::Tone::Warn, &text);
                    }
                }
            }
            ReceiverEvent::Failed { id, reason } => {
                // Owly's sandbox for the upload is gone already; nothing was placed.
                let done = self.take(generation, id);
                match &done {
                    Some(p) => log(p.shown.failed_line(&reason)),
                    None => log(incoming::unknown_failed_line(&reason)),
                }
                let sender = done.map_or_else(String::new, |p| p.sender);
                osd::note(
                    osd::Tone::Warn,
                    &inbox::failed_note(&inbox::shown_sender(&sender)),
                );
            }
            ReceiverEvent::Withdrawn { id } => {
                if let Some(gone) = self.pending.remove(&(generation, id)) {
                    log(gone.shown.withdrawn_line());
                }
            }
            ReceiverEvent::Urls { .. } | ReceiverEvent::Discovered { .. } => return false,
        }
        true
    }

    async fn shutdown(mut self) {
        self.receiver.abort();
        if let Some(mut s) = self.sending.take() {
            s.discovery.abort();
            let _ = s.cmds.send(Cmd::Shutdown);
            if let Some(t) = s.thread.take() {
                let _ = tokio::task::spawn_blocking(move || t.join()).await;
            }
        }
        // The mDNS goodbye goes out when the registration drops.
        drop(self._mdns);
        tokio::time::sleep(GOODBYE_WAIT).await;
    }
}

/// The mDNS service id of this session: the 12 lower-case hex digits in `path` when it holds
/// exactly that, else a new one, written there (mode 0600, atomically). The same id after
/// FloeDrop off and on lets phones that missed the goodbye see the same receiver again, not
/// a second one. `path` is in `$XDG_RUNTIME_DIR`, gone at logout.
fn service_id_at(path: &Path) -> String {
    if let Ok(t) = std::fs::read_to_string(path) {
        let id = t.strip_suffix('\n').unwrap_or(&t);
        if id.len() == 12 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return id.to_string();
        }
    }
    let id = mdns::service_id(random_bytes());
    let write = || -> io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(format!("{id}\n").as_bytes())?;
        drop(f);
        std::fs::rename(&tmp, path)
    };
    if let Err(e) = write() {
        log(format!("{}: {e}", path.display()));
    }
    id
}

/// Completes at the first check time among `pending`; never when there is none.
async fn next_check(pending: &HashMap<(u64, u64), Pending>) {
    match pending.values().map(|p| p.check_at).min() {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// The entries whose check time has come: while an upload is under way (`uploading`) they
/// are looked at again after [`UPLOAD_RECHECK`], else they are taken out and returned. Owly
/// sends no event when a phone goes between the accept and its upload.
fn expire(
    pending: &mut HashMap<(u64, u64), Pending>,
    now: tokio::time::Instant,
    uploading: bool,
) -> Vec<Pending> {
    let due: Vec<(u64, u64)> = pending
        .iter()
        .filter(|(_, p)| p.check_at <= now)
        .map(|(k, _)| *k)
        .collect();
    let mut ended = Vec::new();
    for k in due {
        if uploading {
            if let Some(p) = pending.get_mut(&k) {
                p.check_at = now + UPLOAD_RECHECK;
            }
        } else if let Some(p) = pending.remove(&k) {
            ended.push(p);
        }
    }
    ended
}

/// The entries of the receiver of `generation`, taken out.
fn drop_generation(pending: &mut HashMap<(u64, u64), Pending>, generation: u64) -> Vec<Pending> {
    let keys: Vec<(u64, u64)> = pending
        .keys()
        .filter(|(g, _)| *g == generation)
        .copied()
        .collect();
    keys.into_iter()
        .filter_map(|k| pending.remove(&k))
        .collect()
}

/// Whether something under `dir` (Owly's staging, `.incoming/`) changed within `quiet`
/// before `now`: an upload is under way. At most four levels and 4096 entries are looked at.
fn staging_active(dir: &Path, now: std::time::SystemTime, quiet: Duration) -> bool {
    let mut seen = 0usize;
    let mut stack = vec![(dir.to_path_buf(), 0u32)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > 4096 {
                return false;
            }
            let Ok(m) = e.metadata() else { continue };
            if m.modified()
                .is_ok_and(|t| now.duration_since(t).map_or(true, |age| age < quiet))
            {
                return true;
            }
            if m.is_dir() && depth < 4 {
                stack.push((e.path(), depth + 1));
            }
        }
    }
    false
}

/// Removes one leftover staged entry without following a symlink.
fn remove_entry(p: &Path) {
    let r = match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(p),
        Ok(_) => std::fs::remove_file(p),
        Err(_) => Ok(()),
    };
    if let Err(e) = r {
        log(format!("{}: {e}", p.display()));
    }
}

/// Binds a receiver named `name` and forwards its events tagged with `generation`.
async fn bind(
    iface: Iface,
    identity: &Identity,
    name: &str,
    paths: &Paths,
    generation: u64,
    out: mpsc::Sender<(u64, ReceiverEvent)>,
) -> Result<tokio::task::JoinHandle<()>, String> {
    let config = Config::new(name);
    let addr = SocketAddrV6::new(iface.addr, RECEIVE_PORT, 0, iface.index);
    let (receiver, mut events) =
        Receiver::bind(addr, identity, Options::new(config, paths.incoming.clone()))
            .await
            .map_err(|e| format!("cannot listen on [{}]:{RECEIVE_PORT}: {e}", iface.addr))?;
    let run = tokio::spawn(receiver.run());
    // The forwarder outlives a renamed receiver: its open transfers still report.
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            if out.send((generation, ev)).await.is_err() {
                return;
            }
        }
    });
    Ok(run)
}

#[cfg(test)]
mod parent_tests {
    use super::*;

    #[test]
    fn the_menu_pid_from_the_environment_wins() {
        assert_eq!(expected_parent(Some("4242"), 17), 4242);
        assert_eq!(expected_parent(None, 17), 17);
        assert_eq!(expected_parent(Some("x"), 17), 17);
        assert_eq!(expected_parent(Some("0"), 17), 17);
    }

    #[test]
    fn a_new_parent_or_init_means_the_menu_is_gone() {
        assert!(!parent_gone(4242, 4242));
        assert!(parent_gone(4242, 1));
        assert!(parent_gone(4242, 900));
        assert!(parent_gone(1, 1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use owly_airdrop::proto::messages::AskFile;

    #[test]
    fn default_name_matches_the_menu() {
        let table = include_str!("../../../test/unit/floedrop-names.tsv");
        let mut rows = 0;
        for line in table
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let (host, want) = line.split_once('\t').expect("host<TAB>name");
            assert_eq!(default_name(host), want, "{host}");
            rows += 1;
        }
        assert!(rows >= 5);
    }

    #[test]
    fn proc_is_not_a_tmpfs() {
        assert!(!is_tmpfs(Path::new("/proc")).unwrap());
        if Path::new("/dev/shm").is_dir() {
            assert!(is_tmpfs(Path::new("/dev/shm")).unwrap());
        }
        assert!(is_tmpfs(Path::new("/no/such/dir")).is_err());
    }

    #[test]
    fn the_ready_head_says_no_peers() {
        let f = vec!["no-ble".to_string()];
        assert_eq!(ready_flags(f.clone(), false), ["no-ble", "no-peers"]);
        assert_eq!(ready_flags(f.clone(), true), f);
    }

    #[test]
    fn the_outbox_wakes_on_delete() {
        let m = watch_mask();
        assert!(m.contains(WatchMask::DELETE));
        assert!(m.contains(WatchMask::MOVED_FROM));
    }

    #[test]
    fn stable_ids() {
        assert_eq!(stable_id(None), None);
        assert_eq!(stable_id(Some(&[])), None);
        let a = stable_id(Some(b"record-a")).unwrap();
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(stable_id(Some(b"record-a")).unwrap(), a);
        assert_ne!(stable_id(Some(b"record-b")).unwrap(), a);
    }

    fn target(port: u16, flags: u32, txt: bool) -> Target {
        Target {
            addr: SocketAddrV6::new(Ipv6Addr::LOCALHOST, port, 0, 0),
            flags,
            txt,
        }
    }

    struct Rig {
        dir: PathBuf,
        ob: Outbox,
        tr: ChanTransport,
        rx: mpsc::UnboundedReceiver<FromOutbox>,
    }
    impl Rig {
        fn new(name: &str) -> Rig {
            let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-tmp"))
                .join(format!("{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let (out, rx) = mpsc::unbounded_channel();
            Rig {
                ob: Outbox::new(dir.join("outbox")),
                dir,
                tr: ChanTransport {
                    next: 0,
                    out,
                    targets: Targets::default(),
                },
                rx,
            }
        }
        fn seen(&mut self, key: &str, addr: &str, t: Target) {
            on_seen(&mut self.ob, &mut self.tr, key, addr, "Jane", None, t);
            settle(&mut self.ob, &mut self.tr, Instant::now());
        }
    }
    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_phone_seen_again_after_it_went_has_its_target_again() {
        let mut r = Rig::new("targets-gone");
        let t = target(1, 7, true);
        r.seen("Jane-mdns", "fe80::1", t);
        assert_eq!(r.tr.targets.get("Jane-mdns"), Some(t));
        let later = Instant::now() + crate::outbox::GONE_AFTER + Duration::from_secs(1);
        settle(&mut r.ob, &mut r.tr, later);
        assert_eq!(
            r.tr.targets.get("Jane-mdns"),
            None,
            "a gone phone has no target"
        );
        r.seen("Jane-mdns", "fe80::1", t);
        assert_eq!(r.tr.targets.get("Jane-mdns"), Some(t));
    }

    #[test]
    fn an_offer_carries_its_target_past_a_later_rekey() {
        let mut r = Rig::new("targets-offer");
        let t = target(1, 7, true);
        r.tr.targets.record("Jane-mdns", t);
        let h = r.tr.send("Jane-mdns", vec![PathBuf::from("a")]);
        r.tr.targets.forget(&["Jane-mdns".to_string()], |_| false);
        match r.rx.try_recv() {
            Ok(FromOutbox::Send { handle, target, .. }) => {
                assert_eq!(handle, h);
                assert_eq!(target, Some(t));
            }
            _ => panic!("no offer"),
        }
        r.tr.send("Jane-mdns", Vec::new());
        assert!(matches!(
            r.rx.try_recv(),
            Ok(FromOutbox::Send { target: None, .. })
        ));
    }

    #[test]
    fn a_phone_found_by_the_radio_and_by_mdns_keeps_its_txt_flags() {
        let mut r = Rig::new("targets-txt");
        let guess = target(1, RECEIVER_FLAGS | SUPPORTS_DVZIP, false);
        let txt = target(2, RECEIVER_FLAGS, true);
        r.seen("awdl-aa", "fe80::1", guess);
        r.seen("Jane-mdns", "fe80::1", txt);
        assert_eq!(
            r.tr.targets.get("awdl-aa"),
            Some(txt),
            "the awdl key takes the TXT flags"
        );
        assert_eq!(r.tr.targets.get("Jane-mdns"), None);
        r.seen("awdl-aa", "fe80::1", target(3, guess.flags, false));
        assert_eq!(
            r.tr.targets.get("awdl-aa"),
            Some(target(3, RECEIVER_FLAGS, true)),
            "a later radio answer moves the address only"
        );
    }

    #[test]
    fn forget_keeps_a_key_a_device_answers_to() {
        let mut ts = Targets::default();
        for k in ["awdl-aa", "mdns-1", "other"] {
            ts.record(k, target(1, 0, false));
        }
        ts.forget(&["awdl-aa".to_string(), "mdns-1".to_string()], |k| {
            k == "mdns-1"
        });
        let mut left: Vec<String> = ts.0.keys().cloned().collect();
        left.sort();
        assert_eq!(left, ["mdns-1", "other"]);
    }

    #[test]
    fn the_ready_log_names_the_card() {
        assert_eq!(
            ready_text("awdl0", "wlxc03a55f0d060", "rtw88_8822bu", "exclusive", 6),
            "ready on wlxc03a55f0d060 (rtw88_8822bu, exclusive, channel 6)"
        );
        assert_eq!(
            ready_text("awdl0", "wlx1", "", "shared", 44),
            "ready on wlx1 (shared, channel 44)"
        );
        // An older helper: the link it reported.
        assert_eq!(
            ready_text("awdl0", "", "", "exclusive", 6),
            "ready on awdl0 (exclusive, channel 6)"
        );
        assert_eq!(
            ready_text("awdl0", "wlx1", "rtw88_8822bu", "exclusive", 0),
            "ready on wlx1 (rtw88_8822bu, exclusive)"
        );
    }

    #[test]
    fn the_service_id_is_kept_in_the_runtime_dir() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-tmp"))
            .join(format!("service-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("service-id");
        let hex12 = |s: &str| {
            s.len() == 12
                && s.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        };
        let first = service_id_at(&p);
        assert!(hex12(&first), "{first}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), format!("{first}\n"));
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(service_id_at(&p), first);
        std::fs::write(&p, "nonsense").unwrap();
        let new = service_id_at(&p);
        assert!(hex12(&new), "{new}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), format!("{new}\n"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_transfers_have_two_lines() {
        let one = |sender: &str, since| Incoming {
            sender: sender.into(),
            files: 1,
            bytes: 1,
            first: "a.jpg".into(),
            since,
        };
        let mut pending = HashMap::new();
        pending.insert((0, 1), one("B", 5));
        pending.insert((0, 2), one("A", 3));
        let names = |p: &HashMap<(u64, u64), Incoming>| {
            incoming_list(p.values())
                .into_iter()
                .map(|i| i.sender)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&pending), ["A", "B"]);
        // The same phone twice, the same second: both stay.
        pending.insert((0, 3), one("A", 3));
        assert_eq!(names(&pending), ["A", "A", "B"]);
        pending.remove(&(0, 2));
        pending.remove(&(0, 3));
        assert_eq!(names(&pending), ["B"]);
    }

    fn pending_at(sender: &str, check_at: tokio::time::Instant) -> Pending {
        Pending {
            shown: Incoming {
                sender: sender.into(),
                files: 1,
                bytes: 1,
                first: "a.jpg".into(),
                since: 1,
            },
            sender: sender.into(),
            check_at,
        }
    }

    #[test]
    fn an_accept_with_no_upload_ends_at_its_check_time() {
        let t0 = tokio::time::Instant::now();
        let mut pending = HashMap::new();
        pending.insert((0, 1), pending_at("A", t0 + UPLOAD_START_WAIT));
        pending.insert((0, 2), pending_at("B", t0 + UPLOAD_START_WAIT * 2));
        // Not due yet.
        assert!(expire(&mut pending, t0, false).is_empty());
        // Due while an upload runs: looked at again later.
        let due = t0 + UPLOAD_START_WAIT;
        assert!(expire(&mut pending, due, true).is_empty());
        assert_eq!(pending[&(0, 1)].check_at, due + UPLOAD_RECHECK);
        // Due with nothing arriving: it ends and leaves the lists.
        let ended = expire(&mut pending, due + UPLOAD_RECHECK, false);
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].shown.sender, "A");
        let left = incoming_list(pending.values().map(|p| &p.shown));
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].sender, "B");
    }

    #[test]
    fn a_name_change_ends_the_old_receivers_transfers() {
        let t0 = tokio::time::Instant::now();
        let mut pending = HashMap::new();
        pending.insert((0, 1), pending_at("A", t0));
        pending.insert((0, 2), pending_at("B", t0));
        pending.insert((1, 1), pending_at("C", t0));
        let mut cut: Vec<String> = drop_generation(&mut pending, 0)
            .into_iter()
            .map(|p| p.shown.sender)
            .collect();
        cut.sort();
        assert_eq!(cut, ["A", "B"]);
        assert_eq!(pending.len(), 1);
        assert!(pending.contains_key(&(1, 1)));
    }

    #[test]
    fn a_recent_change_in_staging_is_an_upload() {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-tmp"))
            .join(format!("staging-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("upload-1/Album")).unwrap();
        let now = std::time::SystemTime::now();
        // An empty staging dir counts by its own time; an hour later nothing is recent.
        assert!(staging_active(&dir, now, UPLOAD_QUIET));
        let later = now + Duration::from_secs(3600);
        assert!(!staging_active(&dir, later, UPLOAD_QUIET));
        std::fs::write(dir.join("upload-1/Album/a.jpg"), b"x").unwrap();
        assert!(staging_active(
            &dir,
            std::time::SystemTime::now(),
            UPLOAD_QUIET
        ));
        assert!(!staging_active(&dir.join("missing"), now, UPLOAD_QUIET));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_start_failure_names_the_card() {
        assert_eq!(
            start_failed_text("awdl0", "wlxc03a55f0d060", "rtw88_8822bu", "mDNS: x"),
            "FloeDrop cannot start on wlxc03a55f0d060 (rtw88_8822bu): mDNS: x"
        );
        assert_eq!(
            start_failed_text("awdl0", "wlx1", "", "mDNS: x"),
            "FloeDrop cannot start on wlx1: mDNS: x"
        );
        assert_eq!(
            start_failed_text("awdl0", "", "", "mDNS: x"),
            "FloeDrop cannot start on awdl0: mDNS: x"
        );
    }

    #[test]
    fn helper_lines_parse() {
        assert_eq!(
            parse_helper_line(
                "up\tawdl0\texclusive\t44\tno-ble\twlxc03a55f0d060\trtw88_8822bu\tfuture"
            ),
            Some(HelperLine::Up {
                iface: "awdl0".into(),
                mode: "exclusive".into(),
                channel: 44,
                flags: vec!["no-ble".into()],
                station: "wlxc03a55f0d060".into(),
                driver: "rtw88_8822bu".into(),
            })
        );
        // A helper without the card fields.
        assert_eq!(
            parse_helper_line("up\tawdl0\tshared\t6\t\n"),
            Some(HelperLine::Up {
                iface: "awdl0".into(),
                mode: "shared".into(),
                channel: 6,
                flags: vec![],
                station: String::new(),
                driver: String::new(),
            })
        );
        assert_eq!(
            parse_helper_line("error\tno card"),
            Some(HelperLine::Error("no card".into()))
        );
        assert_eq!(
            parse_helper_line("error"),
            Some(HelperLine::Error(HELPER_GONE.into()))
        );
        assert_eq!(parse_helper_line("hello"), None);
        assert_eq!(parse_helper_line("up\t"), None);
    }

    fn file(size: Option<u64>) -> AskFile {
        AskFile {
            name: "a.jpg".into(),
            file_type: "public.jpeg".into(),
            size,
            bom_path: "./a.jpg".into(),
            is_directory: false,
        }
    }

    #[test]
    fn mac_to_link_local() {
        assert_eq!(
            link_local("02:00:00:00:00:0a"),
            Some("fe80::ff:fe00:a".parse().unwrap())
        );
        assert_eq!(
            link_local("aa:bb:cc:dd:ee:01"),
            Some("fe80::a8bb:ccff:fedd:ee01".parse().unwrap())
        );
        assert_eq!(link_local("nonsense"), None);
        assert_eq!(link_local("aa:bb:cc:dd:ee"), None);
    }

    #[test]
    fn if_inet6_finds_the_link_local() {
        let text = "\
00000000000000000000000000000001 01 80 10 80       lo
20010db8000000000000000000000001 07 40 00 00    awdl0
fe80000000000000a8bbccfffedd ee01 07 40 20 80    awdl0
fe80000000000000a8bbccfffeddee01 07 40 20 80    awdl0
fe800000000000000000000000000002 03 40 20 80     eth0
";
        assert_eq!(
            if_inet6_link_local(text, "awdl0"),
            Some(("fe80::a8bb:ccff:fedd:ee01".parse().unwrap(), 7))
        );
        assert_eq!(
            if_inet6_link_local(text, "eth0"),
            Some(("fe80::2".parse().unwrap(), 3))
        );
        assert_eq!(if_inet6_link_local(text, "lo"), None);
    }

    #[test]
    fn an_offer_that_does_not_fit_is_declined_with_a_note() {
        let offer = Offer::Files(vec![file(Some(10)), file(Some(5))]);
        assert_eq!(answer(&offer, 15, "Jane's iPhone"), Answer::Accept);
        assert_eq!(
            answer(&offer, 14, "Jane's iPhone"),
            Answer::Decline(Some((
                osd::Tone::Warn,
                "Files from Jane's iPhone refused: the FloeDrop folder is full".into()
            )))
        );
        // Links are not taken, and that is no reason for a note.
        let links = Offer::Urls(vec!["https://example.org".into()]);
        assert_eq!(
            answer(&links, 1 << 30, "Jane's iPhone"),
            Answer::Decline(None)
        );
    }

    #[test]
    fn unblock_hangup_clears_sighup_from_the_threads_mask() {
        // On a thread of its own, so the test binary's mask stays as it is.
        std::thread::spawn(|| unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGHUP);
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            unblock_hangup();
            let mut now: libc::sigset_t = std::mem::zeroed();
            libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut now);
            assert_eq!(libc::sigismember(&now, libc::SIGHUP), 0);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn empty_name_is_floedrop_and_the_host_id() {
        assert_eq!(share_name("", "thinfloe-56db68"), "FloeDrop 56db68");
        assert_eq!(share_name("  ", "thinfloe-a1b2c3"), "FloeDrop a1b2c3");
        assert_eq!(share_name("", "desk-7"), "FloeDrop desk-7");
        assert_eq!(share_name("", "thinfloe-"), "FloeDrop thinfloe-");
        assert_eq!(share_name("Front desk", "thinfloe-a1b2c3"), "Front desk");
    }
}
