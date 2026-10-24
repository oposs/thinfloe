//! shadcn Checkbox: 16px square, rounded, filled with primary + checkmark when checked.

use crate::Theme;
use egui::{CornerRadius, Sense, Stroke, StrokeKind, Ui, Vec2};

/// Render a 16×16 shadcn-style checkbox.
///
/// When checked: filled with `palette.primary`, white checkmark drawn inside.
/// When unchecked: transparent with a 1px `palette.border` stroke.
pub fn checkbox(ui: &mut Ui, checked: &mut bool) -> egui::Response {
    let t = Theme::current(ui.ctx());
    let size = Vec2::splat(16.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    let corner = CornerRadius::same(4);
    if *checked {
        ui.painter().rect_filled(rect, corner, t.palette.primary);
        let c = rect.shrink(3.5);
        let stroke = Stroke::new(2.0, t.palette.primary_foreground);
        // Left arm of checkmark: bottom-left corner to middle-bottom
        ui.painter().line_segment(
            [
                egui::pos2(c.left(), c.center().y),
                egui::pos2(c.center().x - 1.0, c.bottom()),
            ],
            stroke,
        );
        // Right arm of checkmark: middle-bottom to top-right
        ui.painter().line_segment(
            [
                egui::pos2(c.center().x - 1.0, c.bottom()),
                egui::pos2(c.right(), c.top()),
            ],
            stroke,
        );
    } else {
        ui.painter().rect_stroke(
            rect,
            corner,
            Stroke::new(1.0, t.palette.border),
            StrokeKind::Inside,
        );
    }
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *checked, "")
    });
    resp
}
