//! `thinfloe-menu --osd`: the on-screen display of the volume and
//! brightness keys, in overlay surfaces above every window (`surface`).
//!
//! For now only `--osd --probe` exists, a test hook: it shows a plain
//! panel and mark on every output, at scale 1, until SIGTERM.

pub mod surface;

use std::ffi::OsString;
use std::os::fd::AsRawFd;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::Color32;
use surface::{SurfaceKind, Surfaces, mark_placement, panel_placement};

pub const OSD_ARG: &str = "--osd";
pub const OSD_KEY_ARG: &str = "--osd-key";
const PROBE_ARG: &str = "--probe";

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

fn xrgb(c: Color32) -> u32 {
    (u32::from(c.r()) << 16) | (u32::from(c.g()) << 8) | u32::from(c.b())
}

/// A `w`×`h` frame in `fill` with a 1 px border in `border`.
fn framed(w: u32, h: u32, fill: u32, border: u32) -> Vec<u32> {
    let (w, h) = (w as usize, h as usize);
    let mut px = vec![fill; w * h];
    for y in 0..h {
        for x in 0..w {
            if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                px[y * w + x] = border;
            }
        }
    }
    px
}

/// Shows the panel and the mark, plain fills from the theme, on every
/// output that has a name, again whenever the outputs change, until SIGTERM.
fn probe() -> Result<(), String> {
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGTERM, on_sigterm as *const () as libc::sighandler_t);
    }
    let theme = crate::brand::theme();
    let tile = xrgb(theme.palette().card);
    let line = xrgb(theme.palette().border);
    let warn = xrgb(theme.role(crate::brand::role::WARN));
    let panel = panel_placement(1.0, true);
    let mark = mark_placement(1.0);
    let panel_px = framed(panel.size.0, panel.size.1, tile, line);
    let mark_px = framed(mark.size.0, mark.size.1, tile, warn);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_framed_fill_has_its_border_on_the_outer_pixels_only() {
        let px = framed(4, 3, 1, 9);
        assert_eq!(px, vec![9, 9, 9, 9, 9, 1, 1, 9, 9, 9, 9, 9]);
    }
}
