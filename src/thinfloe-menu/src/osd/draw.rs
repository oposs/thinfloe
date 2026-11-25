//! Draws the OSD panel and the muted-microphone mark: egui laid out
//! offscreen and rasterised on the CPU into a `raster::Canvas` the size
//! of the surface's `Placement`.

use egui::epaint::StrokeKind;
use egui::text::LayoutJob;
use egui::{Align2, Color32, Pos2, Rect, Stroke, TextFormat, Ui, Vec2, ViewportId};
use shadcn_egui::Theme;
use shadcn_egui::components::icon::{Icon, paint_icon};
use shadcn_egui::icons::lucide;

use super::surface::{Placement, mark_placement, note_placement, panel_placement};
use crate::brand::{
    self, MARK_ICON, OSD_BAR_GAP, OSD_BAR_H, OSD_GAP, OSD_ICON, OSD_NOTE_MAX_W, OSD_NOTE_ROWS, OSD_NOTE_SIDE,
    OSD_PAD_X, OSD_PAD_Y, OSD_TEXT_PT, OSD_W, role,
};
use crate::raster::{Canvas, Textures};

/// What the panel shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelView {
    Volume {
        percent: u8,
    },
    Muted {
        percent: u8,
    },
    MicOff,
    MicOn,
    Brightness {
        percent: u8,
    },
    BrightnessUnsupported,
    /// The brightness key was pressed before detection finished.
    BrightnessUnknown,
    /// A text from another program: up to `OSD_NOTE_ROWS` rows, no icon,
    /// cut with an ellipsis where it does not fit.
    Note {
        warn: bool,
        text: String,
    },
}

impl PanelView {
    /// Whether the panel has the level bar under its row.
    pub fn has_bar(&self) -> bool {
        matches!(
            self,
            PanelView::Volume { .. } | PanelView::Muted { .. } | PanelView::Brightness { .. }
        )
    }
}

impl PanelView {
    /// Where the panel goes at `scale`, for a canvas of `size` from
    /// `Painter::panel_on` (a note is as large as its text).
    pub fn placement(&self, scale: f32, size: (u32, u32)) -> Placement {
        match self {
            PanelView::Note { .. } => note_placement(scale, size),
            v => panel_placement(scale, v.has_bar()),
        }
    }
}

/// A level as the panel writes it.
pub fn percent_text(p: u8) -> String {
    format!("{p} %")
}

/// The colours one view uses, all from the theme.
struct Look {
    icon: &'static Icon,
    icon_color: Color32,
    name: &'static str,
    right: Option<(String, Color32)>,
    bar: Option<(u8, Color32)>,
}

fn look(v: &PanelView, t: &Theme) -> Look {
    let p = t.palette();
    let (ice, muted, warn) = (p.primary, p.muted_foreground, t.role(role::WARN));
    let level = |pc: u8| Some((percent_text(pc), muted));
    match v {
        PanelView::Volume { percent } => Look {
            icon: &lucide::VOLUME_2,
            icon_color: ice,
            name: "Volume",
            right: level(*percent),
            bar: Some((*percent, ice)),
        },
        PanelView::Muted { percent } => Look {
            icon: &lucide::VOLUME_X,
            icon_color: muted,
            name: "Muted",
            right: level(*percent),
            bar: Some((*percent, t.role(role::BAR_OFF))),
        },
        PanelView::MicOff => Look {
            icon: &lucide::MIC_OFF,
            icon_color: warn,
            name: "Microphone off",
            right: None,
            bar: None,
        },
        PanelView::MicOn => Look {
            icon: &lucide::MIC,
            icon_color: ice,
            name: "Microphone on",
            right: None,
            bar: None,
        },
        PanelView::Brightness { percent } => Look {
            icon: &lucide::SUN,
            icon_color: ice,
            name: "Brightness",
            right: level(*percent),
            bar: Some((*percent, ice)),
        },
        PanelView::BrightnessUnsupported => Look {
            icon: &lucide::SUN,
            icon_color: muted,
            name: "Brightness",
            right: Some(("not supported".into(), warn)),
            bar: None,
        },
        PanelView::Note { .. } => unreachable!("a note has no look; `note_ui` draws it"),
        PanelView::BrightnessUnknown => Look {
            icon: &lucide::SUN,
            icon_color: ice,
            name: "Brightness",
            right: None,
            bar: None,
        },
    }
}

