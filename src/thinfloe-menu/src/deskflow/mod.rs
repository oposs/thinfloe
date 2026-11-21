//! `thinfloe-menu --deskflow`: the Deskflow server, a process of its own
//! beside the menu (spec, Processes). Its keyboard and pointer drive the
//! computers attached at the edges of ThinFloe's monitors.
//!
//! `--deskflow --probe-capture <output> <side>` is a test hook for the
//! runtime tests: it puts one strip on that edge of the output, prints one
//! line per capture event, begins a capture as soon as the pointer enters
//! the strip and ends it on Escape, until SIGTERM.

pub mod capture;
pub mod edge;
pub mod ident;
pub mod pair;
pub mod proto;
pub mod session;
pub mod tls;

use std::ffi::{CStr, OsString};
use std::fs::File;
use std::io::ErrorKind;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use capture::{CapEvent, Capture, OutputGeom, StripSpec};
use edge::Rect;

pub const DESKFLOW_ARG: &str = "--deskflow";
const PROBE_CAPTURE_ARG: &str = "--probe-capture";

/// The evdev code of Escape, for a keymap that cannot be read.
const KEY_ESC: u32 = 1;
/// Escape's keysym.
const XK_ESCAPE: u32 = 0xff1b;
/// The largest keymap the probe reads.
const KEYMAP_MAX: u32 = 1 << 20;

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
    eprintln!("usage: thinfloe-menu --deskflow --probe-capture <output> <left-of|right-of|above|below>");
    ExitCode::from(2)
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
    let mut keys: Option<Keymap> = None;
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
                CapEvent::Keymap { fd, size } => match Keymap::read(fd, *size) {
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
                    Some(k) => k.keysym(code) == Some(XK_ESCAPE),
                    None => code == KEY_ESC,
                };
                if escape {
                    cap.end_capture();
                }
            }
        }
    }
    Ok(())
}

/// The probe's line for one event; a key also shows its keysym's name,
/// "-" without a keymap.
fn probe_line(ev: &CapEvent, keys: Option<&Keymap>) -> String {
    let updown = |p: bool| if p { "down" } else { "up" };
    match ev {
        CapEvent::Enter { strip, x, y } => format!("enter {strip} {x:.0} {y:.0}"),
        CapEvent::Leave { strip } => format!("leave {strip}"),
        CapEvent::Relative { dx, dy } => format!("relative {dx:.2} {dy:.2}"),
        CapEvent::Locked => "locked".into(),
        CapEvent::Unlocked => "unlocked".into(),
        CapEvent::Key { code, pressed } => {
            let name = keys
                .and_then(|k| k.keysym(*code))
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

/// The compositor's keymap, compiled by libxkbcommon, for the probe's key
/// names.
struct Keymap {
    xkb: &'static xkbcommon_dl::XkbCommon,
    ctx: *mut xkbcommon_dl::xkb_context,
    keymap: *mut xkbcommon_dl::xkb_keymap,
}

impl Keymap {
    fn read(fd: &OwnedFd, size: u32) -> Result<Keymap, String> {
        let xkb = xkbcommon_dl::xkbcommon_option().ok_or("libxkbcommon is not available")?;
        if size == 0 || size > KEYMAP_MAX {
            return Err(format!("a keymap of {size} bytes"));
        }
        let file = File::from(fd.try_clone().map_err(|e| format!("dup: {e}"))?);
        let mut text = vec![0u8; size as usize];
        file.read_exact_at(&mut text, 0).map_err(|e| format!("read: {e}"))?;
        while text.last() == Some(&0) {
            text.pop();
        }
        text.push(0);
        let text = CStr::from_bytes_until_nul(&text).map_err(|e| e.to_string())?;
        // SAFETY: plain libxkbcommon calls on pointers checked for NULL;
        // the context and keymap are owned by the Keymap and freed in Drop.
        unsafe {
            let ctx = (xkb.xkb_context_new)(xkbcommon_dl::xkb_context_flags::XKB_CONTEXT_NO_FLAGS);
            if ctx.is_null() {
                return Err("xkb_context_new failed".into());
            }
            let keymap = (xkb.xkb_keymap_new_from_string)(
                ctx,
                text.as_ptr(),
                xkbcommon_dl::xkb_keymap_format::XKB_KEYMAP_FORMAT_TEXT_V1,
                xkbcommon_dl::xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS,
            );
            if keymap.is_null() {
                (xkb.xkb_context_unref)(ctx);
                return Err("the keymap does not compile".into());
            }
            Ok(Keymap { xkb, ctx, keymap })
        }
    }

    /// The first keysym of an evdev code, in the first layout and level.
    fn keysym(&self, code: u32) -> Option<u32> {
        let mut syms: *const u32 = std::ptr::null();
        // SAFETY: the keymap is valid; libxkbcommon points syms into it.
        let n = unsafe { (self.xkb.xkb_keymap_key_get_syms_by_level)(self.keymap, code + 8, 0, 0, &mut syms) };
        // SAFETY: n > 0 means syms points to n keysyms.
        (n > 0 && !syms.is_null()).then(|| unsafe { *syms })
    }
}

impl Drop for Keymap {
    fn drop(&mut self) {
        // SAFETY: both were made in Keymap::read and are freed only here.
        unsafe {
            (self.xkb.xkb_keymap_unref)(self.keymap);
            (self.xkb.xkb_context_unref)(self.ctx);
        }
    }
}

fn keysym_name(sym: u32) -> String {
    let Some(xkb) = xkbcommon_dl::xkbcommon_option() else {
        return format!("0x{sym:x}");
    };
    let mut buf = [0 as std::ffi::c_char; 64];
    // SAFETY: the buffer and its length match; libxkbcommon NUL-terminates.
    let n = unsafe { (xkb.xkb_keysym_get_name)(sym, buf.as_mut_ptr(), buf.len()) };
    if n <= 0 {
        return format!("0x{sym:x}");
    }
    // SAFETY: a NUL-terminated string in buf.
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_strip_lies_on_the_named_edge() {
        let out = OutputGeom {
            name: "HEADLESS-1".into(),
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
