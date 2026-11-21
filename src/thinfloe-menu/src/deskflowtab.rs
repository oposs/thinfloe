//! The Deskflow band of the Screen tab: the `[deskflow]` settings as the
//! menu shows them, the daemon's state file, and what a click on the band
//! or on a device in the monitor arrangement asks for. Drawing is in
//! deskflow_ui.rs.

use crate::config::{DeskflowCfg, DeviceCfg};
use crate::deskflow::cmd::Cmd;
use crate::deskflow::edge::{self, Rect};
use crate::deskflow::statefile::{self, DevLine, DevState, Head as StateHead, StateFile};
use crate::deskflowcfg::Resistance;
use crate::heads::Head;
use crate::kanshi::{self, Profile, Side};

/// The edges in the order the Position control lists them.
pub const SIDES: [Side; 4] = [Side::LeftOf, Side::RightOf, Side::Above, Side::Below];
/// The size a device is drawn at before it reported one: 16:9.
pub const DEFAULT_DEVICE: (u32, u32) = (1920, 1080);
/// The size of a monitor that reports no mode, as the arrangement draws it.
const NO_MODE: (i32, i32) = (1920, 1080);
/// The flag of a pending line whose name is known with another fingerprint.
pub const FP_CHANGED: &str = "fp-changed";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskflowTab {
    pub cfg: DeskflowCfg,
    pub state: StateFile,
    /// The pending device (name, fingerprint) whose Position control is open.
    pub attaching: Option<(String, String)>,
    /// Why the last write or command failed.
    pub error: Option<String>,
    /// The attached device selected in the arrangement, by name.
    pub selected: Option<String>,
    /// The daemon runs; set by the app before each frame.
    pub running: bool,
    /// Why the daemon stopped the last time it failed; set by the app.
    pub stopped: Option<String>,
}

impl Default for DeskflowTab {
    fn default() -> DeskflowTab {
        from_config(&DeskflowCfg::default())
    }
}

/// The tab for the settings in `config.toml`; no state file read yet.
pub fn from_config(cfg: &DeskflowCfg) -> DeskflowTab {
    DeskflowTab {
        cfg: cfg.clone(),
        state: no_state(),
        attaching: None,
        error: None,
        selected: None,
        running: false,
        stopped: None,
    }
}

