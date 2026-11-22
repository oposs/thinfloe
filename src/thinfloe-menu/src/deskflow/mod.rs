//! `thinfloe-menu --deskflow`: the Deskflow server, a process of its own
//! beside the menu (spec, Processes). Its keyboard and pointer drive the
//! computers attached at the edges of ThinFloe's monitors.
//!
//! `--deskflow --probe-capture <output> <side>` is a test hook for the
//! runtime tests: it puts one strip on that edge of the output, prints one
//! line per capture event, begins a capture as soon as the pointer enters
//! the strip and ends it on Escape, until SIGTERM.
//!
//! `--deskflow --probe-clip` is a test hook for the clipboard bridge: it
//! prints `changed <len> <sha256-hex>` for each text another client puts on
//! the clipboard, `cleared` for a selection it does not carry (no text, or
//! over the cap), and sets the clipboard from stdin lines `set <text>`.
//!
//! `--deskflow` alone is the daemon ([`run`]).

pub mod capture;
pub mod clip;
pub mod clipdata;
pub mod clipsync;
pub mod cmd;
pub mod conn;
pub mod edge;
pub mod faillog;
pub mod forward;
pub mod ident;
pub mod keymap;
pub mod pair;
pub mod proto;
pub mod run;
pub mod session;
pub mod statefile;
pub mod tls;

use std::ffi::OsString;
use std::io::ErrorKind;
use std::os::fd::AsRawFd;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use capture::{CapEvent, Capture, OutputGeom, StripSpec};
use edge::Rect;
use keymap::{Xkb, keysym_name};

pub const DESKFLOW_ARG: &str = "--deskflow";
/// The pid of the menu that started the daemon (`deskflow_sup`).
pub const PARENT_ENV: &str = "TF_DESKFLOW_PARENT";
const PROBE_CAPTURE_ARG: &str = "--probe-capture";
const PROBE_CLIP_ARG: &str = "--probe-clip";

/// The evdev code of Escape, for a keymap that cannot be read.
const KEY_ESC: u32 = 1;
/// Escape's keysym.
const XK_ESCAPE: u32 = 0xff1b;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigterm(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn install_sigterm() {
    // SAFETY: the handler only stores to an atomic, which is
    // async-signal-safe.
    unsafe {
        libc::signal(libc::SIGTERM, on_sigterm as *const () as libc::sighandler_t);
    }
}

pub fn deskflow_main(args: &[OsString]) -> ExitCode {
    let arg = |i: usize| args.get(i).and_then(|a| a.to_str());
    if arg(0) == Some(PROBE_CAPTURE_ARG)
        && let (Some(output), Some(side)) = (arg(1), arg(2))
    {
        return match probe_capture(output, side) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("thinfloe-menu --deskflow: {e}");
                ExitCode::from(1)
            }
        };
    }
    if arg(0) == Some(PROBE_CLIP_ARG) {
        return match probe_clip() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("thinfloe-menu --deskflow: {e}");
                ExitCode::from(1)
            }
        };
    }
    if args.is_empty() {
        return daemon();
    }
    eprintln!(
        "usage: thinfloe-menu --deskflow [--probe-capture <output> <left-of|right-of|above|below> | --probe-clip]"
    );
    ExitCode::from(2)
}

/// The daemon, with the paths of the session.
fn daemon() -> ExitCode {
    let fail = |e: &str| {
        eprintln!("thinfloe-menu --deskflow: {e}");
        ExitCode::from(1)
    };
    // The menu starts the daemon; it must not outlive it. A parent that
    // died before the prctl sends nothing, so the parent is checked after
    // it: the menu's pid from the environment, else the parent at start.
    // SAFETY: getppid and prctl take no pointers.
    let parent = std::env::var(PARENT_ENV)
        .ok()
        .and_then(|p| p.parse::<libc::pid_t>().ok())
        .unwrap_or_else(|| unsafe { libc::getppid() });
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong, 0, 0, 0) } != 0 {
        return fail(&format!("PR_SET_PDEATHSIG: {}", std::io::Error::last_os_error()));
    }
    if unsafe { libc::getppid() } != parent {
        eprintln!("thinfloe-menu --deskflow: the menu that started it is gone");
        return ExitCode::SUCCESS;
    }
    let (Some(state), Some(socket)) = (statefile::path(), cmd::socket_path()) else {
        return fail("XDG_RUNTIME_DIR is not set");
    };
    let dir = crate::config::ConfigDir::from_env();
    let env = run::RunEnv {
        generated: dir.generated_file("deskflow"),
        identity: dir.root.join("deskflow").join("identity.pem"),
        state,
        socket,
        hostname: hostname(),
        osd_key: run::osd_key_command(std::env::var("TF_OSD_KEY_CMD").ok().as_deref()),
    };
    match run::run(env) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

