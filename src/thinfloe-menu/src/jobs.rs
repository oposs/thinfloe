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
    /// The saved monitor profiles, `None` when there are none, and where
    /// they come from.
    MonLoaded(
        Result<Option<Vec<crate::kanshi::Profile>>, String>,
        crate::monitors::MonSource,
    ),
    MonApplied(Result<(), String>),
    /// An apply of the Keyboard tab ended; `kb` is what it applied.
    KeyboardApplied {
        kb: crate::config::Keyboard,
        result: Result<crate::keyboard::KbOutcome, String>,
    },
    MonKept(Result<(), String>),
    MonReverted(Result<(), String>),
    MonTakenOver(Result<(), String>),
    /// The status layout file was written or removed.
    StatusSaved(Result<(), String>),
    ReportDismissed(Result<(), String>),
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
    /// The settings store, opened once for the whole menu.
    pub config: crate::store::Shared,
    /// Which layout this boot's home has.
    pub layout: crate::migrate::HomeLayout,
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

/// What the Network tab shows: the `[network]` table of the settings.
pub fn net_load(paths: &Paths) -> (netfiles::WifiFile, netfiles::WiredFile) {
    netfiles::view(&lock(&paths.config).cfg.network)
}

/// The settings as they are and the inputs rendering needs, copied so the
/// store is not locked while netplan checks a candidate.
fn net_snapshot(paths: &Paths) -> (crate::config::ThinfloeConfig, crate::generated::Inputs) {
    let s = lock(&paths.config);
    (s.cfg.clone(), s.inputs.clone())
}

/// *Let the menu manage it*: the kept text `which` becomes the menu's own
/// settings, written through the store like any other change. A store that
/// refuses writes refuses before netplan is asked.
pub fn net_take_over(paths: &Paths, which: &str) -> Result<(), String> {
    lock(&paths.config).refuse_read_only()?;
    let (cfg, inputs) = net_snapshot(paths);
    let net = netfiles::take_over(&cfg.network, which, &paths.homes, &inputs)?;
    lock(&paths.config)
        .update(crate::generated::Table::Network, |c| c.network = net)
        .map(|_| ())
}

/// Plan `change`, stage the generated network files, then (only when that
/// worked) let the root tool `apply` them, in a process group of its own
/// that `on_spawn` gets. Success makes the candidate `[network]`; a failure
/// puts back exactly the generated network files the stage replaced. The
/// candidate is built on the settings as they are at that moment, so a
/// keyboard or screen change made while the tool runs is kept.
/// `netboot` is the caller's to know.
pub fn network_apply(
    paths: &Paths,
    change: &netfiles::Change,
    netboot: bool,
    feed: &Arc<Feed>,
    on_spawn: impl FnOnce(i32),
) -> Result<(), ApplyFailed> {
    lock(&paths.config).refuse_read_only().map_err(ApplyFailed::new)?;
    let (cfg, inputs) = net_snapshot(paths);
    let net = netfiles::plan(&cfg.network, change, netboot, &paths.homes, &inputs).map_err(ApplyFailed::new)?;
    let staged = {
        let s = lock(&paths.config);
        let mut candidate = s.cfg.clone();
        candidate.network = net.clone();
        s.stage(crate::generated::Table::Network, &candidate)
            .map_err(|e| ApplyFailed {
                home_changed: e.restore.is_some(),
                reason: e.to_string(),
            })?
    };
    let result = network_json(paths, &["apply"], NETWORK_APPLY_TIMEOUT, feed, on_spawn)
        .and_then(|text| nettab::parse_result(&text));
    match result {
        Ok(()) => {
            let mut s = lock(&paths.config);
            let mut candidate = s.cfg.clone();
            candidate.network = net;
            // The tool applied the staged files, so they stay. The
            // settings that do not follow are told, and the home is saved
            // with the files as they are.
            s.commit(candidate).map_err(|e| ApplyFailed {
                reason: format!("The network was applied, but the settings could not be saved: {e}"),
                home_changed: true,
            })
        }
        Err(reason) => {
            let mut failed = ApplyFailed::new(reason);
            if let Err(e) = lock(&paths.config).unstage(staged) {
                failed.reason = format!("{}; {e}", failed.reason);
                failed.home_changed = true;
            }
            Err(failed)
        }
    }
}

