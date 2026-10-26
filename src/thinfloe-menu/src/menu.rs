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

use crate::addons::{AddonState, AddonStatus, valid_id};
use crate::jobs::{JobId, JobResult, SaveOutcome, Signal};
use crate::log::{Kind, Log};
use crate::settings::{RetryMarker, SettingsEvent};
use crate::tiles::{Action, Grid, Origin, Tile};
use std::time::{Duration, Instant};

/// How long "Home directory saved. Logging out." stays on screen.
pub const LOGOUT_PAUSE: Duration = Duration::from_millis(1500);

pub const RESTORE_FAILED_REASON: &str = "this boot's home restore failed, so saving would overwrite the stored home";

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
    /// The programs are gone; an add-on job still runs. Log out does not
    /// stop it: the job runs as root, and a dpkg cut short leaves a
    /// half-installed package for the rest of the boot. Stay is offered,
    /// and for Restart and Power off also "Restart anyway": the rootfs is
    /// in RAM and goes away with the restart.
    WaitAddons,
    /// Restart or Power off anyway: SIGTERM went to the add-on jobs; the
    /// menu goes on when they ended or at `go_at`, whichever comes first.
    /// The shutdown ends what is left.
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
}

/// What fills the space above the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Grid,
    Settings,
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
    /// A Save-On-Exit program ended while the end-of-session question
    /// waited for an add-on job: the save is done only if the person stays.
    deferred_save: bool,
    /// When the add-ons were last checked, from the tool's last-check file.
    pub last_check: Option<u64>,
    retry_marker: RetryMarker,
    /// No retry is offered while set: at startup the status is read before
    /// the grid was ever on screen, and the dialog would dim an empty one.
    retry_held: bool,
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
            deferred_save: false,
            last_check: None,
            retry_marker: RetryMarker::memory(),
            retry_held: false,
        }
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
                self.screen = Screen::Grid;
                self.offer_retry();
                return None;
            }
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
    fn offer_retry(&mut self) {
        if self.retry_held || self.retry.is_some() || self.logout.is_some() {
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
        self.logout.is_some() || self.retry.is_some()
    }

    /// Whether input is dropped before egui sees it.
    pub fn refuses_input(&self) -> bool {
        self.logout.as_ref().is_some_and(Logout::refuses_input)
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
        }
    }

    /// Whether the tile is at work: a copy of its program runs, or, for
    /// the save tile, a save.
    pub fn active(&self, index: usize) -> bool {
        self.running(index) > 0
            || (self.tiles.get(index).is_some_and(|t| t.action == Action::SaveHome) && self.save.running)
    }

    /// Whether an add-on update waits for the next boot. Independent of
    /// whether a copy runs now.
    pub fn pending_update(&self, index: usize) -> bool {
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
                });
                Some(Effect::Start(Job::Run {
                    id,
                    name: tile.name.clone(),
                    argv: crate::tiles::command_line(&tile),
                }))
            }
            Action::SaveHome => self.request_save(),
            Action::Settings => {
                self.screen = Screen::Settings;
                None
            }
            Action::Logout | Action::Reboot | Action::Poweroff => {
                self.ending = tile.action;
                if self.jobs.is_empty() && self.addon_runs.is_empty() {
                    self.logout_after_jobs(now)
                } else {
                    self.logout = Some(Logout::Confirm);
                    None
                }
            }
        }
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

    /// The programs are gone: wait for the add-on jobs, then go on.
    fn after_programs(&mut self, now: Instant) -> Option<Effect> {
        if self.addon_runs.is_empty() {
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
                if self.logout == Some(Logout::Confirm) && self.jobs.is_empty() && self.addon_runs.is_empty() {
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
                // The last thing the logout waited for has ended.
                let waiting = matches!(self.logout, Some(Logout::WaitAddons | Logout::StoppingAddons { .. }))
                    || (self.logout == Some(Logout::Confirm) && self.jobs.is_empty());
                if waiting && self.addon_runs.is_empty() {
                    return self.logout_after_jobs(now);
                }
                None
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
                let names = self.addon_jobs().join(", ");
                self.log.note(Kind::Warn, format!("Stopping {names}."));
                self.logout = Some(Logout::StoppingAddons {
                    go_at: now + STOP_GRACE,
                });
                let pgids: Vec<i32> = self.addon_runs.iter().filter_map(|a| a.pgid).collect();
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
    /// menu once "saved" has been shown for `LOGOUT_PAUSE`.
    pub fn tick(&mut self, now: Instant) -> Option<Effect> {
        match self.logout {
            Some(Logout::Leaving { until }) if now >= until => self.ending.exit_code().map(Effect::Exit),
            // The shutdown ends what is left of the add-on jobs.
            Some(Logout::StoppingAddons { go_at }) if now >= go_at => {
                self.log
                    .note(Kind::Warn, "The add-on work did not stop in time; going on anyway.");
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
    use crate::settings::{RetryMarker, RowButton, SettingsEvent};
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
}
