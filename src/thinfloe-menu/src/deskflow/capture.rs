//! The Wayland side of the Deskflow server: one edge strip per attached
//! device, the pointer lock and the keyboard focus while a device has
//! them, and every input event in between (spec, Edge capture).
//!
//! A strip is a layer-shell surface on the overlay layer of its output,
//! anchored to one edge, 1 px deep, with exclusive zone 0, keyboard
//! interactivity none while idle, a transparent buffer and the whole strip
//! as its input region: the pointer enters it at the edge, and no key
//! reaches it until a capture begins.
//!
//! A capture first asks for a oneshot pointer lock on the strip and only
//! then sets keyboard interactivity exclusive: labwc 0.9.3 activates a lock
//! only for the active view (`create_constraint`) or when the keyboard
//! focus moves (`seat_focus`), so the lock must exist before the focus
//! moves to the strip. Until the compositor reports `Locked`, the capture
//! passes on no input event. Everything a capture holds is a Wayland object
//! of this connection, so a crash of the process gives pointer and keyboard
//! back.
//!
//! The relative pointer lives as long as the `wl_pointer` does, so motion
//! on a strip is reported before a capture too (edge resistance); the
//! compositor sends it only while the pointer is on one of our surfaces.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, BorrowedFd, FromRawFd, OwnedFd};

use wayland_client::backend::{Backend, WaylandError};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_region, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::{
    zwp_keyboard_shortcuts_inhibit_manager_v1, zwp_keyboard_shortcuts_inhibitor_v1,
};
use wayland_protocols::wp::pointer_constraints::zv1::client::{zwp_locked_pointer_v1, zwp_pointer_constraints_v1};
use wayland_protocols::wp::relative_pointer::zv1::client::{zwp_relative_pointer_manager_v1, zwp_relative_pointer_v1};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use wayland_protocols_wlr::virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1};

use super::edge::Rect;

/// One output in the compositor's logical layout: position and size from
/// `zxdg_output_v1` (logical, so scale and rotation are applied), name and
/// description from `wl_output` v4.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputGeom {
    pub name: String,
    /// wlroots makes it "<make> <model> <serial> (<name>)".
    pub description: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl OutputGeom {
    /// Whether this is the monitor a `[monitors]` criteria string names:
    /// the output name, or "<make> <model> <serial>" (`kanshi::criteria`),
    /// which starts the description.
    pub fn matches(&self, criteria: &str) -> bool {
        self.name == criteria
            || self
                .description
                .strip_prefix(criteria)
                .is_some_and(|rest| rest.starts_with(" ("))
    }
}

/// A strip to show: `rect` in the layout coordinates of `OutputGeom`,
/// along one edge of `output`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StripSpec {
    pub id: u32,
    pub output: String,
    pub rect: Rect,
}

/// What the compositor reported, in order. `Enter` is in layout
/// coordinates; key codes are evdev codes, as `wl_keyboard` sends them.
#[derive(Debug)]
pub enum CapEvent {
    Enter {
        strip: u32,
        x: f64,
        y: f64,
    },
    Leave {
        strip: u32,
    },
    /// The pointer moved on a strip, in layout coordinates.
    Motion {
        x: f64,
        y: f64,
    },
    /// Relative motion: `dx`, `dy` with the pointer acceleration, the
    /// `_unaccel` pair without.
    Relative {
        dx: f64,
        dy: f64,
        dx_unaccel: f64,
        dy_unaccel: f64,
    },
    Locked,
    /// The capture is over: the compositor cancelled the lock, it could
    /// not be made, or `end_capture` destroyed it (the compositor reports
    /// nothing for a lock the client destroys).
    Unlocked,
    Key {
        code: u32,
        pressed: bool,
    },
    Modifiers {
        depressed: u32,
        latched: u32,
        locked: u32,
        group: u32,
    },
    Keymap {
        fd: OwnedFd,
        size: u32,
    },
    RepeatInfo {
        rate: i32,
        delay: i32,
    },
    Button {
        button: u32,
        pressed: bool,
    },
    /// One pointer frame's wheel motion in 1/120 steps.
    Axis {
        v120_x: i32,
        v120_y: i32,
    },
    KeyboardLeft,
    OutputsChanged,
}

impl CapEvent {
    /// Input that a capture passes on only once the lock is active.
    fn is_input(&self) -> bool {
        matches!(
            self,
            CapEvent::Relative { .. } | CapEvent::Key { .. } | CapEvent::Button { .. } | CapEvent::Axis { .. }
        )
    }
}

/// Whether an event reaches the caller: while a capture waits for its lock
/// (`captured` and not `locked`), no input does, so nothing typed or
/// clicked before the lock reaches a device, and a key pressed then is
/// never released on it either.
pub fn passes(captured: bool, locked: bool, ev: &CapEvent) -> bool {
    !(ev.is_input() && captured && !locked)
}

type Margins = (i32, i32, i32, i32);

