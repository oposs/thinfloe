//! The daemon as a process: config, the root helper's lines and signals.
//! A fake helper script stands in for `sudo -n thinfloe-share --radio`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_thinfloe-share");

/// A directory of its own per test below target/, never /tmp.
struct Env {
    dir: PathBuf,
}

impl Env {
    fn new(name: &str) -> Env {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-tmp"))
            .join(format!("daemon-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["run", "home", "media"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        Env { dir }
    }

    fn config(&self, text: &str) {
        std::fs::write(self.dir.join("share"), text).unwrap();
    }

    /// The fake helper: a shell script with `body`; `$D` is the test directory.
    fn helper(&self, body: &str) {
        let p = self.dir.join("helper.sh");
        let script = format!("#!/bin/sh\nD='{}'\n{body}\n", self.dir.display());
        std::fs::write(&p, script).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn spawn(&self) -> Child {
        Command::new(BIN)
            .env("XDG_RUNTIME_DIR", self.dir.join("run"))
            .env("HOME", self.dir.join("home"))
            .env("TF_SHARE_CONFIG", self.dir.join("share"))
            .env("TF_SHARE_RADIO", self.dir.join("helper.sh"))
            .env("TF_SHARE_ROOT", self.dir.join("media"))
            .env_remove("TF_OSD_SOCKET")
            .spawn()
            .unwrap()
    }

    fn state(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("run/thinfloe-share.state")).ok()
    }

    fn exists(&self, name: &str) -> bool {
        self.dir.join(name).exists()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn wait_for(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    cond()
}

fn wait_exit(child: &mut Child, limit: Duration) -> ExitStatus {
    let mut status = None;
    wait_for(limit, || {
        status = child.try_wait().unwrap();
        status.is_some()
    });
    match status {
        Some(s) => s,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the daemon did not exit within {limit:?}");
        }
    }
}

#[test]
fn disabled_config_exits_0() {
    let env = Env::new("disabled");
    env.helper("touch \"$D/ran\"");
    env.config("enabled=false\nname=x\n");
    let mut child = env.spawn();
    assert_eq!(
        wait_exit(&mut child, Duration::from_secs(5)).code(),
        Some(0)
    );
    // No config at all is off too.
    std::fs::remove_file(env.dir.join("share")).unwrap();
    let mut child = env.spawn();
    assert_eq!(
        wait_exit(&mut child, Duration::from_secs(5)).code(),
        Some(0)
    );
    assert!(!env.exists("ran"), "the helper must not start");
    assert_eq!(env.state(), None);
}

#[test]
fn helper_error_line_writes_error_state() {
    let env = Env::new("errline");
    env.helper("printf 'error\\tno card\\n'\nexit 1");
    env.config("enabled=true\n");
    let mut child = env.spawn();
    assert_eq!(
        wait_exit(&mut child, Duration::from_secs(10)).code(),
        Some(1)
    );
    assert_eq!(env.state().as_deref(), Some("error\tno card\n"));
}

#[test]
fn helper_dies_writes_error_state() {
    let env = Env::new("dies");
    env.helper("exit 3");
    env.config("enabled=true\n");
    let mut child = env.spawn();
    assert_eq!(
        wait_exit(&mut child, Duration::from_secs(10)).code(),
        Some(1)
    );
    let state = env.state().expect("a state file");
    let first = state.lines().next().unwrap();
    assert!(first.starts_with("error\t"), "{state:?}");
    assert!(
        first.len() > "error\t".len(),
        "the error has a text: {state:?}"
    );
}

#[test]
fn sigterm_closes_the_helper_stdin() {
    let env = Env::new("sigterm");
    // Says nothing, like a helper still waiting for owlyd; notes stdin's EOF.
    env.helper("touch \"$D/started\"\ncat >/dev/null\ntouch \"$D/eof\"");
    env.config("enabled=true\n");
    let mut child = env.spawn();
    assert!(wait_for(Duration::from_secs(10), || env.exists("started")));
    assert_eq!(env.state().as_deref(), Some("starting\n"));
    // SAFETY: kill with a valid pid and signal number.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    assert_eq!(
        wait_exit(&mut child, Duration::from_secs(10)).code(),
        Some(0)
    );
    assert!(
        wait_for(Duration::from_secs(5), || env.exists("eof")),
        "the helper saw no EOF"
    );
    let state = env.state().expect("a state file");
    assert!(state.starts_with("error\t"), "{state:?}");
}
