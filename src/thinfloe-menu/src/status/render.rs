//! Render a status element's SVG in-process with resvg. The SVG comes from
//! the menu itself (names from devices escaped), so it needs no sandbox,
//! unlike user icons. Its text uses the menu's own fonts: Geo for labels,
//! egui's monospace Hack for numbers (`style`), and egui's proportional sans
//! and emoji fonts for glyphs those lack.

use crate::brand::GEO_TTF;
use egui::{ColorImage, FontDefinitions, FontFamily};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{self, fontdb};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Once, OnceLock};

/// The family names inside the font files; `fonts_have_the_named_families`
/// holds them to the files.
pub const GEO_FAMILY: &str = "Geo";
pub const SANS_FAMILY: &str = "Ubuntu";
/// egui's monospace font, for every number, unit and address.
pub const MONO_FAMILY: &str = "Hack";
/// No element is drawn larger than this many pixels a side.
pub const MAX_SIDE: usize = 8192;

/// The font database, built once: Geo, then the fonts of egui's
/// proportional family in egui's order. It points at the font bytes in the
/// binary; only a font egui holds on the heap (none today) is copied.
pub fn fonts() -> Arc<fontdb::Database> {
    static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_font_source(fontdb::Source::Binary(Arc::new(GEO_TTF)));
        let defs = FontDefinitions::default();
        // The monospace fonts too: Hack draws the numbers (`style`).
        let mut loaded = Vec::new();
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            for name in &defs.families[&family] {
                if loaded.contains(name) {
                    continue;
                }
                if let Some(data) = defs.font_data.get(name) {
                    db.load_font_source(fontdb::Source::Binary(match &data.font {
                        Cow::Borrowed(bytes) => Arc::new(*bytes),
                        Cow::Owned(bytes) => Arc::new(bytes.clone()),
                    }));
                    loaded.push(name.clone());
                }
            }
        }
        db.set_sans_serif_family(SANS_FAMILY);
        Arc::new(db)
    })
    .clone()
}

/// What usvg parses a status SVG with: the menu's fonts, and no `<image>`.
/// usvg's default resolver reads any path it is given and decodes `data:`
/// URIs of any size (see `svg_options` in `icons.rs`); no element draws an
/// image, and a name from a device must never be able to ask for one.
pub fn options() -> usvg::Options<'static> {
    let mut opt = usvg::Options {
        fontdb: fonts(),
        font_family: SANS_FAMILY.to_string(),
        ..usvg::Options::default()
    };
    opt.image_href_resolver.resolve_string = Box::new(|_, _| None);
    opt.image_href_resolver.resolve_data = Box::new(|_, _, _| None);
    opt
}

thread_local! {
    /// Set while `guarded` runs on this thread: the panic hook then writes
    /// nothing and leaves its text in `CAUGHT`.
    static QUIET: Cell<bool> = const { Cell::new(false) };
    static CAUGHT: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let before = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if QUIET.get() {
                CAUGHT.set(Some(info.to_string().replace('\n', " ")));
            } else {
                before(info);
            }
        }));
    });
}

/// Run `f`, with a panic in it as an error: a panic in resvg or usvg costs
/// one element its picture, not the menu with exit 101. The default hook
/// would print it on every attempt, each second for a graph, so it stays
/// quiet here and the caller decides how often to log.
pub fn guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    install_panic_hook();
    QUIET.set(true);
    let got = panic::catch_unwind(AssertUnwindSafe(f));
    QUIET.set(false);
    got.unwrap_or_else(|_| Err(CAUGHT.take().unwrap_or_else(|| "panicked".to_string())))
}

