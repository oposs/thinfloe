//! The Upgrade screen as a state machine: which drive and mode are chosen,
//! what the confirm button may start, how the tool's progress lines move
//! the steps, and the one sentence at the end. No drawing here; the caller
//! runs the `Request`s through `jobs` and feeds the results back.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::drives::{self, Drive, Mode};
use crate::release::Release;
use crate::settings::Tone;
use crate::speed::Speed;
use crate::upgrade_proto::Msg;

/// The drive poll, also the repaint interval of the screen.
pub const POLL: Duration = Duration::from_secs(2);
/// Most steps shown; an unplanned step past this is not added.
pub const STEPS_MAX: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    Choose,
    Running,
    Finished(Outcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done { boot_stick: bool, late_cancel: bool },
    Failed { reason: String },
    Refused { reason: String },
    Cancelled { mode: Mode, stick: CancelledStick },
    NoResult { status: String },
}

/// What a cancel left on the stick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelledStick {
    /// Nothing was written: an upgrade, or a fresh install cancelled
    /// before `@touched`.
    Unchanged,
    /// A fresh install was cut after its first byte: the tool wiped it.
    Wiped,
    /// The image was written (and checked) before the cancel came.
    MayStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepMark {
    Waiting,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepView {
    pub id: String,
    pub label: String,
    pub mark: StepMark,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeEvent {
    /// The device of the drive the person clicked.
    Choose(String),
    Mode(Mode),
    CopyHome(bool),
    Verify(bool),
    ShowAll(bool),
    Advanced(bool),
    Source(String),
    Typed(String),
    CheckNow,
    Confirm,
    Cancel,
    CancelYes,
    CancelNo,
    TryAgain,
    /// After an upgrade of this machine's stick: choose another stick.
    WriteAnother,
    RestartNow,
    Back,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Check {
        source: Option<String>,
    },
    Drives {
        all: bool,
    },
    Write {
        args: Vec<String>,
    },
    /// SIGTERM to the write job's process group.
    Stop,
    Restart,
    Leave,
}

/// The chosen drive: both its id and its device must still match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    pub id: String,
    pub device: String,
}

/// What a running write was started with.
#[derive(Debug, Clone)]
struct Job {
    drive: Drive,
    mode: Mode,
    verify: bool,
    version: Option<String>,
    pgid: Option<i32>,
    current: Option<String>,
    done: HashSet<String>,
    /// `@touched` came: a fresh install has changed the stick.
    touched: bool,
    result: Option<(bool, String)>,
}

#[derive(Debug)]
pub struct Upgrade {
    pub stage: Stage,
    pub drives: Vec<Drive>,
    pub drives_error: Option<String>,
    /// From the status file or a check without a source.
    pub release: Release,
    /// From a check of the address under Advanced.
    pub custom_release: Option<Release>,
    pub installed: Option<String>,
    pub chosen: Option<Chosen>,
    pub mode: Option<Mode>,
    pub copy_home: bool,
    pub verify: bool,
    pub show_all: bool,
    pub advanced: bool,
    pub source_text: String,
    pub typed: String,
    pub steps: Vec<StepView>,
    /// Bytes done and the total of the running step.
    pub progress: Option<(u64, Option<u64>)>,
    pub speed: Speed,
    pub cancel_asked: bool,
    pub cancelling: bool,
    pub checking: bool,
    restore_failed: bool,
    listing: bool,
    next_poll: Option<Instant>,
    logged_error: Option<String>,
    /// The last drive list was answered after the screen last entered
    /// Choose with the current Show all; only such a list confirms a write.
    list_fresh: bool,
    /// The source the running check was sent with.
    check_source: Option<Option<String>>,
    job: Option<Job>,
    /// The version the finished write installed.
    done_version: Option<String>,
}

impl Default for Upgrade {
    fn default() -> Upgrade {
        Upgrade::new()
    }
}

impl Upgrade {
    pub fn new() -> Upgrade {
        Upgrade {
            stage: Stage::Choose,
            drives: Vec::new(),
            drives_error: None,
            release: Release::default(),
            custom_release: None,
            installed: None,
            chosen: None,
            mode: None,
            copy_home: false,
            verify: true,
            show_all: false,
            advanced: false,
            source_text: String::new(),
            typed: String::new(),
            steps: Vec::new(),
            progress: None,
            speed: Speed::new(),
            cancel_asked: false,
            cancelling: false,
            checking: false,
            restore_failed: false,
            listing: false,
            next_poll: None,
            logged_error: None,
            list_fresh: false,
            check_source: None,
            job: None,
            done_version: None,
        }
    }

    /// The screen opens: check the release (unless a check runs) and list
    /// the drives at the next poll.
    /// A finished write is left behind: the screen opens on Choose, so
    /// another stick can be written without a restart of the menu. An
    /// upgrade of this machine's stick stays: its Restart now is still due.
    pub fn open(&mut self, now: Instant) -> Option<Request> {
        if !matches!(self.stage, Stage::Finished(Outcome::Done { boot_stick: true, .. })) {
            self.reset_finished(now);
        }
        self.next_poll = Some(now);
        self.check()
    }

    /// From a finished write back to Choose; nothing in other stages.
    fn reset_finished(&mut self, now: Instant) {
        if !matches!(self.stage, Stage::Finished(_)) || self.job.is_some() {
            return;
        }
        self.stage = Stage::Choose;
        // The list is from before the write; a stick may have been pulled
        // or swapped since.
        self.list_fresh = false;
        self.steps.clear();
        self.progress = None;
        self.speed = Speed::new();
        self.cancelling = false;
        self.cancel_asked = false;
        // An erase is confirmed again, by typing it again.
        self.typed.clear();
        self.next_poll = Some(now);
    }

    /// A `Drives` request when one is due: only while choosing, and only
    /// when no drives job runs.
    pub fn poll(&mut self, now: Instant) -> Option<Request> {
        if self.stage != Stage::Choose || self.listing {
            return None;
        }
        if self.next_poll.is_some_and(|t| now < t) {
            return None;
        }
        self.listing = true;
        self.next_poll = Some(now + POLL);
        Some(Request::Drives { all: self.show_all })
    }

    pub fn event(&mut self, e: UpgradeEvent, restore_failed: bool, now: Instant) -> Option<Request> {
        self.restore_failed = restore_failed;
        if restore_failed {
            self.copy_home = false;
        }
        match e {
            UpgradeEvent::Choose(device) => {
                if self.stage == Stage::Choose {
                    self.choose(&device);
                }
                None
            }
            UpgradeEvent::Mode(m) => {
                if self.stage == Stage::Choose && self.allowed(m) && self.mode != Some(m) {
                    self.mode = Some(m);
                    self.typed.clear();
                }
                None
            }
            UpgradeEvent::CopyHome(on) => {
                self.copy_home = on && !restore_failed;
                None
            }
            UpgradeEvent::Verify(on) => {
                self.verify = on;
                None
            }
            UpgradeEvent::ShowAll(on) => {
                if self.show_all != on {
                    // The list was made for the other setting: drop it, so a
                    // drive only Show all offers is not choosable without it.
                    self.show_all = on;
                    self.drives.clear();
                    self.list_fresh = false;
                    self.next_poll = Some(now);
                }
                None
            }
            UpgradeEvent::Advanced(on) => {
                self.advanced = on;
                None
            }
            UpgradeEvent::Source(text) => {
                if text != self.source_text {
                    self.source_text = text;
                    self.custom_release = None;
                }
                None
            }
            UpgradeEvent::Typed(text) => {
                self.typed = text;
                None
            }
            UpgradeEvent::CheckNow => self.check(),
            UpgradeEvent::Confirm => self.confirm(),
            UpgradeEvent::Cancel => {
                let mode = self.job.as_ref()?.mode;
                if self.cancelling {
                    return None;
                }
                if mode == Mode::Fresh {
                    self.cancel_asked = true;
                    return None;
                }
                self.cancelling = true;
                Some(Request::Stop)
            }
            UpgradeEvent::CancelYes => {
                if !self.cancel_asked || self.cancelling || self.job.is_none() {
                    self.cancel_asked = false;
                    return None;
                }
                self.cancel_asked = false;
                self.cancelling = true;
                Some(Request::Stop)
            }
            UpgradeEvent::CancelNo => {
                self.cancel_asked = false;
                None
            }
            UpgradeEvent::TryAgain => {
                self.reset_finished(now);
                None
            }
            UpgradeEvent::WriteAnother => {
                if matches!(self.stage, Stage::Finished(_)) && self.job.is_none() {
                    self.reset_finished(now);
                    // The stick just upgraded is not the next one to write.
                    self.clear_choice();
                }
                None
            }
            UpgradeEvent::RestartNow => match self.stage {
                Stage::Finished(Outcome::Done { boot_stick: true, .. }) => Some(Request::Restart),
                _ => None,
            },
            UpgradeEvent::Back => Some(Request::Leave),
        }
    }

    /// The answer to a `Drives` request. Returns a line for the log when an
    /// error differs from the last one logged.
    pub fn drives_listed(&mut self, all: bool, r: Result<Vec<Drive>, String>, now: Instant) -> Option<String> {
        self.listing = false;
        if all != self.show_all {
            // Asked for before Show all changed: ask again at once.
            self.next_poll = Some(now);
            return None;
        }
        self.next_poll = Some(now + POLL);
        match r {
            Err(e) => {
                // Fail closed: an error lists no drive, so nothing stays chosen.
                self.drives.clear();
                self.clear_choice();
                self.drives_error = Some(e.clone());
                if self.logged_error.as_deref() == Some(e.as_str()) {
                    return None;
                }
                self.logged_error = Some(e.clone());
                Some(format!("drive list failed: {e}"))
            }
            Ok(mut list) => {
                for d in list.iter_mut().filter(|d| !d.examined) {
                    // A write runs, so the tool did not look at the stick;
                    // what it said last time still holds.
                    if let Some(old) = self.drives.iter().find(|o| o.examined && o.id == d.id) {
                        d.is_thinfloe = old.is_thinfloe;
                        d.has_tl_old = old.has_tl_old;
                        d.has_tl_new = old.has_tl_new;
                        d.version = old.version.clone();
                        d.examined = true;
                    }
                }
                self.drives = list;
                self.drives_error = None;
                self.list_fresh = self.stage == Stage::Choose;
                self.logged_error = None;
                if self.chosen_drive().is_none() {
                    self.clear_choice();
                } else if self.mode.is_some_and(|m| !self.offered(m)) {
                    self.mode = None;
                    self.typed.clear();
                }
                None
            }
        }
    }

    /// The answer to a `Check` request; `custom` when it carried a source.
    /// An answer for an address that is no longer the entered one is
    /// dropped. An answer for the default address always counts: it is the
    /// default's release whatever the field says now.
    pub fn released(&mut self, custom: bool, r: Result<Release, String>) {
        self.checking = false;
        let Some(sent) = self.check_source.take() else { return };
        if custom != sent.is_some() {
            return;
        }
        let slot = match sent {
            None => &mut self.release,
            Some(a) => {
                if self.source() != Ok(Some(a)) {
                    return;
                }
                self.custom_release.get_or_insert_with(Release::default)
            }
        };
        match r {
            Ok(rel) => *slot = rel,
            Err(e) => slot.error = Some(e),
        }
    }

    /// The status file changed.
    pub fn release_file(&mut self, r: Release) {
        self.release = r;
    }

    pub fn started(&mut self, pgid: i32) {
        if let Some(j) = self.job.as_mut() {
            j.pgid = Some(pgid);
        }
    }

    pub fn message(&mut self, m: Msg, now: Instant) {
        let Some(job) = self.job.as_mut() else { return };
        match m {
            Msg::Step { id, text } => {
                if let Some(s) = self.steps.iter_mut().find(|s| s.id == id) {
                    s.mark = StepMark::Running;
                } else if self.steps.len() < STEPS_MAX {
                    let label = if text.is_empty() { id.clone() } else { text };
                    self.steps.push(StepView {
                        id: id.clone(),
                        label,
                        mark: StepMark::Running,
                    });
                }
                if job.current.as_deref() != Some(id.as_str()) {
                    self.progress = None;
                    self.speed = Speed::new();
                }
                job.current = Some(id);
            }
            Msg::Progress { done, total } => {
                if job.current.is_some() {
                    self.progress = Some((done, total));
                    self.speed.sample(now, done);
                }
            }
            Msg::Done { id } => {
                if let Some(s) = self.steps.iter_mut().find(|s| s.id == id) {
                    s.mark = StepMark::Done;
                }
                if job.current.as_deref() == Some(id.as_str()) {
                    job.current = None;
                }
                job.done.insert(id);
            }
            Msg::Touched => job.touched = true,
            Msg::Result { ok, reason } => {
                if job.result.is_none() {
                    job.result = Some((ok, reason));
                }
            }
        }
    }

    /// The write job ended: its exit code, `Ok(None)` for a signal.
    pub fn ended(&mut self, status: Result<Option<i32>, String>) {
        let Some(job) = self.job.take() else { return };
        let outcome = match (&job.result, &status) {
            (None, _) => Outcome::NoResult {
                status: match status {
                    Ok(Some(c)) => format!("exit status {c}"),
                    Ok(None) => "ended by a signal".to_string(),
                    Err(e) => e,
                },
            },
            (Some((true, _)), _) => Outcome::Done {
                boot_stick: job.drive.is_boot && job.mode == Mode::Upgrade,
                late_cancel: self.cancelling,
            },
            (Some((false, _)), Ok(Some(130))) => Outcome::Cancelled {
                mode: job.mode,
                stick: if job.mode != Mode::Fresh {
                    CancelledStick::Unchanged
                } else if job.done.contains("write") && (!job.verify || job.done.contains("verify")) {
                    CancelledStick::MayStart
                } else if job.touched || job.done.contains("write") {
                    CancelledStick::Wiped
                } else {
                    CancelledStick::Unchanged
                },
            },
            (Some((false, reason)), Ok(Some(2))) => Outcome::Refused { reason: reason.clone() },
            (Some((false, reason)), _) => Outcome::Failed { reason: reason.clone() },
        };
        if !matches!(outcome, Outcome::Done { .. }) {
            for s in self.steps.iter_mut().filter(|s| s.mark == StepMark::Running) {
                s.mark = StepMark::Failed;
            }
        }
        self.done_version = job.version;
        self.cancel_asked = false;
        self.stage = Stage::Finished(outcome);
    }

    /// The arguments of the write the choice describes.
    pub fn write_args(&self) -> Result<Vec<String>, String> {
        let d = self.chosen_drive().ok_or("no drive is chosen")?;
        let mode = self.mode.ok_or("no action is chosen")?;
        let source = self.source()?;
        let mut a: Vec<String> = ["write", "--device", &d.device, "--mode"].map(String::from).to_vec();
        a.push(match mode {
            Mode::Fresh => "fresh".to_string(),
            Mode::Upgrade => "upgrade".to_string(),
        });
        a.push("--expect-id".to_string());
        a.push(d.id.clone());
        if let Some(s) = source {
            a.push("--source".to_string());
            a.push(s);
        }
        if mode == Mode::Fresh && self.copy_home_effective() {
            a.push("--copy-home".to_string());
        }
        if mode == Mode::Fresh && self.verify {
            a.push("--verify".to_string());
        }
        if drives::internal(d) {
            a.push("--allow-internal".to_string());
        }
        Ok(a)
    }

    /// The sentence above the confirm button.
    pub fn summary(&self) -> Option<String> {
        let d = self.chosen_drive()?;
        let to = match self.version() {
            Some(v) => format!("ThinFloe {v}"),
            None => "the newest ThinFloe".to_string(),
        };
        let size = drives::format_size(d.size_bytes);
        Some(match self.mode? {
            Mode::Fresh => format!("Erase {}, {size}, and install {to}", name(d)),
            Mode::Upgrade if d.is_boot => {
                format!("Upgrade this machine's stick to {to}. It becomes active after a restart.")
            }
            Mode::Upgrade => format!("Upgrade {}, {size}, to {to}", name(d)),
        })
    }

    pub fn confirm_label(&self) -> Option<String> {
        let d = self.chosen_drive()?;
        Some(match self.mode? {
            Mode::Fresh => format!("Erase {}", drives::confirm_text(d)),
            Mode::Upgrade => "Upgrade".to_string(),
        })
    }

    /// The running write as the end-of-session dialogs name it.
    pub fn job_name(&self) -> Option<String> {
        let j = self.job.as_ref()?;
        let verb = match j.mode {
            Mode::Fresh => "Writing",
            Mode::Upgrade => "Upgrading",
        };
        Some(format!("{verb} {}", name(&j.drive)))
    }

    /// A fresh install runs: stopping it leaves a stick that does not start.
    pub fn fresh_running(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.mode == Mode::Fresh)
    }

    /// A fresh install has written to its stick (`@touched`): stopping it
    /// now leaves a wiped stick. Before that the stick is as it was.
    pub fn fresh_touched(&self) -> bool {
        self.job
            .as_ref()
            .is_some_and(|j| j.mode == Mode::Fresh && (j.touched || j.done.contains("write")))
    }

    pub fn pgid(&self) -> Option<i32> {
        self.job.as_ref()?.pgid
    }

    /// A write was requested and has not ended.
    pub fn writing(&self) -> bool {
        self.job.is_some()
    }

    /// The one sentence at the end, with its colour.
    pub fn outcome_text(&self) -> (String, Tone) {
        let Stage::Finished(o) = &self.stage else {
            return (String::new(), Tone::Plain);
        };
        match o {
            Outcome::Done {
                boot_stick,
                late_cancel,
            } => {
                let mut s = String::new();
                if *late_cancel {
                    s.push_str("The cancel came too late: the write was finished. ");
                }
                if *boot_stick {
                    match &self.done_version {
                        Some(v) => s.push_str(&format!("ThinFloe {v} is ready. It starts after a restart.")),
                        None => s.push_str("The new release is ready. It starts after a restart."),
                    }
                } else {
                    s.push_str("The stick is written. You can remove the stick.");
                }
                (s, Tone::Good)
            }
            Outcome::Failed { reason } => (sentence(reason), Tone::Warn),
            Outcome::Refused { reason } => {
                let reason = reason.trim().trim_end_matches('.');
                let text = if reason.is_empty() {
                    "Nothing was written.".to_string()
                } else {
                    format!("Nothing was written: {reason}.")
                };
                (text, Tone::Warn)
            }
            Outcome::Cancelled {
                mode: Mode::Fresh,
                stick: CancelledStick::Unchanged,
            } => (
                "Cancelled before anything was written. The stick is as it was.".to_string(),
                Tone::Plain,
            ),
            Outcome::Cancelled {
                mode: Mode::Fresh,
                stick: CancelledStick::Wiped,
            } => (
                "Cancelled. The stick will not start until it is written again.".to_string(),
                Tone::Warn,
            ),
            Outcome::Cancelled {
                mode: Mode::Fresh,
                stick: CancelledStick::MayStart,
            } => (
                "Cancelled after the image was written. The stick may start; write it again to be sure.".to_string(),
                Tone::Warn,
            ),
            Outcome::Cancelled {
                mode: Mode::Upgrade, ..
            } => (
                "Cancelled. The stick still starts its current release.".to_string(),
                Tone::Plain,
            ),
            Outcome::NoResult { status } => (
                format!("The writer ended without a result ({status}). The log shows why."),
                Tone::Warn,
            ),
        }
    }

    fn check(&mut self) -> Option<Request> {
        if self.checking {
            return None;
        }
        let source = self.source().ok()?;
        self.checking = true;
        self.check_source = Some(source.clone());
        Some(Request::Check { source })
    }

    /// `Confirm` would start the write now: the screen enables its button.
    pub fn can_confirm(&self) -> bool {
        if self.stage != Stage::Choose || self.job.is_some() || !self.list_fresh {
            return false;
        }
        let (Some(d), Some(mode)) = (self.chosen_drive(), self.mode) else {
            return false;
        };
        if !self.allowed(mode) {
            return false;
        }
        if mode == Mode::Fresh && drives::internal(d) && !drives::typed_matches(&self.typed, d) {
            return false;
        }
        self.write_args().is_ok()
    }

    /// The drive list was answered since the screen last entered Choose
    /// with the current Show all.
    pub fn listed(&self) -> bool {
        self.list_fresh
    }

    /// The id of the step the running write is in.
    pub fn current_step(&self) -> Option<&str> {
        self.job.as_ref()?.current.as_deref()
    }

    /// The address under Advanced is entered and is not http or https.
    pub fn source_invalid(&self) -> bool {
        self.source().is_err()
    }

    /// The drive of the running write, as the dialogs name it.
    pub fn job_drive(&self) -> Option<&str> {
        self.job.as_ref().map(|j| name(&j.drive))
    }

    fn confirm(&mut self) -> Option<Request> {
        if !self.can_confirm() {
            return None;
        }
        let d = self.chosen_drive()?.clone();
        let mode = self.mode?;
        let args = self.write_args().ok()?;
        self.steps = planned(mode, self.verify, self.copy_home_effective(), d.is_boot);
        self.progress = None;
        self.speed = Speed::new();
        self.cancel_asked = false;
        self.cancelling = false;
        self.job = Some(Job {
            version: self.version().map(String::from),
            verify: mode == Mode::Fresh && self.verify,
            drive: d,
            mode,
            pgid: None,
            current: None,
            done: HashSet::new(),
            touched: false,
            result: None,
        });
        self.stage = Stage::Running;
        self.list_fresh = false;
        Some(Request::Write { args })
    }

    fn choose(&mut self, device: &str) {
        let Some(d) = self.drives.iter().find(|d| d.device == device) else {
            return;
        };
        let c = Chosen {
            id: d.id.clone(),
            device: d.device.clone(),
        };
        if self.chosen.as_ref() == Some(&c) {
            return;
        }
        let first = drives::modes(d, self.image_bytes())
            .into_iter()
            .find(|(_, r)| r.is_ok())
            .map(|(m, _)| m);
        self.chosen = Some(c);
        self.mode = first;
        self.typed.clear();
    }

    fn clear_choice(&mut self) {
        self.chosen = None;
        self.mode = None;
        self.typed.clear();
    }

    /// The chosen drive, when the last list still has its id on its device.
    pub fn chosen_drive(&self) -> Option<&Drive> {
        let c = self.chosen.as_ref()?;
        self.drives.iter().find(|d| d.id == c.id && d.device == c.device)
    }

    fn offered(&self, m: Mode) -> bool {
        self.chosen_drive()
            .is_some_and(|d| drives::modes(d, self.image_bytes()).iter().any(|(o, _)| *o == m))
    }

    fn allowed(&self, m: Mode) -> bool {
        self.chosen_drive().is_some_and(|d| {
            drives::modes(d, self.image_bytes())
                .iter()
                .any(|(o, r)| *o == m && r.is_ok())
        })
    }

    fn copy_home_effective(&self) -> bool {
        self.copy_home && !self.restore_failed
    }

    /// The address under Advanced, when one is entered. An entered address
    /// that is not http or https is an error.
    fn source(&self) -> Result<Option<String>, String> {
        let s = self.source_text.trim();
        if !self.advanced || s.is_empty() {
            return Ok(None);
        }
        if source_ok(s) {
            Ok(Some(s.to_string()))
        } else {
            Err("the address must start with http:// or https://".to_string())
        }
    }

    /// The release the write would install: the checked address's when one
    /// is entered, else the default one.
    pub fn target_release(&self) -> Option<&Release> {
        match self.source() {
            Ok(None) => Some(&self.release),
            Ok(Some(_)) => self.custom_release.as_ref(),
            Err(_) => None,
        }
    }

    fn version(&self) -> Option<&str> {
        self.target_release()?.latest.as_deref()
    }

    pub fn image_bytes(&self) -> Option<u64> {
        self.target_release()?.image_bytes
    }
}

