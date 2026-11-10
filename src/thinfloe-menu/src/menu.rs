//! The menu's state machine, apart from any drawing: which tile may be
//! activated, what activating it starts, and what it writes to the log.
//! Several programs run at once, in the background, next to at most one
//! save; nothing here blocks the event loop. The dialog shows only during
//! a logout.
//!
//! The Settings tile swaps the grid for the Settings screen, whose rows
//! start thinfloe-addon. A ticked add-on that failed to install is offered a
//! retry in a dialog over the grid, once per boot.
//!
//! Log out saves the home directory here, behind the dialog, before the
//! menu exits (decided 2026-10-02). thinfloe-session no longer saves after a
//! logout: by the time the menu exits 0 the save has been done, or it
//! failed and the person at the machine chose to log out anyway. Restart
//! and Power off take the same steps and exit 10 and 11. A running add-on
//! job is listed in the dialog too; it is waited for, never stopped.
//!
//! The Upgrade tile swaps the grid for the Upgrade screen (`upgrade.rs`).
//! A stick write it started runs on when the person goes back to the grid,
//! and holds the session's end like an add-on job: Log out waits for it,
//! Restart anyway and Power off anyway send it SIGTERM, never SIGKILL. A
//! SIGKILL would reach only sudo, and the tool would go on holding its lock.

use crate::addons::{AddonState, AddonStatus, valid_id};
use crate::jobs::{JobId, JobResult, SaveOutcome, Signal};
use crate::log::{Kind, Log};
use crate::monitors::{MonEvent, MonRequest, MonTab};
use crate::netfiles;
use crate::network::{NetRequest, NetTab};
use crate::screentab::{self, IdleSource, ScreenTab};
use crate::settings::{RetryMarker, SettingsEvent, SettingsTab};
use crate::tiles::{Action, Grid, Origin, Tile};
use crate::upgrade::{Request, Upgrade, UpgradeEvent};
use std::time::{Duration, Instant};

/// How long "Home directory saved. Logging out." stays on screen.
pub const LOGOUT_PAUSE: Duration = Duration::from_millis(1500);

pub const RESTORE_FAILED_REASON: &str = "this boot's home restore failed, so saving would overwrite the stored home";

/// A screen timeout the running session did not take.
pub const NEXT_SESSION: &str = "The new screen timeout applies from the next session.";

/// How long programs get to end after SIGTERM before SIGKILL, at logout.
pub const STOP_GRACE: Duration = Duration::from_secs(5);

/// An add-on job: `sudo -n thinfloe-addon <args>`, in a process group of
/// its own, because a stop signals the whole group, which would otherwise
/// be the menu's. sudo receives the SIGTERM and relays it to the tool.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AddonRun {
    args: Vec<String>,
    /// Known once the worker reports the start.
    pgid: Option<i32>,
}

/// A program a tile started that has not ended yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub id: JobId,
    pub tile: usize,
    pub name: String,
    /// Known once the worker reports the start.
    pub pgid: Option<i32>,
    /// When the tile was clicked: a Save-On-Exit program's start begins
    /// the network element's session total.
    pub started: Instant,
}

/// The background save: one at a time, and one more at most queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SaveState {
    pub running: bool,
    pub again: bool,
}

/// The steps of a logout. While one is set the dialog is up and the grid
/// frozen behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Logout {
    /// Programs or add-on jobs still run: Stay, or Stop them and log out.
    Confirm,
    /// SIGTERM sent; SIGKILL at `kill_at` to what is left.
    Stopping { kill_at: Instant, killed: bool },
    /// The programs are gone; an add-on job or a stick write still runs.
    /// Log out does not stop it: the job runs as root, and a dpkg cut
    /// short leaves a half-installed package for the rest of the boot.
    /// Stay is offered, and for Restart and Power off also "Restart
    /// anyway": the rootfs is in RAM and goes away with the restart.
    WaitAddons,
    /// Restart or Power off anyway: SIGTERM went to the add-on jobs and the
    /// write; the menu goes on when they ended or at `go_at`, whichever
    /// comes first. The shutdown ends what is left; nothing gets SIGKILL.
    StoppingAddons { go_at: Instant },
    /// The programs are gone; a background save is still running.
    WaitSave,
    /// The logout save runs.
    Saving { since: Instant },
    /// The logout save did not happen: Stay, or Log out anyway.
    Failed { message: String },
    /// Saved; the menu exits 0 at `until`.
    Leaving { until: Instant },
}

impl Logout {
    /// The steps without buttons drop input before egui sees it.
    fn refuses_input(&self) -> bool {
        !matches!(self, Logout::Confirm | Logout::WaitAddons | Logout::Failed { .. })
    }
}

/// The answers the logout dialog offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Stay,
    StopAndLeave,
    LeaveAnyway,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    Save,
    Run {
        id: JobId,
        name: String,
        argv: Vec<String>,
    },
    /// `sudo -n /usr/bin/thinfloe-addon <args>`. What it does is in the
    /// add-on log already, so its stdout is not logged a second time.
    Addon {
        args: Vec<String>,
    },
    /// `sudo -n /usr/bin/thinfloe-upgrade <args>`, a write.
    UpgradeWrite {
        args: Vec<String>,
    },
    /// `thinfloe-upgrade drives --no-size [--all]`.
    Drives {
        all: bool,
    },
    /// `thinfloe-upgrade check [--source URL]`.
    UpgradeCheck {
        source: Option<String>,
    },
    /// Read the network files of the home and the machine's WiFi adapters.
    NetLoad,
    /// `thinfloe-network status`.
    NetStatus,
    /// `thinfloe-network scan`.
    NetScan,
    /// Save `change` into the home, then `sudo -n thinfloe-network apply`,
    /// in a process group of its own.
    NetApply {
        change: netfiles::Change,
    },
    /// *Let the menu manage it* for `"wifi"` or `"wired"`.
    NetTakeOver {
        which: &'static str,
    },
    /// Read `~/.config/kanshi/config`.
    MonLoad,
    /// Write the monitor layout and let kanshi apply it, undoably.
    MonApply {
        text: String,
        menu_output: Option<String>,
        /// Kept as `config.prev` when there is no file.
        prev: Option<String>,
    },
    /// The new layout is confirmed.
    MonKeep,
    /// Put the layout before the Apply back.
    MonRevert,
    /// Move a hand-written kanshi config aside.
    MonTakeOver,
}

/// What fills the space above the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Grid,
    Settings,
    Upgrade,
}

/// The question over the grid: a ticked add-on that has no copy, because
/// its install or its check failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retry {
    pub id: String,
    pub name: String,
    pub message: Option<String>,
}

/// The add-on id of a tile: its file name without `.desktop`.
pub fn addon_id(tile: &Tile) -> &str {
    tile.file.trim_end_matches(".desktop")
}

/// What the event loop must do after a state change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Start(Job),
    /// Send the signal to each of these process groups.
    Signal(Vec<i32>, Signal),
    Exit(i32),
    /// Write the screen file with this idle time, then report with
    /// `screen_written`.
    WriteScreen {
        idle: u32,
    },
    /// Put the menu's header on the screen file written by hand, then
    /// report with `screen_written`.
    TakeOverScreen,
}

pub struct Menu {
    pub tiles: Vec<Tile>,
    pub banner: Vec<String>,
    pub jobs: Vec<Running>,
    pub save: SaveState,
    pub logout: Option<Logout>,
    /// The action that started the logout steps: Log out, Restart or Power
    /// off. Its exit code ends the menu.
    pub ending: Action,
    /// What happened, for the log under the grid.
    pub log: Log,
    restore_failed: bool,
    next_id: JobId,
    /// The last status of the add-ons. An add-on tile with no line here is
    /// not drawn.
    addons: Vec<AddonStatus>,
    pub screen: Screen,
    pub retry: Option<Retry>,
    /// Each add-on job that runs.
    addon_runs: Vec<AddonRun>,
    /// The devices the battery file lists; set by the app on every change.
    pub battery: Vec<crate::battery::Device>,
    /// When `battery` was set: the status area's battery pages count from
    /// here, so a new reading starts at the first page.
    pub battery_since: Option<Instant>,
    /// The status area: which elements show, at which size, in which order.
    pub status: crate::status::Area,
    /// The network sampler's last copy, set by the app before each frame.
    pub netstat: Option<crate::netstat::Snapshot>,
    /// Where the network element's session total counts from.
    pub netstat_since: crate::netstat::Since,
    /// The hardware sampler's last copy, set by the app before each frame.
    pub hardware: Option<crate::hardware::Snapshot>,
    /// `Snapshot::total` at each running session program's first copy, so
    /// that when the oldest ends, the next one's total starts where its own
    /// label says.
    network_bases: Vec<(JobId, (u64, u64))>,
    /// A Save-On-Exit program ended while the end-of-session question
    /// waited for an add-on job: the save is done only if the person stays.
    deferred_save: bool,
    /// When the add-ons were last checked, from the tool's last-check file.
    pub last_check: Option<u64>,
    retry_marker: RetryMarker,
    /// No retry is offered while set: at startup the status is read before
    /// the grid was ever on screen, and the dialog would dim an empty one.
    retry_held: bool,
    /// Windows folded away, as the compositor reports them; the strip
    /// below the log shows them.
    pub folded: Vec<crate::windows::Folded>,
    /// The Upgrade screen, and the write it runs.
    pub upgrade: Upgrade,
    /// The machine booted from a stick; the Upgrade tile is hidden on netboot.
    usb_boot: bool,
    /// The tab the Settings screen shows; Packages each time it opens.
    pub settings_tab: SettingsTab,
    /// The screen timeout, as the Screen tab shows it.
    pub screen_tab: ScreenTab,
    /// The Network tab, and the Apply it runs.
    pub network: NetTab,
    /// The monitors of the Screen tab, and the Apply it runs.
    pub monitors: MonTab,
}

impl Menu {
    /// `restore_failed` greys the save tile out with the reason, before
    /// anyone waits a minute for thinfloe-save-home to refuse.
    pub fn new(grid: Grid, restore_failed: bool) -> Menu {
        let mut tiles = grid.tiles;
        if restore_failed {
            for t in tiles.iter_mut().filter(|t| t.usable() && t.action == Action::SaveHome) {
                t.problem = Some(RESTORE_FAILED_REASON.to_string());
            }
        }
        let mut log = Log::default();
        for line in &grid.banner {
            log.note(Kind::Warn, line.clone());
        }
        Menu {
            tiles,
            banner: grid.banner,
            jobs: Vec::new(),
            save: SaveState::default(),
            logout: None,
            ending: Action::Logout,
            log,
            restore_failed,
            next_id: 1,
            addons: Vec::new(),
            screen: Screen::Grid,
            retry: None,
            addon_runs: Vec::new(),
            battery: Vec::new(),
            battery_since: None,
            status: crate::status::Area::default(),
            netstat: None,
            netstat_since: crate::netstat::Since::default(),
            hardware: None,
            network_bases: Vec::new(),
            deferred_save: false,
            last_check: None,
            retry_marker: RetryMarker::memory(),
            retry_held: false,
            folded: Vec::new(),
            upgrade: Upgrade::new(),
            usb_boot: false,
            settings_tab: SettingsTab::Packages,
            screen_tab: ScreenTab::default(),
            // Until `set_usb_boot` says otherwise: netboot offers less.
            network: NetTab::new(true),
            monitors: MonTab::default(),
        }
    }

    pub fn set_usb_boot(&mut self, usb: bool) {
        self.usb_boot = usb;
        self.network = NetTab::new(!usb);
    }

    fn net_request(&self, r: NetRequest) -> Effect {
        Effect::Start(match r {
            NetRequest::Load => Job::NetLoad,
            NetRequest::Status => Job::NetStatus,
            NetRequest::Scan => Job::NetScan,
            NetRequest::Apply(change) => Job::NetApply { change },
            NetRequest::TakeOver(which) => Job::NetTakeOver { which },
        })
    }

    fn mon_request(r: MonRequest) -> Effect {
        Effect::Start(match r {
            MonRequest::Load => Job::MonLoad,
            MonRequest::Apply {
                text,
                menu_output,
                prev,
            } => Job::MonApply {
                text,
                menu_output,
                prev,
            },
            MonRequest::Keep => Job::MonKeep,
            MonRequest::Revert => Job::MonRevert,
            MonRequest::TakeOver => Job::MonTakeOver,
        })
    }

    /// The compositor reported the monitors.
    pub fn heads_changed(&mut self, heads: Vec<crate::heads::Head>) -> Option<Effect> {
        self.monitors.heads_changed(heads).map(Menu::mon_request)
    }

    /// Something on the monitor part of the Screen tab.
    pub fn monitors_event(&mut self, e: MonEvent, now: Instant) -> Option<Effect> {
        if self.logout.is_some() {
            return None;
        }
        self.monitors.event(e, now).map(Menu::mon_request)
    }

    /// A ThinLinc client runs; the Screen tab says so.
    fn thinlinc_running(&self) -> bool {
        self.jobs
            .iter()
            .any(|j| self.tiles.get(j.tile).is_some_and(|t| addon_id(t) == "10-thinlinc"))
    }

    /// A ThinLinc or OxuLnk session runs: an Apply may cut it off.
    fn remote_running(&self) -> bool {
        self.jobs.iter().any(|j| {
            self.tiles
                .get(j.tile)
                .is_some_and(|t| matches!(addon_id(t), "10-thinlinc" | "20-oxulnk"))
        })
    }

    /// An Apply or a take-over changed the home: save it. While the
    /// end-of-session question is up the logout save follows, unless the
    /// person stays: then this one is owed.
    fn net_changed_home(&mut self) -> Option<Effect> {
        if self.logout.is_some() && !self.restore_failed {
            self.deferred_save = true;
            return None;
        }
        self.save_after_change()
    }

    /// This boot's home directory was not restored.
    pub fn restore_failed(&self) -> bool {
        self.restore_failed
    }

    /// The release this machine runs, from `/etc/thinfloe/version`.
    pub fn set_installed(&mut self, v: Option<String>) {
        self.upgrade.installed = v;
    }

    /// A newer release than the installed one, when both are known.
    fn update_available(&self) -> Option<&str> {
        crate::release::update_available(&self.upgrade.release, self.upgrade.installed.as_deref())
    }

    /// A click on the Upgrade screen.
    pub fn upgrade_event(&mut self, e: UpgradeEvent, now: Instant) -> Option<Effect> {
        if self.dialog() || self.screen != Screen::Upgrade {
            return None;
        }
        let r = self.upgrade.event(e, self.restore_failed, now)?;
        self.upgrade_request(r, now)
    }

    fn upgrade_request(&mut self, r: Request, now: Instant) -> Option<Effect> {
        match r {
            Request::Check { source } => Some(Effect::Start(Job::UpgradeCheck { source })),
            Request::Drives { all } => Some(Effect::Start(Job::Drives { all })),
            Request::Write { args } => Some(Effect::Start(Job::UpgradeWrite { args })),
            // Not known yet: the signal goes out with UpgradeStarted.
            Request::Stop => self.upgrade.pgid().map(|p| Effect::Signal(vec![p], Signal::Term)),
            Request::Restart => self.begin_ending(Action::Reboot, now),
            Request::Leave => {
                self.screen = Screen::Grid;
                self.offer_retry();
                None
            }
        }
    }

    /// The root jobs that run, for the end-of-session dialog: the add-on
    /// jobs, then the stick write, then a network Apply.
    pub fn root_jobs(&self) -> Vec<String> {
        let mut v = self.addon_jobs();
        v.extend(self.upgrade.job_name());
        if self.network.applying() {
            v.push(NET_APPLYING.to_string());
        }
        v
    }

    /// No add-on job, no stick write and no network Apply runs.
    fn root_idle(&self) -> bool {
        self.addon_runs.is_empty() && !self.upgrade.writing() && !self.network.applying()
    }

    /// The known process groups of the root jobs.
    fn root_pgids(&self) -> Vec<i32> {
        self.addon_runs
            .iter()
            .filter_map(|a| a.pgid)
            .chain(self.upgrade.pgid())
            .chain(self.network.pgid())
            .collect()
    }

    /// The name the grid draws: the Upgrade tile names a newer release,
    /// unless the person gave it a name of their own.
    pub fn tile_name(&self, index: usize) -> String {
        let Some(t) = self.tiles.get(index) else {
            return String::new();
        };
        if t.action == Action::Upgrade
            && !matches!(t.origin, Origin::Reworded { .. })
            && let Some(v) = self.update_available()
        {
            return format!("Upgrade to {v}");
        }
        t.name.clone()
    }

    /// Where the add-ons offered a retry in this boot are remembered.
    pub fn set_retry_marker(&mut self, marker: RetryMarker) {
        self.retry_marker = marker;
    }

