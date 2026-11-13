//! The Keyboard tab's state: what is shown, what is applied, and the queue
//! that runs one apply at a time. The picking is in the UI; the apply job
//! itself is in `jobs.rs`.

use crate::config::{Compose, Keyboard, xkb_name_ok};
use crate::kbregistry::Registry;

/// What the person changed on the Keyboard tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KbEvent {
    Layout(String),
    Variant(String),
    CapsAsCtrl(bool),
    Compose(Compose),
    /// The test field: text typed to try the layout. Applies nothing.
    TryText(String),
}

/// How an apply took effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KbOutcome {
    /// The running compositor uses it now.
    Live,
    /// Saved; the compositor reads it at the next start.
    NextStart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardTab {
    /// What the pickers show: always the latest choice.
    pub shown: Keyboard,
    /// What the last successful apply wrote.
    pub applied: Keyboard,
    pub registry: Result<Registry, String>,
    pub applying: bool,
    /// The newest choice made while an apply ran.
    pending: Option<Keyboard>,
    pub note: Option<String>,
    pub try_text: String,
}

/// Whether `kb` may be applied. Names must be XKB names and the layout
/// not empty. With a registry the layout must be in it and the variant
/// empty or one of its variants. Without one, the caller (`event`) holds
/// the choice to the applied layout and variant.
pub fn validate(kb: &Keyboard, reg: Option<&Registry>) -> Result<(), String> {
    if kb.layout.is_empty() {
        return Err("Choose a layout.".into());
    }
    if !xkb_name_ok(&kb.layout) || !xkb_name_ok(&kb.variant) {
        return Err("The layout or variant name is not valid.".into());
    }
    let Some(reg) = reg else { return Ok(()) };
    let Some(layout) = reg.layout(&kb.layout) else {
        return Err(format!("The layout {} is not in the layout list.", kb.layout));
    };
    if !kb.variant.is_empty() && !layout.variants.iter().any(|v| v.name == kb.variant) {
        return Err(format!(
            "The variant {} does not belong to the layout {}.",
            kb.variant, kb.layout
        ));
    }
    Ok(())
}

impl KeyboardTab {
    pub fn new(kb: Keyboard, registry: Result<Registry, String>) -> KeyboardTab {
        let note = registry.as_ref().err().cloned();
        KeyboardTab {
            shown: kb.clone(),
            applied: kb,
            registry,
            applying: false,
            pending: None,
            note,
            try_text: String::new(),
        }
    }

    /// A change on the tab. `Some` is the choice to apply now.
    pub fn event(&mut self, e: KbEvent) -> Option<Keyboard> {
        let mut next = self.shown.clone();
        match e {
            KbEvent::TryText(t) => {
                self.try_text = t;
                return None;
            }
            KbEvent::Layout(l) => {
                next.layout = l;
                next.variant = String::new();
            }
            KbEvent::Variant(v) => next.variant = v,
            KbEvent::CapsAsCtrl(on) => next.caps_as_ctrl = on,
            KbEvent::Compose(c) => next.compose = c,
        }
        let checked = match &self.registry {
            Ok(reg) => validate(&next, Some(reg)),
            Err(_) => validate(&next, None).and_then(|()| {
                if next.layout == self.applied.layout && next.variant == self.applied.variant {
                    Ok(())
                } else {
                    Err("The layout list is missing: only the current layout can be used.".into())
                }
            }),
        };
        if let Err(msg) = checked {
            self.note = Some(msg);
            return None;
        }
        self.shown = next;
        if self.applying {
            self.pending = Some(self.shown.clone());
            return None;
        }
        if self.shown == self.applied {
            return None;
        }
        self.applying = true;
        Some(self.shown.clone())
    }

    /// The apply of `kb` ended. `Some` is the next one to start.
    pub fn done(&mut self, kb: &Keyboard, r: Result<KbOutcome, String>) -> Option<Keyboard> {
        self.applying = false;
        match r {
            Ok(outcome) => {
                self.applied = kb.clone();
                self.note = match outcome {
                    KbOutcome::Live => None,
                    KbOutcome::NextStart => Some("Takes effect at the next start.".into()),
                };
            }
            Err(e) => self.note = Some(e),
        }
        let next = self.pending.take().filter(|p| *p != self.applied)?;
        self.applying = true;
        Some(next)
    }