/// The host name for the certificate, from /etc/hostname (the initramfs
/// writes it).
fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "thinfloe".into())
}

/// The 1 px strip along `side` of `out`, in layout coordinates.
fn probe_strip(out: &OutputGeom, side: &str) -> Option<Rect> {
    let (x, y, w, h) = (out.x, out.y, out.w, out.h);
    Some(match side {
        "left-of" => Rect { x, y, w: 1, h },
        "right-of" => Rect {
            x: x + w - 1,
            y,
            w: 1,
            h,
        },
        "above" => Rect { x, y, w, h: 1 },
        "below" => Rect {
            x,
            y: y + h - 1,
            w,
            h: 1,
        },
        _ => return None,
    })
}

fn probe_capture(output: &str, side: &str) -> Result<(), String> {
    install_sigterm();
    let mut cap = Capture::connect()?;
    let geom = cap
        .outputs()
        .into_iter()
        .find(|o| o.name == output)
        .ok_or_else(|| format!("no output {output}"))?;
    let rect = probe_strip(&geom, side).ok_or_else(|| format!("unknown side {side}"))?;
    cap.set_strips(&[StripSpec {
        id: 1,
        output: output.to_string(),
        rect,
    }]);
    let mut keys: Option<Xkb> = None;
    while !STOP.load(Ordering::SeqCst) {
        cap.flush()?;
        let mut fds = [libc::pollfd {
            fd: cap.fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        // SAFETY: one valid pollfd for the duration of the call.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), 1, 500) };
        if r < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("poll: {e}"));
        }
        if fds[0].revents & (libc::POLLERR | libc::POLLHUP) != 0 {
            return Err("the compositor closed the connection".into());
        }
        for ev in cap.dispatch()? {
            match &ev {
                CapEvent::Enter { strip, .. } => cap.begin_capture(*strip),
                CapEvent::Keymap { fd, size } => match Xkb::from_keymap_fd(fd, *size) {
                    Ok(k) => keys = Some(k),
                    Err(e) => {
                        eprintln!("thinfloe-menu --deskflow: keymap: {e}");
                        keys = None;
                    }
                },
                _ => {}
            }
            println!("{}", probe_line(&ev, keys.as_ref()));
            if let CapEvent::Key { code, pressed: true } = ev {
                let escape = match &keys {
                    Some(k) => k.keysym(code) == XK_ESCAPE,
                    None => code == KEY_ESC,
                };
                if escape {
                    let _ = cap.end_capture(None);
                }
            }
        }
    }
    Ok(())
}

fn probe_clip() -> Result<(), String> {
    install_sigterm();
    let mut clip = clip::Clip::connect()?;
    let mut input = Vec::new();
    let mut stdin_open = true;
    while !STOP.load(Ordering::SeqCst) {
        clip.flush()?;
        let now = Instant::now();
        let mut fds = vec![
            libc::pollfd {
                fd: clip.fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if stdin_open { 0 } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        fds.extend(
            clip.pipe_fds()
                .into_iter()
                .map(|(fd, events)| libc::pollfd { fd, events, revents: 0 }),
        );
        let wait = clip
            .deadline()
            .map_or(500, |d| d.saturating_duration_since(now).as_millis().min(500) as i32);
        // SAFETY: fds is valid for fds.len() entries for the call.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, wait) };
        if r < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("poll: {e}"));
        }
        let now = Instant::now();
        if fds[0].revents & (libc::POLLERR | libc::POLLHUP) != 0 {
            return Err("the compositor closed the connection".into());
        }
        let mut events = clip.dispatch(now)?;
        for p in &fds[2..] {
            if p.revents != 0 {
                events.extend(clip.on_pipe_ready(p.fd, now));
            }
        }
        events.extend(clip.tick(now));
        for ev in events {
            match ev {
                clip::ClipEvent::Changed(text) => {
                    let digest = ring::digest::digest(&ring::digest::SHA256, &text);
                    let hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
                    println!("changed {} {hex}", text.len());
                }
                clip::ClipEvent::Cleared => println!("cleared"),
            }
        }
        if fds[1].revents != 0 {
            let mut buf = [0u8; 4096];
            // SAFETY: buf is valid for buf.len() bytes.
            let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                input.extend_from_slice(&buf[..n as usize]);
            } else if n == 0 {
                stdin_open = false;
            }
            while let Some(i) = input.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = input.drain(..=i).collect();
                if let Some(text) = line.strip_prefix(b"set ") {
                    clip.set_text(text.strip_suffix(b"\n").unwrap_or(text).to_vec());
                }
            }
        }
    }
    Ok(())
}

