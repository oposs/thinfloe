//! The work a tile starts: running a program, saving the home directory.
//! Each runs on a worker thread of its own, several programs at once, in
//! the background; the event loop keeps answering the compositor while
//! they do. thinfloe-save-home on a netbooted machine can spend 60 seconds
//! pinging before it starts, and a window that stops answering frame
//! callbacks for that long looks dead.

use crate::log::{Feed, Kind, Line, Run, Splitter, local_time};
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
    /// An add-on job ended; `args` are the ones it was started with.
    Addon {
        args: Vec<String>,
        status: Result<Option<i32>, String>,
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
}

/// thinfloe-addon runs as root. tfuser has sudo without a password; `-n`
/// fails at once instead of waiting for a password nobody can type.
pub fn default_addon_cmd() -> Vec<String> {
    ["sudo", "-n", "/usr/bin/thinfloe-addon"].map(String::from).to_vec()
}

/// Copy one output pipe into the feed, line by line, until it closes.
fn pump(mut pipe: impl Read, source: String, feed: Arc<Feed>, done: Sender<()>) {
    let mut split = Splitter::default();
    let mut emit = |runs: Vec<Run>| {
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
pub fn addon(cmd: &[String], args: &[String], feed: &Arc<Feed>) -> JobResult {
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]).args(args);
    let status = run_logged(c, crate::addons::LOG_SOURCE, feed, true, |_| {})
        .map(|st| st.code())
        .map_err(|e| format!("{} could not be started: {e}", cmd[0]));
    JobResult::Addon {
        args: args.to_vec(),
        status,
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
        };
        assert_eq!(save(&ok, &Arc::new(Feed::default())), SaveOutcome::Saved);
        let one = Paths {
            save_home: "/bin/false".into(),
            state_dir: d.to_path_buf(),
            addon_cmd: default_addon_cmd(),
        };
        assert_eq!(save(&one, &Arc::new(Feed::default())), SaveOutcome::HomeMissing);
        let missing = Paths {
            save_home: d.join("nope"),
            state_dir: d.to_path_buf(),
            addon_cmd: default_addon_cmd(),
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
        let r = addon(&cmd, &["install".into(), "10-x".into()], &feed);
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
        let JobResult::Addon { status, .. } = addon(&["/nonexistent/sudo".into()], &["check".into()], &feed) else {
            panic!()
        };
        assert!(status.unwrap_err().contains("could not be started"));
        assert_eq!(default_addon_cmd(), ["sudo", "-n", "/usr/bin/thinfloe-addon"]);
    }

    #[test]
    fn save_streams_its_output_as_save_home() {
        let d = dir("save-out");
        let feed = Arc::new(Feed::default());
        let p = Paths {
            save_home: "/bin/sh".into(),
            state_dir: d.to_path_buf(),
            addon_cmd: default_addon_cmd(),
        };
        // /bin/sh with no arguments reads stdin, which is /dev/null: exit 0.
        assert_eq!(save(&p, &feed), SaveOutcome::Saved);
        let p = Paths {
            save_home: "/bin/ls".into(),
            state_dir: d.to_path_buf(),
            addon_cmd: default_addon_cmd(),
        };
        let _ = save(&p, &feed);
        let got = feed.drain();
        assert!(!got.is_empty(), "ls wrote nothing to the feed");
        assert!(got.iter().all(|l| l.source.as_deref() == Some("save-home")));
    }
}