/// Logical pixels as points that land on whole physical pixels, each
/// value rounded on its own as `surface` sizes the frame.
fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

/// The frame's 1 px border (the theme's border width), on whole pixels.
fn border(ui: &Ui, rect: Rect, color: Color32, scale: f32) {
    let w = snap(Theme::current(ui.ctx()).metrics.border, scale).max(1.0 / scale);
    ui.painter()
        .rect_stroke(rect, 0.0, Stroke::new(w, color), StrokeKind::Inside);
}

/// A note's text wrapped to `max_width` points: at most `OSD_NOTE_ROWS`
/// rows, broken between words where it can, the rest cut with an ellipsis.
fn note_job(text: &str, color: Color32, max_width: f32) -> LayoutJob {
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat::simple(brand::geo(OSD_TEXT_PT), color));
    job.wrap.max_width = max_width.max(1.0);
    job.wrap.max_rows = OSD_NOTE_ROWS;
    job.wrap.overflow_character = Some('\u{2026}');
    job
}

/// How a note's text fits its panel: the canvas size in pixels, the rows
/// it takes and whether it had to be cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteFit {
    pub size: (u32, u32),
    pub rows: usize,
    pub elided: bool,
}

/// The widest a note's panel may be at `scale` on an output
/// `output_width` wide (surface pixels), in pixels.
fn note_max_width(scale: f32, output_width: Option<u32>) -> f32 {
    let widest = snap(OSD_NOTE_MAX_W, scale) * scale;
    match output_width {
        Some(w) => widest.min(w as f32 - 2.0 * snap(OSD_NOTE_SIDE, scale) * scale).max(1.0),
        None => widest,
    }
}

/// A note: the frame and its text, centred between the paddings, amber
/// for a warning.
fn note_ui(ui: &mut Ui, warn: bool, text: &str, scale: f32) {
    let t = Theme::current(ui.ctx());
    let p = t.palette();
    let all = ui.ctx().content_rect();
    let s = |v: f32| snap(v, scale);
    let pad_x = s(OSD_PAD_X);
    ui.painter().rect_filled(all, 0.0, p.card);
    border(ui, all, p.border, scale);
    let color = if warn { t.role(role::WARN) } else { p.foreground };
    let galley = ui
        .painter()
        .layout_job(note_job(text, color, all.width() - 2.0 * pad_x));
    let top = s((all.height() - galley.size().y) / 2.0);
    ui.painter().galley(Pos2::new(pad_x, top), galley, color);
}

fn panel_ui(ui: &mut Ui, v: &PanelView, scale: f32) {
    if let PanelView::Note { warn, text } = v {
        return note_ui(ui, *warn, text, scale);
    }
    let t = Theme::current(ui.ctx());
    let p = t.palette();
    let l = look(v, &t);
    let all = ui.ctx().content_rect();
    let s = |v: f32| snap(v, scale);
    let (pad_x, pad_y, icon) = (s(OSD_PAD_X), s(OSD_PAD_Y), s(OSD_ICON));
    let painter = ui.painter();
    painter.rect_filled(all, 0.0, p.card);
    border(ui, all, p.border, scale);

    let icon_rect = Rect::from_min_size(Pos2::new(pad_x, pad_y), Vec2::splat(icon));
    paint_icon(painter, icon_rect, l.icon, l.icon_color, t.metrics.icon_stroke);
    let mid = pad_y + icon / 2.0;
    let font = brand::geo(OSD_TEXT_PT);
    let name_x = pad_x + icon + s(OSD_GAP);
    painter.text(
        Pos2::new(name_x, mid),
        Align2::LEFT_CENTER,
        l.name,
        font.clone(),
        p.foreground,
    );
    if let Some((text, color)) = l.right {
        let right = Pos2::new(all.max.x - pad_x, mid);
        painter.text(right, Align2::RIGHT_CENTER, text, font, color);
    }
    if let Some((percent, fill)) = l.bar {
        let top = pad_y + icon + s(OSD_BAR_GAP);
        let track = Rect::from_min_max(Pos2::new(pad_x, top), Pos2::new(all.max.x - pad_x, top + s(OSD_BAR_H)));
        painter.rect_filled(track, 0.0, p.border);
        let share = f32::from(percent.min(100)) / 100.0;
        let filled = s(track.width() * share);
        if filled > 0.0 {
            let bar = Rect::from_min_size(track.min, Vec2::new(filled, track.height()));
            painter.rect_filled(bar, 0.0, fill);
        }
    }
}

