//! The work a tile starts: running a program, saving the home directory.
//! Each runs on a worker thread of its own, several programs at once, in
//! the background; the event loop keeps answering the compositor while
//! they do. thinfloe-save-home on a netbooted machine can spend 60 seconds
//! pinging before it starts, and a window that stops answering frame
//! callbacks for that long looks dead.

use crate::drives::{self, Drive};
use crate::log::{Feed, Kind, Line, Run, Splitter, local_time};
use crate::netfiles;
use crate::nettab::{self, Iface, InRange};
use crate::release::{self, Release};
use crate::upgrade_proto::{self, Msg};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// How long `run` and `save` wait, once the program has exited, for the
/// rest of its output. A background process it started may hold the pipes
/// open for as long as it lives; its lines are still logged, but the tile
/// is done when the program itself is.
const DRAIN_GRACE: Duration = Duration::from_millis(300);

/// The log source of everything thinfloe-upgrade prints that is not a
/// progress line.
pub const UPGRADE_LOG_SOURCE: &str = "upgrade";
/// How long `upgrade_write` waits, after the tool exited, for its stdout to
/// be read to the end. A `@result` printed just before the exit must reach
/// the caller; this is longer than `DRAIN_GRACE` for that reason.
pub const UPGRADE_DRAIN: Duration = Duration::from_secs(5);
/// Most bytes of the drive list that are read; more is an error.
pub const DRIVES_BYTES: usize = 256 * 1024;
/// Most bytes of the release status that are read.
pub const CHECK_BYTES: usize = 4096;

/// What thinfloe-save-home reported, by its exit code. The codes are its
/// contract, documented at the top of overlay/usr/bin/thinfloe-save-home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved,
    HomeMissing,
    RestoreFailed,
    ServerUnreachable,
    NowhereToStore,
    Failed,
    /// Any other status, a signal, or the program not starting at all.
    Broken(String),
}

impl SaveOutcome {
    pub fn from_status(status: Option<i32>) -> SaveOutcome {
        match status {
            Some(0) => SaveOutcome::Saved,
            Some(1) => SaveOutcome::HomeMissing,
            Some(2) => SaveOutcome::RestoreFailed,
            Some(3) => SaveOutcome::ServerUnreachable,
            Some(4) => SaveOutcome::NowhereToStore,
            Some(5) => SaveOutcome::Failed,
            Some(n) => SaveOutcome::Broken(format!("thinfloe-save-home exited with status {n}")),
            None => SaveOutcome::Broken("thinfloe-save-home was killed by a signal".to_string()),
        }
    }

    pub fn saved(&self) -> bool {
        *self == SaveOutcome::Saved
    }

    /// The sentence the grid shows.
    pub fn message(&self) -> String {
        match self {
            SaveOutcome::Saved => "Home directory saved.".to_string(),
            SaveOutcome::HomeMissing => "Not saved: the home directory is missing.".to_string(),
            SaveOutcome::RestoreFailed => {
                "Not saved: this boot's home restore failed, and saving would overwrite the stored home with a default one."
                    .to_string()
            }
            SaveOutcome::ServerUnreachable => "Not saved: the boot server cannot be reached.".to_string(),
            SaveOutcome::NowhereToStore => "Not saved: this machine has no place to store a home directory.".to_string(),
            SaveOutcome::Failed => "Not saved: the save was attempted and failed. The session log says why.".to_string(),
            SaveOutcome::Broken(why) => format!("Not saved: {why}."),
        }
    }
}

/// Whether this boot's home restore failed, as the initramfs recorded it.
/// thinfloe-save-home refuses to save in that case; the grid greys the save
/// tile out before anyone waits a minute for the refusal. "absent" and a
/// missing file are not failures.
pub fn restore_failed(state_dir: &Path) -> bool {
    std::fs::read_to_string(state_dir.join("home-restore")).is_ok_and(|s| s.trim() == "failed")
}

/// Names one started program, so its start, its output and its end can be
/// matched while others run.
pub type JobId = u64;

/// What finished, handed back to the event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobResult {
    Saved(SaveOutcome),
    /// The program runs, in a process group of its own with this id.
    Started {
        id: JobId,
        pgid: i32,
    },
    /// `status` is the exit code, `Ok(None)` for a signal, `Err` when the
    /// program could not be started.
    Ran {
        id: JobId,
        status: Result<Option<i32>, String>,
    },
    /// An add-on job runs, in a process group of its own with this id.
    AddonStarted {
        args: Vec<String>,
        pgid: i32,
    },
    /// An add-on job ended; `args` are the ones it was started with.
    Addon {
        args: Vec<String>,
        status: Result<Option<i32>, String>,
    },
    /// The upgrade tool runs a write, in a process group of its own.
    UpgradeStarted {
        pgid: i32,
    },
    /// One progress line of the write.
    UpgradeMsg(Msg),
    /// The write ended; `Ok(None)` is a signal.
    UpgradeEnded {
        status: Result<Option<i32>, String>,
    },
    /// The drive list, asked for with or without `--all`.
    Drives {
        all: bool,
        result: Result<Vec<Drive>, String>,
    },
    /// The release check; `custom` is true when it used an address typed in.
    Release {
        custom: bool,
        result: Result<Release, String>,
    },
    /// The network files as read, and the machine's WiFi adapters.
    NetLoaded(netfiles::WifiFile, netfiles::WiredFile, Vec<String>),
    NetStatus(Result<Vec<Iface>, String>),
    NetScan(Result<Vec<InRange>, String>),
    /// The network tool applies the settings, in a process group of its own.
    NetApplyStarted {
        pgid: i32,
    },
    NetApplied(Result<(), ApplyFailed>),
    NetTakenOver(Result<(), String>),
    /// The monitor layout file, `None` when missing, and who wrote it.
    MonLoaded(Result<Option<String>, String>, crate::owned::Owner),
    MonApplied(Result<(), String>),
    MonKept(Result<(), String>),
    MonReverted(Result<(), String>),
    MonTakenOver(Result<(), String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Term,
    Kill,
}

/// The `kill` target for a process group, or `None` when the id cannot be
/// one this menu started: 0 would signal the menu's own group and 1 (as -1)
/// every process of the user.
pub fn stop_target(pgid: i32) -> Option<i32> {
    (pgid > 1).then_some(-pgid)
}

/// Signal a program and everything it started. A group that is already
/// gone is not an error.
pub fn stop(pgid: i32, signal: Signal) {
    let Some(target) = stop_target(pgid) else { return };
    let sig = match signal {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    // SAFETY: kill has no memory preconditions; ESRCH is ignored.
    unsafe {
        libc::kill(target, sig);
    }
}

pub struct Paths {
    pub save_home: PathBuf,
    pub state_dir: PathBuf,
    /// The add-on tool and what runs it as root; the job's arguments follow.
    pub addon_cmd: Vec<String>,
    /// The upgrade tool and what runs it as root; the job's arguments follow.
    pub upgrade_cmd: Vec<String>,
    /// The network tool and what runs it as root; the subcommand follows.
    pub network_cmd: Vec<String>,
    /// Where the network files live.
    pub homes: netfiles::Homes,
    /// The monitor layout file and the kanshi that applies it.
    pub kanshi: Kanshi,
}

/// Like `default_addon_cmd`, for thinfloe-upgrade.
pub fn default_upgrade_cmd() -> Vec<String> {
    ["sudo", "-n", "/usr/bin/thinfloe-upgrade"].map(String::from).to_vec()
}

/// Like `default_addon_cmd`, for thinfloe-network.
pub fn default_network_cmd() -> Vec<String> {
    ["sudo", "-n", "/usr/bin/thinfloe-network"].map(String::from).to_vec()
}

/// thinfloe-addon runs as root. tfuser has sudo without a password; `-n`
/// fails at once instead of waiting for a password nobody can type.
pub fn default_addon_cmd() -> Vec<String> {
    ["sudo", "-n", "/usr/bin/thinfloe-addon"].map(String::from).to_vec()
}

/// Copy one output pipe into the feed, line by line, until it closes.
fn pump(pipe: impl Read, source: String, feed: Arc<Feed>, done: Sender<()>) {
    pump_lines(pipe, source, feed, done, |_| false)
}

/// Like `pump`, but `take` sees the plain text of each line first, in
/// order; a line it returns true for is not logged.
fn pump_lines(
    mut pipe: impl Read,
    source: String,
    feed: Arc<Feed>,
    done: Sender<()>,
    mut take: impl FnMut(&str) -> bool,
) {
    let mut split = Splitter::default();
    let mut emit = |runs: Vec<Run>| {
        if take(&crate::log::plain(&runs)) {
            return;
        }
        feed.push(Line {
            time: local_time(),
            source: Some(source.clone()),
            runs,
            kind: Kind::Output,
        })
    };
    let mut buf = [0u8; 4096];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => split.feed(&buf[..n], &mut emit),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    split.finish(&mut emit);
    let _ = done.send(());
}

/// Start `cmd` with stdout (unless `quiet`) and stderr each read by a
/// thread of its own into the feed, wait for it, then give its output
/// `DRAIN_GRACE` to arrive.
fn run_logged(
    mut cmd: Command,
    source: &str,
    feed: &Arc<Feed>,
    quiet: bool,
    on_spawn: impl FnOnce(u32),
) -> std::io::Result<ExitStatus> {
    let stdout = if quiet { Stdio::null() } else { Stdio::piped() };
    let mut child: Child = cmd.stdin(Stdio::null()).stdout(stdout).stderr(Stdio::piped()).spawn()?;
    on_spawn(child.id());
    let (tx, rx): (Sender<()>, Receiver<()>) = mpsc::channel();
    let mut pumps = 0;
    if let Some(out) = child.stdout.take() {
        let (s, f, t) = (source.to_string(), feed.clone(), tx.clone());
        std::thread::spawn(move || pump(out, s, f, t));
        pumps += 1;
    }
    if let Some(err) = child.stderr.take() {
        let (s, f, t) = (source.to_string(), feed.clone(), tx);
        std::thread::spawn(move || pump(err, s, f, t));
        pumps += 1;
    }
    let status = child.wait();
    let deadline = Instant::now() + DRAIN_GRACE;
    for _ in 0..pumps {
        if rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_err()
        {
            break;
        }
    }
    status
}

/// Run thinfloe-save-home. Its output goes to the log as "save-home".
pub fn save(paths: &Paths, feed: &Arc<Feed>) -> SaveOutcome {
    match run_logged(Command::new(&paths.save_home), "save-home", feed, false, |_| {}) {
        Ok(st) => SaveOutcome::from_status(st.code()),
        Err(e) => SaveOutcome::Broken(format!("{} could not be started: {e}", paths.save_home.display())),
    }
}

/// Run a tile's program and wait for it; its output goes to the log under
/// the tile's name. The program gets a process group of its own, whose id
/// `on_spawn` receives before the wait, so a logout can stop it together
/// with everything it started.
pub fn run(id: JobId, name: &str, argv: &[String], feed: &Arc<Feed>, on_spawn: impl FnOnce(i32)) -> JobResult {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).process_group(0);
    let status = run_logged(cmd, name, feed, false, |pid| on_spawn(pid as i32))
        .map(|st| st.code())
        .map_err(|e| format!("{} could not be started: {e}", argv[0]));
    JobResult::Ran { id, status }
}

