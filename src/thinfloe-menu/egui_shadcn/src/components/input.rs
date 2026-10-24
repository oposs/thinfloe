//! shadcn Input: a single-line text field themed with the input border,
//! 36px height, rounded-md, focus ring.

use crate::components::shared::focus_ring;
use crate::Theme;
use egui::{Margin, TextEdit, Ui, Vec2, Widget};

pub struct Input<'a> {
    text: &'a mut String,
    hint: String,
    password: bool,
    max_width: f32,
}

impl<'a> Input<'a> {
    pub fn new(text: &'a mut String) -> Self {
        Self { text, hint: String::new(), password: false, max_width: 280.0 }
    }
    pub fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = h.into();
        self
    }
    pub fn password(mut self, yes: bool) -> Self {
        self.password = yes;
        self
    }
    /// Override the default 280px maximum width. Pass `f32::INFINITY` for full-width.
    pub fn max_width(mut self, w: f32) -> Self {
        self.max_width = w;
        self
    }
}

impl<'a> Widget for Input<'a> {
    fn ui(self, ui: &mut Ui) -> egui::Response {
        let t = Theme::current(ui.ctx());
        let corner = crate::components::shared::corner(t.radius_md());
        let resp = ui.add_sized(
            Vec2::new(ui.available_width().min(self.max_width), 36.0),
            TextEdit::singleline(self.text)
                .hint_text(self.hint)
                .password(self.password)
                .margin(Margin::symmetric(12, 8))
                .vertical_align(egui::Align::Center)
                .background_color(t.palette.input)
                .text_color(t.palette.foreground),
        );
        // Draw our own 1px border to match shadcn (over egui's frame).
        ui.painter().rect_stroke(
            resp.rect,
            corner,
            egui::Stroke::new(1.0, t.palette.border),
            egui::StrokeKind::Inside,
        );
        focus_ring(ui, &resp, &t, t.radius_md());
        resp
    }
}
