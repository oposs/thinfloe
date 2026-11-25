//! The menu's side of its daemons (spec, Processes): it runs
//! `thinfloe-menu --deskflow` while `[deskflow] listen` is on, and
//! `thinfloe-share` while `[share] enabled` is on; starts one again after
//! it ended (1 s, 2 s, 4 s … 60 s apart), tells it to read its file again
//! after a write, and stops it when the switch goes off or the menu exits.
//! `Daemon` holds what differs between the two.
//!
//! The daemon runs in a process group of its own and is signalled through
//! the group. It sets `PR_SET_PDEATHSIG` itself, so a menu that dies
//! takes it along. That signal follows the *thread* that forked, not the
//! process: the menu calls `tick` (which spawns) only from its event loop
//! on the main thread, which lives as long as the process. A spawn from a
//! job or worker thread would end the daemon when that thread ends.

use std::ffi::OsString;
use std::io::{ErrorKind, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::deskflow::PARENT_ENV;
use crate::deskflow::statefile::{self, Head};
use crate::log::{Feed, Kind, Line, Run, Splitter, local_time};

/// The source the Deskflow daemon's lines carry in the menu's log.
pub const LOG_SOURCE: &str = "deskflow";

/// What differs between the daemons the menu supervises.
#[derive(Clone, Copy)]
pub struct Daemon {
    /// The source its lines carry in the menu's log.
    pub source: &'static str,
    /// Its name in the menu's messages.
    pub label: &'static str,
    /// The prefixes it writes before its lines; the log's source names it
    /// already.
    pub prefixes: &'static [&'static str],
    /// The variable that carries the menu's pid to it, if it reads one.
    pub parent_env: Option<&'static str>,
    /// It starts with SIGHUP blocked and unblocks it once its handler is
    /// set. A daemon that does not unblock it would never see a reload.
    pub block_hup: bool,
    /// A reload's SIGHUP goes to its whole process group, else to its pid
    /// only: a child of the daemon in that group (FloeDrop's radio helper
    /// under sudo, which relays it) would end on it. SIGTERM and SIGKILL
    /// always go to the group.
    pub reload_group: bool,
    /// The error line of its state file, if the text has one.
    pub error_head: fn(&str) -> Option<String>,
}

/// `thinfloe-menu --deskflow`.
pub const DESKFLOW: Daemon = Daemon {
    source: LOG_SOURCE,
    label: "Deskflow",
    prefixes: &["thinfloe-menu --deskflow: ", "thinfloe-deskflow: "],
    parent_env: Some(PARENT_ENV),
    block_hup: true,
    reload_group: true,
    error_head: deskflow_error_head,
};

fn deskflow_error_head(text: &str) -> Option<String> {
    match statefile::parse(text).head {
        Head::Error(why) => Some(why),
        _ => None,
    }
}

/// The first wait before a start again.
const FIRST_DELAY: Duration = Duration::from_secs(1);
/// The longest wait between two starts.
const MAX_DELAY: Duration = Duration::from_secs(60);
/// A run at least this long starts the waits from `FIRST_DELAY` again.
const RESET_AFTER: Duration = Duration::from_secs(60);
/// After SIGTERM for a switch turned off, how long before SIGKILL.
const TERM_GRACE: Duration = Duration::from_secs(1);
/// File times come from a coarse clock, a few milliseconds behind
/// `SystemTime::now`; a state file this much older than the start still
/// counts as written by this run.
const MTIME_SLACK: Duration = Duration::from_secs(1);
/// How often `stop` looks whether the daemon has ended.
const STOP_POLL: Duration = Duration::from_millis(10);

/// Called from the waiter thread when the daemon has ended (not yet
/// reaped); the menu wakes its loop with it.
type Notify = Box<dyn Fn() + Send + Sync>;

struct Running {
    child: Child,
    started: Instant,
    /// When it started, for the age of the state file.
    started_wall: SystemTime,
    /// Set when the menu asked it to end: SIGKILL is due then, and the
    /// exit is no failure.
    kill_at: Option<Instant>,
}