/// Run the add-on tool with `args`. Its stdout repeats the add-on log, which
/// the menu reads already; stderr (sudo refusing, a Python error) is not in
/// that log, so it goes to the menu log under `add-ons`.
///
/// It gets a process group of its own, whose id `on_spawn` receives:
/// `stop` signals a whole group, which would otherwise be the menu's. sudo
/// keeps the user's real uid, so it receives the signal and relays it.
pub fn addon(cmd: &[String], args: &[String], feed: &Arc<Feed>, on_spawn: impl FnOnce(i32)) -> JobResult {
    use std::os::unix::process::CommandExt;
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]).args(args).process_group(0);
    let status = run_logged(c, crate::addons::LOG_SOURCE, feed, true, |pid| on_spawn(pid as i32))
        .map(|st| st.code())
        .map_err(|e| format!("{} could not be started: {e}", cmd[0]));
    JobResult::Addon {
        args: args.to_vec(),
        status,
    }
}

/// Start the tool in a process group of its own: `stop` signals a whole
/// group, which would otherwise be the menu's.
fn upgrade_command(cmd: &[String], args: &[String]) -> Command {
    use std::os::unix::process::CommandExt;
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]).args(args).process_group(0).stdin(Stdio::null());
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    c
}

/// Run `thinfloe-upgrade write` with `args`. Progress lines go to `on_msg`
/// in order; all other output goes to the log. The tool is never killed
/// from here: `on_spawn` gets its process group so the caller can send
/// SIGTERM.
///
/// Returns once the tool exited and its stdout was read to the end, waiting
/// at most `UPGRADE_DRAIN` for that, so a `@result` printed just before the
/// exit is not lost. If a background process keeps stdout open past that,
/// one line says so in the log, and from then on `on_msg` is never called
/// again (later lines go to the log as plain output).
///
/// `on_msg` runs on the pump thread, under a lock that `upgrade_write`
/// takes before it returns: it must not block or panic.
pub fn upgrade_write(
    cmd: &[String],
    args: &[String],
    feed: &Arc<Feed>,
    on_spawn: impl FnOnce(i32),
    on_msg: impl Fn(Msg) + Send + 'static,
) -> Result<Option<i32>, String> {
    let mut child = upgrade_command(cmd, args)
        .spawn()
        .map_err(|e| format!("{} could not be started: {e}", cmd[0]))?;
    on_spawn(child.id() as i32);
    let closed = Arc::new(std::sync::Mutex::new(false));
    let (out_tx, out_rx): (Sender<()>, Receiver<()>) = mpsc::channel();
    let (err_tx, err_rx): (Sender<()>, Receiver<()>) = mpsc::channel();
    let have_out = child.stdout.is_some();
    if let Some(out) = child.stdout.take() {
        let (f, c) = (feed.clone(), closed.clone());
        std::thread::spawn(move || {
            pump_lines(out, UPGRADE_LOG_SOURCE.to_string(), f, out_tx, |line| {
                let closed = c.lock().unwrap_or_else(|e| e.into_inner());
                if *closed {
                    return false;
                }
                match upgrade_proto::parse(line) {
                    Some(m) => {
                        on_msg(m);
                        true
                    }
                    None => false,
                }
            })
        });
    }
    let have_err = child.stderr.is_some();
    if let Some(err) = child.stderr.take() {
        let f = feed.clone();
        std::thread::spawn(move || pump(err, UPGRADE_LOG_SOURCE.to_string(), f, err_tx));
    }
    let status = child
        .wait()
        .map(|s| s.code())
        .map_err(|e| format!("waiting for {}: {e}", cmd[0]));
    let deadline = Instant::now() + UPGRADE_DRAIN;
    let out_done = !have_out || out_rx.recv_timeout(UPGRADE_DRAIN).is_ok();
    if !out_done {
        // From here on no progress line reaches the caller.
        *closed.lock().unwrap_or_else(|e| e.into_inner()) = true;
        feed.push(Line {
            time: local_time(),
            source: Some(UPGRADE_LOG_SOURCE.to_string()),
            runs: vec![Run::plain(format!(
                "the writer's output stayed open {} s after it ended; later lines are ignored",
                UPGRADE_DRAIN.as_secs()
            ))],
            kind: Kind::Warn,
        });
    }
    if have_err {
        let _ = err_rx.recv_timeout(deadline.saturating_duration_since(Instant::now()).max(DRAIN_GRACE));
    }
    // Lines after this point are logged only.
    *closed.lock().unwrap_or_else(|e| e.into_inner()) = true;
    status
}

/// Longest wait for the drive list and for the release check. The check
/// makes two curl calls of up to 30 s each.
pub const DRIVES_TIMEOUT: Duration = Duration::from_secs(30);
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(90);
/// After SIGTERM for a timeout, how long to wait for the tool to end.
/// Then we give up waiting; it may linger, these are not write jobs. A tool
/// that ignores SIGTERM past this is dropped unreaped and stays a zombie
/// until it ends and the menu exits.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// Wait for `child` until `deadline`; `None` when it still runs.
fn wait_until(child: &mut Child, deadline: Instant) -> Option<ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return Some(st),
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Run the tool and read at most `cap` bytes of stdout; stderr goes to the
/// log. More than `cap` is an error and the tool is stopped with SIGTERM,
/// as is a tool that does not finish within `timeout`.
fn capture(
    cmd: &[String],
    args: &[String],
    cap: usize,
    timeout: Duration,
    feed: &Arc<Feed>,
) -> Result<(String, Option<i32>), String> {
    capture_as(cmd, args, cap, timeout, feed, UPGRADE_LOG_SOURCE, |_| {}).map(|(out, status, _)| (out, status))
}

/// How much of a tool's stderr `capture_as` keeps for the caller.
const STDERR_KEEP: usize = 4096;

/// `capture` for any tool: stderr goes to the log under `source`, and
/// `on_spawn` gets the process group before the wait.
fn capture_as(
    cmd: &[String],
    args: &[String],
    cap: usize,
    timeout: Duration,
    feed: &Arc<Feed>,
    source: &str,
    on_spawn: impl FnOnce(i32),
) -> Result<(String, Option<i32>, String), String> {
    let mut child = upgrade_command(cmd, args)
        .spawn()
        .map_err(|e| format!("{} could not be started: {e}", cmd[0]))?;
    on_spawn(child.id() as i32);
    let started = Instant::now();
    let (tx, rx) = mpsc::channel();
    let mut pumps = 0;
    let kept = Arc::new(std::sync::Mutex::new(String::new()));
    if let Some(err) = child.stderr.take() {
        let f = feed.clone();
        let source = source.to_string();
        let kept = kept.clone();
        std::thread::spawn(move || {
            pump_lines(err, source, f, tx, |line| {
                let mut k = kept.lock().unwrap_or_else(|e| e.into_inner());
                if k.len() < STDERR_KEEP {
                    k.push_str(line);
                    k.push('\n');
                }
                false
            })
        });
        pumps += 1;
    }
    let (out_tx, out_rx) = mpsc::channel();
    if let Some(pipe) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = pipe.take(cap as u64 + 1).read_to_end(&mut out);
            // Dropping the pipe here gives a still-writing tool SIGPIPE.
            let _ = out_tx.send(out);
        });
    } else {
        let _ = out_tx.send(Vec::new());
    }
    let out = match out_rx.recv_timeout(timeout) {
        Ok(out) => out,
        Err(_) => {
            stop(child.id() as i32, Signal::Term);
            wait_until(&mut child, Instant::now() + STOP_WAIT);
            return Err(format!(
                "{} did not answer within {} s",
                cmd[0],
                timeout.as_secs_f32().round()
            ));
        }
    };
    let too_long = out.len() > cap;
    if too_long {
        stop(child.id() as i32, Signal::Term);
    }
    let left = timeout
        .saturating_sub(started.elapsed())
        .max(if too_long { STOP_WAIT } else { Duration::ZERO });
    let Some(status) = wait_until(&mut child, Instant::now() + left) else {
        stop(child.id() as i32, Signal::Term);
        wait_until(&mut child, Instant::now() + STOP_WAIT);
        return Err(format!(
            "{} did not end within {} s",
            cmd[0],
            timeout.as_secs_f32().round()
        ));
    };
    let deadline = Instant::now() + DRAIN_GRACE;
    for _ in 0..pumps {
        if rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_err()
        {
            break;
        }
    }
    if too_long {
        return Err(format!("{} printed more than {cap} bytes", cmd[0]));
    }
    let stderr = kept.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Ok((String::from_utf8_lossy(&out).into_owned(), status.code(), stderr))
}

/// The drives the tool offers: `drives --no-size [--all]`.
pub fn upgrade_drives(cmd: &[String], all: bool, feed: &Arc<Feed>) -> Result<Vec<Drive>, String> {
    let mut args = vec!["drives".to_string(), "--no-size".to_string()];
    if all {
        args.push("--all".to_string());
    }
    let (text, status) = capture(cmd, &args, DRIVES_BYTES, DRIVES_TIMEOUT, feed)?;
    if status != Some(0) {
        return Err(match status {
            Some(n) => format!("the drive list failed with status {n}"),
            None => "the drive list was ended by a signal".to_string(),
        });
    }
    drives::parse(&text)
}

/// The latest release: `check [--source URL]`. Exit 0 and 1 both carry the
/// lines (the `error` field says why a check failed); anything else is an
/// error, as is no output at all.
pub fn upgrade_check(cmd: &[String], source: Option<&str>, feed: &Arc<Feed>) -> Result<Release, String> {
    let mut args = vec!["check".to_string()];
    if let Some(url) = source {
        args.push("--source".to_string());
        args.push(url.to_string());
    }
    let (text, status) = capture(cmd, &args, CHECK_BYTES, CHECK_TIMEOUT, feed)?;
    match status {
        Some(0) | Some(1) if !text.trim().is_empty() => Ok(release::parse_status(&text)),
        Some(0) | Some(1) => Err("the release check printed nothing".to_string()),
        Some(n) => Err(format!("the release check failed with status {n}")),
        None => Err("the release check was ended by a signal".to_string()),
    }
}

