//! The monitor part of the Screen tab: the arrangement being edited, the
//! Apply with its 15-second question, and which monitor shows the menu.
//!
//! Nothing here is slow and nothing touches a file: what has to be done
//! comes back as a `MonRequest`, which the menu runs as a job, and the
//! job's result comes back through `loaded`, `applied`, `kept`, `reverted`
//! and `taken_over`. kanshi sets the monitors; this only writes its file.

use crate::heads::{Head, Mode, Rotation};
use crate::kanshi::{self, Place, Profile, Side};
use crate::owned::Owner;
use std::time::{Duration, Instant};

/// How long *Keep this arrangement?* waits before it goes back.
pub const KEEP_SECS: u64 = 15;

/// What the person did with the monitors. `String` is a monitor's criteria.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonEvent {
    Select(String),
    Enabled(bool),
    /// An index into the selected monitor's `Head::modes`.
    Mode(usize),
    Rotation(Rotation),
    /// Put the selected monitor beside the one with these criteria.
    Place(Side, String),
    /// *Show the menu here* on the selected monitor.
    MenuHere,
    Apply,
    Keep,
    GoBack,
    /// *Let the menu manage it* for a file written by hand.
    TakeOver,
}

/// The work the tab asks for; the menu turns each into a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonRequest {
    /// Read `~/.config/kanshi/config`.
    Load,
    /// Write `text` as the file's body and let kanshi apply it.
    /// `menu_output` is the connector name the arrangement puts the menu on.
    Apply { text: String, menu_output: Option<String> },
    /// The person kept the new arrangement.
    Keep,
    /// Put the arrangement before the Apply back.
    Revert,
    /// Move a hand-written file aside.
    TakeOver,
}

/// What the tab waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The first monitors have not arrived yet.
    Start,
    Loading,
    Idle,
    /// The Apply job runs; `answer` is what came before it ended.
    Applying {
        deadline: Instant,
        answer: Option<Answer>,
    },
    /// Applied; *Keep this arrangement?* counts down.
    Asking {
        deadline: Instant,
    },
    Keeping,
    Reverting,
    TakingOver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Keep,
    GoBack,
}

#[derive(Debug, Clone)]
pub struct MonTab {
    heads: Vec<Head>,
    /// The file as loaded: who wrote it and its profiles.
    owner: Owner,
    /// The file could not be read: nothing is written.
    unreadable: bool,
    profiles: Vec<Profile>,
    /// What the file holds once the Apply is kept.
    pending: Option<Vec<Profile>>,
    /// The arrangement on screen in the tab, for the connected monitors.
    draft: Option<Profile>,
    /// Sorted criteria of the monitors `draft` was made for.
    draft_for: Vec<String>,
    selected: Option<String>,
    error: Option<String>,
    phase: Phase,
    thinlinc: bool,
}

impl Default for MonTab {
    fn default() -> MonTab {
        MonTab {
            heads: Vec::new(),
            owner: Owner::Missing,
            unreadable: false,
            profiles: Vec::new(),
            pending: None,
            draft: None,
            draft_for: Vec::new(),
            selected: None,
            error: None,
            phase: Phase::Start,
            thinlinc: false,
        }
    }
}

/// The mode a monitor shows when it is switched on: its current one, else
/// its preferred one, else its first.
fn head_mode(h: &Head) -> Option<Mode> {
    h.current
        .and_then(|i| h.modes.get(i))
        .or_else(|| h.modes.iter().find(|m| m.preferred))
        .or_else(|| h.modes.first())
        .cloned()
}

fn same_mode(a: &Mode, b: &Mode) -> bool {
    (a.w, a.h, a.refresh_mhz) == (b.w, b.h, b.refresh_mhz)
}

impl MonTab {
    // ---- What the menu feeds in ----

    /// The compositor reported the monitors. The first report asks for the
    /// file; a changed set of monitors shows the arrangement saved for the
    /// new set, else the one on screen. During the question nothing but
    /// the monitors changes: the countdown goes on.
    pub fn heads_changed(&mut self, heads: Vec<Head>) -> Option<MonRequest> {
        self.heads = heads;
        if self.phase == Phase::Start {
            self.phase = Phase::Loading;
            return Some(MonRequest::Load);
        }
        if self.phase != Phase::Loading && !self.in_question() && self.connected() != self.draft_for {
            self.rebuild();
        }
        None
    }

