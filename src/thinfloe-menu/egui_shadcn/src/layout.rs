//! Flexbox substitutes for egui's single-pass layout.
//! `gap` maps to item_spacing; space-between right-aligns via a nested layout.

use crate::Theme;
use egui::{CornerRadius, Frame, Margin, Stroke, Ui};
use egui_extras::{Size, StripBuilder};

/// Horizontal stack with an explicit gap (flex-direction: row; gap: N).
pub fn row<R>(ui: &mut Ui, gap: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        add(ui)
    })
    .inner
}

/// Vertical stack with an explicit gap (flex-direction: column; gap: N).
pub fn vstack<R>(ui: &mut Ui, gap: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = gap;
        add(ui)
    })
    .inner
}

/// A row whose `left` content hugs the start and `right` content hugs the end
/// (justify-content: space-between).
pub fn space_between(
    ui: &mut Ui,
    left: impl FnOnce(&mut Ui),
    right: impl FnOnce(&mut Ui),
) {
    ui.horizontal(|ui| {
        left(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            right(ui);
        });
    });
}

/// A shadcn Card surface: card fill, 1px border, rounded-xl, p-6, subtle shadow.
/// Reads the active theme from the context (set via `Theme::apply`).
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let theme = Theme::current(ui.ctx());
    Frame::new()
        .fill(theme.palette.card)
        .stroke(Stroke::new(1.0, theme.palette.border))
        .corner_radius(CornerRadius::same(theme.radius_xl() as u8))
        .inner_margin(Margin::same(24))
        .shadow(egui::epaint::Shadow {
            offset: [0, 1],
            blur: 3,
            spread: 0,
            color: egui::Color32::from_black_alpha(20),
        })
        .show(ui, |ui| add(ui))
        .inner
}

/// A labeled control row: fixed-width label column + control filling the rest.
pub fn form_row(ui: &mut Ui, label_width: f32, label: &str, control: impl FnOnce(&mut Ui)) {
    let row_h = ui.spacing().interact_size.y.max(20.0);
    ui.allocate_ui(egui::vec2(ui.available_width(), row_h), |ui| {
        StripBuilder::new(ui)
            .size(Size::exact(label_width))
            .size(Size::remainder())
            .horizontal(|mut strip| {
                strip.cell(|ui| {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.label(label);
                    });
                });
                strip.cell(|ui| {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        control(ui)
                    });
                });
            });
    });
}
