//! The Eject panel: one row per mounted stick or card, and Eject all. What
//! the buttons do lives in menu.rs; this file paints the panel and reports
//! the click.

use crate::brand;
use crate::drives;
use crate::menu::{EjectEvent, Menu};
use crate::ui::dialog;
use egui::Ui;
use shadcn_egui::components::button::{Button, ButtonVariant};

/// Draw the panel over the frozen grid.
pub fn show(ui: &Ui, menu: &Menu) -> Option<EjectEvent> {
    let mut chose = None;
    let idle = menu.eject.running.is_none();
    dialog(ui, "Eject a stick or card", |ui, t| {
        for stick in &menu.removable {
            ui.horizontal(|ui| {
                ui.label(stick.names.join(", "));
                ui.label(
                    egui::RichText::new(drives::format_size(stick.size_bytes)).color(t.palette().muted_foreground),
                );
                if !stick.model.is_empty() {
                    ui.label(egui::RichText::new(&stick.model).color(t.palette().muted_foreground));
                }
                if ui.add_enabled(idle, Button::new("Eject")).clicked() {
                    chose = Some(EjectEvent::One(stick.disk.clone()));
                }
            });
        }
        if menu.removable.len() > 1 && ui.add_enabled(idle, Button::new("Eject all")).clicked() {
            chose = Some(EjectEvent::All);
        }
        if let Some(target) = &menu.eject.running {
            let what = if target == "all" { "the sticks" } else { "the stick" };
            ui.label(egui::RichText::new(format!("Ejecting {what}...")).color(t.palette().muted_foreground));
        }
        if let Some(m) = &menu.eject.message {
            ui.label(egui::RichText::new(m).color(t.palette().foreground));
        }
        ui.add_space(brand::DIALOG_BUTTONS_GAP);
        if ui.add(Button::new("Close").variant(ButtonVariant::Secondary)).clicked() {
            chose = Some(EjectEvent::Close);
        }
    });
    chose
}
