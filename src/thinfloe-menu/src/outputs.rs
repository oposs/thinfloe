//! The physical size of the outputs, read from the compositor through
//! wl_output. `Collector` is the pure bookkeeping, `read` the Wayland
//! connection that feeds it.

use crate::density::OutputInfo;
use std::collections::BTreeMap;
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, globals::GlobalListContents};

/// One wl_output event, reduced to what the menu needs.
#[derive(Clone, Debug)]
pub enum OutEvent {
    Geometry { mm: (i32, i32) },
    Mode { current: bool, px: (i32, i32) },
    Name(String),
    Done,
}

#[derive(Default)]
struct Seen {
    name: Option<String>,
    mm: (u32, u32),
    mode_px: Option<(u32, u32)>,
}

#[derive(Default)]
pub struct Collector {
    seen: BTreeMap<u32, Seen>,
}

/// A negative size from the compositor counts as 0 (unknown).
fn unsigned(v: (i32, i32)) -> (u32, u32) {
    (v.0.max(0) as u32, v.1.max(0) as u32)
}

impl Collector {
    pub fn apply(&mut self, id: u32, e: OutEvent) {
        let s = self.seen.entry(id).or_default();
        match e {
            OutEvent::Geometry { mm } => s.mm = unsigned(mm),
            OutEvent::Mode { current: true, px } => s.mode_px = Some(unsigned(px)),
            OutEvent::Mode { current: false, .. } => {}
            OutEvent::Name(n) => s.name = Some(n),
            OutEvent::Done => {}
        }
    }

    /// One entry per output that sent a current mode, in id order.
    pub fn outputs(&self) -> Vec<OutputInfo> {
        self.seen
            .values()
            .filter_map(|s| {
                Some(OutputInfo {
                    name: s.name.clone(),
                    mode_px: s.mode_px?,
                    phys_mm: s.mm,
                })
            })
            .collect()
    }
}

struct State {
    collector: Collector,
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

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        &id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let e = match event {
            wl_output::Event::Geometry {
                physical_width,
                physical_height,
                ..
            } => OutEvent::Geometry {
                mm: (physical_width, physical_height),
            },
            wl_output::Event::Mode {
                flags, width, height, ..
            } => OutEvent::Mode {
                current: flags.into_result().is_ok_and(|f| f.contains(wl_output::Mode::Current)),
                px: (width, height),
            },
            wl_output::Event::Name { name } => OutEvent::Name(name),
            wl_output::Event::Done => OutEvent::Done,
            _ => return,
        };
        state.collector.apply(id, e);
    }
}

/// Connects on its own Wayland connection, binds every output and returns
/// what they say. The connection is dropped on return.
pub fn read() -> Result<Vec<OutputInfo>, String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland connection: {e}"))?;
    let (globals, mut queue): (_, EventQueue<State>) =
        wayland_client::globals::registry_queue_init(&conn).map_err(|e| format!("Wayland registry: {e}"))?;
    let qh = queue.handle();
    let mut bound = Vec::new();
    for g in globals.contents().clone_list() {
        if g.interface == "wl_output" {
            let out: wl_output::WlOutput = globals.registry().bind(g.name, g.version.min(4), &qh, g.name);
            bound.push(out);
        }
    }
    let mut state = State {
        collector: Collector::default(),
    };
    // The first roundtrip delivers the bound outputs, the second their events.
    for _ in 0..2 {
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("Wayland roundtrip: {e}"))?;
    }
    Ok(state.collector.outputs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: Option<&str>, px: (u32, u32), mm: (u32, u32)) -> OutputInfo {
        OutputInfo {
            name: name.map(String::from),
            mode_px: px,
            phys_mm: mm,
        }
    }

    #[test]
    fn a_current_mode_and_geometry_make_one_output() {
        let mut c = Collector::default();
        c.apply(1, OutEvent::Geometry { mm: (531, 299) });
        c.apply(
            1,
            OutEvent::Mode {
                current: false,
                px: (1024, 768),
            },
        );
        c.apply(
            1,
            OutEvent::Mode {
                current: true,
                px: (1920, 1080),
            },
        );
        c.apply(1, OutEvent::Name("DP-1".into()));
        c.apply(1, OutEvent::Done);
        assert_eq!(c.outputs(), vec![info(Some("DP-1"), (1920, 1080), (531, 299))]);
    }

    #[test]
    fn an_output_without_a_current_mode_is_left_out() {
        let mut c = Collector::default();
        c.apply(1, OutEvent::Geometry { mm: (531, 299) });
        c.apply(
            1,
            OutEvent::Mode {
                current: false,
                px: (1024, 768),
            },
        );
        c.apply(1, OutEvent::Done);
        assert!(c.outputs().is_empty());
    }

    #[test]
    fn negative_mm_count_as_zero() {
        let mut c = Collector::default();
        c.apply(1, OutEvent::Geometry { mm: (-1, -1) });
        c.apply(
            1,
            OutEvent::Mode {
                current: true,
                px: (800, 600),
            },
        );
        assert_eq!(c.outputs(), vec![info(None, (800, 600), (0, 0))]);
    }

    #[test]
    fn a_version_3_output_has_no_name() {
        let mut c = Collector::default();
        c.apply(7, OutEvent::Geometry { mm: (300, 200) });
        c.apply(
            7,
            OutEvent::Mode {
                current: true,
                px: (1280, 800),
            },
        );
        c.apply(7, OutEvent::Done);
        assert_eq!(c.outputs()[0].name, None);
    }
}