/// Where a layer surface for `rect` sits on `out`: anchor, margins (top,
/// right, bottom, left) and size. A strip taller than wide is on the left
/// or right edge, else on the top or bottom one; it is on the far edge
/// when it starts past the output's middle.
fn placement(out: &OutputGeom, rect: &Rect) -> (zwlr_layer_surface_v1::Anchor, Margins, (u32, u32)) {
    use zwlr_layer_surface_v1::Anchor;
    let size = (rect.w.max(1) as u32, rect.h.max(1) as u32);
    if rect.h > rect.w {
        let side = if rect.x - out.x >= out.w / 2 {
            Anchor::Right
        } else {
            Anchor::Left
        };
        (side | Anchor::Top, (rect.y - out.y, 0, 0, 0), size)
    } else {
        let side = if rect.y - out.y >= out.h / 2 {
            Anchor::Bottom
        } else {
            Anchor::Top
        };
        (side | Anchor::Left, (0, 0, 0, rect.x - out.x), size)
    }
}

/// Output-local pixel-center coordinates `(x, y, x_extent, y_extent)` for
/// `zwlr_virtual_pointer_v1::motion_absolute`, mapping pixel `p` on length
/// `len` to `(2*p + 1) / (2*len)` so `1279` on `1280` stays inside pixel `1279`.
pub fn virtual_pointer_coords(out: &OutputGeom, layout: (f64, f64)) -> Option<(u32, u32, u32, u32)> {
    if out.w <= 0 || out.h <= 0 || !layout.0.is_finite() || !layout.1.is_finite() {
        return None;
    }
    let ox = (layout.0 - f64::from(out.x)).clamp(0.0, f64::from(out.w - 1));
    let oy = (layout.1 - f64::from(out.y)).clamp(0.0, f64::from(out.h - 1));
    Some((
        (ox.round() as u32) * 2 + 1,
        (oy.round() as u32) * 2 + 1,
        (out.w as u32) * 2,
        (out.h as u32) * 2,
    ))
}

/// Full-layout bounding rectangle across outputs with positive size, used when
/// `zwlr_virtual_pointer_manager_v1` is below version 2 (no output binding).
fn layout_bounding_box(outputs: &[OutputGeom]) -> Option<OutputGeom> {
    let mut iter = outputs.iter().filter(|o| o.w > 0 && o.h > 0);
    let first = iter.next()?;
    let (mut min_x, mut min_y) = (i64::from(first.x), i64::from(first.y));
    let (mut max_x, mut max_y) = (min_x + i64::from(first.w), min_y + i64::from(first.h));
    for o in iter {
        min_x = min_x.min(i64::from(o.x));
        min_y = min_y.min(i64::from(o.y));
        max_x = max_x.max(i64::from(o.x) + i64::from(o.w));
        max_y = max_y.max(i64::from(o.y) + i64::from(o.h));
    }
    Some(OutputGeom {
        name: String::new(),
        description: String::new(),
        x: i32::try_from(min_x).ok()?,
        y: i32::try_from(min_y).ok()?,
        w: i32::try_from(max_x - min_x).ok()?,
        h: i32::try_from(max_y - min_y).ok()?,
    })
}

fn virtual_pointer_layout_coords(outputs: &[OutputGeom], layout: (f64, f64)) -> Option<(u32, u32, u32, u32)> {
    let bounds = layout_bounding_box(outputs)?;
    virtual_pointer_coords(&bounds, layout)
}

struct Output {
    wl: wl_output::WlOutput,
    xdg: zxdg_output_v1::ZxdgOutputV1,
    name: Option<String>,
    description: String,
    pos: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
}

impl Output {
    fn geom(&self) -> Option<OutputGeom> {
        let ((x, y), (w, h)) = (self.pos?, self.size?);
        Some(OutputGeom {
            name: self.name.clone()?,
            description: self.description.clone(),
            x,
            y,
            w,
            h,
        })
    }
}

struct Strip {
    spec: StripSpec,
    surface: wl_surface::WlSurface,
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    size: (u32, u32),
}

impl Strip {
    fn destroy(self) {
        self.layer.destroy();
        self.surface.destroy();
    }
}

/// The wheel motion of the current pointer frame.
#[derive(Default)]
struct Wheel {
    v120: (i32, i32),
    /// `axis_discrete` steps, all a seat before version 8 sends.
    discrete: (i32, i32),
    /// Continuous scrolling (touchpads, virtual pointers) in surface
    /// pixels, when the frame has neither of the above.
    continuous: (f64, f64),
}

/// Continuous scrolling per wheel step: libinput's 15 px.
const PX_PER_STEP: f64 = 15.0;

/// A strip the compositor closed is made again this long after, so a
/// compositor that keeps closing it does not keep the loop busy.
const RECREATE_AFTER: std::time::Duration = std::time::Duration::from_secs(1);

/// A strip closed at `closed_at` counts as missing by `now`.
fn recreate_due(closed_at: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    closed_at.is_some_and(|t| now.saturating_duration_since(t) >= RECREATE_AFTER)
}

impl Wheel {
    /// The frame's motion in 1/120 steps.
    fn v120(&self) -> (i32, i32) {
        if self.v120 != (0, 0) {
            return self.v120;
        }
        if self.discrete != (0, 0) {
            return (self.discrete.0.saturating_mul(120), self.discrete.1.saturating_mul(120));
        }
        // `as` saturates and maps NaN to 0.
        let step = |px: f64| (px * 120.0 / PX_PER_STEP).round() as i32;
        (step(self.continuous.0), step(self.continuous.1))
    }
}

