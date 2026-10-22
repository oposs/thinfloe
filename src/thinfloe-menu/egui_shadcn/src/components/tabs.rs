//! shadcn Tabs: a muted track with the active trigger as a raised pill.

use crate::Theme;
use egui::{Align2, CornerRadius, FontId, Sense, Ui, Vec2};

/// Renders a tab bar. `active` is the selected index. Returns true if it changed.
pub fn tab_bar(ui: &mut Ui, active: &mut usize, labels: &[&str]) -> bool {
    let t = Theme::current(ui.ctx());
    let mut changed = false;
    let pad = 3.0;
    let trigger_h = 30.0;
    let bar_h = trigger_h + pad * 2.0;

    let total_w = ui.available_width();
    let (rect, _resp) = ui.allocate_exact_size(Vec2::new(total_w, bar_h), Sense::hover());
    // track background
    ui.painter().rect_filled(rect, CornerRadius::same(t.radius_lg() as u8), t.palette.muted);

    let inner = rect.shrink(pad);
    let n = labels.len().max(1) as f32;
    let tw = inner.width() / n;
    for (i, lbl) in labels.iter().enumerate() {
        let tr = egui::Rect::from_min_size(
            egui::pos2(inner.left() + tw * i as f32, inner.top()),
            Vec2::new(tw, trigger_h),
        );
        let resp = ui.interact(tr, ui.id().with(("tab", i)), Sense::click());
        if resp.clicked() && *active != i {
            *active = i;
            changed = true;
        }
        let is_active = *active == i;
        if is_active {
            // raised pill for the active tab
            ui.painter().rect_filled(
                tr,
                CornerRadius::same(t.radius_md() as u8),
                t.palette.background,
            );
        }
        let color = if is_active {
            t.palette.foreground
        } else {
            t.palette.muted_foreground
        };
        ui.painter().text(
            tr.center(),
            Align2::CENTER_CENTER,
            lbl,
            FontId::proportional(14.0),
            color,
        );
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, lbl));
    }
    changed
}
