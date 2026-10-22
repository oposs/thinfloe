//! shadcn v4 (new-york / OKLCH) design tokens, materialized for egui.

use crate::color::{oklch_to_srgb as c, oklch_to_srgb_a as ca};
use egui::Color32;

/// The semantic shadcn color tokens. Surfaces come in `x` / `x_foreground` pairs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub background: Color32,
    pub foreground: Color32,
    pub card: Color32,
    pub card_foreground: Color32,
    pub popover: Color32,
    pub popover_foreground: Color32,
    pub primary: Color32,
    pub primary_foreground: Color32,
    pub secondary: Color32,
    pub secondary_foreground: Color32,
    pub muted: Color32,
    pub muted_foreground: Color32,
    pub accent: Color32,
    pub accent_foreground: Color32,
    pub destructive: Color32,
    pub destructive_foreground: Color32,
    pub border: Color32,
    pub input: Color32,
    pub ring: Color32,
}

impl Palette {
    pub fn light() -> Self {
        Self {
            background: c(1.0, 0.0, 0.0),
            foreground: c(0.145, 0.0, 0.0),
            card: c(1.0, 0.0, 0.0),
            card_foreground: c(0.145, 0.0, 0.0),
            popover: c(1.0, 0.0, 0.0),
            popover_foreground: c(0.145, 0.0, 0.0),
            primary: c(0.205, 0.0, 0.0),
            primary_foreground: c(0.985, 0.0, 0.0),
            secondary: c(0.97, 0.0, 0.0),
            secondary_foreground: c(0.205, 0.0, 0.0),
            muted: c(0.97, 0.0, 0.0),
            muted_foreground: c(0.556, 0.0, 0.0),
            accent: c(0.97, 0.0, 0.0),
            accent_foreground: c(0.205, 0.0, 0.0),
            destructive: c(0.577, 0.245, 27.325),
            destructive_foreground: c(0.985, 0.0, 0.0),
            border: c(0.922, 0.0, 0.0),
            input: c(0.922, 0.0, 0.0),
            ring: c(0.708, 0.0, 0.0),
        }
    }