/// The monitor layout: kanshi's config, the files that make an Apply
/// undoable, and how kanshi is found and started.
#[derive(Debug, Clone)]
pub struct Kanshi {
    /// `generated/kanshi` in the config directory.
    pub config: PathBuf,
    /// `generated/layout-unconfirmed`; thinfloe-session undoes the layout
    /// when it finds it. It sits beside the config so a home save taken
    /// while the layout is unconfirmed carries it too.
    pub marker: PathBuf,
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
        let mut k = Kanshi::from_dir(&crate::config::ConfigDir::from_env());
        if let Ok(v) = std::env::var("TF_KANSHI")
            && !v.trim().is_empty()
        {
            k.cmd = vec![v];
        }
        k
    }

    /// The layout files under `d`; kanshi is the one on the path.
    pub fn from_dir(d: &crate::config::ConfigDir) -> Kanshi {
        Kanshi {
            config: d.generated_file("kanshi"),
            marker: d.generated_file("layout-unconfirmed"),
            cmd: vec!["kanshi".to_string()],
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
    write_via(path, ".tmp", bytes)
}

/// Write `bytes` to `path` through `<path><suffix>` and a rename, making the
/// directory when it is missing. A stray file or link at the temporary name
/// goes first; `create_new` never follows one that appears in between.
fn write_via(path: &Path, suffix: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(suffix);
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

/// Keep `text`, the status layout as `layout::to_text` writes it, in the
/// `[status]` table of `config.toml`, or with `None` go back to the default
/// list. A refused update (read-only store, failed write) is the error.
pub fn status_save(store: &crate::store::Shared, text: Option<&str>) -> Result<(), String> {
    let lines = text.map(|t| {
        t.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from)
            .collect::<Vec<_>>()
    });
    let mut store = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    store
        .update(crate::generated::Table::Status, |c| c.status.lines = lines)
        .map(|_| ())
}

/// Remove the migration report from the `[notice]` table.
pub fn dismiss_report(store: &crate::store::Shared) -> Result<(), String> {
    let mut store = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    store
        .update(crate::generated::Table::Notice, |c| c.notice.migration = None)
        .map(|_| ())
}

fn remove_if_there(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The saved monitor arrangement from the settings, and where it comes
/// from. A kept hand-written file has no profiles; settings that do not
/// convert are an error and count as kept, so nothing overwrites them.
pub fn mon_load(
    store: &crate::store::Shared,
) -> (
    Result<Option<Vec<crate::kanshi::Profile>>, String>,
    crate::monitors::MonSource,
) {
    use crate::monitors::MonSource;
    let store = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let monitors = &store.cfg.monitors;
    if monitors.kept.is_some() {
        return (Ok(None), MonSource::Kept);
    }
    match monitors.to_profiles() {
        Err(e) => (
            Err(format!("The saved monitor arrangement could not be read: {e}")),
            MonSource::Kept,
        ),
        Ok(p) if p.is_empty() => (Ok(None), MonSource::Missing),
        Ok(p) => (Ok(Some(p)), MonSource::Menu),
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

fn lock(store: &crate::store::Shared) -> std::sync::MutexGuard<'_, crate::store::ConfigStore> {
    store.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Write the new layout and let kanshi apply it. First the old
/// `generated/kanshi` goes to `kanshi.prev`, and the marker `prev` says so,
/// so a crash at any later point is undone by the next session. Without a
/// file, `prev_body` (the body of a file for the layout on screen) is kept
/// instead: a revert needs a layout to go back to, since kanshi keeps the
/// last one it set. The menu never writes the marker `none`. `[monitors]`
/// stays as it is until `mon_keep`. On a failure (the store refusing
/// included) the old file is put back and both marker and prev go.
pub fn mon_apply(
    store: &crate::store::Shared,
    k: &Kanshi,
    profiles: &[crate::kanshi::Profile],
    prev_body: Option<&str>,
) -> Result<(), String> {
    let rel = crate::atomic::home_relative(&k.config);
    let mut candidate = {
        let s = lock(store);
        // Before anything is written: kanshi.prev, the marker and generated/
        // are all the Apply's own doing.
        s.refuse_read_only()?;
        if s.cfg.monitors.kept.is_some() {
            return Err("The monitor arrangement is a kanshi file written by hand; nothing was changed.".to_string());
        }
        s.cfg.clone()
    };
    candidate.monitors = crate::config::MonitorsCfg::from_profiles(profiles);
    let old = crate::atomic::read(&k.config, crate::generated::MAX_BYTES)
        .map_err(|e| format!("Could not read {rel}: {e}"))?;
    let marker = &k.marker;
    let marker_rel = crate::atomic::home_relative(marker);
    // An earlier Apply whose undo failed: its kanshi.prev is the real old
    // layout and must not be replaced. The next session undoes it. A marker
    // that cannot be checked may be one, too.
    match std::fs::symlink_metadata(marker) {
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Ok(_) => {
            return Err("An earlier monitor arrangement is not confirmed yet; nothing was changed.".to_string());
        }
        Err(e) => {
            return Err(format!(
                "The undo marker {marker_rel} could not be checked: {e}; nothing was changed."
            ));
        }
    }
    let prev = k.prev();
    let kept = match (&old, prev_body) {
        (Some(b), _) => write_new(&prev, b),
        (None, Some(body)) => write_new(&prev, format!("{}\n{body}", crate::generated::HEADER).as_bytes()),
        (None, None) => return Err("The layout on screen is not known; nothing was changed.".to_string()),
    };
    kept.map_err(|e| {
        format!(
            "Could not keep the old layout in {}: {e}",
            crate::atomic::home_relative(&prev)
        )
    })?;
    if let Err(e) = write_new(marker, b"prev\n") {
        // No marker is there: it did not exist above, and the failed write
        // left none behind (the rename is its last step).
        let _ = remove_if_there(&prev);
        return Err(format!("Could not write {marker_rel}: {e}"));
    }
    let staged = match lock(store).stage(crate::generated::Table::Monitors, &candidate) {
        Ok(s) => s,
        Err(e) => return Err(mon_undo(store, k, &old, e.to_string(), None)),
    };
    kanshi_reload(k).map_err(|reason| mon_undo(store, k, &old, reason, Some(staged)))
}

/// Put `generated/kanshi` back to `old` (`None`: no file). A file that
/// already holds `old` is left alone.
fn restore_kanshi(k: &Kanshi, old: &Option<Vec<u8>>) -> Result<(), String> {
    let now = crate::atomic::read(&k.config, crate::generated::MAX_BYTES);
    if matches!(&now, Ok(n) if n == old) {
        return Ok(());
    }
    match old {
        Some(b) => crate::atomic::write(&k.config, b, crate::generated::mode_of("kanshi")),
        None => crate::atomic::remove(&k.config),
    }
    .map_err(|e| format!("{} could not be restored: {e}", crate::atomic::home_relative(&k.config)))
}

/// Put `old` back after a failed Apply: through `unstage` when the store
/// staged the file, else by hand. Only when that worked do the marker and
/// `kanshi.prev` go: else the next session undoes it.
fn mon_undo(
    store: &crate::store::Shared,
    k: &Kanshi,
    old: &Option<Vec<u8>>,
    reason: String,
    staged: Option<crate::store::Staged>,
) -> String {
    let put_back = match staged {
        Some(s) => lock(store).unstage(s),
        None => restore_kanshi(k, old),
    };
    if let Err(e) = put_back {
        return format!("{reason}; {e}");
    }
    // The marker first, and prev only once it is gone; see `mon_revert`.
    if let Err(e) = remove_marker(k) {
        return format!("{reason}; {e}");
    }
    let _ = remove_if_there(&k.prev());
    reason
}

/// The new layout is confirmed: `profiles` become `[monitors]` and nothing
/// is left to undo. A marker that stays means the next session goes back:
/// that is an error. The marker goes first; see `mon_revert`. When the
/// store refuses the settings, `generated/kanshi` goes back to what they
/// say, so the next start finds both alike; the person is told the layout
/// goes back at the next login.
pub fn mon_keep(store: &crate::store::Shared, k: &Kanshi, profiles: Vec<crate::kanshi::Profile>) -> Result<(), String> {
    remove_marker(k)?;
    let committed = {
        let mut s = lock(store);
        let mut candidate = s.cfg.clone();
        candidate.monitors = crate::config::MonitorsCfg::from_profiles(&profiles);
        s.commit(candidate)
    };
    if committed.is_err() {
        let s = lock(store);
        if let Ok(body) = crate::generated::render(&s.cfg, "kanshi", &s.inputs) {
            let _ = crate::generated::write_if_changed(&s.dir.generated(), "kanshi", body.as_deref());
        }
    }
    let _ = remove_if_there(&k.prev());
    committed
}

fn remove_marker(k: &Kanshi) -> Result<(), String> {
    let m = &k.marker;
    remove_if_there(m).map_err(|e| {
        format!(
            "The undo marker {} could not be removed: {e}",
            crate::atomic::home_relative(m)
        )
    })
}

/// Go back to the layout before the Apply: `kanshi.prev` goes back over
/// `generated/kanshi`, and kanshi reads it again. Every Apply keeps a
/// `kanshi.prev`; without one nothing is changed and the marker stays, so
/// the next session undoes the Apply. The marker also stays when the file
/// could not be put back, so the next session tries. `[monitors]` was not
/// touched by the Apply; after a first Apply `generated/kanshi` holds the
/// layout that was on screen until the next start regenerates it.
pub fn mon_revert(k: &Kanshi) -> Result<(), String> {
    let prev = k.prev();
    let prev_rel = crate::atomic::home_relative(&prev);
    let old = crate::atomic::read(&prev, crate::generated::MAX_BYTES)
        .map_err(|e| format!("Could not read {prev_rel}: {e}"))?
        .ok_or_else(|| format!("{prev_rel} is missing"))?;
    restore_kanshi(k, &Some(old))?;
    // The marker before prev: thinfloe-session reads marker `prev` without
    // kanshi.prev as "remove the config", and would delete the layout just
    // put back if the menu died between the two.
    remove_marker(k)?;
    let _ = remove_if_there(&prev);
    kanshi_reload(k)
}

/// *Let the menu manage it*: the kept hand-written file is replaced by the
/// layout on screen, which becomes the saved arrangement, and kanshi reads
/// the new file. A refused update reloads nothing.
pub fn mon_take_over(store: &crate::store::Shared, k: &Kanshi, heads: &[crate::heads::Head]) -> Result<(), String> {
    let profiles = if heads.is_empty() {
        Vec::new()
    } else {
        vec![crate::kanshi::from_heads(heads)]
    };
    lock(store)
        .update(crate::generated::Table::Monitors, |c| {
            c.monitors = crate::config::MonitorsCfg::from_profiles(&profiles);
        })
        .map(|_| ())?;
    kanshi_reload(k)
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
            config: crate::store::test_shared(&d.join("cfg")),
            layout: crate::migrate::HomeLayout::New,
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
            config: crate::store::test_shared(&d.join("cfg")),
            layout: crate::migrate::HomeLayout::New,
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
            config: crate::store::test_shared(&d.join("cfg")),
            layout: crate::migrate::HomeLayout::New,
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
            config: crate::store::test_shared(&d.join("cfg")),
            layout: crate::migrate::HomeLayout::New,
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
            config: crate::store::test_shared(&d.join("cfg")),
            layout: crate::migrate::HomeLayout::New,
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

    use crate::config::{NetCfg, NetworkCfg};
    use crate::generated::{Inputs, Table};
    use crate::store::{OpenMode, Shared};

    const SHIPPED_WIRED: &str = include_str!("../../../overlay/etc/netplan/ethernet.yaml");

    fn net_inputs() -> Inputs {
        Inputs {
            default_wired: SHIPPED_WIRED.into(),
            netboot: false,
        }
    }

    /// A session store under `d/cfg` holding `net`, its generated files
    /// written as the menu writes them.
    fn net_store(d: &Path, net: NetworkCfg) -> Shared {
        let dir = crate::config::ConfigDir { root: d.join("cfg") };
        let s = Arc::new(std::sync::Mutex::new(
            crate::store::open(dir, net_inputs(), OpenMode::Session, 0).store,
        ));
        lock(&s).update(Table::Network, |c| c.network = net).unwrap();
        s
    }

    fn home_net() -> NetworkCfg {
        NetworkCfg {
            wifi_device: Some("wlan0".into()),
            networks: vec![NetCfg {
                ssid: "home".into(),
                hidden: false,
                password: Some("secret123".into()),
            }],
            ..NetworkCfg::default()
        }
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
            config: net_store(d, NetworkCfg::default()),
            layout: crate::migrate::HomeLayout::New,
        };
        p.homes = netfiles::Homes {
            home: d.join("home"),
            sys_net: d.join("sys"),
            netplan: netplan.to_string(),
            default_wired: d.join("default-ethernet.yaml"),
        };
        std::fs::write(&p.homes.default_wired, SHIPPED_WIRED).unwrap();
        std::fs::create_dir_all(d.join("home")).unwrap();
        std::fs::create_dir_all(d.join("sys/eno1")).unwrap();
        std::fs::create_dir_all(d.join("sys/wlan0/wireless")).unwrap();
        p
    }

    fn gen_file(p: &Paths, name: &str) -> Option<Vec<u8>> {
        std::fs::read(lock(&p.config).dir.generated_file(name)).ok()
    }

    fn generated_bytes(p: &Paths) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        (gen_file(p, "wifi.yaml"), gen_file(p, "ethernet.yaml"))
    }

    fn config_bytes(p: &Paths) -> Vec<u8> {
        std::fs::read(lock(&p.config).dir.config_toml()).unwrap()
    }

    fn one_net(ssid: &str) -> Vec<netfiles::Net> {
        vec![netfiles::Net {
            ssid: ssid.into(),
            hidden: false,
            password: Some("secret123".into()),
        }]
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
        let mut p = net_paths(&d, "/bin/true", &tool);
        p.homes.default_wired = d.join("missing-default");
        let before = (generated_bytes(&p), config_bytes(&p));
        let change = netfiles::Change {
            wifi: None,
            wired: Some(netfiles::Wired::dhcp()),
        };
        let feed = Arc::new(Feed::default());
        let spawned = std::cell::Cell::new(false);
        let r = network_apply(&p, &change, false, &feed, |_| spawned.set(true));
        assert!(r.is_err(), "{r:?}");
        assert!(!marker.exists(), "the root tool ran after a failed save");
        assert!(!spawned.get());
        // netplan refusing the set fails the save as well.
        let p2 = net_paths(&d, "/bin/false", &tool);
        let change = netfiles::Change {
            wifi: Some(one_net("home")),
            wired: None,
        };
        assert!(network_apply(&p2, &change, false, &feed, |_| {}).is_err());
        assert!(!marker.exists());
        assert_eq!((generated_bytes(&p), config_bytes(&p)), before);
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
        let change = netfiles::Change {
            wifi: Some(one_net("home")),
            wired: None,
        };
        let r = network_apply(&p, &change, false, &feed, |g| pgid.set(g));
        assert_eq!(r, Ok(()));
        assert!(marker.exists());
        assert!(pgid.get() > 1);
        // The generated file is written, then [network] committed.
        let wifi = String::from_utf8(gen_file(&p, "wifi.yaml").unwrap()).unwrap();
        assert!(
            wifi.starts_with(crate::generated::HEADER) && wifi.contains("secret123"),
            "{wifi}"
        );
        let mode = std::fs::metadata(lock(&p.config).dir.generated_file("wifi.yaml"))
            .unwrap()
            .permissions();
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777, 0o600);
        assert_eq!(lock(&p.config).cfg.network.nets(), one_net("home"));
        let on_disk = crate::config::parse(&String::from_utf8(config_bytes(&p)).unwrap()).unwrap();
        assert_eq!(on_disk.network.nets(), one_net("home"));
        // A tool that fails, or does not answer.
        let tool = "echo '{\"result\":\"failed\",\"reason\":\"no link\"}'; exit 1";
        let p = net_paths(&d, "/bin/true", tool);
        assert_eq!(
            network_apply(&p, &netfiles::Change::default(), false, &feed, |_| {}),
            Err(ApplyFailed::new("no link".to_string()))
        );
        let p = net_paths(&d, "/bin/true", "exit 2");
        assert!(network_apply(&p, &netfiles::Change::default(), false, &feed, |_| {}).is_err());
    }

    /// `net_paths` with a shipped wired default and the settings `net`.
    fn net_home(d: &Path, tool: &str, net: NetworkCfg) -> Paths {
        let mut p = net_paths(d, "/bin/true", tool);
        p.config = net_store(d, net);
        p
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
    fn a_failed_network_apply_puts_the_generated_files_back() {
        let d = dir("net-put-back");
        let feed = Arc::new(Feed::default());
        // A changed WiFi file and a new wired file.
        let p = net_home(&d, FAILS, home_net());
        let before = (generated_bytes(&p), config_bytes(&p));
        let cfg_before = lock(&p.config).cfg.clone();
        let change = netfiles::Change {
            wifi: Some(one_net("cafe")),
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
        assert_eq!((generated_bytes(&p), config_bytes(&p)), before);
        assert_eq!(lock(&p.config).cfg, cfg_before, "[network] is unchanged");
        assert!(before.0.1.is_none(), "the new wired file is removed");
        // A wired file that existed comes back after DHCP removed it.
        let mut with_fixed = home_net();
        with_fixed.set_wired(&fixed_wired());
        let p = net_home(&d, FAILS, with_fixed);
        let before = generated_bytes(&p);
        assert!(before.1.is_some());
        let change = netfiles::Change {
            wifi: None,
            wired: Some(netfiles::Wired::dhcp()),
        };
        assert!(network_apply(&p, &change, false, &feed, |_| {}).is_err());
        assert_eq!(generated_bytes(&p), before);
        // The same change that works keeps the new files.
        let p = net_home(&d, r#"echo '{"result":"ok","reason":""}'"#, home_net());
        let change = netfiles::Change {
            wifi: None,
            wired: Some(fixed_wired()),
        };
        assert_eq!(network_apply(&p, &change, false, &feed, |_| {}), Ok(()));
        assert!(generated_bytes(&p).1.is_some());
    }

    /// A change of the DNS servers alone rewrites the WiFi file as well; a
    /// failed apply puts that one back too.
    #[test]
    fn a_failed_dns_change_puts_the_wifi_file_back() {
        let d = dir("net-put-back-dns");
        let feed = Arc::new(Feed::default());
        let p = net_home(&d, FAILS, home_net());
        let before = generated_bytes(&p);
        assert!(before.0.is_some());
        let change = netfiles::Change {
            wifi: None,
            wired: Some(netfiles::Wired::Dhcp {
                dns: vec!["1.1.1.1".parse().unwrap()],
            }),
        };
        assert!(network_apply(&p, &change, false, &feed, |_| {}).is_err());
        assert_eq!(generated_bytes(&p), before);
    }

    /// A save that fails halfway: the WiFi file is written, then the wired
    /// one cannot be, so the WiFi file goes back as it was.
    #[test]
    fn a_save_that_fails_halfway_puts_the_generated_files_back() {
        let d = dir("net-save-halfway");
        let feed = Arc::new(Feed::default());
        let marker = d.join("called");
        let tool = format!("touch {}; {FAILS}", marker.display());
        let p = net_home(&d, &tool, home_net());
        // A directory where the wired write puts its temporary file.
        let tmp = lock(&p.config).dir.generated().join("ethernet.yaml.tmp/x");
        std::fs::create_dir_all(tmp).unwrap();
        let before = generated_bytes(&p);
        let change = netfiles::Change {
            wifi: Some(one_net("cafe")),
            wired: Some(fixed_wired()),
        };
        let r = network_apply(&p, &change, false, &feed, |_| {}).unwrap_err();
        assert!(!r.home_changed, "{r:?}");
        assert!(r.reason.contains("ethernet.yaml"), "{r:?}");
        assert_eq!(generated_bytes(&p), before, "the WiFi file is put back");
        assert!(!marker.exists(), "the root tool ran after a failed save");
    }

    #[test]
    fn a_home_that_cannot_be_put_back_says_so() {
        let d = dir("net-no-put-back");
        let feed = Arc::new(Feed::default());
        // The tool fails and leaves generated/ unwritable.
        let tool = format!(
            r#"chmod 555 {}/cfg/generated; echo '{{"result":"failed","reason":"x"}}'; exit 1"#,
            d.display()
        );
        let p = net_home(&d, &tool, home_net());
        let change = netfiles::Change {
            wifi: Some(one_net("cafe")),
            wired: None,
        };
        let r = network_apply(&p, &change, false, &feed, |_| {}).unwrap_err();
        let dir = lock(&p.config).dir.generated();
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert!(r.home_changed);
        assert!(
            r.reason.starts_with("x; generated/wifi.yaml could not be restored: "),
            "{}",
            r.reason
        );
    }

    #[test]
    fn unstage_never_follows_a_link() {
        let d = dir("net-unstage-link");
        let feed = Arc::new(Feed::default());
        let target = d.join("elsewhere.yaml");
        // A link appears at generated/wifi.yaml during the apply.
        let p0 = net_home(&d, "true", home_net());
        let link = lock(&p0.config).dir.generated_file("wifi.yaml");
        let tool = format!(
            r#"rm -f {0}; ln -s {1} {0}; echo '{{"result":"failed","reason":"x"}}'; exit 1"#,
            link.display(),
            target.display()
        );
        let mut p = p0;
        p.network_cmd = ["/bin/sh", "-c", &tool, "sh"].map(String::from).to_vec();
        std::fs::write(&target, "elsewhere: 1\n").unwrap();
        let before = gen_file(&p, "wifi.yaml").unwrap();
        let change = netfiles::Change {
            wifi: Some(one_net("cafe")),
            wired: None,
        };
        let r = network_apply(&p, &change, false, &feed, |_| {}).unwrap_err();
        assert_eq!(r.reason, "x");
        assert!(!std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&link).unwrap(), before);
        assert_eq!(std::fs::read(&target).unwrap(), b"elsewhere: 1\n");
    }

    /// A keyboard change while a network Apply runs: the Apply's files stay
    /// out of its way, and its commit keeps the keyboard.
    #[test]
    fn a_keyboard_change_during_an_apply_is_kept() {
        let d = dir("net-apply-keyboard");
        let feed = Arc::new(Feed::default());
        let (started, go) = (d.join("started"), d.join("go"));
        let tool = format!(
            r#"touch {}; while [ ! -e {} ]; do sleep 0.05; done; echo '{{"result":"ok","reason":""}}'"#,
            started.display(),
            go.display()
        );
        let p = net_home(&d, &tool, NetworkCfg::default());
        let store = p.config.clone();
        let (s2, g2) = (started.clone(), go.clone());
        let helper = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(20);
            while !s2.exists() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
            let r = lock(&store).update(Table::Keyboard, |c| c.keyboard.layout = "ch".into());
            std::fs::write(g2, "").unwrap();
            r
        });
        let change = netfiles::Change {
            wifi: Some(one_net("home")),
            wired: None,
        };
        assert_eq!(network_apply(&p, &change, false, &feed, |_| {}), Ok(()));
        assert!(helper.join().unwrap().is_ok());
        let cfg = lock(&p.config).cfg.clone();
        assert_eq!(
            (cfg.keyboard.layout.as_str(), cfg.network.nets()),
            ("ch", one_net("home"))
        );
        let on_disk = crate::config::parse(&String::from_utf8(config_bytes(&p)).unwrap()).unwrap();
        assert_eq!(on_disk, cfg);
        assert!(
            String::from_utf8(gen_file(&p, "wifi.yaml").unwrap())
                .unwrap()
                .contains("home")
        );
    }

    /// The failure path of the same handshake: the keyboard change survives,
    /// the generated network files and `[network]` are as they were.
    #[test]
    fn a_keyboard_change_during_an_apply_leaves_the_staged_files() {
        let d = dir("net-apply-keyboard-fails");
        let feed = Arc::new(Feed::default());
        let (started, go) = (d.join("started"), d.join("go"));
        let tool = format!(
            r#"touch {}; while [ ! -e {} ]; do sleep 0.05; done; {FAILS}"#,
            started.display(),
            go.display()
        );
        let p = net_home(&d, &tool, home_net());
        let before = generated_bytes(&p);
        let net_before = lock(&p.config).cfg.network.clone();
        let store = p.config.clone();
        let (s2, g2) = (started.clone(), go.clone());
        let helper = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(20);
            while !s2.exists() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
            let staged_seen = std::fs::read(lock(&store).dir.generated_file("wifi.yaml")).unwrap();
            let r = lock(&store).update(Table::Keyboard, |c| c.keyboard.layout = "ch".into());
            std::fs::write(g2, "").unwrap();
            (r, staged_seen)
        });
        let change = netfiles::Change {
            wifi: Some(one_net("cafe")),
            wired: Some(fixed_wired()),
        };
        let r = network_apply(&p, &change, false, &feed, |_| {}).unwrap_err();
        let (updated, staged_seen) = helper.join().unwrap();
        assert!(updated.is_ok());
        assert_eq!(r, ApplyFailed::new("x".into()));
        assert!(
            String::from_utf8(staged_seen).unwrap().contains("cafe"),
            "the stage was in place"
        );
        assert_eq!(generated_bytes(&p), before);
        let cfg = lock(&p.config).cfg.clone();
        assert_eq!((cfg.keyboard.layout.as_str(), &cfg.network), ("ch", &net_before));
        let on_disk = crate::config::parse(&String::from_utf8(config_bytes(&p)).unwrap()).unwrap();
        assert_eq!(on_disk, cfg);
    }

    /// A rollback after a half-done stage that fails too names the file and
    /// counts the home as changed.
    #[test]
    fn a_failed_rollback_after_a_half_done_stage_says_so() {
        let d = dir("net-stage-rollback-fails");
        let feed = Arc::new(Feed::default());
        let p = net_home(&d, FAILS, home_net());
        let g = lock(&p.config).dir.generated();
        std::fs::create_dir_all(g.join("wifi.yaml.tmp/x")).unwrap();
        std::fs::create_dir_all(g.join("ethernet.yaml/x")).unwrap();
        // The WiFi file stays as rendered; the wired write meets the directory.
        let change = netfiles::Change {
            wifi: None,
            wired: Some(fixed_wired()),
        };
        let r = network_apply(&p, &change, false, &feed, |_| {}).unwrap_err();
        assert!(r.home_changed, "{r:?}");
        assert!(r.reason.contains("ethernet.yaml"), "{r:?}");
        assert!(r.reason.contains("generated/wifi.yaml could not be restored"), "{r:?}");
    }

    #[test]
    fn a_read_only_store_refuses_before_anything_runs() {
        let d = dir("net-read-only");
        let feed = Arc::new(Feed::default());
        let marker = d.join("called");
        let tool = format!(
            "touch {}; echo '{{\"result\":\"ok\",\"reason\":\"\"}}'",
            marker.display()
        );
        let mut p = net_paths(&d, "/bin/true", &tool);
        let dir = crate::config::ConfigDir { root: d.join("ro") };
        p.config = Arc::new(std::sync::Mutex::new(
            crate::store::open(dir, net_inputs(), OpenMode::ReadOnly, 0).store,
        ));
        let change = netfiles::Change {
            wifi: Some(one_net("home")),
            wired: None,
        };
        let r = network_apply(&p, &change, false, &feed, |_| {}).unwrap_err();
        assert!(r.reason.contains("read-only"), "{r:?}");
        assert!(!r.home_changed);
        assert!(!marker.exists());
        assert!(!d.join("ro").exists(), "nothing was written");
        let e = net_take_over(&p, "wifi").unwrap_err();
        assert!(e.contains("read-only"), "{e}");
    }

    #[test]
    fn a_kept_wifi_text_is_taken_over_through_the_store() {
        let d = dir("net-take-over");
        let mut p = net_paths(&d, "/bin/true", "true");
        let hand = "network:\n  version: 2\n  renderer: networkd\n  wifis:\n    wlp2s0:\n      dhcp4: true\n      access-points:\n        home:\n          password: secret123\n";
        p.config = net_store(
            &d,
            NetworkCfg {
                kept_wifi: Some(hand.into()),
                ..NetworkCfg::default()
            },
        );
        assert!(
            String::from_utf8(gen_file(&p, "wifi.yaml").unwrap())
                .unwrap()
                .contains("wlp2s0")
        );
        let (w, _) = net_load(&p);
        assert_eq!(w.state, netfiles::FileState::Hand);
        assert_eq!(net_take_over(&p, "wifi"), Ok(()));
        let (w, _) = net_load(&p);
        assert_eq!((w.state, w.networks), (netfiles::FileState::Menu, one_net("home")));
        assert!(lock(&p.config).cfg.network.kept_wifi.is_none());
        let on_disk = crate::config::parse(&String::from_utf8(config_bytes(&p)).unwrap()).unwrap();
        assert!(on_disk.network.kept_wifi.is_none());
        assert!(net_take_over(&p, "wifi").is_err(), "nothing kept any more");
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
    /// starting one runs `start` with `-c <config>` as `$1 $2`. The store
    /// is a session store on the same config directory.
    fn mon_home(d: &Path, start: &str) -> (crate::store::Shared, Kanshi) {
        fs::create_dir_all(d.join("proc")).unwrap();
        let cd = crate::config::ConfigDir {
            root: d.join("home/.config/thinfloe"),
        };
        let store = crate::store::test_shared(&cd.root);
        let mut k = Kanshi::from_dir(&cd);
        k.cmd = ["/bin/sh", "-c", start, "sh"].map(String::from).to_vec();
        k.proc_dir = d.join("proc");
        (store, k)
    }

    /// A start script that appends its arguments and what the three files
    /// hold at that moment to `<d>/started`; of a layout it shows the
    /// transform of its output.
    fn recording(d: &Path) -> String {
        let cd = d.join("home/.config/thinfloe/generated");
        let tag = |f: &str| format!("$(grep -o 'transform [0-9a-z]*' {} 2>/dev/null)", cd.join(f).display());
        format!(
            "{{ echo \"args $*\"; echo \"marker $(cat {m} 2>/dev/null)\"; echo \"prev {p}\"; echo \"config {c}\"; }} >> {log}",
            m = cd.join("layout-unconfirmed").display(),
            p = tag("kanshi.prev"),
            c = tag("kanshi"),
            log = d.join("started").display()
        )
    }

    /// One monitor, DP-1, turned by `rot`.
    fn layout(rot: crate::heads::Rotation) -> crate::kanshi::Profile {
        crate::kanshi::Profile {
            outputs: vec![crate::kanshi::OutputSet {
                criteria: "DP-1".into(),
                enabled: true,
                mode: crate::heads::Mode {
                    w: 1920,
                    h: 1080,
                    refresh_mhz: 0,
                    preferred: false,
                },
                rotation: rot,
                place: crate::kanshi::Place::Origin,
            }],
            menu_output: None,
        }
    }

    /// `generated/kanshi` as the menu writes it for `profiles`.
    fn kanshi_text(profiles: &[crate::kanshi::Profile]) -> String {
        format!("{}\n\n{}", crate::generated::HEADER, crate::kanshi::render(profiles))
    }

    /// The body `mon_apply` is given for the layout on screen.
    fn onscreen_body(rot: crate::heads::Rotation) -> String {
        format!("\n{}", crate::kanshi::render(&[layout(rot)]))
    }

    /// Make `profiles` the saved arrangement, as a kept Apply does.
    fn save_profiles(store: &crate::store::Shared, profiles: &[crate::kanshi::Profile]) {
        store
            .lock()
            .unwrap()
            .update(crate::generated::Table::Monitors, |c| {
                c.monitors = crate::config::MonitorsCfg::from_profiles(profiles)
            })
            .unwrap();
    }

    fn saved(store: &crate::store::Shared) -> crate::config::MonitorsCfg {
        store.lock().unwrap().cfg.monitors.clone()
    }

    /// `[monitors]` as `config.toml` holds it.
    fn on_disk(d: &Path) -> crate::config::MonitorsCfg {
        let text = fs::read_to_string(d.join("home/.config/thinfloe/config.toml")).unwrap();
        crate::config::parse(&text).unwrap().monitors
    }

    fn generated_kanshi(k: &Kanshi) -> Option<String> {
        read(&k.config)
    }

    fn read(p: &Path) -> Option<String> {
        fs::read_to_string(p).ok()
    }

    /// A store on `d`'s config directory that refuses writes.
    fn read_only_store(d: &Path) -> crate::store::Shared {
        let cd = crate::config::ConfigDir {
            root: d.join("home/.config/thinfloe"),
        };
        let ro = crate::store::open(
            cd,
            crate::generated::Inputs::default(),
            crate::store::OpenMode::ReadOnly,
            0,
        )
        .store;
        Arc::new(std::sync::Mutex::new(ro))
    }

    #[test]
    fn status_save_updates_the_status_table() {
        let d = dir("status-save");
        let store = crate::store::test_shared(&d.join("cfg"));
        let lines = |s: &crate::store::Shared| s.lock().unwrap().cfg.status.lines.clone();
        status_save(&store, Some("clock 1x1\n")).unwrap();
        assert_eq!(lines(&store), Some(vec!["clock 1x1".to_string()]));
        // The menu's own header comment is not a layout line.
        status_save(
            &store,
            Some("# thinfloe status area, written by the menu\nclock 1x1\n-network 2x2\n"),
        )
        .unwrap();
        assert_eq!(
            lines(&store),
            Some(vec!["clock 1x1".to_string(), "-network 2x2".to_string()])
        );
        // It is on disk, and a fresh read gives the same.
        let text = fs::read_to_string(d.join("cfg/config.toml")).unwrap();
        assert_eq!(crate::config::parse(&text).unwrap().status.lines, lines(&store));
        status_save(&store, None).unwrap();
        assert_eq!(lines(&store), None);
        let text = fs::read_to_string(d.join("cfg/config.toml")).unwrap();
        assert_eq!(crate::config::parse(&text).unwrap().status.lines, None);
        // The old file is not touched, nor made.
        assert!(!d.join("cfg/status").exists());
    }

    #[test]
    fn status_save_on_a_read_only_store_is_the_error() {
        let d = dir("status-save-ro");
        let dir_ = crate::config::ConfigDir { root: d.join("cfg") };
        let ro = crate::store::open(
            dir_,
            crate::generated::Inputs::default(),
            crate::store::OpenMode::ReadOnly,
            0,
        )
        .store;
        let store: crate::store::Shared = Arc::new(std::sync::Mutex::new(ro));
        let e = status_save(&store, Some("clock 1x1\n")).unwrap_err();
        assert_eq!(e, "The settings are read-only.");
        assert!(!d.join("cfg/config.toml").exists());
    }

    #[test]
    fn mon_apply_on_a_read_only_store_changes_nothing() {
        use crate::heads::Rotation::Normal;
        let d = dir("mon-apply-ro");
        let (_, k) = mon_home(&d, &recording(&d));
        let root = d.join("home/.config/thinfloe");
        let ro = crate::store::open(
            crate::config::ConfigDir { root: root.clone() },
            crate::generated::Inputs::default(),
            crate::store::OpenMode::ReadOnly,
            0,
        )
        .store;
        let store: crate::store::Shared = Arc::new(std::sync::Mutex::new(ro));
        // The directory does not exist and must not appear.
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            mon_apply(&store, &k, &[layout(Normal)], Some(&onscreen_body(Normal))),
            Err("The settings are read-only.".to_string())
        );
        assert!(!root.exists(), "no config dir was created");
        assert!(!k.prev().exists() && !k.marker.exists());
        assert!(!d.join("started").exists(), "kanshi was not started");
    }

    #[test]
    fn dismiss_report_clears_the_migration_notice() {
        let d = dir("dismiss-report");
        let store = crate::store::test_shared(&d.join("cfg"));
        store
            .lock()
            .unwrap()
            .update(crate::generated::Table::Notice, |c| {
                c.notice.migration = Some("3 files were moved".into());
            })
            .unwrap();
        dismiss_report(&store).unwrap();
        let text = fs::read_to_string(d.join("cfg/config.toml")).unwrap();
        assert_eq!(crate::config::parse(&text).unwrap().notice.migration, None);
        assert!(!text.contains("migration"), "{text}");
        // Read-only: the error, no file.
        let ro = crate::store::open(
            crate::config::ConfigDir { root: d.join("ro") },
            crate::generated::Inputs::default(),
            crate::store::OpenMode::ReadOnly,
            0,
        )
        .store;
        let ro: crate::store::Shared = Arc::new(std::sync::Mutex::new(ro));
        assert_eq!(dismiss_report(&ro), Err("The settings are read-only.".to_string()));
        assert!(!d.join("ro").exists());
    }

    #[test]
    fn mon_apply_writes_prev_marker_and_config() {
        use crate::heads::Rotation::{Normal, R90, R180};
        let d = dir("mon-apply");
        let (store, k) = mon_home(&d, &recording(&d));
        save_profiles(&store, &[layout(Normal)]);
        let old_text = kanshi_text(&[layout(Normal)]);
        assert_eq!(generated_kanshi(&k), Some(old_text.clone()));
        // The layout on screen the menu always sends is not used: the file
        // is the old layout.
        assert_eq!(
            mon_apply(&store, &k, &[layout(R90)], Some(&onscreen_body(R180))),
            Ok(())
        );
        let new_text = kanshi_text(&[layout(R90)]);
        assert!(new_text.starts_with(crate::generated::HEADER));
        assert_eq!(generated_kanshi(&k), Some(new_text));
        assert_eq!(read(&k.prev()), Some(old_text));
        assert_eq!(read(&k.marker).as_deref(), Some("prev\n"));
        // The settings stay as they were until the Apply is kept.
        assert_eq!(
            saved(&store),
            crate::config::MonitorsCfg::from_profiles(&[layout(Normal)])
        );
        assert_eq!(on_disk(&d), saved(&store));
        // kanshi was started on the config, with marker and prev in place.
        let want = format!(
            "args -c {}\nmarker prev\nprev transform normal\nconfig transform 90\n",
            k.config.display()
        );
        assert_eq!(read(&d.join("started")), Some(want));

        // No file before: the layout on screen is kept instead, over a
        // stale prev, and the marker still says prev.
        let d = dir("mon-apply-none");
        let (store, k) = mon_home(&d, &recording(&d));
        fs::write(k.prev(), "stale").unwrap();
        assert_eq!(
            mon_apply(&store, &k, &[layout(R90)], Some(&onscreen_body(R180))),
            Ok(())
        );
        assert_eq!(read(&k.marker).as_deref(), Some("prev\n"));
        assert_eq!(read(&k.prev()), Some(kanshi_text(&[layout(R180)])));
        assert!(
            read(&d.join("started"))
                .unwrap()
                .contains("marker prev\nprev transform 180\nconfig transform 90\n")
        );

        // Neither a file nor the layout on screen: nothing is written.
        let d = dir("mon-apply-unknown");
        let (store, k) = mon_home(&d, &recording(&d));
        assert!(mon_apply(&store, &k, &[layout(R90)], None).is_err());
        assert!(!k.config.exists() && !k.prev().exists() && !k.marker.exists());
        assert!(!d.join("started").exists());
        assert_eq!(saved(&store), crate::config::MonitorsCfg::default());
    }

    #[test]
    fn mon_apply_tells_a_running_kanshi_and_starts_none() {
        use std::os::unix::process::ExitStatusExt;
        let d = dir("mon-hup");
        let (store, k) = mon_home(&d, &recording(&d));
        // A child of this test stands in for kanshi in the fake proc dir.
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let entry = d.join("proc").join(child.id().to_string());
        fs::create_dir_all(&entry).unwrap();
        fs::write(entry.join("comm"), "kanshi\n").unwrap();
        assert_eq!(kanshi_pids(&k.proc_dir), vec![child.id() as i32]);
        let on_screen = onscreen_body(crate::heads::Rotation::Normal);
        assert_eq!(
            mon_apply(&store, &k, &[layout(crate::heads::Rotation::R90)], Some(&on_screen)),
            Ok(())
        );
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
    fn the_layout_files_live_under_generated() {
        let cd = crate::config::ConfigDir {
            root: PathBuf::from("/h/.config/thinfloe"),
        };
        let k = Kanshi::from_dir(&cd);
        assert_eq!(k.config, PathBuf::from("/h/.config/thinfloe/generated/kanshi"));
        assert_eq!(
            k.marker,
            PathBuf::from("/h/.config/thinfloe/generated/layout-unconfirmed")
        );
        assert_eq!(k.prev(), PathBuf::from("/h/.config/thinfloe/generated/kanshi.prev"));
    }

    #[test]
    fn mon_apply_failure_puts_the_old_file_back() {
        use crate::heads::Rotation::{Normal, R90, R180};
        let d = dir("mon-fail");
        let (store, k) = mon_home(&d, "exit 3");
        save_profiles(&store, &[layout(Normal)]);
        let old_text = kanshi_text(&[layout(Normal)]);
        let e = mon_apply(&store, &k, &[layout(R90)], None).unwrap_err();
        assert!(e.contains("stopped at once"), "{e}");
        assert_eq!(generated_kanshi(&k), Some(old_text));
        assert!(!k.prev().exists());
        assert!(!k.marker.exists());
        assert_eq!(
            saved(&store),
            crate::config::MonitorsCfg::from_profiles(&[layout(Normal)])
        );

        // No file before: none after.
        let d = dir("mon-fail-none");
        let (store, k) = mon_home(&d, "exit 3");
        assert!(mon_apply(&store, &k, &[layout(R90)], Some(&onscreen_body(R180))).is_err());
        assert!(!k.prev().exists());
        assert!(!k.config.exists());
        assert!(!k.marker.exists());

        // A kept hand-written file is not touched, and kanshi not started.
        let d = dir("mon-fail-kept");
        let (store, k) = mon_home(&d, &recording(&d));
        store
            .lock()
            .unwrap()
            .update(crate::generated::Table::Monitors, |c| {
                c.monitors.kept = Some("profile {\n\toutput * enable\n}\n".into())
            })
            .unwrap();
        let before = generated_kanshi(&k);
        assert!(before.is_some());
        let e = mon_apply(&store, &k, &[layout(R90)], None).unwrap_err();
        assert!(e.contains("by hand"), "{e}");
        assert_eq!(generated_kanshi(&k), before);
        assert!(!k.prev().exists());
        assert!(!k.marker.exists());
        assert!(!d.join("started").exists());
    }

    #[test]
    fn a_read_only_store_refuses_the_apply_and_leaves_the_files_alone() {
        use crate::heads::Rotation::{Normal, R90};
        let d = dir("mon-ro");
        let (store, k) = mon_home(&d, &recording(&d));
        save_profiles(&store, &[layout(Normal)]);
        let ro = read_only_store(&d);
        let before = generated_kanshi(&k);
        let e = mon_apply(&ro, &k, &[layout(R90)], None).unwrap_err();
        assert!(e.contains("read-only"), "{e}");
        assert_eq!(generated_kanshi(&k), before, "generated/kanshi is as it was");
        assert!(!k.prev().exists());
        assert!(!k.marker.exists());
        assert!(!d.join("started").exists(), "kanshi was not asked");
        // Taking over and loading go through the store, too.
        assert!(mon_take_over(&ro, &k, &[]).unwrap_err().contains("read-only"));
        assert!(!d.join("started").exists(), "a refused take-over reloads nothing");
        assert_eq!(mon_load(&ro).1, crate::monitors::MonSource::Menu);
    }

    #[test]
    fn mon_revert_restores_prev() {
        use crate::heads::Rotation::{Normal, R90, R180};
        let d = dir("mon-revert");
        let (store, k) = mon_home(&d, &recording(&d));
        save_profiles(&store, &[layout(Normal)]);
        mon_apply(&store, &k, &[layout(R90)], None).unwrap();
        assert_eq!(mon_revert(&k), Ok(()));
        assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[layout(Normal)])));
        assert!(!k.prev().exists());
        assert!(!k.marker.exists());
        // kanshi read the old file again.
        let log = read(&d.join("started")).unwrap();
        assert!(log.ends_with("marker \nprev \nconfig transform normal\n"), "{log}");

        // No file before: the layout on screen comes back, and kanshi
        // reads it.
        let d = dir("mon-revert-none");
        let (store, k) = mon_home(&d, &recording(&d));
        mon_apply(&store, &k, &[layout(R90)], Some(&onscreen_body(R180))).unwrap();
        assert_eq!(mon_revert(&k), Ok(()));
        assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[layout(R180)])));
        assert!(!k.prev().exists());
        assert!(!k.marker.exists());
        let log = read(&d.join("started")).unwrap();
        assert!(log.ends_with("marker \nprev \nconfig transform 180\n"), "{log}");
        assert_eq!(on_disk(&d), crate::config::MonitorsCfg::default());
    }

    #[test]
    fn a_first_apply_then_revert_leaves_the_layout_on_screen() {
        use crate::heads::{Head, Mode, Rotation};
        use crate::monitors::{MonEvent, MonRequest, MonSource, MonTab};
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
        let (store, k) = mon_home(&d, &recording(&d));
        let now = Instant::now();
        let mut t = MonTab::default();
        t.heads_changed(heads.clone());
        let (profiles, source) = mon_load(&store);
        t.loaded(profiles, source);
        t.event(MonEvent::Select("DP-2".into()), now);
        t.event(MonEvent::Rotation(Rotation::R90), now);
        let Some(MonRequest::Apply { profiles, prev, .. }) = t.event(MonEvent::Apply, now) else {
            panic!("no Apply")
        };
        assert_eq!(mon_apply(&store, &k, &profiles, prev.as_deref()), Ok(()));
        assert_eq!(t.event(MonEvent::GoBack, now), None, "the job has not ended yet");
        assert_eq!(t.applied(Ok(())), Some(MonRequest::Revert));
        assert_eq!(mon_revert(&k), Ok(()));
        assert_eq!(
            generated_kanshi(&k),
            Some(kanshi_text(&[crate::kanshi::from_heads(&heads)]))
        );
        // Nothing was kept: the settings still hold no arrangement.
        assert_eq!(mon_load(&store), (Ok(None), MonSource::Missing));
        assert!(!k.prev().exists() && !k.marker.exists());
    }

    #[test]
    fn mon_keep_removes_marker_and_prev() {
        use crate::heads::Rotation::{Normal, R90};
        let d = dir("mon-keep");
        let (store, k) = mon_home(&d, &recording(&d));
        save_profiles(&store, &[layout(Normal)]);
        mon_apply(&store, &k, &[layout(R90)], None).unwrap();
        assert_eq!(mon_keep(&store, &k, vec![layout(R90)]), Ok(()));
        assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[layout(R90)])));
        assert!(!k.prev().exists());
        assert!(!k.marker.exists());
        // The settings hold the applied arrangement now, in memory and on disk.
        let want = crate::config::MonitorsCfg::from_profiles(&[layout(R90)]);
        assert_eq!(saved(&store), want);
        assert_eq!(on_disk(&d), want);
    }

    #[test]
    fn a_marker_that_cannot_go_fails_the_keep() {
        use crate::heads::Rotation::{Normal, R90};
        let d = dir("mon-keep-fail");
        let (store, k) = mon_home(&d, &recording(&d));
        save_profiles(&store, &[layout(Normal)]);
        mon_apply(&store, &k, &[layout(R90)], None).unwrap();
        block(&k.marker);
        let e = mon_keep(&store, &k, vec![layout(R90)]).unwrap_err();
        assert!(e.contains("undo marker"), "{e}");
        // prev stays with the marker, so the next session's undo works, and
        // the settings are not changed.
        assert_eq!(read(&k.prev()), Some(kanshi_text(&[layout(Normal)])));
        assert_eq!(
            saved(&store),
            crate::config::MonitorsCfg::from_profiles(&[layout(Normal)])
        );
    }

    #[test]
    fn a_keep_the_store_refuses_fails_and_generated_follows_the_settings() {
        use crate::heads::Rotation::{Normal, R90};
        let d = dir("mon-keep-ro");
        let (store, k) = mon_home(&d, &recording(&d));
        save_profiles(&store, &[layout(Normal)]);
        mon_apply(&store, &k, &[layout(R90)], None).unwrap();
        let ro = read_only_store(&d);
        let e = mon_keep(&ro, &k, vec![layout(R90)]).unwrap_err();
        assert!(e.contains("read-only"), "{e}");
        // The marker is gone, so this session does not undo it; the file
        // matches config.toml, which is what the next start would make.
        assert!(!k.marker.exists());
        assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[layout(Normal)])));
        assert_eq!(
            on_disk(&d),
            crate::config::MonitorsCfg::from_profiles(&[layout(Normal)])
        );
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
        // thinfloe-session reads marker `prev` without kanshi.prev as
        // "remove the config": the marker must never outlive prev.
        use crate::heads::Rotation::{Normal, R90};
        let d = dir("mon-revert-order");
        let (store, k) = mon_home(&d, &recording(&d));
        save_profiles(&store, &[layout(Normal)]);
        mon_apply(&store, &k, &[layout(R90)], None).unwrap();
        // The marker cannot be removed: the revert stops there.
        block(&k.marker);
        assert!(mon_revert(&k).is_err());
        assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[layout(Normal)])));
        assert_eq!(
            read(&k.prev()),
            Some(kanshi_text(&[layout(Normal)])),
            "prev went before the marker"
        );
    }

    #[test]
    fn a_marker_that_cannot_be_checked_stops_the_apply() {
        // Not "no marker": an earlier kanshi.prev may be the real old layout.
        use crate::heads::Rotation::{Normal, R90};
        let d = dir("mon-marker-unknown");
        let (store, mut k) = mon_home(&d, &recording(&d));
        save_profiles(&store, &[layout(Normal)]);
        fs::write(k.prev(), "the real old layout\n").unwrap();
        // A file where the marker's directory should be: ENOTDIR, not ENOENT.
        let file = d.join("not-a-dir");
        fs::write(&file, "x").unwrap();
        k.marker = file.join("layout-unconfirmed");
        let e = mon_apply(&store, &k, &[layout(R90)], None).unwrap_err();
        assert!(e.contains("could not be checked"), "{e}");
        assert_eq!(read(&k.prev()).as_deref(), Some("the real old layout\n"));
        assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[layout(Normal)])));
        assert!(!d.join("started").exists(), "kanshi was not asked");
    }

    #[test]
    fn a_revert_without_prev_fails_and_keeps_the_marker() {
        use crate::heads::Rotation::{Normal, R90};
        let d = dir("mon-revert-noprev");
        let (store, k) = mon_home(&d, &recording(&d));
        save_profiles(&store, &[layout(Normal)]);
        mon_apply(&store, &k, &[layout(R90)], None).unwrap();
        let started = read(&d.join("started"));
        fs::remove_file(k.prev()).unwrap();
        let e = mon_revert(&k).unwrap_err();
        assert!(e.contains("kanshi.prev"), "{e}");
        // Nothing changed: the next session's undo still sees the marker.
        assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[layout(R90)])));
        assert_eq!(read(&k.marker).as_deref(), Some("prev\n"));
        assert_eq!(read(&d.join("started")), started, "kanshi was not asked");
    }

    #[test]
    fn a_failed_apply_removes_the_marker_before_prev() {
        // The fake kanshi blocks the marker, then fails: the undo cannot
        // remove the marker, so prev must stay for the session's undo.
        use crate::heads::Rotation::{Normal, R90};
        let d = dir("mon-undo-order");
        let m = d.join("home/.config/thinfloe/generated/layout-unconfirmed");
        let start = format!("rm -f {m}; mkdir -p {m}; touch {m}/x; exit 3", m = m.display());
        let (store, k) = mon_home(&d, &start);
        save_profiles(&store, &[layout(Normal)]);
        let e = mon_apply(&store, &k, &[layout(R90)], None).unwrap_err();
        assert_eq!(
            read(&k.prev()),
            Some(kanshi_text(&[layout(Normal)])),
            "prev went before the marker"
        );
        assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[layout(Normal)])));
        assert!(e.contains("undo marker"), "{e}");
    }

    #[test]
    fn mon_apply_refuses_while_a_layout_is_unconfirmed() {
        use crate::heads::Rotation::{Normal, R90, R180};
        let d = dir("mon-unconfirmed");
        let (store, k) = mon_home(&d, &recording(&d));
        save_profiles(&store, &[layout(Normal)]);
        mon_apply(&store, &k, &[layout(R90)], None).unwrap();
        let started = read(&d.join("started"));
        let e = mon_apply(&store, &k, &[layout(R180)], None).unwrap_err();
        assert!(e.contains("not confirmed"), "{e}");
        assert_eq!(
            read(&k.prev()),
            Some(kanshi_text(&[layout(Normal)])),
            "the real old layout is kept"
        );
        assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[layout(R90)])));
        assert_eq!(read(&d.join("started")), started, "kanshi was not asked again");
    }

    #[test]
    fn a_failed_marker_write_leaves_the_old_file() {
        use crate::heads::Rotation::{Normal, R90, R180};
        let d = dir("mon-marker-fail");
        let (store, k) = mon_home(&d, &recording(&d));
        save_profiles(&store, &[layout(Normal)]);
        // The marker's temp file cannot be made: a directory is in its way.
        fs::create_dir_all(d.join("home/.config/thinfloe/generated/layout-unconfirmed.tmp")).unwrap();
        assert!(
            mon_apply(&store, &k, &[layout(R90)], None)
                .unwrap_err()
                .contains("Could not write")
        );
        assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[layout(Normal)])));
        assert!(!k.prev().exists());
        assert!(!d.join("started").exists());
        // Without a file, the kept layout on screen goes again.
        let d = dir("mon-marker-fail-none");
        let (store, k) = mon_home(&d, &recording(&d));
        fs::create_dir_all(d.join("home/.config/thinfloe/generated/layout-unconfirmed.tmp")).unwrap();
        assert!(mon_apply(&store, &k, &[layout(R90)], Some(&onscreen_body(R180))).is_err());
        assert!(!k.config.exists() && !k.prev().exists());
    }

    #[test]
    fn a_failed_config_write_is_rolled_back() {
        use crate::heads::Rotation::{Normal, R90, R180};
        let d = dir("mon-write-fail");
        let (store, k) = mon_home(&d, &recording(&d));
        let mut tmp = k.config.as_os_str().to_owned();
        tmp.push(".tmp");
        // The store writes generated/kanshi through kanshi.tmp, which cannot be made.
        block(Path::new(&tmp));
        let e = mon_apply(&store, &k, &[layout(R90)], Some(&onscreen_body(R180))).unwrap_err();
        assert!(e.contains("could not be written"), "{e}");
        assert!(!k.config.exists());
        assert!(!k.prev().exists());
        assert!(!k.marker.exists());
        assert!(!d.join("started").exists());
        assert_eq!(saved(&store), crate::config::MonitorsCfg::default());

        // With an old file the write fails the same way; the file was
        // never replaced, so the undo has nothing to put back.
        let d = dir("mon-write-fail-old");
        let (store, k) = mon_home(&d, &recording(&d));
        save_profiles(&store, &[layout(Normal)]);
        let mut tmp = k.config.as_os_str().to_owned();
        tmp.push(".tmp");
        block(Path::new(&tmp));
        let e = mon_apply(&store, &k, &[layout(R90)], None).unwrap_err();
        assert!(e.contains("could not be written"), "{e}");
        assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[layout(Normal)])));
        assert!(!k.prev().exists());
        assert!(!k.marker.exists());
    }

    #[test]
    fn a_file_that_cannot_be_put_back_keeps_marker_and_prev() {
        // kanshi fails and leaves a directory where the restore writes:
        // the next session undoes the Apply.
        use crate::heads::Rotation::{Normal, R90};
        let d = dir("mon-putback-fail");
        let tmp = d.join("home/.config/thinfloe/generated/kanshi.tmp");
        let start = format!("mkdir -p {t}; touch {t}/x; exit 3", t = tmp.display());
        let (store, k) = mon_home(&d, &start);
        save_profiles(&store, &[layout(Normal)]);
        let e = mon_apply(&store, &k, &[layout(R90)], None).unwrap_err();
        assert!(e.contains("could not be restored"), "{e}");
        assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[layout(R90)])));
        assert_eq!(read(&k.prev()), Some(kanshi_text(&[layout(Normal)])));
        assert_eq!(read(&k.marker).as_deref(), Some("prev\n"));
    }

    #[test]
    fn a_name_kanshi_could_not_hold_is_refused() {
        use crate::heads::Rotation::{Normal, R90};
        for bad in ["a\"b", "a\nb", "a}b", "a{b"] {
            let d = dir("mon-bad-name");
            let (store, k) = mon_home(&d, &recording(&d));
            save_profiles(&store, &[layout(Normal)]);
            let mut p = layout(R90);
            p.outputs[0].criteria = bad.into();
            let e = mon_apply(&store, &k, &[p], None).unwrap_err();
            assert!(e.contains("kanshi"), "{bad:?}: {e}");
            assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[layout(Normal)])), "{bad:?}");
            assert!(!k.prev().exists() && !k.marker.exists());
            assert!(!d.join("started").exists());
        }
    }

    #[test]
    fn mon_load_and_take_over() {
        use crate::heads::{Head, Mode, Rotation};
        use crate::monitors::MonSource;
        let d = dir("mon-load");
        let (store, k) = mon_home(&d, &recording(&d));
        assert_eq!(mon_load(&store), (Ok(None), MonSource::Missing));
        save_profiles(&store, &[layout(Rotation::R90)]);
        assert_eq!(
            mon_load(&store),
            (Ok(Some(vec![layout(Rotation::R90)])), MonSource::Menu)
        );

        // A kept hand-written file: no profiles, read-only.
        let hand = "profile {\n\toutput * enable\n}\n";
        store
            .lock()
            .unwrap()
            .update(crate::generated::Table::Monitors, |c| {
                c.monitors.kept = Some(hand.into())
            })
            .unwrap();
        assert_eq!(mon_load(&store), (Ok(None), MonSource::Kept));
        assert_eq!(
            generated_kanshi(&k),
            Some(format!("{}\n{hand}", crate::generated::HEADER))
        );

        // The take-over replaces it by the layout on screen.
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
        assert_eq!(mon_take_over(&store, &k, &heads), Ok(()));
        let want = crate::kanshi::from_heads(&heads);
        // The settings keep no preferred flag, as the file never did.
        let stored = crate::config::MonitorsCfg::from_profiles(std::slice::from_ref(&want))
            .to_profiles()
            .unwrap();
        assert_eq!(mon_load(&store), (Ok(Some(stored)), MonSource::Menu));
        assert_eq!(saved(&store).kept, None);
        assert_eq!(on_disk(&d).kept, None);
        assert_eq!(generated_kanshi(&k), Some(kanshi_text(&[want])));

        // kanshi was told to read the new file, with the settings written.
        let log = read(&d.join("started")).unwrap();
        assert!(
            log.starts_with(&format!("args -c {}\nmarker \n", k.config.display())),
            "{log}"
        );
        assert!(
            log.contains("config transform normal"),
            "the new file was already written: {log}"
        );

        // Settings that do not convert are an error, and read-only.
        store
            .lock()
            .unwrap()
            .update(crate::generated::Table::Screen, |c| {
                c.monitors.profiles[0].outputs[0].mode = "bad".into()
            })
            .unwrap();
        let (r, source) = mon_load(&store);
        assert!(r.unwrap_err().contains("bad mode"));
        assert_eq!(source, MonSource::Kept);
    }
}