    /// The status line of the add-on a tile belongs to.
    fn addon_of(&self, index: usize) -> Option<&AddonStatus> {
        let tile = self.tiles.get(index).filter(|t| t.addon)?;
        let id = addon_id(tile);
        self.addons.iter().find(|a| a.id == id)
    }

    pub fn addons(&self) -> &[AddonStatus] {
        &self.addons
    }

    /// Whether a check of every add-on runs.
    pub fn checking(&self) -> bool {
        self.addon_runs.iter().any(|a| a.args == ["check"])
    }

    /// Whether an add-on job for `id` runs, or a check of every add-on.
    pub fn addon_busy(&self, id: &str) -> bool {
        self.checking() || self.addon_runs.iter().any(|a| a.args.iter().skip(1).any(|x| x == id))
    }

    /// Whether thinfloe-addon takes the add-on of this tile. It refuses an
    /// id `valid_id` refuses, and one that differs from another add-on's
    /// only in case, since both would share one directory on the vfat ESP:
    /// shipped tiles win, then the user's in file-name order.
    pub fn addon_usable(&self, index: usize) -> bool {
        let Some(t) = self.tiles.get(index).filter(|t| t.addon) else {
            return false;
        };
        let id = addon_id(t);
        fn rank(t: &Tile) -> (bool, &str) {
            (t.origin == Origin::User, &t.file)
        }
        valid_id(id)
            && !self
                .tiles
                .iter()
                .any(|o| o.addon && valid_id(addon_id(o)) && rank(o) < rank(t) && addon_id(o).eq_ignore_ascii_case(id))
    }

    fn is_addon(&self, id: &str) -> bool {
        (0..self.tiles.len()).any(|i| self.addon_usable(i) && addon_id(&self.tiles[i]) == id)
    }

    /// What each running add-on job does, by the add-on's name, for the
    /// end-of-session dialog.
    pub fn addon_jobs(&self) -> Vec<String> {
        let name = |id: &str| {
            self.tiles
                .iter()
                .find(|t| t.addon && addon_id(t) == id)
                .map_or(id, |t| t.name.as_str())
                .to_string()
        };
        self.addon_runs
            .iter()
            .map(|a| match a.args.as_slice() {
                [verb, id] if verb == "install" => format!("Installing {}", name(id)),
                [verb, id] if verb == "update" => format!("Updating {}", name(id)),
                [verb, id] if verb == "remove" => format!("Removing {}", name(id)),
                _ => "Checking the add-ons".to_string(),
            })
            .collect()
    }

    /// Take the status layout; each line of its file that was skipped or
    /// changed opens the log as a warning and is printed by `--check`, like
    /// the tile banner.
    pub fn set_status(&mut self, area: crate::status::Area) {
        for note in &area.layout.notes {
            self.log.note(Kind::Warn, note.clone());
            self.banner.push(note.clone());
        }
        self.status = area;
    }

    /// What the status elements draw from.
    pub fn status_sources(&self) -> crate::status::Data<'_> {
        crate::status::Data {
            battery: &self.battery,
            battery_since: self.battery_since,
            network: self.netstat.as_ref(),
            network_since: Some(&self.netstat_since),
            hardware: self.hardware.as_ref(),
        }
    }

    /// Take the hardware sampler's copy.
    pub fn set_hardware(&mut self, snap: crate::hardware::Snapshot) {
        self.hardware = Some(snap);
    }

    /// Take the network sampler's copy made at `now` (`wall` on the clock).
    /// The session total counts from the oldest Save-On-Exit program that
    /// runs (ThinLinc, OxuLnk), from its first frame after the click; with
    /// none running, from the menu start.
    pub fn set_network(&mut self, snap: crate::netstat::Snapshot, now: Instant, wall: std::time::SystemTime) {
        let session = self
            .jobs
            .iter()
            .filter(|j| self.tiles.get(j.tile).is_some_and(|t| t.save_on_exit))
            .min_by_key(|j| (j.started, j.id));
        self.network_bases
            .retain(|(id, _)| self.jobs.iter().any(|j| j.id == *id));
        for j in self
            .jobs
            .iter()
            .filter(|j| self.tiles.get(j.tile).is_some_and(|t| t.save_on_exit))
        {
            if !self.network_bases.iter().any(|(id, _)| *id == j.id) {
                self.network_bases.push((j.id, snap.total));
            }
        }
        match session {
            Some(j) if self.netstat_since.job != Some(j.id) => {
                let at = wall
                    .checked_sub(now.saturating_duration_since(j.started))
                    .unwrap_or(wall);
                self.netstat_since = crate::netstat::Since {
                    job: Some(j.id),
                    label: format!("since {}", crate::header::clock_text(at)),
                    base: self
                        .network_bases
                        .iter()
                        .find(|(id, _)| *id == j.id)
                        .map_or(snap.total, |(_, base)| *base),
                };
            }
            Some(_) => {}
            None => self.netstat_since = crate::netstat::Since::default(),
        }
        self.netstat = Some(snap);
    }

    /// Take the devices the battery file lists now.
    pub fn set_battery(&mut self, devices: Vec<crate::battery::Device>, now: Instant) {
        self.battery = devices;
        self.battery_since = Some(now);
    }

    fn start_addon(&mut self, args: Vec<String>) -> Option<Effect> {
        self.addon_runs.push(AddonRun {
            args: args.clone(),
            pgid: None,
        });
        Some(Effect::Start(Job::Addon { args }))
    }

    /// A click on the Settings screen.
    pub fn settings(&mut self, event: SettingsEvent) -> Option<Effect> {
        if self.logout.is_some() || self.screen != Screen::Settings {
            return None;
        }
        let (verb, id) = match event {
            SettingsEvent::Back => {
                // Plan call 5: what was not applied goes with the screen.
                if self.network.drop_draft() {
                    self.log.note(Kind::Warn, NET_DROPPED);
                }
                self.screen = Screen::Grid;
                self.offer_retry();
                return None;
            }
            SettingsEvent::Tab(tab) => {
                self.settings_tab = tab;
                return None;
            }
            SettingsEvent::IdleTimeout(idle) => {
                let tab = &self.screen_tab;
                let open = matches!(tab.source, IdleSource::Menu | IdleSource::Unset);
                return (open && idle != tab.idle).then_some(Effect::WriteScreen { idle });
            }
            SettingsEvent::ManageScreen => {
                return (self.screen_tab.source == IdleSource::ByHand).then_some(Effect::TakeOverScreen);
            }
            SettingsEvent::Network(e) => {
                let r = self.network.event(e, self.remote_running())?;
                return Some(self.net_request(r));
            }
            SettingsEvent::Monitors(e) => return self.monitors_event(e, Instant::now()),
            // No ids: the tool then checks only the sources it may check by
            // itself. With ids it would also store what a manual-only source
            // offers, and the next boot would install that unattended.
            SettingsEvent::CheckNow if self.checking() => return None,
            SettingsEvent::CheckNow => return self.start_addon(vec!["check".into()]),
            SettingsEvent::Tick { id, on: true } | SettingsEvent::InstallNow(id) => ("install", id),
            SettingsEvent::Tick { id, on: false } => ("remove", id),
            SettingsEvent::UpdateNow(id) => ("update", id),
        };
        if !self.is_addon(&id) || self.addon_busy(&id) {
            return None;
        }
        self.start_addon(vec![verb.into(), id])
    }

    /// Put up the retry question for the first ticked add-on without a copy
    /// whose install or check failed and that was not offered in this boot.
    /// On the Settings screen the row shows the same, so it is only noted.
    /// On the Upgrade screen nothing is recorded: the question waits for
    /// the grid.
    fn offer_retry(&mut self) {
        if self.retry_held || self.retry.is_some() || self.logout.is_some() || self.screen == Screen::Upgrade {
            return;
        }
        let failed: Vec<Retry> = self
            .tiles
            .iter()
            .filter(|t| t.addon)
            .filter_map(|t| {
                let id = addon_id(t);
                let s = self.addons.iter().find(|a| a.id == id)?;
                let failed = matches!(s.state, AddonState::Failed | AddonState::InstallFailed);
                (s.ticked && failed && s.installed.is_none() && !self.addon_busy(id) && !self.retry_marker.seen(id))
                    .then(|| Retry {
                        id: id.to_string(),
                        name: t.name.clone(),
                        message: s.message.clone(),
                    })
            })
            .collect();
        if self.screen == Screen::Settings {
            for r in &failed {
                self.retry_marker.record(&r.id);
            }
            return;
        }
        if let Some(r) = failed.into_iter().next() {
            self.retry_marker.record(&r.id);
            self.retry = Some(r);
        }
    }

    /// Hold back the retry question, or offer it now that it is let go.
    pub fn hold_retry(&mut self, held: bool) {
        self.retry_held = held;
        self.offer_retry();
    }

    pub fn retry_held(&self) -> bool {
        self.retry_held
    }

    /// Retry or Later in the retry dialog.
    pub fn answer_retry(&mut self, retry: bool) -> Option<Effect> {
        let r = self.retry.take()?;
        let effect = if retry && !self.addon_busy(&r.id) {
            self.start_addon(vec!["install".into(), r.id])
        } else {
            None
        };
        self.offer_retry();
        effect
    }

    /// Whether the grid draws the tile. An add-on tile is drawn while its
    /// add-on has a copy on this machine, a failed check included.
    pub fn visible(&self, index: usize) -> bool {
        match self.tiles.get(index) {
            Some(t) if t.addon => self.addon_of(index).is_some_and(AddonStatus::present),
            Some(t) if t.action == Action::Upgrade => self.usb_boot,
            Some(_) => true,
            None => false,
        }
    }

    /// The indices of the tiles the grid draws, in order.
    pub fn visible_tiles(&self) -> Vec<usize> {
        (0..self.tiles.len()).filter(|&i| self.visible(i)).collect()
    }

    /// Take a new status. Returns the add-on tiles whose installed version
    /// changed: their program and icon may have appeared or gone.
    pub fn set_addons(&mut self, status: Vec<AddonStatus>) -> Vec<usize> {
        let before: Vec<Option<String>> = (0..self.tiles.len())
            .map(|i| self.addon_of(i).and_then(|a| a.installed.clone()))
            .collect();
        self.addons = status;
        let changed = (0..self.tiles.len())
            .filter(|&i| self.tiles[i].addon && before[i] != self.addon_of(i).and_then(|a| a.installed.clone()))
            .collect();
        self.offer_retry();
        changed
    }

    /// Look again for the programs of the add-on tiles: they are installed
    /// while the menu runs. An add-on tile has no other reason to be
    /// disabled than its program.
    pub fn refresh_programs(&mut self, path: &std::ffi::OsStr) {
        for t in self.tiles.iter_mut().filter(|t| t.addon && t.action == Action::Run) {
            t.problem = None;
            crate::tiles::mark_missing_programs(std::slice::from_mut(t), path);
        }
    }

    /// Whether a dialog is up, so the grid behind it is frozen and dimmed.
    pub fn dialog(&self) -> bool {
        self.logout.is_some() || self.retry.is_some() || self.monitors.question_up()
    }

    /// Whether input is dropped before egui sees it.
    pub fn refuses_input(&self) -> bool {
        self.logout.as_ref().is_some_and(Logout::refuses_input)
    }

    /// The tile whose program is the window's `app_id`, case-insensitively:
    /// its icon stands for the window on the strip.
    pub fn tile_for_app(&self, app_id: &str) -> Option<usize> {
        if app_id.is_empty() {
            return None;
        }
        self.tiles.iter().position(|t| {
            t.action == Action::Run
                && t.argv.first().is_some_and(|a| {
                    let name = a.rsplit('/').next().unwrap_or(a);
                    name.eq_ignore_ascii_case(app_id)
                })
        })
    }

    pub fn running(&self, tile: usize) -> usize {
        self.jobs.iter().filter(|j| j.tile == tile).count()
    }

    pub fn can_activate(&self, index: usize) -> bool {
        let Some(tile) = self.tiles.get(index) else {
            return false;
        };
        if self.dialog() || self.screen != Screen::Grid || !tile.usable() || !self.visible(index) {
            return false;
        }
        match tile.action {
            Action::Run => tile.multiple || self.running(index) == 0,
            Action::SaveHome => !self.save.running,
            Action::Logout | Action::Reboot | Action::Poweroff | Action::Settings => true,
            // While a write runs, pressing it shows the progress.
            Action::Upgrade => true,
        }
    }

    /// Whether the tile is at work: a copy of its program runs, or, for
    /// the save tile, a save, or, for the Upgrade tile, a write.
    pub fn active(&self, index: usize) -> bool {
        let action = self.tiles.get(index).map(|t| t.action);
        self.running(index) > 0
            || (action == Some(Action::SaveHome) && self.save.running)
            || (action == Some(Action::Upgrade) && self.upgrade.writing())
    }

    /// Whether an add-on update waits for the next boot, or, for the
    /// Upgrade tile, a newer release is out. Independent of whether a copy
    /// runs now.
    pub fn pending_update(&self, index: usize) -> bool {
        if self.tiles.get(index).is_some_and(|t| t.action == Action::Upgrade) {
            return self.update_available().is_some();
        }
        self.addon_of(index).is_some_and(|a| a.state == AddonState::Pending)
    }

    pub fn activate(&mut self, index: usize, now: Instant) -> Option<Effect> {
        if !self.can_activate(index) {
            return None;
        }
        let tile = self.tiles[index].clone();
        match tile.action {
            Action::Run => {
                let id = self.next_id;
                self.next_id += 1;
                self.log.note(Kind::Event, format!("{} started.", tile.name));
                self.jobs.push(Running {
                    id,
                    tile: index,
                    name: tile.name.clone(),
                    pgid: None,
                    started: now,
                });
                self.monitors.set_thinlinc(self.thinlinc_running());
                Some(Effect::Start(Job::Run {
                    id,
                    name: tile.name.clone(),
                    argv: crate::tiles::command_line(&tile),
                }))
            }
            Action::SaveHome => self.request_save(),
            Action::Settings => {
                self.screen = Screen::Settings;
                self.settings_tab = SettingsTab::Packages;
                None
            }
            Action::Upgrade => {
                self.screen = Screen::Upgrade;
                let r = self.upgrade.open(now)?;
                self.upgrade_request(r, now)
            }
            Action::Logout | Action::Reboot | Action::Poweroff => self.begin_ending(tile.action, now),
        }
    }

    /// Log out, Restart or Power off: ask while programs or root jobs run,
    /// else save at once.
    fn begin_ending(&mut self, action: Action, now: Instant) -> Option<Effect> {
        self.ending = action;
        if self.jobs.is_empty() && self.root_idle() {
            self.logout_after_jobs(now)
        } else {
            self.logout = Some(Logout::Confirm);
            None
        }
    }

    /// The screen file was written, or could not be. `signalled` says the
    /// running session took the new timeout; else it waits for the next.
    /// The file lives in the home directory, so a change is saved.
    /// Called in the same `apply` as the effect, so no logout can have
    /// begun in between; hence no logout guard here.
    pub fn screen_written(&mut self, result: Result<ScreenTab, String>, signalled: bool) -> Option<Effect> {
        let tab = match result {
            Ok(tab) => tab,
            Err(message) => {
                self.log.note(Kind::Warn, message.clone());
                self.screen_tab.error = Some(message);
                return None;
            }
        };
        let text = match tab.idle {
            0 => "Screen never turns off.".to_string(),
            n => format!("Screen turns off after {}.", screentab::label(n)),
        };
        self.screen_tab = ScreenTab { error: None, ..tab };
        self.log.note(Kind::Event, text);
        if !signalled {
            self.log.note(Kind::Event, NEXT_SESSION);
        }
        self.save_after_change()
    }

    /// A file of the home changed: save, or say why not.
    fn save_after_change(&mut self) -> Option<Effect> {
        if self.restore_failed {
            self.log.note(Kind::Warn, SaveOutcome::RestoreFailed.message());
            return None;
        }
        self.request_save()
    }

    /// Start a background save, or queue one more behind the running one.
    fn request_save(&mut self) -> Option<Effect> {
        if self.save.running {
            self.save.again = true;
            return None;
        }
        self.save.running = true;
        self.log.note(Kind::Event, SAVING);
        Some(Effect::Start(Job::Save))
    }

    /// The programs are gone: wait for the add-on jobs and the stick
    /// write, then go on.
    fn after_programs(&mut self, now: Instant) -> Option<Effect> {
        if self.root_idle() {
            return self.logout_after_jobs(now);
        }
        self.logout = Some(Logout::WaitAddons);
        None
    }

    /// The programs and add-on jobs are gone: wait for a background save,
    /// or save now.
    fn logout_after_jobs(&mut self, now: Instant) -> Option<Effect> {
        // The logout save covers a deferred Save-On-Exit.
        self.deferred_save = false;
        // A save that is refused anyway is not attempted: the person is
        // asked straight away.
        if self.restore_failed {
            let message = SaveOutcome::RestoreFailed.message();
            self.log.note(Kind::Warn, message.clone());
            self.logout = Some(Logout::Failed { message });
            return None;
        }
        if self.save.running {
            // The logout save follows; a queued one would only repeat it.
            self.save.again = false;
            self.logout = Some(Logout::WaitSave);
            return None;
        }
        self.save.running = true;
        self.log.note(Kind::Event, SAVING);
        self.logout = Some(Logout::Saving { since: now });
        Some(Effect::Start(Job::Save))
    }

    /// An add-on job or the stick write ended: if it was the last thing the
    /// logout waited for, go on.
    fn root_job_ended(&mut self, now: Instant) -> Option<Effect> {
        let waiting = matches!(self.logout, Some(Logout::WaitAddons | Logout::StoppingAddons { .. }))
            || (self.logout == Some(Logout::Confirm) && self.jobs.is_empty());
        if waiting && self.root_idle() {
            return self.logout_after_jobs(now);
        }
        None
    }

    fn pgids(&self) -> Vec<i32> {
        self.jobs.iter().filter_map(|j| j.pgid).collect()
    }

    pub fn finished(&mut self, result: JobResult, now: Instant) -> Option<Effect> {
        match result {
            JobResult::Started { id, pgid } => {
                let job = self.jobs.iter_mut().find(|j| j.id == id)?;
                job.pgid = Some(pgid);
                // Stop was asked before this program's group was known.
                match self.logout {
                    Some(Logout::Stopping { killed, .. }) => {
                        let sig = if killed { Signal::Kill } else { Signal::Term };
                        Some(Effect::Signal(vec![pgid], sig))
                    }
                    _ => None,
                }
            }
            JobResult::Ran { id, status } => {
                let pos = self.jobs.iter().position(|j| j.id == id)?;
                let job = self.jobs.remove(pos);
                self.monitors.set_thinlinc(self.thinlinc_running());
                let (kind, text) = run_message(&job.name, &status);
                self.log.note(kind, text);
                if let Some(Logout::Stopping { .. }) = self.logout {
                    return if self.jobs.is_empty() {
                        self.after_programs(now)
                    } else {
                        None
                    };
                }
                // Log out was clicked and nothing is left to stop or wait
                // for: go on without asking. The logout save covers
                // Save-On-Exit.
                if self.logout == Some(Logout::Confirm) && self.jobs.is_empty() && self.root_idle() {
                    return self.logout_after_jobs(now);
                }
                if !self.tiles[job.tile].save_on_exit {
                    return None;
                }
                if self.restore_failed {
                    self.log.note(Kind::Warn, SaveOutcome::RestoreFailed.message());
                    return None;
                }
                // The question now waits only for an add-on job, and goes
                // on by itself when it ends: the logout save follows then.
                // Only Stay still needs this save.
                if self.logout == Some(Logout::Confirm) && self.jobs.is_empty() {
                    self.deferred_save = true;
                    return None;
                }
                self.request_save()
            }
            JobResult::UpgradeStarted { pgid } => {
                if !self.upgrade.writing() {
                    return None;
                }
                self.upgrade.started(pgid);
                // Restart anyway or Cancel came before the group was known.
                let stop = matches!(self.logout, Some(Logout::StoppingAddons { .. })) || self.upgrade.cancelling;
                stop.then(|| Effect::Signal(vec![pgid], Signal::Term))
            }
            JobResult::UpgradeMsg(m) => {
                self.upgrade.message(m, now);
                None
            }
            JobResult::UpgradeEnded { status } => {
                if !self.upgrade.writing() {
                    return None;
                }
                if let Err(e) = &status {
                    self.log.note(Kind::Warn, format!("{e}."));
                }
                self.upgrade.ended(status);
                self.root_job_ended(now)
            }
            JobResult::Drives { all, result } => {
                if let Some(line) = self.upgrade.drives_listed(all, result, now) {
                    self.log.note(Kind::Warn, line);
                }
                None
            }
            JobResult::Release { custom, result } => {
                self.upgrade.released(custom, result);
                None
            }
            JobResult::NetLoaded(wifi, wired, devices) => {
                self.network.loaded(wifi, wired, devices);
                None
            }
            JobResult::NetStatus(r) => {
                self.network.status_done(r);
                None
            }
            JobResult::NetScan(r) => {
                self.network.scan_done(r);
                None
            }
            JobResult::NetApplyStarted { pgid } => {
                if !self.network.applying() {
                    return None;
                }
                self.network.apply_started(pgid);
                // Restart anyway came before the group was known.
                matches!(self.logout, Some(Logout::StoppingAddons { .. }))
                    .then(|| Effect::Signal(vec![pgid], Signal::Term))
            }
            JobResult::NetApplied(r) => {
                if !self.network.applying() {
                    return None;
                }
                match &r {
                    Ok(()) => self.log.note(Kind::Event, "Network settings applied."),
                    Err(e) => self
                        .log
                        .note(Kind::Warn, format!("Network settings not applied: {}", e.reason)),
                }
                let save = self.network.apply_done(r);
                // The logout save, if the logout waited for this, covers it.
                if let Some(next) = self.root_job_ended(now) {
                    return Some(next);
                }
                if save { self.net_changed_home() } else { None }
            }
            JobResult::MonLoaded(text, owner) => {
                self.monitors.loaded(text, owner);
                None
            }
            JobResult::MonApplied(r) => {
                if let Err(e) = &r {
                    self.log
                        .note(Kind::Warn, format!("The monitor arrangement was not applied: {e}"));
                }
                self.monitors.applied(r).map(Menu::mon_request)
            }
            JobResult::MonKept(r) => {
                let ok = r.is_ok();
                match &r {
                    Ok(()) => self.log.note(Kind::Event, "Monitor arrangement kept."),
                    Err(e) => self.log.note(
                        Kind::Warn,
                        format!("The monitor arrangement could not be confirmed: {e}"),
                    ),
                }
                if !self.monitors.kept(r) {
                    return None;
                }
                if ok { self.net_changed_home() } else { None }
            }
            JobResult::MonReverted(r) => {
                match &r {
                    Ok(()) => self.log.note(Kind::Event, "The previous monitor arrangement is back."),
                    Err(e) => self.log.note(
                        Kind::Warn,
                        format!("The previous monitor arrangement could not be put back: {e}"),
                    ),
                }
                self.monitors.reverted(r).map(Menu::mon_request)
            }
            JobResult::MonTakenOver(r) => {
                let ok = r.is_ok();
                match &r {
                    Ok(()) => self
                        .log
                        .note(Kind::Event, "~/.config/kanshi/config is now managed by the menu."),
                    Err(e) => self.log.note(Kind::Warn, format!("{e}.")),
                }
                if !self.monitors.taken_over(r) {
                    return None;
                }
                if ok { self.net_changed_home() } else { None }
            }
            JobResult::NetTakenOver(r) => {
                if !self.network.taking_over() {
                    return None;
                }
                let which = self.network.taking_over_file();
                match &r {
                    Ok(()) => self
                        .log
                        .note(Kind::Event, format!("~/{which} is now managed by the menu.")),
                    Err(e) => self.log.note(Kind::Warn, format!("{e}.")),
                }
                if self.network.take_over_done(r) {
                    self.net_changed_home()
                } else {
                    None
                }
            }
            JobResult::AddonStarted { args, pgid } => {
                let run = self
                    .addon_runs
                    .iter_mut()
                    .find(|a| a.args == args && a.pgid.is_none())?;
                run.pgid = Some(pgid);
                // Restart anyway was chosen before this job's group was known.
                matches!(self.logout, Some(Logout::StoppingAddons { .. }))
                    .then(|| Effect::Signal(vec![pgid], Signal::Term))
            }
            JobResult::Addon { args, status } => {
                let pos = self.addon_runs.iter().position(|a| a.args == args)?;
                self.addon_runs.remove(pos);
                // Exit 1 is a failure the tool logged itself.
                let line = match status {
                    Ok(Some(0 | 1)) => None,
                    Ok(Some(n)) => Some(format!("thinfloe-addon {} exited with status {n}.", args.join(" "))),
                    Ok(None) => Some(format!("thinfloe-addon {} was killed by a signal.", args.join(" "))),
                    Err(e) => Some(format!("{e}.")),
                };
                if let Some(line) = line {
                    self.log.note(Kind::Warn, line);
                }
                self.root_job_ended(now)
            }
            JobResult::Saved(outcome) => {
                self.save.running = false;
                let (kind, text) = save_message(&outcome);
                self.log.note(kind, text);
                match self.logout {
                    Some(Logout::Saving { .. }) => {
                        self.logout = Some(if outcome.saved() {
                            Logout::Leaving {
                                until: now + LOGOUT_PAUSE,
                            }
                        } else {
                            Logout::Failed {
                                message: outcome.message(),
                            }
                        });
                        None
                    }
                    Some(Logout::WaitSave) => self.logout_after_jobs(now),
                    // The logout save follows; a queued one would only repeat it.
                    Some(Logout::Stopping { .. } | Logout::WaitAddons | Logout::StoppingAddons { .. }) => {
                        self.save.again = false;
                        None
                    }
                    _ if self.save.again => {
                        self.save.again = false;
                        self.request_save()
                    }
                    _ => None,
                }
            }
        }
    }

    /// An answer in the logout dialog. Ignored when it does not fit the step.
    pub fn choose(&mut self, choice: Choice, now: Instant) -> Option<Effect> {
        match (&self.logout, choice) {
            (Some(Logout::Confirm | Logout::WaitAddons | Logout::Failed { .. }), Choice::Stay) => {
                self.logout = None;
                self.offer_retry();
                if std::mem::take(&mut self.deferred_save) {
                    return self.request_save();
                }
                None
            }
            (Some(Logout::WaitAddons), Choice::LeaveAnyway) if self.ending != Action::Logout => {
                let names = self.root_jobs().join(", ");
                self.log.note(Kind::Warn, format!("Stopping {names}."));
                self.logout = Some(Logout::StoppingAddons {
                    go_at: now + STOP_GRACE,
                });
                // SIGTERM only, here and later: a write job is never killed.
                let pgids = self.root_pgids();
                (!pgids.is_empty()).then_some(Effect::Signal(pgids, Signal::Term))
            }
            (Some(Logout::Confirm), Choice::StopAndLeave) => {
                if self.jobs.is_empty() {
                    return self.after_programs(now);
                }
                let names: Vec<&str> = self.jobs.iter().map(|j| j.name.as_str()).collect();
                self.log.note(Kind::Event, format!("Stopping {}.", names.join(", ")));
                self.logout = Some(Logout::Stopping {
                    kill_at: now + STOP_GRACE,
                    killed: false,
                });
                Some(Effect::Signal(self.pgids(), Signal::Term))
            }
            // The failure is in the log already, from when it happened.
            (Some(Logout::Failed { .. }), Choice::LeaveAnyway) => self.ending.exit_code().map(Effect::Exit),
            _ => None,
        }
    }

    /// Called on every frame: kills what did not stop in time, and ends the
    /// menu once "saved" has been shown for `LOGOUT_PAUSE`. On the Upgrade
    /// screen it also lists the drives when that is due.
    pub fn tick(&mut self, now: Instant) -> Option<Effect> {
        if let Some(r) = self.monitors.tick(now) {
            return Some(Menu::mon_request(r));
        }
        let net_open =
            self.logout.is_none() && self.screen == Screen::Settings && self.settings_tab == SettingsTab::Network;
        if let Some(r) = self.network.poll(now, net_open) {
            return Some(self.net_request(r));
        }
        if self.logout.is_none() && self.screen == Screen::Upgrade {
            let r = self.upgrade.poll(now)?;
            return self.upgrade_request(r, now);
        }
        match self.logout {
            Some(Logout::Leaving { until }) if now >= until => self.ending.exit_code().map(Effect::Exit),
            // The shutdown ends what is left of the add-on jobs.
            Some(Logout::StoppingAddons { go_at }) if now >= go_at => {
                let jobs = self.root_jobs();
                let what = if jobs.is_empty() {
                    "The work".to_string()
                } else {
                    jobs.join(", ")
                };
                self.log
                    .note(Kind::Warn, format!("{what} did not stop in time; going on anyway."));
                self.logout_after_jobs(now)
            }
            Some(Logout::Stopping { kill_at, killed: false }) if now >= kill_at => {
                self.logout = Some(Logout::Stopping { kill_at, killed: true });
                self.log
                    .note(Kind::Warn, "Programs did not stop in time; killing them.");
                Some(Effect::Signal(self.pgids(), Signal::Kill))
            }
            _ => None,
        }
    }
}

