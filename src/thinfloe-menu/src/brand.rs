//! The ThinFloe look: palette, fonts and the logo monogram.

use std::sync::Arc;

use egui::{Color32, ColorImage, Context, FontData, FontDefinitions, FontFamily, FontId};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;
use shadcn_egui::components::item::ItemVariant;
use shadcn_egui::style::{AppVariant, Paint, Radius, Role, Rung, State};
use shadcn_egui::{FontFiles, FontKind, FontSource, Palette, Style};

// The palette. UI code reads these through the theme: the six that are
// shadcn roles as `Theme::palette()` (ICE primary and ring, BG background,
// TILE card, TILE_HOVER accent, LINE border, MUTED muted_foreground), the
// other five as the app colours in `role`.

/// Ice blue, the accent.
const ICE: Color32 = Color32::from_rgb(0x7c, 0xc4, 0xff);
const BG: Color32 = Color32::from_rgb(0x05, 0x07, 0x0a);
const TILE: Color32 = Color32::from_rgb(0x0d, 0x12, 0x18);
const TILE_ACTIVE: Color32 = Color32::from_rgb(0x10, 0x1b, 0x26);
const TILE_HOVER: Color32 = Color32::from_rgb(0x13, 0x1c, 0x26);
const LINE: Color32 = Color32::from_rgb(0x15, 0x1b, 0x22);
const MUTED: Color32 = Color32::from_rgb(0x7d, 0x89, 0x96);
/// The grey of timestamps.
const TIME: Color32 = Color32::from_rgb(0x5d, 0x69, 0x76);
/// A warning amber, for reasons drawn on a disabled tile.
const WARN: Color32 = Color32::from_rgb(0xc7, 0x9a, 0x4a);
const GOOD: Color32 = Color32::from_rgb(0x98, 0xc3, 0x79);
/// A battery about to run out.
const RED: Color32 = Color32::from_rgb(0xff, 0x6b, 0x6b);

/// The palette's values, for tests that look for a colour in drawn shapes.
#[cfg(test)]
pub(crate) mod rgb {
    use egui::Color32;
    pub(crate) const BG: Color32 = super::BG;
    pub(crate) const GOOD: Color32 = super::GOOD;
    pub(crate) const ICE: Color32 = super::ICE;
    pub(crate) const MUTED: Color32 = super::MUTED;
    pub(crate) const TILE: Color32 = super::TILE;
    pub(crate) const TILE_HOVER: Color32 = super::TILE_HOVER;
    pub(crate) const TIME: Color32 = super::TIME;
    pub(crate) const WARN: Color32 = super::WARN;
}

/// The app colours: what the shadcn palette has no role for.
pub mod role {
    use shadcn_egui::style::Role;
    /// A tile while its program runs.
    pub const TILE_ACTIVE: Role = Role::custom("tf-tile-active");
    /// Timestamps in the log.
    pub const TIME: Role = Role::custom("tf-time");
    /// Warnings: a grey tile's reason, a failed save.
    pub const WARN: Role = Role::custom("tf-warn");
    /// Good news: installed, saved.
    pub const GOOD: Role = Role::custom("tf-good");
    /// Alarm: a battery about to run out.
    pub const ALERT: Role = Role::custom("tf-alert");
}

/// The 16 basic terminal colours, picked to read on the dark log
/// background: black and blue are lightened, which a terminal's defaults
/// would leave nearly invisible here.
pub const ANSI_16: [Color32; 16] = [
    Color32::from_rgb(0x6e, 0x6e, 0x6e),
    Color32::from_rgb(0xe0, 0x6c, 0x75),
    Color32::from_rgb(0x98, 0xc3, 0x79),
    Color32::from_rgb(0xe5, 0xc0, 0x7b),
    Color32::from_rgb(0x61, 0xaf, 0xef),
    Color32::from_rgb(0xc6, 0x78, 0xdd),
    Color32::from_rgb(0x56, 0xb6, 0xc2),
    Color32::from_rgb(0xd0, 0xd0, 0xd0),
    Color32::from_rgb(0x8a, 0x8a, 0x8a),
    Color32::from_rgb(0xff, 0x7b, 0x86),
    Color32::from_rgb(0xb5, 0xe8, 0x90),
    Color32::from_rgb(0xff, 0xd7, 0x87),
    Color32::from_rgb(0x82, 0xc4, 0xff),
    Color32::from_rgb(0xe0, 0x9c, 0xf5),
    Color32::from_rgb(0x7f, 0xd8, 0xe3),
    Color32::from_rgb(0xff, 0xff, 0xff),
];

