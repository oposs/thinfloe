//! shadcn Separator: a 1px border-colored divider.

use crate::Theme;
use egui::Ui;

pub fn separator(ui: &mut Ui) {
    let t = Theme::current(ui.ctx());
    let prev = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.visuals_mut().widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, t.palette.border);
    ui.separator();
    ui.visuals_mut().widgets.noninteractive.bg_stroke = prev;
}
