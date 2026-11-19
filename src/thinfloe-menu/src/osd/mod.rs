//! `thinfloe-menu --osd`: the on-screen display of the volume and
//! brightness keys, in overlay surfaces above every window (`surface`).
//!
//! The session starts `--osd` once (`thinfloe-session`); labwc runs
//! `--osd-key <action>` on each media key, which hands the action to the
//! OSD over a datagram socket (`socket`) and returns at once. The OSD runs
//! `pactl` and `ddcutil` (`audio`, `ddc`) off its loop (`run`), keeps
//! every speaker and microphone in step (`sync`) and shows the result for
//! 1.5 s (`state`). Without an OSD the client changes the volume itself
//! and leaves the brightness alone.
//!
//! `--osd --probe` is a test hook: it shows a Volume 45 % panel and the
//! mark on every output, at scale 1, until SIGTERM.

pub mod audio;
pub mod ddc;
pub mod draw;
mod run;
pub mod socket;
pub mod state;
pub mod surface;
pub mod sync;

use std::ffi::{OsStr, OsString};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
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

/// Percent one volume key changes the speakers by.
const VOLUME_STEP: i32 = 5;

/// A key labwc hands to `--osd-key`, or `Reload` from the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    VolumeUp,
    VolumeDown,
    VolumeMute,
    MicMute,
    BrightnessUp,
    BrightnessDown,
    /// Read the volume keys switch again (the menu sends it after a
    /// keyboard apply). No key is bound to it.
    Reload,
}

impl Action {
    pub const ALL: [Action; 7] = [
        Action::VolumeUp,
        Action::VolumeDown,
        Action::VolumeMute,
        Action::MicMute,
        Action::BrightnessUp,
        Action::BrightnessDown,
        Action::Reload,
    ];

    pub fn parse(s: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.as_str() == s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Action::VolumeUp => "volume-up",
            Action::VolumeDown => "volume-down",
            Action::VolumeMute => "volume-mute",
            Action::MicMute => "mic-mute",
            Action::BrightnessUp => "brightness-up",
            Action::BrightnessDown => "brightness-down",
            Action::Reload => "reload",
        }
    }
}

/// Where the OSD listens: `TF_OSD_SOCKET`, else
/// `$XDG_RUNTIME_DIR/thinfloe-osd.sock`; an empty variable is unset.
pub fn socket_path() -> Option<PathBuf> {
    socket_path_from(std::env::var_os("TF_OSD_SOCKET"), std::env::var_os("XDG_RUNTIME_DIR"))
}

fn socket_path_from(sock: Option<OsString>, runtime_dir: Option<OsString>) -> Option<PathBuf> {
    if let Some(p) = sock.filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let dir = runtime_dir.filter(|d| !d.is_empty())?;
    Some(PathBuf::from(dir).join("thinfloe-osd.sock"))
}

/// `thinfloe-menu --osd-key <action>`: hand the key to the OSD, or act
/// without it. labwc waits for nothing: every outcome but an unknown
/// action is exit 0.
pub fn osd_key_main(args: &[OsString]) -> ExitCode {
    let name = args.first().map(|a| a.as_os_str()).unwrap_or(OsStr::new(""));
    let Some(action) = name.to_str().and_then(Action::parse) else {
        eprintln!("thinfloe-menu --osd-key: unknown action '{}'", name.to_string_lossy());
        return ExitCode::from(2);
    };
    let sent = match socket_path() {
        Some(path) => socket::send(&path, action),
        None => Err(socket::SendError::NoServer),
    };
    match sent {
        Ok(()) => {}
        Err(socket::SendError::NoServer) => act_without_osd(action),
        Err(socket::SendError::Io(e)) => eprintln!("thinfloe-menu --osd-key: {}: {e}", action.as_str()),
    }
    ExitCode::SUCCESS
}

/// The volume and mute keys on every speaker or microphone through
/// `pactl`, starting from the default device's value since there is no
/// shared state; the brightness keys need the OSD's monitor list and do
/// nothing. Without an OSD there is nothing to reload: it reads the
/// switch when it starts.
fn act_without_osd(a: Action) {
    let kind = match a {
        Action::Reload => return,
        Action::VolumeUp | Action::VolumeDown | Action::VolumeMute => audio::Kind::Sink,
        Action::MicMute => audio::Kind::Source,
        Action::BrightnessUp | Action::BrightnessDown => {
            eprintln!("thinfloe-menu --osd-key: no OSD running; brightness not changed");
            return;
        }
    };
    let p = audio::Pactl::from_env();
    // A default that cannot be read is an error, not "no default": the
    // keys would start from 100 %.
    let done = p
        .list(kind)
        .and_then(|d| Ok((d, p.default_name(kind)?)))
        .map(|(devices, name)| {
            let default = Some(name).filter(|n| !n.is_empty());
            let (mut s, _) = match kind {
                audio::Kind::Sink => sync::Shared::start(sync::Mode::Keys, &devices, &[], default.as_deref(), None),
                audio::Kind::Source => sync::Shared::start(sync::Mode::Keys, &[], &devices, None, default.as_deref()),
            };
            let sets = match a {
                Action::VolumeUp => s.step(VOLUME_STEP, &devices),
                Action::VolumeDown => s.step(-VOLUME_STEP, &devices),
                Action::VolumeMute => s.toggle_mute(&devices),
                _ => s.toggle_mic(&devices),
            };
            for e in p.apply(&sets) {
                eprintln!("thinfloe-menu --osd-key: no OSD running; {}: {e}", a.as_str());
            }
        });
    match done {
        Ok(()) => eprintln!(
            "thinfloe-menu --osd-key: no OSD running; {} done without a display",
            a.as_str()
        ),
        Err(e) => eprintln!("thinfloe-menu --osd-key: no OSD running; {}: {e}", a.as_str()),
    }
}

/// The longest the OSD sleeps in `poll`. SIGTERM interrupts a running
/// poll, but one that lands between the STOP check and the call is only
/// seen when this runs out.
const POLL_MAX_MS: libc::c_int = 1000;

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
    match run::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("thinfloe-menu --osd: {e}");
            ExitCode::FAILURE
        }
    }
}

fn install_sigterm() {
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGTERM, on_sigterm as *const () as libc::sighandler_t);
    }
}

/// Shows a Volume 45 % panel and the mark on every output that has a
/// name, again whenever the outputs change, until SIGTERM.
fn probe() -> Result<(), String> {
    install_sigterm();
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
        let n = unsafe { libc::poll(&mut pfd, 1, POLL_MAX_MS) };
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
    fn actions_round_trip() {
        for a in Action::ALL {
            assert_eq!(Action::parse(a.as_str()), Some(a));
        }
        assert_eq!(Action::parse("volume"), None);
        assert_eq!(Action::parse("reload"), Some(Action::Reload));
    }

    #[test]
    fn the_socket_is_in_the_runtime_dir_unless_named() {
        let o = |s: &str| Some(OsString::from(s));
        assert_eq!(socket_path_from(o("/a/s"), o("/run")), Some(PathBuf::from("/a/s")));
        assert_eq!(
            socket_path_from(o(""), o("/run/user/1001")),
            Some(PathBuf::from("/run/user/1001/thinfloe-osd.sock"))
        );
        assert_eq!(socket_path_from(None, o("")), None);
        assert_eq!(socket_path_from(None, None), None);
    }

    #[test]
    fn a_blank_override_is_the_default_command() {
        assert_eq!(cmd_from_env("TF_OSD_TEST_UNSET_VARIABLE", "pactl"), ["pactl"]);
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