fn mark_ui(ui: &mut Ui, scale: f32) {
    let t = Theme::current(ui.ctx());
    let warn = t.role(role::WARN);
    let all = ui.ctx().content_rect();
    ui.painter().rect_filled(all, 0.0, t.palette().card);
    border(ui, all, warn, scale);
    let icon = snap(MARK_ICON, scale);
    let off = ((all.width() - icon) / 2.0 * scale).round() / scale;
    let rect = Rect::from_min_size(Pos2::new(off, off), Vec2::splat(icon));
    paint_icon(ui.painter(), rect, &lucide::MIC_OFF, warn, t.metrics.icon_stroke);
}

/// One egui context, themed once, and the textures it has handed out.
pub struct Painter {
    ctx: egui::Context,
    textures: Textures,
}

impl Default for Painter {
    fn default() -> Self {
        Self::new()
    }
}

impl Painter {
    pub fn new() -> Painter {
        let ctx = egui::Context::default();
        brand::apply(&ctx);
        // As app.rs: hard edges for the integer rasteriser, no animation.
        ctx.options_mut(|o| o.tessellation_options.feathering = false);
        ctx.global_style_mut(|s| s.animation_time = 0.0);
        Painter {
            ctx,
            textures: Textures::default(),
        }
    }

    /// The panel for `v` at `scale`, on an output of unknown width.
    pub fn panel(&mut self, v: &PanelView, scale: f32) -> Canvas {
        self.panel_on(v, scale, None)
    }

    /// The panel for `v` at `scale` on an output `output_width` wide: as
    /// large as `panel_placement` says, a note as large as `note_fit`.
    pub fn panel_on(&mut self, v: &PanelView, scale: f32, output_width: Option<u32>) -> Canvas {
        let size = match v {
            PanelView::Note { text, .. } => self.note_fit(text, scale, output_width).size,
            v => panel_placement(scale, v.has_bar()).size,
        };
        self.render(size, scale, |ui| panel_ui(ui, v, scale))
    }

    /// The size of a note's panel for `text`: as wide as the text and its
    /// paddings, at least `OSD_W` and at most `note_max_width`; one row
    /// is as high as a key's panel without a bar, more rows add theirs.
    pub fn note_fit(&mut self, text: &str, scale: f32, output_width: Option<u32>) -> NoteFit {
        let s = |v: f32| snap(v, scale);
        let (pad_x, pad_y, icon) = (s(OSD_PAD_X), s(OSD_PAD_Y), s(OSD_ICON));
        let widest = note_max_width(scale, output_width);
        let narrowest = (s(OSD_W) * scale).min(widest);
        let max_text = widest / scale - 2.0 * pad_x;
        let mut measured = (Vec2::ZERO, 0, false);
        let size = (widest.ceil() as u32, panel_placement(scale, false).size.1);
        self.measure(size, scale, |ui| {
            let g = ui.painter().layout_job(note_job(text, Color32::TRANSPARENT, max_text));
            measured = (g.size(), g.rows.len(), g.elided);
        });
        let (text_size, rows, elided) = measured;
        let w = ((text_size.x + 2.0 * pad_x) * scale).ceil().clamp(narrowest, widest);
        let text_h = if rows > 1 { text_size.y.max(icon) } else { icon };
        let h = ((2.0 * pad_y + text_h) * scale).round();
        NoteFit {
            size: (w as u32, h as u32),
            rows,
            elided,
        }
    }