/// Source name of the network tool's output in the menu log.
pub const NETWORK_LOG_SOURCE: &str = "network";
/// Output limit of the network tool's calls.
pub const NETWORK_BYTES: usize = 256 * 1024;
pub const NETWORK_STATUS_TIMEOUT: Duration = Duration::from_secs(5);
pub const NETWORK_SCAN_TIMEOUT: Duration = Duration::from_secs(40);
/// The tool's worst case for `apply`: 30 s + 30 s + 120 s = 180 s, plus
/// 20 s of margin.
pub const NETWORK_APPLY_TIMEOUT: Duration = Duration::from_secs(200);

/// Run a `thinfloe-network` subcommand that prints JSON and must exit 0.
fn network_json(
    paths: &Paths,
    args: &[&str],
    timeout: Duration,
    feed: &Arc<Feed>,
    on_spawn: impl FnOnce(i32),
) -> Result<String, String> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let (text, status, stderr) = capture_as(
        &paths.network_cmd,
        &args,
        NETWORK_BYTES,
        timeout,
        feed,
        NETWORK_LOG_SOURCE,
        on_spawn,
    )?;
    if status == Some(0) {
        return Ok(text);
    }
    // The tool's own reason, else what sudo or the shell said, else the code.
    Err(nettab::failed_reason(&text)
        .or_else(|| {
            stderr
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(|l| l.chars().take(200).collect())
        })
        .unwrap_or_else(|| match status {
            Some(n) => format!("The network tool did not answer (exit {n})."),
            None => "The network tool did not answer (killed).".to_string(),
        }))
}

/// A failed call prints `{"result": "failed", "reason": ...}`; show that
/// reason instead of a parse error.
fn failed_reason(text: &str) -> Option<String> {
    nettab::failed_reason(text)
}

/// The interfaces: `status`.
pub fn network_status(paths: &Paths, feed: &Arc<Feed>) -> Result<Vec<Iface>, String> {
    let text = network_json(paths, &["status"], NETWORK_STATUS_TIMEOUT, feed, |_| {})?;
    nettab::parse_status(&text).map_err(|e| failed_reason(&text).unwrap_or(e))
}

/// The WiFi networks in range: `scan`.
pub fn network_scan(paths: &Paths, feed: &Arc<Feed>) -> Result<Vec<InRange>, String> {
    let text = network_json(paths, &["scan"], NETWORK_SCAN_TIMEOUT, feed, |_| {})?;
    nettab::parse_scan(&text).map_err(|e| failed_reason(&text).unwrap_or(e))
}

/// A network Apply that did not happen. `home_changed`: the home files
/// could not be put back as they were, so the home is to be saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyFailed {
    pub reason: String,
    pub home_changed: bool,
}

impl ApplyFailed {
    /// Failed, with the home as it was.
    pub fn new(reason: String) -> ApplyFailed {
        ApplyFailed {
            reason,
            home_changed: false,
        }
    }
}

/// Save `change` into the home, then (only when that worked) let the root
/// tool `apply` it, in a process group of its own that `on_spawn` gets.
/// `netboot` is the caller's to know.
pub fn network_apply(
    paths: &Paths,
    change: &netfiles::Change,
    netboot: bool,
    feed: &Arc<Feed>,
    on_spawn: impl FnOnce(i32),
) -> Result<(), ApplyFailed> {
    // Ruling 16: what the home held, so a failed apply can put it back.
    // `save` writes only menu, template and missing files, all readable.
    let files = [
        (netfiles::WIFI_FILE, change.wifi.is_some()),
        (netfiles::WIRED_FILE, change.wired.is_some()),
    ];
    let before: Vec<(&str, Option<Vec<u8>>)> = files
        .iter()
        .filter(|(_, touched)| *touched)
        .filter_map(|(name, _)| {
            crate::owned::read_bytes(&paths.homes.home.join(name))
                .ok()
                .map(|b| (*name, b))
        })
        .collect();
    // A save can fail after it wrote one file; that one goes back too.
    netfiles::save(&paths.homes, change, netboot).map_err(|e| put_back(&paths.homes.home, &before, e))?;
    let result = network_json(paths, &["apply"], NETWORK_APPLY_TIMEOUT, feed, on_spawn)
        .and_then(|text| nettab::parse_result(&text));
    result.map_err(|reason| put_back(&paths.homes.home, &before, reason))
}

/// Put back each file of `before` that the save changed. A file that
/// cannot be put back is named in the reason, and the home counts as
/// changed.
fn put_back(home: &Path, before: &[(&str, Option<Vec<u8>>)], reason: String) -> ApplyFailed {
    let mut failed = ApplyFailed::new(reason);
    for (name, old) in before {
        let path = home.join(name);
        // Unchanged: nothing to do. Anything else, a read that fails
        // included, goes to `put_back`, which keeps a link or a hand file
        // and says so.
        let now = crate::owned::read_bytes(&path);
        if now.as_ref().is_ok_and(|b| b == old) {
            continue;
        }
        if let Err(e) = crate::owned::put_back(&path, old.as_deref()) {
            failed.reason = format!("{}; ~/{name} could not be put back: {e}", failed.reason);
            failed.home_changed = true;
        }
    }
    failed
}

/// The monitor layout: kanshi's config, the files that make an Apply
/// undoable, and how kanshi is found and started.
#[derive(Debug, Clone)]
pub struct Kanshi {
    /// `~/.config/kanshi/config`.
    pub config: PathBuf,
    /// `$XDG_RUNTIME_DIR/thinfloe-layout-unconfirmed`; thinfloe-session
    /// undoes the layout when it finds it. `None` without a runtime dir:
    /// then no Apply is made.
    pub marker: Option<PathBuf>,
    /// kanshi, when none runs; `-c <config>` follows.
    pub cmd: Vec<String>,
    /// Where running processes are looked for: `/proc`.
    pub proc_dir: PathBuf,
}

/// How long a kanshi the menu started may take to fail at once.
pub const KANSHI_START_GRACE: Duration = Duration::from_millis(300);

impl Kanshi {
    /// The session's files; `TF_KANSHI` replaces the kanshi program.
    pub fn from_env() -> Kanshi {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/home/tfuser"));
        let marker = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|d| !d.is_empty())
            .map(|d| PathBuf::from(d).join("thinfloe-layout-unconfirmed"));
        let cmd = match std::env::var("TF_KANSHI") {
            Ok(v) if !v.trim().is_empty() => vec![v],
            _ => vec!["kanshi".to_string()],
        };
        Kanshi {
            config: home.join(".config/kanshi/config"),
            marker,
            cmd,
            proc_dir: PathBuf::from("/proc"),
        }
    }

    /// The config as it was before an Apply that is not confirmed yet.
    pub fn prev(&self) -> PathBuf {
        let mut p = self.config.as_os_str().to_owned();
        p.push(".prev");
        PathBuf::from(p)
    }
}

/// Write `bytes` to `path` through a new file beside it and a rename.
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let written = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

fn remove_if_there(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The text of the layout file and who wrote it.
pub fn mon_load(k: &Kanshi) -> (Result<Option<String>, String>, crate::owned::Owner) {
    match crate::owned::read(&k.config) {
        Ok(t) => {
            let owner = crate::owned::owner(t.as_deref());
            (Ok(t), owner)
        }
        Err(e) => (
            Err(format!(
                "Could not read {}: {e}",
                crate::owned::home_relative(&k.config)
            )),
            crate::owned::Owner::ByHand,
        ),
    }
}

/// The processes of this user whose name is `kanshi`, as thinfloe-session
/// finds them.
pub fn kanshi_pids(proc_dir: &Path) -> Vec<i32> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let Ok(dir) = std::fs::read_dir(proc_dir) else {
        return Vec::new();
    };
    let mut pids: Vec<i32> = dir
        .flatten()
        .filter_map(|e| {
            let pid: i32 = e.file_name().to_str()?.parse().ok()?;
            let meta = std::fs::metadata(e.path()).ok()?;
            let comm = crate::bounded::read_bounded(&e.path().join("comm"), 64, true).ok()?;
            (pid > 1 && meta.uid() == uid && comm.strip_suffix(b"\n").unwrap_or(&comm) == b"kanshi").then_some(pid)
        })
        .collect();
    pids.sort_unstable();
    pids
}

/// Tell the running kanshi to read its config again; when none runs,
/// start one in a session of its own, so it outlives the menu.
pub fn kanshi_reload(k: &Kanshi) -> Result<(), String> {
    // SAFETY: kill takes no pointers; a stale pid fails with ESRCH.
    let told = kanshi_pids(&k.proc_dir)
        .into_iter()
        .filter(|&pid| unsafe { libc::kill(pid, libc::SIGHUP) } == 0)
        .count();
    if told > 0 {
        return Ok(());
    }
    use std::os::unix::process::CommandExt;
    let (prog, args) = k.cmd.split_first().ok_or("no kanshi program")?;
    let mut cmd = Command::new(prog);
    cmd.args(args).arg("-c").arg(&k.config).stdin(Stdio::null());
    // SAFETY: setsid is async-signal-safe and touches no memory.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("kanshi could not be started: {e}"))?;
    let deadline = Instant::now() + KANSHI_START_GRACE;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(s)) if s.success() => return Ok(()),
            Ok(Some(s)) => return Err(format!("kanshi stopped at once ({s}).")),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("kanshi: {e}")),
        }
    }
    // Reaped when it ends, so it leaves no zombie behind.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Write the new layout and let kanshi apply it. First the old file goes