const SAVING: &str = "Saving the home directory…";

/// *Back* dropped a network draft (plan call 5).
pub const NET_DROPPED: &str = "Network changes that were not applied were dropped.";

/// The network Apply as the end-of-session dialog names it.
pub const NET_APPLYING: &str = "Applying network settings";

fn run_message(name: &str, status: &Result<Option<i32>, String>) -> (Kind, String) {
    match status {
        Ok(Some(0)) => (Kind::Event, format!("{name} exited.")),
        Ok(Some(n)) => (Kind::Warn, format!("{name} exited with status {n}.")),
        Ok(None) => (Kind::Warn, format!("{name} was killed by a signal.")),
        Err(e) => (Kind::Warn, format!("{e}.")),
    }
}

fn save_message(outcome: &SaveOutcome) -> (Kind, String) {
    let kind = if outcome.saved() { Kind::Good } else { Kind::Warn };
    (kind, outcome.message())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop;
    use crate::jobs::{JobId, Signal};
    use crate::log::Kind;
    use crate::screentab::{IdleSource, ScreenTab};
    use crate::settings::{RetryMarker, RowButton, SettingsEvent, SettingsTab};
    use crate::tiles::{SourceFile, merge};
    use std::os::unix::fs::PermissionsExt as _;

    /// The log as (kind, text), oldest first.
    fn log(m: &Menu) -> Vec<(Kind, String)> {
        m.log.lines().map(|l| (l.kind, l.plain())).collect()
    }

    fn ev(t: &str) -> (Kind, String) {
        (Kind::Event, t.to_string())
    }

    fn good(t: &str) -> (Kind, String) {
        (Kind::Good, t.to_string())
    }

    fn warn(t: &str) -> (Kind, String) {
        (Kind::Warn, t.to_string())
    }

    fn grid() -> Grid {
        let f = |n: &str, t: &str| SourceFile {
            name: n.into(),
            entry: desktop::parse(t),
        };
        Grid {
            tiles: merge(
                &[
                    f(
                        "10-thinlinc.desktop",
                        "[Desktop Entry]\nName=ThinLinc\nExec=tlclient\nX-TF-Save-On-Exit=true\n",
                    ),
                    f(
                        "30-terminal.desktop",
                        "[Desktop Entry]\nName=Terminal\nExec=foot\nX-TF-Multiple=true\n",
                    ),
                    f(
                        "40-save-home.desktop",
                        "[Desktop Entry]\nName=Back up home\nX-TF-Action=save-home\n",
                    ),
                    f(
                        "80-logout.desktop",
                        "[Desktop Entry]\nName=Log out\nX-TF-Action=logout\n",
                    ),
                    f(
                        "80-reboot.desktop",
                        "[Desktop Entry]\nName=Restart\nX-TF-Action=reboot\n",
                    ),
                    f(
                        "81-poweroff.desktop",
                        "[Desktop Entry]\nName=Power off\nX-TF-Action=poweroff\n",
                    ),
                ],
                &[],
            ),
            banner: Vec::new(),
        }
    }

    fn started(m: &mut Menu, index: usize, now: Instant) -> JobId {
        let Some(Effect::Start(Job::Run { id, .. })) = m.activate(index, now) else {
            panic!("tile {index} did not start")
        };
        m.finished(
            JobResult::Started {
                id,
                pgid: 1000 + id as i32,
            },
            now,
        );
        id
    }

    fn ran(m: &mut Menu, id: JobId, code: i32, now: Instant) -> Option<Effect> {
        m.finished(
            JobResult::Ran {
                id,
                status: Ok(Some(code)),
            },
            now,
        )
    }

    #[test]
    fn programs_run_side_by_side() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        let a = started(&mut m, 0, now);
        let b = started(&mut m, 1, now);
        assert_eq!(m.jobs.len(), 2);
        assert!(m.can_activate(2), "save while programs run");
        assert!(!m.dialog());
        ran(&mut m, b, 0, now);
        assert_eq!(m.jobs.iter().map(|j| j.id).collect::<Vec<_>>(), [a]);
    }

    #[test]
    fn a_tile_without_multiple_runs_once() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        started(&mut m, 0, now);
        assert!(!m.can_activate(0));
        assert_eq!(m.activate(0, now), None);
        assert!(m.active(0));
        assert_eq!(m.running(0), 1);
    }

    #[test]
    fn a_multiple_tile_runs_again_and_counts() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        started(&mut m, 1, now);
        started(&mut m, 1, now);
        assert!(m.can_activate(1));
        assert_eq!(m.running(1), 2);
        assert!(!m.active(2));
    }

    #[test]
    fn save_on_exit_saves_in_the_background() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        let a = started(&mut m, 0, now);
        assert_eq!(ran(&mut m, a, 1, now), Some(Effect::Start(Job::Save)));
        assert!(!m.dialog(), "no dialog for a background save");
        assert!(m.save.running);
        assert!(m.active(2) && m.running(2) == 0);
        assert!(!m.can_activate(2));
        assert_eq!(m.finished(JobResult::Saved(SaveOutcome::ServerUnreachable), now), None);
        assert!(!m.save.running);
        assert_eq!(
            log(&m),
            [
                ev("ThinLinc started."),
                warn("ThinLinc exited with status 1."),
                ev("Saving the home directory…"),
                warn("Not saved: the boot server cannot be reached."),
            ]
        );
    }

    #[test]
    fn requests_during_a_save_give_exactly_one_more() {
        let mut g = grid();
        // A second Save-On-Exit tile that may run twice.
        g.tiles[0].multiple = true;
        let mut m = Menu::new(g, false);
        let now = Instant::now();
        let a = started(&mut m, 0, now);
        let b = started(&mut m, 0, now);
        let c = started(&mut m, 0, now);
        assert_eq!(ran(&mut m, a, 0, now), Some(Effect::Start(Job::Save)));
        assert_eq!(ran(&mut m, b, 0, now), None, "queued");
        assert_eq!(ran(&mut m, c, 0, now), None, "still one queued");
        assert_eq!(
            m.finished(JobResult::Saved(SaveOutcome::Saved), now),
            Some(Effect::Start(Job::Save)),
            "one more"
        );
        assert_eq!(
            m.finished(JobResult::Saved(SaveOutcome::Saved), now),
            None,
            "and no third"
        );
        assert!(!m.save.running);
    }

    #[test]
    fn failed_restore_greys_save_and_skips_save_on_exit() {
        let mut m = Menu::new(grid(), true);
        assert_eq!(m.tiles[2].problem.as_deref(), Some(RESTORE_FAILED_REASON));
        let now = Instant::now();
        assert!(!m.can_activate(2));
        let a = started(&mut m, 0, now);
        assert_eq!(ran(&mut m, a, 0, now), None);
        assert_eq!(
            log(&m),
            [
                ev("ThinLinc started."),
                ev("ThinLinc exited."),
                warn(&SaveOutcome::RestoreFailed.message())
            ]
        );
    }

    #[test]
    fn logout_with_nothing_running_saves_then_exits_zero_after_the_pause() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        assert_eq!(m.activate(3, now), Some(Effect::Start(Job::Save)));
        assert_eq!(m.logout, Some(Logout::Saving { since: now }));
        assert!(m.refuses_input());
        assert_eq!(m.finished(JobResult::Saved(SaveOutcome::Saved), now), None);
        assert_eq!(
            log(&m),
            [ev("Saving the home directory…"), good("Home directory saved.")]
        );
        assert_eq!(m.tick(now), None, "the result is shown first");
        assert_eq!(m.tick(now + LOGOUT_PAUSE), Some(Effect::Exit(0)));
    }

    #[test]
    fn logout_with_programs_asks_and_stay_returns() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        started(&mut m, 1, now);
        assert_eq!(m.activate(3, now), None);
        assert_eq!(m.logout, Some(Logout::Confirm));
        assert!(m.dialog() && !m.refuses_input(), "the dialog has buttons");
        assert!(!m.can_activate(1), "the grid is frozen");
        assert_eq!(m.choose(Choice::Stay, now), None);
        assert_eq!(m.logout, None);
        assert_eq!(m.jobs.len(), 1, "nothing stopped");
    }

    #[test]
    fn stop_them_terms_each_group_then_saves() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        let a = started(&mut m, 0, now);
        let b = started(&mut m, 1, now);
        m.activate(3, now);
        assert_eq!(
            m.choose(Choice::StopAndLeave, now),
            Some(Effect::Signal(vec![1000 + a as i32, 1000 + b as i32], Signal::Term))
        );
        assert_eq!(
            m.logout,
            Some(Logout::Stopping {
                kill_at: now + STOP_GRACE,
                killed: false
            })
        );
        assert_eq!(
            ran(&mut m, a, 0, now),
            None,
            "Save-On-Exit is not acted on while stopping"
        );
        assert!(!m.save.running);
        assert_eq!(
            m.finished(
                JobResult::Ran {
                    id: b,
                    status: Ok(None)
                },
                now
            ),
            Some(Effect::Start(Job::Save)),
            "the last one gone: the logout save"
        );
        assert_eq!(m.logout, Some(Logout::Saving { since: now }));
    }

    #[test]
    fn a_program_ignoring_term_is_killed_at_the_deadline() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        let a = started(&mut m, 1, now);
        m.activate(3, now);
        m.choose(Choice::StopAndLeave, now);
        assert_eq!(m.tick(now + STOP_GRACE / 2), None);
        assert_eq!(
            m.tick(now + STOP_GRACE),
            Some(Effect::Signal(vec![1000 + a as i32], Signal::Kill))
        );
        assert_eq!(m.tick(now + STOP_GRACE * 2), None, "killed once");
        assert_eq!(
            m.finished(
                JobResult::Ran {
                    id: a,
                    status: Ok(None)
                },
                now
            ),
            Some(Effect::Start(Job::Save))
        );
    }

    #[test]
    fn a_program_started_late_is_stopped_when_its_group_is_known() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        let Some(Effect::Start(Job::Run { id, .. })) = m.activate(1, now) else {
            panic!()
        };
        m.activate(3, now);
        assert_eq!(
            m.choose(Choice::StopAndLeave, now),
            Some(Effect::Signal(vec![], Signal::Term))
        );
        assert_eq!(
            m.finished(JobResult::Started { id, pgid: 77 }, now),
            Some(Effect::Signal(vec![77], Signal::Term))
        );
        m.tick(now + STOP_GRACE);
        let Some(Effect::Start(Job::Run { id: late, .. })) = ({
            m.logout = None;
            m.activate(1, now)
        }) else {
            panic!()
        };
        m.activate(3, now);
        m.choose(Choice::StopAndLeave, now);
        m.tick(now + STOP_GRACE);
        assert_eq!(
            m.finished(JobResult::Started { id: late, pgid: 78 }, now),
            Some(Effect::Signal(vec![78], Signal::Kill)),
            "after the deadline, straight to SIGKILL"
        );
    }

    #[test]
    fn logout_waits_for_a_background_save_then_saves() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        assert_eq!(m.activate(2, now), Some(Effect::Start(Job::Save)));
        assert_eq!(m.activate(3, now), None);
        assert_eq!(m.logout, Some(Logout::WaitSave));
        assert_eq!(
            m.finished(JobResult::Saved(SaveOutcome::Saved), now),
            Some(Effect::Start(Job::Save)),
            "the logout's own save"
        );
        assert_eq!(m.logout, Some(Logout::Saving { since: now }));
        m.finished(JobResult::Saved(SaveOutcome::Saved), now);
        assert!(matches!(m.logout, Some(Logout::Leaving { .. })));
    }

    #[test]
    fn failed_logout_save_asks_and_stay_returns_to_the_grid() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        m.activate(3, now);
        m.finished(JobResult::Saved(SaveOutcome::ServerUnreachable), now);
        assert_eq!(
            m.logout,
            Some(Logout::Failed {
                message: SaveOutcome::ServerUnreachable.message()
            })
        );
        assert_eq!(m.tick(now + LOGOUT_PAUSE * 10), None, "never exits by itself");
        assert_eq!(m.activate(1, now), None, "the grid is refused while asking");
        assert_eq!(m.choose(Choice::Stay, now), None);
        assert_eq!(m.logout, None);
    }

    #[test]
    fn failed_logout_save_can_log_out_anyway() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        m.activate(3, now);
        m.finished(JobResult::Saved(SaveOutcome::Failed), now);
        assert_eq!(m.choose(Choice::LeaveAnyway, now), Some(Effect::Exit(0)));
    }

    #[test]
    fn logout_after_a_failed_restore_asks_without_saving() {
        let mut m = Menu::new(grid(), true);
        let now = Instant::now();
        assert_eq!(m.activate(3, now), None, "no save job started");
        assert!(matches!(m.logout, Some(Logout::Failed { .. })));
        assert_eq!(log(&m), [warn(&SaveOutcome::RestoreFailed.message())]);
        assert_eq!(m.choose(Choice::LeaveAnyway, now), Some(Effect::Exit(0)));
    }

    #[test]
    fn restart_saves_then_exits_ten() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        assert_eq!(m.activate(4, now), Some(Effect::Start(Job::Save)));
        m.finished(JobResult::Saved(SaveOutcome::Saved), now);
        assert_eq!(m.tick(now + LOGOUT_PAUSE), Some(Effect::Exit(10)));
    }

    #[test]
    fn power_off_with_programs_stops_them_saves_and_exits_eleven() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        let id = started(&mut m, 0, now);
        assert_eq!(m.activate(5, now), None);
        assert_eq!(m.logout, Some(Logout::Confirm));
        assert_eq!(
            m.choose(Choice::StopAndLeave, now),
            Some(Effect::Signal(vec![1000 + id as i32], Signal::Term))
        );
        assert_eq!(ran(&mut m, id, 0, now), Some(Effect::Start(Job::Save)));
        m.finished(JobResult::Saved(SaveOutcome::Saved), now);
        assert_eq!(m.tick(now + LOGOUT_PAUSE), Some(Effect::Exit(11)));
    }

    #[test]
    fn a_failed_save_can_restart_or_power_off_anyway() {
        for (tile, code) in [(4, 10), (5, 11)] {
            let mut m = Menu::new(grid(), false);
            let now = Instant::now();
            m.activate(tile, now);
            m.finished(JobResult::Saved(SaveOutcome::Failed), now);
            assert_eq!(m.choose(Choice::LeaveAnyway, now), Some(Effect::Exit(code)));
        }
    }

    #[test]
    fn stay_forgets_which_way_the_menu_was_leaving() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        let id = started(&mut m, 0, now);
        m.activate(5, now);
        m.choose(Choice::Stay, now);
        ran(&mut m, id, 0, now);
        m.finished(JobResult::Saved(SaveOutcome::Saved), now);
        assert_eq!(m.activate(3, now), Some(Effect::Start(Job::Save)));
        m.finished(JobResult::Saved(SaveOutcome::Saved), now);
        assert_eq!(m.tick(now + LOGOUT_PAUSE), Some(Effect::Exit(0)));
    }

    #[test]
    fn choice_outside_the_question_is_ignored() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        assert_eq!(m.choose(Choice::LeaveAnyway, now), None);
        assert_eq!(m.choose(Choice::StopAndLeave, now), None);
        assert_eq!(m.logout, None);
    }

    #[test]
    fn unstartable_program_is_reported_and_removed() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        let Some(Effect::Start(Job::Run { id, .. })) = m.activate(1, now) else {
            panic!()
        };
        m.finished(
            JobResult::Ran {
                id,
                status: Err("foot could not be started: x".into()),
            },
            now,
        );
        assert_eq!(log(&m)[1], warn("foot could not be started: x."));
        assert!(m.jobs.is_empty());
    }

    #[test]
    fn a_signal_is_a_warning() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        let id = started(&mut m, 1, now);
        m.finished(JobResult::Ran { id, status: Ok(None) }, now);
        assert_eq!(log(&m)[1], warn("Terminal was killed by a signal."));
    }

    #[test]
    fn an_unknown_job_id_is_ignored() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        assert_eq!(
            m.finished(
                JobResult::Ran {
                    id: 99,
                    status: Ok(Some(0))
                },
                now
            ),
            None
        );
        assert_eq!(m.finished(JobResult::Started { id: 99, pgid: 5 }, now), None);
        assert!(log(&m).is_empty());
    }

    #[test]
    fn the_banner_opens_the_log_as_warnings() {
        let mut g = grid();
        g.banner = vec!["Your own tiles could not be read.".into()];
        let m = Menu::new(g, false);
        assert_eq!(log(&m), [warn("Your own tiles could not be read.")]);
        assert_eq!(m.banner.len(), 1, "--check still prints it");
    }

    #[test]
    fn a_queued_save_is_dropped_while_programs_are_stopped() {
        // The logout save follows; a queued one would only repeat it, and on
        // netboot each save can take a minute.
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        let a = started(&mut m, 1, now);
        assert_eq!(m.activate(2, now), Some(Effect::Start(Job::Save)));
        assert_eq!(m.activate(2, now), None, "not clickable while saving");
        m.save.again = true;
        m.activate(3, now);
        m.choose(Choice::StopAndLeave, now);
        assert_eq!(
            m.finished(JobResult::Saved(SaveOutcome::Saved), now),
            None,
            "no queued save"
        );
        assert!(!m.save.running && !m.save.again);
        assert_eq!(
            m.finished(
                JobResult::Ran {
                    id: a,
                    status: Ok(None)
                },
                now
            ),
            Some(Effect::Start(Job::Save)),
            "the logout save, once"
        );
    }

    #[test]
    fn the_last_program_ending_during_the_question_continues_the_logout() {
        // The person asked to log out; with nothing left to stop there is
        // nothing to ask, and the logout save covers Save-On-Exit.
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        let a = started(&mut m, 0, now);
        m.activate(3, now);
        assert_eq!(m.logout, Some(Logout::Confirm));
        assert_eq!(ran(&mut m, a, 0, now), Some(Effect::Start(Job::Save)));
        assert_eq!(m.logout, Some(Logout::Saving { since: now }));
    }

    #[test]
    fn a_program_ending_during_the_question_with_others_left_still_saves() {
        // The person may still choose Stay, so Save-On-Exit acts as usual;
        // the logout then waits for that save.
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        let a = started(&mut m, 0, now);
        let b = started(&mut m, 1, now);
        m.activate(3, now);
        assert_eq!(ran(&mut m, a, 0, now), Some(Effect::Start(Job::Save)));
        assert_eq!(m.logout, Some(Logout::Confirm));
        assert_eq!(ran(&mut m, b, 0, now), None);
        assert_eq!(m.logout, Some(Logout::WaitSave));
    }

    fn addon_menu(bin: &std::path::Path) -> Menu {
        let f = |n: &str, t: &str| SourceFile {
            name: n.into(),
            entry: desktop::parse(t),
        };
        let exec = bin.join("tlclient");
        let mut m = Menu::new(
            Grid {
                tiles: merge(
                    &[
                        f(
                            "10-thinlinc.desktop",
                            &format!(
                                "[Desktop Entry]\nName=ThinLinc\nExec={}\nX-TF-Source-URL=https://e.example/t.deb\n",
                                exec.display()
                            ),
                        ),
                        f("30-terminal.desktop", "[Desktop Entry]\nName=Terminal\nExec=foot\n"),
                    ],
                    &[],
                ),
                banner: Vec::new(),
            },
            false,
        );
        m.refresh_programs(bin.as_os_str());
        m
    }

    fn st(state: AddonState, installed: Option<&str>) -> AddonStatus {
        AddonStatus {
            id: "10-thinlinc".into(),
            ticked: true,
            state,
            installed: installed.map(String::from),
            pending: None,
            auto: true,
            message: None,
        }
    }

    #[test]
    fn uninstalled_addon_tile_hidden() {
        let bin = crate::testdir::TestDir::new("menu", "hidden");
        let mut m = addon_menu(&bin);
        // No status at all: hidden, and it cannot be activated.
        assert_eq!(m.visible_tiles(), [1]);
        assert!(!m.can_activate(0));
        for s in [
            st(AddonState::Absent, None),
            st(AddonState::InstallFailed, None),
            st(AddonState::Failed, None),
            st(AddonState::Installed, None),
        ] {
            m.set_addons(vec![s]);
            assert_eq!(m.visible_tiles(), [1]);
        }
        // A status line for another add-on does not draw it.
        let mut other = st(AddonState::Installed, Some("1"));
        other.id = "20-other".into();
        m.set_addons(vec![other]);
        assert_eq!(m.visible_tiles(), [1]);
        m.set_addons(vec![st(AddonState::Installed, Some("4.21.0"))]);
        assert_eq!(m.visible_tiles(), [0, 1]);
        assert!(!m.pending_update(0));
    }

    #[test]
    fn pending_addon_tile_has_marker() {
        let bin = crate::testdir::TestDir::new("menu", "pending");
        let mut m = addon_menu(&bin);
        m.set_addons(vec![st(AddonState::Pending, Some("4.21.0"))]);
        assert!(m.visible(0));
        assert!(m.pending_update(0));
        assert!(!m.pending_update(1));
    }

    #[test]
    fn failed_check_keeps_installed_tile() {
        let bin = crate::testdir::TestDir::new("menu", "failed");
        let mut m = addon_menu(&bin);
        m.set_addons(vec![st(AddonState::Failed, Some("4.21.0"))]);
        assert!(m.visible(0));
        assert!(!m.pending_update(0));
    }

    #[test]
    fn a_program_installed_after_the_menu_started_enables_the_tile() {
        let bin = crate::testdir::TestDir::new("menu", "late");
        let mut m = addon_menu(&bin);
        assert!(m.tiles[0].problem.is_some());
        let changed = m.set_addons(vec![st(AddonState::Installed, Some("4.21.0"))]);
        assert_eq!(changed, [0]);
        // The status says installed, the program is not there yet.
        assert!(!m.can_activate(0));
        std::fs::write(bin.join("tlclient"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(bin.join("tlclient"), std::fs::Permissions::from_mode(0o755)).unwrap();
        m.refresh_programs(bin.as_os_str());
        assert!(m.tiles[0].usable());
        assert!(m.can_activate(0));
        // The same version again changes nothing.
        assert!(m.set_addons(vec![st(AddonState::Installed, Some("4.21.0"))]).is_empty());
        // A new version does.
        assert_eq!(m.set_addons(vec![st(AddonState::Pending, Some("4.22.0"))]), [0]);
    }

    fn addon(args: &[&str]) -> Option<Effect> {
        Some(Effect::Start(Job::Addon {
            args: args.iter().map(|s| s.to_string()).collect(),
        }))
    }

    fn addon_done(m: &mut Menu, args: &[&str], code: i32) -> Option<Effect> {
        m.finished(
            JobResult::Addon {
                args: args.iter().map(|s| s.to_string()).collect(),
                status: Ok(Some(code)),
            },
            Instant::now(),
        )
    }

    /// The add-on menu with the settings tile, on the Settings screen.
    fn settings_menu(tag: &str) -> (crate::testdir::TestDir, Menu) {
        let bin = crate::testdir::TestDir::new("menu", tag);
        let mut m = addon_menu(&bin);
        m.tiles.push(
            merge(
                &[SourceFile {
                    name: "60-settings.desktop".into(),
                    entry: desktop::parse("[Desktop Entry]\nName=Settings\nX-TF-Action=settings\n"),
                }],
                &[],
            )
            .remove(0),
        );
        assert_eq!(m.screen, Screen::Grid);
        assert_eq!(m.activate(2, Instant::now()), None);
        assert_eq!(m.screen, Screen::Settings);
        (bin, m)
    }

    #[test]
    fn tick_starts_install() {
        let (_d, mut m) = settings_menu("tick");
        let mut s = st(AddonState::Absent, None);
        s.ticked = false;
        m.set_addons(vec![s]);
        assert_eq!(
            m.settings(SettingsEvent::Tick {
                id: "10-thinlinc".into(),
                on: true
            }),
            addon(&["install", "10-thinlinc"])
        );
        // Install now does the same for an add-on that is not installed.
        assert_eq!(addon_done(&mut m, &["install", "10-thinlinc"], 0), None);
        assert_eq!(
            m.settings(SettingsEvent::InstallNow("10-thinlinc".into())),
            addon(&["install", "10-thinlinc"])
        );
        // An id that is no add-on tile starts nothing.
        assert_eq!(
            m.settings(SettingsEvent::Tick {
                id: "30-terminal".into(),
                on: true
            }),
            None
        );
    }

    #[test]
    fn untick_starts_remove() {
        let (_d, mut m) = settings_menu("untick");
        m.set_addons(vec![st(AddonState::Installed, Some("4.21.0"))]);
        assert_eq!(
            m.settings(SettingsEvent::Tick {
                id: "10-thinlinc".into(),
                on: false
            }),
            addon(&["remove", "10-thinlinc"])
        );
    }

    #[test]
    fn update_now_starts_update() {
        let (_d, mut m) = settings_menu("update");
        m.set_addons(vec![st(AddonState::Pending, Some("4.21.0"))]);
        let rows = crate::settings::rows(&m);
        assert_eq!(rows.len(), 1, "one row per add-on tile");
        assert_eq!(rows[0].button, Some(RowButton::UpdateNow));
        assert_eq!(
            m.settings(SettingsEvent::UpdateNow("10-thinlinc".into())),
            addon(&["update", "10-thinlinc"])
        );
    }

    #[test]
    fn check_now_checks_ticked() {
        // `check` with no ids: the tool checks every ticked add-on whose
        // source may be checked automatically, and only reports the others.
        let (_d, mut m) = settings_menu("check");
        m.set_addons(vec![st(AddonState::Installed, Some("4.21.0"))]);
        assert_eq!(m.settings(SettingsEvent::CheckNow), addon(&["check"]));
        assert!(m.checking());
        assert_eq!(m.settings(SettingsEvent::CheckNow), None, "one check at a time");
        assert_eq!(addon_done(&mut m, &["check"], 1), None);
        assert!(!m.checking());
        assert_eq!(m.settings(SettingsEvent::Back), None);
        assert_eq!(m.screen, Screen::Grid);
    }

    #[test]
    fn buttons_disabled_while_job_runs() {
        let (_d, mut m) = settings_menu("busy");
        m.set_addons(vec![st(AddonState::Pending, Some("4.21.0"))]);
        assert!(!crate::settings::rows(&m)[0].busy);
        assert!(m.settings(SettingsEvent::UpdateNow("10-thinlinc".into())).is_some());
        assert!(crate::settings::rows(&m)[0].busy);
        for ev in [
            SettingsEvent::UpdateNow("10-thinlinc".into()),
            SettingsEvent::InstallNow("10-thinlinc".into()),
            SettingsEvent::Tick {
                id: "10-thinlinc".into(),
                on: false,
            },
        ] {
            assert_eq!(m.settings(ev), None);
        }
        assert_eq!(addon_done(&mut m, &["update", "10-thinlinc"], 0), None);
        assert!(!crate::settings::rows(&m)[0].busy);
        // A check of every add-on holds every row.
        m.settings(SettingsEvent::CheckNow);
        assert!(crate::settings::rows(&m)[0].busy);
        assert_eq!(m.settings(SettingsEvent::UpdateNow("10-thinlinc".into())), None);
    }

    #[test]
    fn an_addon_job_that_cannot_start_is_logged() {
        let (_d, mut m) = settings_menu("nostart");
        m.settings(SettingsEvent::CheckNow);
        m.finished(
            JobResult::Addon {
                args: vec!["check".into()],
                status: Err("sudo could not be started: x".into()),
            },
            Instant::now(),
        );
        assert_eq!(log(&m).last().unwrap(), &warn("sudo could not be started: x."));
        assert!(!m.checking());
    }

    fn failed_install(message: &str) -> AddonStatus {
        let mut s = st(AddonState::InstallFailed, None);
        s.message = Some(message.into());
        s
    }

    #[test]
    fn retry_dialog_once_per_boot() {
        let d = crate::testdir::TestDir::new("menu", "retry");
        let marker = d.join("retry-shown");
        let mut m = addon_menu(&d);
        m.set_retry_marker(RetryMarker::load(&marker));
        m.set_addons(vec![failed_install("HTTP 404")]);
        assert_eq!(
            m.retry,
            Some(Retry {
                id: "10-thinlinc".into(),
                name: "ThinLinc".into(),
                message: Some("HTTP 404".into()),
            })
        );
        assert!(m.dialog() && !m.refuses_input());
        assert!(!m.can_activate(1), "the grid is frozen");
        assert_eq!(m.answer_retry(true), addon(&["install", "10-thinlinc"]));
        assert_eq!(m.retry, None);
        addon_done(&mut m, &["install", "10-thinlinc"], 1);
        m.set_addons(vec![failed_install("HTTP 500")]);
        assert_eq!(m.retry, None, "not twice in one boot");
        // A menu started again in the same boot remembers it.
        let mut again = addon_menu(&d);
        again.set_retry_marker(RetryMarker::load(&marker));
        again.set_addons(vec![failed_install("HTTP 500")]);
        assert_eq!(again.retry, None);
        // Not for an add-on that is not ticked, has a copy, or did not fail.
        for s in [
            {
                let mut s = failed_install("x");
                s.ticked = false;
                s
            },
            st(AddonState::Failed, Some("4.21.0")),
            st(AddonState::Absent, None),
        ] {
            let mut fresh = addon_menu(&d);
            fresh.set_addons(vec![s]);
            assert_eq!(fresh.retry, None);
        }
        // A failed check with no copy asks too.
        let mut fresh = addon_menu(&d);
        fresh.set_addons(vec![st(AddonState::Failed, None)]);
        assert!(fresh.retry.is_some());
    }

    #[test]
    fn a_held_retry_is_offered_when_let_go() {
        let d = crate::testdir::TestDir::new("menu", "held");
        let mut m = addon_menu(&d);
        m.hold_retry(true);
        m.set_addons(vec![failed_install("x")]);
        assert_eq!(m.retry, None);
        assert!(!m.dialog());
        m.hold_retry(false);
        assert!(m.retry.is_some());
    }

    #[test]
    fn later_closes_retry() {
        let d = crate::testdir::TestDir::new("menu", "later");
        let mut m = addon_menu(&d);
        m.set_addons(vec![failed_install("dpkg failed")]);
        assert!(m.retry.is_some());
        assert_eq!(m.answer_retry(false), None);
        assert_eq!(m.retry, None);
        assert!(!m.dialog());
        assert!(m.can_activate(1));
        m.set_addons(vec![failed_install("dpkg failed")]);
        assert_eq!(m.retry, None);
        // An answer without the dialog does nothing.
        assert_eq!(m.answer_retry(true), None);
    }

    #[test]
    fn a_failure_seen_on_the_settings_screen_asks_no_more() {
        // The row shows the same state.
        let (_d, mut m) = settings_menu("seen");
        m.set_addons(vec![failed_install("x")]);
        assert_eq!(m.retry, None);
        m.settings(SettingsEvent::Back);
        assert_eq!(m.retry, None);
    }

    #[test]
    fn settings_rows_follow_the_tools_id_rule() {
        // The same table as test_addon.py's case-twin and id tests: a row the
        // tool would refuse has no buttons, and a click on it starts nothing.
        let f = |n: &str| SourceFile {
            name: format!("{n}.desktop"),
            entry: desktop::parse(&format!(
                "[Desktop Entry]\nName={n}\nExec=true\nX-TF-Source-URL=https://e.example/{n}.deb\n"
            )),
        };
        let mut m = Menu::new(
            Grid {
                tiles: merge(
                    &[f("10-thinlinc")],
                    &[
                        f("10-ThinLinc"),
                        f("my+app"),
                        f("-lead"),
                        f("trail."),
                        f("A-B"),
                        f("a-b"),
                        f(&"x".repeat(64)),
                        f(&"y".repeat(65)),
                    ],
                ),
                banner: Vec::new(),
            },
            false,
        );
        m.screen = Screen::Settings;
        let rows = crate::settings::rows(&m);
        let usable: Vec<(&str, bool)> = rows.iter().map(|r| (r.id.as_str(), !r.unusable)).collect();
        let x64 = "x".repeat(64);
        let y65 = "y".repeat(65);
        let mut want = vec![
            ("-lead", false),
            ("10-ThinLinc", false),
            ("10-thinlinc", true),
            ("A-B", true),
            ("a-b", false),
            ("my+app", true),
            ("trail.", false),
            (x64.as_str(), true),
            (y65.as_str(), false),
        ];
        want.sort();
        let mut got = usable.clone();
        got.sort();
        assert_eq!(got, want);
        for r in rows.iter().filter(|r| r.unusable) {
            assert_eq!(r.button, None, "{}", r.id);
            assert_eq!(r.state, crate::settings::UNUSABLE_TEXT);
            assert_eq!(m.settings(SettingsEvent::InstallNow(r.id.clone())), None, "{}", r.id);
            assert_eq!(
                m.settings(SettingsEvent::Tick {
                    id: r.id.clone(),
                    on: true
                }),
                None
            );
        }
        // `+` reaches the tool, and its tile is drawn once installed.
        assert_eq!(
            m.settings(SettingsEvent::InstallNow("my+app".into())),
            addon(&["install", "my+app"])
        );
        let mut s = st(AddonState::Installed, Some("1"));
        s.id = "my+app".into();
        m.set_addons(vec![s]);
        let i = m.tiles.iter().position(|t| t.file == "my+app.desktop").unwrap();
        assert!(m.visible(i));
    }

    fn screen(idle: u32, source: IdleSource) -> ScreenTab {
        ScreenTab {
            idle,
            source,
            error: None,
        }
    }

    #[test]
    fn settings_opens_on_packages() {
        let (_d, mut m) = settings_menu("tabs");
        assert_eq!(m.settings_tab, SettingsTab::Packages);
        assert_eq!(m.settings(SettingsEvent::Tab(SettingsTab::Screen)), None);
        assert_eq!(m.settings_tab, SettingsTab::Screen);
        assert_eq!(m.settings(SettingsEvent::Back), None);
        assert_eq!(m.activate(2, Instant::now()), None);
        assert_eq!(m.screen, Screen::Settings);
        assert_eq!(m.settings_tab, SettingsTab::Packages);
    }

    #[test]
    fn idle_change_asks_for_a_write() {
        let (_d, mut m) = settings_menu("idle");
        m.screen_tab = screen(300, IdleSource::Menu);
        assert_eq!(
            m.settings(SettingsEvent::IdleTimeout(600)),
            Some(Effect::WriteScreen { idle: 600 })
        );
        assert_eq!(m.settings(SettingsEvent::IdleTimeout(300)), None, "unchanged");
        m.screen_tab = screen(300, IdleSource::Unset);
        assert_eq!(
            m.settings(SettingsEvent::IdleTimeout(0)),
            Some(Effect::WriteScreen { idle: 0 })
        );
        assert_eq!(m.settings(SettingsEvent::ManageScreen), None, "nothing to take over");
    }

    #[test]
    fn idle_change_refused_when_rc_or_by_hand() {
        let (_d, mut m) = settings_menu("idle-refused");
        m.screen_tab = screen(300, IdleSource::Rc);
        assert_eq!(m.settings(SettingsEvent::IdleTimeout(600)), None);
        assert_eq!(m.settings(SettingsEvent::ManageScreen), None);
        m.screen_tab = screen(300, IdleSource::ByHand);
        assert_eq!(m.settings(SettingsEvent::IdleTimeout(600)), None);
        assert_eq!(m.settings(SettingsEvent::ManageScreen), Some(Effect::TakeOverScreen));
        m.logout = Some(Logout::Confirm);
        assert_eq!(m.settings(SettingsEvent::ManageScreen), None, "not during a logout");
        m.screen_tab = screen(300, IdleSource::Menu);
        assert_eq!(m.settings(SettingsEvent::IdleTimeout(600)), None, "not during a logout");
    }

    #[test]
    fn screen_written_queues_one_save() {
        let mut m = Menu::new(grid(), false);
        assert_eq!(
            m.screen_written(Ok(screen(600, IdleSource::Menu)), true),
            Some(Effect::Start(Job::Save))
        );
        assert!(m.save.running && !m.save.again);
        assert_eq!(m.screen_written(Ok(screen(0, IdleSource::Menu)), true), None);
        assert!(m.save.again);
        assert_eq!(m.screen_tab, screen(0, IdleSource::Menu));
        assert_eq!(
            log(&m),
            [
                ev("Screen turns off after 10 minutes."),
                ev("Saving the home directory…"),
                ev("Screen never turns off."),
            ]
        );
    }

    #[test]
    fn screen_change_does_not_save_after_failed_restore() {
        let mut m = Menu::new(grid(), true);
        assert_eq!(m.screen_written(Ok(screen(300, IdleSource::Menu)), true), None);
        assert!(!m.save.running);
        assert_eq!(
            log(&m),
            [
                ev("Screen turns off after 5 minutes."),
                warn(&SaveOutcome::RestoreFailed.message()),
            ]
        );
    }

    #[test]
    fn screen_written_without_signal_says_next_session() {
        let mut m = Menu::new(grid(), false);
        assert_eq!(
            m.screen_written(Ok(screen(60, IdleSource::Menu)), false),
            Some(Effect::Start(Job::Save))
        );
        assert_eq!(
            log(&m),
            [
                ev("Screen turns off after 1 minute."),
                ev("The new screen timeout applies from the next session."),
                ev("Saving the home directory…"),
            ]
        );
    }

    #[test]
    fn screen_write_error_shows_on_the_tab() {
        let mut m = Menu::new(grid(), false);
        m.screen_tab = screen(300, IdleSource::Menu);
        assert_eq!(m.screen_written(Err("disk full".into()), true), None);
        assert_eq!(m.screen_tab.error.as_deref(), Some("disk full"));
        assert_eq!(m.screen_tab.idle, 300);
        assert!(!m.save.running);
        assert_eq!(log(&m), [warn("disk full")]);
        // A later write that works clears it.
        m.screen_written(Ok(screen(600, IdleSource::Menu)), true);
        assert_eq!(m.screen_tab.error, None);
    }

    /// The Settings menu with a Restart tile (index 3), back on the grid,
    /// with an install of ThinLinc running.
    fn installing_menu(tag: &str) -> (crate::testdir::TestDir, Menu) {
        let (d, mut m) = settings_menu(tag);
        m.tiles.push(
            merge(
                &[SourceFile {
                    name: "80-reboot.desktop".into(),
                    entry: desktop::parse("[Desktop Entry]\nName=Restart\nX-TF-Action=reboot\n"),
                }],
                &[],
            )
            .remove(0),
        );
        assert_eq!(
            m.settings(SettingsEvent::InstallNow("10-thinlinc".into())),
            addon(&["install", "10-thinlinc"])
        );
        m.settings(SettingsEvent::Back);
        assert_eq!(m.screen, Screen::Grid);
        (d, m)
    }

    #[test]
    fn restart_waits_for_an_addon_job() {
        let (_d, mut m) = installing_menu("wait-addon");
        let now = Instant::now();
        assert_eq!(m.activate(3, now), None);
        assert_eq!(m.logout, Some(Logout::Confirm));
        assert_eq!(m.addon_jobs(), ["Installing ThinLinc"]);
        assert!(!m.refuses_input());
        // Nothing to signal: the job is waited for.
        assert_eq!(m.choose(Choice::StopAndLeave, now), None);
        assert_eq!(m.logout, Some(Logout::WaitAddons));
        assert!(!m.refuses_input(), "Stay stays available");
        assert_eq!(m.tick(now + STOP_GRACE * 2), None);
        assert_eq!(
            addon_done(&mut m, &["install", "10-thinlinc"], 0),
            Some(Effect::Start(Job::Save))
        );
        assert!(matches!(m.logout, Some(Logout::Saving { .. })));
    }

    #[test]
    fn stay_while_waiting_for_an_addon_job() {
        let (_d, mut m) = installing_menu("stay-addon");
        let now = Instant::now();
        m.activate(3, now);
        m.choose(Choice::StopAndLeave, now);
        assert_eq!(m.choose(Choice::Stay, now), None);
        assert_eq!(m.logout, None);
        // The job ending afterwards does not resume the restart.
        assert_eq!(addon_done(&mut m, &["install", "10-thinlinc"], 0), None);
        assert_eq!(m.logout, None);
    }

    #[test]
    fn an_addon_job_ending_while_asked_goes_on() {
        let (_d, mut m) = installing_menu("addon-ends");
        let now = Instant::now();
        m.activate(3, now);
        assert_eq!(
            addon_done(&mut m, &["install", "10-thinlinc"], 1),
            Some(Effect::Start(Job::Save))
        );
        assert!(matches!(m.logout, Some(Logout::Saving { .. })));
    }

    #[test]
    fn programs_are_stopped_and_addon_jobs_waited_for() {
        let (_d, mut m) = installing_menu("both");
        let now = Instant::now();
        let id = started(&mut m, 1, now);
        m.activate(3, now);
        assert_eq!(m.logout, Some(Logout::Confirm));
        // Only the program's group is signalled.
        assert_eq!(
            m.choose(Choice::StopAndLeave, now),
            Some(Effect::Signal(vec![1000 + id as i32], Signal::Term))
        );
        assert_eq!(ran(&mut m, id, 143, now), None);
        assert_eq!(m.logout, Some(Logout::WaitAddons));
        assert_eq!(
            addon_done(&mut m, &["install", "10-thinlinc"], 0),
            Some(Effect::Start(Job::Save))
        );
        // A program ending while asked does not go on while the job runs.
        let (_d, mut m) = installing_menu("both-asked");
        let id = started(&mut m, 1, now);
        m.activate(3, now);
        assert_eq!(ran(&mut m, id, 0, now), None);
        assert_eq!(m.logout, Some(Logout::Confirm));
        assert_eq!(m.addon_jobs(), ["Installing ThinLinc"]);
    }

    fn push_tile(m: &mut Menu, name: &str, text: &str) -> usize {
        m.tiles.push(
            merge(
                &[SourceFile {
                    name: name.into(),
                    entry: desktop::parse(text),
                }],
                &[],
            )
            .remove(0),
        );
        m.tiles.len() - 1
    }

    fn addon_started(m: &mut Menu, args: &[&str], pgid: i32) -> Option<Effect> {
        m.finished(
            JobResult::AddonStarted {
                args: args.iter().map(|s| s.to_string()).collect(),
                pgid,
            },
            Instant::now(),
        )
    }

    const INSTALL: [&str; 2] = ["install", "10-thinlinc"];

    #[test]
    fn restart_anyway_stops_the_addon_job() {
        let (_d, mut m) = installing_menu("anyway");
        let now = Instant::now();
        assert_eq!(addon_started(&mut m, &INSTALL, 4242), None);
        m.activate(3, now);
        m.choose(Choice::StopAndLeave, now);
        assert_eq!(m.logout, Some(Logout::WaitAddons));
        assert_eq!(
            m.choose(Choice::LeaveAnyway, now),
            Some(Effect::Signal(vec![4242], Signal::Term))
        );
        assert_eq!(
            m.logout,
            Some(Logout::StoppingAddons {
                go_at: now + STOP_GRACE
            })
        );
        assert!(m.refuses_input());
        assert_eq!(m.tick(now), None);
        assert_eq!(addon_done(&mut m, &INSTALL, 143), Some(Effect::Start(Job::Save)));
        assert!(matches!(m.logout, Some(Logout::Saving { .. })));
    }

    #[test]
    fn restart_anyway_goes_on_after_the_grace() {
        let (_d, mut m) = installing_menu("anyway-grace");
        let now = Instant::now();
        m.activate(3, now);
        m.choose(Choice::StopAndLeave, now);
        // The group is not known yet: it is signalled once it is.
        assert_eq!(m.choose(Choice::LeaveAnyway, now), None);
        assert_eq!(
            addon_started(&mut m, &INSTALL, 4243),
            Some(Effect::Signal(vec![4243], Signal::Term))
        );
        assert_eq!(m.tick(now + STOP_GRACE), Some(Effect::Start(Job::Save)));
        assert!(matches!(m.logout, Some(Logout::Saving { .. })));
        // The job ending later changes nothing; the save ends the menu.
        assert_eq!(addon_done(&mut m, &INSTALL, 143), None);
        assert_eq!(m.finished(JobResult::Saved(SaveOutcome::Saved), now), None);
        assert_eq!(m.tick(now + STOP_GRACE + LOGOUT_PAUSE), Some(Effect::Exit(10)));
    }

    #[test]
    fn power_off_anyway_too_but_log_out_only_waits() {
        let (_d, mut m) = installing_menu("anyway-poweroff");
        let now = Instant::now();
        let off = push_tile(
            &mut m,
            "81-poweroff.desktop",
            "[Desktop Entry]\nName=Power off\nX-TF-Action=poweroff\n",
        );
        addon_started(&mut m, &INSTALL, 4244);
        m.activate(off, now);
        m.choose(Choice::StopAndLeave, now);
        assert!(m.choose(Choice::LeaveAnyway, now).is_some());
        assert!(matches!(m.logout, Some(Logout::StoppingAddons { .. })));
        let (_d, mut m) = installing_menu("anyway-logout");
        let out = push_tile(
            &mut m,
            "80-logout.desktop",
            "[Desktop Entry]\nName=Log out\nX-TF-Action=logout\n",
        );
        addon_started(&mut m, &INSTALL, 4245);
        m.activate(out, now);
        m.choose(Choice::StopAndLeave, now);
        assert_eq!(m.choose(Choice::LeaveAnyway, now), None);
        assert_eq!(m.logout, Some(Logout::WaitAddons));
        assert_eq!(m.tick(now + STOP_GRACE * 10), None);
    }

    #[test]
    fn save_on_exit_while_only_an_addon_job_is_waited_for() {
        // N3: the logout save follows once the job ends, so no extra
        // background save; if the person stays, the save is done then.
        let (_d, mut m) = installing_menu("deferred");
        let now = Instant::now();
        let sx = push_tile(
            &mut m,
            "15-sx.desktop",
            "[Desktop Entry]\nName=SX\nExec=true\nX-TF-Save-On-Exit=true\n",
        );
        let id = started(&mut m, sx, now);
        m.activate(3, now);
        assert_eq!(m.logout, Some(Logout::Confirm));
        assert_eq!(ran(&mut m, id, 0, now), None);
        assert!(!m.save.running);
        assert_eq!(m.logout, Some(Logout::Confirm));
        assert_eq!(addon_done(&mut m, &INSTALL, 0), Some(Effect::Start(Job::Save)));
        assert!(matches!(m.logout, Some(Logout::Saving { .. })));

        let (_d, mut m) = installing_menu("deferred-stay");
        let sx = push_tile(
            &mut m,
            "15-sx.desktop",
            "[Desktop Entry]\nName=SX\nExec=true\nX-TF-Save-On-Exit=true\n",
        );
        let id = started(&mut m, sx, now);
        m.activate(3, now);
        assert_eq!(ran(&mut m, id, 0, now), None);
        assert_eq!(m.choose(Choice::Stay, now), Some(Effect::Start(Job::Save)));
        assert_eq!(m.logout, None);
        // Stay again later owes nothing.
        m.finished(JobResult::Saved(SaveOutcome::Saved), now);
        m.activate(3, now);
        assert_eq!(m.choose(Choice::Stay, now), None);
    }

    // The Network tab and its jobs.

    use crate::jobs::ApplyFailed;
    use crate::netfiles::{FileState, Net, WifiFile, Wired, WiredFile};
    use crate::network::{NetEvent, POLL as NET_POLL};

    const DROPPED: &str = "Network changes that were not applied were dropped.";

    fn net_loaded() -> JobResult {
        JobResult::NetLoaded(
            WifiFile {
                state: FileState::Menu,
                device: Some("wlp1".into()),
                networks: vec![Net {
                    ssid: "Home".into(),
                    hidden: false,
                    password: Some("secret123".into()),
                }],
                manageable: true,
                error: None,
            },
            WiredFile {
                state: FileState::Menu,
                wired: Wired::Dhcp,
                manageable: true,
                error: None,
            },
            vec!["wlp1".into()],
        )
    }

    /// `grid()` on a stick-booted machine, on the Network tab, files read.
    fn net_menu(restore_failed: bool) -> Menu {
        let mut m = Menu::new(grid(), restore_failed);
        m.set_usb_boot(true);
        m.screen = Screen::Settings;
        m.settings_tab = SettingsTab::Network;
        assert_eq!(m.finished(net_loaded(), Instant::now()), None);
        m
    }

    fn net(m: &mut Menu, e: NetEvent) -> Option<Effect> {
        m.settings(SettingsEvent::Network(e))
    }

    /// Forget the saved network and Apply.
    fn net_apply(m: &mut Menu) {
        assert_eq!(net(m, NetEvent::Forget("Home".into())), None);
        assert_eq!(
            net(m, NetEvent::Apply),
            Some(Effect::Start(Job::NetApply {
                change: netfiles::Change {
                    wifi: Some(Vec::new()),
                    wired: None
                }
            }))
        );
        assert!(m.network.applying());
    }

    #[test]
    fn settings_has_three_tabs() {
        assert_eq!(
            SettingsTab::ALL,
            [SettingsTab::Packages, SettingsTab::Network, SettingsTab::Screen]
        );
        assert_eq!(SettingsTab::Network.label(), "Network");
    }

    #[test]
    fn back_drops_the_draft_with_a_line() {
        let mut m = net_menu(false);
        net(&mut m, NetEvent::WiredMode(true));
        net(&mut m, NetEvent::Forget("Home".into()));
        // Another tab keeps the draft.
        m.settings(SettingsEvent::Tab(SettingsTab::Screen));
        assert!(m.network.dirty());
        m.settings(SettingsEvent::Back);
        assert!(!m.network.dirty());
        assert_eq!(log(&m), [warn(DROPPED)]);
        // Nothing to drop: no line.
        m.screen = Screen::Settings;
        m.settings(SettingsEvent::Back);
        assert_eq!(log(&m), [warn(DROPPED)]);
    }

    #[test]
    fn network_events_only_on_the_settings_screen() {
        let mut m = net_menu(false);
        m.screen = Screen::Grid;
        assert_eq!(net(&mut m, NetEvent::Forget("Home".into())), None);
        assert!(!m.network.dirty());
        m.screen = Screen::Settings;
        m.logout = Some(Logout::Confirm);
        assert_eq!(net(&mut m, NetEvent::Forget("Home".into())), None);
        assert!(!m.network.dirty());
    }

    #[test]
    fn apply_while_thinlinc_runs_asks_first() {
        let mut m = net_menu(false);
        let now = Instant::now();
        m.screen = Screen::Grid;
        started(&mut m, 0, now);
        m.screen = Screen::Settings;
        net(&mut m, NetEvent::Forget("Home".into()));
        assert_eq!(net(&mut m, NetEvent::Apply), None);
        assert!(m.network.confirm_open());
        assert!(matches!(
            net(&mut m, NetEvent::ConfirmApply),
            Some(Effect::Start(Job::NetApply { .. }))
        ));
        // A Terminal is no remote session.
        let mut m = net_menu(false);
        m.screen = Screen::Grid;
        started(&mut m, 1, now);
        m.screen = Screen::Settings;
        net(&mut m, NetEvent::Forget("Home".into()));
        assert!(matches!(
            net(&mut m, NetEvent::Apply),
            Some(Effect::Start(Job::NetApply { .. }))
        ));
    }

    #[test]
    fn tick_polls_only_on_the_network_tab() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        m.screen = Screen::Settings;
        assert_eq!(m.tick(now), None, "Packages tab");
        m.settings_tab = SettingsTab::Network;
        assert_eq!(m.tick(now), Some(Effect::Start(Job::NetLoad)));
        assert_eq!(m.tick(now), Some(Effect::Start(Job::NetStatus)));
        assert_eq!(m.tick(now), None);
        m.finished(JobResult::NetStatus(Ok(Vec::new())), now);
        assert_eq!(m.tick(now + NET_POLL), Some(Effect::Start(Job::NetStatus)));
        m.finished(JobResult::NetStatus(Ok(Vec::new())), now);
        m.screen = Screen::Grid;
        assert_eq!(m.tick(now + NET_POLL * 2), None);
    }

    #[test]
    fn restart_waits_for_network_apply() {
        let mut m = net_menu(false);
        let now = Instant::now();
        net_apply(&mut m);
        // Back during the Apply drops nothing: the change is in flight.
        m.settings(SettingsEvent::Back);
        assert!(!log(&m).contains(&warn(DROPPED)));
        assert_eq!(m.activate(4, now), None);
        assert_eq!(m.logout, Some(Logout::Confirm));
        assert_eq!(m.root_jobs(), ["Applying network settings"]);
        assert_eq!(m.finished(JobResult::NetApplyStarted { pgid: 4242 }, now), None);
        assert_eq!(m.choose(Choice::StopAndLeave, now), None);
        assert_eq!(m.logout, Some(Logout::WaitAddons));
        assert_eq!(
            m.choose(Choice::LeaveAnyway, now),
            Some(Effect::Signal(vec![4242], Signal::Term))
        );
        // Ended: the logout save follows, and covers the change.
        assert_eq!(
            m.finished(JobResult::NetApplied(Ok(())), now),
            Some(Effect::Start(Job::Save))
        );
        assert!(matches!(m.logout, Some(Logout::Saving { .. })));
        assert!(!m.deferred_save);
    }

    #[test]
    fn stay_after_a_network_apply_saves() {
        // A program still runs while the Apply ends: the logout save may
        // never come, so Stay saves.
        let mut m = net_menu(false);
        let now = Instant::now();
        net_apply(&mut m);
        m.settings(SettingsEvent::Back);
        started(&mut m, 1, now);
        let i = m.tiles.iter().position(|t| t.action == Action::Logout).unwrap();
        assert_eq!(m.activate(i, now), None);
        assert_eq!(m.logout, Some(Logout::Confirm));
        assert_eq!(m.finished(JobResult::NetApplied(Ok(())), now), None);
        assert!(!m.save.running);
        assert_eq!(m.choose(Choice::Stay, now), Some(Effect::Start(Job::Save)));
    }

    #[test]
    fn restart_anyway_stops_an_apply_started_late() {
        let mut m = net_menu(false);
        let now = Instant::now();
        net_apply(&mut m);
        m.settings(SettingsEvent::Back);
        m.activate(4, now);
        assert_eq!(m.choose(Choice::StopAndLeave, now), None);
        assert_eq!(m.choose(Choice::LeaveAnyway, now), None, "no group known yet");
        assert_eq!(
            m.finished(JobResult::NetApplyStarted { pgid: 77 }, now),
            Some(Effect::Signal(vec![77], Signal::Term))
        );
    }

    #[test]
    fn network_apply_queues_one_save() {
        let mut m = net_menu(false);
        let now = Instant::now();
        net_apply(&mut m);
        assert_eq!(m.finished(JobResult::NetApplyStarted { pgid: 9 }, now), None);
        assert_eq!(m.root_jobs(), ["Applying network settings"]);
        assert_eq!(
            m.finished(JobResult::NetApplied(Ok(())), now),
            Some(Effect::Start(Job::Save))
        );
        assert!(m.save.running && !m.save.again);
        assert!(m.root_jobs().is_empty());
        // The files are read again.
        assert_eq!(m.tick(now), Some(Effect::Start(Job::NetLoad)));
        // A take-over while that save runs queues one more.
        m.finished(
            JobResult::NetLoaded(
                WifiFile {
                    state: FileState::Hand,
                    device: None,
                    networks: Vec::new(),
                    manageable: true,
                    error: None,
                },
                WiredFile {
                    state: FileState::Menu,
                    wired: Wired::Dhcp,
                    manageable: true,
                    error: None,
                },
                Vec::new(),
            ),
            now,
        );
        assert_eq!(
            net(&mut m, NetEvent::TakeOver("wifi")),
            Some(Effect::Start(Job::NetTakeOver { which: "wifi" }))
        );
        assert_eq!(m.finished(JobResult::NetTakenOver(Ok(())), now), None);
        assert!(m.save.again);
        assert_eq!(
            log(&m),
            [
                ev("Network settings applied."),
                ev("Saving the home directory…"),
                ev("~/wifi.yaml is now managed by the menu."),
            ]
        );
    }

    #[test]
    fn network_apply_failure_is_logged_and_not_saved() {
        let mut m = net_menu(false);
        let now = Instant::now();
        net_apply(&mut m);
        assert_eq!(
            m.finished(
                JobResult::NetApplied(Err(ApplyFailed {
                    reason: "no link".into(),
                    home_changed: false
                })),
                now
            ),
            None
        );
        assert!(!m.save.running);
        assert_eq!(m.network.error(), Some("no link"));
        assert_eq!(log(&m), [warn("Network settings not applied: no link")]);
        // An answer nobody asked for is dropped.
        assert_eq!(m.finished(JobResult::NetApplied(Ok(())), now), None);
        assert_eq!(m.finished(JobResult::NetApplyStarted { pgid: 3 }, now), None);
        assert_eq!(m.network.pgid(), None);
    }

    #[test]
    fn a_home_not_put_back_is_saved() {
        let mut m = net_menu(false);
        let now = Instant::now();
        net_apply(&mut m);
        let failed = ApplyFailed {
            reason: "x; ~/wifi.yaml could not be put back: busy".into(),
            home_changed: true,
        };
        assert_eq!(
            m.finished(JobResult::NetApplied(Err(failed)), now),
            Some(Effect::Start(Job::Save))
        );
        assert_eq!(
            log(&m),
            [
                warn("Network settings not applied: x; ~/wifi.yaml could not be put back: busy"),
                ev("Saving the home directory…"),
            ]
        );
    }

    #[test]
    fn network_apply_after_failed_restore_does_not_save() {
        let mut m = net_menu(true);
        let now = Instant::now();
        net_apply(&mut m);
        assert_eq!(m.finished(JobResult::NetApplied(Ok(())), now), None);
        assert!(!m.save.running);
        assert_eq!(
            log(&m),
            [
                ev("Network settings applied."),
                warn(&SaveOutcome::RestoreFailed.message())
            ]
        );
    }

    #[test]
    fn screen_written_still_saves() {
        let mut m = Menu::new(grid(), false);
        assert_eq!(
            m.screen_written(Ok(screen(300, IdleSource::Menu)), true),
            Some(Effect::Start(Job::Save))
        );
        let mut m = Menu::new(grid(), true);
        assert_eq!(m.screen_written(Ok(screen(300, IdleSource::Menu)), true), None);
        assert_eq!(
            log(&m),
            [
                ev("Screen turns off after 5 minutes."),
                warn(&SaveOutcome::RestoreFailed.message())
            ]
        );
    }

    #[test]
    fn netboot_offers_no_fixed_address() {
        let mut m = Menu::new(grid(), false);
        m.screen = Screen::Settings;
        m.finished(net_loaded(), Instant::now());
        net(&mut m, NetEvent::WiredMode(true));
        assert!(!m.network.dirty(), "netboot is the default");
        m.set_usb_boot(true);
        m.finished(net_loaded(), Instant::now());
        net(&mut m, NetEvent::WiredMode(true));
        assert!(m.network.dirty());
    }

    // The Upgrade screen and its write job.

    use crate::drives::{Drive, Mode};
    use crate::release::Release;
    use crate::upgrade::{Outcome, POLL, Stage};
    use crate::upgrade_proto::Msg;

    /// The index of the Upgrade tile in `upgrade_menu`.
    const UPGRADE: usize = 6;
    const REBOOT: usize = 4;
    const LOGOUT: usize = 3;

    fn shipped_upgrade_text() -> String {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../overlay/etc/thinfloe/menu");
        std::fs::read_to_string(dir.join("50-upgrade-image.desktop")).unwrap()
    }

    /// `grid()` with the shipped Upgrade tile, booted from a stick, 4.0.0.
    fn upgrade_menu() -> Menu {
        let mut g = grid();
        g.tiles.push(
            merge(
                &[SourceFile {
                    name: "50-upgrade-image.desktop".into(),
                    entry: desktop::parse(&shipped_upgrade_text()),
                }],
                &[],
            )
            .remove(0),
        );
        assert_eq!(g.tiles[UPGRADE].action, Action::Upgrade);
        let mut m = Menu::new(g, false);
        m.set_usb_boot(true);
        m.set_installed(Some("4.0.0".into()));
        m
    }

    fn usb_stick(device: &str, id: &str) -> Drive {
        Drive {
            device: device.into(),
            model: "SanDisk Cruzer".into(),
            size_bytes: 16_008_609_792,
            serial: Some("AA1".into()),
            transport: Some("usb".into()),
            removable: true,
            mounted: false,
            is_boot: false,
            is_thinfloe: false,
            has_tl_old: false,
            has_tl_new: false,
            examined: true,
            id: id.into(),
            fits: Some(true),
            version: None,
        }
    }

    fn boot_stick() -> Drive {
        Drive {
            is_boot: true,
            mounted: true,
            is_thinfloe: true,
            ..usb_stick("/dev/sda", "sn-B:1:1")
        }
    }

    fn release() -> Release {
        Release {
            latest: Some("4.1.0".into()),
            image_bytes: Some(3_100_000_000),
            ..Release::default()
        }
    }

    /// Press the tile, answer the check and one drive list.
    fn opened(m: &mut Menu, drives: Vec<Drive>, now: Instant) {
        assert_eq!(
            m.activate(UPGRADE, now),
            Some(Effect::Start(Job::UpgradeCheck { source: None }))
        );
        assert_eq!(m.screen, Screen::Upgrade);
        let rel = JobResult::Release {
            custom: false,
            result: Ok(release()),
        };
        assert_eq!(m.finished(rel, now), None);
        assert_eq!(m.tick(now), Some(Effect::Start(Job::Drives { all: false })));
        let list = JobResult::Drives {
            all: false,
            result: Ok(drives),
        };
        assert_eq!(m.finished(list, now), None);
    }

    /// Choose `drive` and `mode` and confirm; the write job is asked for.
    fn confirmed(m: &mut Menu, drive: Drive, mode: Mode, now: Instant) {
        let device = drive.device.clone();
        opened(m, vec![drive], now);
        assert_eq!(m.upgrade_event(UpgradeEvent::Choose(device), now), None);
        assert_eq!(m.upgrade_event(UpgradeEvent::Mode(mode), now), None);
        let Some(Effect::Start(Job::UpgradeWrite { args })) = m.upgrade_event(UpgradeEvent::Confirm, now) else {
            panic!("no write started")
        };
        assert!(args.contains(&"--expect-id".to_string()), "{args:?}");
        assert!(m.upgrade.writing());
    }

    /// A write of `drive` runs, in process group 4242.
    fn writing(m: &mut Menu, drive: Drive, mode: Mode, now: Instant) {
        confirmed(m, drive, mode, now);
        assert_eq!(m.finished(JobResult::UpgradeStarted { pgid: 4242 }, now), None);
    }

    fn fresh_write(m: &mut Menu, now: Instant) {
        writing(m, usb_stick("/dev/sdb", "sn-A:1:1"), Mode::Fresh, now);
        assert!(m.upgrade.fresh_running());
    }

    fn msg(m: &mut Menu, msg: Msg) -> Option<Effect> {
        m.finished(JobResult::UpgradeMsg(msg), Instant::now())
    }

    fn upgrade_ended(m: &mut Menu, status: Result<Option<i32>, String>) -> Option<Effect> {
        m.finished(JobResult::UpgradeEnded { status }, Instant::now())
    }

    fn no_kill(e: &Option<Effect>) -> bool {
        !matches!(e, Some(Effect::Signal(_, Signal::Kill)))
    }

    #[test]
    fn the_upgrade_tile_is_hidden_on_netboot() {
        let mut m = upgrade_menu();
        let now = Instant::now();
        assert!(m.visible(UPGRADE));
        assert!(m.can_activate(UPGRADE));
        m.set_usb_boot(false);
        assert!(!m.visible(UPGRADE));
        assert!(!m.visible_tiles().contains(&UPGRADE));
        assert!(!m.can_activate(UPGRADE));
        assert_eq!(m.activate(UPGRADE, now), None);
        assert_eq!(m.screen, Screen::Grid);
    }

    #[test]
    fn a_newer_release_names_and_marks_the_tile() {
        let mut m = upgrade_menu();
        // No release known yet.
        assert_eq!(m.tile_name(UPGRADE), "Upgrade");
        assert!(!m.pending_update(UPGRADE));
        m.upgrade.release_file(release());
        assert_eq!(m.upgrade.installed.as_deref(), Some("4.0.0"));
        assert_eq!(m.tile_name(UPGRADE), "Upgrade to 4.1.0");
        assert!(m.pending_update(UPGRADE));
        assert_eq!(m.tile_name(0), "ThinLinc");
        assert!(!m.pending_update(0));
        // The installed version is not known: no marker, the plain name.
        m.set_installed(None);
        assert_eq!(m.upgrade.installed, None);
        assert_eq!(m.tile_name(UPGRADE), "Upgrade");
        assert!(!m.pending_update(UPGRADE));
        // Already on it.
        m.set_installed(Some("4.1.0".into()));
        assert_eq!(m.tile_name(UPGRADE), "Upgrade");
        assert!(!m.pending_update(UPGRADE));
        // A reworded tile keeps its name and gets the marker.
        m.set_installed(Some("4.0.0".into()));
        let shipped = SourceFile {
            name: "50-upgrade-image.desktop".into(),
            entry: desktop::parse(&shipped_upgrade_text()),
        };
        let user = SourceFile {
            name: "50-upgrade-image.desktop".into(),
            entry: desktop::parse("[Desktop Entry]\nName=Neu\n"),
        };
        m.tiles[UPGRADE] = merge(&[shipped], &[user]).remove(0);
        assert_eq!(m.tile_name(UPGRADE), "Neu");
        assert!(m.pending_update(UPGRADE));
    }

    #[test]
    fn pressing_the_tile_checks_and_then_polls() {
        let mut m = upgrade_menu();
        let now = Instant::now();
        assert_eq!(
            m.activate(UPGRADE, now),
            Some(Effect::Start(Job::UpgradeCheck { source: None }))
        );
        assert_eq!(m.screen, Screen::Upgrade);
        assert!(!m.can_activate(0), "the grid is not on screen");
        assert_eq!(m.tick(now), Some(Effect::Start(Job::Drives { all: false })));
        assert_eq!(m.tick(now), None, "one list at a time");
        let list = JobResult::Drives {
            all: false,
            result: Ok(vec![usb_stick("/dev/sdb", "sn-A:1:1")]),
        };
        assert_eq!(m.finished(list, now), None);
        assert_eq!(m.upgrade.drives.len(), 1);
        assert_eq!(m.tick(now), None);
        assert_eq!(m.tick(now + POLL), Some(Effect::Start(Job::Drives { all: false })));
        // Back on the grid nothing is polled.
        assert_eq!(m.upgrade_event(UpgradeEvent::Back, now), None);
        assert_eq!(m.screen, Screen::Grid);
        let list = JobResult::Drives {
            all: false,
            result: Ok(vec![]),
        };
        m.finished(list, now);
        assert_eq!(m.tick(now + POLL * 5), None);
    }

    #[test]
    fn a_drive_list_that_cannot_start_keeps_polling() {
        let mut m = upgrade_menu();
        let now = Instant::now();
        m.activate(UPGRADE, now);
        assert_eq!(m.tick(now), Some(Effect::Start(Job::Drives { all: false })));
        let failed = JobResult::Drives {
            all: false,
            result: Err("sudo could not be started: No such file or directory".into()),
        };
        assert_eq!(m.finished(failed, now), None);
        assert!(
            log(&m).contains(&warn(
                "drive list failed: sudo could not be started: No such file or directory"
            )),
            "{:?}",
            log(&m)
        );
        assert_eq!(m.tick(now + POLL), Some(Effect::Start(Job::Drives { all: false })));
    }

    #[test]
    fn a_check_that_cannot_start_can_be_asked_again() {
        let mut m = upgrade_menu();
        let now = Instant::now();
        m.activate(UPGRADE, now);
        let failed = JobResult::Release {
            custom: false,
            result: Err("sudo could not be started: denied".into()),
        };
        assert_eq!(m.finished(failed, now), None);
        assert_eq!(
            m.upgrade.release.error.as_deref(),
            Some("sudo could not be started: denied")
        );
        assert_eq!(
            m.upgrade_event(UpgradeEvent::CheckNow, now),
            Some(Effect::Start(Job::UpgradeCheck { source: None }))
        );
    }

    #[test]
    fn a_write_that_cannot_start_ends_the_job() {
        let mut m = upgrade_menu();
        let now = Instant::now();
        confirmed(&mut m, usb_stick("/dev/sdb", "sn-A:1:1"), Mode::Fresh, now);
        assert!(m.active(UPGRADE));
        assert_eq!(
            upgrade_ended(&mut m, Err("sudo could not be started: denied".into())),
            None
        );
        assert!(!m.upgrade.writing());
        assert!(!m.active(UPGRADE));
        assert!(matches!(m.upgrade.stage, Stage::Finished(Outcome::NoResult { .. })));
        assert!(
            log(&m).contains(&warn("sudo could not be started: denied.")),
            "{:?}",
            log(&m)
        );
        // Nothing holds the session's end any more.
        m.upgrade_event(UpgradeEvent::Back, now);
        assert_eq!(m.activate(REBOOT, now), Some(Effect::Start(Job::Save)));
    }

    #[test]
    fn cancel_signals_the_write_once_its_group_is_known() {
        let mut m = upgrade_menu();
        let now = Instant::now();
        let mut d = usb_stick("/dev/sdb", "sn-A:1:1");
        d.is_thinfloe = true;
        confirmed(&mut m, d, Mode::Upgrade, now);
        // Not known yet: the signal goes out with the start.
        assert_eq!(m.upgrade_event(UpgradeEvent::Cancel, now), None);
        assert_eq!(
            m.finished(JobResult::UpgradeStarted { pgid: 4242 }, now),
            Some(Effect::Signal(vec![4242], Signal::Term))
        );
        // A fresh install asks first.
        let mut m = upgrade_menu();
        fresh_write(&mut m, now);
        assert_eq!(m.upgrade_event(UpgradeEvent::Cancel, now), None);
        assert_eq!(
            m.upgrade_event(UpgradeEvent::CancelYes, now),
            Some(Effect::Signal(vec![4242], Signal::Term))
        );
    }

    #[test]
    fn restart_waits_for_the_write_and_anyway_warns_on_fresh() {
        let mut m = upgrade_menu();
        let now = Instant::now();
        fresh_write(&mut m, now);
        assert_eq!(m.upgrade_event(UpgradeEvent::Back, now), None);
        assert_eq!(m.screen, Screen::Grid);
        let prog = started(&mut m, 1, now);
        assert_eq!(m.activate(REBOOT, now), None);
        assert_eq!(m.logout, Some(Logout::Confirm));
        assert_eq!(m.root_jobs(), ["Writing SanDisk Cruzer"]);
        // Only the program's group is stopped, and only it is killed.
        assert_eq!(
            m.choose(Choice::StopAndLeave, now),
            Some(Effect::Signal(vec![1000 + prog as i32], Signal::Term))
        );
        assert_eq!(
            m.tick(now + STOP_GRACE),
            Some(Effect::Signal(vec![1000 + prog as i32], Signal::Kill))
        );
        assert_eq!(ran(&mut m, prog, 137, now), None);
        assert_eq!(m.logout, Some(Logout::WaitAddons));
        assert!(!m.refuses_input(), "Stay and Restart anyway are offered");
        assert_eq!(m.tick(now + STOP_GRACE * 4), None, "the write is waited for");
        // The dialog can say the stick will not start: a fresh install runs.
        assert!(m.upgrade.fresh_running());
        assert_eq!(
            m.choose(Choice::LeaveAnyway, now),
            Some(Effect::Signal(vec![4242], Signal::Term))
        );
        assert_eq!(
            m.logout,
            Some(Logout::StoppingAddons {
                go_at: now + STOP_GRACE
            })
        );
        for t in [now, now + STOP_GRACE / 2] {
            let e = m.tick(t);
            assert!(no_kill(&e), "{e:?}");
        }
        assert_eq!(
            msg(
                &mut m,
                Msg::Result {
                    ok: false,
                    reason: "cancelled".into()
                }
            ),
            None
        );
        assert_eq!(upgrade_ended(&mut m, Ok(Some(130))), Some(Effect::Start(Job::Save)));
        assert!(matches!(m.logout, Some(Logout::Saving { .. })));
        assert!(
            m.upgrade
                .outcome_text()
                .0
                .contains("will not start until it is written again")
        );

        // A write that does not end within the grace is never killed: the
        // menu goes on, the shutdown ends it.
        let mut m = upgrade_menu();
        fresh_write(&mut m, now);
        m.upgrade_event(UpgradeEvent::Back, now);
        m.activate(REBOOT, now);
        assert_eq!(m.choose(Choice::StopAndLeave, now), None);
        assert_eq!(m.logout, Some(Logout::WaitAddons));
        assert_eq!(
            m.choose(Choice::LeaveAnyway, now),
            Some(Effect::Signal(vec![4242], Signal::Term))
        );
        let job = m.upgrade.job_name().unwrap();
        assert_eq!(m.tick(now + STOP_GRACE), Some(Effect::Start(Job::Save)));
        assert!(
            log(&m).contains(&(Kind::Warn, format!("{job} did not stop in time; going on anyway."))),
            "{:?}",
            log(&m)
        );
        for k in 1..20 {
            let e = m.tick(now + STOP_GRACE * k);
            assert!(no_kill(&e), "{e:?}");
        }
        assert_eq!(m.finished(JobResult::Saved(SaveOutcome::Saved), now), None);
        for k in 1..20 {
            let e = m.tick(now + STOP_GRACE * k + LOGOUT_PAUSE);
            assert!(no_kill(&e), "{e:?}");
        }
        assert_eq!(m.tick(now + STOP_GRACE + LOGOUT_PAUSE), Some(Effect::Exit(10)));
    }

    #[test]
    fn restart_anyway_before_the_write_group_is_known() {
        let mut m = upgrade_menu();
        let now = Instant::now();
        confirmed(&mut m, usb_stick("/dev/sdb", "sn-A:1:1"), Mode::Fresh, now);
        m.upgrade_event(UpgradeEvent::Back, now);
        m.activate(REBOOT, now);
        m.choose(Choice::StopAndLeave, now);
        assert_eq!(m.choose(Choice::LeaveAnyway, now), None);
        assert_eq!(
            m.finished(JobResult::UpgradeStarted { pgid: 4243 }, now),
            Some(Effect::Signal(vec![4243], Signal::Term))
        );
    }

    #[test]
    fn log_out_only_waits_for_the_write() {
        let mut m = upgrade_menu();
        let now = Instant::now();
        let mut d = usb_stick("/dev/sdb", "sn-A:1:1");
        d.is_thinfloe = true;
        writing(&mut m, d, Mode::Upgrade, now);
        m.upgrade_event(UpgradeEvent::Back, now);
        assert_eq!(m.activate(LOGOUT, now), None);
        assert_eq!(m.logout, Some(Logout::Confirm));
        assert_eq!(m.root_jobs(), ["Upgrading SanDisk Cruzer"]);
        assert_eq!(m.choose(Choice::StopAndLeave, now), None);
        assert_eq!(m.logout, Some(Logout::WaitAddons));
        assert_eq!(m.choose(Choice::LeaveAnyway, now), None);
        assert_eq!(m.logout, Some(Logout::WaitAddons));
        for k in 0..20 {
            assert_eq!(m.tick(now + STOP_GRACE * k), None);
        }
        msg(
            &mut m,
            Msg::Result {
                ok: true,
                reason: String::new(),
            },
        );
        assert_eq!(upgrade_ended(&mut m, Ok(Some(0))), Some(Effect::Start(Job::Save)));
        assert!(matches!(m.logout, Some(Logout::Saving { .. })));
        // The write ending while the question is still up goes on, too.
        let mut m = upgrade_menu();
        fresh_write(&mut m, now);
        m.upgrade_event(UpgradeEvent::Back, now);
        m.activate(LOGOUT, now);
        assert_eq!(m.logout, Some(Logout::Confirm));
        assert_eq!(upgrade_ended(&mut m, Ok(Some(1))), Some(Effect::Start(Job::Save)));
    }

    #[test]
    fn back_while_writing_keeps_the_job_and_the_tile_shows_it() {
        let mut m = upgrade_menu();
        let now = Instant::now();
        fresh_write(&mut m, now);
        assert_eq!(m.upgrade.stage, Stage::Running);
        assert_eq!(m.upgrade_event(UpgradeEvent::Back, now), None);
        assert_eq!(m.screen, Screen::Grid);
        assert!(m.upgrade.writing());
        assert_eq!(m.upgrade.pgid(), Some(4242));
        assert!(m.active(UPGRADE));
        assert!(m.can_activate(UPGRADE));
        m.activate(UPGRADE, now);
        assert_eq!(m.screen, Screen::Upgrade);
        assert_eq!(m.upgrade.stage, Stage::Running);
        assert_eq!(m.tick(now + POLL), None, "no drive list while writing");
    }

    #[test]
    fn restart_now_after_a_boot_stick_upgrade_takes_the_restart_steps() {
        let mut m = upgrade_menu();
        let now = Instant::now();
        writing(&mut m, boot_stick(), Mode::Upgrade, now);
        msg(
            &mut m,
            Msg::Result {
                ok: true,
                reason: String::new(),
            },
        );
        assert_eq!(upgrade_ended(&mut m, Ok(Some(0))), None);
        assert_eq!(
            m.upgrade.stage,
            Stage::Finished(Outcome::Done {
                boot_stick: true,
                late_cancel: false
            })
        );
        assert_eq!(
            m.upgrade_event(UpgradeEvent::RestartNow, now),
            Some(Effect::Start(Job::Save))
        );
        assert_eq!(m.ending, Action::Reboot);
        assert_eq!(m.logout, Some(Logout::Saving { since: now }));
        assert_eq!(m.finished(JobResult::Saved(SaveOutcome::Saved), now), None);
        assert_eq!(m.tick(now + LOGOUT_PAUSE), Some(Effect::Exit(10)));
    }

    #[test]
    fn no_retry_dialog_over_the_upgrade_screen() {
        let d = crate::testdir::TestDir::new("menu", "upgrade-retry");
        let mut m = addon_menu(&d);
        let up = push_tile(&mut m, "50-upgrade-image.desktop", &shipped_upgrade_text());
        m.set_usb_boot(true);
        let now = Instant::now();
        assert!(m.activate(up, now).is_some());
        assert_eq!(m.screen, Screen::Upgrade);
        m.set_addons(vec![failed_install("HTTP 404")]);
        assert_eq!(m.retry, None);
        assert_eq!(m.upgrade_event(UpgradeEvent::Back, now), None);
        assert_eq!(m.screen, Screen::Grid);
        assert_eq!(m.retry.as_ref().map(|r| r.id.as_str()), Some("10-thinlinc"));
    }

    fn mon_head(name: &str, x: i32) -> crate::heads::Head {
        crate::heads::Head {
            name: name.into(),
            make: "Dell".into(),
            model: "U".into(),
            serial: String::new(),
            phys_mm: (0, 0),
            enabled: true,
            modes: vec![crate::heads::Mode {
                w: 1920,
                h: 1080,
                refresh_mhz: 60_000,
                preferred: true,
            }],
            current: Some(0),
            pos: (x, 0),
            rotation: crate::heads::Rotation::Normal,
        }
    }

    /// `grid()` with two monitors reported and no layout file.
    fn mon_menu() -> Menu {
        let mut m = Menu::new(grid(), false);
        let heads = vec![mon_head("DP-1", 0), mon_head("DP-2", 1920)];
        assert_eq!(m.heads_changed(heads), Some(Effect::Start(Job::MonLoad)));
        assert_eq!(
            m.finished(
                JobResult::MonLoaded(Ok(None), crate::owned::Owner::Missing),
                Instant::now()
            ),
            None
        );
        m
    }

    #[test]
    fn the_monitor_apply_runs_as_jobs_and_reverts_from_the_tick() {
        let now = Instant::now();
        let mut m = mon_menu();
        m.screen = Screen::Settings;
        m.settings_tab = SettingsTab::Screen;
        m.monitors_event(MonEvent::Select("DP-2".into()), now);
        assert_eq!(m.monitors_event(MonEvent::MenuHere, now), None);
        let Some(Effect::Start(Job::MonApply { text, menu_output, .. })) = m.monitors_event(MonEvent::Apply, now)
        else {
            panic!("no Apply job")
        };
        assert!(text.contains("# thinfloe menu DP-2"), "{text}");
        assert_eq!(menu_output.as_deref(), Some("DP-2"));
        assert_eq!(m.finished(JobResult::MonApplied(Ok(())), now), None);
        // The countdown runs on whatever the screen shows.
        m.screen = Screen::Grid;
        assert_eq!(m.tick(now + Duration::from_secs(14)), None);
        assert_eq!(
            m.tick(now + Duration::from_secs(15)),
            Some(Effect::Start(Job::MonRevert))
        );
        assert_eq!(m.finished(JobResult::MonReverted(Ok(())), now), None);
        assert_eq!(
            m.log.last().unwrap().plain(),
            "The previous monitor arrangement is back."
        );
        assert_eq!(m.monitors.menu_output(), None);
    }

    #[test]
    fn a_keep_that_fails_is_logged_and_saves_nothing() {
        let now = Instant::now();
        let mut m = mon_menu();
        assert!(m.monitors_event(MonEvent::Apply, now).is_some());
        assert_eq!(m.finished(JobResult::MonApplied(Ok(())), now), None);
        assert_eq!(m.monitors_event(MonEvent::Keep, now), Some(Effect::Start(Job::MonKeep)));
        assert_eq!(m.finished(JobResult::MonKept(Err("EISDIR".into())), now), None);
        assert!(m.log.last().unwrap().plain().contains("could not be confirmed: EISDIR"));
    }

    #[test]
    fn a_kept_monitor_layout_saves_the_home() {
        let now = Instant::now();
        let mut m = mon_menu();
        m.screen = Screen::Settings;
        assert!(matches!(
            m.settings(SettingsEvent::Monitors(MonEvent::Apply)),
            Some(Effect::Start(Job::MonApply { .. }))
        ));
        assert_eq!(m.finished(JobResult::MonApplied(Ok(())), now), None);
        assert_eq!(m.monitors_event(MonEvent::Keep, now), Some(Effect::Start(Job::MonKeep)));
        assert_eq!(
            m.finished(JobResult::MonKept(Ok(())), now),
            Some(Effect::Start(Job::Save))
        );
        // A failed Apply says why in the log.
        assert!(m.monitors_event(MonEvent::Apply, now).is_some());
        assert_eq!(
            m.finished(JobResult::MonApplied(Err("kanshi stopped at once".into())), now),
            None
        );
        assert!(m.log.last().unwrap().plain().contains("kanshi stopped at once"));
    }

    #[test]
    fn the_screen_tab_notes_a_running_thinlinc() {
        let now = Instant::now();
        let mut m = mon_menu();
        assert!(!m.monitors.thinlinc_note());
        let id = started(&mut m, 0, now);
        assert!(m.monitors.thinlinc_note());
        ran(&mut m, id, 0, now);
        assert!(!m.monitors.thinlinc_note());
        // A Terminal is no ThinLinc.
        started(&mut m, 1, now);
        assert!(!m.monitors.thinlinc_note());
    }

    #[test]
    fn skipped_status_lines_open_the_log_and_reach_check() {
        let mut m = Menu::new(grid(), false);
        let layout = crate::status::layout::parse(b"clock 1x1 1x1\n", &crate::status::offered);
        let note = "status line 1 skipped: expected '<id> <w>x<h>', got 'clock 1x1 1x1'";
        assert_eq!(layout.notes, [note]);
        m.set_status(crate::status::Area { layout });
        assert_eq!(log(&m).last().unwrap(), &warn(note));
        assert_eq!(m.banner, [note]);
    }

    #[test]
    fn the_network_total_counts_from_the_running_session_program() {
        use crate::netstat::{Since, Snapshot};
        let mut m = Menu::new(grid(), false);
        // Tile 0 is ThinLinc (Save-On-Exit), tile 1 the Terminal.
        let sx = 0;
        let t0 = Instant::now();
        let wall = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
        let snap = |rx: u64| Snapshot {
            total: (rx, rx / 10),
            ..Snapshot::default()
        };
        m.set_network(snap(100), t0, wall);
        assert_eq!(m.netstat_since, Since::default());
        assert_eq!(m.status_sources().network.unwrap().total, (100, 10));
        // A program without Save-On-Exit is no session.
        let other = started(&mut m, 1, t0);
        m.set_network(snap(200), t0, wall);
        assert_eq!(m.netstat_since.label, "since menu start");
        // The session program starts; the first copy after it is the base,
        // the label is the click's time on the clock.
        let id = started(&mut m, sx, t0);
        let later = t0 + std::time::Duration::from_secs(1);
        m.set_network(snap(300), later, wall);
        let since = m.netstat_since.clone();
        assert_eq!(since.job, Some(id));
        assert_eq!(since.base, (300, 30));
        assert_eq!(
            since.label,
            format!(
                "since {}",
                crate::header::clock_text(wall - std::time::Duration::from_secs(1))
            )
        );
        // Later copies keep the base.
        m.set_network(snap(900), later + std::time::Duration::from_secs(60), wall);
        assert_eq!(m.netstat_since, since);
        assert_eq!(m.status_sources().network_since, Some(&since));
        // It ends: back to the menu start.
        ran(&mut m, id, 0, later);
        ran(&mut m, other, 0, later);
        m.set_network(snap(1000), later, wall);
        assert_eq!(m.netstat_since, Since::default());
    }

    #[test]
    fn the_network_total_of_two_sessions_counts_from_the_one_that_remains() {
        use crate::netstat::{Since, Snapshot};
        use std::time::Duration;
        let mut m = Menu::new(grid(), false);
        // Tile 0 runs once; tile 1 is made a second session program.
        m.tiles[1].save_on_exit = true;
        let t0 = Instant::now();
        let wall = std::time::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let snap = |rx: u64| Snapshot {
            total: (rx, rx / 10),
            ..Snapshot::default()
        };
        let first = started(&mut m, 0, t0);
        m.set_network(snap(100), t0, wall);
        assert_eq!((m.netstat_since.job, m.netstat_since.base), (Some(first), (100, 10)));
        // A second session starts: the oldest keeps the total.
        let t1 = t0 + Duration::from_secs(60);
        let second = started(&mut m, 1, t1);
        m.set_network(snap(500), t1, wall);
        assert_eq!((m.netstat_since.job, m.netstat_since.base), (Some(first), (100, 10)));
        // The oldest ends: label and base are both the second's.
        let t2 = t1 + Duration::from_secs(5);
        ran(&mut m, first, 0, t2);
        m.set_network(snap(900), t2, wall);
        assert_eq!(
            m.netstat_since,
            Since {
                job: Some(second),
                label: format!("since {}", crate::header::clock_text(wall - Duration::from_secs(5))),
                base: (500, 50),
            }
        );
        // The second ends too: nothing is kept.
        ran(&mut m, second, 0, t2);
        m.set_network(snap(1000), t2, wall);
        assert_eq!(m.netstat_since, Since::default());
        assert!(m.network_bases.is_empty(), "{:?}", m.network_bases);
    }

    #[test]
    fn a_reading_starts_the_battery_pages_again() {
        let mut m = Menu::new(grid(), false);
        let t0 = Instant::now();
        m.set_battery(Vec::new(), t0);
        assert_eq!(m.battery_since, Some(t0));
        let t1 = t0 + std::time::Duration::from_secs(7);
        m.set_battery(Vec::new(), t1);
        assert_eq!(m.status_sources().battery_since, Some(t1));
    }
}