    /// The file was read: `text` is its content, `None` when missing.
    pub fn loaded(&mut self, text: Result<Option<String>, String>, owner: Owner) {
        self.phase = Phase::Idle;
        self.profiles.clear();
        match text {
            Err(e) => {
                self.unreadable = true;
                self.owner = owner;
                self.error = Some(e);
            }
            Ok(text) => {
                self.unreadable = false;
                self.owner = owner;
                if owner == Owner::Menu
                    && let Some(t) = text.as_deref()
                {
                    match kanshi::parse(t) {
                        Ok(p) => self.profiles = p,
                        Err(e) => self.error = Some(format!("The saved arrangement could not be read: {e}")),
                    }
                }
            }
        }
        self.rebuild();
    }

    /// The Apply job ended.
    pub fn applied(&mut self, r: Result<(), String>) -> Option<MonRequest> {
        let Phase::Applying { deadline, answer } = self.phase else {
            return None;
        };
        if let Err(e) = r {
            self.phase = Phase::Idle;
            self.pending = None;
            self.error = Some(format!("The arrangement was not applied: {e}"));
            return None;
        }
        match answer {
            Some(Answer::Keep) => {
                self.phase = Phase::Keeping;
                Some(MonRequest::Keep)
            }
            Some(Answer::GoBack) => {
                self.phase = Phase::Reverting;
                Some(MonRequest::Revert)
            }
            None => {
                self.phase = Phase::Asking { deadline };
                None
            }
        }
    }

    /// The new arrangement is the saved one now.
    pub fn kept(&mut self) {
        if self.phase != Phase::Keeping {
            return;
        }
        if let Some(p) = self.pending.take() {
            self.profiles = p;
        }
        if self.owner == Owner::Missing {
            self.owner = Owner::Menu;
        }
        self.phase = Phase::Idle;
        self.rebuild();
    }

    /// The old arrangement is back. When that failed, nobody knows what the
    /// file holds now: it is read again.
    pub fn reverted(&mut self, r: Result<(), String>) -> Option<MonRequest> {
        if self.phase != Phase::Reverting {
            return None;
        }
        self.pending = None;
        match r {
            Ok(()) => {
                self.phase = Phase::Idle;
                self.rebuild();
                None
            }
            Err(e) => {
                self.error = Some(format!("The previous arrangement could not be put back: {e}"));
                self.phase = Phase::Loading;
                Some(MonRequest::Load)
            }
        }
    }

    /// The hand-written file was moved aside to `config.by-hand`: there is
    /// none now, and the tab starts from the monitors as they are. Returns
    /// whether the tab waited for this.
    pub fn taken_over(&mut self, r: Result<(), String>) -> bool {
        if self.phase != Phase::TakingOver {
            return false;
        }
        self.phase = Phase::Idle;
        match r {
            Ok(()) => {
                self.error = None;
                self.owner = Owner::Missing;
                self.profiles.clear();
                self.rebuild();
            }
            Err(e) => self.error = Some(format!("Could not take over the file: {e}")),
        }
        true
    }

    /// Whether a ThinLinc client runs: it takes a new arrangement only
    /// from its next start.
    pub fn set_thinlinc(&mut self, on: bool) {
        self.thinlinc = on;
    }

    // ---- What the person does ----

    pub fn event(&mut self, e: MonEvent, now: Instant) -> Option<MonRequest> {
        match e {
            MonEvent::Select(c) => {
                if self
                    .draft
                    .as_ref()
                    .is_some_and(|d| d.outputs.iter().any(|o| o.criteria == c))
                {
                    self.selected = Some(c);
                }
                None
            }
            MonEvent::Keep => self.answer(Answer::Keep),
            MonEvent::GoBack => self.answer(Answer::GoBack),
            MonEvent::TakeOver => {
                if self.phase != Phase::Idle || self.owner != Owner::ByHand || self.unreadable {
                    return None;
                }
                self.phase = Phase::TakingOver;
                Some(MonRequest::TakeOver)
            }
            MonEvent::Apply => {
                if !self.editable() {
                    return None;
                }
                self.apply(now)
            }
            edit => {
                if !self.editable() {
                    return None;
                }
                self.edit(edit);
                None
            }
        }
    }

    /// The countdown: at the deadline, without an answer, go back.
    pub fn tick(&mut self, now: Instant) -> Option<MonRequest> {
        match self.phase {
            Phase::Asking { deadline } if now >= deadline => {
                self.phase = Phase::Reverting;
                Some(MonRequest::Revert)
            }
            // The Apply job is slow: go back as soon as it ends.
            Phase::Applying { deadline, answer: None } if now >= deadline => {
                self.phase = Phase::Applying {
                    deadline,
                    answer: Some(Answer::GoBack),
                };
                None
            }
            _ => None,
        }
    }