// Text sizes in points, one per place they are used.
/// "ThinFloe" in the header.
pub const NAME_PT: f32 = 34.0;
/// The byline under the name.
pub const BYLINE_PT: f32 = 14.0;
/// Host name and clock, top right.
pub const HEADER_PT: f32 = 16.0;
/// The running timer of the save dialog.
pub const TIMER_PT: f32 = 28.0;
/// The Settings screen's title.
pub const SETTINGS_TITLE_PT: f32 = 28.0;
/// An add-on's name in a Settings row.
pub const ROW_NAME_PT: f32 = 20.0;
/// The source address under an add-on's name.
pub const ROW_SOURCE_PT: f32 = 11.0;
/// The status line of a Settings row.
pub const ROW_STATUS_PT: f32 = 12.0;
/// A tile's name.
pub const TILE_NAME_PT: f32 = 21.0;
/// A tile's second line: its reason or comment.
pub const TILE_NOTE_PT: f32 = 11.0;
/// The "×2" of a tile whose program runs more than once.
pub const RUNNING_PT: f32 = 15.0;
/// The small marks on a tile ("update", "user").
pub const MARK_PT: f32 = 11.0;
/// A dialog's title.
pub const DIALOG_TITLE_PT: f32 = 24.0;

/// A block heading on the Upgrade screen (Release, Target, ...).
pub const SECTION_PT: f32 = 18.0;

/// The text and spacing of a status element, in points, for one cell
/// height (`status::sizes_for` interpolates between the two sets).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusSizes {
    /// A label in Geo (a battery's kind, "Time").
    pub label: f32,
    /// A number or value in the body font.
    pub value: f32,
    /// Axis ticks, a unit caption and a legend, in the monospace font.
    pub axis: f32,
    /// One line.
    pub row: f32,
    /// Left and right of the content, and above and below it.
    pub pad_x: f32,
    pub pad_y: f32,
    /// Between two columns (label and value, symbol and percent).
    pub gap: f32,
}

/// The cell height of the full tile (`ui::TILE_H`) and its sizes.
pub const STATUS_CELL_LARGE_H: f32 = 76.0;
pub const STATUS_LARGE: StatusSizes = StatusSizes {
    label: 15.0,
    value: 12.0,
    axis: 10.0,
    row: 16.0,
    pad_x: 12.0,
    pad_y: 6.0,
    gap: 6.0,
};

/// The smallest cell height (`ui::TILE_H_MIN`) and its sizes.
pub const STATUS_CELL_SMALL_H: f32 = 64.0;
pub const STATUS_SMALL: StatusSizes = StatusSizes {
    label: 13.0,
    value: 11.0,
    axis: 9.0,
    row: 14.0,
    pad_x: 10.0,
    pad_y: 5.0,
    gap: 5.0,
};

/// The label column of a form row on the Settings screen's Screen tab
/// (points): "Turn the screen off after" and a gap.
pub const FORM_LABEL_W: f32 = 240.0;

/// The widest an address field on the Network tab gets (points).
pub const NET_FIELD_W: f32 = 320.0;

/// The tallest the list of networks in range gets in the add dialog
/// (points); a longer list scrolls.
pub const NET_LIST_H: f32 = 240.0;

/// The Upgrade screen's Choose page puts the drives beside Action and
/// Options when its panel is at least this wide (points); narrower, they
/// stack in one column.
pub const UPGRADE_TWO_COLUMNS_W: f32 = 720.0;

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

