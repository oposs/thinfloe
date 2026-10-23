//! The menu's state machine, apart from any drawing: which tile may be
//! activated, what activating it starts, and what it writes to the log.
//! Several programs run at once, in the background, next to at most one
//! save; nothing here blocks the event loop. The dialog shows only during
//! a logout.
//!
//! Log out saves the home directory here, behind the dialog, before the
//! menu exits (decided 2026-10-02). thinfloe-session no longer saves after a
//! logout: by the time the menu exits 0 the save has been done, or it
//! failed and the person at the machine chose to log out anyway. Restart
//! and Power off take the same steps and exit 10 and 11.

use crate::addons::{AddonState, AddonStatus};
use crate::jobs::{JobId, JobResult, SaveOutcome, Signal};
use crate::log::{Kind, Log};
use crate::tiles::{Action, Grid, Tile};
use std::time::{Duration, Instant};

/// How long "Home directory saved. Logging out." stays on screen.
pub const LOGOUT_PAUSE: Duration = Duration::from_millis(1500);

pub const RESTORE_FAILED_REASON: &str = "this boot's home restore failed, so saving would overwrite the stored home";

/// How long programs get to end after SIGTERM before SIGKILL, at logout.
pub const STOP_GRACE: Duration = Duration::from_secs(5);

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
    /// Programs still run: Stay, or Stop them and log out.
    Confirm,
    /// SIGTERM sent; SIGKILL at `kill_at` to what is left.
    Stopping { kill_at: Instant, killed: bool },
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
        !matches!(self, Logout::Confirm | Logout::Failed { .. })
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
    Run { id: JobId, name: String, argv: Vec<String> },
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
        }
    }

    /// The status line of the add-on a tile belongs to: the id is the tile's
    /// file name without `.desktop`.
    fn addon_of(&self, index: usize) -> Option<&AddonStatus> {
        let tile = self.tiles.get(index).filter(|t| t.addon)?;
        let id = tile.file.trim_end_matches(".desktop");
        self.addons.iter().find(|a| a.id == id)
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
        (0..self.tiles.len())
            .filter(|&i| self.tiles[i].addon && before[i] != self.addon_of(i).and_then(|a| a.installed.clone()))
            .collect()
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

    /// Whether the dialog is up, so the grid behind it is frozen and dimmed.
    pub fn dialog(&self) -> bool {
        self.logout.is_some()
    }

    /// Whether input is dropped before egui sees it.
    pub fn refuses_input(&self) -> bool {
        self.logout.as_ref().is_some_and(Logout::refuses_input)
    }

    fn running(&self, tile: usize) -> usize {
        self.jobs.iter().filter(|j| j.tile == tile).count()
    }

    pub fn can_activate(&self, index: usize) -> bool {
        let Some(tile) = self.tiles.get(index) else {
            return false;
        };
        if self.logout.is_some() || !tile.usable() || !self.visible(index) {
            return false;
        }
        match tile.action {
            Action::Run => tile.multiple || self.running(index) == 0,
            Action::SaveHome => !self.save.running,
            Action::Logout | Action::Reboot | Action::Poweroff => true,
        }
    }

    /// What the tile shows besides its text while it is at work.
    pub fn status(&self, index: usize) -> Option<String> {
        match self.running(index) {
            0 if self.tiles.get(index)?.action == Action::SaveHome && self.save.running => Some("running".into()),
            0 if self.addon_of(index).is_some_and(|a| a.state == AddonState::Pending) => Some("update".into()),
            0 => None,
            1 => Some("running".into()),
            n => Some(format!("{n} running")),
        }
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
            Action::Logout | Action::Reboot | Action::Poweroff => {
                self.ending = tile.action;
                if self.jobs.is_empty() {
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

    /// The programs are gone: wait for a background save, or save now.
    fn logout_after_jobs(&mut self, now: Instant) -> Option<Effect> {
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
                        self.logout_after_jobs(now)
                    } else {
                        None
                    };
                }
                // Log out was clicked and nothing is left to stop: go on
                // without asking. The logout save covers Save-On-Exit.
                if self.logout == Some(Logout::Confirm) && self.jobs.is_empty() {
                    return self.logout_after_jobs(now);
                }
                if !self.tiles[job.tile].save_on_exit {
                    return None;
                }
                if self.restore_failed {
                    self.log.note(Kind::Warn, SaveOutcome::RestoreFailed.message());
                    return None;
                }
                self.request_save()
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
                    Some(Logout::Stopping { .. }) => {
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
            (Some(Logout::Confirm | Logout::Failed { .. }), Choice::Stay) => {
                self.logout = None;
                None
            }
            (Some(Logout::Confirm), Choice::StopAndLeave) => {
                if self.jobs.is_empty() {
                    return self.logout_after_jobs(now);
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
    let kind = if outcome.saved() { Kind::Event } else { Kind::Warn };
    (kind, outcome.message())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop;
    use crate::jobs::{JobId, Signal};
    use crate::log::Kind;
    use crate::tiles::{SourceFile, merge};
    use std::os::unix::fs::PermissionsExt as _;

    /// The log as (kind, text), oldest first.
    fn log(m: &Menu) -> Vec<(Kind, String)> {
        m.log.lines().map(|l| (l.kind, l.plain())).collect()
    }

    fn ev(t: &str) -> (Kind, String) {
        (Kind::Event, t.to_string())
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
        assert_eq!(m.status(0).as_deref(), Some("running"));
    }

    #[test]
    fn a_multiple_tile_runs_again_and_counts() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        started(&mut m, 1, now);
        started(&mut m, 1, now);
        assert!(m.can_activate(1));
        assert_eq!(m.status(1).as_deref(), Some("2 running"));
        assert_eq!(m.status(2), None);
    }

    #[test]
    fn save_on_exit_saves_in_the_background() {
        let mut m = Menu::new(grid(), false);
        let now = Instant::now();
        let a = started(&mut m, 0, now);
        assert_eq!(ran(&mut m, a, 1, now), Some(Effect::Start(Job::Save)));
        assert!(!m.dialog(), "no dialog for a background save");
        assert!(m.save.running);
        assert_eq!(m.status(2).as_deref(), Some("running"));
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
        assert_eq!(log(&m), [ev("Saving the home directory…"), ev("Home directory saved.")]);
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
        assert_eq!(m.status(0), None);
    }

    #[test]
    fn pending_addon_tile_has_marker() {
        let bin = crate::testdir::TestDir::new("menu", "pending");
        let mut m = addon_menu(&bin);
        m.set_addons(vec![st(AddonState::Pending, Some("4.21.0"))]);
        assert!(m.visible(0));
        assert_eq!(m.status(0).as_deref(), Some("update"));
        assert_eq!(m.status(1), None);
    }

    #[test]
    fn failed_check_keeps_installed_tile() {
        let bin = crate::testdir::TestDir::new("menu", "failed");
        let mut m = addon_menu(&bin);
        m.set_addons(vec![st(AddonState::Failed, Some("4.21.0"))]);
        assert!(m.visible(0));
        assert_eq!(m.status(0), None);
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
}