    fn answer(&mut self, a: Answer) -> Option<MonRequest> {
        match self.phase {
            Phase::Applying { deadline, answer: None } => {
                self.phase = Phase::Applying {
                    deadline,
                    answer: Some(a),
                };
                None
            }
            Phase::Asking { .. } => match a {
                Answer::Keep => {
                    self.phase = Phase::Keeping;
                    Some(MonRequest::Keep)
                }
                Answer::GoBack => {
                    self.phase = Phase::Reverting;
                    Some(MonRequest::Revert)
                }
            },
            _ => None,
        }
    }

    fn edit(&mut self, e: MonEvent) {
        let Some(sel) = self.selected.clone() else { return };
        let head_mode_of = |tab: &MonTab, i: Option<usize>| {
            let h = tab.head_named(&sel)?;
            match i {
                Some(i) => h.modes.get(i).cloned(),
                None => head_mode(h),
            }
        };
        let changed = match e {
            MonEvent::Enabled(false) => {
                if !self.off_allowed() {
                    return;
                }
                self.output_mut(&sel).map(|o| o.enabled = false).is_some()
            }
            MonEvent::Enabled(true) => {
                let fallback = head_mode_of(self, None);
                self.output_mut(&sel)
                    .map(|o| {
                        o.enabled = true;
                        // A monitor that was off has no mode in the file.
                        if (o.mode.w == 0 || o.mode.h == 0)
                            && let Some(m) = fallback
                        {
                            o.mode = m;
                        }
                    })
                    .is_some()
            }
            MonEvent::Mode(i) => {
                let Some(m) = head_mode_of(self, Some(i)) else { return };
                self.output_mut(&sel).map(|o| o.mode = m).is_some()
            }
            MonEvent::Rotation(r) => self.output_mut(&sel).map(|o| o.rotation = r).is_some(),
            MonEvent::Place(side, to) => {
                let known = self
                    .draft
                    .as_ref()
                    .is_some_and(|d| d.outputs.iter().any(|o| o.criteria == to));
                if to == sel || !known {
                    return;
                }
                if let Some(d) = self.draft.as_mut() {
                    kanshi::set_place(d, &sel, Place::Next { to, side });
                }
                true
            }
            MonEvent::MenuHere => {
                if !self.output(&sel).is_some_and(|o| o.enabled) {
                    return;
                }
                if let Some(d) = self.draft.as_mut() {
                    d.menu_output = Some(sel.clone());
                }
                true
            }
            _ => false,
        };
        if changed {
            self.error = None;
        }
    }

    fn apply(&mut self, now: Instant) -> Option<MonRequest> {
        let draft = self.draft.clone()?;
        if let Err(e) = kanshi::positions(&draft) {
            self.error = Some(e);
            return None;
        }
        let mut profiles = self.profiles.clone();
        match kanshi::profile_for(&profiles, &self.heads) {
            Some(i) => profiles[i] = draft.clone(),
            None => profiles.push(draft.clone()),
        }
        let text = format!("\n{}", kanshi::render(&profiles));
        self.pending = Some(profiles);
        self.error = None;
        self.phase = Phase::Applying {
            deadline: now + Duration::from_secs(KEEP_SECS),
            answer: None,
        };
        let menu_output = draft
            .menu_output
            .as_deref()
            .and_then(|c| self.connector(c))
            .map(String::from);
        Some(MonRequest::Apply { text, menu_output })
    }

    // ---- Bookkeeping ----

    fn in_question(&self) -> bool {
        matches!(
            self.phase,
            Phase::Applying { .. } | Phase::Asking { .. } | Phase::Keeping | Phase::Reverting
        )
    }

    fn connected(&self) -> Vec<String> {
        let mut v: Vec<String> = self.heads.iter().map(|h| kanshi::criteria(h, &self.heads)).collect();
        v.sort();
        v
    }

    /// The profiles that are in effect: the applied ones while the question
    /// runs, else the saved ones.
    fn in_effect(&self) -> &[Profile] {
        match (&self.pending, self.in_question()) {
            (Some(p), true) => p,
            _ => &self.profiles,
        }
    }

