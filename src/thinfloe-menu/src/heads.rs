//! Monitors as the compositor reports them, read through
//! zwlr_output_manager_v1.
//!
//! The menu only listens here; kanshi is the one program that sets monitors.
//! `HeadCollector` is the pure bookkeeping, `Heads` the Wayland connection
//! that feeds it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use wayland_client::protocol::wl_registry;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child, globals::GlobalListContents,
};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_head_v1::{self as head_v1, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self as manager_v1, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self as mode_v1, ZwlrOutputModeV1},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mode {
    pub w: u32,
    pub h: u32,
    /// Refresh rate in millihertz; headless outputs report 0.
    pub refresh_mhz: u32,
    pub preferred: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotation {
    Normal,
    R90,
    R180,
    R270,
}

impl Rotation {
    /// Flipped transforms (4..7) read as their unflipped rotation.
    fn from_transform(t: i32) -> Rotation {
        match t & 3 {
            1 => Rotation::R90,
            2 => Rotation::R180,
            3 => Rotation::R270,
            _ => Rotation::Normal,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub name: String,
    pub make: String,
    pub model: String,
    pub serial: String,
    pub phys_mm: (u32, u32),
    pub enabled: bool,
    pub modes: Vec<Mode>,
    /// Index into `modes`.
    pub current: Option<usize>,
    pub pos: (i32, i32),
    pub rotation: Rotation,
}

/// One protocol event of one head; mode events carry the mode's id.
#[derive(Clone, Debug)]
pub enum HeadEvent {
    Name(String),
    Make(String),
    Model(String),
    Serial(String),
    PhysMm(i32, i32),
    Enabled(bool),
    Position(i32, i32),
    Transform(i32),
    Mode { id: u32 },
    ModeSize { id: u32, w: i32, h: i32 },
    ModeRefresh { id: u32, mhz: i32 },
    ModePreferred { id: u32 },
    CurrentMode { id: u32 },
    Finished,
}

struct Building {
    head: Head,
    mode_ids: Vec<u32>,
    current_id: Option<u32>,
}

impl Building {
    fn new() -> Building {
        Building {
            head: Head {
                name: String::new(),
                make: String::new(),
                model: String::new(),
                serial: String::new(),
                phys_mm: (0, 0),
                enabled: false,
                modes: Vec::new(),
                current: None,
                pos: (0, 0),
                rotation: Rotation::Normal,
            },
            mode_ids: Vec::new(),
            current_id: None,
        }
    }

    fn mode(&mut self, id: u32) -> Option<&mut Mode> {
        let i = self.mode_ids.iter().position(|&m| m == id)?;
        self.head.modes.get_mut(i)
    }
}

#[derive(Default)]
pub struct HeadCollector {
    heads: HashMap<u32, Building>,
}

impl HeadCollector {
    pub fn head(&mut self, head: u32, e: HeadEvent) {
        if matches!(e, HeadEvent::Finished) {
            self.heads.remove(&head);
            return;
        }
        let b = self.heads.entry(head).or_insert_with(Building::new);
        match e {
            HeadEvent::Name(s) => b.head.name = s,
            HeadEvent::Make(s) => b.head.make = s,
            HeadEvent::Model(s) => b.head.model = s,
            HeadEvent::Serial(s) => b.head.serial = s,
            HeadEvent::PhysMm(w, h) => b.head.phys_mm = (w.max(0) as u32, h.max(0) as u32),
            HeadEvent::Enabled(on) => b.head.enabled = on,
            HeadEvent::Position(x, y) => b.head.pos = (x, y),
            HeadEvent::Transform(t) => b.head.rotation = Rotation::from_transform(t),
            HeadEvent::Mode { id } => {
                b.mode_ids.push(id);
                b.head.modes.push(Mode {
                    w: 0,
                    h: 0,
                    refresh_mhz: 0,
                    preferred: false,
                });
            }
            HeadEvent::ModeSize { id, w, h } => {
                if let Some(m) = b.mode(id) {
                    m.w = w.max(0) as u32;
                    m.h = h.max(0) as u32;
                }
            }
            HeadEvent::ModeRefresh { id, mhz } => {
                if let Some(m) = b.mode(id) {
                    m.refresh_mhz = mhz.max(0) as u32;
                }
            }
            HeadEvent::ModePreferred { id } => {
                if let Some(m) = b.mode(id) {
                    m.preferred = true;
                }
            }
            HeadEvent::CurrentMode { id } => b.current_id = Some(id),
            HeadEvent::Finished => {}
        }
    }

    /// The state at the manager's `done`: every live head, sorted by name.
    pub fn done(&mut self) -> Vec<Head> {
        let mut v: Vec<Head> = self
            .heads
            .values()
            .map(|b| {
                let mut h = b.head.clone();
                h.current = b.current_id.and_then(|id| b.mode_ids.iter().position(|&m| m == id));
                h
            })
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}

struct State {
    collector: HeadCollector,
    shared: Arc<Mutex<Vec<Head>>>,
    on_change: Box<dyn Fn() + Send>,
    /// Keeps the head objects alive; the protocol sends no events to dead ones.
    live: HashMap<u32, ZwlrOutputHeadV1>,
    /// Mode events name no head; this maps a mode id to the head that owns it.
    mode_owner: HashMap<u32, u32>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Ids for heads and modes; a counter, so a finished object's id never returns.
fn next_id() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputManagerV1,
        event: manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            manager_v1::Event::Head { head } => {
                let id = head.data::<u32>().copied().unwrap_or(0);
                state.live.insert(id, head);
            }
            manager_v1::Event::Done { .. } => {
                *lock(&state.shared) = state.collector.done();
                (state.on_change)();
            }
            _ => {}
        }
    }

    event_created_child!(State, ZwlrOutputManagerV1, [
        manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, next_id()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputHeadV1,
        event: head_v1::Event,
        &id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let e = match event {
            head_v1::Event::Name { name } => HeadEvent::Name(name),
            head_v1::Event::Make { make } => HeadEvent::Make(make),
            head_v1::Event::Model { model } => HeadEvent::Model(model),
            head_v1::Event::SerialNumber { serial_number } => HeadEvent::Serial(serial_number),
            head_v1::Event::PhysicalSize { width, height } => HeadEvent::PhysMm(width, height),
            head_v1::Event::Enabled { enabled } => HeadEvent::Enabled(enabled != 0),
            head_v1::Event::Position { x, y } => HeadEvent::Position(x, y),
            head_v1::Event::Transform { transform } => {
                HeadEvent::Transform(transform.into_result().map(|t| t as i32).unwrap_or(0))
            }
            head_v1::Event::Mode { mode } => {
                let mode_id = mode.data::<u32>().copied().unwrap_or(0);
                state.mode_owner.insert(mode_id, id);
                state.collector.head(id, HeadEvent::Mode { id: mode_id });
                return;
            }
            head_v1::Event::CurrentMode { mode } => {
                HeadEvent::CurrentMode {
                    id: mode.data::<u32>().copied().unwrap_or(0),
                }
            }
            head_v1::Event::Finished => {
                if let Some(h) = state.live.remove(&id) {
                    // `release` exists from version 3 on; older ones would
                    // take it as a protocol error.
                    if h.version() >= 3 {
                        h.release();
                    }
                }
                state.mode_owner.retain(|_, &mut owner| owner != id);
                HeadEvent::Finished
            }
            _ => return,
        };
        state.collector.head(id, e);
    }

    event_created_child!(State, ZwlrOutputHeadV1, [
        head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, next_id()),
    ]);
}

impl Dispatch<ZwlrOutputModeV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputModeV1,
        event: mode_v1::Event,
        &id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let e = match event {
            mode_v1::Event::Size { width, height } => HeadEvent::ModeSize {
                id,
                w: width,
                h: height,
            },
            mode_v1::Event::Refresh { refresh } => HeadEvent::ModeRefresh { id, mhz: refresh },
            mode_v1::Event::Preferred => HeadEvent::ModePreferred { id },
            _ => return,
        };
        if let Some(&head) = state.mode_owner.get(&id) {
            state.collector.head(head, e);
        }
    }
}