struct State {
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1,
    xdg_outputs: zxdg_output_manager_v1::ZxdgOutputManagerV1,
    constraints: zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
    relative_mgr: zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
    /// None on a compositor without it: the capture works, but the
    /// compositor's media keys still run while it is on.
    inhibit_mgr: Option<zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1>,
    virtual_pointer_mgr: Option<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1>,
    seat: wl_seat::WlSeat,
    pointer: Option<wl_pointer::WlPointer>,
    relative: Option<zwp_relative_pointer_v1::ZwpRelativePointerV1>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    /// By the output's global name.
    outputs: BTreeMap<u32, Output>,
    strips: Vec<Strip>,
    /// The strip under the pointer.
    hover: Option<u32>,
    /// The strip being captured, its lock, and whether the lock is active.
    captured: Option<u32>,
    lock: Option<zwp_locked_pointer_v1::ZwpLockedPointerV1>,
    locked: bool,
    /// The captured strip's shortcuts inhibitor: ThinFloe's labwc runs no
    /// keybinding, not even a media key, while the strip has the keyboard
    /// (patch 0007), so every key goes to the device.
    inhibitor: Option<zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1>,
    wheel: Wheel,
    events: Vec<CapEvent>,
    outputs_changed: bool,
    /// When the compositor last closed a strip, until it is made again.
    closed_at: Option<std::time::Instant>,
}

pub struct Capture {
    conn: Connection,
    backend: Backend,
    queue: EventQueue<State>,
    state: State,
}

impl Capture {
    pub fn connect() -> Result<Capture, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland connection: {e}"))?;
        let (globals, mut queue): (_, EventQueue<State>) =
            registry_queue_init(&conn).map_err(|e| format!("Wayland registry: {e}"))?;
        let qh = queue.handle();
        let err = |what: &'static str| move |e| format!("{what}: {e}");
        let mut state = State {
            compositor: globals.bind(&qh, 4..=4, ()).map_err(err("wl_compositor v4"))?,
            shm: globals.bind(&qh, 1..=1, ()).map_err(err("wl_shm"))?,
            layer_shell: globals.bind(&qh, 1..=4, ()).map_err(err("zwlr_layer_shell_v1"))?,
            xdg_outputs: globals.bind(&qh, 2..=3, ()).map_err(err("zxdg_output_manager_v1 v2"))?,
            constraints: globals
                .bind(&qh, 1..=1, ())
                .map_err(err("zwp_pointer_constraints_v1"))?,
            relative_mgr: globals
                .bind(&qh, 1..=1, ())
                .map_err(err("zwp_relative_pointer_manager_v1"))?,
            inhibit_mgr: globals.bind(&qh, 1..=1, ()).ok(),
            virtual_pointer_mgr: globals.bind(&qh, 1..=2, ()).ok(),
            seat: globals.bind(&qh, 5..=8, ()).map_err(err("wl_seat v5"))?,
            pointer: None,
            relative: None,
            keyboard: None,
            outputs: BTreeMap::new(),
            strips: Vec::new(),
            hover: None,
            captured: None,
            lock: None,
            locked: false,
            inhibitor: None,
            wheel: Wheel::default(),
            events: Vec::new(),
            outputs_changed: false,
            closed_at: None,
        };
        if state.inhibit_mgr.is_none() {
            eprintln!(
                "thinfloe-menu --deskflow: no zwp_keyboard_shortcuts_inhibit_manager_v1: the compositor's keys still act while a device has the keyboard"
            );
        }
        for g in globals.contents().clone_list() {
            if g.interface == "wl_output" {
                state.bind_output(globals.registry(), g.name, g.version, &qh);
            }
        }
        // The outputs' names and logical geometry, the seat's capabilities.
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("Wayland roundtrip: {e}"))?;
        state.outputs_changed = false;
        let backend = conn.backend();
        Ok(Capture {
            conn,
            backend,
            queue,
            state,
        })
    }

    /// The connection's file descriptor, to poll for readable events.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.backend.poll_fd()
    }

    /// Reads what the compositor sent, without blocking, and returns what
    /// happened since the last call.
    pub fn dispatch(&mut self) -> Result<Vec<CapEvent>, String> {
        self.flush()?;
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| format!("Wayland dispatch: {e}"))?;
        if let Some(guard) = self.queue.prepare_read() {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(format!("Wayland read: {e}")),
            }
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| format!("Wayland dispatch: {e}"))?;
        if std::mem::take(&mut self.state.outputs_changed) {
            self.state.events.push(CapEvent::OutputsChanged);
        }
        Ok(std::mem::take(&mut self.state.events))
    }

    pub fn flush(&mut self) -> Result<(), String> {
        match self.conn.flush() {
            Ok(()) => Ok(()),
            Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(format!("Wayland flush: {e}")),
        }
    }

    /// The outputs whose name and logical geometry are known.
    pub fn outputs(&self) -> Vec<OutputGeom> {
        self.state.outputs.values().filter_map(Output::geom).collect()
    }

    /// Show exactly these strips: a strip whose spec is unchanged stays, one
    /// that changed is made again, one not listed goes (and ends a capture
    /// on it). A strip on an output that is not known is left out.
    pub fn set_strips(&mut self, strips: &[StripSpec]) {
        let qh = self.queue.handle();
        let (keep, gone): (Vec<Strip>, Vec<Strip>) = std::mem::take(&mut self.state.strips)
            .into_iter()
            .partition(|s| strips.contains(&s.spec));
        self.state.strips = keep;
        for s in gone {
            self.state.remove_strip(s);
        }
        for spec in strips {
            if self.state.strips.iter().any(|s| s.spec.id == spec.id) {
                continue;
            }
            if let Some(strip) = self.state.make_strip(spec, &qh) {
                self.state.strips.push(strip);
            }
        }
    }

    /// True once, when a strip the compositor closed is due to be made
    /// again: the caller passes its strips to `set_strips` then, although
    /// they did not change.
    pub fn take_closed(&mut self, now: std::time::Instant) -> bool {
        if recreate_due(self.state.closed_at, now) {
            self.state.closed_at = None;
            return true;
        }
        false
    }

    /// Take pointer and keyboard on `strip`: the lock first, then the
    /// exclusive keyboard focus. A capture that cannot start (no such
    /// strip, no pointer) reports `Unlocked` at once.
    pub fn begin_capture(&mut self, strip: u32) {
        let s = &mut self.state;
        if s.captured.is_some() {
            return;
        }
        let (Some(st), Some(pointer)) = (s.strips.iter().find(|x| x.spec.id == strip), s.pointer.as_ref()) else {
            s.events.push(CapEvent::Unlocked);
            return;
        };
        let qh = self.queue.handle();
        s.lock = Some(s.constraints.lock_pointer(
            &st.surface,
            pointer,
            None,
            zwp_pointer_constraints_v1::Lifetime::Oneshot,
            &qh,
            (),
        ));
        st.layer
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
        s.inhibitor = s
            .inhibit_mgr
            .as_ref()
            .map(|m| m.inhibit_shortcuts(&st.surface, &s.seat, &qh, ()));
        st.surface.commit();
        s.captured = Some(strip);
        s.locked = false;
    }

    /// Give pointer and keyboard back: the lock goes and the strip takes
    /// no keys again. Reports `Unlocked` when a capture was on, and then
    /// returns true. When `warp` is given (in layout coordinates) and the
    /// lock was active, warps the pointer there with `zwlr_virtual_pointer_v1`
    /// after releasing the lock.
    pub fn end_capture(&mut self, warp: Option<(f64, f64)>) -> bool {
        let qh = self.queue.handle();
        self.state.end_capture(warp, Some(&qh))
    }
}