/// What the menu shows while there is no state file.
pub fn no_state() -> StateFile {
    StateFile {
        head: StateHead::Error(String::new()),
        devices: Vec::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeskflowEvent {
    Listen(bool),
    Resistance(Resistance),
    /// *Attach…* on a pending device: its Position control opens.
    Attach {
        name: String,
        fp: String,
    },
    /// An edge chosen in a Position control: for the device being attached,
    /// or to move an attached one.
    Place {
        name: String,
        output: String,
        side: Side,
    },
    Reject {
        name: String,
        fp: String,
    },
    Forget {
        name: String,
    },
    CancelAttach,
    /// A device box clicked in the arrangement.
    Select(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeskflowRequest {
    /// Keep this `[deskflow]` table, then tell the daemon.
    Save(DeskflowCfg),
    /// Send this to the daemon's socket.
    Send(Cmd),
}

/// Carry out `ev` on the tab; what the menu has to do then.
pub fn apply(tab: &mut DeskflowTab, ev: DeskflowEvent) -> Option<DeskflowRequest> {
    let save = |tab: &mut DeskflowTab| Some(DeskflowRequest::Save(tab.cfg.clone()));
    match ev {
        DeskflowEvent::Listen(on) => {
            if tab.cfg.listen == on {
                return None;
            }
            tab.cfg.listen = on;
            save(tab)
        }
        DeskflowEvent::Resistance(r) => {
            if tab.cfg.edge_resistance == r {
                return None;
            }
            tab.cfg.edge_resistance = r;
            save(tab)
        }
        DeskflowEvent::Attach { name, fp } => {
            tab.attaching = Some((name, fp));
            None
        }
        DeskflowEvent::CancelAttach => {
            tab.attaching = None;
            None
        }
        DeskflowEvent::Select(name) => {
            tab.selected = Some(name);
            None
        }
        DeskflowEvent::Place { name, output, side } => {
            // The fingerprint of a device being attached is the one of its
            // pending line; an attached device keeps its own.
            if let Some((_, fp)) = tab.attaching.take_if(|(n, _)| *n == name) {
                let clipboard = tab
                    .cfg
                    .devices
                    .iter()
                    .find(|d| d.name == name)
                    .is_none_or(|d| d.clipboard);
                tab.cfg.devices.retain(|d| d.name != name);
                tab.cfg.devices.push(DeviceCfg {
                    name,
                    fingerprint: fp,
                    output,
                    side,
                    clipboard,
                });
                return save(tab);
            }
            let d = tab.cfg.devices.iter_mut().find(|d| d.name == name)?;
            if d.output == output && d.side == side {
                return None;
            }
            d.output = output;
            d.side = side;
            save(tab)
        }
        DeskflowEvent::Reject { name, fp } => {
            if tab.attaching.as_ref().is_some_and(|(n, f)| *n == name && *f == fp) {
                tab.attaching = None;
            }
            Some(DeskflowRequest::Send(Cmd::Reject { name, fingerprint: fp }))
        }
        DeskflowEvent::Forget { name } => {
            let before = tab.cfg.devices.len();
            tab.cfg.devices.retain(|d| d.name != name);
            if tab.selected.as_deref() == Some(name.as_str()) {
                tab.selected = None;
            }
            if tab.cfg.devices.len() == before {
                return None;
            }
            save(tab)
        }
    }
}

/// The pending lines of the state file, in its order, while the daemon
/// runs: a file left from an earlier run lists devices that can be neither
/// attached nor rejected.
pub fn pending(tab: &DeskflowTab) -> Vec<&DevLine> {
    if !tab.running {
        return Vec::new();
    }
    tab.state
        .devices
        .iter()
        .filter(|d| d.state == DevState::Pending)
        .collect()
}

/// The state file's line of an attached device: same name and fingerprint.
pub fn line_of<'a>(state: &'a StateFile, dev: &DeviceCfg) -> Option<&'a DevLine> {
    state
        .devices
        .iter()
        .find(|l| l.name == dev.name && l.fingerprint == dev.fingerprint && l.state != DevState::Pending)
}

/// Connected, active or offline: an attached device the daemon does not
/// report (or a daemon that does not run) is offline.
pub fn device_state(tab: &DeskflowTab, dev: &DeviceCfg) -> DevState {
    match line_of(&tab.state, dev).map(|l| l.state) {
        Some(s @ (DevState::Connected | DevState::Active)) if tab.running => s,
        _ => DevState::Offline,
    }
}

/// The size the device reported, else `DEFAULT_DEVICE`.
pub fn device_size(state: &StateFile, dev: &DeviceCfg) -> (u32, u32) {
    line_of(state, dev)
        .map(|l| (l.width, l.height))
        .filter(|&(w, h)| w > 0 && h > 0)
        .unwrap_or(DEFAULT_DEVICE)
}

/// The size of `o` on the desktop: its mode, else the head's current or
/// preferred mode, else `NO_MODE`.
fn output_size(o: &kanshi::OutputSet, heads: &[Head]) -> (i32, i32) {
    if o.mode.w > 0 && o.mode.h > 0 {
        return kanshi::size(o);
    }
    heads
        .iter()
        .find(|h| kanshi::criteria(h, heads) == o.criteria)
        .and_then(|h| {
            h.current
                .and_then(|i| h.modes.get(i))
                .or_else(|| h.modes.iter().find(|m| m.preferred))
                .or_else(|| h.modes.first())
        })
        .map_or(NO_MODE, |m| (m.w as i32, m.h as i32))
}

/// The monitors that are on, by criteria, where `profile` puts them.
pub fn output_rects(profile: &Profile, heads: &[Head]) -> Vec<(String, Rect)> {
    let Ok(placed) = kanshi::place_all(profile) else {
        return Vec::new();
    };
    profile
        .outputs
        .iter()
        .zip(placed)
        .filter_map(|(o, p)| {
            let (x, y) = p?;
            let (w, h) = output_size(o, heads);
            Some((o.criteria.clone(), Rect { x, y, w, h }))
        })
        .collect()
}

/// The edges of the monitors that are on in `profile` that no other
/// monitor touches and no device in `devices` holds, the device named
/// `except` left out: what the Position control offers.
pub fn free_edges(
    profile: &Profile,
    heads: &[Head],
    devices: &[DeviceCfg],
    except: Option<&str>,
) -> Vec<(String, Side)> {
    let rects = output_rects(profile, heads);
    let taken = |c: &str, s: Side| {
        devices
            .iter()
            .any(|d| Some(d.name.as_str()) != except && d.output == c && d.side == s)
    };
    let mut free = Vec::new();
    for (i, (criteria, r)) in rects.iter().enumerate() {
        let others: Vec<Rect> = rects
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != i)
            .map(|(_, (_, o))| *o)
            .collect();
        for side in SIDES {
            if edge::edge_free(*r, side, &others) && !taken(criteria, side) {
                free.push((criteria.clone(), side));
            }
        }
    }
    free
}

/// The edges the Position control of the device `name` offers: the free
/// ones and the edge a device of that name holds, so a device attached
/// again under a new fingerprint can keep its edge.
pub fn position_edges(profile: &Profile, heads: &[Head], devices: &[DeviceCfg], name: &str) -> Vec<(String, Side)> {
    free_edges(profile, heads, devices, Some(name))
}

/// The edge of `dev` is not free in `profile` (its monitor missing or
/// off, or another monitor at that edge): it gets no strip.
pub fn inactive(profile: &Profile, heads: &[Head], devices: &[DeviceCfg], dev: &DeviceCfg) -> bool {
    !free_edges(profile, heads, devices, Some(&dev.name))
        .iter()
        .any(|(c, s)| *c == dev.output && *s == dev.side)
}

/// The attached devices the band lists with their rows: the selected
/// one, and those the arrangement does not show at their edge (their
/// monitor off or missing, or the edge not free), so they can be moved
/// or forgotten.
pub fn band_devices<'a>(tab: &'a DeskflowTab, profile: Option<&Profile>, heads: &[Head]) -> Vec<&'a DeviceCfg> {
    let shown = |d: &DeviceCfg| {
        profile.is_some_and(|p| {
            p.outputs.iter().any(|o| o.enabled && o.criteria == d.output) && !inactive(p, heads, &tab.cfg.devices, d)
        })
    };
    tab.cfg
        .devices
        .iter()
        .filter(|d| tab.selected.as_deref() == Some(d.name.as_str()) || !shown(d))
        .collect()
}