/// to `config.prev`, and the marker `prev` says so, so a crash at any
/// later point is undone by the next session. Without a file, `prev_body`
/// (the body of a file for the layout on screen) is kept instead: a revert
/// needs a layout to go back to, since kanshi keeps the last one it set.
/// The menu never writes the marker `none`. On a failure the old file is
/// put back and both go.
pub fn mon_apply(k: &Kanshi, text: &str, prev_body: Option<&str>) -> Result<(), String> {
    let rel = crate::owned::home_relative(&k.config);
    let old = crate::owned::read_bytes(&k.config).map_err(|e| format!("Could not read {rel}: {e}"))?;
    let old_text = old.as_deref().map(String::from_utf8_lossy);
    if crate::owned::owner(old_text.as_deref()) == crate::owned::Owner::ByHand {
        return Err(format!("{rel} was written by hand"));
    }
    // Ruling 15: no crash guard, no Apply.
    let Some(marker) = &k.marker else {
        return Err("There is no XDG_RUNTIME_DIR for the undo marker; nothing was changed.".to_string());
    };
    // An earlier Apply whose undo failed: its config.prev is the real old
    // layout and must not be replaced. The next session undoes it.
    if std::fs::symlink_metadata(marker).is_ok() {
        return Err("An earlier monitor arrangement is not confirmed yet; nothing was changed.".to_string());
    }
    let prev = k.prev();
    let kept = match (&old, prev_body) {
        (Some(b), _) => write_new(&prev, b),
        (None, Some(body)) => write_new(&prev, format!("{}\n{body}", crate::owned::HEADER).as_bytes()),
        (None, None) => return Err("The layout on screen is not known; nothing was changed.".to_string()),
    };
    kept.map_err(|e| {
        format!(
            "Could not keep the old layout in {}: {e}",
            crate::owned::home_relative(&prev)
        )
    })?;
    if let Err(e) = write_new(marker, b"prev\n") {
        // No marker is there: it did not exist above, and the failed write
        // left none behind (the rename is its last step).
        let _ = remove_if_there(&prev);
        return Err(format!("Could not write {}: {e}", marker.display()));
    }
    crate::owned::write(&k.config, text)
        .map_err(|e| format!("Could not write {rel}: {e}"))
        .and_then(|()| kanshi_reload(k))
        .map_err(|reason| mon_undo(k, old.as_deref(), reason))
}

/// Put `old` back after a failed Apply. Only when that worked do the
/// marker and `config.prev` go: else the next session undoes it.
fn mon_undo(k: &Kanshi, old: Option<&[u8]>, reason: String) -> String {
    if let Err(e) = crate::owned::put_back(&k.config, old) {
        return format!(
            "{reason}; {} could not be put back: {e}",
            crate::owned::home_relative(&k.config)
        );
    }
    // The marker first, and prev only once it is gone; see `mon_revert`.
    if let Err(e) = remove_marker(k) {
        return format!("{reason}; {e}");
    }
    let _ = remove_if_there(&k.prev());
    reason
}

/// The new layout is confirmed: nothing is left to undo.
/// A marker that stays means the next session goes back: that is an error.
/// The marker goes first; see `mon_revert`.
pub fn mon_keep(k: &Kanshi) -> Result<(), String> {
    remove_marker(k)?;
    let _ = remove_if_there(&k.prev());
    Ok(())
}

fn remove_marker(k: &Kanshi) -> Result<(), String> {
    match &k.marker {
        Some(m) => remove_if_there(m).map_err(|e| format!("The undo marker {} could not be removed: {e}", m.display())),
        None => Ok(()),
    }
}

/// Go back to the layout before the Apply: `config.prev` goes back over
/// the config, or the config goes when there was none, and kanshi reads
/// it again. The marker stays when the file could not be put back, so the
/// next session tries.
pub fn mon_revert(k: &Kanshi) -> Result<(), String> {
    let prev = k.prev();
    let old = crate::owned::read_bytes(&prev)
        .map_err(|e| format!("Could not read {}: {e}", crate::owned::home_relative(&prev)))?;
    crate::owned::put_back(&k.config, old.as_deref())
        .map_err(|e| format!("{} could not be put back: {e}", crate::owned::home_relative(&k.config)))?;
    // The marker before prev: thinfloe-session reads marker `prev` without
    // config.prev as "remove the config", and would delete the layout just
    // put back if the menu died between the two.
    remove_marker(k)?;
    let _ = remove_if_there(&prev);
    if old.is_some() { kanshi_reload(k) } else { Ok(()) }
}

