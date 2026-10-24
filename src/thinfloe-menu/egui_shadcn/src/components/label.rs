//! shadcn Label: small medium-weight text; plus a muted description helper.

use crate::theme::FAMILY_MEDIUM;
use egui::{RichText, Ui};

pub fn label(ui: &mut Ui, text: &str) -> egui::Response {
    let family = crate::theme::family(ui.ctx(), FAMILY_MEDIUM);
    ui.label(RichText::new(text).family(family).size(14.0))
}

pub fn description(ui: &mut Ui, text: &str) -> egui::Response {
    let muted = crate::Theme::current(ui.ctx()).palette.muted_foreground;
    ui.label(RichText::new(text).size(14.0).color(muted))
}