/// "Deskflow stopped: <reason>" while the daemon does not run and its last
/// run failed.
pub fn stopped_line(tab: &DeskflowTab) -> Option<String> {
    if tab.running {
        return None;
    }
    tab.stopped.as_ref().map(|why| format!("Deskflow stopped: {why}"))
}

/// "Right of DP-1".
pub fn edge_label(side: Side, monitor: &str) -> String {
    let word = match side {
        Side::LeftOf => "Left of",
        Side::RightOf => "Right of",
        Side::Above => "Above",
        Side::Below => "Below",
    };
    format!("{word} {monitor}")
}

/// "This computer: <addr>:<port>" from the state file's head while the
/// daemon runs. A wildcard listen address shows `ipv4`, the machine's
/// address, when it is known.
pub fn self_line(tab: &DeskflowTab, ipv4: Option<std::net::Ipv4Addr>) -> Option<String> {
    let StateHead::SelfInfo { listen, .. } = &tab.state.head else {
        return None;
    };
    if !tab.running {
        return None;
    }
    let (host, port) = listen.rsplit_once(':')?;
    let wildcard = matches!(host, "" | "0.0.0.0" | "::" | "[::]");
    let host = match (wildcard, ipv4) {
        (true, Some(ip)) => ip.to_string(),
        _ => host.to_string(),
    };
    Some(format!("This computer: {host}:{port}"))
}

/// ThinFloe's own fingerprint from the state file's head.
pub fn self_fingerprint(tab: &DeskflowTab) -> Option<&str> {
    match &tab.state.head {
        StateHead::SelfInfo { fingerprint, .. } if tab.running && !fingerprint.is_empty() => Some(fingerprint),
        _ => None,
    }
}