pub struct Supervisor {
    program: PathBuf,
    args: Vec<OsString>,
    daemon: Daemon,
    state_file: Option<PathBuf>,
    notify: Option<std::sync::Arc<Notify>>,
    /// Where the daemon's stderr goes; none leaves it on the menu's.
    log: Option<Arc<Feed>>,
    wanted: bool,
    running: Option<Running>,
    /// When the next start is due, while wanted and not running.
    start_at: Option<Instant>,
    /// The wait before the start after the next exit.
    delay: Duration,
    failure: Option<String>,
}

impl Supervisor {
    /// `program` is run without arguments (see `with_args`), as the
    /// Deskflow daemon (see `with_daemon`).
    pub fn new(program: PathBuf) -> Supervisor {
        Supervisor {
            program,
            args: Vec::new(),
            daemon: DESKFLOW,
            state_file: statefile::path(),
            notify: None,
            log: None,
            wanted: false,
            running: None,
            start_at: None,
            delay: FIRST_DELAY,
            failure: None,
        }
    }

    /// The arguments `program` gets: `[DESKFLOW_ARG]` for the menu's own
    /// binary.
    pub fn with_args(mut self, args: Vec<OsString>) -> Supervisor {
        self.args = args;
        self
    }

    /// Which daemon this is; `DESKFLOW` by default.
    pub fn with_daemon(mut self, daemon: Daemon) -> Supervisor {
        self.daemon = daemon;
        self
    }

    /// The state file the daemon writes its error head to; by default the
    /// one `statefile::path` names.
    pub fn with_state_file(mut self, path: Option<PathBuf>) -> Supervisor {
        self.state_file = path;
        self
    }

    /// The daemon's stderr goes, line by line, into the menu's log, as its
    /// `Daemon::source`; the log copies each line to the session log.
    pub fn with_log(mut self, feed: Arc<Feed>) -> Supervisor {
        self.log = Some(feed);
        self
    }

    /// `f` runs on a thread of its own each time a daemon ends, so the
    /// caller can wake up and `tick`.
    pub fn with_notify(mut self, f: impl Fn() + Send + Sync + 'static) -> Supervisor {
        self.notify = Some(std::sync::Arc::new(Box::new(f)));
        self
    }

    /// The switch: on starts the daemon at the next `tick`, off sends it
    /// SIGTERM (SIGKILL follows after `TERM_GRACE`).
    pub fn want(&mut self, listen: bool, now: Instant) {
        if listen == self.wanted {
            return;
        }
        self.wanted = listen;
        self.failure = None;
        self.delay = FIRST_DELAY;
        if listen {
            self.start_at = Some(now);
            return;
        }
        self.start_at = None;
        if let Some(r) = self.running.as_mut()
            && r.kill_at.is_none()
        {
            signal_group(&r.child, libc::SIGTERM);
            r.kill_at = Some(now + TERM_GRACE);
        }
    }

    /// Reap an ended daemon, start one that is due, send a SIGKILL that is
    /// due. Returns when it wants to be called next; `None` while
    /// it waits only for the daemon to end (the notifier says when).
    pub fn tick(&mut self, now: Instant) -> Option<Instant> {
        if let Some(r) = self.running.as_mut() {
            match r.child.try_wait() {
                Ok(Some(status)) => self.ended(status, now),
                Ok(None) => {
                    if let Some(at) = r.kill_at {
                        if now < at {
                            return Some(at);
                        }
                        signal_group(&r.child, libc::SIGKILL);
                    }
                    return None;
                }
                Err(e) => self.lost(e, now),
            }
        }
        if !self.wanted {
            return None;
        }
        let at = self.start_at?;
        if now < at {
            return Some(at);
        }
        self.start_at = None;
        if let Err(e) = self.spawn(now) {
            eprintln!("thinfloe-menu: {e}");
            self.failure = Some(e);
            self.schedule(now);
            return self.start_at;
        }
        None
    }

