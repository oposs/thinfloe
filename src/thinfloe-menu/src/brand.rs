//! The ThinFloe look: palette, fonts and the logo monogram.

use std::sync::Arc;

use egui::{Color32, ColorImage, Context, FontData, FontDefinitions, FontFamily, FontId};
use resvg::tiny_skia::{Pixmap, Transform};
use shadcn_egui::style::Radius;
use shadcn_egui::{FontFiles, FontKind, FontSource, Palette, Style};
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

/// egui's own plain sans as the theme's body font, in every weight: the menu
/// keeps the small text it has always had (owner, 2026-11-05).
pub static UBUNTU_LIGHT: FontFiles = FontFiles {
    name: "ubuntu-light",
    kind: FontKind::Sans,
    source: FontSource::Static {
        regular: epaint_default_fonts::UBUNTU_LIGHT,
        medium: None,
        semibold: None,
        bold: None,
    },
    y_offset_factor: 0.0,
    license: include_str!("../assets/Ubuntu-UFL.txt"),
};

pub fn theme() -> shadcn_egui::Theme {
    let mut p = Palette::dark();
    p.background = BG;
    p.card = TILE;
    p.accent = TILE_HOVER;
    // The primary button is ice blue with dark text, as in the mockups.
    p.primary = ICE;
    p.primary_foreground = Color32::from_rgb(0x04, 0x12, 0x1f);
    p.ring = ICE;
    p.border = LINE;
    p.muted_foreground = MUTED;
    // One look in both modes, so a light system changes nothing; square
    // corners everywhere, as in the mockups.
    let mut style = Style::vega();
    // vega's tick box is `rounded-[4px]`, not a step of the theme radius.
    style.checkbox.radius.set(Radius::Px(0.0));
    shadcn_egui::Theme::new(style, p, p, 0.0).fonts(&UBUNTU_LIGHT, None)
}

/// Apply the theme and install the fonts. Once per context: the context
/// outlives a replaced window, and a per-frame apply would undo style
/// writes.
pub fn apply(ctx: &Context) {
    theme().apply_with_fonts(ctx, fonts());
    ctx.set_theme(egui::Theme::Dark);
}

/// Geo, unhinted. Its strokes are about 1.5 px at tile size, and egui's
/// hinting snaps only vertically: horizontal bars rounded down to one pixel
/// row next to vertical stems spread over two, so the bars of "e", "a" and
/// "B" looked too thin.
fn geo_data() -> FontData {
    let mut data = FontData::from_static(include_bytes!("../assets/Geo-Oblique.ttf"));
    data.tweak.hinting = Some(false);
    data
}

fn fonts() -> FontDefinitions {
    // egui's own set: a plain sans for Proportional, Hack for Monospace,
    // and the emoji fonts at the end of both.
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(GEO_FONT.into(), Arc::new(geo_data()));
    let sans = fonts.families[&FontFamily::Proportional].clone();
    // Geo has Latin only; everything else draws through the plain sans.
    let mut geo = vec![GEO_FONT.to_string()];
    geo.extend(sans.iter().cloned());
    fonts.families.insert(FontFamily::Name(GEO.into()), geo);
    fonts
}