/// *Let the menu manage it*: the hand-written file goes to `config.by-hand`.
pub fn mon_take_over(k: &Kanshi) -> Result<(), String> {
    crate::owned::take_over(&k.config).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::{Feed, Kind, LINE_BYTES};
    use crate::testdir::TestDir;
    use std::fs;
    use std::sync::Arc;

    fn dir(tag: &str) -> TestDir {
        TestDir::new("jobs", tag)
    }

    // No test here writes a script and then executes it. Another test thread
    // forking in between inherits the still-open write descriptor, and the
    // exec fails with "Text file busy". System binaries avoid that race.

    #[test]
    fn every_save_exit_code_has_its_own_outcome() {
        let all: Vec<SaveOutcome> = (0..=5).map(|c| SaveOutcome::from_status(Some(c))).collect();
        assert_eq!(
            all,
            [
                SaveOutcome::Saved,
                SaveOutcome::HomeMissing,
                SaveOutcome::RestoreFailed,
                SaveOutcome::ServerUnreachable,
                SaveOutcome::NowhereToStore,
                SaveOutcome::Failed
            ]
        );
        assert!(matches!(SaveOutcome::from_status(Some(127)), SaveOutcome::Broken(_)));
        assert!(matches!(SaveOutcome::from_status(None), SaveOutcome::Broken(_)));
        assert!(!SaveOutcome::NowhereToStore.saved());
        // On netboot a failed upload may have changed what the server
        // stores, so the message claims nothing about the stored home.
        assert_eq!(
            SaveOutcome::Failed.message(),
            "Not saved: the save was attempted and failed. The session log says why."
        );
    }

    #[test]
    fn restore_state_is_read_tolerantly() {
        let d = dir("state");
        assert!(!restore_failed(&d), "missing file is not a failure");
        fs::write(d.join("home-restore"), "absent\n").unwrap();
        assert!(!restore_failed(&d));
        fs::write(d.join("home-restore"), "failed\n").unwrap();
        assert!(restore_failed(&d));
        fs::write(d.join("home-restore"), "failed").unwrap();
        assert!(restore_failed(&d), "no trailing newline");
        fs::write(d.join("home-restore"), "failed\r\n").unwrap();
        assert!(restore_failed(&d), "CRLF");
    }

    #[test]
    fn save_runs_the_command_and_maps_its_status() {
        let d = dir("save");
        let ok = Paths {
            save_home: "/bin/true".into(),
            state_dir: d.to_path_buf(),
            addon_cmd: default_addon_cmd(),
            upgrade_cmd: default_upgrade_cmd(),
            network_cmd: default_network_cmd(),
            homes: netfiles::Homes::from_env(),
            kanshi: Kanshi::from_env(),
        };
        assert_eq!(save(&ok, &Arc::new(Feed::default())), SaveOutcome::Saved);
        let one = Paths {
            save_home: "/bin/false".into(),
            state_dir: d.to_path_buf(),
            addon_cmd: default_addon_cmd(),
            upgrade_cmd: default_upgrade_cmd(),
            network_cmd: default_network_cmd(),
            homes: netfiles::Homes::from_env(),
            kanshi: Kanshi::from_env(),
        };
        assert_eq!(save(&one, &Arc::new(Feed::default())), SaveOutcome::HomeMissing);
        let missing = Paths {
            save_home: d.join("nope"),
            state_dir: d.to_path_buf(),
            addon_cmd: default_addon_cmd(),
            upgrade_cmd: default_upgrade_cmd(),
            network_cmd: default_network_cmd(),
            homes: netfiles::Homes::from_env(),
            kanshi: Kanshi::from_env(),
        };
        assert!(matches!(
            save(&missing, &Arc::new(Feed::default())),
            SaveOutcome::Broken(_)
        ));
    }

    #[test]
    fn run_reports_the_exit_status() {
        let argv: Vec<String> = ["/bin/sh", "-c", "exit 7"].map(String::from).to_vec();
        assert_eq!(
            run(1, "P", &argv, &Arc::new(Feed::default()), |_| {}),
            JobResult::Ran {
                id: 1,
                status: Ok(Some(7)),
            }
        );
    }

    #[test]
    fn run_reports_a_program_that_cannot_start() {
        let JobResult::Ran { status, .. } = run(
            1,
            "X",
            &["/nonexistent/prog".into()],
            &Arc::new(Feed::default()),
            |_| {},
        ) else {
            panic!()
        };
        assert!(status.unwrap_err().contains("could not be started"));
    }

    fn sh(script: &str) -> Vec<String> {
        ["/bin/sh", "-c", script].map(String::from).to_vec()
    }

    #[test]
    fn run_streams_stdout_and_stderr_into_the_feed() {
        let feed = Arc::new(Feed::default());
        run(1, "Tool", &sh("echo out; echo err >&2; exit 3"), &feed, |_| {});
        let mut got: Vec<_> = feed
            .drain()
            .into_iter()
            .map(|l| (l.plain(), l.source, l.kind))
            .map(|(t, s, k)| (s, t, k))
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                (Some("Tool".to_string()), "err".to_string(), Kind::Output),
                (Some("Tool".to_string()), "out".to_string(), Kind::Output),
            ]
        );
    }

    #[test]
    fn every_line_is_in_the_feed_before_run_returns() {
        // The exit is logged after the program's last lines, not before.
        let feed = Arc::new(Feed::default());
        run(
            1,
            "Tool",
            &sh("i=0; while [ $i -lt 200 ]; do echo line$i; i=$((i+1)); done"),
            &feed,
            |_| {},
        );
        let got = feed.drain();
        assert_eq!(got.len(), 200);
        assert_eq!(got[199].plain(), "line199");
    }

    #[test]
    fn a_long_line_arrives_cut() {
        let feed = Arc::new(Feed::default());
        run(
            1,
            "Tool",
            &sh("head -c 100000 /dev/zero | tr '\\0' x; echo; echo after"),
            &feed,
            |_| {},
        );
        let got: Vec<_> = feed.drain().into_iter().map(|l| l.plain()).collect();
        assert_eq!(got, [format!("{}…", "x".repeat(LINE_BYTES)), "after".to_string()]);
    }

    #[test]
    fn a_background_child_holding_the_pipe_does_not_hold_the_tile() {
        let feed = Arc::new(Feed::default());
        let t0 = Instant::now();
        let JobResult::Ran { status, .. } = run(
            1,
            "Tool",
            &sh("(sleep 1; echo late) & echo early; exit 0"),
            &feed,
            |_| {},
        ) else {
            panic!()
        };
        assert_eq!(status, Ok(Some(0)));
        assert!(t0.elapsed() < Duration::from_millis(900), "waited {:?}", t0.elapsed());
        assert_eq!(
            feed.drain().into_iter().map(|l| l.plain()).collect::<Vec<_>>(),
            ["early"]
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut late = Vec::new();
        while late.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            late = feed.drain();
        }
        assert_eq!(
            late.into_iter().map(|l| l.plain()).collect::<Vec<_>>(),
            ["late"],
            "still logged"
        );
    }

    #[test]
    fn stop_target_refuses_0_and_1() {
        // kill(-0) is the menu's own group, kill(-1) every process the user
        // owns. A pgid that is not known yet must never reach kill.
        assert_eq!(stop_target(0), None);
        assert_eq!(stop_target(1), None);
        assert_eq!(stop_target(-5), None);
        assert_eq!(stop_target(4242), Some(-4242));
    }

    #[test]
    fn stopping_the_group_ends_the_program_and_its_background_child() {
        let feed = Arc::new(Feed::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let f = feed.clone();
        let worker = std::thread::spawn(move || {
            run(1, "Tool", &sh("sleep 30 & echo child $!; wait"), &f, move |pgid| {
                let _ = tx.send(pgid);
            })
        });
        let pgid = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // Wait for the child's pid in the output.
        let mut child = None;
        let t0 = Instant::now();
        while child.is_none() && t0.elapsed() < Duration::from_secs(5) {
            child = feed
                .drain()
                .into_iter()
                .find_map(|l| l.plain().strip_prefix("child ").and_then(|p| p.parse::<i32>().ok()));
            std::thread::sleep(Duration::from_millis(20));
        }
        let child = child.expect("child pid printed");
        stop(pgid, Signal::Term);
        let JobResult::Ran { status, .. } = worker.join().unwrap() else {
            panic!()
        };
        assert_eq!(status, Ok(None), "ended by the signal");
        let t0 = Instant::now();
        // SAFETY: signal 0 only checks that the process exists.
        while unsafe { libc::kill(child, 0) } == 0 && t0.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(unsafe { libc::kill(child, 0) }, 0, "the background child ended too");
    }

    #[test]
    fn stopping_a_finished_group_does_nothing() {
        let feed = Arc::new(Feed::default());
        let mut pgid = 0;
        run(1, "Tool", &sh("exit 0"), &feed, |p| pgid = p);
        assert!(pgid > 1);
        stop(pgid, Signal::Kill);
    }

    #[test]
    fn addon_logs_stderr_only_and_passes_the_args() {
        let feed = Arc::new(Feed::default());
        let cmd = sh("echo out; echo \"err $0 $1\" >&2; exit 3");
        let mut pgid = 0;
        let r = addon(&cmd, &["install".into(), "10-x".into()], &feed, |p| pgid = p);
        assert!(pgid > 1);
        assert_eq!(
            r,
            JobResult::Addon {
                args: vec!["install".into(), "10-x".into()],
                status: Ok(Some(3)),
            }
        );
        let got: Vec<_> = feed
            .drain()
            .into_iter()
            .map(|l| (l.source.clone(), l.plain()))
            .collect();
        assert_eq!(got, [(Some("add-ons".to_string()), "err install 10-x".to_string())]);
        let JobResult::Addon { status, .. } = addon(&["/nonexistent/sudo".into()], &["check".into()], &feed, |_| {})
        else {
            panic!()
        };
        assert!(status.unwrap_err().contains("could not be started"));
        assert_eq!(default_addon_cmd(), ["sudo", "-n", "/usr/bin/thinfloe-addon"]);
    }

    #[test]
    fn an_addon_job_has_a_process_group_of_its_own() {
        // A stop signals the job's process group; shared with the menu, that
        // would signal the menu itself.
        let feed = Arc::new(Feed::default());
        let cmd = sh("read -r _ _ _ _ pg _ < /proc/$$/stat; [ \"$pg\" = \"$$\" ] && echo own >&2");
        let mut pgid = 0;
        addon(&cmd, &[], &feed, |p| pgid = p);
        let got: Vec<_> = feed.drain().into_iter().map(|l| l.plain()).collect();
        assert_eq!(got, ["own"]);
        // SAFETY: getpgrp has no preconditions.
        assert_ne!(pgid, unsafe { libc::getpgrp() });
    }

    #[test]
    fn save_streams_its_output_as_save_home() {
        let d = dir("save-out");
        let feed = Arc::new(Feed::default());
        let p = Paths {
            save_home: "/bin/sh".into(),
            state_dir: d.to_path_buf(),
            addon_cmd: default_addon_cmd(),
            upgrade_cmd: default_upgrade_cmd(),
            network_cmd: default_network_cmd(),
            homes: netfiles::Homes::from_env(),
            kanshi: Kanshi::from_env(),
        };
        // /bin/sh with no arguments reads stdin, which is /dev/null: exit 0.
        assert_eq!(save(&p, &feed), SaveOutcome::Saved);
        let p = Paths {
            save_home: "/bin/ls".into(),
            state_dir: d.to_path_buf(),
            addon_cmd: default_addon_cmd(),
            upgrade_cmd: default_upgrade_cmd(),
            network_cmd: default_network_cmd(),
            homes: netfiles::Homes::from_env(),
            kanshi: Kanshi::from_env(),
        };
        let _ = save(&p, &feed);
        let got = feed.drain();
        assert!(!got.is_empty(), "ls wrote nothing to the feed");
        assert!(got.iter().all(|l| l.source.as_deref() == Some("save-home")));
    }

    fn collect_write(script: &str, feed: &Arc<Feed>) -> (Result<Option<i32>, String>, Vec<Msg>) {
        let got = Arc::new(std::sync::Mutex::new(Vec::new()));
        let g = got.clone();
        let st = upgrade_write(&sh(script), &[], feed, |_| {}, move |m| g.lock().unwrap().push(m));
        let v = got.lock().unwrap().clone();
        (st, v)
    }

    #[test]
    fn upgrade_write_routes_progress_and_logs_the_rest() {
        let feed = Arc::new(Feed::default());
        let (st, msgs) = collect_write(
            "echo \"@step write W\"; echo plain; echo err >&2; echo \"@result ok\"",
            &feed,
        );
        assert_eq!(st, Ok(Some(0)));
        assert_eq!(
            msgs,
            [
                Msg::Step {
                    id: "write".into(),
                    text: "W".into()
                },
                Msg::Result {
                    ok: true,
                    reason: String::new()
                }
            ]
        );
        let mut got: Vec<_> = feed
            .drain()
            .into_iter()
            .map(|l| (l.source.clone(), l.plain()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                (Some("upgrade".to_string()), "err".to_string()),
                (Some("upgrade".to_string()), "plain".to_string())
            ]
        );
    }

    #[test]
    fn a_result_printed_just_before_exit_is_never_lost() {
        for run in 0..50 {
            let feed = Arc::new(Feed::default());
            let (st, msgs) = collect_write(
                "i=0; while [ $i -lt 2000 ]; do echo \"@progress $i 2000\"; i=$((i+1)); done; echo \"@result ok\"",
                &feed,
            );
            assert_eq!(st, Ok(Some(0)));
            assert_eq!(msgs.len(), 2001, "run {run}");
            assert!(matches!(msgs.last(), Some(Msg::Result { ok: true, .. })), "run {run}");
        }
    }

    #[test]
    fn a_write_job_has_a_process_group_of_its_own() {
        let feed = Arc::new(Feed::default());
        let cmd = sh("read -r _ _ _ _ pg _ < /proc/$$/stat; [ \"$pg\" = \"$$\" ] && echo own >&2");
        let mut pgid = 0;
        upgrade_write(&cmd, &[], &feed, |p| pgid = p, |_| {}).unwrap();
        let got: Vec<_> = feed.drain().into_iter().map(|l| l.plain()).collect();
        assert_eq!(got, ["own"]);
        // SAFETY: getpgrp has no preconditions.
        assert!(pgid > 1);
        assert_ne!(pgid, unsafe { libc::getpgrp() });
    }

    #[test]
    fn sudo_refusal_reaches_the_log() {
        let feed = Arc::new(Feed::default());
        let (st, msgs) = collect_write("echo \"sudo: a password is required\" >&2; exit 1", &feed);
        assert_eq!(st, Ok(Some(1)));
        assert!(msgs.is_empty());
        let got: Vec<_> = feed.drain().into_iter().map(|l| l.plain()).collect();
        assert_eq!(got, ["sudo: a password is required"]);
    }

    #[test]
    fn drives_output_over_the_cap_is_an_error() {
        let feed = Arc::new(Feed::default());
        let cmd = sh("head -c 300000 /dev/zero | tr \"\\0\" \" \"");
        assert!(upgrade_drives(&cmd, false, &feed).is_err());
    }

    #[test]
    fn drives_passes_no_size_and_all() {
        let feed = Arc::new(Feed::default());
        let cmd: Vec<String> = [
            "/bin/sh",
            "-c",
            "echo \"[{\\\"device\\\":\\\"/dev/sdb\\\",\\\"id\\\":\\\"x\\\",\\\"model\\\":\\\"$*\\\"}]\"",
            "sh",
        ]
        .map(String::from)
        .to_vec();
        let d = upgrade_drives(&cmd, true, &feed).unwrap();
        assert_eq!(d.len(), 1);
        assert!(d[0].model.contains("drives --no-size --all"), "{:?}", d[0].model);
        let d = upgrade_drives(&cmd, false, &feed).unwrap();
        assert!(d[0].model.ends_with("drives --no-size"), "{:?}", d[0].model);
    }

    #[test]
    fn check_exit_one_still_returns_the_lines() {
        let feed = Arc::new(Feed::default());
        let r = upgrade_check(&sh("echo error=no network; exit 1"), Some("http://x"), &feed).unwrap();
        assert_eq!(r.error.as_deref(), Some("no network"));
        let r = upgrade_check(&sh("echo latest=4.1"), None, &feed).unwrap();
        assert_eq!(r.latest.as_deref(), Some("4.1"));
        assert!(upgrade_check(&sh("exit 2"), None, &feed).is_err());
        assert!(upgrade_check(&sh("exit 0"), None, &feed).is_err(), "no lines");
        assert_eq!(default_upgrade_cmd(), ["sudo", "-n", "/usr/bin/thinfloe-upgrade"]);
    }

    #[test]
    fn a_late_result_after_the_drain_is_logged_not_delivered() {
        let feed = Arc::new(Feed::default());
        let got = Arc::new(std::sync::Mutex::new(Vec::new()));
        let g = got.clone();
        let t0 = Instant::now();
        let st = upgrade_write(
            &sh("echo \"@step write W\"; (sleep 8; echo \"@result ok\") & exit 0"),
            &[],
            &feed,
            |_| {},
            move |m| g.lock().unwrap().push(m),
        );
        let took = t0.elapsed();
        assert_eq!(st, Ok(Some(0)));
        assert!(took >= UPGRADE_DRAIN && took < Duration::from_secs(8), "{took:?}");
        assert_eq!(got.lock().unwrap().len(), 1);
        let lines: Vec<_> = feed.drain().into_iter().map(|l| l.plain()).collect();
        assert!(lines.iter().any(|l| l.contains("stayed open")), "{lines:?}");
        // Wait for the late line in the log, however slow the machine.
        let mut late = Vec::new();
        let until = Instant::now() + Duration::from_secs(20);
        while !late.contains(&"@result ok".to_string()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(100));
            late.extend(feed.drain().into_iter().map(|l| l.plain()));
        }
        assert!(late.contains(&"@result ok".to_string()), "{late:?}");
        assert_eq!(got.lock().unwrap().len(), 1, "no on_msg after return");
    }

    #[test]
    fn a_tool_that_does_not_answer_is_stopped_at_the_deadline() {
        let feed = Arc::new(Feed::default());
        let t0 = Instant::now();
        let r = capture(&sh("sleep 5"), &[], 100, Duration::from_millis(300), &feed);
        assert!(r.unwrap_err().contains("did not answer"));
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    }

    fn net_paths(d: &Path, netplan: &str, tool: &str) -> Paths {
        let mut p = Paths {
            save_home: PathBuf::from("/nonexistent/save-home"),
            state_dir: PathBuf::from("/nonexistent/state"),
            addon_cmd: default_addon_cmd(),
            upgrade_cmd: default_upgrade_cmd(),
            network_cmd: ["/bin/sh", "-c", tool, "sh"].map(String::from).to_vec(),
            homes: netfiles::Homes::from_env(),
            kanshi: Kanshi::from_env(),
        };
        p.homes = netfiles::Homes {
            home: d.join("home"),
            sys_net: d.join("sys"),
            netplan: netplan.to_string(),
            default_wired: d.join("missing-default"),
        };
        std::fs::create_dir_all(d.join("home")).unwrap();
        std::fs::create_dir_all(d.join("sys/eno1")).unwrap();
        p
    }

    #[test]
    fn network_apply_skips_apply_after_failed_save() {
        let d = dir("net-apply-fail");
        let marker = d.join("called");
        let tool = format!(
            "touch {}; echo '{{\"result\":\"ok\",\"reason\":\"\"}}'",
            marker.display()
        );
        // The default wired file is missing, so no new set can be built.
        let p = net_paths(&d, "/bin/true", &tool);
        let change = netfiles::Change {
            wifi: None,
            wired: Some(netfiles::Wired::Dhcp),
        };
        let feed = Arc::new(Feed::default());
        let spawned = std::cell::Cell::new(false);
        let r = network_apply(&p, &change, false, &feed, |_| spawned.set(true));
        assert!(r.is_err(), "{r:?}");
        assert!(!marker.exists(), "the root tool ran after a failed save");
        assert!(!spawned.get());
        // netplan refusing the set fails the save as well.
        let p = net_paths(&d, "/bin/false", &tool);
        let change = netfiles::Change {
            wifi: Some(Vec::new()),
            wired: None,
        };
        assert!(network_apply(&p, &change, false, &feed, |_| {}).is_err());
        assert!(!marker.exists());
    }

    #[test]
    fn network_apply_runs_the_tool_after_a_save_and_reads_its_result() {
        let d = dir("net-apply-ok");
        let marker = d.join("called");
        let tool = format!(
            "touch {}; echo noise; echo '{{\"result\":\"ok\",\"reason\":\"\"}}'",
            marker.display()
        );
        let p = net_paths(&d, "/bin/true", &tool);
        let feed = Arc::new(Feed::default());
        let pgid = std::cell::Cell::new(0);
        let r = network_apply(&p, &netfiles::Change::default(), false, &feed, |g| pgid.set(g));
        assert_eq!(r, Ok(()));
        assert!(marker.exists());
        assert!(pgid.get() > 1);
        let tool = "echo '{\"result\":\"failed\",\"reason\":\"no link\"}'; exit 1";
        let p = net_paths(&d, "/bin/true", tool);
        assert_eq!(
            network_apply(&p, &netfiles::Change::default(), false, &feed, |_| {}),
            Err(ApplyFailed::new("no link".to_string()))
        );
        let p = net_paths(&d, "/bin/true", "exit 2");
        assert!(network_apply(&p, &netfiles::Change::default(), false, &feed, |_| {}).is_err());
    }

    /// `net_paths` with a shipped wired file, a menu ~/wifi.yaml and,
    /// when `wired` is given, a menu ~/ethernet.yaml.
    fn net_home(d: &Path, tool: &str, wired: Option<&str>) -> Paths {
        let mut p = net_paths(d, "/bin/true", tool);
        let default = d.join("default-ethernet.yaml");
        std::fs::write(
            &default,
            "network:\n  version: 2\n  ethernets:\n    eth:\n      match:\n        name: 'en*'\n      dhcp4: true\n",
        )
        .unwrap();
        p.homes.default_wired = default;
        let home = d.join("home");
        for f in ["wifi.yaml", "ethernet.yaml"] {
            let path = home.join(f);
            if path.is_dir() {
                std::fs::remove_dir(&path).unwrap();
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
        std::fs::write(
            home.join("wifi.yaml"),
            format!("{}\nnetwork:\n  version: 2\n  # old\n", crate::owned::HEADER),
        )
        .unwrap();
        if let Some(w) = wired {
            std::fs::write(home.join("ethernet.yaml"), format!("{}\n{w}", crate::owned::HEADER)).unwrap();
        }
        p
    }

    fn home_bytes(p: &Paths) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        let get = |f: &str| std::fs::read(p.homes.home.join(f)).ok();
        (get("wifi.yaml"), get("ethernet.yaml"))
    }

    fn fixed_wired() -> netfiles::Wired {
        netfiles::Wired::Static {
            address: "192.168.1.10".parse().unwrap(),
            prefix: 24,
            gateway: None,
            dns: Vec::new(),
        }
    }

    const FAILS: &str = r#"echo '{"result":"failed","reason":"x"}'; exit 1"#;

    #[test]
    fn a_failed_network_apply_puts_the_home_back() {
        let d = dir("net-put-back");
        let feed = Arc::new(Feed::default());
        // A changed WiFi file and a new wired file.
        let p = net_home(&d, FAILS, None);
        let before = home_bytes(&p);
        let change = netfiles::Change {
            wifi: Some(Vec::new()),
            wired: Some(fixed_wired()),
        };
        let r = network_apply(&p, &change, false, &feed, |_| {});
        assert_eq!(
            r,
            Err(ApplyFailed {
                reason: "x".into(),
                home_changed: false
            })
        );
        assert_eq!(home_bytes(&p), before);
        assert!(before.1.is_none(), "the new wired file is removed");
        // A wired file that existed comes back after DHCP removed it.
        let p = net_home(&d, FAILS, Some("network:\n  version: 2\n  # fixed\n"));
        let before = home_bytes(&p);
        let change = netfiles::Change {
            wifi: None,
            wired: Some(netfiles::Wired::Dhcp),
        };
        assert!(network_apply(&p, &change, false, &feed, |_| {}).is_err());
        assert_eq!(home_bytes(&p), before);
        assert!(before.1.is_some());
        // The same change that works keeps the new files.
        let p = net_home(&d, r#"echo '{"result":"ok","reason":""}'"#, None);
        let change = netfiles::Change {
            wifi: None,
            wired: Some(fixed_wired()),
        };
        assert_eq!(network_apply(&p, &change, false, &feed, |_| {}), Ok(()));
        assert!(home_bytes(&p).1.is_some());
    }

    /// Ruling 16 for a save that fails halfway: ~/wifi.yaml is written,
    /// then ~/ethernet.yaml cannot be, so ~/wifi.yaml goes back as it was.
    #[test]
    fn a_save_that_fails_halfway_puts_the_home_back() {
        let d = dir("net-save-halfway");
        let feed = Arc::new(Feed::default());
        let marker = d.join("called");
        let tool = format!("touch {}; {FAILS}", marker.display());
        let p = net_home(&d, &tool, None);
        // A directory where the wired write puts its temporary file.
        std::fs::create_dir_all(p.homes.home.join("ethernet.yaml.tmp/x")).unwrap();
        let before = home_bytes(&p);
        let change = netfiles::Change {
            wifi: Some(Vec::new()),
            wired: Some(fixed_wired()),
        };
        let r = network_apply(&p, &change, false, &feed, |_| {}).unwrap_err();
        assert!(!r.home_changed, "{r:?}");
        assert!(!r.reason.is_empty());
        assert_eq!(home_bytes(&p), before, "~/wifi.yaml is put back");
        assert!(!marker.exists(), "the root tool ran after a failed save");
    }

    #[test]
    fn a_home_that_cannot_be_put_back_says_so() {
        let d = dir("net-no-put-back");
        let feed = Arc::new(Feed::default());
        // The tool fails and leaves a directory where ~/ethernet.yaml was.
        let home = d.join("home");
        let tool = format!(
            r#"rm -f {0}/ethernet.yaml; mkdir {0}/ethernet.yaml; echo '{{"result":"failed","reason":"x"}}'; exit 1"#,
            home.display()
        );
        let p = net_home(&d, &tool, None);
        let change = netfiles::Change {
            wifi: None,
            wired: Some(fixed_wired()),
        };
        let r = network_apply(&p, &change, false, &feed, |_| {}).unwrap_err();
        assert!(r.home_changed);
        assert!(
            r.reason.starts_with("x; ~/ethernet.yaml could not be put back: "),
            "{}",
            r.reason
        );
    }

    #[test]
    fn putting_back_never_writes_over_a_link_or_a_hand_file() {
        let d = dir("net-put-back-refused");
        let feed = Arc::new(Feed::default());
        let home = d.join("home");
        let target = d.join("elsewhere.yaml");
        let change = netfiles::Change {
            wifi: Some(Vec::new()),
            wired: None,
        };
        // (a) A link appears at ~/wifi.yaml during the apply.
        let tool = format!(
            r#"rm -f {0}/wifi.yaml; ln -s {1} {0}/wifi.yaml; echo '{{"result":"failed","reason":"x"}}'; exit 1"#,
            home.display(),
            target.display()
        );
        let p = net_home(&d, &tool, None);
        std::fs::write(&target, "elsewhere: 1\n").unwrap();
        let r = network_apply(&p, &change, false, &feed, |_| {}).unwrap_err();
        assert!(r.home_changed);
        assert!(
            r.reason.starts_with("x; ~/wifi.yaml could not be put back: "),
            "{}",
            r.reason
        );
        let link = home.join("wifi.yaml");
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
        assert_eq!(std::fs::read(&target).unwrap(), b"elsewhere: 1\n");
        std::fs::remove_file(&link).unwrap();
        // (b) A hand file without the header appears.
        let tool = format!(
            r#"printf 'hand: 1\n' > {0}/wifi.yaml; echo '{{"result":"failed","reason":"x"}}'; exit 1"#,
            home.display()
        );
        let p = net_home(&d, &tool, None);
        let r = network_apply(&p, &change, false, &feed, |_| {}).unwrap_err();
        assert!(r.home_changed);
        assert!(
            r.reason.starts_with("x; ~/wifi.yaml could not be put back: "),
            "{}",
            r.reason
        );
        assert_eq!(std::fs::read(home.join("wifi.yaml")).unwrap(), b"hand: 1\n");
    }

    #[test]
    fn a_network_failure_says_why() {
        let d = dir("net-why");
        let feed = Arc::new(Feed::default());
        let why = |tool: &str| network_status(&net_paths(&d, "/bin/true", tool), &feed).unwrap_err();
        assert_eq!(
            why(r#"echo '{"result":"failed","reason":"bad usage"}'; exit 2"#),
            "bad usage"
        );
        assert_eq!(
            why("echo 'sudo: a password is required' >&2; exit 1"),
            "sudo: a password is required"
        );
        assert_eq!(why("exit 3"), "The network tool did not answer (exit 3).");
        // The tool's reason wins over stderr.
        assert_eq!(
            why(r#"echo noise >&2; echo '{"result":"failed","reason":"no adapter"}'; exit 1"#),
            "no adapter"
        );
    }

    #[test]
    fn network_status_and_scan_parse_the_tool_output() {
        let d = dir("net-status");
        let feed = Arc::new(Feed::default());
        let status = r#"echo '{"interfaces":[{"name":"eno1","kind":"wired","state":"no-cable"}]}'"#;
        let got = network_status(&net_paths(&d, "/bin/true", status), &feed).unwrap();
        assert_eq!(got.len(), 1);
        let scan =
            r#"echo '{"device":"w","networks":[{"ssid":"a","signal_dbm":-40,"secured":true,"supported":true}]}'"#;
        assert_eq!(network_scan(&net_paths(&d, "/bin/true", scan), &feed).unwrap().len(), 1);
        let failed = r#"echo '{"result":"failed","reason":"no adapter"}'; exit 1"#;
        assert_eq!(
            network_scan(&net_paths(&d, "/bin/true", failed), &feed),
            Err("no adapter".to_string())
        );
    }

    /// A layout home under `d`: no kanshi runs (an empty proc dir), and
    /// starting one runs `start` with `-c <config>` as `$1 $2`.
    fn kanshi_home(d: &Path, start: &str) -> Kanshi {
        fs::create_dir_all(d.join("proc")).unwrap();
        Kanshi {
            config: d.join("home/.config/kanshi/config"),
            marker: Some(d.join("run/thinfloe-layout-unconfirmed")),
            cmd: ["/bin/sh", "-c", start, "sh"].map(String::from).to_vec(),
            proc_dir: d.join("proc"),
        }
    }

    /// A start script that appends its arguments and what the three files
    /// hold at that moment to `<d>/started`.
    fn recording(d: &Path) -> String {
        let log = d.join("started");
        format!(
            "{{ echo \"args $*\"; echo \"marker $(cat {m} 2>/dev/null)\"; echo \"prev $(tail -n 1 {p} 2>/dev/null)\"; echo \"config $(tail -n 1 {c} 2>/dev/null)\"; }} >> {log}",
            m = d.join("run/thinfloe-layout-unconfirmed").display(),
            p = d.join("home/.config/kanshi/config.prev").display(),
            c = d.join("home/.config/kanshi/config").display(),
            log = log.display()
        )
    }

    fn old_layout() -> String {
        format!("{}\nold\n", crate::owned::HEADER)
    }

    fn read(p: &Path) -> Option<String> {
        fs::read_to_string(p).ok()
    }

    #[test]
    fn mon_apply_writes_prev_marker_and_config() {
        let d = dir("mon-apply");
        let k = kanshi_home(&d, &recording(&d));
        fs::create_dir_all(k.config.parent().unwrap()).unwrap();
        fs::write(&k.config, old_layout()).unwrap();
        assert_eq!(mon_apply(&k, "\nnew\n", None), Ok(()));
        assert_eq!(read(&k.config), Some(format!("{}\n\nnew\n", crate::owned::HEADER)));
        assert_eq!(read(&k.prev()), Some(old_layout()));
        assert_eq!(read(k.marker.as_ref().unwrap()).as_deref(), Some("prev\n"));
        // kanshi was started on the config, with marker and prev in place.
        let want = format!("args -c {}\nmarker prev\nprev old\nconfig new\n", k.config.display());
        assert_eq!(read(&d.join("started")), Some(want));

        // No file before: the layout on screen is kept instead, over a
        // stale prev, and the marker still says prev.
        let d = dir("mon-apply-none");
        let k = kanshi_home(&d, &recording(&d));
        fs::create_dir_all(k.config.parent().unwrap()).unwrap();
        fs::write(k.prev(), "stale").unwrap();
        assert_eq!(mon_apply(&k, "\nnew\n", Some("\nonscreen\n")), Ok(()));
        assert_eq!(read(k.marker.as_ref().unwrap()).as_deref(), Some("prev\n"));
        assert_eq!(read(&k.prev()), Some(format!("{}\n\nonscreen\n", crate::owned::HEADER)));
        assert!(
            read(&d.join("started"))
                .unwrap()
                .contains("marker prev\nprev onscreen\nconfig new\n")
        );

        // Neither a file nor the layout on screen: nothing is written.
        let d = dir("mon-apply-unknown");
        let k = kanshi_home(&d, &recording(&d));
        assert!(mon_apply(&k, "\nnew\n", None).is_err());
        assert!(!k.config.exists() && !k.prev().exists() && !k.marker.as_ref().unwrap().exists());
        assert!(!d.join("started").exists());
    }

    #[test]
    fn mon_apply_tells_a_running_kanshi_and_starts_none() {
        use std::os::unix::process::ExitStatusExt;
        let d = dir("mon-hup");
        let k = kanshi_home(&d, &recording(&d));
        // A child of this test stands in for kanshi in the fake proc dir.
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let entry = d.join("proc").join(child.id().to_string());
        fs::create_dir_all(&entry).unwrap();
        fs::write(entry.join("comm"), "kanshi\n").unwrap();
        assert_eq!(kanshi_pids(&k.proc_dir), vec![child.id() as i32]);
        assert_eq!(mon_apply(&k, "\nnew\n", Some("\nonscreen\n")), Ok(()));
        let t0 = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait().unwrap() {
                break Some(s);
            }
            if t0.elapsed() > Duration::from_secs(3) {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(status.and_then(|s| s.signal()), Some(libc::SIGHUP));
        assert!(!d.join("started").exists(), "a second kanshi was started");
    }

    #[test]
    fn kanshi_pids_match_the_name_only() {
        let d = dir("mon-pids");
        let p = d.join("proc");
        for (pid, comm) in [
            ("4242", "kanshi\n"),
            ("4243", "kanshi-x\n"),
            ("4244", "bash\n"),
            ("1", "kanshi\n"),
            ("self", "kanshi\n"),
        ] {
            fs::create_dir_all(p.join(pid)).unwrap();
            fs::write(p.join(pid).join("comm"), comm).unwrap();
        }
        fs::create_dir_all(p.join("4245")).unwrap();
        assert_eq!(kanshi_pids(&p), vec![4242]);
        assert_eq!(kanshi_pids(&d.join("missing")), Vec::<i32>::new());
    }

    #[test]
    fn mon_apply_failure_puts_the_old_file_back() {
        let d = dir("mon-fail");
        let k = kanshi_home(&d, "exit 3");
        fs::create_dir_all(k.config.parent().unwrap()).unwrap();
        fs::write(&k.config, old_layout()).unwrap();
        let e = mon_apply(&k, "\nnew\n", None).unwrap_err();
        assert!(e.contains("stopped at once"), "{e}");
        assert_eq!(read(&k.config), Some(old_layout()));
        assert!(!k.prev().exists());
        assert!(!k.marker.as_ref().unwrap().exists());

        // No file before: none after.
        fs::remove_file(&k.config).unwrap();
        assert!(mon_apply(&k, "\nnew\n", Some("\nonscreen\n")).is_err());
        assert!(!k.prev().exists());
        assert!(!k.config.exists());
        assert!(!k.marker.as_ref().unwrap().exists());

        // A file written by hand is not touched, and kanshi not started.
        let k = kanshi_home(&d, &recording(&d));
        fs::write(&k.config, "profile {\n\toutput * enable\n}\n").unwrap();
        let e = mon_apply(&k, "\nnew\n", None).unwrap_err();
        assert!(e.contains("written by hand"), "{e}");
        assert_eq!(read(&k.config).as_deref(), Some("profile {\n\toutput * enable\n}\n"));
        assert!(!k.prev().exists());
        assert!(!k.marker.as_ref().unwrap().exists());
        assert!(!d.join("started").exists());
    }

    #[test]
    fn mon_apply_without_a_runtime_dir_changes_nothing() {
        let d = dir("mon-no-xdg");
        let mut k = kanshi_home(&d, &recording(&d));
        k.marker = None;
        fs::create_dir_all(k.config.parent().unwrap()).unwrap();
        fs::write(&k.config, old_layout()).unwrap();
        let e = mon_apply(&k, "\nnew\n", None).unwrap_err();
        assert!(e.contains("XDG_RUNTIME_DIR"), "{e}");
        assert_eq!(read(&k.config), Some(old_layout()));
        assert!(!k.prev().exists());
        assert!(!d.join("started").exists());
        // Nor without a file.
        fs::remove_file(&k.config).unwrap();
        assert!(mon_apply(&k, "\nnew\n", Some("\nonscreen\n")).is_err());
        assert!(!k.config.exists() && !k.prev().exists());
    }

    #[test]
    fn mon_revert_restores_prev() {
        let d = dir("mon-revert");
        let k = kanshi_home(&d, &recording(&d));
        fs::create_dir_all(k.config.parent().unwrap()).unwrap();
        fs::write(&k.config, old_layout()).unwrap();
        mon_apply(&k, "\nnew\n", None).unwrap();
        assert_eq!(mon_revert(&k), Ok(()));
        assert_eq!(read(&k.config), Some(old_layout()));
        assert!(!k.prev().exists());
        assert!(!k.marker.as_ref().unwrap().exists());
        // kanshi read the old file again.
        let log = read(&d.join("started")).unwrap();
        assert!(log.ends_with("marker \nprev \nconfig old\n"), "{log}");

        // No file before: the layout on screen comes back, and kanshi
        // reads it.
        fs::remove_file(&k.config).unwrap();
        mon_apply(&k, "\nnew\n", Some("\nonscreen\n")).unwrap();
        assert_eq!(mon_revert(&k), Ok(()));
        assert_eq!(read(&k.config), Some(format!("{}\n\nonscreen\n", crate::owned::HEADER)));
        assert!(!k.prev().exists());
        assert!(!k.marker.as_ref().unwrap().exists());
        let log = read(&d.join("started")).unwrap();
        assert!(log.ends_with("marker \nprev \nconfig onscreen\n"), "{log}");

        // A file written by hand meanwhile is kept, and so is the marker.
        mon_apply(&k, "\nnew\n", None).unwrap();
        fs::write(&k.config, "by hand\n").unwrap();
        assert!(mon_revert(&k).is_err());
        assert_eq!(read(&k.config).as_deref(), Some("by hand\n"));
        assert!(k.marker.as_ref().unwrap().exists());
    }

    #[test]
    fn a_first_apply_then_revert_leaves_the_layout_on_screen() {
        use crate::heads::{Head, Mode, Rotation};
        use crate::monitors::{MonEvent, MonRequest, MonTab};
        let head = |name: &str, x: i32| Head {
            name: name.into(),
            make: "Dell".into(),
            model: "U".into(),
            serial: String::new(),
            phys_mm: (0, 0),
            enabled: true,
            modes: vec![Mode {
                w: 1920,
                h: 1080,
                refresh_mhz: 60_000,
                preferred: true,
            }],
            current: Some(0),
            pos: (x, 0),
            rotation: Rotation::Normal,
        };
        let heads = vec![head("DP-1", 0), head("DP-2", 1920)];
        let d = dir("mon-first");
        let k = kanshi_home(&d, &recording(&d));
        let now = Instant::now();
        let mut t = MonTab::default();
        t.heads_changed(heads.clone());
        let (text, owner) = mon_load(&k);
        t.loaded(text, owner);
        t.event(MonEvent::Select("DP-2".into()), now);
        t.event(MonEvent::Rotation(Rotation::R90), now);
        let Some(MonRequest::Apply { text, prev, .. }) = t.event(MonEvent::Apply, now) else {
            panic!("no Apply")
        };
        assert_eq!(mon_apply(&k, &text, prev.as_deref()), Ok(()));
        assert_eq!(t.event(MonEvent::GoBack, now), None, "the job has not ended yet");
        assert_eq!(t.applied(Ok(())), Some(MonRequest::Revert));
        assert_eq!(mon_revert(&k), Ok(()));
        let want = format!(
            "{}\n\n{}",
            crate::owned::HEADER,
            crate::kanshi::render(&[crate::kanshi::from_heads(&heads)])
        );
        assert_eq!(read(&k.config), Some(want));
        assert_eq!(mon_load(&k).1, crate::owned::Owner::Menu);
    }

    #[test]
    fn mon_keep_removes_marker_and_prev() {
        let d = dir("mon-keep");
        let k = kanshi_home(&d, &recording(&d));
        fs::create_dir_all(k.config.parent().unwrap()).unwrap();
        fs::write(&k.config, old_layout()).unwrap();
        mon_apply(&k, "\nnew\n", None).unwrap();
        assert_eq!(mon_keep(&k), Ok(()));
        assert_eq!(read(&k.config), Some(format!("{}\n\nnew\n", crate::owned::HEADER)));
        assert!(!k.prev().exists());
        assert!(!k.marker.as_ref().unwrap().exists());
    }

    #[test]
    fn a_marker_that_cannot_go_fails_the_keep() {
        let d = dir("mon-keep-fail");
        let k = kanshi_home(&d, &recording(&d));
        fs::create_dir_all(k.config.parent().unwrap()).unwrap();
        fs::write(&k.config, old_layout()).unwrap();
        mon_apply(&k, "\nnew\n", None).unwrap();
        block(k.marker.as_ref().unwrap());
        let e = mon_keep(&k).unwrap_err();
        assert!(e.contains("undo marker"), "{e}");
        // prev stays with the marker, so the next session's undo works.
        assert_eq!(read(&k.prev()), Some(old_layout()));
    }

    /// Put a directory with a file in it where `path` is: removing it as a
    /// file fails, and so does writing it.
    fn block(path: &Path) {
        let _ = fs::remove_file(path);
        fs::create_dir_all(path).unwrap();
        fs::write(path.join("x"), "x").unwrap();
    }

    #[test]
    fn mon_revert_removes_the_marker_before_prev() {
        // thinfloe-session reads marker `prev` without config.prev as
        // "remove the config": the marker must never outlive prev.
        let d = dir("mon-revert-order");
        let k = kanshi_home(&d, &recording(&d));
        fs::create_dir_all(k.config.parent().unwrap()).unwrap();
        fs::write(&k.config, old_layout()).unwrap();
        mon_apply(&k, "\nnew\n", None).unwrap();
        // The marker cannot be removed: the revert stops there.
        block(k.marker.as_ref().unwrap());
        assert!(mon_revert(&k).is_err());
        assert_eq!(read(&k.config), Some(old_layout()));
        assert_eq!(read(&k.prev()), Some(old_layout()), "prev went before the marker");
    }

    #[test]
    fn a_failed_apply_removes_the_marker_before_prev() {
        // The fake kanshi blocks the marker, then fails: the undo cannot
        // remove the marker, so prev must stay for the session's undo.
        let d = dir("mon-undo-order");
        let m = d.join("run/thinfloe-layout-unconfirmed");
        let start = format!("rm -f {m}; mkdir -p {m}; touch {m}/x; exit 3", m = m.display());
        let k = kanshi_home(&d, &start);
        fs::create_dir_all(k.config.parent().unwrap()).unwrap();
        fs::write(&k.config, old_layout()).unwrap();
        let e = mon_apply(&k, "\nnew\n", None).unwrap_err();
        assert_eq!(read(&k.prev()), Some(old_layout()), "prev went before the marker");
        assert_eq!(read(&k.config), Some(old_layout()));
        assert!(e.contains("undo marker"), "{e}");
    }

    #[test]
    fn mon_apply_refuses_while_a_layout_is_unconfirmed() {
        let d = dir("mon-unconfirmed");
        let k = kanshi_home(&d, &recording(&d));
        fs::create_dir_all(k.config.parent().unwrap()).unwrap();
        fs::write(
            &k.config,
            "# Written by the ThinFloe menu. Edits here are replaced.\nnew\n",
        )
        .unwrap();
        fs::write(k.prev(), old_layout()).unwrap();
        fs::create_dir_all(k.marker.as_ref().unwrap().parent().unwrap()).unwrap();
        fs::write(k.marker.as_ref().unwrap(), "prev\n").unwrap();
        let e = mon_apply(&k, "\nnewer\n", None).unwrap_err();
        assert!(e.contains("not confirmed"), "{e}");
        assert_eq!(read(&k.prev()), Some(old_layout()), "the real old layout is kept");
        assert!(read(&k.config).unwrap().ends_with("\nnew\n"));
        assert!(!d.join("started").exists());
    }

    #[test]
    fn a_failed_marker_write_leaves_the_old_file() {
        let d = dir("mon-marker-fail");
        let k = kanshi_home(&d, &recording(&d));
        fs::create_dir_all(k.config.parent().unwrap()).unwrap();
        fs::write(&k.config, old_layout()).unwrap();
        // The marker's directory is a file.
        fs::write(d.join("run"), "not a dir").unwrap();
        assert!(mon_apply(&k, "\nnew\n", None).unwrap_err().contains("Could not write"));
        assert_eq!(read(&k.config), Some(old_layout()));
        assert!(!k.prev().exists());
        assert!(!d.join("started").exists());
        // Without a file, the kept layout on screen goes again.
        fs::remove_file(&k.config).unwrap();
        assert!(mon_apply(&k, "\nnew\n", Some("\nonscreen\n")).is_err());
        assert!(!k.config.exists() && !k.prev().exists());
    }

    #[test]
    fn a_failed_config_write_is_rolled_back() {
        let d = dir("mon-write-fail");
        let k = kanshi_home(&d, &recording(&d));
        fs::create_dir_all(k.config.parent().unwrap()).unwrap();
        let mut tmp = k.config.as_os_str().to_owned();
        tmp.push(".tmp");
        // owned::write goes through config.tmp, which cannot be made.
        block(Path::new(&tmp));
        let e = mon_apply(&k, "\nnew\n", Some("\nonscreen\n")).unwrap_err();
        assert!(e.contains("Could not write"), "{e}");
        assert!(!k.config.exists());
        assert!(!k.prev().exists());
        assert!(!k.marker.as_ref().unwrap().exists());
        assert!(!d.join("started").exists());
        // With an old file the put-back fails the same way: marker and
        // prev stay, so the next session undoes it.
        fs::write(&k.config, old_layout()).unwrap();
        let e = mon_apply(&k, "\nnew\n", None).unwrap_err();
        assert!(e.contains("could not be put back"), "{e}");
        assert_eq!(read(&k.config), Some(old_layout()));
        assert_eq!(read(&k.prev()), Some(old_layout()));
        assert_eq!(read(k.marker.as_ref().unwrap()).as_deref(), Some("prev\n"));
    }

    #[test]
    fn mon_load_and_take_over() {
        let d = dir("mon-load");
        let k = kanshi_home(&d, "exit 0");
        assert_eq!(mon_load(&k), (Ok(None), crate::owned::Owner::Missing));
        fs::create_dir_all(k.config.parent().unwrap()).unwrap();
        fs::write(&k.config, old_layout()).unwrap();
        assert_eq!(mon_load(&k), (Ok(Some(old_layout())), crate::owned::Owner::Menu));
        fs::write(&k.config, "by hand\n").unwrap();
        assert_eq!(
            mon_load(&k),
            (Ok(Some("by hand\n".into())), crate::owned::Owner::ByHand)
        );
        assert_eq!(mon_take_over(&k), Ok(()));
        assert!(!k.config.exists());
        assert_eq!(read(&crate::owned::aside(&k.config)).as_deref(), Some("by hand\n"));
        // A link is not read through.
        std::os::unix::fs::symlink(d.join("elsewhere"), &k.config).unwrap();
        let (r, owner) = mon_load(&k);
        assert!(r.unwrap_err().starts_with("Could not read"));
        assert_eq!(owner, crate::owned::Owner::ByHand);
    }
}
