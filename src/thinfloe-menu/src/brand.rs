//! The ThinFloe look: palette, fonts and the logo monogram.

use std::sync::Arc;

use egui::{Color32, ColorImage, Context, FontData, FontDefinitions, FontFamily, FontId};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;

/// Ice blue, the accent.
pub const ICE: Color32 = Color32::from_rgb(0x7c, 0xc4, 0xff);
pub const BG: Color32 = Color32::from_rgb(0x05, 0x07, 0x0a);
pub const TILE: Color32 = Color32::from_rgb(0x0d, 0x12, 0x18);
pub const TILE_ACTIVE: Color32 = Color32::from_rgb(0x10, 0x1b, 0x26);
pub const TILE_HOVER: Color32 = Color32::from_rgb(0x13, 0x1c, 0x26);
pub const LINE: Color32 = Color32::from_rgb(0x15, 0x1b, 0x22);
pub const MUTED: Color32 = Color32::from_rgb(0x7d, 0x89, 0x96);
/// The grey of timestamps.
pub const TIME: Color32 = Color32::from_rgb(0x5d, 0x69, 0x76);
/// A warning amber, for reasons drawn on a disabled tile.
pub const WARN: Color32 = Color32::from_rgb(0xc7, 0x9a, 0x4a);
pub const GOOD: Color32 = Color32::from_rgb(0x98, 0xc3, 0x79);

/// Font family of the name, titles and tile names.
pub const GEO: &str = "geo";
const GEO_FONT: &str = "Geo-Oblique";

pub fn geo(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(GEO.into()))
}

pub fn theme() -> egui_shadcn::Theme {
    let mut t = egui_shadcn::Theme::dark();
    t.palette.background = BG;
    t.palette.card = TILE;
    t.palette.accent = TILE_HOVER;
    t.palette.ring = ICE;
    t.palette.border = LINE;
    t.palette.muted_foreground = MUTED;
    t
}

/// Apply the theme and install the fonts. Safe to call every frame: the
/// font atlas is built once per context.
pub fn apply(ctx: &Context) {
    // egui_shadcn installs its own fonts on its first call; ours are set
    // after that, so they replace them.
    theme().apply(ctx);
    let id = egui::Id::new("thinfloe::brand_fonts_installed");
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        return;
    }
    ctx.set_fonts(fonts());
    ctx.data_mut(|d| d.insert_temp(id, true));
}

fn fonts() -> FontDefinitions {
    // egui's own set: a plain sans for Proportional, Hack for Monospace,
    // and the emoji fonts at the end of both.
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        GEO_FONT.into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/Geo-Oblique.ttf"))),
    );
    let sans = fonts.families[&FontFamily::Proportional].clone();
    // Geo has Latin only; everything else draws through the plain sans.
    let mut geo = vec![GEO_FONT.to_string()];
    geo.extend(sans.iter().cloned());
    fonts.families.insert(FontFamily::Name(GEO.into()), geo);
    // egui_shadcn's Heading style names these two; keep them off Oxanium.
    for name in [egui_shadcn::theme::FAMILY_MEDIUM, egui_shadcn::theme::FAMILY_SEMIBOLD] {
        fonts.families.insert(FontFamily::Name(name.into()), sans.clone());
    }
    fonts
}

const LOGO_T: &str = "M278 388H625V447H433V632H360V447H278Z";
const LOGO_F: &str = "M498 502H624V562H570V632H498Z";
/// The monogram's bounds after the slant, with a 2 unit margin.
const LOGO_BOX: (f32, f32, f32, f32) = (276.0, 386.0, 390.0, 248.0);

/// The slanted TF monogram, white on transparent, `height_px` high.
pub fn logo(height_px: u32) -> ColorImage {
    let (x, y, w, h) = LOGO_BOX;
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{x} {y} {w} {h}\">\
         <g fill=\"#fff\" transform=\"translate(0 632) skewX(-9) translate(0 -632)\">\
         <path d=\"{LOGO_T}\"/><path d=\"{LOGO_F}\"/></g></svg>"
    );
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).expect("the logo SVG is a constant");
    let scale = height_px as f32 / h;
    let width_px = (w * scale).round() as u32;
    let mut pix = Pixmap::new(width_px, height_px).expect("the logo size is nonzero");
    resvg::render(&tree, Transform::from_scale(scale, scale), &mut pix.as_mut());
    ColorImage::from_rgba_premultiplied([width_px as usize, height_px as usize], pix.data())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_is_the_spec_palette() {
        assert_eq!(ICE, Color32::from_rgb(0x7c, 0xc4, 0xff));
        assert_eq!(BG, Color32::from_rgb(0x05, 0x07, 0x0a));
        assert_eq!(TILE, Color32::from_rgb(0x0d, 0x12, 0x18));
        assert_eq!(TILE_ACTIVE, Color32::from_rgb(0x10, 0x1b, 0x26));
        let t = theme();
        assert_eq!((t.palette.background, t.palette.card, t.palette.ring), (BG, TILE, ICE));
    }

    #[test]
    fn geo_falls_back_for_glyphs_it_lacks() {
        let ctx = Context::default();
        apply(&ctx);
        // fonts land one frame late
        for _ in 0..2 {
            ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        }
        ctx.fonts(|f| {
            let fam = f.definitions().families.get(&FontFamily::Name(GEO.into())).unwrap().clone();
            assert_eq!(fam[0], "Geo-Oblique");
            assert!(fam.len() >= 2, "Geo needs a fallback: {fam:?}");
            assert!(!fam.iter().any(|n| n.contains("oxanium")), "Oxanium is gone: {fam:?}");
        });
        ctx.fonts_mut(|f| {
            assert!(f.has_glyphs(&geo(20.0), "Über Straße"), "fallback must cover umlauts");
            // egui's default fonts carry no CJK; there is nothing to fall back to.
        });
    }

    #[test]
    fn the_logo_is_white_slanted_and_about_three_by_two() {
        let img = logo(52);
        assert_eq!(img.size[1], 52);
        let w = img.size[0] as f32 / 52.0;
        assert!((1.2..1.8).contains(&w), "aspect {w}");
        assert!(img.pixels.iter().any(|p| *p == Color32::WHITE));
        assert!(img.pixels.iter().all(|p| p.r() == p.g() && p.g() == p.b()), "white only");
    }
}
