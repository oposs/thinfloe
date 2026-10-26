//! Folded windows, read from labwc through zwlr_foreign_toplevel_manager_v1.
//!
//! With zero keybindings and no window list, a minimized window has no way
//! back unless the menu offers one. `Tracker` is the pure bookkeeping, `Windows`
//! the Wayland connection that feeds it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child, globals::GlobalListContents,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self as handle_v1, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self as manager_v1, ZwlrForeignToplevelManagerV1},
};

/// The window of the menu itself; it is never offered back to the person.
const MENU_APP_ID: &str = "thinfloe-menu";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folded {
    pub id: u64,
    pub title: String,
    pub app_id: String,
}

/// One toplevel, as the protocol says it: `Done` ends a batch.
#[derive(Clone, Debug)]
pub enum Event {
    Title(u64, String),
    AppId(u64, String),
    Minimized(u64, bool),
    Done(u64),
    Closed(u64),
}

#[derive(Clone, Default)]
struct Shown {
    title: String,
    app_id: String,
    minimized: bool,
}

struct Entry {
    /// What the last `Done` committed; `None` until the first one.
    shown: Option<Shown>,
    pending: Shown,
    /// Position in the folding order; set when the window becomes folded.
    fold_seq: u64,
}

#[derive(Default)]
pub struct Tracker {
    entries: HashMap<u64, Entry>,
    next_seq: u64,
}

impl Tracker {
    /// Returns true when the folded list changed.
    pub fn apply(&mut self, e: Event) -> bool {
        let before = self.folded();
        match e {
            Event::Title(id, t) => self.entry(id).pending.title = t,
            Event::AppId(id, a) => self.entry(id).pending.app_id = a,
            Event::Minimized(id, m) => self.entry(id).pending.minimized = m,
            Event::Done(id) => {
                let seq = self.next_seq;
                let entry = self.entry(id);
                let was_folded = entry.shown.as_ref().is_some_and(|s| s.minimized);
                entry.shown = Some(entry.pending.clone());
                if entry.pending.minimized && !was_folded {
                    entry.fold_seq = seq;
                    self.next_seq += 1;
                }
            }
            Event::Closed(id) => {
                self.entries.remove(&id);
            }
        }
        self.folded() != before
    }

    fn entry(&mut self, id: u64) -> &mut Entry {
        self.entries.entry(id).or_insert_with(|| Entry {
            shown: None,
            pending: Shown::default(),
            fold_seq: 0,
        })
    }

    /// Folded windows, oldest fold first.
    pub fn folded(&self) -> Vec<Folded> {
        let mut v: Vec<(u64, Folded)> = self
            .entries
            .iter()
            .filter_map(|(&id, e)| {
                let s = e.shown.as_ref()?;
                (s.minimized && s.app_id != MENU_APP_ID).then(|| {
                    (
                        e.fold_seq,
                        Folded { id, title: s.title.clone(), app_id: s.app_id.clone() },
                    )
                })
            })
            .collect();
        v.sort_by_key(|(seq, _)| *seq);
        v.into_iter().map(|(_, f)| f).collect()
    }

    /// Every committed window but the menu, as (id, app_id, title, folded).
    pub fn all(&self) -> Vec<(u64, String, String, bool)> {
        let mut v: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(&id, e)| {
                let s = e.shown.as_ref()?;
                (s.app_id != MENU_APP_ID).then(|| (id, s.app_id.clone(), s.title.clone(), s.minimized))
            })
            .collect();
        v.sort_by_key(|(id, ..)| *id);
        v
    }
}

type Handles = HashMap<u64, ZwlrForeignToplevelHandleV1>;

