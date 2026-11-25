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
pub mod notes;
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
    /// The Sound tab's slider: every speaker to this percent, 0 to 100.
    /// No key is bound to it, and `--osd-key` refuses it.
    VolumeSet(u8),
    /// The Sound tab's mute switch: every speaker muted or not. No key is
    /// bound to it, and `--osd-key` refuses it.
    VolumeMuteSet(bool),
}

impl Action {
    /// The actions without an argument.
    pub const ALL: [Action; 7] = [
        Action::VolumeUp,
        Action::VolumeDown,
        Action::VolumeMute,
        Action::MicMute,
        Action::BrightnessUp,
        Action::BrightnessDown,
        Action::Reload,
    ];

    /// An action as `encode` writes it: a name, or `volume-set <n>`
    /// with n from 0 to 100, or `volume-mute-set 0|1`. Anything else,
    /// extra blanks included, is `None`.
    pub fn parse(s: &str) -> Option<Action> {
        if let Some(a) = Action::ALL.into_iter().find(|a| a.as_str() == s) {
            return Some(a);
        }
        let (verb, arg) = s.split_once(' ')?;
        match verb {
            "volume-set" => {
                let digits = (1..=3).contains(&arg.len()) && arg.bytes().all(|b| b.is_ascii_digit());
                let n: u8 = arg.parse().ok().filter(|_| digits)?;
                (n <= 100).then_some(Action::VolumeSet(n))
            }
            "volume-mute-set" => match arg {
                "0" => Some(Action::VolumeMuteSet(false)),
                "1" => Some(Action::VolumeMuteSet(true)),
                _ => None,
            },
            _ => None,
        }
    }

    /// The action as one datagram's text, without the newline.
    pub fn encode(self) -> String {
        match self {
            Action::VolumeSet(n) => format!("{} {n}", self.as_str()),
            Action::VolumeMuteSet(m) => format!("{} {}", self.as_str(), u8::from(m)),
            _ => self.as_str().to_string(),
        }
    }

    /// Only the menu sends it: labwc never binds it to a key.
    pub fn menu_only(self) -> bool {
        matches!(self, Action::VolumeSet(_) | Action::VolumeMuteSet(_))
    }

    /// The action's name, without its argument.
    pub fn as_str(self) -> &'static str {
        match self {
            Action::VolumeUp => "volume-up",
            Action::VolumeDown => "volume-down",
            Action::VolumeMute => "volume-mute",
            Action::MicMute => "mic-mute",
            Action::BrightnessUp => "brightness-up",
            Action::BrightnessDown => "brightness-down",
            Action::Reload => "reload",
            Action::VolumeSet(_) => "volume-set",
            Action::VolumeMuteSet(_) => "volume-mute-set",
        }
    }
}

/// The file the OSD keeps the speaker state in for the menu's Sound tab:
/// `TF_VOLUME_FILE`, else `$XDG_RUNTIME_DIR/thinfloe-volume`; an empty
/// variable is unset. One line `<percent> <muted>\n`, muted `0` or `1`.
pub fn volume_file_path() -> Option<PathBuf> {
    volume_file_from(std::env::var_os("TF_VOLUME_FILE"), std::env::var_os("XDG_RUNTIME_DIR"))
}