    pub fn dark() -> Self {
        Self {
            background: c(0.145, 0.0, 0.0),
            foreground: c(0.985, 0.0, 0.0),
            card: c(0.205, 0.0, 0.0),
            card_foreground: c(0.985, 0.0, 0.0),
            popover: c(0.205, 0.0, 0.0),
            popover_foreground: c(0.985, 0.0, 0.0),
            primary: c(0.922, 0.0, 0.0),
            primary_foreground: c(0.205, 0.0, 0.0),
            secondary: c(0.269, 0.0, 0.0),
            secondary_foreground: c(0.985, 0.0, 0.0),
            muted: c(0.269, 0.0, 0.0),
            muted_foreground: c(0.708, 0.0, 0.0),
            accent: c(0.269, 0.0, 0.0),
            accent_foreground: c(0.985, 0.0, 0.0),
            destructive: c(0.704, 0.191, 22.216),
            destructive_foreground: c(0.985, 0.0, 0.0),
            border: ca(1.0, 0.0, 0.0, 0.10),
            input: ca(1.0, 0.0, 0.0, 0.15),
            ring: c(0.556, 0.0, 0.0), // solid mid-gray (border/input above are translucent white)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub palette: Palette,
    pub radius: f32,
    pub dark: bool,
}

impl Theme {
    pub fn light() -> Self {
        Self { palette: Palette::light(), radius: 10.0, dark: false }
    }
    pub fn dark() -> Self {
        Self { palette: Palette::dark(), radius: 10.0, dark: true }
    }

    pub fn radius_sm(&self) -> f32 { (self.radius - 4.0).max(0.0) }
    pub fn radius_md(&self) -> f32 { (self.radius - 2.0).max(0.0) }
    pub fn radius_lg(&self) -> f32 { self.radius }
    pub fn radius_xl(&self) -> f32 { self.radius + 4.0 }
}

// ---- Font + apply + accessor ----

use egui::{FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};
use std::sync::Arc;

fn theme_id() -> egui::Id {
    static ID: std::sync::LazyLock<egui::Id> =
        std::sync::LazyLock::new(|| egui::Id::new("egui_shadcn::active_theme"));
    *ID
}

/// Named font families for shadcn-style weight emphasis.
pub const FAMILY_MEDIUM: &str = "oxanium-medium";
pub const FAMILY_SEMIBOLD: &str = "oxanium-semibold";

/// Return the named font family if it's registered (via `Theme::apply`), else
/// fall back to Proportional. egui defers `set_fonts` to the next frame, so on
/// the very first frame after `apply` a named family may not be in the atlas yet.
pub fn family(ctx: &egui::Context, name: &str) -> egui::FontFamily {
    let fam = egui::FontFamily::Name(name.into());
    if ctx.fonts(|f| f.definitions().families.contains_key(&fam)) {
        fam
    } else {
        egui::FontFamily::Proportional
    }
}

// NOTE: The Google Fonts repository only ships Oxanium as a variable font
// (Oxanium[wght].ttf) — there are no separate static weight files any more.
// All three weight slots use the same variable font binary; egui will render
// at one effective weight.  A future improvement would be to use font
// variation settings once egui gains that support.
fn install_fonts(ctx: &egui::Context) {
    // Building the font atlas is expensive and `apply` runs every frame, so
    // install the fonts only once per context.
    let installed_id = egui::Id::new("egui_shadcn::fonts_installed");
    if ctx.data(|d| d.get_temp::<bool>(installed_id)).unwrap_or(false) {
        return;
    }
    // Oxanium's ascent reserves more empty space above the cap height than below
    // the baseline, so egui's row-box centering (`Align2::CENTER_CENTER` in
    // buttons/badges/tabs, and `TextEdit` vertical centering) leaves single-line
    // text sitting ~0.18em too high. Nudge every glyph down by that fraction so
    // centered text is optically balanced. Measured against the reference button.
    let tweak = egui::FontTweak { y_offset_factor: 0.18, ..Default::default() };
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "oxanium".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/Oxanium-Regular.ttf")).tweak(tweak.clone())),
    );
    fonts.font_data.insert(
        FAMILY_MEDIUM.into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/Oxanium-Medium.ttf")).tweak(tweak.clone())),
    );
    fonts.font_data.insert(
        FAMILY_SEMIBOLD.into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/Oxanium-SemiBold.ttf")).tweak(tweak)),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "oxanium".into());
    fonts
        .families
        .insert(FontFamily::Name(FAMILY_MEDIUM.into()), vec![FAMILY_MEDIUM.into()]);
    fonts
        .families
        .insert(FontFamily::Name(FAMILY_SEMIBOLD.into()), vec![FAMILY_SEMIBOLD.into()]);
    ctx.set_fonts(fonts);
    ctx.data_mut(|d| d.insert_temp(installed_id, true));
}

impl Theme {
    /// Read the active theme stored by [`Theme::apply`]. Falls back to dark.
    pub fn current(ctx: &egui::Context) -> Theme {
        ctx.data(|d| d.get_temp::<Theme>(theme_id())).unwrap_or_else(Theme::dark)
    }

    /// Push this theme into the egui context: fonts, type scale, spacing, colors.
    ///
    /// Safe to call once per frame: the font atlas is only built on the first
    /// call per context (subsequent calls just refresh the style and stored
    /// theme). egui-native widgets pick up the inactive/hovered/active visuals
    /// set here, while the crate's own custom components manage their own
    /// per-state colors.
    pub fn apply(&self, ctx: &egui::Context) {
        install_fonts(ctx);
        let p = &self.palette;

        ctx.global_style_mut(|s| {
            use FontFamily::Proportional;
            s.text_styles = [
                (TextStyle::Small, FontId::new(12.0, Proportional)),
                (TextStyle::Body, FontId::new(14.0, Proportional)),
                (TextStyle::Button, FontId::new(14.0, Proportional)),
                (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
                (TextStyle::Heading, FontId::new(20.0, FontFamily::Name(FAMILY_SEMIBOLD.into()))),
            ]
            .into();

            s.spacing.item_spacing = egui::vec2(8.0, 8.0);
            s.spacing.button_padding = egui::vec2(16.0, 8.0);
            s.spacing.window_margin = egui::Margin::same(0);
            s.spacing.interact_size.y = 36.0;

            let v = &mut s.visuals;
            v.dark_mode = self.dark;
            v.panel_fill = p.background;
            v.window_fill = p.card;
            v.window_stroke = Stroke::new(1.0, p.border);
            v.extreme_bg_color = p.input;
            v.faint_bg_color = p.muted;
            v.override_text_color = Some(p.foreground);
            v.hyperlink_color = p.primary;
            v.selection.bg_fill = p.primary.gamma_multiply(0.35);
            v.selection.stroke = Stroke::new(1.0, p.ring);
            v.widgets.noninteractive.bg_fill = p.background;
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.foreground);
            for w in [
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.bg_fill = p.secondary;
                w.weak_bg_fill = p.secondary;
                w.bg_stroke = Stroke::new(1.0, p.border);
                w.fg_stroke = Stroke::new(1.0, p.foreground);
                w.corner_radius = egui::CornerRadius::same(self.radius_md() as u8);
            }
            // shadcn uses the `accent` token for hover/active feedback so
            // native widgets (e.g. ComboBox dropdown items) don't look inert.
            for w in [&mut v.widgets.hovered, &mut v.widgets.active] {
                w.bg_fill = p.accent;
                w.weak_bg_fill = p.accent;
            }
        });

        ctx.data_mut(|d| d.insert_temp(theme_id(), *self));
    }
}

/// Convenience accessor for components.
pub fn theme(ctx: &egui::Context) -> Theme {
    Theme::current(ctx)
}