/// A drive or an action on the Upgrade screen: a tile-coloured row with
/// a border, the chosen one (`Item::selected`) with the ice border.
pub const CHOICE_ROW: AppVariant<ItemVariant> = ItemVariant::app("tf-choice");

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
    // One look in both modes, so a light system changes nothing. lyra
    // because it is square in every component, as in the mockups (owner,
    // 2026-11-06); radius 0 alone would not do that for every style.
    // lyra's switch alone is round (`rounded-full`); square like the rest.
    let mut style = Style::lyra();
    style.switch.radius.set(Radius::Rung(Rung::None));
    style.switch_thumb.radius.set(Radius::Rung(Rung::None));
    shadcn_egui::Theme::new(style, p, p, 0.0)
        .fonts(&UBUNTU_LIGHT, None)
        .color(role::TILE_ACTIVE, TILE_ACTIVE, TILE_ACTIVE)
        .color(role::TIME, TIME, TIME)
        .color(role::WARN, WARN, WARN)
        .color(role::GOOD, GOOD, GOOD)
        .color(role::ALERT, RED, RED)
        .variant(CHOICE_ROW, ItemVariant::Outline, |r, _| {
            r.item.bg.set(Paint::Role(Role::Card, 1.0));
            r.item.bg.on(State::HOVER, Paint::Role(Role::Accent, 1.0));
            r.item.bg.on(State::SELECTED, Paint::Role(role::TILE_ACTIVE, 1.0));
            r.item.border_color.on(State::SELECTED, Paint::Role(Role::Primary, 1.0));
        })
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

    /// A colour or size written into UI code instead of taken from the
    /// theme or a named constant (the build-ui skill's grep, plus `geo(`).
    fn style_literal(line: &str) -> bool {
        const PLAIN: [&str; 3] = ["Color32::from_", "from_black_alpha", "from_white_alpha"];
        if PLAIN.iter().any(|p| line.contains(p)) {
            return true;
        }
        // A named colour (`Color32::GRAY`); TRANSPARENT is no colour.
        let named = line.match_indices("Color32::").any(|(i, p)| {
            let rest = &line[i + p.len()..];
            rest.starts_with(|c: char| c.is_ascii_uppercase()) && !rest.starts_with("TRANSPARENT")
        });
        if named {
            return true;
        }
        let digit_after = |at: usize| line[at..].starts_with(|c: char| c.is_ascii_digit());
        let call_with_digit = |pat: &str| line.match_indices(pat).any(|(i, _)| digit_after(i + pat.len()));
        if call_with_digit(".size(") || call_with_digit("geo(") {
            return true;
        }
        // FontId::<word>(<digit>
        line.match_indices("FontId::").any(|(i, p)| {
            let rest = &line[i + p.len()..];
            let word = rest
                .find(|c: char| !(c.is_ascii_lowercase() || c == '_'))
                .unwrap_or(rest.len());
            word > 0 && rest[word..].starts_with('(') && digit_after(i + p.len() + word + 1)
        })
    }

    #[test]
    fn ui_code_has_no_style_literals() {
        // Positive control: the matcher sees what it must.
        for bad in [
            "Color32::from_rgb(1,2,3)",
            "Color32::WHITE",
            "from_black_alpha(9)",
            "FontId::proportional(11.0)",
            ".size(28.0)",
            "brand::geo(34.0)",
            "geo(21.0)",
        ] {
            assert!(style_literal(bad), "{bad}");
        }
        for good in [
            "brand::geo(NAME_PT)",
            "FontId::proportional(LOG_PT)",
            ".size(t.metrics.icon)",
        ] {
            assert!(!style_literal(good), "{good}");
        }
        for (name, src) in [
            ("ui.rs", include_str!("ui.rs")),
            ("app.rs", include_str!("app.rs")),
            ("header.rs", include_str!("header.rs")),
            ("desktop.rs", include_str!("desktop.rs")),
            ("settings.rs", include_str!("settings.rs")),
            ("upgrade_ui.rs", include_str!("upgrade_ui.rs")),
            ("network_ui.rs", include_str!("network_ui.rs")),
            ("monitors_ui.rs", include_str!("monitors_ui.rs")),
            ("status/mod.rs", include_str!("status/mod.rs")),
            ("status/battery.rs", include_str!("status/battery.rs")),
            ("status/clock.rs", include_str!("status/clock.rs")),
            ("status/network.rs", include_str!("status/network.rs")),
            ("status/network_info.rs", include_str!("status/network_info.rs")),
            ("status/layout.rs", include_str!("status/layout.rs")),
        ] {
            let code = src.split("#[cfg(test)]").next().unwrap();
            let hits = style_literal_lines(name, code);
            assert!(hits.is_empty(), "style literals:\n{}", hits.join("\n"));
        }
    }

    /// The lines of `code` that hold a style literal, except one under a
    /// `// token-ok:` comment. A call rustfmt broke after its `(` is
    /// joined with the next line, so `.size(\n 28.0)` counts too.
    fn style_literal_lines(name: &str, code: &str) -> Vec<String> {
        let lines: Vec<&str> = code.lines().collect();
        let ok_under = |i: usize| i > 0 && lines[i - 1].trim_start().starts_with("// token-ok:");
        let mut hits = Vec::new();
        for (n, line) in lines.iter().enumerate() {
            let joined = n > 0 && lines[n - 1].trim_end().ends_with('(');
            let text = if joined {
                format!("{}{}", lines[n - 1].trim(), line.trim_start())
            } else {
                line.to_string()
            };
            let ok = ok_under(n) || (joined && ok_under(n - 1));
            if !ok && style_literal(&text) {
                hits.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
        hits
    }

    #[test]
    fn the_literal_scan_sees_named_colours_and_broken_calls() {
        for bad in ["Color32::GRAY", "Color32::RED", "Color32::LIGHT_GRAY"] {
            assert!(style_literal(bad), "{bad}");
        }
        assert!(!style_literal("Color32::TRANSPARENT"));
        assert_eq!(style_literal_lines("x", "a.size(\n    28.0,\n)").len(), 1);
        assert_eq!(style_literal_lines("x", "FontId::new(\n    11.0,\n    f)").len(), 1);
        assert!(style_literal_lines("x", "// token-ok: why\na.size(\n    28.0,\n)").is_empty());
        assert!(style_literal_lines("x", "a.size(\n    NAME_PT,\n)").is_empty());
    }

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
        let app = [role::TILE_ACTIVE, role::TIME, role::WARN, role::GOOD, role::ALERT].map(|r| t.role(r));
        assert_eq!(app, [TILE_ACTIVE, TIME, WARN, GOOD, RED]);
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
            assert!(
                !geo.iter().any(|n| n.starts_with("shadcn-") || n.contains("oxanium")),
                "{geo:?}"
            );
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

    /// The fonts a pass draws with are the ones in force inside it; the
    /// first pass after `apply` must already have Geo and Ubuntu Light.
    #[test]
    fn the_first_pass_draws_in_the_theme_fonts() {
        let ctx = Context::default();
        apply(&ctx);
        let mut seen = None;
        ctx.run_ui(Default::default(), |ui| {
            seen = Some(ui.ctx().fonts(|f| {
                let d = f.definitions();
                let geo = d.families[&FontFamily::Name(GEO.into())][0].clone();
                let body = &d.families[&FontFamily::Proportional][0];
                let ubuntu = (*d.font_data[body].font).as_ref() == epaint_default_fonts::UBUNTU_LIGHT;
                (geo, ubuntu)
            }));
        })
        .textures_delta
        .clear();
        assert_eq!(seen, Some(("Geo-Oblique".to_string(), true)));
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
        // Both palettes are the same, so the colours alone pass in light
        // mode too; lyra's `dark:` rules (the tick box fill, among others)
        // apply only when the mode itself stays dark.
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        assert!(shadcn_egui::Theme::current(&ctx).is_dark());
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
