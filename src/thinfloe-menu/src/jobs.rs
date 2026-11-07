//! The work a tile starts: running a program, saving the home directory.
//! Each runs on a worker thread of its own, several programs at once, in
//! the background; the event loop keeps answering the compositor while
//! they do. thinfloe-save-home on a netbooted machine can spend 60 seconds
//! pinging before it starts, and a window that stops answering frame
//! callbacks for that long looks dead.

use crate::drives::{self, Drive};
use crate::log::{Feed, Kind, Line, Run, Splitter, local_time};
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
}

/// Like `default_addon_cmd`, for thinfloe-upgrade.
pub fn default_upgrade_cmd() -> Vec<String> {
    ["sudo", "-n", "/usr/bin/thinfloe-upgrade"].map(String::from).to_vec()
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
fn upgrade_capture(
    cmd: &[String],
    args: &[String],
    cap: usize,
    timeout: Duration,
    feed: &Arc<Feed>,
) -> Result<(String, Option<i32>), String> {
    let mut child = upgrade_command(cmd, args)
        .spawn()
        .map_err(|e| format!("{} could not be started: {e}", cmd[0]))?;
    let started = Instant::now();
    let (tx, rx) = mpsc::channel();
    let mut pumps = 0;
    if let Some(err) = child.stderr.take() {
        let f = feed.clone();
        std::thread::spawn(move || pump(err, UPGRADE_LOG_SOURCE.to_string(), f, tx));
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
    Ok((String::from_utf8_lossy(&out).into_owned(), status.code()))
}

/// The drives the tool offers: `drives --no-size [--all]`.
pub fn upgrade_drives(cmd: &[String], all: bool, feed: &Arc<Feed>) -> Result<Vec<Drive>, String> {
    let mut args = vec!["drives".to_string(), "--no-size".to_string()];
    if all {
        args.push("--all".to_string());
    }
    let (text, status) = upgrade_capture(cmd, &args, DRIVES_BYTES, DRIVES_TIMEOUT, feed)?;
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
    let (text, status) = upgrade_capture(cmd, &args, CHECK_BYTES, CHECK_TIMEOUT, feed)?;
    match status {
        Some(0) | Some(1) if !text.trim().is_empty() => Ok(release::parse_status(&text)),
        Some(0) | Some(1) => Err("the release check printed nothing".to_string()),
        Some(n) => Err(format!("the release check failed with status {n}")),
        None => Err("the release check was ended by a signal".to_string()),
    }
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
        };
        assert_eq!(save(&ok, &Arc::new(Feed::default())), SaveOutcome::Saved);
        let one = Paths {
            save_home: "/bin/false".into(),
            state_dir: d.to_path_buf(),
            addon_cmd: default_addon_cmd(),
            upgrade_cmd: default_upgrade_cmd(),
        };
        assert_eq!(save(&one, &Arc::new(Feed::default())), SaveOutcome::HomeMissing);
        let missing = Paths {
            save_home: d.join("nope"),
            state_dir: d.to_path_buf(),
            addon_cmd: default_addon_cmd(),
            upgrade_cmd: default_upgrade_cmd(),
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
        };
        // /bin/sh with no arguments reads stdin, which is /dev/null: exit 0.
        assert_eq!(save(&p, &feed), SaveOutcome::Saved);
        let p = Paths {
            save_home: "/bin/ls".into(),
            state_dir: d.to_path_buf(),
            addon_cmd: default_addon_cmd(),
            upgrade_cmd: default_upgrade_cmd(),
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
        let r = upgrade_capture(&sh("sleep 5"), &[], 100, Duration::from_millis(300), &feed);
        assert!(r.unwrap_err().contains("did not answer"));
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    }
}