const LOGO_T: &str = "M278 388H625V447H433V632H360V447H278Z";
const LOGO_F: &str = "M498 502H624V562H570V632H498Z";
/// The monogram's bounds after the slant, with a 2 unit margin. skewX(-9)
/// moves a point at height y right by tan(9 deg) * (632 - y): the T bar's
/// lower-left corner (278, 447) lands at x 307.3, its upper-right corner
/// (625, 388) at x 663.6.
const LOGO_BOX: (f32, f32, f32, f32) = (305.0, 386.0, 361.0, 248.0);

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
        for mode in [egui::Theme::Dark, egui::Theme::Light] {
            let p = t.palette_of(mode);
            assert_eq!((p.background, p.card, p.ring, p.primary), (BG, TILE, ICE, ICE));
            assert_eq!((p.accent, p.border, p.muted_foreground), (TILE_HOVER, LINE, MUTED));
        }
        assert_eq!(t.radius, 0.0);
    }

    /// Runs `n` empty passes: fonts land one pass after `apply`.
    fn settle(ctx: &Context, input: &egui::RawInput, n: usize) {
        for _ in 0..n {
            ctx.run_ui(input.clone(), |_| {}).textures_delta.clear();
        }
    }

    #[test]
    fn geo_and_the_body_font_survive_the_theme() {
        let ctx = Context::default();
        apply(&ctx);
        settle(&ctx, &Default::default(), 3);
        ctx.fonts(|f| {
            let d = f.definitions();
            let geo = &d.families[&FontFamily::Name(GEO.into())];
            assert_eq!(geo[0], "Geo-Oblique");
            // No theme face (Inter's fallback or a body face) sits in Geo's list.
            assert!(!geo.iter().any(|n| n.starts_with("shadcn-") || n.contains("oxanium")), "{geo:?}");
            let first = |fam: &FontFamily| (*d.font_data[&d.families[fam][0]].font).as_ref().to_vec();
            let ubuntu = epaint_default_fonts::UBUNTU_LIGHT;
            assert!(first(&FontFamily::Proportional) == ubuntu, "body is Ubuntu Light");
            let heading = FontFamily::Name(shadcn_egui::theme::FAMILY_HEADING.into());
            if d.families.contains_key(&heading) {
                assert!(first(&heading) == ubuntu, "headings are Ubuntu Light");
            }
            let semibold = FontFamily::Name(shadcn_egui::theme::FAMILY_SEMIBOLD.into());
            assert!(first(&semibold) == ubuntu, "semibold is Ubuntu Light");
        });
    }

    #[test]
    fn a_light_system_keeps_the_dark_look() {
        let ctx = Context::default();
        apply(&ctx);
        let input = egui::RawInput {
            system_theme: Some(egui::Theme::Light),
            ..Default::default()
        };
        settle(&ctx, &input, 2);
        assert_eq!(shadcn_egui::Theme::current(&ctx).palette().background, BG);
        assert_eq!(ctx.global_style().visuals.panel_fill, BG);
    }

    #[test]
    fn the_theme_outlives_a_new_window_pass() {
        let ctx = Context::default();
        apply(&ctx);
        let at = |w, h| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))),
            ..Default::default()
        };
        settle(&ctx, &at(1920.0, 1080.0), 1);
        settle(&ctx, &at(3840.0, 2160.0), 1);
        assert_eq!(shadcn_egui::Theme::current(&ctx).palette().background, BG);
    }

    #[test]
    fn geo_is_drawn_unhinted() {
        assert_eq!(fonts().font_data[GEO_FONT].tweak.hinting, Some(false));
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
            let fam = f
                .definitions()
                .families
                .get(&FontFamily::Name(GEO.into()))
                .unwrap()
                .clone();
            assert_eq!(fam[0], "Geo-Oblique");
            assert!(fam.len() >= 2, "Geo needs a fallback: {fam:?}");
            assert!(
                !fam.iter().any(|n| n.contains("oxanium") || n.starts_with("shadcn-")),
                "no theme font in Geo: {fam:?}"
            );
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
        assert!(img.pixels.contains(&Color32::WHITE));
        assert!(
            img.pixels.iter().all(|p| p.r() == p.g() && p.g() == p.b()),
            "white only"
        );
        // The slant: the stem's left edge moves right going up.
        let [iw, ih] = img.size;
        let opaque = |x: usize, y: usize| img.pixels[y * iw + x].a() > 128;
        let left = |y: usize| (0..iw).find(|&x| opaque(x, y)).expect("a row with ink");
        assert!(left(22) > left(50) + 2, "slanted: {} vs {}", left(22), left(50));
        // No empty margin: the ink reaches every edge within 2 px.
        let xs: Vec<usize> = (0..ih).flat_map(|y| (0..iw).filter(move |&x| opaque(x, y))).collect();
        let ys: Vec<usize> = (0..ih).filter(|&y| (0..iw).any(|x| opaque(x, y))).collect();
        assert!(
            *xs.iter().min().unwrap() <= 2,
            "left margin {}",
            xs.iter().min().unwrap()
        );
        assert!(*xs.iter().max().unwrap() >= iw - 3, "right margin");
        assert!(ys[0] <= 2 && *ys.last().unwrap() >= ih - 3, "top/bottom margin");
    }
}
