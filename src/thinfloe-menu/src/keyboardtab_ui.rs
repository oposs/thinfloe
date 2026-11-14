//! The Keyboard tab: layout, variant, CapsLock, compose key and a test field.

use crate::brand;
use crate::config::Compose;
use crate::keyboard::{KbEvent, KeyboardTab};
use crate::ui::{GAP, ROW_LEFT, ROW_MARGIN_X, ROW_RIGHT};
use egui::{RichText, Ui};
use shadcn_egui::Theme;
use shadcn_egui::components::input::Input;
use shadcn_egui::components::select::Select;
use shadcn_egui::components::switch::Switch;

/// The tab's body; the change the person made, if any. The controls stay
/// enabled while an apply runs: the tab queues the changes.
pub fn keyboard_tab(ui: &mut Ui, tab: &KeyboardTab, width: f32) -> Option<KbEvent> {
    let t = Theme::current(ui.ctx());
    let mut event = None;
    egui::ScrollArea::vertical()
        .id_salt("keyboard")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let inner_w = ui.available_width().min(width) - ROW_MARGIN_X;
            let card = |ui: &mut Ui, add: &mut dyn FnMut(&mut Ui)| {
                egui::Frame::new()
                    .fill(t.palette().card)
                    .inner_margin(egui::Margin {
                        left: ROW_LEFT,
                        right: ROW_RIGHT,
                        top: 12,
                        bottom: 12,
                    })
                    .show(ui, |ui| {
                        ui.set_width(inner_w);
                        add(ui);
                    });
            };
            card(ui, &mut |ui| {
                if let Some(e) = pickers(ui, &t, tab) {
                    event = Some(e);
                }
            });
            // The only place a failed apply shows: directly under the card.
            if let Some(note) = &tab.note {
                ui.add_space(GAP / 2.0);
                ui.add(egui::Label::new(RichText::new(note).color(t.palette().muted_foreground)).truncate());
            }
            ui.add_space(GAP);
            card(ui, &mut |ui| {
                let mut text = tab.try_text.clone();
                row(ui, &t, "Try it here", |ui| {
                    let r = ui.add(Input::new(&mut text).max_width(brand::NET_FIELD_W).id_salt("tf-kb-try"));
                    if r.changed() {
                        event = Some(KbEvent::TryText(text.clone()));
                    }
                });
            });
        });
    event
}

/// A row: the label in its column, then what `add` draws.
fn row(ui: &mut Ui, t: &Theme, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.set_min_width(brand::FORM_LABEL_W);
            ui.set_max_width(brand::FORM_LABEL_W);
            ui.label(RichText::new(label).color(t.palette().foreground));
        });
        add(ui);
    });
}

/// Card 1: the four settings.
fn pickers(ui: &mut Ui, t: &Theme, tab: &KeyboardTab) -> Option<KbEvent> {
    let mut event = None;

    let layouts = tab.layouts();
    let labels: Vec<&str> = layouts.iter().map(|(_, l)| l.as_str()).collect();
    let mut idx = layouts.iter().position(|(n, _)| *n == tab.shown.layout).unwrap_or(0);
    row(ui, t, "Layout", |ui| {
        if ui.add(Select::new("tf-kb-layout", &mut idx, &labels)).changed() {
            event = layouts.get(idx).map(|(n, _)| KbEvent::Layout(n.clone()));
        }
    });

    let variants = tab.variants();
    let labels: Vec<&str> = variants.iter().map(|(_, l)| l.as_str()).collect();
    let mut idx = variants.iter().position(|(n, _)| *n == tab.shown.variant).unwrap_or(0);
    row(ui, t, "Variant", |ui| {
        if ui.add(Select::new("tf-kb-variant", &mut idx, &labels)).changed() {
            event = variants.get(idx).map(|(n, _)| KbEvent::Variant(n.clone()));
        }
    });

    let mut caps = tab.shown.caps_as_ctrl;
    row(ui, t, "CapsLock acts as Ctrl", |ui| {
        if ui.add(Switch::new(&mut caps)).changed() {
            event = Some(KbEvent::CapsAsCtrl(caps));
        }
    });

    let labels: Vec<&str> = Compose::ALL.iter().map(|c| c.label()).collect();
    let mut idx = Compose::ALL.iter().position(|&c| c == tab.shown.compose).unwrap_or(0);
    row(ui, t, "Compose key", |ui| {
        if ui.add(Select::new("tf-kb-compose", &mut idx, &labels)).changed() {
            event = Compose::ALL.get(idx).map(|&c| KbEvent::Compose(c));
        }
    });
    event
}
