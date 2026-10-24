//! shadcn Switch: a pill track + sliding round thumb (~32x18px).

use crate::Theme;
use egui::{Sense, Ui, Vec2};

/// Render a pill-shaped toggle switch.
///
/// Returns the response; the caller should check `.clicked()` to mutate state,
/// or use the fact that this function toggles `*on` directly on click.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let t = Theme::current(ui.ctx());
    let size = Vec2::new(32.0, 18.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let how_on = ui.ctx().animate_bool(resp.id, *on);
    let track = if *on { t.palette.primary } else { t.palette.input };
    let radius = rect.height() / 2.0;
    ui.painter().rect_filled(rect, radius, track);
    let knob_r = rect.height() / 2.0 - 2.0;
    let cx = egui::lerp(
        (rect.left() + knob_r + 2.0)..=(rect.right() - knob_r - 2.0),
        how_on,
    );
    ui.painter().circle_filled(
        egui::pos2(cx, rect.center().y),
        knob_r,
        t.palette.background,
    );
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, "")
    });
    resp
}
