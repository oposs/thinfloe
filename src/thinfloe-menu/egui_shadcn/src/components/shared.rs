//! Shared painting helpers for shadcn-style components.

use egui::{Color32, CornerRadius, Response, Stroke, Ui};
use crate::Theme;

/// Build a CornerRadius from an f32 radius, rounding to the nearest pixel.
pub fn corner(r: f32) -> CornerRadius {
    CornerRadius::same(r.round() as u8)
}

/// Linearly blend `a` toward `b` by `t` in 0..=1 (sRGB space, good enough for hover).
pub fn mix_toward(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()), a.a())
}

/// shadcn hover = base color at 90% opacity over the background. We approximate
/// by mixing the fill 10% toward the page background.
pub fn hover_fill(fill: Color32, bg: Color32) -> Color32 {
    mix_toward(fill, bg, 0.10)
}

/// Paint the shadcn focus ring: a 3px ring at 50% of the ring color, just outside
/// the widget rect, when the response has keyboard focus.
pub fn focus_ring(ui: &Ui, resp: &Response, theme: &Theme, corner: f32) {
    if resp.has_focus() {
        let ring = theme.palette.ring.gamma_multiply(0.5);
        let rect = resp.rect.expand(2.0);
        ui.painter().rect_stroke(
            rect,
            corner,
            Stroke::new(3.0, ring),
            egui::StrokeKind::Outside,
        );
    }
}