/// The state file at `path`; no file, or one that cannot be read, is no
/// state.
pub fn read_state(path: &std::path::Path) -> StateFile {
    /// Eight pending lines and a few devices fit many times over.
    const MAX_BYTES: u64 = 64 * 1024;
    crate::bounded::read_text(path, MAX_BYTES).map_or_else(no_state, |t| statefile::parse(&t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heads::{Mode, Rotation};
    use crate::kanshi::{OutputSet, Place};

    const FP_A: &str = "v2:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const FP_B: &str = "v2:sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn line(name: &str, fp: &str, state: DevState) -> DevLine {
        DevLine {
            name: name.into(),
            fingerprint: fp.into(),
            state,
            since: 1,
            width: 0,
            height: 0,
            flags: vec![],
        }
    }

    fn device(name: &str, fp: &str, output: &str, side: Side) -> DeviceCfg {
        DeviceCfg {
            name: name.into(),
            fingerprint: fp.into(),
            output: output.into(),
            side,
            clipboard: true,
        }
    }

    fn out(criteria: &str, place: Place) -> OutputSet {
        OutputSet {
            criteria: criteria.into(),
            enabled: true,
            mode: Mode {
                w: 1920,
                h: 1080,
                refresh_mhz: 60_000,
                preferred: true,
            },
            rotation: Rotation::Normal,
            place,
        }
    }

    /// DP-1 with DP-2 to its right.
    fn two() -> Profile {
        Profile {
            outputs: vec![
                out("DP-1", Place::Origin),
                out(
                    "DP-2",
                    Place::Next {
                        to: "DP-1".into(),
                        side: Side::RightOf,
                    },
                ),
            ],
            menu_output: None,
        }
    }

    fn with_pending() -> DeskflowTab {
        let mut tab = DeskflowTab::default();
        tab.state.devices = vec![line("laptop", FP_A, DevState::Pending)];
        tab
    }

    #[test]
    fn attach_then_place_saves_device() {
        let mut tab = with_pending();
        let r = apply(
            &mut tab,
            DeskflowEvent::Attach {
                name: "laptop".into(),
                fp: FP_A.into(),
            },
        );
        assert_eq!(r, None, "Attach only opens the Position control");
        assert_eq!(tab.attaching, Some(("laptop".into(), FP_A.into())));
        let r = apply(
            &mut tab,
            DeskflowEvent::Place {
                name: "laptop".into(),
                output: "DP-2".into(),
                side: Side::RightOf,
            },
        );
        let want = device("laptop", FP_A, "DP-2", Side::RightOf);
        let Some(DeskflowRequest::Save(cfg)) = r else {
            panic!("no save: {r:?}");
        };
        assert_eq!(cfg.devices, vec![want.clone()]);
        assert_eq!(tab.cfg.devices, vec![want]);
        assert_eq!(tab.attaching, None);
    }

    #[test]
    fn attach_again_with_a_new_fingerprint_replaces_the_device() {
        let mut tab = with_pending();
        tab.cfg.devices = vec![device("laptop", FP_B, "DP-1", Side::LeftOf)];
        apply(
            &mut tab,
            DeskflowEvent::Attach {
                name: "laptop".into(),
                fp: FP_A.into(),
            },
        );
        apply(
            &mut tab,
            DeskflowEvent::Place {
                name: "laptop".into(),
                output: "DP-2".into(),
                side: Side::RightOf,
            },
        );
        assert_eq!(tab.cfg.devices, vec![device("laptop", FP_A, "DP-2", Side::RightOf)]);
    }

    #[test]
    fn place_moves_an_attached_device() {
        let mut tab = DeskflowTab::default();
        tab.cfg.devices = vec![device("laptop", FP_A, "DP-1", Side::LeftOf)];
        let r = apply(
            &mut tab,
            DeskflowEvent::Place {
                name: "laptop".into(),
                output: "DP-2".into(),
                side: Side::Below,
            },
        );
        assert!(matches!(r, Some(DeskflowRequest::Save(_))));
        assert_eq!(tab.cfg.devices, vec![device("laptop", FP_A, "DP-2", Side::Below)]);
        // A name neither attaching nor attached changes nothing.
        let r = apply(
            &mut tab,
            DeskflowEvent::Place {
                name: "other".into(),
                output: "DP-2".into(),
                side: Side::Above,
            },
        );
        assert_eq!(r, None);
    }

    #[test]
    fn reject_sends_cmd() {
        let mut tab = with_pending();
        tab.attaching = Some(("laptop".into(), FP_A.into()));
        let r = apply(
            &mut tab,
            DeskflowEvent::Reject {
                name: "laptop".into(),
                fp: FP_A.into(),
            },
        );
        assert_eq!(
            r,
            Some(DeskflowRequest::Send(Cmd::Reject {
                name: "laptop".into(),
                fingerprint: FP_A.into(),
            }))
        );
        assert_eq!(tab.attaching, None, "a rejected device's Position control closes");
        assert!(tab.cfg.devices.is_empty());
    }

    #[test]
    fn forget_removes() {
        let mut tab = DeskflowTab::default();
        tab.cfg.devices = vec![
            device("laptop", FP_A, "DP-1", Side::LeftOf),
            device("mac", FP_B, "DP-2", Side::RightOf),
        ];
        tab.selected = Some("laptop".into());
        let r = apply(&mut tab, DeskflowEvent::Forget { name: "laptop".into() });
        let Some(DeskflowRequest::Save(cfg)) = r else {
            panic!("no save: {r:?}");
        };
        assert_eq!(cfg.devices, vec![device("mac", FP_B, "DP-2", Side::RightOf)]);
        assert_eq!(tab.selected, None);
        assert_eq!(apply(&mut tab, DeskflowEvent::Forget { name: "laptop".into() }), None);
    }

    #[test]
    fn free_edges_excludes_shared_and_taken() {
        let p = two();
        let all = free_edges(&p, &[], &[], None);
        // DP-1's right edge and DP-2's left edge touch each other.
        assert_eq!(
            all,
            vec![
                ("DP-1".to_string(), Side::LeftOf),
                ("DP-1".to_string(), Side::Above),
                ("DP-1".to_string(), Side::Below),
                ("DP-2".to_string(), Side::RightOf),
                ("DP-2".to_string(), Side::Above),
                ("DP-2".to_string(), Side::Below),
            ]
        );
        let devs = [device("laptop", FP_A, "DP-2", Side::RightOf)];
        let free = free_edges(&p, &[], &devs, None);
        assert!(!free.contains(&("DP-2".to_string(), Side::RightOf)), "taken by laptop");
        assert_eq!(free.len(), 5);
        // The device itself may keep its edge.
        assert!(free_edges(&p, &[], &devs, Some("laptop")).contains(&("DP-2".to_string(), Side::RightOf)));
        // A monitor that is off has no edges.
        let mut off = two();
        off.outputs[1].enabled = false;
        assert_eq!(free_edges(&off, &[], &[], None).len(), 4);
    }

    #[test]
    fn the_band_lists_a_device_whose_edge_another_monitor_took() {
        let mut tab = DeskflowTab::default();
        tab.cfg.devices = vec![
            device("laptop", FP_A, "DP-1", Side::RightOf),
            device("mac", FP_B, "DP-2", Side::RightOf),
        ];
        let names = |tab: &DeskflowTab, p: Option<&Profile>| {
            band_devices(tab, p, &[])
                .iter()
                .map(|d| d.name.clone())
                .collect::<Vec<_>>()
        };
        // DP-2 docked right of DP-1 takes the laptop's edge.
        assert_eq!(names(&tab, Some(&two())), vec!["laptop".to_string()]);
        // The selected device is listed too.
        tab.selected = Some("mac".into());
        assert_eq!(names(&tab, Some(&two())), vec!["laptop".to_string(), "mac".to_string()]);
        tab.selected = None;
        // DP-1 alone: the laptop's edge is free, DP-2 is missing.
        let mut one = two();
        one.outputs.truncate(1);
        assert_eq!(names(&tab, Some(&one)), vec!["mac".to_string()]);
        // No arrangement: every device is listed.
        assert_eq!(names(&tab, None).len(), 2);
    }

    #[test]
    fn the_stopped_line_shows_only_while_the_daemon_does_not_run() {
        let mut tab = DeskflowTab {
            stopped: Some("port 24800 is in use".into()),
            running: false,
            ..DeskflowTab::default()
        };
        assert_eq!(
            stopped_line(&tab).as_deref(),
            Some("Deskflow stopped: port 24800 is in use")
        );
        tab.running = true;
        assert_eq!(stopped_line(&tab), None);
        tab.running = false;
        tab.stopped = None;
        assert_eq!(stopped_line(&tab), None);
    }

    #[test]
    fn inactive_when_the_edge_is_shared_or_the_monitor_missing() {
        let p = two();
        let at = |o: &str, s| device("laptop", FP_A, o, s);
        assert!(!inactive(&p, &[], &[], &at("DP-2", Side::RightOf)));
        assert!(inactive(&p, &[], &[], &at("DP-1", Side::RightOf)));
        assert!(inactive(&p, &[], &[], &at("HDMI-1", Side::RightOf)));
    }

    #[test]
    fn listen_toggle_saves() {
        let mut tab = DeskflowTab::default();
        let r = apply(&mut tab, DeskflowEvent::Listen(true));
        let Some(DeskflowRequest::Save(cfg)) = r else {
            panic!("no save: {r:?}");
        };
        assert!(cfg.listen);
        assert!(tab.cfg.listen);
        assert_eq!(
            apply(&mut tab, DeskflowEvent::Listen(true)),
            None,
            "no change, no write"
        );
        let r = apply(&mut tab, DeskflowEvent::Resistance(Resistance::High));
        assert!(matches!(r, Some(DeskflowRequest::Save(c)) if c.edge_resistance == Resistance::High));
    }

    #[test]
    fn select_and_cancel_change_only_the_tab() {
        let mut tab = with_pending();
        tab.cfg.devices = vec![device("mac", FP_B, "DP-1", Side::LeftOf)];
        assert_eq!(apply(&mut tab, DeskflowEvent::Select("mac".into())), None);
        assert_eq!(tab.selected.as_deref(), Some("mac"));
        apply(
            &mut tab,
            DeskflowEvent::Attach {
                name: "laptop".into(),
                fp: FP_A.into(),
            },
        );
        assert_eq!(apply(&mut tab, DeskflowEvent::CancelAttach), None);
        assert_eq!(tab.attaching, None);
    }

    #[test]
    fn device_state_and_size_come_from_the_line_of_that_fingerprint() {
        let mut tab = DeskflowTab::default();
        let d = device("laptop", FP_A, "DP-1", Side::LeftOf);
        let mut l = line("laptop", FP_A, DevState::Active);
        l.width = 2560;
        l.height = 1600;
        tab.state.devices = vec![l, line("laptop", FP_B, DevState::Pending)];
        assert_eq!(device_state(&tab, &d), DevState::Offline, "no daemon runs");
        tab.running = true;
        assert_eq!(device_state(&tab, &d), DevState::Active);
        assert_eq!(device_size(&tab.state, &d), (2560, 1600));
        let other = device("laptop", FP_B, "DP-1", Side::LeftOf);
        assert_eq!(
            device_state(&tab, &other),
            DevState::Offline,
            "a pending line is not its state"
        );
        assert_eq!(device_size(&tab.state, &other), DEFAULT_DEVICE);
    }

    #[test]
    fn self_line_shows_the_machine_address_for_a_wildcard() {
        let mut tab = DeskflowTab::default();
        tab.state.head = StateHead::SelfInfo {
            fingerprint: FP_A.into(),
            listen: "0.0.0.0:24800".into(),
        };
        assert_eq!(self_line(&tab, None), None, "nothing while the daemon does not run");
        tab.running = true;
        let ip = Some(std::net::Ipv4Addr::new(192, 168, 1, 23));
        assert_eq!(
            self_line(&tab, ip).as_deref(),
            Some("This computer: 192.168.1.23:24800")
        );
        assert_eq!(self_line(&tab, None).as_deref(), Some("This computer: 0.0.0.0:24800"));
        assert_eq!(self_fingerprint(&tab), Some(FP_A));
    }

    #[test]
    fn pending_rows_show_only_while_the_daemon_runs() {
        // A state file left from an earlier run lists a device that can be
        // neither attached nor rejected.
        let mut tab = with_pending();
        assert!(pending(&tab).is_empty());
        tab.running = true;
        assert_eq!(pending(&tab).len(), 1);
    }

    #[test]
    fn a_device_attached_again_may_keep_its_own_edge() {
        let p = two();
        let devs = [device("laptop", FP_B, "DP-2", Side::RightOf)];
        let own = ("DP-2".to_string(), Side::RightOf);
        // The pending line of the laptop's new fingerprint.
        assert!(position_edges(&p, &[], &devs, "laptop").contains(&own));
        assert!(!position_edges(&p, &[], &devs, "mac").contains(&own));
    }

    #[test]
    fn edge_labels() {
        assert_eq!(edge_label(Side::RightOf, "DP-1"), "Right of DP-1");
        assert_eq!(edge_label(Side::Above, "HDMI-A-1"), "Above HDMI-A-1");
    }
}