/// Draw `svg` scaled to fill `px` (width, height) pixels.
pub fn render(svg: &str, px: [usize; 2]) -> Result<ColorImage, String> {
    if px[0] == 0 || px[1] == 0 || px[0] > MAX_SIDE || px[1] > MAX_SIDE {
        return Err(format!("no picture of {} x {} pixels", px[0], px[1]));
    }
    guarded(|| {
        let tree = usvg::Tree::from_str(svg, &options()).map_err(|e| e.to_string())?;
        let size = tree.size();
        let mut pix = Pixmap::new(px[0] as u32, px[1] as u32).ok_or("no pixmap")?;
        let (sx, sy) = (px[0] as f32 / size.width(), px[1] as f32 / size.height());
        resvg::render(&tree, Transform::from_scale(sx, sy), &mut pix.as_mut());
        Ok(ColorImage::from_rgba_premultiplied(px, pix.data()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_have_the_named_families() {
        let db = fonts();
        let has = |family: &str| db.faces().any(|f| f.families.iter().any(|(name, _)| name == family));
        assert!(has(GEO_FAMILY), "Geo is loaded under its family name");
        assert!(has(SANS_FAMILY), "the sans is loaded under its family name");
        assert!(has(MONO_FAMILY), "the monospace font is loaded under its family name");
    }

    fn ink(img: &ColorImage) -> usize {
        img.pixels.iter().filter(|p| p.a() > 0).count()
    }

    #[test]
    fn text_in_the_menu_fonts_draws_ink_at_the_asked_size() {
        for family in [GEO_FAMILY, SANS_FAMILY] {
            let svg = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"20\" viewBox=\"0 0 100 20\">\
                 <text x=\"2\" y=\"15\" font-family=\"{family}\" font-size=\"14\" fill=\"#fff\">Keyboard 100 %</text></svg>"
            );
            let img = render(&svg, [200, 40]).unwrap();
            assert_eq!(img.size, [200, 40]);
            assert!(ink(&img) > 200, "{family}: {} pixels of ink", ink(&img));
        }
        // Text without a family draws in the sans.
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"50\" height=\"20\">\
                   <text x=\"2\" y=\"15\" font-size=\"14\">Mouse</text></svg>";
        assert!(ink(&render(svg, [50, 20]).unwrap()) > 50);
    }

    #[test]
    fn geo_and_the_sans_draw_differently() {
        let draw = |family: &str| {
            render(
                &format!(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"20\">\
                     <text x=\"2\" y=\"15\" font-family=\"{family}\" font-size=\"14\" fill=\"#fff\">Keyboard</text></svg>"
                ),
                [100, 20],
            )
            .unwrap()
            .pixels
        };
        assert_ne!(draw(GEO_FAMILY), draw(SANS_FAMILY));
    }

    #[test]
    fn fonts_stay_in_the_binary_and_are_not_copied() {
        let db = fonts();
        let mut seen = 0;
        for face in db.faces() {
            let fontdb::Source::Binary(data) = &face.source;
            let bytes: &[u8] = (**data).as_ref();
            if face.families.iter().any(|(name, _)| name == GEO_FAMILY) {
                assert_eq!(bytes.as_ptr(), GEO_TTF.as_ptr(), "Geo was copied");
                seen += 1;
            }
            if face.families.iter().any(|(name, _)| name == MONO_FAMILY) {
                let defs = FontDefinitions::default();
                assert_eq!(bytes.as_ptr(), defs.font_data["Hack"].font.as_ptr(), "Hack was copied");
                seen += 1;
            }
        }
        assert_eq!(seen, 2);
    }

    #[test]
    fn an_embedded_or_linked_image_is_not_loaded() {
        // A red square, which usvg's default resolvers load and draw.
        const SQUARE: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><rect width=\"10\" height=\"10\" fill=\"red\"/></svg>";
        const BASE64: &str = "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxMCIgaGVpZ2h0PSIxMCI+PHJlY3Qgd2lkdGg9IjEwIiBoZWlnaHQ9IjEwIiBmaWxsPSJyZWQiLz48L3N2Zz4=";
        let svg = |href: &str| {
            format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
                 width=\"10\" height=\"10\"><image width=\"10\" height=\"10\" xlink:href=\"{href}\"/></svg>"
            )
        };
        let img = render(&svg(&format!("data:image/svg+xml;base64,{BASE64}")), [10, 10]).unwrap();
        assert_eq!(ink(&img), 0, "a data: image was drawn");
        let d = crate::testdir::TestDir::new("render", "image");
        let file = d.join("square.svg");
        std::fs::write(&file, SQUARE).unwrap();
        let img = render(&svg(file.to_str().unwrap()), [10, 10]).unwrap();
        assert_eq!(ink(&img), 0, "a file was read");
    }

    #[test]
    fn a_panic_in_the_renderer_is_an_error_for_that_element() {
        let got: Result<(), String> = guarded(|| panic!("boom in resvg"));
        let e = got.unwrap_err();
        assert!(e.contains("boom in resvg"), "{e}");
        // What it guards still gets through, and the next call works.
        assert_eq!(guarded(|| Ok(5)), Ok(5));
        assert_eq!(guarded::<u8>(|| Err("no".to_string())), Err("no".to_string()));
        assert!(!QUIET.get(), "the panic hook is back to normal for this thread");
    }

    #[test]
    fn odd_sizes_and_bad_svg_are_errors() {
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"/>";
        assert!(render(svg, [0, 10]).is_err());
        assert!(render(svg, [MAX_SIDE + 1, 10]).is_err());
        assert!(render("<svg", [10, 10]).is_err());
        assert_eq!(render(svg, [10, 10]).unwrap().size, [10, 10]);
    }
}
