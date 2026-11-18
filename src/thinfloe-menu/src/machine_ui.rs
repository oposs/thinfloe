//! The This machine tab: one band of label and value rows, and *Copy*.

use crate::brand::{self, role};
use crate::menu::Menu;
use crate::settings::SettingsEvent;
use crate::ui::{GAP, ROW_LEFT, ROW_MARGIN_X, ROW_RIGHT};
use egui::{RichText, Ui};
use shadcn_egui::Theme;
use shadcn_egui::components::button::{Button, ButtonVariant};
use std::time::Instant;

/// The tab's body; `MachineCopy` when *Copy* was clicked.
pub fn machine_tab(ui: &mut Ui, menu: &Menu, width: f32, now: Instant) -> Option<SettingsEvent> {
    let t = Theme::current(ui.ctx());
    let tab = &menu.machine;
    let rows = menu.machine_rows(now);
    let mut event = None;
    egui::ScrollArea::vertical()
        .id_salt("machine")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let inner_w = ui.available_width().min(width) - ROW_MARGIN_X;
            let muted = |ui: &mut Ui, text: &str| {
                ui.add(egui::Label::new(RichText::new(text).color(t.palette().muted_foreground)).truncate());
            };
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
                    if tab.facts().is_none() {
                        muted(ui, "Reading…");
                        return;
                    }
                    // As the Screen tab's rows: the label in the form's
                    // column, its value muted beside it.
                    for (label, value) in &rows {
                        ui.horizontal(|ui| {
                            ui.scope(|ui| {
                                ui.set_min_width(brand::FORM_LABEL_W);
                                ui.set_max_width(brand::FORM_LABEL_W);
                                ui.label(RichText::new(label).color(t.palette().foreground));
                            });
                            muted(ui, value);
                        });
                    }
                    ui.add_space(GAP / 2.0);
                    ui.horizontal(|ui| {
                        let copy = Button::new("Copy").variant(ButtonVariant::Secondary);
                        // Off while a copy runs: a second click would start a second job.
                        if ui.add_enabled(!tab.copying(), copy).clicked() {
                            event = Some(SettingsEvent::MachineCopy);
                        }
                        match &tab.copied {
                            Some(Ok(())) => muted(ui, "Copied"),
                            Some(Err(e)) => {
                                ui.add(egui::Label::new(RichText::new(e).color(t.role(role::WARN))).truncate());
                            }
                            None => {}
                        }
                    });
                });
        });
    event
}
