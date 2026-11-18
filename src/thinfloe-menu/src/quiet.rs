//! Run a helper program quietly and boundedly: no output reaches the log or
//! the terminal, at most `cap` bytes of its stdout are read, and it is
//! killed with its whole process group at `cap` or at the timeout. For a
//! background thread that asks a program for an answer (the public IP
//! lookup), where a stuck or flooding helper must cost neither memory nor
//! a thread for long.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// What a quiet run gave: the stdout read, and the exit code (`None`: ended
/// by a signal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quiet {
    pub stdout: Vec<u8>,
    pub status: Option<i32>,
}

/// How often the exit of the child is looked for after its stdout closed.
const POLL: Duration = Duration::from_millis(10);

/// Run `cmd` with `args` after it, `stdin` written to it (none: `/dev/null`),
/// in a process group of its own, stderr to `/dev/null`, and stdout to
/// `/dev/null` when `cap` is 0. More than `cap` bytes on stdout, or no exit
/// within `timeout`, kills the group and is an `Err`.
pub fn run(
    cmd: &[String],
    args: &[String],
    stdin: Option<&[u8]>,
    cap: usize,
    timeout: Duration,
) -> Result<Quiet, String> {
    let (prog, pre) = cmd.split_first().ok_or("no command")?;
    let deadline = Instant::now() + timeout;
    let mut c = Command::new(prog);
    c.args(pre)
        .args(args)
        // Its own group, so a kill reaches whatever it started too.
        .process_group(0)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(if cap == 0 { Stdio::null() } else { Stdio::piped() })
        .stderr(Stdio::null());
    let mut child = c.spawn().map_err(|e| format!("{prog}: {e}"))?;
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let bytes = bytes.to_vec();
        // In a thread: a child that reads nothing must not block the write
        // past the timeout. A broken pipe is the child's choice, not ours.
        std::thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        });
    }
    let mut stdout = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        let (tx, rx) = mpsc::channel();
        // The read blocks until the child closes its stdout, so it runs in a
        // thread and the wait for it has the deadline.
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let r = pipe.take(cap as u64 + 1).read_to_end(&mut bytes).map(|_| bytes);
            let _ = tx.send(r);
        });
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Ok(bytes)) if bytes.len() > cap => {
                kill(&mut child);
                return Err(format!("more than {cap} bytes of output"));
            }
            Ok(Ok(bytes)) => stdout = bytes,
            Ok(Err(e)) => {
                kill(&mut child);
                return Err(format!("reading its output: {e}"));
            }
            Err(_) => {
                kill(&mut child);
                return Err(timed_out(timeout));
            }
        }
    }
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                return Ok(Quiet {
                    stdout,
                    status: st.code(),
                });
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            Ok(None) => {
                kill(&mut child);
                return Err(timed_out(timeout));
            }
            Err(e) => {
                kill(&mut child);
                return Err(e.to_string());
            }
        }
    }
}

fn timed_out(timeout: Duration) -> String {
    format!("no answer within {} ms", timeout.as_millis())
}

/// SIGKILL to the child's group, then reap the child.
fn kill(child: &mut Child) {
    if let Ok(pgid) = i32::try_from(child.id()) {
        // SAFETY: kill(2) has no memory preconditions; a negative pid names
        // the group the child leads (`process_group(0)`).
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        ["/bin/sh", "-c", script, "sh"].map(String::from).to_vec()
    }

    #[test]
    fn a_tool_past_its_timeout_is_killed() {
        let started = Instant::now();
        let r = run(&["sleep".into()], &["5".into()], None, 16, Duration::from_millis(200));
        assert!(r.is_err(), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
        // With stdout to /dev/null the timeout holds too.
        let started = Instant::now();
        let r = run(&["sleep".into()], &["5".into()], None, 0, Duration::from_millis(200));
        assert!(r.is_err(), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    }

    #[test]
    fn a_kill_reaches_what_the_tool_started() {
        // The shell starts a sleep that holds stdout open; the kill of the
        // group ends both, so the run returns at the timeout.
        let started = Instant::now();
        let r = run(&sh("sleep 5; true"), &[], None, 16, Duration::from_millis(200));
        assert!(r.is_err(), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    }

    #[test]
    fn more_than_cap_is_an_error() {
        let r = run(&sh("printf 12345678901234567"), &[], None, 16, Duration::from_secs(5));
        assert!(r.unwrap_err().contains("16 bytes"));
        // An endless stream ends at the cap, not at the timeout.
        let started = Instant::now();
        let r = run(&["yes".into()], &[], None, 16, Duration::from_secs(5));
        assert!(r.is_err());
        assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
        // Exactly `cap` is fine.
        let r = run(&sh("printf 1234567890123456"), &[], None, 16, Duration::from_secs(5)).unwrap();
        assert_eq!(r.stdout, b"1234567890123456");
    }

    #[test]
    fn stdin_reaches_the_tool() {
        let r = run(&["cat".into()], &[], Some(b"hello"), 16, Duration::from_secs(5)).unwrap();
        assert_eq!(
            r,
            Quiet {
                stdout: b"hello".to_vec(),
                status: Some(0)
            }
        );
    }

    #[test]
    fn args_follow_the_command_and_the_exit_code_comes_back() {
        let r = run(
            &sh("printf '%s|' \"$@\"; exit 3"),
            &["a b".into(), "-c".into()],
            None,
            64,
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(r.stdout, b"a b|-c|");
        assert_eq!(r.status, Some(3));
        assert!(run(&[], &[], None, 16, Duration::from_secs(1)).is_err());
        assert!(run(&["/nonexistent/tool".into()], &[], None, 16, Duration::from_secs(1)).is_err());
    }
}