struct State {
    tracker: Arc<Mutex<Tracker>>,
    handles: Arc<Mutex<Handles>>,
    on_change: Box<dyn Fn() + Send>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Ids are the protocol object ids, which are never reused on one connection
/// while a handle lives, but a counter keeps a closed id from ever returning.
fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
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

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // The handle's own events carry its id as user data; this only has
        // to keep the object alive.
        if let manager_v1::Event::Toplevel { toplevel } = event {
            let id = toplevel.data::<u64>().copied().unwrap_or(0);
            lock(&state.handles).insert(id, toplevel);
        }
    }

    event_created_child!(State, ZwlrForeignToplevelManagerV1, [
        manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, next_id()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, u64> for State {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: handle_v1::Event,
        &id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let e = match event {
            handle_v1::Event::Title { title } => Event::Title(id, title),
            handle_v1::Event::AppId { app_id } => Event::AppId(id, app_id),
            handle_v1::Event::State { state } => {
                // An array of native-endian u32 state values.
                let minimized = state.chunks_exact(4).any(|c| {
                    u32::from_ne_bytes([c[0], c[1], c[2], c[3]]) == handle_v1::State::Minimized as u32
                });
                Event::Minimized(id, minimized)
            }
            handle_v1::Event::Done => Event::Done(id),
            handle_v1::Event::Closed => {
                lock(&state.handles).remove(&id);
                handle.destroy();
                Event::Closed(id)
            }
            _ => return,
        };
        if lock(&state.tracker).apply(e) {
            (state.on_change)();
        }
    }
}

pub struct Windows {
    conn: Connection,
    seat: wl_seat::WlSeat,
    tracker: Arc<Mutex<Tracker>>,
    handles: Arc<Mutex<Handles>>,
}

impl Windows {
    /// Connects on its own Wayland connection and dispatches on a thread of
    /// its own. `Err` when the compositor has no foreign-toplevel manager or
    /// no seat; the caller then shows no strip.
    pub fn connect(on_change: impl Fn() + Send + 'static) -> Result<Windows, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland connection: {e}"))?;
        let (globals, mut queue): (_, EventQueue<State>) =
            wayland_client::globals::registry_queue_init(&conn).map_err(|e| format!("Wayland registry: {e}"))?;
        let qh = queue.handle();
        let seat: wl_seat::WlSeat = globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| format!("no wl_seat: {e}"))?;
        let _manager: ZwlrForeignToplevelManagerV1 = globals
            .bind(&qh, 1..=3, ())
            .map_err(|_| "the compositor lacks zwlr_foreign_toplevel_manager_v1".to_string())?;
        let tracker = Arc::new(Mutex::new(Tracker::default()));
        let handles = Arc::new(Mutex::new(Handles::new()));
        let mut state = State {
            tracker: tracker.clone(),
            handles: handles.clone(),
            on_change: Box::new(|| {}),
        };
        // The first roundtrip delivers the toplevels, the second their
        // properties; the windows open now are known when this returns.
        for _ in 0..2 {
            queue.roundtrip(&mut state).map_err(|e| format!("Wayland roundtrip: {e}"))?;
        }
        state.on_change = Box::new(on_change);
        std::thread::spawn(move || {
            let _manager = _manager;
            while queue.blocking_dispatch(&mut state).is_ok() {}
        });
        Ok(Windows { conn, seat, tracker, handles })
    }

    pub fn folded(&self) -> Vec<Folded> {
        lock(&self.tracker).folded()
    }

    /// (id, app_id, title, folded) of every window.
    pub fn all(&self) -> Vec<(u64, String, String, bool)> {
        lock(&self.tracker).all()
    }

    /// Unfolds and activates a window. An unknown id (the window just
    /// closed) does nothing.
    pub fn restore(&self, id: u64) {
        let handles = lock(&self.handles);
        let Some(h) = handles.get(&id) else { return };
        h.unset_minimized();
        h.activate(&self.seat);
        let _ = self.conn.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(t: &mut Tracker, id: u64, title: &str) {
        t.apply(Event::Title(id, title.into()));
        t.apply(Event::AppId(id, "app".into()));
        t.apply(Event::Minimized(id, true));
        t.apply(Event::Done(id));
    }

    #[test]
    fn a_window_is_listed_only_after_done() {
        let mut t = Tracker::default();
        assert!(!t.apply(Event::Title(1, "ThinLinc – lab".into())));
        assert!(!t.apply(Event::Minimized(1, true)));
        assert!(t.folded().is_empty());
        assert!(t.apply(Event::Done(1)));
        assert_eq!(t.folded()[0].title, "ThinLinc – lab");
    }

    #[test]
    fn the_menu_itself_is_never_listed() {
        let mut t = Tracker::default();
        for e in [Event::AppId(2, "thinfloe-menu".into()), Event::Minimized(2, true), Event::Done(2)] {
            t.apply(e);
        }
        assert!(t.folded().is_empty());
        assert!(t.all().is_empty());
    }

    #[test]
    fn closing_or_unfolding_removes_the_chip_and_a_title_change_renames_it() {
        let mut t = Tracker::default();
        fold(&mut t, 1, "one");
        assert!(!t.apply(Event::Title(1, "renamed".into())));
        assert_eq!(t.folded()[0].title, "one");
        assert!(t.apply(Event::Done(1)));
        assert_eq!(t.folded()[0].title, "renamed");
        t.apply(Event::Minimized(1, false));
        assert!(t.apply(Event::Done(1)));
        assert!(t.folded().is_empty());
        assert_eq!(t.all().len(), 1);
        t.apply(Event::Minimized(1, true));
        t.apply(Event::Done(1));
        assert!(t.apply(Event::Closed(1)));
        assert!(t.folded().is_empty());
        assert!(!t.apply(Event::Closed(99)));
    }

    #[test]
    fn folded_windows_keep_the_order_they_were_folded_in() {
        let mut t = Tracker::default();
        fold(&mut t, 3, "c");
        fold(&mut t, 1, "a");
        fold(&mut t, 2, "b");
        let ids: Vec<u64> = t.folded().iter().map(|f| f.id).collect();
        assert_eq!(ids, [3, 1, 2]);
        // A later Done that changes nothing about folding keeps the place.
        t.apply(Event::Title(3, "c2".into()));
        t.apply(Event::Done(3));
        let ids: Vec<u64> = t.folded().iter().map(|f| f.id).collect();
        assert_eq!(ids, [3, 1, 2]);
    }
}