/// The probe's line for one event; a key also shows its keysym's name,
/// "-" without a keymap.
fn probe_line(ev: &CapEvent, keys: Option<&Xkb>) -> String {
    let updown = |p: bool| if p { "down" } else { "up" };
    match ev {
        CapEvent::Enter { strip, x, y } => format!("enter {strip} {x:.0} {y:.0}"),
        CapEvent::Leave { strip } => format!("leave {strip}"),
        CapEvent::Motion { x, y } => format!("motion {x:.0} {y:.0}"),
        CapEvent::Relative { dx, dy, .. } => format!("relative {dx:.2} {dy:.2}"),
        CapEvent::Locked => "locked".into(),
        CapEvent::Unlocked => "unlocked".into(),
        CapEvent::Key { code, pressed } => {
            let name = keys
                .map(|k| k.keysym(*code))
                .filter(|s| *s != 0)
                .map_or_else(|| "-".into(), keysym_name);
            format!("key {code} {} {name}", updown(*pressed))
        }
        CapEvent::Modifiers {
            depressed,
            latched,
            locked,
            group,
        } => format!("modifiers {depressed} {latched} {locked} {group}"),
        CapEvent::Keymap { size, .. } => format!("keymap {size}"),
        CapEvent::RepeatInfo { rate, delay } => format!("repeat {rate} {delay}"),
        CapEvent::Button { button, pressed } => format!("button {button} {}", updown(*pressed)),
        CapEvent::Axis { v120_x, v120_y } => format!("axis {v120_x} {v120_y}"),
        CapEvent::KeyboardLeft => "keyboard-left".into(),
        CapEvent::OutputsChanged => "outputs-changed".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_strip_lies_on_the_named_edge() {
        let out = OutputGeom {
            name: "HEADLESS-1".into(),
            description: String::new(),
            x: 1280,
            y: 0,
            w: 1280,
            h: 720,
        };
        assert_eq!(
            probe_strip(&out, "right-of"),
            Some(Rect {
                x: 2559,
                y: 0,
                w: 1,
                h: 720
            })
        );
        assert_eq!(
            probe_strip(&out, "left-of"),
            Some(Rect {
                x: 1280,
                y: 0,
                w: 1,
                h: 720
            })
        );
        assert_eq!(
            probe_strip(&out, "above"),
            Some(Rect {
                x: 1280,
                y: 0,
                w: 1280,
                h: 1
            })
        );
        assert_eq!(
            probe_strip(&out, "below"),
            Some(Rect {
                x: 1280,
                y: 719,
                w: 1280,
                h: 1
            })
        );
        assert_eq!(probe_strip(&out, "behind"), None);
    }

    #[test]
    fn probe_lines_name_each_event() {
        assert_eq!(
            probe_line(
                &CapEvent::Enter {
                    strip: 1,
                    x: 1279.5,
                    y: 360.0
                },
                None
            ),
            "enter 1 1280 360"
        );
        assert_eq!(probe_line(&CapEvent::Locked, None), "locked");
        assert_eq!(probe_line(&CapEvent::Unlocked, None), "unlocked");
        assert_eq!(
            probe_line(
                &CapEvent::Key {
                    code: 45,
                    pressed: true
                },
                None
            ),
            "key 45 down -"
        );
        assert_eq!(
            probe_line(
                &CapEvent::Button {
                    button: 272,
                    pressed: false
                },
                None
            ),
            "button 272 up"
        );
        assert_eq!(probe_line(&CapEvent::KeyboardLeft, None), "keyboard-left");
    }
}
