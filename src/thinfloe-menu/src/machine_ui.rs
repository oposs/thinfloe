//! The This machine tab: the machine's facts, the add-ons, the network and
//! the graphics as info panels, in two columns on a wide panel, and *Copy*.

use crate::brand::{self, role};
use crate::infopanel::{self, Panel};
use crate::machine::MachineTab;
use crate::menu::Menu;
use crate::settings::SettingsEvent;
use crate::ui::{GAP, TILE_GAP};
use egui::{RichText, Ui};
use shadcn_egui::Theme;
use shadcn_egui::components::button::{Button, ButtonVariant};
use std::time::Instant;

/// The panels of the left column; the others go right.
const LEFT: [&str; 2] = [crate::machine::TITLE, crate::machine::ADDONS_TITLE];

/// The tab's body; `MachineCopy` when *Copy* was clicked.
pub fn machine_tab(ui: &mut Ui, menu: &Menu, width: f32, now: Instant) -> Option<SettingsEvent> {
    let t = Theme::current(ui.ctx());
    let tab = &menu.machine;
    let panels = menu.machine_panels(now);
    let mut event = None;
    egui::ScrollArea::vertical()
        .id_salt("machine")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let w = ui.available_width().min(width);
            if w >= brand::INFO_TWO_COLUMNS_W {
                // The machine's facts are one block of known height, the
                // network grows with the interfaces: each column keeps its
                // panels together, and Copy stands under the short left
                // one, in view without a scroll.
                let (left, right): (Vec<&Panel>, Vec<&Panel>) = panels.iter().partition(|p| LEFT.contains(&p.title));
                let col_w = (w - brand::INFO_COL_GAP) / 2.0;
                // The gap between the columns only: inside them the
                // spacing stays as on the other tabs, where it sets the
                // gap between a panel's labels and values.
                let inner = ui.spacing().item_spacing.x;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = brand::INFO_COL_GAP;
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.x = inner;
                        ui.set_width(col_w);
                        bands(ui, &t, col_w, &left);
                        copy_row(ui, &t, tab, &mut event);
                    });
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.x = inner;
                        ui.set_width(col_w);
                        bands(ui, &t, col_w, &right);
                    });
                });
            } else {
                let all: Vec<&Panel> = panels.iter().collect();
                bands(ui, &t, w, &all);
                copy_row(ui, &t, tab, &mut event);
            }
        });
    event
}

/// *Copy* and how the last copy went.
fn copy_row(ui: &mut Ui, t: &Theme, tab: &MachineTab, event: &mut Option<SettingsEvent>) {
    ui.add_space(GAP / 2.0);
    ui.horizontal(|ui| {
        let copy = Button::new("Copy").variant(ButtonVariant::Secondary);
        // Off while a copy runs, a second click would start a second job,
        // and until the machine's facts are read.
        if ui.add_enabled(!tab.copying() && tab.facts().is_some(), copy).clicked() {
            *event = Some(SettingsEvent::MachineCopy);
        }
        match &tab.copied {
            Some(Ok(())) => {
                ui.label(RichText::new("Copied").color(t.palette().muted_foreground));
            }
            Some(Err(e)) => {
                ui.add(egui::Label::new(RichText::new(e).color(t.role(role::WARN))).truncate());
            }
            None => {}
        }
    });
}

/// `panels` one below the other, `TILE_GAP` apart as the bands of the
/// other tabs; inside a band, and after the last one, the spacing stays
/// as it was.
fn bands(ui: &mut Ui, t: &Theme, width: f32, panels: &[&Panel]) {
    let between = ui.spacing().item_spacing.y;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = TILE_GAP;
        for p in panels {
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = between;
                infopanel::band(ui, t, width, p.title, &p.lines);
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infopanel::Line;

    /// The bands' spacing stays with the bands: what follows them, *Copy*,
    /// stands as far below as anywhere else.
    #[test]
    fn the_bands_leave_the_spacing_as_they_found_it() {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let panel = Panel {
            title: crate::machine::TITLE,
            lines: vec![Line::row("Maker", "HP")],
        };
        let mut spacing = None;
        let mut o = ctx.run_ui(egui::RawInput::default(), |ui| {
            let t = Theme::current(ui.ctx());
            let before = ui.spacing().item_spacing;
            bands(ui, &t, 400.0, &[&panel, &panel]);
            spacing = Some((before, ui.spacing().item_spacing));
        });
        o.textures_delta.clear();
        let (before, after) = spacing.expect("drawn");
        assert_eq!(after, before);
    }
}