    /// The muted-microphone mark at `scale`, as large as `mark_placement`.
    pub fn mark(&mut self, scale: f32) -> Canvas {
        let size = mark_placement(scale).size;
        self.render(size, scale, |ui| mark_ui(ui, scale))
    }

    /// One pass of `draw` that is not rasterised: for laying out text.
    fn measure(&mut self, size: (u32, u32), scale: f32, mut draw: impl FnMut(&mut Ui)) {
        let mut out = self.ctx.run_ui(Self::input(size, scale), |ui| draw(ui));
        // The font atlas may have grown: keep the textures in step.
        self.textures.apply_set(&out.textures_delta);
        self.textures.apply_free(&out.textures_delta);
        out.textures_delta.clear();
    }

    fn input(size: (u32, u32), scale: f32) -> egui::RawInput {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(size.0 as f32, size.1 as f32) / scale,
            )),
            ..Default::default()
        };
        input
            .viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(scale);
        input
    }

    fn render(&mut self, size: (u32, u32), scale: f32, mut draw: impl FnMut(&mut Ui)) -> Canvas {
        let (w, h) = (size.0 as usize, size.1 as usize);
        let mut out = self.ctx.run_ui(Self::input(size, scale), |ui| draw(ui));
        let ppp = out.pixels_per_point;
        self.textures.apply_set(&out.textures_delta);
        let prims = self.ctx.tessellate(out.shapes, ppp);
        let mut canvas = Canvas::new(w, h, Theme::current(&self.ctx).palette().card);
        let all = canvas.bounds();
        canvas.draw(&prims, &self.textures, ppp, all);
        self.textures.apply_free(&out.textures_delta);
        out.textures_delta.clear();
        canvas
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brand::rgb::{BAR_OFF, ICE, LINE, TILE, WARN};

    fn px(c: &Canvas, x: usize, y: usize) -> Color32 {
        let p = c.pixels[y * c.width + x];
        Color32::from_rgb((p >> 16) as u8, (p >> 8) as u8, p as u8)
    }

    /// Whether any pixel in `x0..x1` by `y0..y1` is `want`.
    fn any(c: &Canvas, (x0, x1): (usize, usize), (y0, y1): (usize, usize), want: Color32) -> bool {
        (y0..y1).any(|y| (x0..x1).any(|x| px(c, x, y) == want))
    }

    #[test]
    fn the_longest_stick_note_is_not_cut() {
        use crate::osd::notes::{NAME_MAX, Note, text};
        let name = "W".repeat(NAME_MAX);
        for n in [
            Note::Connected(name.clone()),
            Note::Removable(name.clone()),
            Note::Unclean(name),
        ] {
            let (t, _) = text(&n);
            let fit = Painter::new().note_fit(&t, 1.0, None);
            assert!(!fit.elided, "{t:?} is cut");
        }
    }

    #[test]
    fn volume_panel_has_the_size_and_an_ice_bar_to_the_level() {
        let c = Painter::new().panel(&PanelView::Volume { percent: 50 }, 1.0);
        assert_eq!((c.width, c.height), (320, 61));
        let bar_y = 61 - 1 - 12 - 3; // middle of the 6 px bar
        assert_eq!(px(&c, 17 + 10, bar_y), ICE); // inside the filled half
        assert_eq!(px(&c, 320 - 17 - 10, bar_y), LINE); // the empty track
        assert_eq!(px(&c, 0, 30), LINE); // border
        assert_eq!(px(&c, 3, 3), TILE); // fill
    }

    #[test]
    fn muted_bar_is_grey() {
        let c = Painter::new().panel(&PanelView::Muted { percent: 50 }, 1.0);
        assert_eq!(px(&c, 17 + 10, 61 - 1 - 12 - 3), BAR_OFF);
    }

    #[test]
    fn mic_off_has_no_bar_and_an_amber_icon() {
        let c = Painter::new().panel(&PanelView::MicOff, 1.0);
        assert_eq!(c.height, 46);
        assert!(any(&c, (17, 39), (12, 34), WARN));
        assert!(!any(&c, (17, 39), (12, 34), ICE));
    }

    #[test]
    fn unsupported_brightness_says_so_in_warn() {
        let c = Painter::new().panel(&PanelView::BrightnessUnsupported, 1.0);
        assert_eq!(c.height, 46);
        assert!(any(&c, (160, 320), (0, 46), WARN));
    }

    #[test]
    fn scale_two_doubles_the_panel() {
        let c = Painter::new().panel(&PanelView::Volume { percent: 45 }, 2.0);
        assert_eq!((c.width, c.height), (640, 122));
    }

    #[test]
    fn the_canvas_is_the_placement_at_a_fractional_scale() {
        let mut p = Painter::new();
        let c = p.panel(&PanelView::Brightness { percent: 70 }, 1.25);
        assert_eq!((c.width as u32, c.height as u32), panel_placement(1.25, true).size);
        let m = p.mark(1.25);
        assert_eq!((m.width as u32, m.height as u32), mark_placement(1.25).size);
    }

    #[test]
    fn mark_is_square_with_a_warn_border() {
        let c = Painter::new().mark(1.0);
        assert_eq!((c.width, c.height), (30, 30));
        assert_eq!(px(&c, 0, 15), WARN);
        assert_eq!(px(&c, 3, 3), TILE);
        assert!(any(&c, (6, 24), (6, 24), WARN));
    }

    fn note(warn: bool, text: &str) -> PanelView {
        PanelView::Note {
            warn,
            text: text.into(),
        }
    }

    #[test]
    fn a_short_note_panel_is_one_row_high() {
        let mut p = Painter::new();
        let fit = p.note_fit("From Anna: a.pdf", 1.0, None);
        assert_eq!(fit.rows, 1);
        assert!(!fit.elided);
        // As high as a key's panel without a bar, at least as wide.
        assert_eq!(fit.size.1, panel_placement(1.0, false).size.1);
        assert!(fit.size.0 >= panel_placement(1.0, false).size.0);
        let c = p.panel(&note(false, "From Anna: a.pdf"), 1.0);
        assert_eq!((c.width as u32, c.height as u32), fit.size);
        assert_eq!(c.height, 46);
        // The text is drawn, in the foreground colour, left of the middle.
        let fg = Theme::current(&Painter::new().ctx).palette().foreground;
        assert!(any(&c, (16, 200), (10, 36), fg));
    }

    #[test]
    fn a_warn_note_is_amber() {
        let c = Painter::new().panel(&note(true, "No space left"), 1.0);
        assert!(any(&c, (16, 200), (10, 36), WARN));
        let c = Painter::new().panel(&note(false, "No space left"), 1.0);
        assert!(!any(&c, (16, 200), (10, 36), WARN));
    }

    /// The longest texts `thinfloe-share` sends, with a long sender name.
    const LONGEST: [&str; 3] = [
        "Files from Jane Appleseed's iPhone 16 Pro Max refused: the FloeDrop folder is full",
        "IMG_20261125_154233_0042.HEIC received from Jane Appleseed's iPhone 16 Pro Max",
        "Files from Jane Appleseed's iPhone 16 Pro Max could not be received",
    ];

    #[test]
    fn the_longest_floedrop_notes_show_whole() {
        let mut p = Painter::new();
        for scale in [1.0, 1.5, 2.0] {
            for text in LONGEST {
                let fit = p.note_fit(text, scale, Some(1280));
                assert!(!fit.elided, "{text:?} cut at scale {scale}");
                assert!(fit.rows <= OSD_NOTE_ROWS);
            }
        }
        // A two-row note is higher than one row and fits its canvas.
        let two = format!("{} {}", LONGEST[0], LONGEST[1]);
        let fit = p.note_fit(&two, 1.0, None);
        assert_eq!(fit.rows, 2);
        assert!(fit.size.1 > panel_placement(1.0, false).size.1);
        let c = p.panel(&note(true, &two), 1.0);
        assert_eq!((c.width as u32, c.height as u32), fit.size);
        // Both rows are drawn: amber text in the upper and the lower half.
        let half = c.height / 2;
        assert!(any(&c, (16, c.width - 16), (4, half), WARN));
        assert!(any(&c, (16, c.width - 16), (half, c.height - 4), WARN));
    }

    #[test]
    fn a_note_panel_stays_inside_a_narrow_output() {
        let mut p = Painter::new();
        let fit = p.note_fit(LONGEST[0], 2.0, Some(1024));
        assert!(fit.size.0 <= 1024 - 2 * 44, "{fit:?}");
        let two = format!("{} {}", LONGEST[0], LONGEST[1]);
        let wide = p.note_fit(&two, 1.0, Some(3840));
        assert_eq!(wide.rows, 2);
        assert!(wide.size.0 <= 720, "{wide:?}");
    }

    #[test]
    fn a_long_note_is_cut_inside_the_panel() {
        let fit = Painter::new().note_fit(&"x".repeat(200), 1.0, None);
        assert!(fit.elided);
        assert_eq!(fit.rows, OSD_NOTE_ROWS);
        let long = "x".repeat(200);
        let c = Painter::new().panel(&note(false, &long), 1.0);
        let card = TILE;
        // The right padding (and the bottom row below the text) holds no text pixel.
        let right = (c.width - 16, c.width - 1);
        for y in 1..c.height - 1 {
            for x in right.0..right.1 {
                assert_eq!(px(&c, x, y), card, "text in the padding at {x},{y}");
            }
        }
        // The same at a fractional scale.
        let c = Painter::new().panel(&note(false, &long), 1.5);
        for y in 1..c.height - 1 {
            for x in (c.width - 24)..(c.width - 1) {
                // The frame's border may be two pixels wide at this scale.
                assert!([card, LINE].contains(&px(&c, x, y)), "text in the padding at {x},{y}");
            }
        }
    }

    #[test]
    fn percent_text_has_a_space_before_the_sign() {
        assert_eq!(percent_text(45), "45 %");
    }

    /// Writes PNG files of every panel state and the mark, for looking at.
    /// Run by hand: `TF_SHOT_DIR=<dir> cargo test --lib osd_shots -- --ignored`.
    #[test]
    #[ignore]
    fn osd_shots() {
        let Some(dir) = std::env::var_os("TF_SHOT_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let save = |c: &Canvas, name: &str| {
            let mut pm = resvg::tiny_skia::Pixmap::new(c.width as u32, c.height as u32).unwrap();
            for (d, p) in pm.data_mut().chunks_exact_mut(4).zip(&c.pixels) {
                d.copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
            }
            pm.save_png(dir.join(name)).unwrap();
        };
        let mut p = Painter::new();
        let views = [
            ("volume", PanelView::Volume { percent: 45 }),
            ("muted", PanelView::Muted { percent: 45 }),
            ("mic-off", PanelView::MicOff),
            ("mic-on", PanelView::MicOn),
            ("brightness", PanelView::Brightness { percent: 70 }),
            ("brightness-unsupported", PanelView::BrightnessUnsupported),
            ("brightness-unknown", PanelView::BrightnessUnknown),
            ("note-info", note(false, "From Anna: holiday-photos.zip")),
            (
                "note-warn",
                note(true, "No space left for a-very-long-file-name-that-is-cut.pdf"),
            ),
            ("note-two-rows", note(true, &format!("{} {}", LONGEST[0], LONGEST[1]))),
            ("note-cut", note(false, &"IMG_0042.HEIC ".repeat(14))),
        ];
        for s in [1.0, 2.0] {
            for (name, v) in &views {
                save(&p.panel(v, s), &format!("panel-{name}-x{s}.png"));
            }
            save(&p.mark(s), &format!("mark-x{s}.png"));
        }
    }
}