pub struct Heads {
    _conn: Connection,
    shared: Arc<Mutex<Vec<Head>>>,
}

impl Heads {
    /// Connects on its own Wayland connection and dispatches on a thread of
    /// its own. `Err` when the compositor offers no output manager.
    pub fn connect(on_change: impl Fn() + Send + 'static) -> Result<Heads, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland connection: {e}"))?;
        let (globals, mut queue): (_, EventQueue<State>) =
            wayland_client::globals::registry_queue_init(&conn).map_err(|e| format!("Wayland registry: {e}"))?;
        let qh = queue.handle();
        let manager: ZwlrOutputManagerV1 = globals
            .bind(&qh, 1..=4, ())
            .map_err(|_| "the compositor offers no output management".to_string())?;
        let shared = Arc::new(Mutex::new(Vec::new()));
        let mut state = State {
            collector: HeadCollector::default(),
            shared: shared.clone(),
            on_change: Box::new(|| {}),
            live: HashMap::new(),
            mode_owner: HashMap::new(),
        };
        // The first roundtrip delivers the heads, the second their
        // properties; the monitors present now are known when this returns.
        for _ in 0..2 {
            queue
                .roundtrip(&mut state)
                .map_err(|e| format!("Wayland roundtrip: {e}"))?;
        }
        state.on_change = Box::new(on_change);
        std::thread::spawn(move || {
            let _manager = manager;
            loop {
                if let Err(e) = queue.blocking_dispatch(&mut state) {
                    eprintln!("thinfloe-menu: monitor list stopped: {e}");
                    break;
                }
            }
        });
        Ok(Heads { _conn: conn, shared })
    }