    /// Tell a running daemon to read its file again (SIGHUP). A daemon
    /// that has not yet set its handler gets the signal once it has: it
    /// starts with SIGHUP blocked.
    pub fn reload(&self) {
        if let Some(r) = self.running.as_ref()
            && r.kill_at.is_none()
        {
            if self.daemon.reload_group {
                signal_group(&r.child, libc::SIGHUP);
            } else {
                // SAFETY: kill has no memory preconditions. The child is
                // not reaped yet, so its pid is still its own.
                unsafe { libc::kill(r.child.id() as libc::pid_t, libc::SIGHUP) };
            }
        }
    }

    /// End the daemon now: SIGTERM to its group, at most `wait` for it to
    /// end, then SIGKILL. Blocks; for the menu's exit.
    pub fn stop(&mut self, wait: Duration) {
        self.wanted = false;
        self.start_at = None;
        let Some(mut r) = self.running.take() else { return };
        signal_group(&r.child, libc::SIGTERM);
        let deadline = Instant::now() + wait;
        loop {
            match r.child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) if Instant::now() >= deadline => break,
                Ok(None) => std::thread::sleep(STOP_POLL),
            }
        }
        signal_group(&r.child, libc::SIGKILL);
        let _ = r.child.wait();
    }

    /// Why the daemon ended the last time it failed: its error line from
    /// the state file, else its exit status. `None` after a switch and
    /// after a run of `RESET_AFTER`.
    pub fn last_failure(&self) -> Option<String> {
        self.failure.clone()
    }

    /// When the running daemon started, by the wall clock; for the age
    /// of its state file.
    pub fn started(&self) -> Option<SystemTime> {
        self.running.as_ref().map(|r| r.started_wall)
    }

    /// The daemon's pid while it runs.
    pub fn pid(&self) -> Option<u32> {
        self.running.as_ref().map(|r| r.child.id())
    }

    fn spawn(&mut self, now: Instant) -> Result<(), String> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args).stdin(Stdio::null()).process_group(0);
        if let Some(var) = self.daemon.parent_env {
            cmd.env(var, OsString::from(std::process::id().to_string()));
        }
        let block_hup = self.daemon.block_hup;
        // stdout goes where the menu's goes, the session log; stderr to
        // the menu's log when there is one.
        if self.log.is_some() {
            cmd.stderr(Stdio::piped());
        }
        // SAFETY: only signal and sigprocmask, both async-signal-safe.
        unsafe {
            cmd.pre_exec(move || {
                crate::jobs::reset_signals();
                // A SIGHUP before the daemon's handler is set would end it;
                // blocked, it waits for the daemon to unblock it.
                if block_hup {
                    let mut set: libc::sigset_t = std::mem::zeroed();
                    libc::sigemptyset(&mut set);
                    libc::sigaddset(&mut set, libc::SIGHUP);
                    libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                }
                Ok(())
            });
        }
        let label = self.daemon.label;
        let child = cmd.spawn().map_err(|e| {
            format!(
                "the {label} daemon could not be started: {} {e}",
                self.program.display()
            )
        })?;
        let mut child = child;
        if let (Some(feed), Some(pipe)) = (self.log.clone(), child.stderr.take()) {
            let daemon = self.daemon;
            std::thread::spawn(move || pump(pipe, &feed, daemon.source, daemon.prefixes));
        }
        if let Some(n) = self.notify.clone() {
            let pid = child.id() as libc::id_t;
            std::thread::spawn(move || {
                wait_ended(pid);
                n();
            });
        }
        self.running = Some(Running {
            child,
            started: now,
            started_wall: SystemTime::now(),
            kill_at: None,
        });
        Ok(())
    }

    fn ended(&mut self, status: ExitStatus, now: Instant) {
        let Some(r) = self.running.take() else { return };
        if r.kill_at.is_some() {
            // Asked to end: a switch turned on again in the meantime starts
            // at once.
            if self.wanted {
                self.start_at = Some(now);
            }
            return;
        }
        if now.saturating_duration_since(r.started) >= RESET_AFTER {
            self.delay = FIRST_DELAY;
            self.failure = None;
        }
        if !status.success() {
            let why = self.reason(status, r.started_wall);
            eprintln!("thinfloe-menu: the {} daemon stopped: {why}", self.daemon.label);
            self.failure = Some(why);
        }
        if self.wanted {
            self.schedule(now);
        }
    }

    /// `try_wait` failed: the child is beyond reach (reaped elsewhere, so
    /// its pid may already be another process's). It counts as ended and is
    /// never signalled again; a start again follows as after a failure.
    fn lost(&mut self, e: std::io::Error, now: Instant) {
        let Some(r) = self.running.take() else { return };
        let why = format!("the {} daemon cannot be waited for: {e}", self.daemon.label);
        eprintln!("thinfloe-menu: {why}");
        if r.kill_at.is_none() {
            self.failure = Some(why);
        }
        if self.wanted {
            if r.kill_at.is_some() {
                self.start_at = Some(now);
            } else {
                self.schedule(now);
            }
        }
    }

    /// The next start after `delay`, and the wait after it doubled.
    fn schedule(&mut self, now: Instant) {
        self.start_at = Some(now + self.delay);
        self.delay = (self.delay * 2).min(MAX_DELAY);
    }

    /// The error head the daemon wrote in this run, else its status.
    fn reason(&self, status: ExitStatus, started: SystemTime) -> String {
        if status.code() == Some(1)
            && let Some(path) = self.state_file.as_ref()
            && let Ok(meta) = std::fs::metadata(path)
            && meta.modified().is_ok_and(|m| m + MTIME_SLACK >= started)
            && let Ok(text) = std::fs::read_to_string(path)
            && let Some(why) = (self.daemon.error_head)(&text)
        {
            return why;
        }
        match (status.code(), status.signal()) {
            (Some(code), _) => format!("it ended with status {code}"),
            (None, Some(sig)) => format!("it was ended by signal {sig}"),
            (None, None) => "it ended".into(),
        }
    }
}