    /// Show the arrangement saved for the connected monitors, else the one
    /// on screen, and keep the selection on a monitor that is in it.
    fn rebuild(&mut self) {
        let draft = match kanshi::profile_for(&self.profiles, &self.heads) {
            Some(i) => self.profiles[i].clone(),
            None => kanshi::from_heads(&self.heads),
        };
        let keep = self
            .selected
            .as_ref()
            .is_some_and(|s| draft.outputs.iter().any(|o| &o.criteria == s));
        if !keep {
            self.selected = draft.outputs.first().map(|o| o.criteria.clone());
        }
        self.draft = Some(draft);
        self.draft_for = self.connected();
    }

    fn output(&self, criteria: &str) -> Option<&kanshi::OutputSet> {
        self.draft.as_ref()?.outputs.iter().find(|o| o.criteria == criteria)
    }

    fn output_mut(&mut self, criteria: &str) -> Option<&mut kanshi::OutputSet> {
        self.draft.as_mut()?.outputs.iter_mut().find(|o| o.criteria == criteria)
    }

    /// The connected monitor with these criteria.
    fn head_named(&self, criteria: &str) -> Option<&Head> {
        self.heads.iter().find(|h| kanshi::criteria(h, &self.heads) == criteria)
    }

    /// The connector name of the switched-on monitor with these criteria.
    fn connector(&self, criteria: &str) -> Option<&str> {
        self.head_named(criteria).filter(|h| h.enabled).map(|h| h.name.as_str())
    }

    // ---- What the screen shows ----

    /// The connector name of the monitor the menu should be on now: the
    /// choice of the arrangement in effect, when that monitor is connected
    /// and on. `None`: labwc places the menu.
    pub fn menu_output(&self) -> Option<&str> {
        let saved = self.in_effect();
        let p = &saved[kanshi::profile_for(saved, &self.heads)?];
        self.connector(p.menu_output.as_deref()?)
    }

    /// Whole seconds left on *Keep this arrangement?*; `None` when it is
    /// not up.
    pub fn countdown(&self, now: Instant) -> Option<u32> {
        let deadline = match self.phase {
            Phase::Applying { deadline, answer: None } | Phase::Asking { deadline } => deadline,
            _ => return None,
        };
        let left = deadline.saturating_duration_since(now).as_millis();
        Some(left.div_ceil(1000) as u32)
    }

    /// How long the loop may sleep before the countdown shows another
    /// number or ends.
    pub fn next_wake(&self, now: Instant) -> Option<Duration> {
        let deadline = match self.phase {
            Phase::Applying { deadline, answer: None } | Phase::Asking { deadline } => deadline,
            _ => return None,
        };
        let left = deadline.saturating_duration_since(now).as_millis() as u64;
        let step = match left % 1000 {
            0 => 1000,
            r => r,
        };
        Some(Duration::from_millis(step.min(left.max(1))))
    }

    /// The arrangement the tab shows.
    pub fn layout(&self) -> Option<&Profile> {
        self.draft.as_ref()
    }

    pub fn heads(&self) -> &[Head] {
        &self.heads
    }

    /// The connected monitor with these criteria.
    pub fn head(&self, criteria: &str) -> Option<&Head> {
        self.head_named(criteria)
    }

    /// Criteria of the selected monitor.
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The selected monitor's mode, as an index into its `Head::modes`.
    pub fn selected_mode(&self) -> Option<usize> {
        let sel = self.selected.as_deref()?;
        let o = self.output(sel)?;
        self.head_named(sel)?.modes.iter().position(|m| same_mode(m, &o.mode))
    }

    /// The selected monitor may be switched off: it is on, it does not
    /// carry the menu, and another one stays on.
    pub fn off_allowed(&self) -> bool {
        let (Some(sel), Some(d)) = (self.selected.as_deref(), self.draft.as_ref()) else {
            return false;
        };
        if !self.editable() {
            return false;
        }
        let on = d.outputs.iter().filter(|o| o.enabled).count();
        self.output(sel).is_some_and(|o| o.enabled) && d.menu_output.as_deref() != Some(sel) && on > 1
    }

    /// Criteria of the monitor the arrangement in the tab puts the menu on.
    pub fn menu_choice(&self) -> Option<&str> {
        self.draft.as_ref()?.menu_output.as_deref()
    }

    /// The file is the menu's, or there is none: the arrangement can be
    /// changed, unless a job runs or the question is up.
    pub fn editable(&self) -> bool {
        self.phase == Phase::Idle && !self.unreadable && self.owner != Owner::ByHand && self.draft.is_some()
    }

    /// Who wrote `~/.config/kanshi/config`.
    pub fn owner(&self) -> Owner {
        self.owner
    }

