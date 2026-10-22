//! shadcn Select: a styled egui ComboBox matching Input's border/height.

use egui::Ui;

/// Renders a select. `selected` is the chosen index. Returns true if it changed.
pub fn select(ui: &mut Ui, id: &str, selected: &mut usize, options: &[&str]) -> bool {
    let mut changed = false;
    let current = options.get(*selected).copied().unwrap_or("");
    let w = ui.available_width().min(280.0);
    egui::ComboBox::from_id_salt(id)
        .selected_text(current)
        .width(w)
        .show_ui(ui, |ui| {
            for (i, opt) in options.iter().enumerate() {
                if ui.selectable_label(*selected == i, *opt).clicked() {
                    *selected = i;
                    changed = true;
                }
            }
        });
    changed
}