/// Signal the daemon's process group (its pid: it leads the group).
/// Copy the daemon's stderr into the log until it closes, without the
/// daemon's own prefix.
fn pump(mut pipe: impl Read, feed: &Feed, source: &str, prefixes: &[&str]) {
    let mut split = Splitter::default();
    let mut emit = |mut runs: Vec<Run>| {
        if let Some(first) = runs.first_mut()
            && let Some(rest) = prefixes.iter().find_map(|p| first.text.strip_prefix(p))
        {
            first.text = rest.to_string();
        }
        feed.push(Line {
            time: local_time(),
            source: Some(source.to_string()),
            runs,
            kind: Kind::Output,
        });
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
}

fn signal_group(child: &Child, sig: libc::c_int) {
    if let Some(target) = crate::jobs::stop_target(child.id() as i32) {
        // SAFETY: kill has no memory preconditions; ESRCH is ignored.
        unsafe { libc::kill(target, sig) };
    }
}

/// Wait until `pid` has ended, without reaping it: `tick` reaps.
fn wait_ended(pid: libc::id_t) {
    loop {
        // SAFETY: a zeroed siginfo_t that waitid fills.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let r = unsafe { libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT) };
        if r == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deskflow::DESKFLOW_ARG;
    use crate::testdir::TestDir;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    /// A stand-in daemon: `body` as a shell script, run with `--deskflow`.
    fn program(d: &Path, body: &str) -> PathBuf {
        program_in(d, "/bin/sh", body)
    }

    fn program_in(d: &Path, shell: &str, body: &str) -> PathBuf {
        let p = d.join("daemon");
        std::fs::write(&p, format!("#!{shell}\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn sup(d: &Path, body: &str) -> Supervisor {
        Supervisor::new(program(d, body))
            .with_args(vec![DESKFLOW_ARG.into()])
            .with_state_file(Some(d.join("state")))
    }

    /// Tick at `now` until the running daemon has ended and been
    /// reaped; the next wake is then returned.
    fn until_ended(s: &mut Supervisor, now: Instant) -> Option<Instant> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let wake = s.tick(now);
            if s.pid().is_none() {
                return wake;
            }
            assert!(Instant::now() < deadline, "the stand-in did not end");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Ended, as a zombie or gone.
    fn gone(pid: u32) -> bool {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat.rsplit_once(") ").is_some_and(|(_, rest)| rest.starts_with('Z')),
            Err(_) => true,
        }
    }

    fn gone_within(pid: u32, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        while !gone(pid) {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    #[test]
    fn backoff_doubles_to_60() {
        let d = TestDir::new("deskflow-sup", "backoff");
        let mut s = sup(&d, "exit 1");
        let mut now = Instant::now();
        s.want(true, now);
        assert_eq!(s.tick(now), None, "the switch starts the daemon at once");
        assert!(s.pid().is_some());
        let mut waits = Vec::new();
        for _ in 0..8 {
            let at = until_ended(&mut s, now).expect("a start again is due");
            waits.push((at - now).as_secs());
            assert_eq!(s.tick(at - Duration::from_millis(1)), Some(at), "nothing starts early");
            now = at;
            assert_eq!(s.tick(now), None);
            assert!(s.pid().is_some(), "started again when due");
        }
        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(s.last_failure().as_deref(), Some("it ended with status 1"));
        s.stop(Duration::from_secs(1));
    }

    #[test]
    fn long_run_resets_backoff() {
        let d = TestDir::new("deskflow-sup", "reset");
        let mut s = sup(&d, "exit 3");
        let mut now = Instant::now();
        s.want(true, now);
        s.tick(now);
        for _ in 0..3 {
            now = until_ended(&mut s, now).unwrap();
            s.tick(now);
        }
        // The fourth run lasts 61 s as far as the supervisor can tell.
        let later = now + Duration::from_secs(61);
        let at = until_ended(&mut s, later).unwrap();
        assert_eq!(at - later, Duration::from_secs(1));
        assert_eq!(s.last_failure().as_deref(), Some("it ended with status 3"));
        s.stop(Duration::from_secs(1));
    }

    #[test]
    fn listen_off_stops() {
        let d = TestDir::new("deskflow-sup", "off");
        let mut s = sup(&d, "exec sleep 100");
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        let pid = s.pid().unwrap();
        s.want(false, now);
        until_ended(&mut s, now);
        assert!(gone(pid));
        assert_eq!(s.tick(now + Duration::from_secs(120)), None, "no start again");
        assert_eq!(s.pid(), None);
        assert_eq!(s.last_failure(), None, "a stop asked for is no failure");
    }

    #[test]
    fn listen_off_kills_a_daemon_that_ignores_sigterm() {
        let d = TestDir::new("deskflow-sup", "off-kill");
        let ready = d.join("ready");
        let mut s = sup(
            &d,
            &format!("trap '' TERM\n: >{}\nwhile :; do sleep 1; done", ready.display()),
        );
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        s.want(false, now);
        let at = s.tick(now).expect("SIGKILL is due");
        assert_eq!(at - now, Duration::from_secs(1));
        assert!(s.pid().is_some(), "still running before the SIGKILL");
        until_ended(&mut s, at);
        assert_eq!(s.pid(), None);
    }

    #[test]
    fn stop_kills_group() {
        let d = TestDir::new("deskflow-sup", "stop");
        let kid = d.join("kid");
        // The daemon and a child of it ignore SIGTERM: only SIGKILL to the
        // whole group ends both.
        let mut s = sup(
            &d,
            &format!(
                "trap '' TERM\nsleep 100 &\necho $! >{}.tmp\nmv {0}.tmp {0}\nwait",
                kid.display()
            ),
        );
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        let pid = s.pid().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !kid.exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let kid: u32 = std::fs::read_to_string(&kid).unwrap().trim().parse().unwrap();
        let t0 = Instant::now();
        s.stop(Duration::from_secs(1));
        let took = t0.elapsed();
        assert!(
            took >= Duration::from_secs(1) && took < Duration::from_secs(3),
            "{took:?}"
        );
        assert!(gone(pid));
        assert!(
            gone_within(kid, Duration::from_secs(2)),
            "the daemon's child outlived the SIGKILL"
        );
        assert_eq!(s.pid(), None);
        assert_eq!(
            s.tick(now + Duration::from_secs(120)),
            None,
            "a stopped daemon is not started again"
        );
    }

    #[test]
    fn stop_waits_no_longer_than_the_daemon_takes() {
        let d = TestDir::new("deskflow-sup", "stop-quick");
        let mut s = sup(&d, "exec sleep 100");
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        let pid = s.pid().expect("started");
        let t0 = Instant::now();
        s.stop(Duration::from_secs(1));
        assert!(gone(pid));
        assert!(t0.elapsed() < Duration::from_millis(900), "{:?}", t0.elapsed());
    }

    #[test]
    fn an_exit_0_with_listen_still_wanted_starts_again() {
        let d = TestDir::new("deskflow-sup", "exit0");
        let mut s = sup(&d, "exit 0");
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        let at = until_ended(&mut s, now).expect("a start again is due");
        assert_eq!(at - now, Duration::from_secs(1));
        assert_eq!(s.last_failure(), None, "exit 0 is no failure");
        s.tick(at);
        assert!(s.pid().is_some());
        s.stop(Duration::from_secs(1));
    }

    #[test]
    fn the_error_head_of_this_run_is_the_last_failure() {
        let d = TestDir::new("deskflow-sup", "error-head");
        let state = d.join("state");
        let mut s = sup(
            &d,
            &format!("printf 'error\\tport 24800 is in use\\n' >{}\nexit 1", state.display()),
        );
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        until_ended(&mut s, now);
        assert_eq!(s.last_failure().as_deref(), Some("port 24800 is in use"));
        // A switch turned off and on again forgets it.
        s.want(false, now);
        assert_eq!(s.last_failure(), None);
    }

    #[test]
    fn an_old_error_head_is_not_this_runs_reason() {
        let d = TestDir::new("deskflow-sup", "old-head");
        let state = d.join("state");
        std::fs::write(&state, "error\tan old reason\n").unwrap();
        let old = SystemTime::now() - Duration::from_secs(10);
        std::fs::File::options()
            .write(true)
            .open(&state)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let mut s = sup(&d, "exit 1");
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        until_ended(&mut s, now);
        assert_eq!(s.last_failure().as_deref(), Some("it ended with status 1"));
    }

    #[test]
    fn a_reload_reaches_the_daemon_also_before_its_handler_is_set() {
        let d = TestDir::new("deskflow-sup", "reload");
        // The stand-in has no handler at all, like the daemon before its
        // config, identity and listener. bash, as dash clears the mask.
        let mut s = Supervisor::new(program_in(&d, "/bin/bash", "sleep 0.3")).with_state_file(None);
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        assert!(s.pid().is_some());
        s.reload();
        until_ended(&mut s, now);
        assert_eq!(s.last_failure(), None, "the early SIGHUP ended the daemon");
    }

    #[test]
    fn the_daemon_runs_in_a_group_of_its_own_with_the_parent_pid() {
        let d = TestDir::new("deskflow-sup", "group");
        let out = d.join("out");
        let mut s = sup(
            &d,
            &format!(
                "echo \"$1 $(ps -o pgid= -p $$ | tr -d ' ') $$ $TF_DESKFLOW_PARENT\" >{}",
                out.display()
            ),
        );
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        let pid = s.pid().unwrap();
        until_ended(&mut s, now);
        let text = std::fs::read_to_string(&out).unwrap();
        assert_eq!(text.trim(), format!("--deskflow {pid} {pid} {}", std::process::id()));
    }

    #[test]
    fn a_daemon_that_cannot_be_waited_for_counts_as_ended() {
        let d = TestDir::new("deskflow-sup", "wait-err");
        let mut s = sup(&d, "exit 0");
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        let pid = s.pid().expect("started");
        assert!(gone_within(pid, Duration::from_secs(5)), "the stand-in did not end");
        // Someone else reaps it: try_wait fails from now on (ECHILD).
        let mut status = 0;
        // SAFETY: waitpid on our own child, with a valid status pointer.
        assert_eq!(
            unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) },
            pid as libc::pid_t
        );
        let at = s.tick(now).expect("a start again is due");
        assert_eq!(at - now, Duration::from_secs(1));
        assert_eq!(s.pid(), None, "the lost child is dropped, never signalled again");
        assert!(s.last_failure().is_some_and(|f| f.contains("cannot be waited for")));
        s.tick(at);
        assert!(s.pid().is_some(), "started again when due");
        s.stop(Duration::from_secs(1));
    }

    #[test]
    fn the_daemons_stderr_reaches_the_log_without_its_prefix() {
        let d = TestDir::new("deskflow-sup", "log");
        let feed = Arc::new(Feed::default());
        let mut s = Supervisor::new(program(
            &d,
            "echo 'thinfloe-menu --deskflow: a connection ended before pairing: TLS: x' >&2\n\
             echo 'thinfloe-deskflow: released (escape)' >&2\n\
             echo 'plain line' >&2",
        ))
        .with_state_file(None)
        .with_log(feed.clone());
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        until_ended(&mut s, now);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut lines = Vec::new();
        while lines.len() < 3 && Instant::now() < deadline {
            lines.extend(feed.drain());
            std::thread::sleep(Duration::from_millis(5));
        }
        let got: Vec<_> = lines.iter().map(|l| (l.source.as_deref(), l.plain(), l.kind)).collect();
        assert_eq!(
            got,
            [
                (
                    Some(LOG_SOURCE),
                    "a connection ended before pairing: TLS: x".to_string(),
                    Kind::Output
                ),
                (Some(LOG_SOURCE), "released (escape)".to_string(), Kind::Output),
                (Some(LOG_SOURCE), "plain line".to_string(), Kind::Output),
            ]
        );
    }

    #[test]
    fn supervisor_passes_its_args() {
        let d = TestDir::new("deskflow-sup", "args");
        let out = d.join("out");
        let record = format!("printf '%s|' \"$@\" >{0}.tmp\nmv {0}.tmp {0}", out.display());
        for (args, want) in [
            (vec![], ""),
            (vec![OsString::from("--a"), OsString::from("b c")], "--a|b c|"),
        ] {
            let _ = std::fs::remove_file(&out);
            let mut s = Supervisor::new(program(&d, &record))
                .with_state_file(None)
                .with_args(args);
            let now = Instant::now();
            s.want(true, now);
            s.tick(now);
            until_ended(&mut s, now);
            let got = std::fs::read_to_string(&out).unwrap();
            // printf with no arguments still prints its format once.
            let want = if want.is_empty() { "|" } else { want };
            assert_eq!(got, want);
        }
    }

    #[test]
    fn the_notifier_fires_when_the_daemon_ends() {
        let d = TestDir::new("deskflow-sup", "notify");
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let mut s = Supervisor::new(program(&d, "sleep 0.1"))
            .with_state_file(None)
            .with_notify(move || {
                let _ = tx.lock().unwrap().send(());
            });
        let now = Instant::now();
        s.want(true, now);
        s.tick(now);
        assert!(s.pid().is_some());
        rx.recv_timeout(Duration::from_secs(5)).expect("no wake for the exit");
        assert!(
            s.tick(now).is_some(),
            "the exit is reaped at the first tick after the wake"
        );
        assert_eq!(s.pid(), None);
    }
}