/// The time of an input event we send, in milliseconds of CLOCK_MONOTONIC
/// (wrapping at 32 bits): the clock wlroots, and so labwc, stamps its own
/// pointer events with.
fn input_time_ms() -> u32 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: ts is a valid timespec for clock_gettime to fill.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
        return 0;
    }
    (ts.tv_sec as u64)
        .wrapping_mul(1000)
        .wrapping_add(ts.tv_nsec as u64 / 1_000_000) as u32
}

impl State {
    fn end_capture(&mut self, warp: Option<(f64, f64)>, qh: Option<&QueueHandle<State>>) -> bool {
        let Some(id) = self.captured.take() else {
            return false;
        };
        let was_locked = self.locked;
        if let Some(lock) = self.lock.take() {
            lock.destroy();
        }
        if let Some(inhibitor) = self.inhibitor.take() {
            inhibitor.destroy();
        }
        self.locked = false;
        let strip_output = if let Some(st) = self.strips.iter().find(|x| x.spec.id == id) {
            st.layer
                .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
            st.surface.commit();
            Some(st.spec.output.clone())
        } else {
            None
        };
        if was_locked
            && let Some(layout) = warp
            && let Some(qh) = qh
            && let Some(out_name) = strip_output
        {
            self.warp_cursor(&out_name, layout, qh);
        }
        self.events.push(CapEvent::Unlocked);
        true
    }

    fn warp_cursor(&self, output: &str, layout: (f64, f64), qh: &QueueHandle<State>) {
        let Some(mgr) = self.virtual_pointer_mgr.as_ref() else {
            return;
        };
        let (vp, (x, y, x_ext, y_ext)) = if mgr.version() >= 2 {
            let Some(out) = self.outputs.values().find(|o| o.name.as_deref() == Some(output)) else {
                return;
            };
            let Some(coords) = out.geom().and_then(|g| virtual_pointer_coords(&g, layout)) else {
                return;
            };
            (
                mgr.create_virtual_pointer_with_output(Some(&self.seat), Some(&out.wl), qh, ()),
                coords,
            )
        } else {
            let geoms: Vec<OutputGeom> = self.outputs.values().filter_map(Output::geom).collect();
            let Some(coords) = virtual_pointer_layout_coords(&geoms, layout) else {
                return;
            };
            (mgr.create_virtual_pointer(Some(&self.seat), qh, ()), coords)
        };
        // Daemon::release_at also sets `hover.at` to this point itself: the
        // next capture's entry point (CINN) is taken from `hover.at`, and
        // the re-entry design (2026-11-22) has it match where the cursor
        // re-enters rather than where it left for the device.
        vp.motion_absolute(input_time_ms(), x, y, x_ext, y_ext);
        vp.frame();
        vp.destroy();
    }