    /// A job for the tab runs.
    pub fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle | Phase::Asking { .. })
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Say that ThinLinc uses the new arrangement from its next start.
    pub fn thinlinc_note(&self) -> bool {
        self.thinlinc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kanshi::OutputSet;
    use crate::owned::HEADER;

    fn mode(w: u32, h: u32, preferred: bool) -> Mode {
        Mode {
            w,
            h,
            refresh_mhz: 60_000,
            preferred,
        }
    }

    /// A monitor without a serial: its criteria are its name.
    fn head(name: &str, x: i32, w: u32, h: u32) -> Head {
        Head {
            name: name.into(),
            make: "Dell".into(),
            model: "U".into(),
            serial: String::new(),
            phys_mm: (0, 0),
            enabled: true,
            modes: vec![mode(1280, 720, false), mode(w, h, true)],
            current: Some(1),
            pos: (x, 0),
            rotation: Rotation::Normal,
        }
    }

    fn two() -> Vec<Head> {
        vec![head("DP-1", 0, 1920, 1080), head("DP-2", 1920, 2560, 1440)]
    }

    fn set(c: &str, w: u32, h: u32, place: Place) -> OutputSet {
        // The file keeps no preferred flag.
        OutputSet {
            criteria: c.into(),
            enabled: true,
            mode: mode(w, h, false),
            rotation: Rotation::Normal,
            place,
        }
    }

    fn next(to: &str, side: Side) -> Place {
        Place::Next { to: to.into(), side }
    }

    /// DP-2 at the origin, DP-1 left of it, the menu on DP-2: not what
    /// `from_heads` makes of `two()`.
    fn saved_two() -> Profile {
        Profile {
            outputs: vec![
                set("DP-2", 2560, 1440, Place::Origin),
                set("DP-1", 1920, 1080, next("DP-2", Side::LeftOf)),
            ],
            menu_output: Some("DP-2".into()),
        }
    }

    fn file(profiles: &[Profile]) -> String {
        format!("{HEADER}\n\n{}", kanshi::render(profiles))
    }

    fn tab(heads: Vec<Head>, text: Option<String>) -> MonTab {
        let mut t = MonTab::default();
        assert_eq!(t.heads_changed(heads), Some(MonRequest::Load));
        let owner = crate::owned::owner(text.as_deref());
        t.loaded(Ok(text), owner);
        t
    }

    fn ev(t: &mut MonTab, e: MonEvent, now: Instant) -> Option<MonRequest> {
        t.event(e, now)
    }

    fn output<'a>(t: &'a MonTab, c: &str) -> &'a OutputSet {
        t.layout().unwrap().outputs.iter().find(|o| o.criteria == c).unwrap()
    }

    /// Apply and let the job succeed.
    fn applied_tab(t: &mut MonTab, now: Instant) {
        assert!(matches!(ev(t, MonEvent::Apply, now), Some(MonRequest::Apply { .. })));
        assert_eq!(t.applied(Ok(())), None);
    }

    #[test]
    fn first_heads_choose_the_saved_profile_else_the_current_layout() {
        let mut t = tab(two(), Some(file(&[saved_two()])));
        assert_eq!(t.layout(), Some(&saved_two()));
        assert_eq!(t.selected(), Some("DP-2"));
        assert_eq!(t.menu_output(), Some("DP-2"));
        // Later reports ask for nothing more.
        assert_eq!(t.heads_changed(two()), None);
        assert_eq!(t.layout(), Some(&saved_two()));

        let only_dp1 = Profile {
            outputs: vec![set("DP-1", 1920, 1080, Place::Origin)],
            menu_output: None,
        };
        let t = tab(two(), Some(file(&[only_dp1])));
        assert_eq!(t.layout(), Some(&kanshi::from_heads(&two())));
        assert_eq!(t.menu_output(), None);
        let t = tab(two(), None);
        assert_eq!(t.layout(), Some(&kanshi::from_heads(&two())));
        assert!(t.editable());
    }

    #[test]
    fn the_menu_monitor_cannot_be_switched_off() {
        let now = Instant::now();
        let mut t = tab(two(), Some(file(&[saved_two()])));
        ev(&mut t, MonEvent::Select("DP-2".into()), now);
        assert!(!t.off_allowed());
        assert_eq!(ev(&mut t, MonEvent::Enabled(false), now), None);
        assert!(output(&t, "DP-2").enabled);
        ev(&mut t, MonEvent::Select("DP-1".into()), now);
        assert!(t.off_allowed());
        ev(&mut t, MonEvent::Enabled(false), now);
        assert!(!output(&t, "DP-1").enabled);

        // The last monitor that is on, without a menu choice.
        let mut t = tab(two(), None);
        assert_eq!(t.menu_choice(), None);
        ev(&mut t, MonEvent::Select("DP-1".into()), now);
        assert!(t.off_allowed());
        ev(&mut t, MonEvent::Enabled(false), now);
        assert!(!output(&t, "DP-1").enabled);
        ev(&mut t, MonEvent::Select("DP-2".into()), now);
        assert!(!t.off_allowed());
        ev(&mut t, MonEvent::Enabled(false), now);
        assert!(output(&t, "DP-2").enabled);
        // An off monitor is not "switched off" again.
        ev(&mut t, MonEvent::Select("DP-1".into()), now);
        assert!(!t.off_allowed());
    }

    #[test]
    fn switching_on_takes_the_mode_from_the_monitor() {
        let now = Instant::now();
        let mut p = saved_two();
        p.outputs[1].enabled = false;
        let mut t = tab(two(), Some(file(&[p])));
        // Read back from the file, an off monitor has no mode.
        assert_eq!((output(&t, "DP-1").mode.w, output(&t, "DP-1").mode.h), (0, 0));
        ev(&mut t, MonEvent::Select("DP-1".into()), now);
        ev(&mut t, MonEvent::Enabled(true), now);
        let o = output(&t, "DP-1");
        assert!(o.enabled);
        assert_eq!((o.mode.w, o.mode.h), (1920, 1080), "the current mode of the monitor");
        assert_eq!(t.selected_mode(), Some(1));
        ev(&mut t, MonEvent::Mode(0), now);
        assert_eq!((output(&t, "DP-1").mode.w, output(&t, "DP-1").mode.h), (1280, 720));
        assert_eq!(t.selected_mode(), Some(0));
        // A mode the monitor does not offer changes nothing.
        ev(&mut t, MonEvent::Mode(7), now);
        assert_eq!(t.selected_mode(), Some(0));
    }

    #[test]
    fn apply_sends_the_rendered_text_and_starts_the_countdown() {
        let now = Instant::now();
        let only_dp1 = Profile {
            outputs: vec![set("DP-1", 1920, 1080, Place::Origin)],
            menu_output: None,
        };
        let mut t = tab(two(), Some(file(std::slice::from_ref(&only_dp1))));
        assert_eq!(t.countdown(now), None);
        ev(&mut t, MonEvent::Select("DP-2".into()), now);
        ev(&mut t, MonEvent::MenuHere, now);
        ev(&mut t, MonEvent::Rotation(Rotation::R90), now);
        let mut want = kanshi::from_heads(&two());
        want.outputs[1].rotation = Rotation::R90;
        want.menu_output = Some("DP-2".into());
        assert_eq!(t.layout(), Some(&want));
        // The profile of the other set of monitors stays in the file.
        let text = format!("\n{}", kanshi::render(&[only_dp1, want]));
        assert_eq!(
            ev(&mut t, MonEvent::Apply, now),
            Some(MonRequest::Apply {
                text,
                menu_output: Some("DP-2".into())
            })
        );
        assert_eq!(t.countdown(now), Some(15));
        assert_eq!(t.countdown(now + Duration::from_secs(10)), Some(5));
        assert_eq!(t.countdown(now + Duration::from_millis(14_500)), Some(1));
        // A second Apply while the first runs sends nothing.
        assert_eq!(ev(&mut t, MonEvent::Apply, now), None);
    }

    #[test]
    fn no_answer_reverts_after_15_seconds() {
        let now = Instant::now();
        let mut t = tab(two(), None);
        ev(&mut t, MonEvent::Select("DP-2".into()), now);
        ev(&mut t, MonEvent::Rotation(Rotation::R180), now);
        applied_tab(&mut t, now);
        assert_eq!(t.tick(now + Duration::from_secs(14)), None);
        assert_eq!(t.tick(now + Duration::from_secs(15)), Some(MonRequest::Revert));
        assert_eq!(t.tick(now + Duration::from_secs(16)), None, "once");
        assert_eq!(t.countdown(now + Duration::from_secs(15)), None);
        assert_eq!(t.reverted(Ok(())), None);
        assert_eq!(
            t.layout(),
            Some(&kanshi::from_heads(&two())),
            "the old arrangement shows again"
        );
        assert!(t.editable());

        // Keep before the deadline: no revert at all.
        let mut t = tab(two(), None);
        ev(&mut t, MonEvent::Select("DP-2".into()), now);
        ev(&mut t, MonEvent::Rotation(Rotation::R180), now);
        applied_tab(&mut t, now);
        assert_eq!(
            ev(&mut t, MonEvent::Keep, now + Duration::from_secs(5)),
            Some(MonRequest::Keep)
        );
        assert_eq!(t.tick(now + Duration::from_secs(20)), None);
        t.kept();
        assert_eq!(output(&t, "DP-2").rotation, Rotation::R180);
        assert_eq!(t.owner(), Owner::Menu);
        assert_eq!(t.countdown(now + Duration::from_secs(5)), None);

        // The Apply job outlives the deadline: go back once it ends.
        let mut t = tab(two(), None);
        assert!(ev(&mut t, MonEvent::Apply, now).is_some());
        assert_eq!(t.tick(now + Duration::from_secs(15)), None);
        assert_eq!(t.applied(Ok(())), Some(MonRequest::Revert));
    }

    #[test]
    fn a_failed_apply_keeps_the_draft_and_says_why() {
        let now = Instant::now();
        let mut t = tab(two(), None);
        ev(&mut t, MonEvent::Select("DP-2".into()), now);
        ev(&mut t, MonEvent::Rotation(Rotation::R90), now);
        assert!(ev(&mut t, MonEvent::Apply, now).is_some());
        assert_eq!(t.applied(Err("kanshi did not start".into())), None);
        assert_eq!(t.countdown(now), None);
        assert_eq!(t.tick(now + Duration::from_secs(20)), None);
        assert!(t.error().unwrap().contains("kanshi did not start"));
        assert_eq!(output(&t, "DP-2").rotation, Rotation::R90);
        assert!(t.editable());
    }

    #[test]
    fn hotplug_during_the_countdown_still_reverts() {
        let now = Instant::now();
        let mut t = tab(two(), None);
        ev(&mut t, MonEvent::Select("DP-2".into()), now);
        ev(&mut t, MonEvent::Rotation(Rotation::R90), now);
        applied_tab(&mut t, now);
        // DP-2, the selected one, is unplugged.
        assert_eq!(t.heads_changed(vec![head("DP-1", 0, 1920, 1080)]), None);
        assert_eq!(t.selected_mode(), None);
        assert!(!t.off_allowed());
        assert_eq!(t.menu_output(), None);
        assert_eq!(t.countdown(now + Duration::from_secs(5)), Some(10));
        assert_eq!(t.tick(now + Duration::from_secs(15)), Some(MonRequest::Revert));
        assert_eq!(t.reverted(Ok(())), None);
        assert_eq!(t.layout(), Some(&kanshi::from_heads(&[head("DP-1", 0, 1920, 1080)])));
        assert_eq!(t.selected(), Some("DP-1"));

        // Go back after a hotplug.
        let mut t = tab(two(), None);
        applied_tab(&mut t, now);
        t.heads_changed(vec![head("DP-1", 0, 1920, 1080)]);
        assert_eq!(ev(&mut t, MonEvent::GoBack, now), Some(MonRequest::Revert));
    }

    #[test]
    fn a_missing_menu_monitor_clears_the_choice_once() {
        let mut t = tab(two(), Some(file(&[saved_two()])));
        assert_eq!(t.menu_output(), Some("DP-2"));
        let alone = vec![head("DP-1", 0, 1920, 1080)];
        assert_eq!(t.heads_changed(alone.clone()), None);
        assert_eq!(t.menu_output(), None);
        let (layout, selected) = (t.layout().cloned(), t.selected().map(String::from));
        assert_eq!(t.heads_changed(alone), None);
        assert_eq!(t.menu_output(), None);
        assert_eq!(
            (t.layout().cloned(), t.selected().map(String::from)),
            (layout, selected)
        );
        // Back at the dock, the choice is there again.
        assert_eq!(t.heads_changed(two()), None);
        assert_eq!(t.menu_output(), Some("DP-2"));
        // A menu monitor that is connected but off carries no menu.
        let mut off = two();
        off[1].enabled = false;
        t.heads_changed(off);
        assert_eq!(t.menu_output(), None);
    }

    #[test]
    fn a_hand_file_is_read_only() {
        let now = Instant::now();
        let text = "profile {\n\toutput * enable\n}\n".to_string();
        let mut t = MonTab::default();
        t.heads_changed(two());
        t.loaded(Ok(Some(text)), Owner::ByHand);
        assert!(!t.editable());
        let before = t.layout().cloned();
        assert_eq!(before, Some(kanshi::from_heads(&two())), "the monitors still show");
        for e in [
            MonEvent::Select("DP-2".into()),
            MonEvent::Enabled(false),
            MonEvent::Mode(0),
            MonEvent::Rotation(Rotation::R90),
            MonEvent::Place(Side::LeftOf, "DP-1".into()),
            MonEvent::MenuHere,
            MonEvent::Apply,
            MonEvent::Keep,
            MonEvent::GoBack,
        ] {
            assert_eq!(ev(&mut t, e.clone(), now), None, "{e:?}");
        }
        assert_eq!(t.layout().cloned(), before);
        assert_eq!(t.selected(), Some("DP-2"), "selecting is only looking");
        assert_eq!(ev(&mut t, MonEvent::TakeOver, now), Some(MonRequest::TakeOver));
        assert_eq!(ev(&mut t, MonEvent::TakeOver, now), None, "once");
        assert!(t.taken_over(Ok(())));
        assert_eq!(t.owner(), Owner::Missing);
        assert!(t.editable());
        assert!(!t.taken_over(Ok(())), "nothing was asked");

        // A file that cannot be read is not written either, nor taken over.
        let mut t = MonTab::default();
        t.heads_changed(two());
        t.loaded(
            Err("Could not read ~/.config/kanshi/config: link".into()),
            Owner::ByHand,
        );
        assert!(!t.editable());
        assert_eq!(ev(&mut t, MonEvent::Apply, now), None);
        assert_eq!(ev(&mut t, MonEvent::TakeOver, now), None);
        assert!(t.error().unwrap().contains("Could not read"));
    }

    #[test]
    fn overlap_shows_an_error_and_sends_nothing() {
        let now = Instant::now();
        let three = vec![
            head("DP-1", 0, 1920, 1080),
            head("DP-2", 1920, 1920, 1080),
            head("DP-3", 3840, 1920, 1080),
        ];
        let mut t = tab(three, None);
        ev(&mut t, MonEvent::Select("DP-3".into()), now);
        ev(&mut t, MonEvent::Place(Side::RightOf, "DP-1".into()), now);
        assert_eq!(ev(&mut t, MonEvent::Apply, now), None);
        assert_eq!(t.error(), Some("DP-2 and DP-3 overlap."));
        assert_eq!(t.countdown(now), None);
        // The next change clears it.
        ev(&mut t, MonEvent::Place(Side::Below, "DP-1".into()), now);
        assert_eq!(t.error(), None);
        assert!(matches!(
            ev(&mut t, MonEvent::Apply, now),
            Some(MonRequest::Apply { .. })
        ));
    }

    #[test]
    fn a_place_beside_itself_or_an_unknown_monitor_is_ignored() {
        let now = Instant::now();
        let mut t = tab(two(), None);
        let before = t.layout().cloned();
        ev(&mut t, MonEvent::Select("DP-2".into()), now);
        ev(&mut t, MonEvent::Place(Side::Below, "DP-2".into()), now);
        ev(&mut t, MonEvent::Place(Side::Below, "HDMI-9".into()), now);
        ev(&mut t, MonEvent::Select("HDMI-9".into()), now);
        assert_eq!(t.layout().cloned(), before);
        assert_eq!(t.selected(), Some("DP-2"));
    }

    #[test]
    fn a_revert_that_fails_reads_the_file_again() {
        let now = Instant::now();
        let mut t = tab(two(), None);
        applied_tab(&mut t, now);
        assert_eq!(ev(&mut t, MonEvent::GoBack, now), Some(MonRequest::Revert));
        assert_eq!(t.reverted(Err("disk full".into())), Some(MonRequest::Load));
        assert!(t.error().unwrap().contains("disk full"));
    }

    #[test]
    fn the_wake_lands_on_the_next_whole_second() {
        let now = Instant::now();
        let mut t = tab(two(), None);
        assert_eq!(t.next_wake(now), None);
        applied_tab(&mut t, now);
        assert_eq!(t.next_wake(now), Some(Duration::from_secs(1)));
        assert_eq!(
            t.next_wake(now + Duration::from_millis(14_700)),
            Some(Duration::from_millis(300))
        );
        assert_eq!(
            t.next_wake(now + Duration::from_millis(9_250)),
            Some(Duration::from_millis(750))
        );
    }
}