/// `^https?://\S+$`.
fn source_ok(s: &str) -> bool {
    let rest = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://"));
    rest.is_some_and(|r| !r.is_empty() && !r.chars().any(char::is_whitespace))
}

/// The model, or the device when the drive names no model.
pub fn name(d: &Drive) -> &str {
    if d.model.trim().is_empty() { &d.device } else { &d.model }
}

/// The tool's reason as a sentence: a capital first, a full stop last.
fn sentence(s: &str) -> String {
    let s = s.trim();
    let mut c = s.chars();
    let mut out: String = match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => return "The write failed.".to_string(),
    };
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// The steps the tool will run, all waiting.
fn planned(mode: Mode, verify: bool, copy_home: bool, boot_stick: bool) -> Vec<StepView> {
    let mut v: Vec<(&str, &str)> = Vec::new();
    match mode {
        Mode::Fresh => {
            v.push(("write", "Downloading and writing"));
            if verify {
                v.push(("verify", "Checking the stick"));
            }
            v.push(("stamp", "Marking the stick"));
            if copy_home {
                v.push(("home", "Copying the home directory"));
            }
            v.push(("save", "Saving this machine's home"));
        }
        Mode::Upgrade => {
            if boot_stick {
                v.push(("save", "Saving the home directory"));
            }
            v.push(("download", "Downloading the new release"));
            v.push(("check", "Checking the files"));
            v.push(("switch", "Copying the new release to the stick"));
        }
    }
    v.into_iter()
        .map(|(id, label)| StepView {
            id: id.to_string(),
            label: label.to_string(),
            mark: StepMark::Waiting,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn stick(device: &str, id: &str) -> Drive {
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

    fn boot(device: &str, id: &str) -> Drive {
        Drive {
            is_boot: true,
            mounted: true,
            is_thinfloe: true,
            ..stick(device, id)
        }
    }

    fn release() -> Release {
        Release {
            latest: Some("4.1.0".into()),
            image_bytes: Some(3_100_000_000),
            ..Release::default()
        }
    }

    /// An open screen that has listed `drives` once.
    fn ready(drives: Vec<Drive>) -> (Upgrade, Instant) {
        let mut u = Upgrade::new();
        let t = Instant::now();
        u.release_file(release());
        assert_eq!(u.open(t), Some(Request::Check { source: None }));
        assert_eq!(u.poll(t), Some(Request::Drives { all: false }));
        assert_eq!(u.drives_listed(false, Ok(drives), t), None);
        (u, t)
    }

    fn ev(u: &mut Upgrade, e: UpgradeEvent, t: Instant) -> Option<Request> {
        u.event(e, false, t)
    }

    fn write_of(r: Option<Request>) -> Vec<String> {
        match r {
            Some(Request::Write { args }) => args,
            other => panic!("not a write: {other:?}"),
        }
    }

    /// A fresh write of `/dev/sdb` that has started.
    fn running_fresh(verify: bool) -> (Upgrade, Instant) {
        let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-A:1:1")]);
        ev(&mut u, UpgradeEvent::Choose("/dev/sdb".into()), t);
        ev(&mut u, UpgradeEvent::Verify(verify), t);
        write_of(ev(&mut u, UpgradeEvent::Confirm, t));
        u.started(4242);
        (u, t)
    }

    fn running_upgrade(drive: Drive) -> (Upgrade, Instant) {
        let dev = drive.device.clone();
        let (mut u, t) = ready(vec![drive]);
        ev(&mut u, UpgradeEvent::Choose(dev), t);
        ev(&mut u, UpgradeEvent::Mode(Mode::Upgrade), t);
        write_of(ev(&mut u, UpgradeEvent::Confirm, t));
        u.started(4242);
        (u, t)
    }

    fn step(id: &str) -> Msg {
        Msg::Step {
            id: id.into(),
            text: format!("text of {id}"),
        }
    }

    fn done(id: &str) -> Msg {
        Msg::Done { id: id.into() }
    }

    fn result(ok: bool, reason: &str) -> Msg {
        Msg::Result {
            ok,
            reason: reason.into(),
        }
    }

    fn mark(u: &Upgrade, id: &str) -> StepMark {
        u.steps
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("no step {id}"))
            .mark
    }

    fn repo(rel: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
    }

    #[test]
    fn new_has_verify_on_and_nothing_else() {
        let u = Upgrade::new();
        assert!(u.verify);
        assert!(!u.copy_home && !u.show_all && !u.advanced && !u.checking);
        assert!(u.chosen.is_none() && u.mode.is_none() && u.drives.is_empty());
        assert_eq!(u.stage, Stage::Choose);
    }

    #[test]
    fn write_args_match_the_shared_fixture() {
        let text = std::fs::read_to_string(repo("test/unit/upgrade-write-args.tsv")).unwrap();
        let want: Vec<Vec<String>> = text
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| l.split('\t').map(String::from).collect())
            .collect();
        assert_eq!(want.len(), 5);

        let mut got = Vec::new();

        // 1: a blank USB stick, fresh, verify (the default).
        let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-AA1:16008609792:3f2a")]);
        ev(&mut u, UpgradeEvent::Choose("/dev/sdb".into()), t);
        assert_eq!(u.mode, Some(Mode::Fresh));
        let args = write_of(ev(&mut u, UpgradeEvent::Confirm, t));
        assert_eq!(Ok(args.clone()), u.write_args());
        got.push(args);

        // 2: no serial, copy the home.
        let (mut u, t) = ready(vec![stick("/dev/sdb", "nosn:16008609792:3f2a")]);
        ev(&mut u, UpgradeEvent::Choose("/dev/sdb".into()), t);
        ev(&mut u, UpgradeEvent::CopyHome(true), t);
        got.push(write_of(ev(&mut u, UpgradeEvent::Confirm, t)));

        // 3: the boot stick, upgrade; verify is on but not for an upgrade.
        let (mut u, t) = ready(vec![boot("/dev/sda", "sn-BOOT:31914983424:9c01")]);
        ev(&mut u, UpgradeEvent::Choose("/dev/sda".into()), t);
        assert_eq!(u.mode, Some(Mode::Upgrade));
        got.push(write_of(ev(&mut u, UpgradeEvent::Confirm, t)));

        // 4: an internal drive, shown with Show all, its name typed back.
        let nvme = Drive {
            model: "Samsung SSD".into(),
            transport: Some("nvme".into()),
            removable: false,
            size_bytes: 512_110_190_592,
            ..stick("/dev/nvme0n1", "sn-X:512110190592:77aa")
        };
        let (mut u, t) = ready(vec![]);
        assert_eq!(ev(&mut u, UpgradeEvent::ShowAll(true), t), None);
        assert_eq!(u.poll(t), Some(Request::Drives { all: true }));
        u.drives_listed(true, Ok(vec![nvme.clone()]), t);
        ev(&mut u, UpgradeEvent::Choose("/dev/nvme0n1".into()), t);
        ev(&mut u, UpgradeEvent::Verify(false), t);
        ev(&mut u, UpgradeEvent::Typed(drives::confirm_text(&nvme)), t);
        got.push(write_of(ev(&mut u, UpgradeEvent::Confirm, t)));

        // 5: another ThinFloe stick, upgrade from the address under Advanced.
        let tf = Drive {
            is_thinfloe: true,
            ..stick("/dev/sdc", "sn-C:8004304896:0011")
        };
        let (mut u, t) = ready(vec![tf]);
        ev(&mut u, UpgradeEvent::Choose("/dev/sdc".into()), t);
        ev(&mut u, UpgradeEvent::Mode(Mode::Upgrade), t);
        ev(&mut u, UpgradeEvent::Advanced(true), t);
        ev(&mut u, UpgradeEvent::Source("https://example.org/rel".into()), t);
        got.push(write_of(ev(&mut u, UpgradeEvent::Confirm, t)));

        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            assert_eq!(g, w, "line {}", i + 1);
        }
    }

    #[test]
    fn planned_step_ids_are_the_tools() {
        let tool = std::fs::read_to_string(repo("overlay/usr/bin/thinfloe-upgrade")).unwrap();
        let mut ids = Vec::new();
        ids.extend(planned(Mode::Fresh, true, true, false));
        ids.extend(planned(Mode::Upgrade, true, true, true));
        assert_eq!(ids.len(), 9);
        for s in ids {
            let needle = format!("progress.step(\"{}\", ", s.id);
            assert!(tool.contains(&needle), "{needle}");
            assert_eq!(s.mark, StepMark::Waiting);
        }
    }

    #[test]
    fn planned_steps_follow_the_options() {
        let ids = |m, v, h, b| planned(m, v, h, b).into_iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(
            ids(Mode::Fresh, true, true, false),
            ["write", "verify", "stamp", "home", "save"]
        );
        assert_eq!(ids(Mode::Fresh, false, false, false), ["write", "stamp", "save"]);
        assert_eq!(
            ids(Mode::Upgrade, true, true, true),
            ["save", "download", "check", "switch"]
        );
        assert_eq!(ids(Mode::Upgrade, true, true, false), ["download", "check", "switch"]);
        let fresh = planned(Mode::Fresh, true, true, false);
        assert_eq!(fresh[0].label, "Downloading and writing");
        assert_eq!(fresh[4].label, "Saving this machine's home");
        let upgrade = planned(Mode::Upgrade, true, true, false);
        assert_eq!(upgrade[2].label, "Copying the new release to the stick");
        assert_eq!(
            planned(Mode::Upgrade, true, true, true)[0].label,
            "Saving the home directory"
        );
    }

    #[test]
    fn a_pulled_stick_shows_the_tools_reason() {
        let (mut u, t) = running_fresh(true);
        u.message(step("write"), t);
        assert_eq!(mark(&u, "write"), StepMark::Running);
        u.message(
            Msg::Progress {
                done: 1_000_000,
                total: Some(3_000_000),
            },
            t,
        );
        u.message(
            Msg::Progress {
                done: 2_000_000,
                total: Some(3_000_000),
            },
            t + Duration::from_secs(4),
        );
        assert_eq!(u.progress, Some((2_000_000, Some(3_000_000))));
        assert!(u.speed.rate().is_some());
        u.message(result(false, "the stick was removed"), t);
        u.ended(Ok(Some(1)));
        assert_eq!(
            u.stage,
            Stage::Finished(Outcome::Failed {
                reason: "the stick was removed".into()
            })
        );
        assert_eq!(u.outcome_text(), ("The stick was removed.".to_string(), Tone::Warn));
        assert_eq!(mark(&u, "write"), StepMark::Failed);
        assert_eq!(mark(&u, "verify"), StepMark::Waiting);
        assert!(!u.writing());
        assert_eq!(u.pgid(), None);
        // Try again, and the next poll no longer lists the stick.
        assert_eq!(ev(&mut u, UpgradeEvent::TryAgain, t), None);
        assert_eq!(u.stage, Stage::Choose);
        assert_eq!(u.poll(t), Some(Request::Drives { all: false }));
        u.drives_listed(false, Ok(vec![]), t);
        assert!(u.chosen.is_none());
        assert_eq!(ev(&mut u, UpgradeEvent::Confirm, t), None);
    }

    #[test]
    fn a_drive_that_disappears_clears_the_choice() {
        let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-A:1:1"), stick("/dev/sdc", "sn-C:1:1")]);
        ev(&mut u, UpgradeEvent::Choose("/dev/sdb".into()), t);
        assert_eq!(
            u.chosen,
            Some(Chosen {
                id: "sn-A:1:1".into(),
                device: "/dev/sdb".into()
            })
        );
        assert_eq!(u.poll(t), None, "the next poll is POLL away");
        let later = t + POLL;
        assert_eq!(u.poll(later), Some(Request::Drives { all: false }));
        u.drives_listed(false, Ok(vec![stick("/dev/sdc", "sn-C:1:1")]), later);
        assert!(u.chosen.is_none() && u.mode.is_none() && u.typed.is_empty());
        assert_eq!(ev(&mut u, UpgradeEvent::Confirm, later), None);
        assert!(u.write_args().is_err());
    }

    #[test]
    fn a_device_with_a_new_id_is_not_the_chosen_drive() {
        let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-A:16008609792:3f2a")]);
        ev(&mut u, UpgradeEvent::Choose("/dev/sdb".into()), t);
        ev(&mut u, UpgradeEvent::Typed("x".into()), t);
        assert!(u.chosen.is_some());
        u.drives_listed(false, Ok(vec![stick("/dev/sdb", "sn-B:16008609792:3f2a")]), t + POLL);
        assert!(u.chosen.is_none() && u.mode.is_none() && u.typed.is_empty());
        assert_eq!(ev(&mut u, UpgradeEvent::Confirm, t + POLL), None);
        // The same id on another device is not the choice either.
        let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-A:1:1")]);
        ev(&mut u, UpgradeEvent::Choose("/dev/sdb".into()), t);
        u.drives_listed(false, Ok(vec![stick("/dev/sdc", "sn-A:1:1")]), t + POLL);
        assert!(u.chosen.is_none());
    }

    #[test]
    fn an_unexamined_row_keeps_what_the_last_examined_row_said() {
        let tf = Drive {
            is_thinfloe: true,
            has_tl_old: true,
            version: Some("4.0.0".into()),
            ..stick("/dev/sdb", "sn-A:1:1")
        };
        let (mut u, t) = ready(vec![tf]);
        let bare = Drive {
            examined: false,
            ..stick("/dev/sdb", "sn-A:1:1")
        };
        let other = Drive {
            examined: false,
            ..stick("/dev/sdc", "sn-C:1:1")
        };
        u.drives_listed(false, Ok(vec![bare, other]), t + POLL);
        let a = &u.drives[0];
        assert!(a.examined && a.is_thinfloe && a.has_tl_old && !a.has_tl_new);
        assert_eq!(a.version.as_deref(), Some("4.0.0"));
        // No earlier examined row: it stays unexamined and is refused.
        let c = &u.drives[1];
        assert!(!c.examined);
        assert!(drives::modes(c, None).iter().all(|(_, r)| r.is_err()));
        ev(&mut u, UpgradeEvent::Choose("/dev/sdc".into()), t);
        assert_eq!(u.mode, None);
        assert_eq!(ev(&mut u, UpgradeEvent::Mode(Mode::Fresh), t), None);
        assert_eq!(ev(&mut u, UpgradeEvent::Confirm, t), None);
    }

    #[test]
    fn no_result_line_is_a_failure() {
        for code in [0, 1] {
            let (mut u, t) = running_fresh(true);
            u.message(step("write"), t);
            u.ended(Ok(Some(code)));
            let Stage::Finished(Outcome::NoResult { status }) = &u.stage else {
                panic!("{:?}", u.stage)
            };
            assert!(status.contains(&code.to_string()), "{status}");
            let (text, tone) = u.outcome_text();
            assert_eq!(
                text,
                format!("The writer ended without a result ({status}). The log shows why.")
            );
            assert_eq!(tone, Tone::Warn);
        }
        // Exit 130 or 2 without a result is no result too.
        for code in [130, 2] {
            let (mut u, _) = running_fresh(true);
            u.ended(Ok(Some(code)));
            assert!(matches!(u.stage, Stage::Finished(Outcome::NoResult { .. })), "{code}");
        }
        let (mut u, _) = running_fresh(true);
        u.ended(Err("could not wait".into()));
        assert_eq!(
            u.stage,
            Stage::Finished(Outcome::NoResult {
                status: "could not wait".into()
            })
        );
    }

    #[test]
    fn result_wins_over_the_exit_code() {
        let (mut u, t) = running_fresh(false);
        u.message(result(true, ""), t);
        u.ended(Ok(Some(1)));
        assert_eq!(
            u.stage,
            Stage::Finished(Outcome::Done {
                boot_stick: false,
                late_cancel: false
            })
        );
        assert_eq!(
            u.outcome_text(),
            (
                "The stick is written. You can remove the stick.".to_string(),
                Tone::Good
            )
        );
        assert_eq!(ev(&mut u, UpgradeEvent::RestartNow, t), None);
    }

    #[test]
    fn exit_130_after_the_write_may_leave_a_good_stick() {
        let (mut u, t) = running_fresh(true);
        for m in [
            step("write"),
            done("write"),
            step("verify"),
            done("verify"),
            step("stamp"),
        ] {
            u.message(m, t);
        }
        assert_eq!(ev(&mut u, UpgradeEvent::Cancel, t), None);
        assert_eq!(ev(&mut u, UpgradeEvent::CancelYes, t), Some(Request::Stop));
        u.message(result(false, "cancelled"), t);
        u.ended(Ok(Some(130)));
        assert_eq!(
            u.stage,
            Stage::Finished(Outcome::Cancelled {
                mode: Mode::Fresh,
                stick: CancelledStick::MayStart
            })
        );
        assert_eq!(
            u.outcome_text().0,
            "Cancelled after the image was written. The stick may start; write it again to be sure."
        );
        // Without verify, `@done write` is enough.
        let (mut u, t) = running_fresh(false);
        u.message(step("write"), t);
        u.message(done("write"), t);
        u.message(result(false, "cancelled"), t);
        u.ended(Ok(Some(130)));
        assert_eq!(
            u.stage,
            Stage::Finished(Outcome::Cancelled {
                mode: Mode::Fresh,
                stick: CancelledStick::MayStart
            })
        );
    }

    #[test]
    fn a_cancel_before_the_first_byte_leaves_the_stick() {
        // No @touched: the tool wrote nothing, the stick is as it was.
        let (mut u, t) = running_fresh(true);
        u.message(step("write"), t);
        assert!(u.fresh_running());
        assert!(!u.fresh_touched());
        u.message(result(false, "cancelled"), t);
        u.ended(Ok(Some(130)));
        assert_eq!(
            u.stage,
            Stage::Finished(Outcome::Cancelled {
                mode: Mode::Fresh,
                stick: CancelledStick::Unchanged
            })
        );
        assert_eq!(
            u.outcome_text(),
            (
                "Cancelled before anything was written. The stick is as it was.".to_string(),
                Tone::Plain
            )
        );
    }

    #[test]
    fn a_cancel_after_the_first_byte_leaves_a_wiped_stick() {
        let (mut u, t) = running_fresh(true);
        u.message(step("write"), t);
        u.message(Msg::Touched, t);
        assert!(u.fresh_touched());
        u.message(result(false, "cancelled"), t);
        u.ended(Ok(Some(130)));
        assert_eq!(
            u.stage,
            Stage::Finished(Outcome::Cancelled {
                mode: Mode::Fresh,
                stick: CancelledStick::Wiped
            })
        );
        assert_eq!(
            u.outcome_text().0,
            "Cancelled. The stick will not start until it is written again."
        );
    }

    #[test]
    fn before_the_write_the_stick_is_wiped() {
        let (mut u, t) = running_fresh(true);
        u.message(step("write"), t);
        u.message(done("write"), t);
        u.message(step("verify"), t);
        u.message(result(false, "cancelled"), t);
        u.ended(Ok(Some(130)));
        assert_eq!(
            u.stage,
            Stage::Finished(Outcome::Cancelled {
                mode: Mode::Fresh,
                stick: CancelledStick::Wiped
            })
        );
        assert_eq!(
            u.outcome_text().0,
            "Cancelled. The stick will not start until it is written again."
        );
        assert_eq!(mark(&u, "verify"), StepMark::Failed);
        // An upgrade that is cancelled leaves the stick as it was.
        let (mut u, t) = running_upgrade(Drive {
            is_thinfloe: true,
            ..stick("/dev/sdc", "sn-C:1:1")
        });
        assert_eq!(ev(&mut u, UpgradeEvent::Cancel, t), Some(Request::Stop));
        u.message(result(false, "cancelled"), t);
        u.ended(Ok(Some(130)));
        assert_eq!(
            u.stage,
            Stage::Finished(Outcome::Cancelled {
                mode: Mode::Upgrade,
                stick: CancelledStick::Unchanged
            })
        );
        assert_eq!(
            u.outcome_text().0,
            "Cancelled. The stick still starts its current release."
        );
    }

    #[test]
    fn a_cancel_during_switch_reports_too_late() {
        let (mut u, t) = running_upgrade(boot("/dev/sda", "sn-BOOT:1:1"));
        assert_eq!(
            u.steps.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["save", "download", "check", "switch"]
        );
        for m in [
            step("save"),
            done("save"),
            step("download"),
            done("download"),
            step("check"),
            done("check"),
            step("switch"),
        ] {
            u.message(m, t);
        }
        assert_eq!(ev(&mut u, UpgradeEvent::Cancel, t), Some(Request::Stop));
        assert!(u.cancelling && !u.cancel_asked);
        u.message(done("switch"), t);
        u.message(result(true, ""), t);
        u.ended(Ok(Some(0)));
        assert_eq!(
            u.stage,
            Stage::Finished(Outcome::Done {
                boot_stick: true,
                late_cancel: true
            })
        );
        assert_eq!(
            u.outcome_text().0,
            "The cancel came too late: the write was finished. ThinFloe 4.1.0 is ready. It starts after a restart."
        );
        assert_eq!(ev(&mut u, UpgradeEvent::RestartNow, t), Some(Request::Restart));
    }

    #[test]
    fn the_boot_stick_text_without_a_version() {
        let (mut u, t) = ready(vec![boot("/dev/sda", "sn-BOOT:1:1")]);
        u.release_file(Release::default());
        ev(&mut u, UpgradeEvent::Choose("/dev/sda".into()), t);
        write_of(ev(&mut u, UpgradeEvent::Confirm, t));
        u.message(result(true, ""), t);
        u.ended(Ok(Some(0)));
        assert_eq!(
            u.outcome_text(),
            (
                "The new release is ready. It starts after a restart.".to_string(),
                Tone::Good
            )
        );
    }

    #[test]
    fn fresh_cancel_asks_first_and_signals_once() {
        let (mut u, t) = running_fresh(true);
        assert!(u.fresh_running());
        assert_eq!(u.job_name().as_deref(), Some("Writing SanDisk Cruzer"));
        assert_eq!(u.pgid(), Some(4242));
        assert_eq!(ev(&mut u, UpgradeEvent::Cancel, t), None);
        assert!(u.cancel_asked && !u.cancelling);
        assert_eq!(ev(&mut u, UpgradeEvent::CancelNo, t), None);
        assert!(!u.cancel_asked && !u.cancelling);
        assert_eq!(ev(&mut u, UpgradeEvent::Cancel, t), None);
        assert_eq!(ev(&mut u, UpgradeEvent::CancelYes, t), Some(Request::Stop));
        assert!(u.cancelling && !u.cancel_asked);
        assert_eq!(ev(&mut u, UpgradeEvent::Cancel, t), None);
        assert_eq!(ev(&mut u, UpgradeEvent::CancelYes, t), None);
        assert!(!u.cancel_asked);
        // An upgrade signals at once, and also only once.
        let (mut u, t) = running_upgrade(Drive {
            is_thinfloe: true,
            ..stick("/dev/sdc", "sn-C:1:1")
        });
        assert!(!u.fresh_running());
        assert_eq!(u.job_name().as_deref(), Some("Upgrading SanDisk Cruzer"));
        assert_eq!(ev(&mut u, UpgradeEvent::Cancel, t), Some(Request::Stop));
        assert_eq!(ev(&mut u, UpgradeEvent::Cancel, t), None);
    }

    #[test]
    fn refused_goes_back_with_try_again_keeping_the_choices() {
        let tf = Drive {
            is_thinfloe: true,
            ..stick("/dev/sdc", "sn-C:1:1")
        };
        let (mut u, t) = ready(vec![tf]);
        ev(&mut u, UpgradeEvent::Choose("/dev/sdc".into()), t);
        ev(&mut u, UpgradeEvent::Mode(Mode::Fresh), t);
        ev(&mut u, UpgradeEvent::CopyHome(true), t);
        ev(&mut u, UpgradeEvent::Verify(false), t);
        let first = write_of(ev(&mut u, UpgradeEvent::Confirm, t));
        assert_eq!(ev(&mut u, UpgradeEvent::Confirm, t), None, "one write at a time");
        u.started(7);
        u.message(result(false, "another write is running"), t);
        u.ended(Ok(Some(2)));
        assert_eq!(
            u.stage,
            Stage::Finished(Outcome::Refused {
                reason: "another write is running".into()
            })
        );
        assert_eq!(
            u.outcome_text(),
            ("Nothing was written: another write is running.".to_string(), Tone::Warn)
        );
        assert_eq!(ev(&mut u, UpgradeEvent::RestartNow, t), None);
        assert_eq!(ev(&mut u, UpgradeEvent::TryAgain, t), None);
        assert_eq!(u.stage, Stage::Choose);
        assert!(u.steps.is_empty() && u.progress.is_none());
        assert_eq!(u.chosen.as_ref().map(|c| c.device.as_str()), Some("/dev/sdc"));
        assert_eq!(u.mode, Some(Mode::Fresh));
        assert!(u.copy_home && !u.verify);
        // A write is confirmed again only from a list made after this one.
        let tf = Drive {
            is_thinfloe: true,
            ..stick("/dev/sdc", "sn-C:1:1")
        };
        u.drives_listed(false, Ok(vec![tf]), t);
        assert_eq!(write_of(ev(&mut u, UpgradeEvent::Confirm, t)), first);
    }

    #[test]
    fn back_leaves_in_every_stage_and_the_job_keeps_running() {
        let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-A:1:1")]);
        assert_eq!(ev(&mut u, UpgradeEvent::Back, t), Some(Request::Leave));
        let (mut u, t) = running_fresh(true);
        assert_eq!(ev(&mut u, UpgradeEvent::Back, t), Some(Request::Leave));
        assert!(u.writing() && u.stage == Stage::Running);
        u.message(result(true, ""), t);
        u.ended(Ok(Some(0)));
        assert_eq!(ev(&mut u, UpgradeEvent::Back, t), Some(Request::Leave));
        assert!(matches!(u.stage, Stage::Finished(_)), "Back alone keeps the outcome");
    }

    #[test]
    fn reopening_after_a_boot_stick_upgrade_keeps_restart_now() {
        let (mut u, t) = running_upgrade(Drive {
            is_boot: true,
            ..stick("/dev/sda", "sn-B:1:1")
        });
        u.message(result(true, ""), t);
        u.ended(Ok(Some(0)));
        assert_eq!(ev(&mut u, UpgradeEvent::Back, t), Some(Request::Leave));
        u.open(t + Duration::from_secs(30));
        assert!(
            matches!(u.stage, Stage::Finished(Outcome::Done { boot_stick: true, .. })),
            "{:?}",
            u.stage
        );
        assert_eq!(ev(&mut u, UpgradeEvent::RestartNow, t), Some(Request::Restart));
    }

    #[test]
    fn write_another_stick_after_a_boot_stick_upgrade() {
        let (mut u, t) = running_upgrade(Drive {
            is_boot: true,
            ..stick("/dev/sda", "sn-B:1:1")
        });
        u.message(result(true, ""), t);
        u.ended(Ok(Some(0)));
        assert_eq!(ev(&mut u, UpgradeEvent::WriteAnother, t), None);
        assert_eq!(u.stage, Stage::Choose);
        assert!(u.chosen.is_none() && u.mode.is_none() && u.steps.is_empty());
        assert!(!u.listed());
        assert_eq!(u.poll(t), Some(Request::Drives { all: false }));
        // Only from a finished write.
        let (mut u, t) = running_fresh(true);
        ev(&mut u, UpgradeEvent::WriteAnother, t);
        assert_eq!(u.stage, Stage::Running);
    }

    #[test]
    fn reopening_after_a_write_offers_another_write() {
        let (mut u, t) = running_fresh(true);
        u.message(result(true, ""), t);
        u.ended(Ok(Some(0)));
        assert_eq!(ev(&mut u, UpgradeEvent::Back, t), Some(Request::Leave));
        let later = t + Duration::from_secs(30);
        u.open(later);
        assert_eq!(u.stage, Stage::Choose);
        assert!(u.steps.is_empty() && u.progress.is_none());
        assert!(!u.listed(), "the list from before the write confirms nothing");
        assert!(!u.can_confirm());
        assert_eq!(u.poll(later), Some(Request::Drives { all: false }));
        u.drives_listed(false, Ok(vec![stick("/dev/sdb", "sn-A:1:1")]), later);
        ev(&mut u, UpgradeEvent::Choose("/dev/sdb".into()), later);
        assert!(u.can_confirm(), "a second write can start without a restart");
        // A write that still runs is not reset by opening the screen.
        let (mut u, t) = running_fresh(true);
        u.open(t);
        assert_eq!(u.stage, Stage::Running);
    }

    #[test]
    fn an_internal_drive_needs_its_name_typed() {
        let nvme = Drive {
            model: "Samsung SSD".into(),
            transport: Some("nvme".into()),
            removable: false,
            size_bytes: 512_110_190_592,
            ..stick("/dev/nvme0n1", "sn-X:1:1")
        };
        let (mut u, t) = ready(vec![]);
        ev(&mut u, UpgradeEvent::ShowAll(true), t);
        u.poll(t);
        u.drives_listed(true, Ok(vec![nvme]), t);
        ev(&mut u, UpgradeEvent::Choose("/dev/nvme0n1".into()), t);
        assert_eq!(u.mode, Some(Mode::Fresh));
        assert_eq!(u.confirm_label().as_deref(), Some("Erase Samsung SSD 512 GB"));
        assert_eq!(ev(&mut u, UpgradeEvent::Confirm, t), None);
        ev(&mut u, UpgradeEvent::Typed("samsung ssd 500 gb".into()), t);
        assert_eq!(ev(&mut u, UpgradeEvent::Confirm, t), None);
        ev(&mut u, UpgradeEvent::Typed("  samsung  SSD 512 gb ".into()), t);
        let args = write_of(ev(&mut u, UpgradeEvent::Confirm, t));
        assert_eq!(args.last().map(String::as_str), Some("--allow-internal"));
        // A list without Show all no longer has it.
        let (mut u, t) = ready(vec![]);
        ev(&mut u, UpgradeEvent::ShowAll(true), t);
        u.drives_listed(false, Ok(vec![stick("/dev/sdb", "sn-A:1:1")]), t);
        assert!(u.drives.is_empty(), "a list for the other setting is dropped");
        assert_eq!(u.poll(t), Some(Request::Drives { all: true }));
    }

    #[test]
    fn copy_home_is_never_passed_after_a_failed_restore() {
        let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-A:1:1")]);
        u.event(UpgradeEvent::Choose("/dev/sdb".into()), true, t);
        u.event(UpgradeEvent::CopyHome(true), true, t);
        assert!(!u.copy_home);
        let args = write_of(u.event(UpgradeEvent::Confirm, true, t));
        assert!(!args.iter().any(|a| a == "--copy-home"), "{args:?}");
        assert!(!u.steps.iter().any(|s| s.id == "home"));
        // Ticked before, and the restore failed since: still not passed.
        let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-A:1:1")]);
        u.event(UpgradeEvent::Choose("/dev/sdb".into()), false, t);
        u.event(UpgradeEvent::CopyHome(true), false, t);
        let args = write_of(u.event(UpgradeEvent::Confirm, true, t));
        assert!(!args.iter().any(|a| a == "--copy-home"), "{args:?}");
    }

    #[test]
    fn an_invalid_source_blocks_confirm() {
        for bad in ["ftp://x", "http:// x", "-o /etc/passwd", "http://", "https://a\tb"] {
            let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-A:1:1")]);
            u.released(false, Ok(release()));
            ev(&mut u, UpgradeEvent::Choose("/dev/sdb".into()), t);
            ev(&mut u, UpgradeEvent::Advanced(true), t);
            ev(&mut u, UpgradeEvent::Source(bad.into()), t);
            assert_eq!(ev(&mut u, UpgradeEvent::Confirm, t), None, "{bad}");
            assert!(u.write_args().is_err(), "{bad}");
            assert_eq!(ev(&mut u, UpgradeEvent::CheckNow, t), None, "{bad}");
        }
        let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-A:1:1")]);
        ev(&mut u, UpgradeEvent::Choose("/dev/sdb".into()), t);
        ev(&mut u, UpgradeEvent::Advanced(true), t);
        ev(&mut u, UpgradeEvent::Source("http://boot.example/rel".into()), t);
        assert!(write_of(ev(&mut u, UpgradeEvent::Confirm, t)).contains(&"http://boot.example/rel".to_string()));
    }

    #[test]
    fn the_source_check_uses_its_own_release() {
        let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-A:1:1")]);
        // The check of open() still runs.
        assert_eq!(ev(&mut u, UpgradeEvent::CheckNow, t), None);
        u.released(false, Ok(release()));
        assert!(!u.checking);
        ev(&mut u, UpgradeEvent::Advanced(true), t);
        ev(&mut u, UpgradeEvent::Source("https://example.org/rel".into()), t);
        assert_eq!(
            ev(&mut u, UpgradeEvent::CheckNow, t),
            Some(Request::Check {
                source: Some("https://example.org/rel".into())
            })
        );
        assert!(u.checking);
        u.released(
            true,
            Ok(Release {
                latest: Some("4.2.0".into()),
                ..Release::default()
            }),
        );
        ev(&mut u, UpgradeEvent::Choose("/dev/sdb".into()), t);
        assert_eq!(
            u.summary().as_deref(),
            Some("Erase SanDisk Cruzer, 16 GB, and install ThinFloe 4.2.0")
        );
        // A new address drops the release of the old one.
        ev(&mut u, UpgradeEvent::Source("https://example.org/other".into()), t);
        assert!(u.custom_release.is_none());
        assert_eq!(
            u.summary().as_deref(),
            Some("Erase SanDisk Cruzer, 16 GB, and install the newest ThinFloe")
        );
        // A failed check keeps its reason.
        ev(&mut u, UpgradeEvent::Advanced(false), t);
        assert_eq!(
            ev(&mut u, UpgradeEvent::CheckNow, t),
            Some(Request::Check { source: None })
        );
        u.released(false, Err("no network".into()));
        assert_eq!(u.release.error.as_deref(), Some("no network"));
    }

    #[test]
    fn summary_and_confirm_label() {
        let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-A:1:1"), boot("/dev/sda", "sn-B:1:1")]);
        assert_eq!(u.summary(), None);
        assert_eq!(u.confirm_label(), None);
        ev(&mut u, UpgradeEvent::Choose("/dev/sdb".into()), t);
        assert_eq!(
            u.summary().as_deref(),
            Some("Erase SanDisk Cruzer, 16 GB, and install ThinFloe 4.1.0")
        );
        assert_eq!(u.confirm_label().as_deref(), Some("Erase SanDisk Cruzer 16 GB"));
        ev(&mut u, UpgradeEvent::Choose("/dev/sda".into()), t);
        assert_eq!(
            u.summary().as_deref(),
            Some("Upgrade this machine's stick to ThinFloe 4.1.0. It becomes active after a restart.")
        );
        assert_eq!(u.confirm_label().as_deref(), Some("Upgrade"));
        let tf = Drive {
            is_thinfloe: true,
            ..stick("/dev/sdc", "sn-C:1:1")
        };
        let (mut u, t) = ready(vec![tf]);
        ev(&mut u, UpgradeEvent::Choose("/dev/sdc".into()), t);
        assert_eq!(
            u.mode,
            Some(Mode::Upgrade),
            "an upgrade is the default for a ThinFloe stick"
        );
        assert_eq!(
            u.summary().as_deref(),
            Some("Upgrade SanDisk Cruzer, 16 GB, to ThinFloe 4.1.0")
        );
        ev(&mut u, UpgradeEvent::Mode(Mode::Fresh), t);
        assert_eq!(u.confirm_label().as_deref(), Some("Erase SanDisk Cruzer 16 GB"));
    }

    #[test]
    fn steps_follow_the_tool() {
        let (mut u, t) = running_fresh(true);
        assert!(u.steps.iter().all(|s| s.mark == StepMark::Waiting));
        // Progress before any step belongs to no step.
        u.message(Msg::Progress { done: 5, total: None }, t);
        assert_eq!(u.progress, None);
        u.message(step("write"), t);
        u.message(
            Msg::Progress {
                done: 5,
                total: Some(10),
            },
            t,
        );
        u.message(
            Msg::Progress {
                done: 9,
                total: Some(10),
            },
            t + Duration::from_secs(5),
        );
        assert!(u.speed.rate().is_some());
        u.message(done("write"), t);
        assert_eq!(mark(&u, "write"), StepMark::Done);
        u.message(step("verify"), t);
        assert_eq!(u.progress, None, "a new step starts without progress");
        assert!(u.speed.rate().is_none(), "and with a new speed");
        u.message(
            Msg::Step {
                id: "extra".into(),
                text: "Something new".into(),
            },
            t,
        );
        let extra = u.steps.last().unwrap();
        assert_eq!(
            (extra.id.as_str(), extra.label.as_str(), extra.mark),
            ("extra", "Something new", StepMark::Running)
        );
        for i in 0..40 {
            u.message(step(&format!("x{i}")), t);
        }
        assert_eq!(u.steps.len(), STEPS_MAX);
        // A second @result does not replace the first.
        u.message(result(true, ""), t);
        u.message(result(false, "late"), t);
        u.ended(Ok(Some(0)));
        assert!(matches!(u.stage, Stage::Finished(Outcome::Done { .. })));
    }

    #[test]
    fn messages_after_the_end_are_ignored() {
        let (mut u, t) = running_fresh(true);
        u.message(step("write"), t);
        u.message(result(false, "the stick was removed"), t);
        u.ended(Ok(Some(1)));
        let stage = u.stage.clone();
        let steps = u.steps.clone();
        u.message(step("verify"), t);
        u.message(Msg::Progress { done: 1, total: None }, t);
        u.message(result(true, ""), t);
        u.started(99);
        u.ended(Ok(Some(0)));
        assert_eq!(u.stage, stage);
        assert_eq!(u.steps, steps);
        assert_eq!(u.pgid(), None);
        assert_eq!(u.progress, None);
    }

    #[test]
    fn drives_errors_are_logged_once_until_they_change() {
        let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-A:1:1")]);
        ev(&mut u, UpgradeEvent::Choose("/dev/sdb".into()), t);
        let line = u.drives_listed(false, Err("sudo: a password is required".into()), t);
        assert!(line.unwrap().contains("sudo: a password is required"));
        assert_eq!(u.drives_error.as_deref(), Some("sudo: a password is required"));
        assert!(u.drives.is_empty() && u.chosen.is_none(), "an error lists nothing");
        assert_eq!(
            u.drives_listed(false, Err("sudo: a password is required".into()), t),
            None
        );
        assert!(
            u.drives_listed(false, Err("drive list: not an array".into()), t)
                .is_some()
        );
        assert_eq!(u.drives_listed(false, Err("drive list: not an array".into()), t), None);
        assert_eq!(u.drives_listed(false, Ok(vec![]), t), None);
        assert_eq!(u.drives_error, None);
        // After a good list the same error is news again.
        assert!(
            u.drives_listed(false, Err("drive list: not an array".into()), t)
                .is_some()
        );
    }

    #[test]
    fn polls_only_while_choosing() {
        let (mut u, t) = running_fresh(true);
        for s in 0..10 {
            assert_eq!(u.poll(t + Duration::from_secs(s * 2)), None, "running");
        }
        u.message(result(true, ""), t);
        u.ended(Ok(Some(0)));
        assert_eq!(u.poll(t + Duration::from_secs(60)), None, "finished");
        // Back to Choose, a poll runs at once, and none while it runs.
        ev(&mut u, UpgradeEvent::TryAgain, t + Duration::from_secs(60));
        let t2 = t + Duration::from_secs(60);
        assert_eq!(u.poll(t2), Some(Request::Drives { all: false }));
        assert_eq!(u.poll(t2 + Duration::from_secs(10)), None, "one drives job at a time");
        u.drives_listed(false, Ok(vec![]), t2 + Duration::from_secs(10));
        assert_eq!(u.poll(t2 + Duration::from_secs(11)), None);
        assert!(u.poll(t2 + Duration::from_secs(12)).is_some());
    }

    #[test]
    fn try_again_waits_for_a_fresh_drive_list() {
        let (mut u, t) = running_fresh(true);
        u.message(step("write"), t);
        u.message(result(false, "the stick was removed"), t);
        u.ended(Ok(Some(1)));
        ev(&mut u, UpgradeEvent::TryAgain, t);
        assert_eq!(u.stage, Stage::Choose);
        // The list from before the write says nothing about the stick now.
        assert_eq!(ev(&mut u, UpgradeEvent::Confirm, t), None);
        assert_eq!(u.poll(t), Some(Request::Drives { all: false }));
        assert_eq!(ev(&mut u, UpgradeEvent::Confirm, t), None, "the answer is not in yet");
        u.drives_listed(false, Ok(vec![stick("/dev/sdb", "sn-A:1:1")]), t);
        assert!(matches!(
            ev(&mut u, UpgradeEvent::Confirm, t),
            Some(Request::Write { .. })
        ));
    }

    #[test]
    fn a_check_answer_for_another_address_is_dropped() {
        let (mut u, t) = ready(vec![stick("/dev/sdb", "sn-A:1:1")]);
        u.released(false, Ok(release()));
        ev(&mut u, UpgradeEvent::Choose("/dev/sdb".into()), t);
        ev(&mut u, UpgradeEvent::Advanced(true), t);
        ev(&mut u, UpgradeEvent::Source("https://a.example/rel".into()), t);
        assert_eq!(
            ev(&mut u, UpgradeEvent::CheckNow, t),
            Some(Request::Check {
                source: Some("https://a.example/rel".into())
            })
        );
        ev(&mut u, UpgradeEvent::Source("https://b.example/rel".into()), t);
        u.released(
            true,
            Ok(Release {
                latest: Some("9.9.9".into()),
                image_bytes: Some(1),
                ..Release::default()
            }),
        );
        assert!(!u.checking);
        assert!(u.custom_release.is_none(), "A's answer is not B's release");
        assert_eq!(
            u.summary().as_deref(),
            Some("Erase SanDisk Cruzer, 16 GB, and install the newest ThinFloe")
        );
        // Advanced turned off while the check runs: the answer is dropped too.
        ev(&mut u, UpgradeEvent::CheckNow, t);
        ev(&mut u, UpgradeEvent::Advanced(false), t);
        u.released(
            true,
            Ok(Release {
                latest: Some("9.9.9".into()),
                ..Release::default()
            }),
        );
        assert!(u.custom_release.is_none());
        assert_eq!(u.release.latest.as_deref(), Some("4.1.0"));
        // A check of the default address still counts after an address is entered.
        ev(&mut u, UpgradeEvent::CheckNow, t);
        ev(&mut u, UpgradeEvent::Advanced(true), t);
        u.released(
            false,
            Ok(Release {
                latest: Some("4.1.1".into()),
                ..Release::default()
            }),
        );
        assert_eq!(u.release.latest.as_deref(), Some("4.1.1"));
        assert!(u.custom_release.is_none());
    }

    #[test]
    fn show_all_off_drops_the_internal_choice() {
        let nvme = Drive {
            transport: Some("nvme".into()),
            removable: false,
            ..stick("/dev/nvme0n1", "sn-X:1:1")
        };
        let (mut u, t) = ready(vec![]);
        ev(&mut u, UpgradeEvent::ShowAll(true), t);
        u.poll(t);
        u.drives_listed(true, Ok(vec![nvme.clone()]), t);
        ev(&mut u, UpgradeEvent::Choose("/dev/nvme0n1".into()), t);
        ev(&mut u, UpgradeEvent::Typed(drives::confirm_text(&nvme)), t);
        ev(&mut u, UpgradeEvent::ShowAll(false), t);
        assert!(u.drives.is_empty(), "the Show all list is gone");
        assert_eq!(u.summary(), None);
        assert_eq!(ev(&mut u, UpgradeEvent::Confirm, t), None);
    }

    #[test]
    fn refused_without_a_reason() {
        let (mut u, t) = running_fresh(true);
        u.message(result(false, ""), t);
        u.ended(Ok(Some(2)));
        assert_eq!(u.outcome_text(), ("Nothing was written.".to_string(), Tone::Warn));
    }

    #[test]
    fn open_checks_once() {
        let mut u = Upgrade::new();
        let t = Instant::now();
        assert_eq!(u.open(t), Some(Request::Check { source: None }));
        assert!(u.checking);
        assert_eq!(u.open(t), None);
        u.released(false, Ok(release()));
        assert_eq!(u.release.latest.as_deref(), Some("4.1.0"));
        assert_eq!(u.open(t), Some(Request::Check { source: None }));
    }
}