    pub fn snapshot(&self) -> Vec<Head> {
        lock(&self.shared).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(c: &mut HeadCollector, head: u32, id: u32, w: i32, h: i32, mhz: i32) {
        c.head(head, HeadEvent::Mode { id });
        c.head(head, HeadEvent::ModeSize { id, w, h });
        c.head(head, HeadEvent::ModeRefresh { id, mhz });
    }

    #[test]
    fn a_head_with_modes_and_a_current_mode() {
        let mut c = HeadCollector::default();
        c.head(1, HeadEvent::Name("eDP-1".into()));
        c.head(1, HeadEvent::Enabled(true));
        mode(&mut c, 1, 10, 1920, 1080, 60000);
        mode(&mut c, 1, 11, 1920, 1080, 59940);
        c.head(1, HeadEvent::ModePreferred { id: 10 });
        c.head(1, HeadEvent::CurrentMode { id: 11 });
        let v = c.done();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].modes.len(), 2);
        assert_eq!(v[0].current, Some(1));
        assert_eq!(v[0].modes[v[0].current.unwrap()].refresh_mhz, 59940);
        assert!(v[0].modes[0].preferred && !v[0].modes[1].preferred);
    }

    #[test]
    fn a_finished_head_is_dropped() {
        let mut c = HeadCollector::default();
        c.head(1, HeadEvent::Name("eDP-1".into()));
        c.head(2, HeadEvent::Name("HDMI-A-1".into()));
        c.head(1, HeadEvent::Finished);
        let v = c.done();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "HDMI-A-1");
    }

    #[test]
    fn a_mode_finished_before_current_is_ignored() {
        let mut c = HeadCollector::default();
        c.head(1, HeadEvent::Name("eDP-1".into()));
        mode(&mut c, 1, 10, 1920, 1080, 60000);
        c.head(1, HeadEvent::CurrentMode { id: 99 });
        assert_eq!(c.done()[0].current, None);
    }

    #[test]
    fn transforms_map_to_rotations() {
        let rot = |t| {
            let mut c = HeadCollector::default();
            c.head(1, HeadEvent::Transform(t));
            c.done()[0].rotation
        };
        assert_eq!(rot(0), Rotation::Normal);
        assert_eq!(rot(1), Rotation::R90);
        assert_eq!(rot(2), Rotation::R180);
        assert_eq!(rot(3), Rotation::R270);
        assert_eq!(rot(4), Rotation::Normal);
        assert_eq!(rot(5), Rotation::R90);
        assert_eq!(rot(6), Rotation::R180);
        assert_eq!(rot(7), Rotation::R270);
    }

    #[test]
    fn heads_are_sorted_by_name() {
        let mut c = HeadCollector::default();
        let names = ["eDP-1", "DP-3", "HDMI-A-1", "DP-1", "HDMI-A-2", "DP-2", "Virtual-1", "DP-10"];
        for (i, n) in names.iter().enumerate() {
            c.head(i as u32 + 1, HeadEvent::Name((*n).into()));
        }
        let got: Vec<_> = c.done().into_iter().map(|h| h.name).collect();
        let mut want: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(got, want);
        assert_eq!(got[0], "DP-1");
        assert_eq!(got[2], "DP-2");
    }
}