    /// `(name, label)` of every layout the picker offers.
    pub fn layouts(&self) -> Vec<(String, String)> {
        match &self.registry {
            Ok(reg) => reg
                .layouts
                .iter()
                .map(|l| (l.name.clone(), l.description.clone()))
                .collect(),
            Err(_) => vec![(self.applied.layout.clone(), self.applied.layout.clone())],
        }
    }

    /// `(name, label)` of the variants of the shown layout, the default
    /// (empty name) first.
    pub fn variants(&self) -> Vec<(String, String)> {
        let mut v = vec![(String::new(), "Default".to_string())];
        match &self.registry {
            Ok(reg) => {
                if let Some(l) = reg.layout(&self.shown.layout) {
                    v.extend(l.variants.iter().map(|x| (x.name.clone(), x.description.clone())));
                }
            }
            Err(_) => {
                if self.shown.layout == self.applied.layout && !self.applied.variant.is_empty() {
                    v.push((self.applied.variant.clone(), self.applied.variant.clone()));
                }
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kbregistry;

    fn reg() -> Registry {
        kbregistry::parse(include_str!("testdata/evdev-small.xml")).unwrap()
    }

    fn kb(layout: &str, variant: &str) -> Keyboard {
        Keyboard {
            layout: layout.into(),
            variant: variant.into(),
            ..Keyboard::default()
        }
    }

    fn tab(layout: &str, variant: &str) -> KeyboardTab {
        KeyboardTab::new(kb(layout, variant), Ok(reg()))
    }

    #[test]
    fn a_layout_change_resets_the_variant() {
        let mut t = tab("ch", "de_nodeadkeys");
        let k = t.event(KbEvent::Layout("de".into())).unwrap();
        assert_eq!(k.variant, "");
        assert_eq!(t.shown.variant, "");
        assert_eq!(t.shown.layout, "de");
    }

    #[test]
    fn every_change_starts_an_apply() {
        let mut t = tab("us", "");
        let k = t.event(KbEvent::CapsAsCtrl(false)).unwrap();
        assert!(!k.caps_as_ctrl);
        assert!(t.applying);
        // Back to what is applied starts nothing when idle.
        let mut t = tab("us", "");
        assert_eq!(t.event(KbEvent::CapsAsCtrl(true)), None);
        assert!(!t.applying);
    }

    #[test]
    fn a_change_during_an_apply_waits_and_merges() {
        let mut t = tab("us", "");
        let first = t.event(KbEvent::Layout("de".into())).unwrap();
        assert_eq!(t.event(KbEvent::CapsAsCtrl(false)), None);
        assert_eq!(t.event(KbEvent::Compose(Compose::Menu)), None);
        let next = t.done(&first, Ok(KbOutcome::Live)).unwrap();
        assert_eq!(next.layout, "de");
        assert!(!next.caps_as_ctrl);
        assert_eq!(next.compose, Compose::Menu);
        assert!(t.applying);
        assert_eq!(t.done(&next, Ok(KbOutcome::Live)), None);
        assert!(!t.applying);
    }

    #[test]
    fn an_old_result_cannot_overwrite_a_newer_choice() {
        let mut t = tab("us", "");
        let first = t.event(KbEvent::Layout("de".into())).unwrap();
        t.event(KbEvent::CapsAsCtrl(false));
        let third = t.event(KbEvent::Compose(Compose::Menu));
        assert_eq!(third, None);
        let second = t.done(&first, Ok(KbOutcome::Live)).unwrap();
        assert_eq!(t.shown, second);
        assert_eq!(t.applied.layout, "de");
        assert!(t.applied.caps_as_ctrl);
        assert_eq!(t.shown.compose, Compose::Menu);
        assert!(!t.shown.caps_as_ctrl);
    }

    #[test]
    fn a_pending_choice_equal_to_the_applied_one_is_dropped() {
        let mut t = tab("us", "");
        let first = t.event(KbEvent::CapsAsCtrl(false)).unwrap();
        assert_eq!(t.event(KbEvent::CapsAsCtrl(true)), None);
        assert_eq!(t.done(&first, Ok(KbOutcome::Live)), Some(kb("us", "")));
        // The applied caps is now false; pending (true) differs, so it ran.
        let mut t = tab("us", "");
        let first = t.event(KbEvent::CapsAsCtrl(false)).unwrap();
        t.event(KbEvent::CapsAsCtrl(false));
        assert_eq!(t.done(&first, Ok(KbOutcome::Live)), None);
    }

    #[test]
    fn a_failed_apply_keeps_the_applied_state_and_says_why() {
        let mut t = tab("us", "");
        let first = t.event(KbEvent::Layout("de".into())).unwrap();
        assert_eq!(t.done(&first, Err("disk full".into())), None);
        assert_eq!(t.applied.layout, "us");
        assert_eq!(t.note.as_deref(), Some("disk full"));
        assert!(!t.applying);
    }

    #[test]
    fn validation_refuses_a_foreign_variant() {
        let r = reg();
        assert!(validate(&kb("us", "de_nodeadkeys"), Some(&r)).is_err());
        assert!(validate(&kb("ch", ""), Some(&r)).is_ok());
        assert!(validate(&kb("ch", "fr"), Some(&r)).is_ok());
        assert!(validate(&kb("zz", ""), Some(&r)).is_err());
        assert!(validate(&kb("c h", ""), Some(&r)).is_err());
    }

    #[test]
    fn validation_refuses_an_empty_layout() {
        assert!(validate(&kb("", ""), None).is_err());
        assert!(validate(&kb("", ""), Some(&reg())).is_err());
        assert!(validate(&kb("us", ""), None).is_ok());
    }

    #[test]
    fn a_refused_change_sets_a_note_and_starts_nothing() {
        let mut t = tab("us", "");
        assert_eq!(t.event(KbEvent::Variant("de_nodeadkeys".into())), None);
        assert!(t.note.is_some());
        assert!(!t.applying);
        assert_eq!(t.shown, t.applied);
        assert_eq!(t.event(KbEvent::Layout(String::new())), None);
        assert_eq!(t.shown, t.applied);
    }

    #[test]
    fn without_a_registry_only_the_current_layout_is_offered() {
        let e = "/usr/share/X11/xkb/rules/evdev.xml: missing".to_string();
        let mut t = KeyboardTab::new(kb("ch", "fr"), Err(e.clone()));
        assert_eq!(t.layouts(), [("ch".to_string(), "ch".to_string())]);
        assert_eq!(t.note, Some(e));
        assert_eq!(t.variants()[0], (String::new(), "Default".to_string()));
        assert_eq!(t.variants().len(), 2);
        // Another layout is refused, a flag still goes through.
        assert_eq!(t.event(KbEvent::Layout("de".into())), None);
        assert_eq!(t.shown.layout, "ch");
        let k = t.event(KbEvent::CapsAsCtrl(false)).unwrap();
        assert_eq!((k.layout.as_str(), k.variant.as_str()), ("ch", "fr"));
    }

    #[test]
    fn the_pickers_list_the_registry() {
        let t = tab("ch", "");
        let names: Vec<String> = t.layouts().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["us", "de", "ch"]);
        let v: Vec<String> = t.variants().into_iter().map(|(n, _)| n).collect();
        assert_eq!(v, ["", "fr", "de_nodeadkeys"]);
    }

    #[test]
    fn next_start_says_so() {
        let mut t = tab("us", "");
        let k = t.event(KbEvent::CapsAsCtrl(false)).unwrap();
        t.done(&k, Ok(KbOutcome::NextStart));
        assert_eq!(t.note.as_deref(), Some("Takes effect at the next start."));
        let k = t.event(KbEvent::CapsAsCtrl(true)).unwrap();
        t.done(&k, Ok(KbOutcome::Live));
        assert_eq!(t.note, None);
    }

    #[test]
    fn try_text_starts_nothing() {
        let mut t = tab("us", "");
        assert_eq!(t.event(KbEvent::TryText("äöü".into())), None);
        assert_eq!(t.try_text, "äöü");
        assert!(!t.applying);
        assert_eq!(t.shown, t.applied);
    }
}
