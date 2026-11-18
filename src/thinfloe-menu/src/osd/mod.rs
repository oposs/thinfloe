//! `thinfloe-menu --osd`: the on-screen display of the volume and
//! brightness keys, in overlay surfaces above every window (`surface`).
//!
//! For now only `--osd --probe` exists, a test hook: it shows a Volume
//! 45 % panel and the mark (`draw`) on every output, at scale 1, until
//! SIGTERM.

pub mod audio;
pub mod ddc;
pub mod draw;
pub mod surface;

use std::ffi::OsString;
use std::os::fd::AsRawFd;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use draw::{Painter, PanelView};
use surface::{SurfaceKind, Surfaces, mark_placement, panel_placement};

pub const OSD_ARG: &str = "--osd";
pub const OSD_KEY_ARG: &str = "--osd-key";
const PROBE_ARG: &str = "--probe";

/// Most bytes of a tool's stdout the OSD reads; more is an error.
const TOOL_BYTES: usize = 64 * 1024;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigterm(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

pub fn osd_main(args: &[OsString]) -> ExitCode {
    if args.iter().any(|a| a == PROBE_ARG) {
        return match probe() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("thinfloe-menu --osd: {e}");
                ExitCode::FAILURE
            }
        };
    }
    eprintln!("thinfloe-menu --osd: not yet");
    ExitCode::from(2)
}

/// Shows a Volume 45 % panel and the mark on every output that has a
/// name, again whenever the outputs change, until SIGTERM.
fn probe() -> Result<(), String> {
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGTERM, on_sigterm as *const () as libc::sighandler_t);
    }
    let mut painter = Painter::new();
    let view = PanelView::Volume { percent: 45 };
    let panel = panel_placement(1.0, view.has_bar());
    let mark = mark_placement(1.0);
    let panel_px = painter.panel(&view, 1.0).pixels;
    let mark_px = painter.mark(1.0).pixels;

    let mut s = Surfaces::connect()?;
    let mut changed = true;
    while !STOP.load(Ordering::SeqCst) {
        if changed {
            for out in s.outputs() {
                s.show(&out, SurfaceKind::Panel, &panel, panel.size.0, &panel_px)?;
                s.show(&out, SurfaceKind::Mark, &mark, mark.size.0, &mark_px)?;
            }
        }
        s.flush()?;
        let mut pfd = libc::pollfd {
            fd: s.fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd for the duration of the call.
        let n = unsafe { libc::poll(&mut pfd, 1, 1000) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("poll: {e}"));
        }
        if pfd.revents & (libc::POLLERR | libc::POLLHUP) != 0 {
            return Err("the compositor closed the connection".into());
        }
        changed = s.dispatch()?.0;
    }
    Ok(())
}

/// The command in the environment variable `var`, split on whitespace;
/// `default` when it is unset or blank. Tests point it at fakes.
fn cmd_from_env(var: &str, default: &str) -> Vec<String> {
    let cmd: Vec<String> = std::env::var(var)
        .unwrap_or_default()
        .split_whitespace()
        .map(String::from)
        .collect();
    if cmd.is_empty() { vec![default.to_string()] } else { cmd }
}

/// Run `cmd` with `args` the way the menu runs its tools (`jobs`): a
/// process group of its own, default signals, stdout capped at 64 KiB and
/// the whole group stopped at `timeout`. Its stdout, or why it failed: a
/// status other than 0 is an error carrying the first line of stderr.
fn run_tool(cmd: &[String], args: &[String], timeout: Duration) -> Result<String, String> {
    // The OSD has no log window; stderr is kept in the error instead.
    let feed = Arc::new(crate::log::Feed::default());
    let (out, status, stderr) = crate::jobs::capture_as(cmd, args, TOOL_BYTES, timeout, &feed, &cmd[0], |_| {})?;
    if status == Some(0) {
        return Ok(out);
    }
    let what = [&cmd[..1], args].concat().join(" ");
    let first = stderr.lines().next().unwrap_or("").trim();
    Err(match (first.is_empty(), status) {
        (false, _) => format!("{what}: {first}"),
        (true, Some(n)) => format!("{what} failed with status {n}"),
        (true, None) => format!("{what} was ended by a signal"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_override_is_the_default_command() {
        assert_eq!(cmd_from_env("TF_OSD_TEST_UNSET_VARIABLE", "wpctl"), ["wpctl"]);
    }

    #[test]
    fn a_tool_status_other_than_0_is_an_error_with_its_stderr() {
        let sh = |s: &str| ["/bin/sh", "-c", s, "fake"].map(String::from).to_vec();
        let t = Duration::from_secs(5);
        assert_eq!(run_tool(&sh("echo hi"), &[], t), Ok("hi\n".to_string()));
        assert_eq!(
            run_tool(&sh("echo oops >&2; exit 3"), &["x".into()], t),
            Err("/bin/sh x: oops".to_string())
        );
        assert_eq!(
            run_tool(&sh("exit 3"), &[], t),
            Err("/bin/sh failed with status 3".to_string())
        );
    }
}