fn volume_file_from(file: Option<OsString>, runtime_dir: Option<OsString>) -> Option<PathBuf> {
    if let Some(p) = file.filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let dir = runtime_dir.filter(|d| !d.is_empty())?;
    Some(PathBuf::from(dir).join("thinfloe-volume"))
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
    let Some(action) = name.to_str().and_then(Action::parse).filter(|a| !a.menu_only()) else {
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
        // The menu sends these, never `--osd-key`.
        Action::Reload | Action::VolumeSet(_) | Action::VolumeMuteSet(_) => return,
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
/// the whole group stopped at `timeout` (SIGTERM, SIGKILL shortly after,
/// always reaped). Its stdout, or why it failed: a status other than 0 is
/// an error carrying the first line of stderr.
fn run_tool(cmd: &[String], args: &[String], timeout: Duration) -> Result<String, String> {
    // The OSD has no log window; stderr is kept in the error instead.
    let feed = Arc::new(crate::log::Feed::default());
    let (out, status, stderr) = crate::jobs::capture_killable(cmd, args, TOOL_BYTES, timeout, &feed, &cmd[0])?;
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
            assert_eq!(Action::parse(&a.encode()), Some(a));
            assert!(!a.menu_only(), "{a:?}");
        }
        assert_eq!(Action::parse("volume"), None);
        assert_eq!(Action::parse("reload"), Some(Action::Reload));
        for a in [
            Action::VolumeSet(0),
            Action::VolumeSet(7),
            Action::VolumeSet(100),
            Action::VolumeMuteSet(false),
            Action::VolumeMuteSet(true),
        ] {
            assert_eq!(Action::parse(&a.encode()), Some(a));
            assert!(a.menu_only(), "{a:?}");
        }
        assert_eq!(Action::VolumeSet(42).encode(), "volume-set 42");
        assert_eq!(Action::VolumeMuteSet(true).encode(), "volume-mute-set 1");
    }

    #[test]
    fn set_actions_reject_bad_arguments() {
        for bad in [
            "volume-set 101",
            "volume-set x",
            "volume-set",
            "volume-set ",
            "volume-set -1",
            "volume-set +5",
            "volume-set 1000",
            "volume-set  5",
            "volume-set 5 ",
            "volume-set 2.5",
            "volume-mute-set 2",
            "volume-mute-set on",
            "volume-mute-set",
            "volume-up 5",
        ] {
            assert_eq!(Action::parse(bad), None, "{bad:?}");
        }
        assert_eq!(Action::parse("volume-set 007"), Some(Action::VolumeSet(7)));
    }

    #[test]
    fn osd_key_refuses_the_menus_actions() {
        for a in ["volume-set 50", "volume-mute-set 1", "nonsense"] {
            let code = osd_key_main(&[OsString::from(a)]);
            assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(2)), "{a}");
        }
    }

    #[test]
    fn the_volume_file_is_in_the_runtime_dir_unless_named() {
        let o = |s: &str| Some(OsString::from(s));
        assert_eq!(volume_file_from(o("/a/v"), o("/run")), Some(PathBuf::from("/a/v")));
        assert_eq!(
            volume_file_from(o(""), o("/run/user/1001")),
            Some(PathBuf::from("/run/user/1001/thinfloe-volume"))
        );
        assert_eq!(volume_file_from(None, None), None);
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

    #[test]
    fn a_tool_that_ignores_sigterm_is_killed_and_reaped() {
        let d = crate::testdir::TestDir::new("osd", "ignore-term");
        let pidfile = d.join("pid");
        // SIG_IGN is inherited across exec, so `sleep` ignores SIGTERM too.
        let script = format!("trap '' TERM; echo $$ > {}; sleep 30", pidfile.display());
        let cmd = ["/bin/sh", "-c", &script, "fake"].map(String::from).to_vec();
        let t0 = std::time::Instant::now();
        let r = run_tool(&cmd, &[], Duration::from_millis(300));
        assert!(r.unwrap_err().contains("did not answer"));
        assert!(t0.elapsed() < Duration::from_secs(3), "took {:?}", t0.elapsed());
        let pid: libc::pid_t = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        // A zombie still answers kill(pid, 0); only a reaped group is gone.
        let gone = |p: libc::pid_t| unsafe { libc::kill(p, 0) } != 0;
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while !(gone(pid) && gone(-pid)) && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(gone(pid), "the tool {pid} still exists (alive or a zombie)");
        assert!(gone(-pid), "a process of group {pid} still exists");
    }
}
