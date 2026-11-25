//! The on-screen display's Wayland surfaces: one layer-shell surface per
//! kind (panel, mark) on each `wl_output`, in the overlay layer, above a
//! fullscreen window. They take no keyboard focus (keyboard
//! interactivity none), reserve no space (exclusive zone -1) and no
//! pointer input (an empty input region), so a click goes through them to
//! the window below.
//!
//! Each draw is one memfd, one `wl_shm_pool` and one `wl_buffer`, which is
//! destroyed when the compositor releases it. An output whose `name` event
//! (`wl_output` v4) has not arrived yet gets nothing; a removed output
//! takes its surfaces with it. A surface the compositor `closed` is not
//! made again until the outputs change, so a compositor that closes every
//! new surface cannot drive the OSD into a loop.

use crate::brand::{
    MARK_RIGHT, MARK_SIDE, MARK_TOP, OSD_BAR_GAP, OSD_BAR_H, OSD_BOTTOM, OSD_ICON, OSD_NOTE_W, OSD_PAD_Y, OSD_W,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, BorrowedFd, FromRawFd};
use wayland_client::backend::{Backend, WaylandError};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_region, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

pub use zwlr_layer_surface_v1::Anchor;

/// Where a surface sits on its output: the edges it is anchored to, the
/// margins to them (top, right, bottom, left) and its size, in pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub anchor: Anchor,
    pub margin: (i32, i32, i32, i32),
    pub size: (u32, u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SurfaceKind {
    Panel,
    Mark,
}

/// Whether an output came, went or got its name since the last dispatch.
/// A `closed` surface does not count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputsChanged(pub bool);

fn px(v: f32, scale: f32) -> f32 {
    (v * scale).round()
}

/// The panel at the bottom centre: `OSD_W` wide, the icon row between its
/// paddings, and the level bar below it when `bar`.
pub fn panel_placement(scale: f32, bar: bool) -> Placement {
    // Each term is rounded on its own, as the drawing lays them out, so
    // the frame and the surface agree at every scale.
    let mut h = 2.0 * px(OSD_PAD_Y, scale) + px(OSD_ICON, scale);
    if bar {
        h += px(OSD_BAR_GAP, scale) + px(OSD_BAR_H, scale);
    }
    Placement {
        anchor: Anchor::Bottom,
        margin: (0, 0, px(OSD_BOTTOM, scale) as i32, 0),
        size: (px(OSD_W, scale) as u32, h as u32),
    }
}

/// A text note's panel: as `panel_placement` without a bar, `OSD_NOTE_W` wide.
pub fn note_placement(scale: f32) -> Placement {
    Placement {
        size: (px(OSD_NOTE_W, scale) as u32, panel_placement(scale, false).size.1),
        ..panel_placement(scale, false)
    }
}

/// The muted-microphone mark in the top right corner.
pub fn mark_placement(scale: f32) -> Placement {
    let side = px(MARK_SIDE, scale) as u32;
    Placement {
        anchor: Anchor::Top | Anchor::Right,
        margin: (px(MARK_TOP, scale) as i32, px(MARK_RIGHT, scale) as i32, 0, 0),
        size: (side, side),
    }
}

/// One layer surface and what it waits to show.
struct Layer {
    surface: wl_surface::WlSurface,
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    place: Placement,
    /// The compositor has sent a configure of the current placement's size.
    configured: bool,
    /// A frame that waits for the configure.
    pending: Option<(u32, Vec<u32>)>,
}

impl Layer {
    fn destroy(self) {
        self.layer.destroy();
        self.surface.destroy();
    }
}

struct Output {
    wl: wl_output::WlOutput,
    name: Option<String>,
    panel: Option<Layer>,
    mark: Option<Layer>,
}

impl Output {
    fn slot(&mut self, kind: SurfaceKind) -> &mut Option<Layer> {
        match kind {
            SurfaceKind::Panel => &mut self.panel,
            SurfaceKind::Mark => &mut self.mark,
        }
    }

    fn drop_surfaces(&mut self) {
        for kind in [SurfaceKind::Panel, SurfaceKind::Mark] {
            if let Some(l) = self.slot(kind).take() {
                l.destroy();
            }
        }
    }
}

struct State {
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1,
    /// By the output's global name.
    outputs: BTreeMap<u32, Output>,
    /// Surfaces the compositor closed, by output id; `show` leaves them
    /// alone until the outputs change.
    closed: BTreeSet<(u32, SurfaceKind)>,
    changed: bool,
    /// The first error seen inside an event handler.
    error: Option<String>,
}

pub struct Surfaces {
    conn: Connection,
    backend: Backend,
    queue: EventQueue<State>,
    state: State,
}

impl Surfaces {
    pub fn connect() -> Result<Surfaces, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland connection: {e}"))?;
        let (globals, mut queue): (_, EventQueue<State>) =
            registry_queue_init(&conn).map_err(|e| format!("Wayland registry: {e}"))?;
        let qh = queue.handle();
        let compositor = globals
            .bind(&qh, 4..=4, ())
            .map_err(|e| format!("wl_compositor v4: {e}"))?;
        let shm = globals.bind(&qh, 1..=1, ()).map_err(|e| format!("wl_shm: {e}"))?;
        let layer_shell = globals
            .bind(&qh, 4..=4, ())
            .map_err(|e| format!("zwlr_layer_shell_v1 v4: {e}"))?;
        let mut state = State {
            compositor,
            shm,
            layer_shell,
            outputs: BTreeMap::new(),
            closed: BTreeSet::new(),
            changed: false,
            error: None,
        };
        for g in globals.contents().clone_list() {
            if g.interface == "wl_output" {
                state.bind_output(globals.registry(), g.name, g.version, &qh);
            }
        }
        // Delivers the outputs' name events.
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("Wayland roundtrip: {e}"))?;
        let backend = conn.backend();
        Ok(Surfaces {
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

    /// Reads what the compositor sent, without blocking, and handles it.
    pub fn dispatch(&mut self) -> Result<OutputsChanged, String> {
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
        if let Some(e) = self.state.error.take() {
            return Err(e);
        }
        Ok(OutputsChanged(std::mem::take(&mut self.state.changed)))
    }

    /// The names of the outputs that have one, e.g. "HEADLESS-1".
    pub fn outputs(&self) -> Vec<String> {
        self.state.outputs.values().filter_map(|o| o.name.clone()).collect()
    }

    /// Show `pixels` (XRGB8888, row-major, width*height) for `kind` on
    /// `output`, creating the layer surface when needed. A surface the
    /// compositor closed stays away, without an error, until the outputs
    /// change.
    pub fn show(
        &mut self,
        output: &str,
        kind: SurfaceKind,
        place: &Placement,
        width: u32,
        pixels: &[u32],
    ) -> Result<(), String> {
        if width == 0 || pixels.is_empty() || !pixels.len().is_multiple_of(width as usize) {
            return Err(format!("{} pixels do not make rows of {width}", pixels.len()));
        }
        let height = (pixels.len() / width as usize) as u32;
        if (width, height) != place.size {
            return Err(format!(
                "a {width}x{height} frame for a {}x{} surface",
                place.size.0, place.size.1
            ));
        }
        let qh = self.queue.handle();
        let Some(id) = self.state.output_id(output) else {
            return Err(format!("no output {output}"));
        };
        if self.state.closed.contains(&(id, kind)) {
            return Ok(());
        }
        let frame = (width, pixels.to_vec());
        let out = self.state.outputs.get_mut(&id).expect("output_id found it");
        let wl = out.wl.clone();
        match out.slot(kind) {
            Some(l) if l.configured && l.place == *place => {
                let surface = l.surface.clone();
                return self.state.attach(&surface, frame, &qh);
            }
            Some(l) => {
                // A new placement waits for its configure, as the first one.
                if l.place != *place {
                    apply_placement(&l.layer, place);
                    l.place = place.clone();
                    l.configured = false;
                    l.surface.commit();
                }
                l.pending = Some(frame);
                return Ok(());
            }
            None => {}
        }
        let surface = self.state.compositor.create_surface(&qh, ());
        let layer = self.state.layer_shell.get_layer_surface(
            &surface,
            Some(&wl),
            zwlr_layer_shell_v1::Layer::Overlay,
            "thinfloe-osd".into(),
            &qh,
            (id, kind),
        );
        apply_placement(&layer, place);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        let region = self.state.compositor.create_region(&qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        surface.commit();
        let out = self.state.outputs.get_mut(&id).expect("output_id found it");
        *out.slot(kind) = Some(Layer {
            surface,
            layer,
            place: place.clone(),
            configured: false,
            pending: Some(frame),
        });
        Ok(())
    }

    pub fn hide(&mut self, output: &str, kind: SurfaceKind) {
        if let Some(id) = self.state.output_id(output)
            && let Some(out) = self.state.outputs.get_mut(&id)
            && let Some(l) = out.slot(kind).take()
        {
            l.destroy();
        }
    }

    pub fn flush(&mut self) -> Result<(), String> {
        match self.conn.flush() {
            Ok(()) => Ok(()),
            Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(format!("Wayland flush: {e}")),
        }
    }
}

fn apply_placement(layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, place: &Placement) {
    let (top, right, bottom, left) = place.margin;
    layer.set_size(place.size.0, place.size.1);
    layer.set_anchor(place.anchor);
    layer.set_margin(top, right, bottom, left);
}

/// Whether a configure of `(width, height)` lets a frame of `size` show.
/// A configure of an earlier placement may still arrive after `show`
/// changed the size; the frame of the new size waits for one that
/// matches. 0 leaves that side to us.
fn configure_fits(size: (u32, u32), (width, height): (u32, u32)) -> bool {
    (width == 0 || width == size.0) && (height == 0 || height == size.1)
}

/// A memfd with `pixels` in wl_shm's XRGB8888, little-endian.
fn shm_file(pixels: &[u32]) -> Result<File, String> {
    let name = CString::new("thinfloe-osd").expect("no NUL");
    // SAFETY: a valid C string; the returned descriptor is checked and
    // owned by the File from here on.
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: fd is a fresh descriptor nothing else owns.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let bytes: Vec<u8> = pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
    file.write_all(&bytes).map_err(|e| format!("memfd write: {e}"))?;
    Ok(file)
}

impl State {
    /// An output came, went or got its name: closed surfaces may be made
    /// again.
    fn outputs_changed(&mut self) {
        self.changed = true;
        self.closed.clear();
    }

    fn output_id(&self, name: &str) -> Option<u32> {
        self.outputs
            .iter()
            .find(|(_, o)| o.name.as_deref() == Some(name))
            .map(|(&id, _)| id)
    }

    fn bind_output(&mut self, registry: &wl_registry::WlRegistry, id: u32, version: u32, qh: &QueueHandle<State>) {
        // Only v4 sends the name the surfaces are keyed by.
        if version < 4 {
            return;
        }
        let wl = registry.bind(id, 4, qh, id);
        self.outputs.insert(
            id,
            Output {
                wl,
                name: None,
                panel: None,
                mark: None,
            },
        );
        self.outputs_changed();
    }

    fn attach(
        &mut self,
        surface: &wl_surface::WlSurface,
        (width, pixels): (u32, Vec<u32>),
        qh: &QueueHandle<State>,
    ) -> Result<(), String> {
        let height = pixels.len() as u32 / width;
        let file = shm_file(&pixels)?;
        let size = pixels.len() * 4;
        let pool = self.shm.create_pool(file.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            width as i32 * 4,
            wl_shm::Format::Xrgb8888,
            qh,
            BufferFile { _file: file },
        );
        pool.destroy();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width as i32, height as i32);
        surface.commit();
        Ok(())
    }
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
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(mut out) = state.outputs.remove(&name) {
                    out.drop_surfaces();
                    out.wl.release();
                    state.outputs_changed();
                }
            }
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
        if let wl_output::Event::Name { name } = event
            && let Some(out) = state.outputs.get_mut(id)
        {
            out.name = Some(name);
            state.outputs_changed();
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, (u32, SurfaceKind)> for State {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        &(id, kind): &(u32, SurfaceKind),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let Some(out) = state.outputs.get_mut(&id) else {
            return;
        };
        let slot = out.slot(kind);
        // An event for a surface already replaced or dropped.
        if slot.as_ref().is_none_or(|l| l.layer != *layer) {
            return;
        }
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                let l = slot.as_mut().expect("checked above");
                layer.ack_configure(serial);
                if !configure_fits(l.place.size, (width, height)) {
                    return;
                }
                l.configured = true;
                if let Some(frame) = l.pending.take() {
                    let surface = l.surface.clone();
                    if let Err(e) = state.attach(&surface, frame, qh) {
                        state.error.get_or_insert(e);
                    }
                } else {
                    l.surface.commit();
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                if let Some(l) = slot.take() {
                    l.destroy();
                }
                state.closed.insert((id, kind));
            }
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
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}

no_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_region::WlRegion,
    wl_surface::WlSurface,
    zwlr_layer_shell_v1::ZwlrLayerShellV1
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_and_mark_placement_follow_the_scale() {
        assert_eq!(panel_placement(1.0, true).size, (320, 61));
        assert_eq!(panel_placement(1.0, false).size, (320, 46));
        assert_eq!(panel_placement(2.0, true).size, (640, 122));
        // Rounded per value (15+15+28+11+8), not the sum (76.25 -> 76).
        assert_eq!(panel_placement(1.25, true).size, (400, 77));
        assert_eq!(mark_placement(1.25).size, (38, 38));
        assert_eq!(mark_placement(1.25).margin, (43, 18, 0, 0));
        assert_eq!(panel_placement(1.5, true).margin, (0, 0, 33, 0));
        assert_eq!(mark_placement(1.0).margin, (34, 14, 0, 0));
        assert_eq!(mark_placement(2.0).size, (60, 60));
        assert_eq!(note_placement(1.0).size, (480, 46));
        assert_eq!(note_placement(2.0).size, (960, 92));
        assert_eq!(note_placement(1.0).anchor, Anchor::Bottom);
        assert_eq!(note_placement(1.5).margin, panel_placement(1.5, false).margin);
    }

    #[test]
    fn only_a_configure_of_the_current_size_releases_a_frame() {
        let with_bar = panel_placement(1.0, true).size;
        let without = panel_placement(1.0, false).size;
        assert!(configure_fits(with_bar, with_bar));
        assert!(!configure_fits(with_bar, without));
        assert!(!configure_fits(panel_placement(2.0, true).size, with_bar));
        assert!(configure_fits(with_bar, (0, 0)));
        assert!(configure_fits(with_bar, (with_bar.0, 0)));
        assert!(!configure_fits(with_bar, (0, without.1)));
    }

    #[test]
    fn the_panel_is_anchored_bottom_and_the_mark_top_right() {
        assert_eq!(panel_placement(1.0, true).anchor, Anchor::Bottom);
        assert_eq!(mark_placement(1.0).anchor, Anchor::Top | Anchor::Right);
    }
}
