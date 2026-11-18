//! `thinfloe-menu --osd`: the on-screen display of the volume and
//! brightness keys, in overlay surfaces above every window (`surface`).
//!
//! For now only `--osd --probe` exists, a test hook: it shows a Volume
//! 45 % panel and the mark (`draw`) on every output, at scale 1, until
//! SIGTERM.

pub mod draw;
pub mod surface;

use std::ffi::OsString;
use std::os::fd::AsRawFd;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use draw::{Painter, PanelView};
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