    /// An event for the caller; input during a capture waits for the lock.
    fn push(&mut self, e: CapEvent) {
        if passes(self.captured.is_some(), self.locked, &e) {
            self.events.push(e);
        }
    }

    fn strip_of(&self, surface: &wl_surface::WlSurface) -> Option<&Strip> {
        self.strips.iter().find(|s| s.surface == *surface)
    }

    fn remove_strip(&mut self, s: Strip) {
        if self.captured == Some(s.spec.id) {
            self.end_capture(None, None);
        }
        if self.hover == Some(s.spec.id) {
            self.hover = None;
        }
        s.destroy();
    }

    fn make_strip(&mut self, spec: &StripSpec, qh: &QueueHandle<State>) -> Option<Strip> {
        let out = self
            .outputs
            .values()
            .find(|o| o.name.as_deref() == Some(&spec.output))?;
        let (anchor, (top, right, bottom, left), size) = placement(&out.geom()?, &spec.rect);
        let surface = self.compositor.create_surface(qh, ());
        let layer = self.layer_shell.get_layer_surface(
            &surface,
            Some(&out.wl),
            zwlr_layer_shell_v1::Layer::Overlay,
            "thinfloe-deskflow".into(),
            qh,
            spec.id,
        );
        layer.set_size(size.0, size.1);
        layer.set_anchor(anchor);
        layer.set_margin(top, right, bottom, left);
        layer.set_exclusive_zone(0);
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        let region = self.compositor.create_region(qh, ());
        region.add(0, 0, size.0 as i32, size.1 as i32);
        surface.set_input_region(Some(&region));
        region.destroy();
        surface.commit();
        Some(Strip {
            spec: spec.clone(),
            surface,
            layer,
            size,
        })
    }

    /// A fully transparent ARGB buffer of the strip's size.
    fn attach_clear(&self, strip: &Strip, qh: &QueueHandle<State>) -> Result<(), String> {
        let (w, h) = (strip.size.0 as i32, strip.size.1 as i32);
        let file = clear_file(w as usize * h as usize * 4)?;
        let pool = self.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, BufferFile { _file: file });
        pool.destroy();
        strip.surface.attach(Some(&buffer), 0, 0);
        strip.surface.damage_buffer(0, 0, w, h);
        strip.surface.commit();
        Ok(())
    }

    fn bind_output(&mut self, registry: &wl_registry::WlRegistry, id: u32, version: u32, qh: &QueueHandle<State>) {
        // Only v4 sends the name strips are placed by.
        if version < 4 {
            return;
        }
        let wl: wl_output::WlOutput = registry.bind(id, 4, qh, id);
        let xdg = self.xdg_outputs.get_xdg_output(&wl, qh, id);
        self.outputs.insert(
            id,
            Output {
                wl,
                xdg,
                name: None,
                description: String::new(),
                pos: None,
                size: None,
            },
        );
        self.outputs_changed = true;
    }

    fn drop_output(&mut self, id: u32) {
        let Some(out) = self.outputs.remove(&id) else {
            return;
        };
        out.xdg.destroy();
        out.wl.release();
        let (gone, keep): (Vec<Strip>, Vec<Strip>) = std::mem::take(&mut self.strips)
            .into_iter()
            .partition(|s| out.name.as_ref() == Some(&s.spec.output));
        self.strips = keep;
        for s in gone {
            self.remove_strip(s);
        }
        self.outputs_changed = true;
    }

    fn capabilities(&mut self, caps: wl_seat::Capability, qh: &QueueHandle<State>) {
        let has_pointer = caps.contains(wl_seat::Capability::Pointer);
        if has_pointer && self.pointer.is_none() {
            let p = self.seat.get_pointer(qh, ());
            self.relative = Some(self.relative_mgr.get_relative_pointer(&p, qh, ()));
            self.pointer = Some(p);
        } else if !has_pointer && let Some(p) = self.pointer.take() {
            if let Some(r) = self.relative.take() {
                r.destroy();
            }
            p.release();
        }
        let has_keyboard = caps.contains(wl_seat::Capability::Keyboard);
        if has_keyboard && self.keyboard.is_none() {
            self.keyboard = Some(self.seat.get_keyboard(qh, ()));
        } else if !has_keyboard && let Some(k) = self.keyboard.take() {
            k.release();
        }
    }
}

/// A memfd of `len` zero bytes: transparent ARGB8888.
fn clear_file(len: usize) -> Result<File, String> {
    let name = CString::new("thinfloe-deskflow").expect("no NUL");
    // SAFETY: a valid C string; the returned descriptor is checked and
    // owned by the File from here on.
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: fd is a fresh descriptor nothing else owns.
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(&vec![0u8; len])
        .map_err(|e| format!("memfd write: {e}"))?;
    Ok(file)
}

