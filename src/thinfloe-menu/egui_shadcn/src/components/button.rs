//! shadcn Button: variants (default/destructive/outline/secondary/ghost/link)
//! x sizes (sm/default/lg/icon). Custom-painted for full per-state control.

use crate::components::shared::{corner, focus_ring, hover_fill};
use crate::Theme;
use egui::{Color32, Response, Sense, Stroke, StrokeKind, Ui, Vec2, Widget};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    Default,
    Destructive,
    Outline,
    Secondary,
    Ghost,
    Link,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonSize {
    Sm,
    Default,
    Lg,
    Icon,
}

pub struct Button {
    text: String,
    variant: ButtonVariant,
    size: ButtonSize,
}

impl Button {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), variant: ButtonVariant::Default, size: ButtonSize::Default }
    }
    pub fn variant(mut self, v: ButtonVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn size(mut self, s: ButtonSize) -> Self {
        self.size = s;
        self
    }
}

struct Colors {
    fill: Color32,
    text: Color32,
    border: Option<Color32>,
}

fn colors_for(variant: ButtonVariant, t: &Theme) -> Colors {
    let p = &t.palette;
    match variant {
        ButtonVariant::Default => Colors { fill: p.primary, text: p.primary_foreground, border: None },
        ButtonVariant::Destructive => Colors { fill: p.destructive, text: p.destructive_foreground, border: None },
        ButtonVariant::Secondary => Colors { fill: p.secondary, text: p.secondary_foreground, border: None },
        ButtonVariant::Outline => Colors { fill: p.background, text: p.foreground, border: Some(p.border) },
        ButtonVariant::Ghost => Colors { fill: Color32::TRANSPARENT, text: p.foreground, border: None },
        ButtonVariant::Link => Colors { fill: Color32::TRANSPARENT, text: p.primary, border: None },
    }
}

impl Widget for Button {
    fn ui(self, ui: &mut Ui) -> Response {
        let t = Theme::current(ui.ctx());
        let (height, pad_x) = match self.size {
            ButtonSize::Sm => (32.0, 12.0),
            ButtonSize::Default => (36.0, 16.0),
            ButtonSize::Lg => (40.0, 24.0),
            ButtonSize::Icon => (36.0, 0.0),
        };
        let c = colors_for(self.variant, &t);

        // Lay out the text with the explicit color + medium family baked into the
        // galley, so it always wins over the global `override_text_color`
        // (shadcn buttons are font-medium).
        let fam = crate::theme::family(ui.ctx(), crate::theme::FAMILY_MEDIUM);
        let galley = ui.painter().layout_no_wrap(
            self.text.clone(),
            egui::FontId::new(14.0, fam.clone()),
            c.text,
        );

        let width = if self.size == ButtonSize::Icon {
            36.0
        } else {
            galley.size().x + pad_x * 2.0
        };
        let desired = Vec2::new(width, height);
        let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &self.text));

        let hovered = resp.hovered();
        let mut fill = c.fill;
        if hovered {
            fill = match self.variant {
                ButtonVariant::Ghost => t.palette.accent,
                // Link has no background on hover (text-decoration only)
                ButtonVariant::Link => Color32::TRANSPARENT,
                _ => hover_fill(fill, t.palette.background),
            };
        }

        let corner = corner(t.radius_md());
        if ui.is_rect_visible(rect) {
            if fill != Color32::TRANSPARENT {
                ui.painter().rect_filled(rect, corner, fill);
            }
            if let Some(b) = c.border {
                ui.painter().rect_stroke(rect, corner, Stroke::new(1.0, b), StrokeKind::Inside);
            }
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &self.text,
                egui::FontId::new(14.0, fam.clone()),
                c.text,
            );
        }
        focus_ring(ui, &resp, &t, t.radius_md());
        resp
    }
}