/// The memfd behind a buffer, closed with the buffer.
struct BufferFile {
    _file: File,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => state.bind_output(registry, name, version, qh),
            wl_registry::Event::GlobalRemove { name } => state.drop_output(name),
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(out) = state.outputs.get_mut(id) else {
            return;
        };
        match event {
            wl_output::Event::Name { name } => out.name = Some(name),
            wl_output::Event::Description { description } => out.description = description,
            _ => return,
        }
        state.outputs_changed = true;
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(out) = state.outputs.get_mut(id) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } if out.pos != Some((x, y)) => {
                out.pos = Some((x, y));
                state.outputs_changed = true;
            }
            zxdg_output_v1::Event::LogicalSize { width, height } if out.size != Some((width, height)) => {
                out.size = Some((width, height));
                state.outputs_changed = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, u32> for State {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        id: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        // An event for a strip already replaced or dropped.
        let Some(pos) = state.strips.iter().position(|s| s.layer == *layer) else {
            return;
        };
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                layer.ack_configure(serial);
                // The strip's size is ours; each configure gets a fresh buffer.
                if let Err(e) = state.attach_clear(&state.strips[pos], qh) {
                    eprintln!("thinfloe-menu --deskflow: strip {id}: {e}");
                }
            }
            // Gone without being asked: `set_strips` makes it again once
            // `take_closed` says so.
            zwlr_layer_surface_v1::Event::Closed => {
                let s = state.strips.remove(pos);
                state.remove_strip(s);
                state.closed_at.get_or_insert_with(std::time::Instant::now);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            state.capabilities(caps, qh);
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wl_pointer::{Axis, ButtonState, Event};
        match event {
            Event::Enter {
                surface,
                surface_x,
                surface_y,
                ..
            } => {
                if let Some(s) = state.strip_of(&surface) {
                    let (strip, r) = (s.spec.id, s.spec.rect);
                    let (x, y) = (f64::from(r.x) + surface_x, f64::from(r.y) + surface_y);
                    state.hover = Some(strip);
                    state.push(CapEvent::Enter { strip, x, y });
                }
            }
            Event::Leave { surface, .. } => {
                if let Some(s) = state.strip_of(&surface) {
                    let strip = s.spec.id;
                    if state.hover == Some(strip) {
                        state.hover = None;
                    }
                    state.push(CapEvent::Leave { strip });
                }
            }
            Event::Motion {
                surface_x, surface_y, ..
            } => {
                let at = state.hover.and_then(|h| state.strips.iter().find(|s| s.spec.id == h));
                if let Some(r) = at.map(|s| s.spec.rect) {
                    let (x, y) = (f64::from(r.x) + surface_x, f64::from(r.y) + surface_y);
                    state.push(CapEvent::Motion { x, y });
                }
            }
            Event::Button { button, state: b, .. } => {
                let pressed = matches!(b, WEnum::Value(ButtonState::Pressed));
                state.push(CapEvent::Button { button, pressed });
            }
            Event::AxisValue120 { axis, value120 } => match axis {
                WEnum::Value(Axis::HorizontalScroll) => state.wheel.v120.0 += value120,
                WEnum::Value(Axis::VerticalScroll) => state.wheel.v120.1 += value120,
                _ => {}
            },
            Event::Axis { axis, value, .. } => match axis {
                WEnum::Value(Axis::HorizontalScroll) => state.wheel.continuous.0 += value,
                WEnum::Value(Axis::VerticalScroll) => state.wheel.continuous.1 += value,
                _ => {}
            },
            Event::AxisDiscrete { axis, discrete } => match axis {
                WEnum::Value(Axis::HorizontalScroll) => state.wheel.discrete.0 += discrete,
                WEnum::Value(Axis::VerticalScroll) => state.wheel.discrete.1 += discrete,
                _ => {}
            },
            Event::Frame => {
                let (x, y) = std::mem::take(&mut state.wheel).v120();
                if (x, y) != (0, 0) {
                    state.push(CapEvent::Axis { v120_x: x, v120_y: y });
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_relative_pointer_v1::ZwpRelativePointerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &zwp_relative_pointer_v1::ZwpRelativePointerV1,
        event: zwp_relative_pointer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_relative_pointer_v1::Event::RelativeMotion {
            dx,
            dy,
            dx_unaccel,
            dy_unaccel,
            ..
        } = event
        {
            state.push(CapEvent::Relative {
                dx,
                dy,
                dx_unaccel,
                dy_unaccel,
            });
        }
    }
}

impl Dispatch<zwp_locked_pointer_v1::ZwpLockedPointerV1, ()> for State {
    fn event(
        state: &mut Self,
        lock: &zwp_locked_pointer_v1::ZwpLockedPointerV1,
        event: zwp_locked_pointer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if state.lock.as_ref() != Some(lock) {
            return;
        }
        match event {
            zwp_locked_pointer_v1::Event::Locked => {
                state.locked = true;
                state.events.push(CapEvent::Locked);
            }
            // A oneshot lock is dead once it is deactivated.
            zwp_locked_pointer_v1::Event::Unlocked => {
                state.end_capture(None, None);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wl_keyboard::{Event, KeyState, KeymapFormat};
        match event {
            Event::Keymap {
                format: WEnum::Value(KeymapFormat::XkbV1),
                fd,
                size,
            } => state.events.push(CapEvent::Keymap { fd, size }),
            // The keys held when the focus arrives are not passed on.
            Event::Enter { .. } => {}
            Event::Leave { .. } => state.events.push(CapEvent::KeyboardLeft),
            Event::Key { key, state: k, .. } => {
                let pressed = match k {
                    WEnum::Value(KeyState::Pressed) => true,
                    WEnum::Value(KeyState::Released) => false,
                    // A compositor-side repeat (wl_seat v10): not bound here.
                    _ => return,
                };
                state.push(CapEvent::Key { code: key, pressed });
            }
            Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => state.events.push(CapEvent::Modifiers {
                depressed: mods_depressed,
                latched: mods_latched,
                locked: mods_locked,
                group,
            }),
            Event::RepeatInfo { rate, delay } => state.events.push(CapEvent::RepeatInfo { rate, delay }),
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, BufferFile> for State {
    fn event(
        _: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &BufferFile,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            buffer.destroy();
        }
    }
}

macro_rules! no_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for State {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}

no_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_region::WlRegion,
    wl_surface::WlSurface,
    zwlr_layer_shell_v1::ZwlrLayerShellV1,
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
    zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
    zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1,
    zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1
);

#[cfg(test)]
mod tests {
    use super::*;
    use zwlr_layer_surface_v1::Anchor;

    /// The warp's timestamp: milliseconds that advance with time, not 0.
    #[test]
    fn input_time_counts_milliseconds() {
        let a = input_time_ms();
        std::thread::sleep(std::time::Duration::from_millis(30));
        let b = input_time_ms();
        let d = b.wrapping_sub(a);
        assert!((30..1000).contains(&d), "{a} -> {b}");
        assert_ne!(b, 0);
    }

    fn out() -> OutputGeom {
        OutputGeom {
            name: "HEADLESS-1".into(),
            description: "Dell Inc. DELL U2720Q 7XK0T13 (HEADLESS-1)".into(),
            x: 1280,
            y: 0,
            w: 1920,
            h: 1080,
        }
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn a_closed_strip_counts_as_missing_a_second_later() {
        let t = std::time::Instant::now();
        let ms = std::time::Duration::from_millis;
        assert!(!recreate_due(None, t + ms(5000)), "nothing closed");
        assert!(!recreate_due(Some(t), t + ms(999)));
        assert!(recreate_due(Some(t), t + ms(1000)));
        assert!(recreate_due(Some(t), t + ms(60_000)));
    }

    #[test]
    fn a_strip_is_anchored_to_the_edge_it_lies_on() {
        assert_eq!(
            placement(&out(), &rect(1280 + 1919, 0, 1, 1080)),
            (Anchor::Right | Anchor::Top, (0, 0, 0, 0), (1, 1080))
        );
        assert_eq!(
            placement(&out(), &rect(1280, 100, 1, 500)),
            (Anchor::Left | Anchor::Top, (100, 0, 0, 0), (1, 500))
        );
        assert_eq!(
            placement(&out(), &rect(1280, 0, 1920, 1)),
            (Anchor::Top | Anchor::Left, (0, 0, 0, 0), (1920, 1))
        );
        assert_eq!(
            placement(&out(), &rect(1380, 1079, 800, 1)),
            (Anchor::Bottom | Anchor::Left, (0, 0, 0, 100), (800, 1))
        );
    }

    #[test]
    fn only_motion_keys_buttons_and_the_wheel_wait_for_the_lock() {
        assert!(
            CapEvent::Key {
                code: 45,
                pressed: true
            }
            .is_input()
        );
        assert!(
            CapEvent::Relative {
                dx: 1.0,
                dy: 0.0,
                dx_unaccel: 1.0,
                dy_unaccel: 0.0
            }
            .is_input()
        );
        assert!(!CapEvent::Motion { x: 1.0, y: 2.0 }.is_input());
        assert!(
            CapEvent::Button {
                button: 272,
                pressed: true
            }
            .is_input()
        );
        assert!(CapEvent::Axis { v120_x: 0, v120_y: 120 }.is_input());
        assert!(!CapEvent::Locked.is_input());
        assert!(
            !CapEvent::Enter {
                strip: 1,
                x: 0.0,
                y: 0.0
            }
            .is_input()
        );
        let m = CapEvent::Modifiers {
            depressed: 0,
            latched: 0,
            locked: 0,
            group: 0,
        };
        assert!(!m.is_input());
    }

    #[test]
    fn input_waits_for_the_lock_and_nothing_else_does() {
        let key = CapEvent::Key {
            code: 30,
            pressed: true,
        };
        let wheel = CapEvent::Axis { v120_x: 0, v120_y: 120 };
        // Idle: motion and keys pass (edge resistance needs the motion).
        assert!(passes(false, false, &key));
        // Waiting for the lock: no input.
        assert!(!passes(true, false, &key));
        assert!(!passes(true, false, &wheel));
        // Locked: all of it.
        assert!(passes(true, true, &key));
        assert!(passes(true, true, &wheel));
        // The lock itself, the keymap and the modifiers always pass.
        for ev in [
            CapEvent::Locked,
            CapEvent::Unlocked,
            CapEvent::KeyboardLeft,
            CapEvent::RepeatInfo { rate: 25, delay: 600 },
        ] {
            assert!(passes(true, false, &ev), "{ev:?}");
        }
    }

    #[test]
    fn an_output_matches_its_name_or_its_kanshi_criteria() {
        let o = out();
        assert!(o.matches("HEADLESS-1"));
        assert!(o.matches("Dell Inc. DELL U2720Q 7XK0T13"));
        assert!(!o.matches("Dell Inc. DELL U2720Q"));
        assert!(!o.matches("HEADLESS-2"));
        assert!(!o.matches(""));
    }

    #[test]
    fn the_wheel_prefers_v120_then_steps_then_pixels() {
        let w = Wheel {
            v120: (0, 60),
            discrete: (0, 1),
            continuous: (0.0, 15.0),
        };
        assert_eq!(w.v120(), (0, 60));
        let w = Wheel {
            discrete: (-1, 2),
            continuous: (3.0, 15.0),
            ..Wheel::default()
        };
        assert_eq!(w.v120(), (-120, 240));
        let w = Wheel {
            continuous: (-7.5, 30.0),
            ..Wheel::default()
        };
        assert_eq!(w.v120(), (-60, 240));
        let w = Wheel {
            continuous: (f64::NAN, f64::INFINITY),
            ..Wheel::default()
        };
        assert_eq!(w.v120(), (0, i32::MAX));
    }

    #[test]
    fn virtual_pointer_coords_maps_edges_clamps_and_rejects_invalid() {
        let single = OutputGeom {
            name: "HEADLESS-1".into(),
            description: String::new(),
            x: 0,
            y: 0,
            w: 1280,
            h: 720,
        };
        assert_eq!(
            virtual_pointer_coords(&single, (1279.0, 360.0)),
            Some((2559, 721, 2560, 1440))
        );
        assert_eq!(
            virtual_pointer_coords(&single, (0.0, 180.0)),
            Some((1, 361, 2560, 1440))
        );
        assert_eq!(
            virtual_pointer_coords(&single, (640.0, 0.0)),
            Some((1281, 1, 2560, 1440))
        );
        assert_eq!(
            virtual_pointer_coords(&single, (640.0, 719.0)),
            Some((1281, 1439, 2560, 1440))
        );

        let offset = OutputGeom {
            name: "DP-1".into(),
            description: String::new(),
            x: 1280,
            y: 100,
            w: 1920,
            h: 1080,
        };
        assert_eq!(
            virtual_pointer_coords(&offset, (3199.0, 640.0)),
            Some((3839, 1081, 3840, 2160))
        );
        assert_eq!(
            virtual_pointer_coords(&offset, (1280.0, 100.0)),
            Some((1, 1, 3840, 2160))
        );
        assert_eq!(
            virtual_pointer_coords(&offset, (2240.0, 1179.0)),
            Some((1921, 2159, 3840, 2160))
        );
        assert_eq!(
            virtual_pointer_coords(&offset, (3199.0, 100.0)),
            Some((3839, 1, 3840, 2160))
        );

        // Out-of-bounds layout coordinates clamp inside the output.
        assert_eq!(
            virtual_pointer_coords(&offset, (1200.0, 50.0)),
            Some((1, 1, 3840, 2160))
        );
        assert_eq!(
            virtual_pointer_coords(&offset, (5000.0, 5000.0)),
            Some((3839, 2159, 3840, 2160))
        );

        // Non-finite coordinates and non-positive output dimensions return None.
        assert_eq!(virtual_pointer_coords(&single, (f64::NAN, 360.0)), None);
        assert_eq!(virtual_pointer_coords(&single, (100.0, f64::INFINITY)), None);
        assert_eq!(virtual_pointer_coords(&single, (f64::NEG_INFINITY, 100.0)), None);
        assert_eq!(
            virtual_pointer_coords(&OutputGeom { w: 0, ..single.clone() }, (100.0, 100.0)),
            None
        );
        assert_eq!(
            virtual_pointer_coords(&OutputGeom { h: -1, ..single }, (100.0, 100.0)),
            None
        );
    }

    #[test]
    fn virtual_pointer_layout_coords_spans_multi_monitor_bounding_box() {
        let a = OutputGeom {
            name: "DP-1".into(),
            description: String::new(),
            x: -1920,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let b = OutputGeom {
            name: "DP-2".into(),
            description: String::new(),
            x: 0,
            y: 180,
            w: 1280,
            h: 720,
        };
        assert_eq!(
            virtual_pointer_layout_coords(&[a, b], (1279.0, 600.0)),
            Some((6399, 1201, 6400, 2160))
        );
        assert_eq!(virtual_pointer_layout_coords(&[], (0.0, 0.0)), None);
    }
}
