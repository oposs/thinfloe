//! Drawing the grid, the Settings screen in its place, the log under them,
//! and the dialogs. Everything that decides something lives in menu.rs; this
//! file only paints it and reports clicks.

use crate::brand::{self, GOOD, ICE, MUTED, TILE, TILE_ACTIVE, TILE_HOVER, TIME};
use crate::density::GROUP_W;
use crate::header::{Header, header_text, until_next_minute};
use crate::icons::{self, IconJob, IconRoots};
use crate::log::{Ansi, Kind, Line, Log, Run};
use crate::menu::{Choice, Logout, Menu, Screen};
use crate::settings::{self, RowButton, SettingsEvent};
use crate::tiles::{Action, Origin, Tile};
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align2, Color32, ColorImage, FontId, Rect, Sense, Stroke, StrokeKind, TextFormat, TextureHandle, Ui, UiBuilder,
    Vec2, pos2, vec2,
};
use egui_shadcn::Theme;
use egui_shadcn::components::button::{Button, ButtonVariant};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Four columns of bands: the eight shipped tiles take two rows.
pub const COLUMNS: usize = 4;
/// Space around the log and between the groups of the screen.
pub const GAP: f32 = 16.0;
/// Space between two tiles.
pub const TILE_GAP: f32 = 3.0;
/// A band is this tall, down to `TILE_H_MIN` when the screen is short.
pub const TILE_H: f32 = 76.0;
pub const TILE_H_MIN: f32 = 64.0;
/// Between the tile grid and the status area below it.
pub const STATUS_GAP: f32 = 32.0;
/// A screen under this many points high is short: status elements take
/// their `ShortSize`.
pub const SHORT_SCREEN_H: f32 = 900.0;

pub fn short_screen(height: f32) -> bool {
    height < SHORT_SCREEN_H
}
/// The header: 32 above the 52 tall logo, 16 below.
pub const HEADER_H: f32 = HEADER_PAD_TOP + LOGO_H + 16.0;
const HEADER_PAD_X: f32 = 32.0;
const HEADER_PAD_TOP: f32 = 32.0;
pub const LOGO_H: f32 = 52.0;
/// The line under the name.
pub const BYLINE: &str = "by oetiker.ai";
const ICON_BOX: f32 = 44.0;
/// The folded-window strip: its height, the chip height inside it, the gap
/// to the log and the widest a chip grows.
pub const STRIP_H: f32 = 40.0;
pub const CHIP_H: f32 = 32.0;
const STRIP_GAP: f32 = 8.0;
const CHIP_MAX_W: f32 = 280.0;
const CHIP_ICON: f32 = 20.0;
const CHIP_FONT: f32 = 15.0;
const EDGE_W: f32 = 5.0;
/// The log takes this share of the screen height, and at least
/// `LOG_MIN_ROWS` rows.
pub const LOG_SHARE: f32 = 0.3;
pub const LOG_MIN_ROWS: usize = 8;
/// One log row, in points; the log draws one line per row.
pub const LOG_ROW: f32 = 18.0;
const LOG_PAD: f32 = 10.0;
const LOG_FONT: f32 = 13.0;
pub use crate::brand::WARN;

/// The side of an icon drawn `points` tall, in whole pixels.
fn icon_px(points: f32, ppp: f32) -> usize {
    ((points * ppp).round() as usize).max(1)
}

/// The side the renderer draws an icon at: twice the tile's icon, so that
/// shrinking it to each size it is drawn at averages several samples per
/// pixel.
pub fn render_side(ppp: f32) -> u32 {
    (2 * icon_px(ICON_BOX, ppp)).min(1024) as u32
}

/// One icon at each size it is drawn at: the tile, the Settings row and the
/// chip. The rasteriser samples a texture at the nearest texel, so a texture
/// drawn at any other size than its own drops or doubles rows and columns.
#[derive(Clone)]
pub struct Icon {
    pub tile: TextureHandle,
    pub row: TextureHandle,
    pub chip: TextureHandle,
}

impl Icon {
    pub fn new(ctx: &egui::Context, name: &str, img: &ColorImage, ppp: f32) -> Icon {
        let load = |suffix: &str, points: f32| {
            ctx.load_texture(
                format!("{name}#{suffix}"),
                shrink(img, icon_px(points, ppp)),
                egui::TextureOptions::LINEAR,
            )
        };
        Icon {
            tile: load("tile", ICON_BOX),
            row: load("row", ROW_ICON),
            chip: load("chip", CHIP_ICON),
        }
    }
}

/// `img`, square, scaled to `side` pixels by averaging the area each new
/// pixel covers. The colours are premultiplied, so averaging them is right.
pub fn shrink(img: &ColorImage, side: usize) -> ColorImage {
    let [w, h] = img.size;
    let (sx, sy) = (w as f32 / side as f32, h as f32 / side as f32);
    // For each new pixel along one axis, the source pixels it covers and
    // how much of each.
    let spans = |scale: f32, len: usize| -> Vec<Vec<(usize, f32)>> {
        (0..side)
            .map(|i| {
                let (a, b) = (i as f32 * scale, (i + 1) as f32 * scale);
                (a.floor() as usize..(b.ceil() as usize).min(len))
                    .map(|j| (j, (b.min(j as f32 + 1.0) - a.max(j as f32)).max(0.0)))
                    .filter(|&(_, f)| f > 0.0)
                    .collect()
            })
            .collect()
    };
    let (xs, ys) = (spans(sx, w), spans(sy, h));
    let mut pixels = Vec::with_capacity(side * side);
    for ry in &ys {
        for rx in &xs {
            let mut acc = [0.0f32; 4];
            let mut total = 0.0;
            for &(y, fy) in ry {
                for &(x, fx) in rx {
                    let c = img.pixels[y * w + x];
                    let f = fx * fy;
                    for (k, v) in [c.r(), c.g(), c.b(), c.a()].into_iter().enumerate() {
                        acc[k] += v as f32 * f;
                    }
                    total += f;
                }
            }
            let ch = |k: usize| (acc[k] / total).round().min(255.0) as u8;
            pixels.push(Color32::from_rgba_premultiplied(ch(0), ch(1), ch(2), ch(3)));
        }
    }
    ColorImage::new([side, side], pixels)
}

/// Draw `tex` with its top left corner at `min`, on a whole pixel, at its own
/// size in pixels.
fn draw_icon(p: &egui::Painter, tex: &TextureHandle, min: egui::Pos2, tint: Color32) {
    let ppp = p.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let [w, h] = tex.size();
    let rect = Rect::from_min_size(pos2(snap(min.x), snap(min.y)), vec2(w as f32, h as f32) / ppp);
    p.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
}

/// Each tile's icon, rasterised in renderer processes as the menu draws it:
/// `None` when the tile names no icon that resolves, otherwise the path and
/// the image or why there is none. Both draw the letter.
pub fn render_icons(
    tiles: &[Tile],
    roots: &IconRoots,
    side: u32,
) -> Vec<Option<(PathBuf, Result<ColorImage, String>)>> {
    let fg = crate::brand::theme().palette.foreground;
    let rgb = [fg.r(), fg.g(), fg.b()];
    let (index, jobs): (Vec<usize>, Vec<IconJob>) = tiles
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let path = icons::resolve(t.icon.as_deref()?, roots)?;
            let symbolic = icons::is_symbolic(&path, roots);
            Some((
                i,
                IconJob {
                    path,
                    side,
                    rgb,
                    symbolic,
                },
            ))
        })
        .unzip();
    // The renderer is this program, started with --render-icon.
    let rendered = match std::env::current_exe() {
        Ok(exe) => icons::render_all(&exe, &jobs, icons::ICON_DEADLINE, icons::ICONS_BUDGET),
        Err(e) => vec![Err(format!("cannot find the renderer: {e}")); jobs.len()],
    };
    let mut out = vec![None; tiles.len()];
    for ((i, job), result) in index.into_iter().zip(jobs).zip(rendered) {
        out[i] = Some((job.path, result));
    }
    out
}

/// Rasterise every tile's icon once, at startup, for a screen of `ppp`
/// pixels per point. `None` draws the letter.
pub fn load_icons(ctx: &egui::Context, tiles: &[Tile], roots: &IconRoots, ppp: f32) -> Vec<Option<Icon>> {
    render_icons(tiles, roots, render_side(ppp))
        .into_iter()
        .zip(tiles)
        .map(|(icon, tile)| match icon {
            Some((_, Ok(img))) => Some(Icon::new(ctx, &tile.file, &img, ppp)),
            Some((path, Err(e))) => {
                eprintln!("thinfloe-menu: icon {} for {}: {e}", path.display(), tile.file);
                None
            }
            None => {
                if let Some(name) = &tile.icon {
                    eprintln!("thinfloe-menu: icon {name} for {}: not found", tile.file);
                }
                None
            }
        })
        .collect()
}

/// One tile's icon, rendered again in a renderer process: the add-on's icon
/// file arrives while the menu runs.
pub fn load_icon(ctx: &egui::Context, tile: &Tile, roots: &IconRoots, ppp: f32) -> Option<Icon> {
    load_icons(ctx, std::slice::from_ref(tile), roots, ppp).pop().flatten()
}

fn elapsed(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

/// What the person did this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    Tile(usize),
    Chose(Choice),
    Settings(SettingsEvent),
    /// Retry (true) or Later in the retry dialog.
    Retry(bool),
    /// The person clicked the chip of a folded window (its toplevel id).
    Unfold(u64),
}

/// Where the header, the grid and the log go on a screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// The strip along the top: logo, name, host and clock.
    pub header: Rect,
    /// The log, along the bottom.
    pub log: Rect,
    /// The space between header and log.
    pub area: Rect,
    /// The part of `area` the tile grid scrolls in: all of it, or the band
    /// above the status area.
    pub tiles: Rect,
    /// The grid itself: with the status area centred in `area`, or from its
    /// top when they are taller.
    pub grid: Rect,
    /// The size of one band, and of one cell of the status area.
    pub tile: Vec2,
    /// The status area below the grid, four cells wide and as many high as
    /// its shown elements use; `None` when none shows anything.
    pub status: Option<Rect>,
    /// The row of folded-window chips at the bottom edge; `None` when no
    /// window is folded.
    pub strip: Option<Rect>,
}

/// `status_rows` is the rows the status area uses now (0 to 3; 0 draws none);
/// `planned_rows` the rows it has when every element shows
/// (`status::Area::planned_rows`). The tiles and the top of the area are placed
/// by the planned rows, so they stay where they are when an element appears or
/// goes (the elements inside the area close up); with none planned the tiles
/// are centred alone.
pub fn layout(screen: Rect, tiles: usize, folded: usize, status_rows: usize, planned_rows: usize) -> Layout {
    let log_h = (screen.height() * LOG_SHARE).max(LOG_MIN_ROWS as f32 * LOG_ROW + 2.0 * LOG_PAD);
    let (strip, log_bottom) = if folded == 0 {
        (None, screen.bottom() - GAP)
    } else {
        let strip = Rect::from_min_max(
            pos2(screen.left() + GAP, screen.bottom() - GAP - STRIP_H),
            pos2(screen.right() - GAP, screen.bottom() - GAP),
        );
        (Some(strip), strip.top() - STRIP_GAP)
    };
    // The strip takes its room from the log only, down to the log's minimum.
    let log_min = LOG_MIN_ROWS as f32 * LOG_ROW + 2.0 * LOG_PAD;
    let log_top = (screen.bottom() - GAP - log_h).min(log_bottom - log_min);
    let log = Rect::from_min_max(
        pos2(screen.left() + GAP, log_top),
        pos2(screen.right() - GAP, log_bottom),
    );
    let header = Rect::from_min_size(screen.min, vec2(screen.width(), HEADER_H));
    let area = Rect::from_min_max(pos2(screen.left(), header.bottom()), pos2(screen.right(), log.top()));
    let cols = COLUMNS.min(tiles.max(1));
    let rows = tiles.max(1).div_ceil(COLUMNS);
    let group = GROUP_W.min(screen.width() - 2.0 * GAP);
    let tile_w = (group - (COLUMNS as f32 - 1.0) * TILE_GAP) / COLUMNS as f32;
    let max_rows = crate::status::ROWS as usize;
    let (srows, planned) = (status_rows.min(max_rows), planned_rows.min(max_rows));
    // The cells take the tiles' height (64 to 76 pt).
    let fit_h = (area.height() - 2.0 * GAP - (rows as f32 - 1.0) * TILE_GAP) / rows as f32;
    let tile = vec2(tile_w, fit_h.clamp(TILE_H_MIN, TILE_H));
    let span = |n: usize, size: f32| n as f32 * size + (n as f32 - 1.0) * TILE_GAP;
    let size = vec2(span(cols, tile.x), span(rows, tile.y));
    let status_size = vec2(span(COLUMNS, tile.x), span(srows, tile.y));
    let planned_h = span(planned, tile.y);
    let block_h = if planned > 0 {
        size.y + STATUS_GAP + planned_h
    } else {
        size.y
    };
    let min_left = area.left() + GAP.min(area.width() / 2.0);
    let left = (screen.center().x - size.x / 2.0).max(min_left);
    let top = area.top() + ((area.height() - block_h) / 2.0).max(GAP);
    let grid = Rect::from_min_size(pos2(left, top), size);
    // The area stands below the grid; where the planned rows do not fit it is
    // pinned GAP above the log, and only the tiles scroll, in the band above.
    let status_top = (grid.bottom() + STATUS_GAP)
        .min(area.bottom() - GAP - planned_h)
        .max(area.top());
    let status = (srows > 0).then(|| {
        let left = (screen.center().x - status_size.x / 2.0).max(min_left);
        Rect::from_min_size(pos2(left, status_top), status_size)
    });
    let tiles = if planned > 0 {
        Rect::from_min_max(area.min, pos2(area.right(), status_top))
    } else {
        area
    };
    Layout {
        header,
        log,
        area,
        tiles,
        grid,
        tile,
        status,
        strip,
    }
}

/// What the header shows besides the grid: the names, the clock and the
/// logo texture. The logo is `None` in tests that draw without one.
#[derive(Default)]
pub struct Chrome {
    pub header: Header,
    /// A window covers the menu: nothing changes on screen for the eye,
    /// so no element of the status area asks for a frame.
    pub covered: bool,
    /// `HH:MM`; the app refreshes it before every frame.
    pub clock: String,
    pub logo: Option<TextureHandle>,
}

/// Logo, name, host, version and clock along the top of `rect`.
fn show_header(ui: &mut Ui, chrome: &Chrome, rect: Rect) {
    let p = ui.painter();
    let fg = brand::theme().palette.foreground;
    // The mark and the name sit back at 60 %.
    let dim = |c: Color32| c.gamma_multiply(0.6);
    let top = rect.top() + HEADER_PAD_TOP;
    let mut x = rect.left() + HEADER_PAD_X;
    if let Some(tex) = &chrome.logo {
        let size = tex.size_vec2();
        let logo = Rect::from_min_size(pos2(x, top), vec2(LOGO_H * size.x / size.y, LOGO_H));
        p.image(
            tex.id(),
            logo,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            dim(Color32::WHITE),
        );
        x = logo.right() + 14.0;
    }
    let mid = top + LOGO_H / 2.0;
    // Name and byline form one block, centred on the logo.
    let name = p.layout_no_wrap("ThinFloe".into(), brand::geo(34.0), dim(fg));
    let by = p.layout_no_wrap(BYLINE.into(), brand::geo(14.0), MUTED);
    let y = mid - (name.size().y + by.size().y) / 2.0;
    let by_y = y + name.size().y;
    p.galley(pos2(x, y), name, fg);
    p.galley(pos2(x, by_y), by, MUTED);
    p.text(
        pos2(rect.right() - HEADER_PAD_X, mid),
        Align2::RIGHT_CENTER,
        header_text(&chrome.header, &chrome.clock),
        brand::geo(16.0),
        MUTED,
    );
}

/// The words the dialog uses for the way the menu ends: the verb, and its
/// "-ing" form.
fn ending_words(action: Action) -> (&'static str, &'static str) {
    match action {
        Action::Reboot => ("restart", "restarting"),
        Action::Poweroff => ("power off", "powering off"),
        _ => ("log out", "logging out"),
    }
}

/// The first letter in upper case, for a button or a sentence start.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Draw one frame.
pub fn show(ui: &mut Ui, menu: &Menu, icons: &[Option<Icon>], chrome: &Chrome, now: Instant) -> Option<UiEvent> {
    // The clock changes on the minute and at no other time.
    ui.ctx()
        .request_repaint_after(until_next_minute(std::time::SystemTime::now()));
    let (verb, verbing) = ending_words(menu.ending);
    // No background fill here: the canvas restores the page colour itself,
    // and while the dialog is up it holds the frozen grid, which a fill
    // would erase. Only the dialog is drawn then.
    match &menu.logout {
        Some(Logout::Confirm) => {
            let mut chose = None;
            let addon_jobs = menu.addon_jobs();
            let title = if menu.jobs.is_empty() {
                "Add-ons are being changed"
            } else {
                "Programs are still running"
            };
            dialog(ui, title, |ui, t| {
                for job in &menu.jobs {
                    ui.label(egui::RichText::new(&job.name).color(t.palette.foreground));
                }
                for job in &addon_jobs {
                    ui.label(egui::RichText::new(job).color(t.palette.foreground));
                }
                if !menu.jobs.is_empty() {
                    ui.label(
                        egui::RichText::new("They are stopped before the home directory is saved.")
                            .color(t.palette.muted_foreground),
                    );
                }
                if !addon_jobs.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("The add-on work is finished before you {verb}."))
                            .color(t.palette.muted_foreground),
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(Button::new("Stay")).clicked() {
                        chose = Some(Choice::Stay);
                    }
                    let leave = if menu.jobs.is_empty() {
                        format!("{} when done", capitalized(verb))
                    } else {
                        format!("Stop them and {verb}")
                    };
                    if ui.add(Button::new(leave).variant(ButtonVariant::Destructive)).clicked() {
                        chose = Some(Choice::StopAndLeave);
                    }
                });
            });
            return chose.map(UiEvent::Chose);
        }
        Some(Logout::WaitAddons) => {
            let mut chose = None;
            dialog(ui, &format!("Waiting for the add-ons before {verbing}"), |ui, t| {
                for job in menu.addon_jobs() {
                    ui.label(egui::RichText::new(job).color(t.palette.muted_foreground));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(Button::new("Stay")).clicked() {
                        chose = Some(Choice::Stay);
                    }
                    // Log out keeps the system running: a dpkg cut short
                    // would stay half-installed, so it only waits.
                    if menu.ending != Action::Logout
                        && ui
                            .add(
                                Button::new(format!("{} anyway", capitalized(verb)))
                                    .variant(ButtonVariant::Destructive),
                            )
                            .clicked()
                    {
                        chose = Some(Choice::LeaveAnyway);
                    }
                });
            });
            return chose.map(UiEvent::Chose);
        }
        Some(Logout::StoppingAddons { go_at }) => {
            dialog(ui, "Stopping the add-on work", |ui, t| {
                for job in menu.addon_jobs() {
                    ui.label(egui::RichText::new(job).color(t.palette.muted_foreground));
                }
            });
            // Wake for the deadline; the jobs ending arrive as events.
            ui.ctx()
                .request_repaint_after(go_at.saturating_duration_since(now).min(Duration::from_secs(1)));
            return None;
        }
        Some(Logout::Stopping { kill_at, killed }) => {
            dialog(ui, "Stopping programs", |ui, t| {
                for job in &menu.jobs {
                    ui.label(egui::RichText::new(&job.name).color(t.palette.muted_foreground));
                }
            });
            // Wake for the SIGKILL deadline without any input. After it,
            // every remaining step arrives as a job event, which redraws;
            // asking here would redraw at full speed until then.
            if !killed {
                ui.ctx()
                    .request_repaint_after(kill_at.saturating_duration_since(now).min(Duration::from_secs(1)));
            }
            return None;
        }
        Some(Logout::WaitSave) => {
            dialog(ui, &format!("Saving your home directory before {verbing}"), |ui, t| {
                ui.label(
                    egui::RichText::new("Waiting for the backup that is running.").color(t.palette.muted_foreground),
                );
            });
            return None;
        }
        Some(Logout::Saving { since }) => {
            dialog(ui, &format!("Saving your home directory before {verbing}"), |ui, t| {
                ui.label(
                    egui::RichText::new(elapsed(now.duration_since(*since)))
                        .size(28.0)
                        .monospace()
                        .color(t.palette.ring),
                );
                ui.label(
                    egui::RichText::new("On a network-booted machine this can take a minute.")
                        .color(t.palette.muted_foreground),
                );
            });
            ui.ctx().request_repaint_after(Duration::from_secs(1));
            return None;
        }
        Some(Logout::Leaving { until }) => {
            dialog(ui, "Home directory saved", |ui, t| {
                ui.label(egui::RichText::new(format!("{}.", capitalized(verbing))).color(t.palette.muted_foreground));
            });
            ui.ctx().request_repaint_after(until.saturating_duration_since(now));
            return None;
        }
        Some(Logout::Failed { message }) => {
            let mut chose = None;
            dialog(ui, "Your home directory was not saved", |ui, t| {
                ui.label(egui::RichText::new(message).color(WARN));
                ui.label(
                    egui::RichText::new(format!("If you {verb} now, the changes since the last save are lost."))
                        .color(t.palette.muted_foreground),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(Button::new("Stay")).clicked() {
                        chose = Some(Choice::Stay);
                    }
                    if ui
                        .add(Button::new(format!("{} anyway", capitalized(verb))).variant(ButtonVariant::Destructive))
                        .clicked()
                    {
                        chose = Some(Choice::LeaveAnyway);
                    }
                });
            });
            return chose.map(UiEvent::Chose);
        }
        None => {}
    }
    if let Some(r) = &menu.retry {
        let mut chose = None;
        let text = match &r.message {
            Some(m) => format!("{} could not be installed: {m}", r.name),
            None => format!("{} could not be installed.", r.name),
        };
        dialog(ui, "Add-on not installed", |ui, t| {
            ui.label(egui::RichText::new(text).color(WARN));
            ui.label(egui::RichText::new("Settings shows it as well.").color(t.palette.muted_foreground));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(Button::new("Retry")).clicked() {
                    chose = Some(true);
                }
                if ui.add(Button::new("Later").variant(ButtonVariant::Secondary)).clicked() {
                    chose = Some(false);
                }
            });
        });
        return chose.map(UiEvent::Retry);
    }
    let shown = menu.visible_tiles();
    let sources = menu.status_sources();
    let short = short_screen(ui.max_rect().height());
    let l = layout(
        ui.max_rect(),
        shown.len(),
        menu.folded.len(),
        menu.status.rows(&sources, short),
        menu.status.planned_rows(short),
    );
    show_header(ui, chrome, l.header);
    let unfold = l
        .strip
        .and_then(|r| show_strip(ui, menu, icons, r))
        .map(UiEvent::Unfold);
    if menu.screen == Screen::Settings {
        let event = show_settings(ui, menu, icons, l.area);
        show_log(ui, &menu.log, l.log);
        return event.map(UiEvent::Settings).or(unfold);
    }
    let mut clicked = None;
    let mut grid_ui = ui.new_child(UiBuilder::new().max_rect(l.tiles));
    egui::ScrollArea::vertical()
        .id_salt("grid")
        .auto_shrink([false, false])
        .show(&mut grid_ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(TILE_GAP, TILE_GAP);
            // add_space moves the cursor by exactly its amount; the item
            // spacing comes only between the rows and tiles that follow.
            ui.add_space(l.grid.top() - l.area.top());
            for chunk in shown.chunks(COLUMNS) {
                ui.horizontal(|ui| {
                    ui.add_space(l.grid.left() - l.area.left());
                    for &index in chunk {
                        let tile = &menu.tiles[index];
                        let enabled = menu.can_activate(index);
                        let sense = if enabled { Sense::click() } else { Sense::hover() };
                        let (rect, resp) = ui.allocate_exact_size(l.tile, sense);
                        let running = menu.running(index);
                        paint_tile(
                            ui,
                            rect,
                            tile,
                            icons[index].as_ref(),
                            TileState {
                                hovered: resp.hovered() && enabled,
                                focused: resp.has_focus(),
                                running,
                                active: menu.active(index),
                                update: menu.pending_update(index),
                            },
                        );
                        if resp.clicked() {
                            clicked = Some(UiEvent::Tile(index));
                        }
                    }
                });
            }
            ui.add_space(0.0);
        });
    // Outside the scroll area: the tiles scroll, the status area stays.
    if let Some(st) = l.status {
        let place = StatusPlace {
            origin: st.min,
            cell: l.tile,
            short,
        };
        paint_status(ui, &menu.status, &sources, place, now, chrome.covered);
    }
    show_log(ui, &menu.log, l.log);
    clicked.or(unfold)
}

/// Where the status elements go: the top-left cell, the cell size, and
/// whether the screen is short.
#[derive(Clone, Copy)]
struct StatusPlace {
    origin: egui::Pos2,
    cell: Vec2,
    short: bool,
}

/// Draw each shown status element in `place` and ask for a frame when one
/// changes on its own, unless the menu is covered.
fn paint_status(
    ui: &Ui,
    area: &crate::status::Area,
    data: &crate::status::Sources,
    place: StatusPlace,
    now: Instant,
    covered: bool,
) {
    let ctx = ui.ctx();
    let ppp = ctx.pixels_per_point();
    let StatusPlace { origin, cell, short } = place;
    for (p, cells) in area.shown(data, short) {
        let r = cells.rect(origin, cell, TILE_GAP);
        // On whole pixels, so the picture is drawn 1:1.
        let min = (r.min.to_vec2() * ppp).round() / ppp;
        let px = [(r.width() * ppp).round() as usize, (r.height() * ppp).round() as usize];
        let at = Rect::from_min_size(min.to_pos2(), vec2(px[0] as f32, px[1] as f32) / ppp);
        let svg = p.element.svg(data, cells.room(cell, TILE_GAP), now);
        if let (Some(tex), _) = status_texture(ctx, &p.name, &svg, px) {
            ui.painter().image(
                tex.id(),
                at,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
    if !covered && let Some(d) = area.wake(data, cell, TILE_GAP, now, short) {
        ctx.request_repaint_after(d);
    }
}

/// One element's last picture: the SVG text and pixel size it was made
/// from, and the texture, `None` when that SVG did not render.
#[derive(Clone)]
struct StatusTexture {
    svg: String,
    px: [usize; 2],
    tex: Option<TextureHandle>,
    /// When a failed render of this element was last logged.
    logged: Option<Instant>,
}

/// A failed render is logged at most this often per element: a graph's SVG
/// changes every second, and it would fail every second.
const STATUS_LOG_EVERY: Duration = Duration::from_secs(300);

/// Whether a failure now is worth a log line, after one logged at `last`.
fn status_log_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|at| now.saturating_duration_since(at) >= STATUS_LOG_EVERY)
}

/// The name of every status element texture starts so; the app damages
/// only the element's cell when one is uploaded.
pub const STATUS_TEXTURE: &str = "thinfloe-status ";

fn status_id(name: &str) -> egui::Id {
    egui::Id::new(("thinfloe-status", name))
}

/// The texture of element `name` for `svg` at `px` pixels, and whether it
/// was rendered now: only when the text or the size differs from the last
/// one, so a picture is made again only when its content, page, cell size
/// or scale changed. A failed render, a panic in the renderer included,
/// draws nothing and is logged at most every `STATUS_LOG_EVERY`.
fn status_texture(ctx: &egui::Context, name: &str, svg: &str, px: [usize; 2]) -> (Option<TextureHandle>, bool) {
    let id = status_id(name);
    let cached = ctx.data(|d| d.get_temp::<StatusTexture>(id));
    if let Some(c) = &cached
        && c.svg == svg
        && c.px == px
    {
        return (c.tex.clone(), false);
    }
    let mut logged = cached.and_then(|c| c.logged);
    let tex = match crate::status::render::render(svg, px) {
        Ok(img) => Some(ctx.load_texture(format!("{STATUS_TEXTURE}{name}"), img, egui::TextureOptions::NEAREST)),
        Err(e) => {
            let now = Instant::now();
            if status_log_due(logged, now) {
                eprintln!("thinfloe-menu: status element {name}: {e}");
                logged = Some(now);
            }
            None
        }
    };
    let entry = StatusTexture {
        svg: svg.to_string(),
        px,
        tex: tex.clone(),
        logged,
    };
    ctx.data_mut(|d| d.insert_temp(id, entry));
    (tex, true)
}

/// The texture element `name` draws now and its size in pixels, for tests.
#[cfg(test)]
fn status_cache_key(ctx: &egui::Context, name: &str) -> Option<(egui::TextureId, [usize; 2])> {
    ctx.data(|d| d.get_temp::<StatusTexture>(status_id(name)))
        .and_then(|c| c.tex.map(|t| (t.id(), c.px)))
}

/// The folded windows as chips in one row. A row wider than the strip
/// scrolls sideways (wheel and drag), so no chip is out of reach: with zero
/// labwc keybindings this is the only way back to a folded window.
/// Returns the id of the chip clicked.
fn show_strip(ui: &mut Ui, menu: &Menu, icons: &[Option<Icon>], rect: Rect) -> Option<u64> {
    let t = Theme::current(ui.ctx());
    let mut clicked = None;
    let mut strip_ui = ui.new_child(UiBuilder::new().max_rect(rect));
    // A mouse wheel turns vertically; the row only goes sideways.
    strip_ui.style_mut().always_scroll_the_only_direction = true;
    egui::ScrollArea::horizontal()
        .id_salt("strip")
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(&mut strip_ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
            ui.horizontal_centered(|ui| {
                for f in &menu.folded {
                    // The title comes from the window: one line, cut with an
                    // ellipsis at the chip's widest.
                    let title = if f.title.is_empty() { &f.app_id } else { &f.title };
                    let text_max = CHIP_MAX_W - CHIP_ICON - 3.0 * 10.0;
                    let mut job = LayoutJob::simple_singleline(
                        title.clone(),
                        FontId::proportional(CHIP_FONT),
                        t.palette.foreground,
                    );
                    job.wrap = TextWrapping::truncate_at_width(text_max);
                    let galley = ui.painter().layout_job(job);
                    let w = 10.0 + CHIP_ICON + 10.0 + galley.size().x.ceil() + 10.0;
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, CHIP_H), Sense::click_and_drag());
                    // A chip under the pointer takes the drag from the
                    // scroll area, so it hands the movement back.
                    if resp.dragged() {
                        ui.scroll_with_delta(vec2(resp.drag_delta().x, 0.0));
                    }
                    let p = ui.painter();
                    let fill = if resp.hovered() { TILE_HOVER } else { TILE };
                    p.rect_filled(rect, 0.0, fill);
                    p.rect_stroke(rect, 0.0, Stroke::new(1.0, brand::LINE), StrokeKind::Inside);
                    if resp.has_focus() {
                        p.rect_stroke(
                            rect.expand(2.0),
                            0.0,
                            Stroke::new(3.0, t.palette.ring),
                            StrokeKind::Outside,
                        );
                    }
                    let icon_rect = Rect::from_min_size(
                        pos2(rect.left() + 10.0, rect.center().y - CHIP_ICON / 2.0),
                        Vec2::splat(CHIP_ICON),
                    );
                    let tex = menu
                        .tile_for_app(&f.app_id)
                        .and_then(|i| icons.get(i))
                        .and_then(|i| i.as_ref());
                    match tex {
                        Some(icon) => draw_icon(p, &icon.chip, icon_rect.min, Color32::WHITE),
                        None => {
                            p.rect_filled(icon_rect, t.radius_sm(), t.palette.muted);
                            let letter: String = title
                                .chars()
                                .next()
                                .map(|c| c.to_uppercase().collect())
                                .unwrap_or_else(|| "?".into());
                            p.text(
                                icon_rect.center(),
                                Align2::CENTER_CENTER,
                                letter,
                                brand::geo(CHIP_ICON * 0.6),
                                t.palette.foreground,
                            );
                        }
                    }
                    let y = rect.center().y - galley.size().y / 2.0;
                    ui.painter_at(rect)
                        .galley(pos2(icon_rect.right() + 10.0, y), galley, t.palette.foreground);
                    if resp.clicked() {
                        clicked = Some(f.id);
                    }
                }
            });
        });
    clicked
}

/// The left and right padding of a Settings row; the button ends `ROW_RIGHT` from its edge.
const ROW_LEFT: i8 = 22;
const ROW_RIGHT: i8 = 16;
const ROW_MARGIN_X: f32 = (ROW_LEFT + ROW_RIGHT) as f32;
/// The side of a row's icon on the Settings screen.
const ROW_ICON: f32 = 40.0;

/// The Settings screen in `area`: the add-ons, one row each.
fn show_settings(ui: &mut Ui, menu: &Menu, icons: &[Option<Icon>], area: Rect) -> Option<SettingsEvent> {
    let t = Theme::current(ui.ctx());
    let mut event = None;
    // The panel is as wide as the tile group of the desktop, centred.
    let inner = area.shrink(GAP);
    let width = GROUP_W.min(inner.width());
    let panel = Rect::from_center_size(inner.center(), vec2(width, inner.height()));
    let mut s_ui = ui.new_child(UiBuilder::new().max_rect(panel));
    s_ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Add-ons")
                .font(brand::geo(28.0))
                .color(t.palette.foreground),
        );
        ui.add_space(12.0);
        // SAFETY: time(NULL) has no preconditions.
        let now = unsafe { libc::time(std::ptr::null_mut()) } as i64;
        ui.label(egui::RichText::new(settings::last_checked(menu.last_check, now)).color(t.palette.muted_foreground));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(Button::new("Back").variant(ButtonVariant::Secondary)).clicked() {
                event = Some(SettingsEvent::Back);
            }
            if ui
                .add_enabled(
                    !menu.checking(),
                    Button::new("Check now").variant(ButtonVariant::Secondary),
                )
                .clicked()
            {
                event = Some(SettingsEvent::CheckNow);
            }
        });
    });
    s_ui.add_space(GAP);
    egui::ScrollArea::vertical()
        .id_salt("settings")
        .auto_shrink([false, false])
        .show(&mut s_ui, |ui| {
            let rows = settings::rows(menu);
            if rows.is_empty() {
                ui.label(egui::RichText::new("No add-ons are defined.").color(t.palette.muted_foreground));
            }
            ui.spacing_mut().item_spacing.y = TILE_GAP;
            let row_inner = ui.available_width().min(width) - ROW_MARGIN_X;
            for row in rows {
                // A TILE band; the ice edge on the left marks a waiting update.
                let framed = egui::Frame::new()
                    .fill(TILE)
                    .inner_margin(egui::Margin {
                        left: ROW_LEFT,
                        right: ROW_RIGHT,
                        top: 12,
                        bottom: 12,
                    })
                    .show(ui, |ui| {
                        // Bounded by the panel, not by what a long text asked for before.
                        ui.set_width(row_inner);
                        settings_row(ui, &row, icons.get(row.tile).and_then(Option::as_ref), &t)
                    });
                if row.update_ready {
                    let r = framed.response.rect;
                    ui.painter()
                        .rect_filled(Rect::from_min_max(r.min, pos2(r.min.x + 5.0, r.max.y)), 0.0, ICE);
                }
                if let Some(e) = framed.inner {
                    event = Some(e);
                }
            }
        });
    event
}

/// One add-on: tick box, icon, name, source, state, and its button.
fn settings_row(ui: &mut Ui, row: &settings::Row, icon: Option<&Icon>, t: &Theme) -> Option<SettingsEvent> {
    let mut event = None;
    ui.horizontal(|ui| {
        let mut ticked = row.ticked;
        let tick = ui.add_enabled_ui(!row.busy && !row.unusable, |ui| {
            egui_shadcn::components::checkbox::checkbox(ui, &mut ticked)
        });
        if tick.inner.changed() {
            event = Some(SettingsEvent::Tick {
                id: row.id.clone(),
                on: ticked,
            });
        }
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(ROW_ICON), Sense::hover());
        match icon {
            Some(icon) => draw_icon(ui.painter(), &icon.row, rect.min, Color32::WHITE),
            None => {
                ui.painter().rect_filled(rect, 0.0, TILE_HOVER);
                let letter: String = row
                    .name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().collect())
                    .unwrap_or_default();
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    letter,
                    brand::geo(ROW_ICON * 0.5),
                    t.palette.foreground,
                );
            }
        }
        // The button first from the right, so the texts get what is left
        // and a long source is cut instead of pushing it off the row. A row
        // without a button gives its text the whole width.
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), ROW_ICON),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let label = match row.button {
                    Some(RowButton::InstallNow) => Some("Install now"),
                    Some(RowButton::UpdateNow) => Some("Update now"),
                    None => None,
                };
                if let Some(label) = label {
                    let b = Button::new(label);
                    let b = if row.button == Some(RowButton::UpdateNow) {
                        b
                    } else {
                        b.variant(ButtonVariant::Secondary)
                    };
                    if ui.add_enabled(!row.busy, b).clicked() {
                        event = Some(match row.button {
                            Some(RowButton::UpdateNow) => SettingsEvent::UpdateNow(row.id.clone()),
                            _ => SettingsEvent::InstallNow(row.id.clone()),
                        });
                    }
                }
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), ROW_ICON),
                    egui::Layout::top_down(egui::Align::Min).with_main_align(egui::Align::Center),
                    |ui| {
                        ui.set_max_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 0.0;
                        // Not `horizontal`: its minimum height would grow the row past its icon.
                        ui.allocate_ui_with_layout(
                            vec2(ui.available_width(), 22.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                // At most half the width, cut with "…", so the source stays visible.
                                let name_w = ui.available_width() * 0.5;
                                ui.scope(|ui| {
                                    ui.set_max_width(name_w);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&row.name)
                                                .font(brand::geo(20.0))
                                                .color(t.palette.foreground),
                                        )
                                        .truncate(),
                                    );
                                });
                                if let Some(source) = &row.source {
                                    let job =
                                        LayoutJob::simple_singleline(source.clone(), FontId::monospace(11.0), MUTED);
                                    // A horizontal layout lets a label extend; cut it instead.
                                    ui.add(egui::Label::new(job).truncate());
                                }
                            },
                        );
                        // One line: the items joined by dots, each in its tone.
                        let font = FontId::proportional(12.0);
                        let mut job = LayoutJob::default();
                        for (i, (text, tone)) in settings::status_items(row).into_iter().enumerate() {
                            if i > 0 {
                                job.append(" · ", 0.0, egui::TextFormat::simple(font.clone(), MUTED));
                            }
                            let color = match tone {
                                settings::Tone::Plain => t.palette.foreground,
                                settings::Tone::Good => GOOD,
                                settings::Tone::Ice => ICE,
                                settings::Tone::Warn => WARN,
                            };
                            job.append(&text, 0.0, egui::TextFormat::simple(font.clone(), color));
                        }
                        job.wrap = TextWrapping::truncate_at_width(ui.available_width());
                        ui.label(job);
                    },
                );
            },
        );
    });
    event
}

/// The log: one line per row, newest at the bottom, following new lines
/// unless the person scrolled up.
fn show_log(ui: &mut Ui, log: &Log, rect: Rect) {
    let t = Theme::current(ui.ctx());
    let mut log_ui = ui.new_child(UiBuilder::new().max_rect(rect));
    egui::Frame::new()
        .fill(t.palette.card)
        .stroke(Stroke::new(1.0, t.palette.border))
        .corner_radius(t.radius_md())
        .inner_margin(LOG_PAD)
        .show(&mut log_ui, |ui| {
            ui.set_min_size(rect.size() - Vec2::splat(2.0 * LOG_PAD));
            ui.spacing_mut().item_spacing.y = 0.0;
            let n = log.lines().len();
            egui::ScrollArea::vertical()
                .id_salt("log")
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show_rows(ui, LOG_ROW, n, |ui, range| {
                    let width = ui.available_width();
                    for line in log.lines().skip(range.start).take(range.len()) {
                        let (row, _) = ui.allocate_exact_size(vec2(width, LOG_ROW), Sense::hover());
                        let mut job = log_job(line, &t);
                        job.wrap = TextWrapping::truncate_at_width(width);
                        let galley = ui.painter().layout_job(job);
                        let y = row.center().y - galley.size().y / 2.0;
                        ui.painter_at(row)
                            .galley(pos2(row.left(), y), galley, t.palette.foreground);
                    }
                });
        });
}

/// The 16 basic terminal colours, picked to read on the dark log
/// background: black and blue are lightened, which a terminal's defaults
/// would leave nearly invisible here.
const ANSI16: [Color32; 16] = [
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

pub fn ansi_color(a: Ansi) -> Color32 {
    match a {
        Ansi::Basic(n) => ANSI16[(n & 15) as usize],
        Ansi::Indexed(n @ 0..=15) => ANSI16[n as usize],
        Ansi::Indexed(n @ 16..=231) => {
            let level = [0u8, 95, 135, 175, 215, 255];
            let i = n - 16;
            Color32::from_rgb(
                level[(i / 36) as usize],
                level[(i / 6 % 6) as usize],
                level[(i % 6) as usize],
            )
        }
        Ansi::Indexed(n) => {
            let g = 8 + 10 * (n - 232);
            Color32::from_rgb(g, g, g)
        }
        Ansi::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    }
}

/// A run's colour: its own, brightened when bold the way terminals do, or
/// the line's colour by kind; bold without a colour is the strong text.
fn run_color(run: &Run, line_color: Color32, t: &Theme) -> Color32 {
    match (run.fg, run.bold) {
        (Some(Ansi::Basic(n)), true) if n < 8 => ansi_color(Ansi::Basic(n + 8)),
        (Some(a), _) => ansi_color(a),
        (None, true) => t.palette.foreground,
        (None, false) => line_color,
    }
}

/// One log line: the time, then the program it came from, then the text.
fn log_job(line: &Line, t: &Theme) -> LayoutJob {
    let font = FontId::monospace(LOG_FONT);
    let text_color = match line.kind {
        Kind::Event => t.palette.foreground,
        Kind::Output => t.palette.muted_foreground,
        Kind::Warn => WARN,
        Kind::Good => GOOD,
    };
    let mut job = LayoutJob::default();
    job.append(&line.time, 0.0, TextFormat::simple(font.clone(), TIME));
    let mut lead = 16.0;
    if let Some(source) = &line.source {
        job.append(source, 16.0, TextFormat::simple(font.clone(), ICE));
        job.append(" | ", 0.0, TextFormat::simple(font.clone(), TIME));
        lead = 0.0;
    }
    for run in &line.runs {
        job.append(
            &run.text,
            lead,
            TextFormat::simple(font.clone(), run_color(run, text_color, t)),
        );
        lead = 0.0;
    }
    job
}

/// What a tile shows about the programs behind it.
#[derive(Debug, Clone, Copy, Default)]
struct TileState {
    hovered: bool,
    focused: bool,
    /// Copies of its program running.
    running: usize,
    /// Whether the ice edge is drawn: a program runs, or the save does.
    active: bool,
    /// An add-on update waits for the next boot.
    update: bool,
}

fn paint_tile(ui: &Ui, rect: Rect, tile: &Tile, icon: Option<&Icon>, state: TileState) {
    let t = Theme::current(ui.ctx());
    let p = ui.painter();
    let usable = tile.usable();
    // A tile that cannot be used fades; its reason stays at full strength.
    let fade = |c: Color32| if usable { c } else { c.gamma_multiply(0.35) };
    let fill = if state.hovered {
        TILE_HOVER
    } else if state.active {
        TILE_ACTIVE
    } else {
        TILE
    };
    p.rect_filled(rect, 0.0, fade(fill));
    if state.active {
        p.rect_filled(Rect::from_min_size(rect.min, vec2(EDGE_W, rect.height())), 0.0, ICE);
    }
    if state.focused {
        p.rect_stroke(
            rect.expand(2.0),
            0.0,
            Stroke::new(3.0, t.palette.ring),
            StrokeKind::Outside,
        );
    }
    let ppp = ui.ctx().pixels_per_point();
    // On a whole pixel, or the icon texture is sampled between two pixels
    // and its edges turn ragged.
    let snap = |v: f32| (v * ppp).round() / ppp;
    let icon_rect = Rect::from_min_size(
        pos2(snap(rect.left() + 22.0), snap(rect.center().y - ICON_BOX / 2.0)),
        Vec2::splat(ICON_BOX),
    );
    match icon {
        Some(icon) => draw_icon(p, &icon.tile, icon_rect.min, fade(Color32::WHITE)),
        None => {
            p.rect_filled(icon_rect, t.radius_sm(), fade(t.palette.muted));
            let letter: String = tile
                .name
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default();
            p.text(
                icon_rect.center(),
                Align2::CENTER_CENTER,
                letter,
                brand::geo(ICON_BOX * 0.5),
                fade(t.palette.foreground),
            );
        }
    }
    // Name and Comment come from files the user writes. Each is one line,
    // cut with an ellipsis, and all text is clipped to the tile, so no file
    // can paint over its neighbours.
    let text_x = icon_rect.right() + 14.0;
    let text_w = (rect.right() - 14.0 - text_x).max(0.0);
    let clipped = ui.painter_at(rect);
    let mut lines = Vec::new();
    let mut line = |text: &str, font: FontId, color: Color32| {
        let mut job = LayoutJob::simple_singleline(text.to_string(), font, color);
        job.wrap = TextWrapping::truncate_at_width(text_w);
        lines.push(clipped.layout_job(job));
    };
    line(&tile.name, brand::geo(21.0), fade(t.palette.foreground));
    // One second line: why the tile is grey, else its comment, else the
    // note about its file. `--check` has the long form.
    if let Some(why) = &tile.problem {
        line(why, FontId::proportional(11.0), WARN);
    } else if let Some(c) = tile.comment.clone().or(tile.note.clone()).or_else(|| {
        // Only a reworded file that lost keys; plain "reworded" is no news.
        match &tile.origin {
            Origin::Reworded { ignored } if !ignored.is_empty() => tile.origin.reworded_text(),
            _ => None,
        }
    }) {
        line(&c, FontId::proportional(11.0), fade(MUTED));
    }
    let heights: Vec<f32> = lines.iter().map(|g| g.size().y).collect();
    let block_h = heights.iter().sum::<f32>() + 2.0 * (lines.len() as f32 - 1.0);
    let mut y = snap(rect.center().y - block_h / 2.0);
    for (galley, h) in lines.into_iter().zip(heights) {
        clipped.galley(pos2(text_x, y), galley, Color32::WHITE);
        y += h + 2.0;
    }
    if state.running > 1 {
        p.text(
            rect.right_top() + vec2(-9.0, 7.0),
            Align2::RIGHT_TOP,
            format!("×{}", state.running),
            brand::geo(15.0),
            ICE,
        );
    }
    // Small marks along the bottom right.
    let mut right = rect.right() - 9.0;
    let mut mark = |text: &str, color: Color32| {
        let r = p.text(
            pos2(right, rect.bottom() - 6.0),
            Align2::RIGHT_BOTTOM,
            text,
            FontId::proportional(11.0),
            color,
        );
        right = r.left() - 8.0;
    };
    if state.update {
        mark("update", ICE);
    }
    if tile.origin == Origin::User {
        mark("user", MUTED);
    }
}

/// A centred card over the frozen grid.
fn dialog(ui: &Ui, title: &str, body: impl FnOnce(&mut Ui, &Theme)) {
    let t = Theme::current(ui.ctx());
    egui::Area::new(egui::Id::new("dialog"))
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(TILE)
                .stroke(Stroke::new(1.0, brand::LINE))
                .inner_margin(24.0)
                .show(ui, |ui| {
                    ui.set_width(460.0);
                    ui.label(egui::RichText::new(title).font(brand::geo(24.0)));
                    body(ui, &t);
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::battery;
    use crate::tiles::{Action, Grid};

    // Before the fix a long Name wrapped down the screen and the painter was
    // clipped only to the scroll area, so text ran over the rows below.
    #[test]
    fn tile_text_stays_on_one_line_inside_its_tile() {
        let long = "W".repeat(120);
        let tile = Tile {
            file: "1.desktop".into(),
            name: long.clone(),
            comment: Some(long.clone()),
            icon: None,
            action: Action::Run,
            argv: vec!["x".into()],
            terminal: false,
            save_on_exit: false,
            multiple: false,
            origin: Origin::User,
            problem: None,
            note: None,
            addon: false,
            source: None,
        };
        let menu = Menu::new(
            Grid {
                tiles: vec![tile],
                banner: vec![],
            },
            false,
        );
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            show(ui, &menu, &[None], &Chrome::default(), Instant::now());
        });
        // The font atlas upload is not applied anywhere here.
        out.textures_delta.clear();
        let texts: Vec<_> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text().starts_with("WWW") => Some((c.clip_rect, t)),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2, "name and comment drawn");
        for (clip, t) in texts {
            assert_eq!(t.galley.rows.len(), 1, "one line");
            assert!(clip.height() <= 260.0, "clipped to the tile, not {clip:?}");
            assert!(clip.contains_rect(t.visual_bounding_rect()), "inside its clip");
        }
    }

    fn dev(name: &str, percent: u8, state: &str) -> battery::Device {
        battery::Device {
            name: name.into(),
            percent: Some(percent),
            state: state.into(),
            read_epoch: 0,
            kind: battery::DeviceKind::Unknown,
        }
    }

    fn kdev(name: &str, kind: battery::DeviceKind, percent: u8, state: &str) -> battery::Device {
        battery::Device {
            kind,
            ..dev(name, percent, state)
        }
    }

    /// The shipped status files.
    fn shipped_status() -> crate::status::Area {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../overlay/etc/thinfloe/status");
        crate::status::config::load(&dir)
    }

    fn status_menu(devices: Vec<battery::Device>, t0: Instant) -> Menu {
        let mut m = Menu::new(shipped_grid(), false);
        m.set_status(shipped_status());
        m.set_battery(devices, t0);
        m
    }

    /// The shapes of one 1280 x 720 frame, `at` the frame's time, and the
    /// repaint delay egui was asked for.
    fn status_frame(
        ctx: &egui::Context,
        menu: &Menu,
        at: Instant,
        covered: bool,
    ) -> (Vec<egui::epaint::ClippedShape>, Duration) {
        status_frame_at(ctx, menu, at, covered, None)
    }

    /// One frame of a screen `w` by `h`.
    fn status_frame_on(
        ctx: &egui::Context,
        menu: &Menu,
        at: Instant,
        (w, h): (f32, f32),
    ) -> Vec<egui::epaint::ClippedShape> {
        status_frame_sized(ctx, menu, at, false, None, screen(w, h)).0
    }

    /// `status_frame` with egui's clock at `time` seconds, so its own
    /// animations (a scroll bar fading in) end.
    fn status_frame_at(
        ctx: &egui::Context,
        menu: &Menu,
        at: Instant,
        covered: bool,
        time: Option<f64>,
    ) -> (Vec<egui::epaint::ClippedShape>, Duration) {
        status_frame_sized(ctx, menu, at, covered, time, screen(1280.0, 720.0))
    }

    fn status_frame_sized(
        ctx: &egui::Context,
        menu: &Menu,
        at: Instant,
        covered: bool,
        time: Option<f64>,
        rect: Rect,
    ) -> (Vec<egui::epaint::ClippedShape>, Duration) {
        let input = egui::RawInput {
            screen_rect: Some(rect),
            time,
            ..Default::default()
        };
        let icons: Vec<Option<Icon>> = menu.tiles.iter().map(|_| None).collect();
        let chrome = Chrome {
            covered,
            ..Chrome::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            show(ui, menu, &icons, &chrome, at);
        });
        out.textures_delta.clear();
        let delay = out.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        (out.shapes, delay)
    }

    fn new_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        ctx
    }

    /// The rectangles of the textured meshes drawn: with no icons and no
    /// logo, only status elements draw pictures.
    fn pictures(shapes: &[egui::epaint::ClippedShape]) -> Vec<Rect> {
        shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => Some(m.calc_bounds()),
                _ => None,
            })
            .collect()
    }

    fn five() -> Vec<battery::Device> {
        ["A", "B", "C", "D", "E"]
            .iter()
            .map(|n| dev(&format!("Dev {n}"), 50, "discharging"))
            .collect()
    }

    #[test]
    fn the_battery_element_draws_one_picture_in_the_top_left_cell() {
        let t0 = Instant::now();
        let menu = status_menu(vec![kdev("K", battery::DeviceKind::Keyboard, 80, "discharging")], t0);
        let (shapes, _) = status_frame(&new_ctx(), &menu, t0, false);
        let l = layout(screen(1280.0, 720.0), menu.visible_tiles().len(), 0, 1, 1);
        let st = l.status.unwrap();
        let cell = crate::status::Cells::parse("0,0,1,1")
            .unwrap()
            .rect(st.min, l.tile, TILE_GAP);
        let got = pictures(&shapes);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!((got[0].min - cell.min).length() < 1.0, "{:?} vs {cell:?}", got[0]);
        assert!((got[0].size() - cell.size()).length() < 1.0, "{:?} vs {cell:?}", got[0]);
        // No tile of its own any more.
        assert!(menu.tiles.iter().all(|t| t.name != "Battery"));
    }

    fn tile_name_top(shapes: &[egui::epaint::ClippedShape], name: &str) -> f32 {
        shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text() == name => Some(t.pos.y),
                _ => None,
            })
            .expect("tile name drawn")
    }

    #[test]
    fn without_a_device_nothing_is_drawn_and_the_tiles_keep_their_place() {
        let t0 = Instant::now();
        let with_area = status_menu(Vec::new(), t0);
        let without = Menu::new(shipped_grid(), false);
        let (a, _) = status_frame(&new_ctx(), &with_area, t0, false);
        let (b, _) = status_frame(&new_ctx(), &without, t0, false);
        assert!(pictures(&a).is_empty());
        // The files plan a row, so the tiles stand higher than in a menu with
        // no status file at all, which keeps its tiles centred alone.
        assert!(
            tile_name_top(&a, "Terminal") < tile_name_top(&b, "Terminal"),
            "{} {}",
            tile_name_top(&a, "Terminal"),
            tile_name_top(&b, "Terminal")
        );
        // With a device the grid stays where it was: no tile jump.
        let (c, _) = status_frame(&new_ctx(), &status_menu(five(), t0), t0, false);
        assert_eq!(tile_name_top(&c, "Terminal"), tile_name_top(&a, "Terminal"));
    }

    #[test]
    fn a_page_turn_is_asked_for_only_while_the_menu_is_shown() {
        let t0 = Instant::now();
        let menu = status_menu(five(), t0);
        let at = t0 + Duration::from_millis(1500);
        let (_, delay) = status_frame(&new_ctx(), &menu, at, false);
        assert!(delay <= Duration::from_millis(2500), "{delay:?}");
        let (_, covered) = status_frame(&new_ctx(), &menu, at, true);
        // Only the minute clock asks; it never lands on the page's 2.5 s.
        assert_ne!(covered, Duration::from_millis(2500));
        let (_, calm) = status_frame(&new_ctx(), &status_menu(five()[..2].to_vec(), t0), at, false);
        assert_ne!(calm, Duration::from_millis(2500), "nothing pages: no wake");
    }

    #[test]
    fn an_element_is_rendered_again_only_when_its_picture_changes() {
        let ctx = new_ctx();
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"5\"><rect width=\"10\" height=\"5\" fill=\"#fff\"/></svg>";
        let other = svg.replace("#fff", "#f00");
        let mut first = None;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let c = ui.ctx();
            let (tex, rendered) = status_texture(c, "a", svg, [20, 10]);
            assert!(rendered && tex.is_some());
            first = tex.map(|t| t.id());
            assert!(!status_texture(c, "a", svg, [20, 10]).1, "same text, same size: cached");
            assert_eq!(status_texture(c, "a", svg, [20, 10]).0.map(|t| t.id()), first);
            assert!(status_texture(c, "b", svg, [20, 10]).1, "each element has its own");
            assert!(!status_texture(c, "a", svg, [20, 10]).1, "b did not push a out");
            assert!(status_texture(c, "a", svg, [40, 20]).1, "new scale or cell size");
            assert!(status_texture(c, "a", &other, [40, 20]).1, "new content or page");
            let (bad, rendered) = status_texture(c, "a", "<svg", [40, 20]);
            assert!(rendered && bad.is_none(), "a broken SVG draws nothing");
            assert!(
                !status_texture(c, "a", "<svg", [40, 20]).1,
                "and is not tried every frame"
            );
        });
        out.textures_delta.clear();
        // Paging a battery element changes its text; the same page does not.
        let t0 = Instant::now();
        let menu = status_menu(five(), t0);
        let ctx = new_ctx();
        let frame = |at| status_frame(&ctx, &menu, at, false);
        frame(t0);
        assert!(!frame(t0 + Duration::from_millis(500)).0.is_empty());
        let id = |ctx: &egui::Context| status_cache_key(ctx, "10-battery.conf");
        let before = id(&ctx);
        frame(t0 + Duration::from_millis(900));
        assert_eq!(id(&ctx), before, "same page: no new texture");
        frame(t0 + Duration::from_secs(4));
        assert_ne!(id(&ctx), before, "next page: a new picture");
    }

    #[test]
    fn a_new_scale_or_cell_size_renders_again_and_never_stretches() {
        let t0 = Instant::now();
        let menu = status_menu(vec![dev("Dev A", 50, "discharging")], t0);
        let ctx = new_ctx();
        let key = |ctx: &egui::Context| status_cache_key(ctx, "10-battery.conf");
        let run = |ctx: &egui::Context, h: f32| {
            let input = egui::RawInput {
                screen_rect: Some(screen(1280.0, h)),
                ..Default::default()
            };
            let icons: Vec<Option<Icon>> = menu.tiles.iter().map(|_| None).collect();
            let mut out = ctx.run_ui(input, |ui| {
                show(ui, &menu, &icons, &Chrome::default(), t0);
            });
            out.textures_delta.clear();
            pictures(&out.shapes)[0]
        };
        run(&ctx, 720.0);
        let first = key(&ctx);
        let pic = run(&ctx, 720.0);
        assert_eq!(key(&ctx), first, "same data, same pixels: cached");
        // The scale settles after the first frame: egui takes a zoom
        // factor at the start of the next pass.
        ctx.set_zoom_factor(2.0);
        run(&ctx, 720.0);
        let zoomed = run(&ctx, 720.0);
        let ppp = ctx.pixels_per_point();
        assert_eq!(ppp, 2.0);
        assert_ne!(key(&ctx), first, "twice the pixels: rendered again");
        assert_eq!(
            key(&ctx).unwrap().1,
            [
                (zoomed.width() * ppp).round() as usize,
                (zoomed.height() * ppp).round() as usize
            ],
            "drawn 1:1"
        );
        assert_eq!(first.unwrap().1, [pic.width() as usize, pic.height() as usize]);
        // A screen too low for 76 pt tiles squeezes the cell: rendered again.
        ctx.set_zoom_factor(1.0);
        run(&ctx, 720.0);
        run(&ctx, 720.0);
        let tall = key(&ctx);
        let short = run(&ctx, 400.0);
        assert!(short.height() < TILE_H, "{short:?}");
        assert_ne!(key(&ctx), tall, "a lower cell: rendered again");
    }

    #[test]
    fn a_page_turn_uploads_only_the_element_and_damages_only_its_cell() {
        let t0 = Instant::now();
        let menu = status_menu(five(), t0);
        let ctx = new_ctx();
        let input = || egui::RawInput {
            screen_rect: Some(screen(1280.0, 720.0)),
            ..Default::default()
        };
        let icons: Vec<Option<Icon>> = menu.tiles.iter().map(|_| None).collect();
        let frame = |at| {
            ctx.run_ui(input(), |ui| {
                show(ui, &menu, &icons, &Chrome::default(), at);
            })
        };
        // Fonts land one frame late; let them settle.
        for _ in 0..3 {
            frame(t0).textures_delta.clear();
        }
        let mut out = frame(t0 + Duration::from_secs(4));
        let set: Vec<egui::TextureId> = out.textures_delta.set.keys().copied().collect();
        out.textures_delta.clear();
        assert_eq!(set.len(), 1, "one new picture: {set:?}");
        let own = |id| {
            ctx.tex_manager()
                .read()
                .meta(id)
                .is_some_and(|m| m.name.starts_with(STATUS_TEXTURE))
        };
        let pic = pictures(&out.shapes)[0];
        let prims = ctx.tessellate(out.shapes, 1.0);
        let dmg = crate::damage::texture_damage(&set, own, &prims, 1.0).expect("only a status texture");
        assert_eq!(
            dmg,
            crate::raster::IRect::from_points(pic, 1.0),
            "its cell, not the canvas"
        );
    }

    /// A network copy with `n` samples of a curve, on eth0 with an
    /// address, a gateway, DNS and wg0 up.
    fn net_snap(n: usize) -> crate::network::Snapshot {
        use crate::network::{Info, Link, Vpn};
        let curve = |k: f64| -> Vec<f64> {
            (0..n)
                .map(|i| {
                    let t = i as f64 / 300.0 * std::f64::consts::TAU;
                    (k * (1.2 + (t * 3.0).sin() + 0.6 * (t * 11.0).sin())).max(0.0)
                })
                .collect()
        };
        crate::network::Snapshot {
            rx: curve(4.0e6),
            tx: curve(6.0e5),
            total: (1_234_000_000, 98_700_000),
            info: Info {
                iface: Some("eth0".into()),
                default_route: true,
                link: Link::Wired(Some(1000)),
                ipv4: Some((std::net::Ipv4Addr::new(192, 168, 1, 23), 24)),
                gateway: Some(std::net::Ipv4Addr::new(192, 168, 1, 1)),
                dns: vec!["192.168.1.1".into(), "9.9.9.9".into()],
                vpn: Vpn::Up,
                addressed: true,
            },
        }
    }

    fn net_menu(n: usize, t0: Instant) -> Menu {
        let mut m = status_menu(vec![kdev("K", battery::DeviceKind::Keyboard, 80, "discharging")], t0);
        m.set_network(net_snap(n), t0, std::time::SystemTime::now());
        m
    }

    #[test]
    fn the_network_graph_spans_two_by_two_cells_and_its_list_stands_right_of_it() {
        let t0 = Instant::now();
        let menu = net_menu(120, t0);
        let full = (1920.0, 1080.0);
        assert_eq!(
            menu.status.rows(&menu.status_sources(), false),
            3,
            "two rows and the clock's"
        );
        let shapes = status_frame_on(&new_ctx(), &menu, t0, full);
        let l = layout(screen(full.0, full.1), menu.visible_tiles().len(), 0, 3, 3);
        let st = l.status.unwrap();
        let want = crate::status::Cells::parse("1,0,2,2")
            .unwrap()
            .rect(st.min, l.tile, TILE_GAP);
        let list = crate::status::Cells::parse("3,0,1,2")
            .unwrap()
            .rect(st.min, l.tile, TILE_GAP);
        let got = pictures(&shapes);
        assert_eq!(got.len(), 4, "battery, graph, list and clock: {got:?}");
        assert!((got[1].min - want.min).length() < 1.0, "{:?} vs {want:?}", got[1]);
        assert!((got[1].size() - want.size()).length() < 1.0, "{:?} vs {want:?}", got[1]);
        assert!((got[2].min - list.min).length() < 1.0, "{:?} vs {list:?}", got[2]);
        assert!((got[2].size() - list.size()).length() < 1.0, "{:?} vs {list:?}", got[2]);
        // With nothing to measure it is gone, the battery stays, the tiles
        // do not move.
        let mut gone = net_menu(120, t0);
        gone.set_network(crate::network::Snapshot::default(), t0, std::time::SystemTime::now());
        let g = status_frame_on(&new_ctx(), &gone, t0, full);
        assert_eq!(pictures(&g).len(), 2, "the battery and the clock");
        assert_eq!(gone.status.rows(&gone.status_sources(), false), 1, "back to one row");
        assert_eq!(tile_name_top(&g, "Terminal"), tile_name_top(&shapes, "Terminal"));
        // Without a battery device graph and list still take two rows, the
        // clock under them the third.
        let mut alone = net_menu(120, t0);
        alone.set_battery(Vec::new(), t0);
        assert_eq!(alone.status.rows(&alone.status_sources(), false), 3);
        let a = status_frame_on(&new_ctx(), &alone, t0, full);
        assert_eq!(pictures(&a).len(), 3);
        assert!((pictures(&a)[0].min - want.min).length() < 1.0);
    }

    #[test]
    fn a_failing_element_draws_nothing_and_is_logged_once_in_a_while() {
        let ctx = new_ctx();
        let logged = |ctx: &egui::Context| {
            ctx.data(|d| d.get_temp::<StatusTexture>(status_id("bad")))
                .and_then(|c| c.logged)
        };
        // Its text changes each second, and it fails each second.
        let (tex, _) = status_texture(&ctx, "bad", "<svg 1", [10, 10]);
        assert!(tex.is_none());
        let first = logged(&ctx).expect("the first failure is logged");
        let (tex, _) = status_texture(&ctx, "bad", "<svg 2", [10, 10]);
        assert!(tex.is_none());
        assert_eq!(logged(&ctx), Some(first), "the second one is not");
        // A success between two failures does not start the count again.
        let good = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"/>";
        let (tex, _) = status_texture(&ctx, "bad", good, [10, 10]);
        assert!(tex.is_some());
        status_texture(&ctx, "bad", "<svg 3", [10, 10]);
        assert_eq!(logged(&ctx), Some(first));
        // And the time rule itself.
        assert!(status_log_due(None, first));
        assert!(!status_log_due(
            Some(first),
            first + STATUS_LOG_EVERY - Duration::from_secs(1)
        ));
        assert!(status_log_due(Some(first), first + STATUS_LOG_EVERY));
    }

    /// Whether a picture is drawn on `cells` of the area in layout `l`.
    fn drawn_on(got: &[Rect], l: &Layout, cells: &str) -> bool {
        let want = crate::status::Cells::parse(cells)
            .unwrap()
            .rect(l.status.unwrap().min, l.tile, TILE_GAP);
        got.iter()
            .any(|r| (r.min - want.min).length() < 1.0 && (r.size() - want.size()).length() < 1.0)
    }

    #[test]
    fn the_hardware_tiles_stand_where_the_owner_put_them_and_are_gone_on_a_short_screen() {
        let t0 = Instant::now();
        let mut menu = net_menu(120, t0);
        menu.set_hardware(crate::status::hardware::sample());
        let full = (1920.0, 1080.0);
        let n = menu.visible_tiles().len();
        let l = layout(screen(full.0, full.1), n, 0, 3, 3);
        assert_eq!(menu.status.rows(&menu.status_sources(), false), 3);
        let all = status_frame_on(&new_ctx(), &menu, t0, full);
        let got = pictures(&all);
        assert_eq!(got.len(), 8, "{got:?}");
        for cells in [
            "0,0,1,1", "1,0,2,2", "3,0,1,2", "0,1,1,1", "0,2,1,1", "1,2,1,1", "2,2,1,1", "3,2,1,1",
        ] {
            assert!(drawn_on(&got, &l, cells), "{cells} in {got:?}");
        }
        // No battery: its column closes up, the others stay, the tiles do not
        // move.
        let mut no_battery = net_menu(120, t0);
        no_battery.set_battery(Vec::new(), t0);
        no_battery.set_hardware(crate::status::hardware::sample());
        let shapes = status_frame_on(&new_ctx(), &no_battery, t0, full);
        let got = pictures(&shapes);
        assert_eq!(got.len(), 7, "{got:?}");
        for cells in [
            "0,0,1,1", "0,1,1,1", "1,0,2,2", "3,0,1,2", "1,2,1,1", "2,2,1,1", "3,2,1,1",
        ] {
            assert!(drawn_on(&got, &l, cells), "{cells} in {got:?}");
        }
        assert!(!drawn_on(&got, &l, "0,2,1,1"), "{got:?}");
        assert_eq!(tile_name_top(&shapes, "Terminal"), tile_name_top(&all, "Terminal"));
        // No network: memory and system in the top row, tiles unmoved.
        let mut no_net = net_menu(120, t0);
        no_net.set_network(crate::network::Snapshot::default(), t0, std::time::SystemTime::now());
        no_net.set_hardware(crate::status::hardware::sample());
        let shapes = status_frame_on(&new_ctx(), &no_net, t0, full);
        let got = pictures(&shapes);
        for cells in ["0,0,1,1", "0,1,1,1", "0,2,1,1", "1,0,1,1", "2,0,1,1"] {
            assert!(drawn_on(&got, &l, cells), "{cells} in {got:?}");
        }
        assert_eq!(tile_name_top(&shapes, "Terminal"), tile_name_top(&all, "Terminal"));
        // A VM: one row, in the top row of the same place, tiles unmoved.
        let mut vm = net_menu(120, t0);
        vm.set_battery(Vec::new(), t0);
        vm.set_network(crate::network::Snapshot::default(), t0, std::time::SystemTime::now());
        vm.set_hardware(crate::hardware::Snapshot {
            temps: vec![],
            ..crate::status::hardware::sample()
        });
        assert_eq!(vm.status.rows(&vm.status_sources(), false), 1, "no blank row");
        let shapes = status_frame_on(&new_ctx(), &vm, t0, full);
        let got = pictures(&shapes);
        assert_eq!(got.len(), 4, "{got:?}");
        for cells in ["0,0,1,1", "1,0,1,1", "2,0,1,1", "3,0,1,1"] {
            assert!(drawn_on(&got, &l, cells), "{cells} in {got:?}");
        }
        assert_eq!(tile_name_top(&shapes, "Terminal"), tile_name_top(&all, "Terminal"));
        // Nothing yet from the sampler: the same place again.
        let shapes = status_frame_on(&new_ctx(), &net_menu(120, t0), t0, full);
        assert_eq!(tile_name_top(&shapes, "Terminal"), tile_name_top(&all, "Terminal"));
        // A short screen: battery and the 3 x 1 graph, nothing else.
        let small = (1280.0, 720.0);
        let l = layout(screen(small.0, small.1), n, 0, 1, 1);
        let got = pictures(&status_frame_on(&new_ctx(), &menu, t0, small));
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(menu.status.rows(&menu.status_sources(), true), 1);
        assert!(drawn_on(&got, &l, "1,0,3,1"), "{got:?}");
    }

    #[test]
    fn the_shipped_status_area_covers_no_tile_on_any_screen() {
        let t0 = Instant::now();
        let mut menu = net_menu(120, t0);
        menu.set_hardware(crate::status::hardware::sample());
        let n = menu.visible_tiles().len();
        for (w, h, planned) in SHIPPED_SCREENS {
            let short = short_screen(h);
            assert_eq!(menu.status.planned_rows(short), planned, "{h}");
            let l = layout(
                screen(w, h),
                n,
                0,
                menu.status.rows(&menu.status_sources(), short),
                planned,
            );
            assert!(
                l.grid.bottom() + STATUS_GAP <= l.status.unwrap().top() + 0.01,
                "{w}x{h}: {l:?}"
            );
            for r in pictures(&status_frame_on(&new_ctx(), &menu, t0, (w, h))) {
                assert!(r.min.y >= l.status.unwrap().top() - 1.0, "{w}x{h}: {r:?}");
                assert!(r.max.y <= l.log.top() - GAP + 1.0, "{w}x{h}: {r:?}");
            }
        }
    }

    #[test]
    fn short_screens_give_the_network_one_row_and_cover_no_shipped_tile() {
        let t0 = Instant::now();
        assert!(short_screen(SHORT_SCREEN_H - 0.5));
        assert!(!short_screen(SHORT_SCREEN_H));
        let menu = net_menu(120, t0);
        let n = menu.visible_tiles().len();
        assert_eq!(n, 8, "the shipped tiles");
        for size in [(1280.0, 720.0), (1366.0, 768.0), (1280.0, 800.0), (1440.0, 899.0)] {
            assert_eq!(menu.status.rows(&menu.status_sources(), true), 1);
            let l = layout(screen(size.0, size.1), n, 0, 1, 1);
            let st = l.status.unwrap();
            assert!(l.grid.bottom() <= l.tiles.bottom() + 0.01, "{size:?}: no tile covered");
            let shapes = status_frame_on(&new_ctx(), &menu, t0, size);
            let got = pictures(&shapes);
            assert_eq!(got.len(), 2, "{size:?}: {got:?}");
            let want = crate::status::Cells::parse("1,0,3,1")
                .unwrap()
                .rect(st.min, l.tile, TILE_GAP);
            assert!(
                (got[1].min - want.min).length() < 1.0,
                "{size:?}: {:?} vs {want:?}",
                got[1]
            );
            assert!(
                (got[1].size() - want.size()).length() < 1.0,
                "{size:?}: {:?} vs {want:?}",
                got[1]
            );
        }
        // From the threshold up: both rows, and still no tile covered.
        for size in [(1440.0, 900.0), (1600.0, 1000.0), (1920.0, 1080.0)] {
            let l = layout(screen(size.0, size.1), n, 0, 2, 3);
            assert!(l.grid.bottom() <= l.tiles.bottom() + 0.01, "{size:?}: no tile covered");
            let got = pictures(&status_frame_on(&new_ctx(), &menu, t0, size));
            assert!(
                (got[1].height() - (2.0 * l.tile.y + TILE_GAP)).abs() < 1.0,
                "{size:?}: {got:?}"
            );
        }
    }

    #[test]
    fn nine_tiles_at_1280x720_fit_with_the_one_row_area_and_keep_their_place() {
        let t0 = Instant::now();
        let nine = || {
            let mut grid = shipped_grid();
            let terminal = grid.tiles.iter().find(|t| t.name == "Terminal").unwrap().clone();
            grid.tiles.push(crate::tiles::Tile {
                name: "Ninth".into(),
                ..terminal
            });
            Menu::new(grid, false)
        };
        let mut menu = nine();
        menu.set_status(shipped_status());
        menu.set_battery(vec![kdev("K", battery::DeviceKind::Keyboard, 80, "discharging")], t0);
        menu.set_network(net_snap(120), t0, std::time::SystemTime::now());
        let n = menu.visible_tiles().len();
        assert_eq!(n, 9, "{:?}", menu.tiles.iter().map(|t| &t.name).collect::<Vec<_>>());
        assert_eq!(menu.status.rows(&menu.status_sources(), true), 1);
        let l = layout(screen(1280.0, 720.0), n, 0, 1, 1);
        let st = l.status.unwrap();
        assert!((st.height() - l.tile.y).abs() < 0.01);
        assert!(st.bottom() <= l.log.top() - GAP + 0.01, "{st:?} above {:?}", l.log);
        assert!(l.grid.bottom() <= l.tiles.bottom() + 0.01, "nothing to scroll: {l:?}");
        assert!((st.top() - l.grid.bottom() - STATUS_GAP).abs() < 0.01, "{l:?}");
        assert_eq!(l.grid, layout(screen(1280.0, 720.0), n, 0, 0, 1).grid, "no tile jump");
        let (with, _) = status_frame(&new_ctx(), &menu, t0, false);
        let mut empty = nine();
        empty.set_status(shipped_status());
        let (none_shown, _) = status_frame(&new_ctx(), &empty, t0, false);
        assert_eq!(tile_name_top(&with, "Terminal"), tile_name_top(&none_shown, "Terminal"));
        // Both pictures inside the area.
        for r in pictures(&with) {
            assert!(st.expand(1.0).contains_rect(r), "{r:?} in {st:?}");
        }
    }

    #[test]
    fn the_network_element_asks_for_a_frame_a_second_only_while_shown() {
        // `show` always asks for the next minute, and that wins in the last
        // second of one: start past the boundary when it is close.
        let left = until_next_minute(std::time::SystemTime::now());
        if left < Duration::from_secs(5) {
            std::thread::sleep(left);
        }
        let t0 = Instant::now();
        let mut menu = net_menu(10, t0);
        // No battery device: only the network element asks.
        menu.set_battery(Vec::new(), t0);
        // The fonts land in the first frames, which ask for the next at
        // once; where the tiles scroll, egui fades their scroll bar in over
        // the first frames.
        let settled = |covered| {
            let ctx = new_ctx();
            let mut delay = Duration::ZERO;
            for i in 0..8 {
                delay = status_frame_at(&ctx, &menu, t0, covered, Some(i as f64 * 0.5)).1;
            }
            delay
        };
        let shown = settled(false);
        // egui counts from the frame's start, a little before the request.
        assert!(
            shown <= Duration::from_secs(1) && shown > Duration::from_millis(900),
            "{shown:?}"
        );
        let covered = settled(true);
        assert!(covered > Duration::from_secs(1), "{covered:?}");
    }

    #[test]
    fn a_new_network_sample_uploads_only_its_picture_and_damages_only_its_cells() {
        let t0 = Instant::now();
        let ctx = new_ctx();
        let input = || egui::RawInput {
            screen_rect: Some(screen(1280.0, 720.0)),
            ..Default::default()
        };
        let frame = |menu: &Menu| {
            let icons: Vec<Option<Icon>> = menu.tiles.iter().map(|_| None).collect();
            ctx.run_ui(input(), |ui| {
                show(ui, menu, &icons, &Chrome::default(), t0);
            })
        };
        let menu = net_menu(100, t0);
        for _ in 0..3 {
            frame(&menu).textures_delta.clear();
        }
        // One second later: one more sample.
        let next = net_menu(101, t0);
        let mut out = frame(&next);
        let set: Vec<egui::TextureId> = out.textures_delta.set.keys().copied().collect();
        out.textures_delta.clear();
        assert_eq!(set.len(), 1, "only the network picture: {set:?}");
        let own = |id| {
            ctx.tex_manager()
                .read()
                .meta(id)
                .is_some_and(|m| m.name.starts_with(STATUS_TEXTURE))
        };
        let pics = pictures(&out.shapes);
        let net = pics[1];
        let prims = ctx.tessellate(out.shapes, 1.0);
        let dmg = crate::damage::texture_damage(&set, own, &prims, 1.0).expect("only a status texture");
        assert_eq!(
            dmg,
            crate::raster::IRect::from_points(net, 1.0),
            "its cells, not the canvas"
        );
        // The same sample again: nothing rendered, nothing uploaded.
        let mut out = frame(&next);
        assert!(out.textures_delta.set.is_empty());
        out.textures_delta.clear();
    }

    #[test]
    fn the_end_of_session_dialogs_draw_no_status_area() {
        let t0 = Instant::now();
        let mut menu = status_menu(five(), t0);
        menu.logout = Some(Logout::Failed { message: "x".into() });
        let (shapes, _) = status_frame(&new_ctx(), &menu, t0, false);
        assert!(pictures(&shapes).is_empty());
    }

    #[test]
    fn elapsed_is_minutes_and_seconds() {
        assert_eq!(elapsed(Duration::from_secs(0)), "0:00");
        assert_eq!(elapsed(Duration::from_secs(65)), "1:05");
    }

    fn screen(w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h))
    }

    #[test]
    fn the_header_has_32_points_margin_and_the_byline() {
        let menu = Menu::new(
            Grid {
                tiles: vec![],
                banner: vec![],
            },
            false,
        );
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(screen(1920.0, 1080.0)),
            ..Default::default()
        };
        let mut out = None;
        for _ in 0..2 {
            let mut o = ctx.run_ui(input.clone(), |ui| {
                show(ui, &menu, &[], &Chrome::default(), Instant::now());
            });
            o.textures_delta.clear();
            out = Some(o);
        }
        let shapes = out.unwrap().shapes;
        let find = |text: &str| {
            shapes
                .iter()
                .find_map(|c| match &c.shape {
                    egui::Shape::Text(t) if t.galley.text() == text => Some(t.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{text:?} not drawn"))
        };
        let word = find("ThinFloe");
        let by = find(BYLINE);
        let word_r = word.visual_bounding_rect();
        let by_r = by.visual_bounding_rect();
        let header = layout(screen(1920.0, 1080.0), 0, 0, 0, 0).header;
        assert!((word.pos.x - 32.0).abs() < 0.5, "wordmark at {:?}", word.pos);
        assert!(
            (by.pos.x - word.pos.x).abs() < 0.5,
            "byline left {:?} vs {:?}",
            by.pos,
            word.pos
        );
        assert!(by_r.top() >= word_r.bottom() - 2.0, "byline {by_r:?} under {word_r:?}");
        assert!(by_r.bottom() <= header.bottom(), "byline {by_r:?} inside {header:?}");
        let f = &by.galley.job.sections[0].format;
        assert_eq!(f.font_id, brand::geo(14.0));
        assert_eq!(f.color, MUTED);
        let right = shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text().contains(" · ") => Some(t.pos.x + t.galley.size().x),
                _ => None,
            })
            .next()
            .expect("header text drawn");
        assert!((right - (1920.0 - 32.0)).abs() < 0.5, "header text ends at {right}");
    }

    #[test]
    fn tiles_are_220_by_76_on_every_screen() {
        use crate::density::GROUP_W;
        for (w, h) in [(1920.0, 1080.0), (3840.0, 2160.0), (1280.0, 720.0), (1707.0, 960.0)] {
            let l = layout(screen(w, h), 8, 0, 0, 0);
            assert_eq!(l.tile, vec2(220.0, 76.0), "{w}x{h}: {:?}", l.tile);
            assert!((l.grid.width() - GROUP_W).abs() < 0.5, "{w}x{h}: {:?}", l.grid);
            assert!((l.grid.center().x - w / 2.0).abs() < 1.0, "{w}x{h}: {:?}", l.grid);
            assert!(l.grid.top() > l.header.bottom() && l.grid.bottom() < l.log.top());
            assert!(l.log.bottom() <= h && l.log.left() >= 0.0 && l.log.right() <= w);
        }
    }

    /// The screens the layout is checked on, with the rows of the shipped files
    /// there: one on a short screen, three on any other.
    const SHIPPED_SCREENS: [(f32, f32, usize); 5] = [
        (1280.0, 720.0, 1),
        (1366.0, 768.0, 1),
        (1440.0, 900.0, 3),
        (1920.0, 1080.0, 3),
        (3840.0, 2160.0, 3),
    ];

    #[test]
    fn tiles_and_status_area_are_centred_together_between_header_and_log() {
        use crate::density::GROUP_W;
        for (w, h, planned) in SHIPPED_SCREENS {
            let l = layout(screen(w, h), 8, 0, planned, planned);
            let st = l.status.expect("rows");
            assert_eq!(l.tile, vec2(220.0, 76.0), "{w}x{h}");
            assert!((st.top() - l.grid.bottom() - STATUS_GAP).abs() < 0.01, "{w}x{h}: {l:?}");
            let above = l.grid.top() - l.area.top();
            let below = l.area.bottom() - st.bottom();
            assert!((above - below).abs() < 0.01, "{w}x{h}: {above} above, {below} below");
            assert!(above >= GAP, "{w}x{h}: {above}");
            assert!((st.width() - GROUP_W).abs() < 0.5, "four columns: {st:?}");
            assert!((st.left() - l.grid.left()).abs() < 0.5, "columns line up: {st:?}");
            let p = planned as f32;
            assert!((st.height() - (p * l.tile.y + (p - 1.0) * TILE_GAP)).abs() < 0.01);
            assert!(l.grid.bottom() <= l.tiles.bottom() + 0.01, "{w}x{h}: no tile covered");
            assert_eq!(l.log, layout(screen(w, h), 8, 0, 0, 0).log, "the log stays as it is");
        }
        assert_eq!(
            layout(screen(1920.0, 1080.0), 8, 0, 9, 9).status.unwrap().height(),
            layout(screen(1920.0, 1080.0), 8, 0, 3, 3).status.unwrap().height(),
            "at most 3 rows"
        );
    }

    #[test]
    fn nothing_jumps_when_elements_appear_or_go() {
        for (w, h, planned) in SHIPPED_SCREENS {
            let all = layout(screen(w, h), 8, 0, planned, planned);
            let top = all.status.unwrap().top();
            for shown in 0..=planned {
                let l = layout(screen(w, h), 8, 0, shown, planned);
                assert_eq!(l.grid, all.grid, "{w}x{h}, {shown} shown");
                assert_eq!(l.tile, all.tile, "{w}x{h}, {shown} shown");
                assert_eq!(l.tiles, all.tiles, "{w}x{h}, {shown} shown");
                assert_eq!(l.log, all.log, "{w}x{h}, {shown} shown");
                match l.status {
                    None => assert_eq!(shown, 0, "{w}x{h}"),
                    Some(st) => {
                        assert_eq!(st.top(), top, "{w}x{h}, {shown} shown");
                        let n = shown as f32;
                        assert!((st.height() - (n * l.tile.y + (n - 1.0) * TILE_GAP)).abs() < 0.01);
                    }
                }
            }
        }
    }

    #[test]
    fn a_menu_with_no_status_file_keeps_its_tiles_centred_alone() {
        for (w, h, _) in SHIPPED_SCREENS {
            let l = layout(screen(w, h), 8, 0, 0, 0);
            assert_eq!(l.status, None);
            assert_eq!(l.tiles, l.area);
            let above = l.grid.top() - l.area.top();
            let below = l.area.bottom() - l.grid.bottom();
            assert!((above - below).abs() < 0.01, "{w}x{h}: {above} above, {below} below");
        }
    }

    #[test]
    fn where_it_does_not_fit_the_area_is_pinned_above_the_log_and_only_the_tiles_scroll() {
        // Four rows of tiles over the three rows of a tall screen, or the one
        // row of a short one, with the tiles at their full height.
        for (w, h, tiles, planned) in [(1280.0, 720.0, 16, 1), (1920.0, 900.0, 16, 3), (1280.0, 620.0, 12, 1)] {
            let l = layout(screen(w, h), tiles, 0, planned, planned);
            let st = l.status.unwrap();
            assert!(st.bottom() <= l.log.top() - GAP + 0.01, "{h}: {st:?} above {:?}", l.log);
            assert!(st.top() >= l.area.top(), "{h}: {st:?}");
            let r = planned as f32;
            assert!((st.height() - (r * l.tile.y + (r - 1.0) * TILE_GAP)).abs() < 0.01);
            assert!(l.tile.y >= TILE_H_MIN && l.tile.y <= TILE_H);
            // The tiles scroll in the band above it, from a GAP under the header.
            assert!(l.tiles.bottom() <= st.top() + 0.01, "{h}: {:?}", l.tiles);
            assert_eq!(l.tiles.top(), l.area.top());
            assert!((l.grid.top() - l.area.top() - GAP).abs() < 0.01, "{h}: {l:?}");
            assert!(l.grid.bottom() > l.tiles.bottom(), "{h}: {l:?}");
            // With fewer rows shown the same: no jump.
            let fewer = layout(screen(w, h), tiles, 0, 1, planned);
            assert_eq!(fewer.grid, l.grid, "{h}");
            assert_eq!(fewer.status.unwrap().top(), st.top(), "{h}");
        }
        // With room to spare nothing scrolls and both sit together.
        let l = layout(screen(1920.0, 1080.0), 8, 0, 1, 3);
        let st = l.status.unwrap();
        assert!(l.grid.bottom() <= l.tiles.bottom() + 0.01);
        assert!((st.top() - (l.grid.bottom() + STATUS_GAP)).abs() < 0.01);
        // A smaller screen shrinks tiles and cells together.
        let small = layout(screen(1280.0, 480.0), 9, 0, 1, 1);
        assert!(small.tile.y < TILE_H, "{:?}", small.tile);
        assert!((small.status.unwrap().height() - small.tile.y).abs() < 0.01);
        // Narrow screens stay inside.
        for (w, h, n) in [(1024.0, 560.0, 8), (700.0, 900.0, 3)] {
            let st = layout(screen(w, h), n, 0, 2, 3).status.unwrap();
            assert!(st.left() >= 0.0 && st.right() <= w, "{st:?}");
        }
    }

    #[test]
    fn the_group_never_exceeds_the_screen() {
        let l = layout(screen(900.0, 600.0), 8, 0, 0, 0);
        assert!(l.grid.left() >= 0.0 && l.grid.right() <= 900.0, "{:?}", l.grid);
    }

    #[test]
    fn folding_a_window_takes_only_from_the_log() {
        let open = layout(screen(1920.0, 1080.0), 8, 0, 0, 0);
        let folded = layout(screen(1920.0, 1080.0), 8, 1, 0, 0);
        assert_eq!(open.area, folded.area);
        assert_eq!(open.grid, folded.grid);
        assert_eq!(open.log.top(), folded.log.top());
        assert_eq!(folded.log.bottom(), folded.strip.unwrap().top() - STRIP_GAP);
    }

    #[test]
    fn the_log_keeps_its_minimum_and_the_area_gives_the_rest() {
        // 0.3 * 600 = 180 is 16 above the minimum, less than STRIP_H + STRIP_GAP.
        let min = LOG_MIN_ROWS as f32 * LOG_ROW + 2.0 * LOG_PAD;
        let open = layout(screen(1024.0, 600.0), 8, 0, 0, 0);
        let folded = layout(screen(1024.0, 600.0), 8, 1, 0, 0);
        assert!(open.log.height() - min < STRIP_H + STRIP_GAP, "{:?}", open.log);
        assert!(open.log.height() > min, "{:?}", open.log);
        assert!((folded.log.height() - min).abs() < 0.5, "{:?}", folded.log);
        let strip = folded.strip.unwrap();
        assert!((folded.log.bottom() - (strip.top() - STRIP_GAP)).abs() < 0.5);
        assert_eq!(folded.area.bottom(), folded.log.top());
        assert!(folded.area.bottom() < open.area.bottom());
    }

    #[test]
    fn nine_tiles_fit_between_header_and_log_on_1280_by_720() {
        let l = layout(screen(1280.0, 720.0), 9, 0, 0, 0);
        assert!(
            l.grid.top() > l.header.bottom() && l.grid.bottom() < l.log.top(),
            "{l:?}"
        );
        assert!(l.grid.left() >= 0.0 && l.grid.right() <= 1280.0, "{l:?}");
        assert!(l.tile.x > l.tile.y * 2.5, "{:?}", l.tile);
    }

    fn texts_of(menu: &Menu, icons: &[Option<Icon>]) -> Vec<String> {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(screen(1280.0, 720.0)),
            ..Default::default()
        };
        let mut out = None;
        for _ in 0..2 {
            let mut o = ctx.run_ui(input.clone(), |ui| {
                show(ui, menu, icons, &Chrome::default(), Instant::now());
            });
            o.textures_delta.clear();
            out = Some(o);
        }
        out.unwrap()
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    fn addon_grid() -> Grid {
        let f = |n: &str, t: &str| crate::tiles::SourceFile {
            name: n.into(),
            entry: crate::desktop::parse(t),
        };
        Grid {
            tiles: crate::tiles::merge(
                &[
                    f(
                        "10-thinlinc.desktop",
                        "[Desktop Entry]\nName=ThinLinc\nExec=tlclient\nX-TF-Source-Page=https://e.example/dl\n\
                         X-TF-Source-Pattern=x\n",
                    ),
                    f(
                        "60-settings.desktop",
                        "[Desktop Entry]\nName=Settings\nX-TF-Action=settings\n",
                    ),
                ],
                &[],
            ),
            banner: vec![],
        }
    }

    fn failed() -> crate::addons::AddonStatus {
        crate::addons::AddonStatus {
            id: "10-thinlinc".into(),
            ticked: true,
            state: crate::addons::AddonState::InstallFailed,
            installed: None,
            pending: None,
            auto: false,
            message: Some("HTTP 404".into()),
        }
    }

    #[test]
    fn the_settings_screen_draws_a_row_per_addon() {
        let mut menu = Menu::new(addon_grid(), false);
        menu.activate(1, Instant::now());
        menu.set_addons(vec![failed()]);
        let texts = texts_of(&menu, &[None, None]);
        for want in [
            "Add-ons",
            "not checked yet",
            "Check now",
            "Back",
            "ThinLinc",
            "https://e.example/dl",
            "not installed · install failed: HTTP 404 · updates only by hand",
            "Install now",
        ] {
            assert!(texts.iter().any(|t| t == want), "{want} not in {texts:?}");
        }
    }

    #[test]
    fn the_retry_dialog_names_the_addon_and_why() {
        let mut menu = Menu::new(addon_grid(), false);
        menu.set_addons(vec![failed()]);
        let texts = texts_of(&menu, &[None, None]);
        for want in ["ThinLinc could not be installed: HTTP 404", "Retry", "Later"] {
            assert!(texts.iter().any(|t| t == want), "{want} not in {texts:?}");
        }
    }

    /// Four add-ons: waiting update, current, failed install, manual.
    fn four_addons() -> (Menu, usize) {
        let f = |n: &str, t: &str| crate::tiles::SourceFile {
            name: n.into(),
            entry: crate::desktop::parse(t),
        };
        let tile = |file: &str, name: &str| {
            f(
                file,
                &format!(
                    "[Desktop Entry]\nName={name}\nExec=x\nX-TF-Source-Page=https://e.example/{name}\nX-TF-Source-Pattern=x\n"
                ),
            )
        };
        let grid = Grid {
            tiles: crate::tiles::merge(
                &[
                    tile("10-thinlinc.desktop", "ThinLinc"),
                    tile("20-oxulnk.desktop", "OxuLnk"),
                    tile("30-remmina.desktop", "Remmina"),
                    tile("40-manual.desktop", "Manual tool"),
                    f(
                        "60-settings.desktop",
                        "[Desktop Entry]\nName=Settings\nX-TF-Action=settings\n",
                    ),
                ],
                &[],
            ),
            banner: vec![],
        };
        let n = grid.tiles.len();
        let mut menu = Menu::new(grid, false);
        let st = |id: &str, state, installed: Option<&str>, pending: Option<&str>, auto, message: Option<&str>| {
            crate::addons::AddonStatus {
                id: id.into(),
                ticked: true,
                state,
                installed: installed.map(Into::into),
                pending: pending.map(Into::into),
                auto,
                message: message.map(Into::into),
            }
        };
        use crate::addons::AddonState::*;
        menu.set_addons(vec![
            st("10-thinlinc", Pending, Some("4.21.0"), Some("4.22.0"), true, None),
            st("20-oxulnk", Installed, Some("0.1.0"), None, true, None),
            st("30-remmina", InstallFailed, None, None, true, Some("HTTP 404")),
            st("40-manual", Installed, Some("2.0"), None, false, None),
        ]);
        // The Retry question for the failed install is not under test here.
        menu.retry = None;
        (menu, n)
    }

    #[test]
    fn only_a_row_with_a_waiting_update_has_the_ice_edge() {
        let (mut menu, n) = four_addons();
        let settings = menu.tiles.iter().position(|t| t.name == "Settings").unwrap();
        menu.activate(settings, Instant::now());
        let edges = |m: &Menu| {
            shapes_of(m, n, Instant::now())
                .iter()
                .filter(|c| matches!(&c.shape, egui::Shape::Rect(r) if r.fill == ICE && (r.rect.width() - 5.0).abs() < 0.01))
                .count()
        };
        assert_eq!(edges(&menu), 1);
        // Nothing waits any more: no edge, though the manual row still has Update now.
        menu.set_addons(vec![]);
        assert_eq!(edges(&menu), 0);
    }

    #[test]
    fn the_dialog_title_is_drawn_in_geo() {
        let mut menu = Menu::new(addon_grid(), false);
        menu.set_addons(vec![failed()]);
        let shapes = shapes_of(&menu, 2, Instant::now());
        let family = shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text() == "Add-on not installed" => {
                    Some(t.galley.job.sections[0].format.font_id.family.clone())
                }
                _ => None,
            })
            .expect("dialog title not drawn");
        assert_eq!(family, egui::FontFamily::Name(crate::brand::GEO.into()));
    }

    #[test]
    fn the_log_keeps_its_minimum_rows_on_a_short_screen() {
        let l = layout(screen(1280.0, 480.0), 6, 0, 0, 0);
        assert!(l.log.height() >= LOG_MIN_ROWS as f32 * LOG_ROW, "{:?}", l.log);
    }

    #[test]
    fn many_tiles_start_at_the_top_and_overflow_downwards() {
        let l = layout(screen(1920.0, 1080.0), 38, 0, 0, 0);
        assert!(
            l.grid.top() >= l.area.top() && l.grid.height() > l.area.height(),
            "{:?}",
            l
        );
        assert!((l.grid.center().x - 960.0).abs() < 0.5);
    }

    #[test]
    fn a_log_line_is_drawn_below_the_tiles_with_its_time() {
        let mut menu = Menu::new(
            Grid {
                tiles: vec![Tile {
                    file: "1.desktop".into(),
                    name: "TileName".into(),
                    comment: None,
                    icon: None,
                    action: Action::Run,
                    argv: vec!["x".into()],
                    terminal: false,
                    save_on_exit: false,
                    multiple: false,
                    origin: Origin::User,
                    problem: None,
                    note: None,
                    addon: false,
                    source: None,
                }],
                banner: vec![],
            },
            false,
        );
        menu.log.push(crate::log::Line {
            time: "12:34:56".into(),
            source: Some("Tool".into()),
            runs: vec![crate::log::Run::plain("hello-log")],
            kind: crate::log::Kind::Output,
        });
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(screen(1280.0, 720.0)),
            ..Default::default()
        };
        // Two passes: the font atlas and the scroll areas settle in the first.
        let mut out = None;
        for _ in 0..2 {
            let mut o = ctx.run_ui(input.clone(), |ui| {
                show(ui, &menu, &[None], &Chrome::default(), Instant::now());
            });
            // The font atlas upload is not applied anywhere here.
            o.textures_delta.clear();
            out = Some(o);
        }
        let out = out.unwrap();
        let find = |needle: &str| {
            out.shapes
                .iter()
                .find_map(|c| match &c.shape {
                    egui::Shape::Text(t) if t.galley.text().contains(needle) => Some(t.visual_bounding_rect()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{needle} not drawn"))
        };
        let name = find("TileName");
        let line = find("hello-log");
        assert!(line.top() > name.bottom(), "log line {line:?} not below tile {name:?}");
        let text = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text().contains("hello-log") => Some(t.galley.text().to_string()),
                _ => None,
            })
            .unwrap();
        assert!(text.starts_with("12:34:56"), "{text}");
        assert!(text.contains("Tool | hello-log"), "{text}");
    }

    fn plain_tile(name: &str) -> Tile {
        Tile {
            file: format!("{name}.desktop"),
            name: name.into(),
            comment: None,
            icon: None,
            action: Action::Run,
            argv: vec!["x".into()],
            terminal: false,
            save_on_exit: false,
            multiple: false,
            origin: Origin::Shipped,
            problem: None,
            note: None,
            addon: false,
            source: None,
        }
    }

    // `layout()` was right while the drawn grid sat a GAP above and left of
    // it: the screenshot of 2026-10-08 showed the top row touching the screen
    // edge. This checks the painted tiles, not the computed rectangle.
    #[test]
    fn the_painted_grid_is_where_the_layout_puts_it_with_centred_content() {
        let names = ["T0", "T1", "T2", "T3", "T4", "T5"];
        let menu = Menu::new(
            Grid {
                tiles: names.iter().map(|n| plain_tile(n)).collect(),
                banner: vec![],
            },
            false,
        );
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(screen(1280.0, 720.0)),
            ..Default::default()
        };
        let icons: Vec<_> = names.iter().map(|_| None).collect();
        let mut out = None;
        for _ in 0..2 {
            let mut o = ctx.run_ui(input.clone(), |ui| {
                show(ui, &menu, &icons, &Chrome::default(), Instant::now());
            });
            o.textures_delta.clear();
            out = Some(o);
        }
        let out = out.unwrap();
        let l = layout(screen(1280.0, 720.0), names.len(), 0, 0, 0);
        let tiles: Vec<Rect> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.fill != Color32::TRANSPARENT && (r.rect.size() - l.tile).length() < 0.5 => {
                    Some(r.rect)
                }
                _ => None,
            })
            .collect();
        assert_eq!(tiles.len(), names.len(), "one band per tile");
        let painted = tiles.iter().fold(Rect::NOTHING, |a, r| a.union(*r));
        assert!(
            (painted.min - l.grid.min).length() < 0.5 && (painted.max - l.grid.max).length() < 0.5,
            "painted {painted:?}, layout {:?}",
            l.grid
        );
        assert!(painted.top() >= GAP, "{painted:?}");
        assert!((painted.center().x - 640.0).abs() < 0.5, "{painted:?}");
        // The letter placeholder and the name, as one block, sit in the
        // middle of the tile, not in its top half.
        let tile = tiles
            .iter()
            .find(|r| r.contains(painted.min + Vec2::splat(1.0)))
            .unwrap();
        let name = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text() == "T0" => Some(Rect::from_min_size(t.pos, t.galley.size())),
                _ => None,
            })
            .unwrap();
        let icon = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Rect(r)
                    if r.fill != Color32::TRANSPARENT
                        && tile.contains_rect(r.rect)
                        && r.rect.width() < l.tile.x - 1.0 =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .expect("letter placeholder drawn");
        assert_eq!(icon.top(), icon.top().round(), "icon on a whole pixel");
        assert!(icon.left() - tile.left() >= 20.0, "icon clear of the edge");
        let above = icon.top() - tile.top();
        let below = tile.bottom() - icon.bottom();
        assert!(
            (above - below).abs() <= 1.0,
            "icon {above} from the top, {below} from the bottom"
        );
        assert!(name.left() > icon.right(), "name {name:?} right of the icon {icon:?}");
        let above = name.top() - tile.top();
        let below = tile.bottom() - name.bottom();
        assert!(
            (above - below).abs() <= 3.0,
            "name {above} from the top, {below} from the bottom"
        );
    }

    #[test]
    fn a_coloured_log_line_is_drawn_in_its_colours() {
        let mut menu = Menu::new(
            Grid {
                tiles: vec![plain_tile("T0")],
                banner: vec![],
            },
            false,
        );
        menu.log.push(crate::log::Line {
            time: "12:34:56".into(),
            source: Some("oxulnk".into()),
            runs: crate::log::parse(b"plain \x1b[33mYELLOWPART\x1b[0m end"),
            kind: crate::log::Kind::Output,
        });
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(screen(1280.0, 720.0)),
            ..Default::default()
        };
        let mut out = None;
        for _ in 0..2 {
            let mut o = ctx.run_ui(input.clone(), |ui| {
                show(ui, &menu, &[None], &Chrome::default(), Instant::now());
            });
            o.textures_delta.clear();
            out = Some(o);
        }
        let out = out.unwrap();
        let galley = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text().contains("YELLOWPART") => Some(t.galley.clone()),
                _ => None,
            })
            .expect("log line drawn");
        let text = galley.text();
        let at = text.find("YELLOWPART").unwrap();
        let section = galley
            .job
            .sections
            .iter()
            .find(|s| s.byte_range.contains(&egui::text::ByteIndex(at)))
            .unwrap();
        assert_eq!(section.format.color, ansi_color(Ansi::Basic(3)));
        assert!(!text.contains("[33m"), "{text}");
    }

    #[test]
    fn a_log_line_has_the_colour_roles() {
        let t = Theme::current(&{
            let c = egui::Context::default();
            brand::apply(&c);
            c
        });
        let line = |kind| crate::log::Line {
            time: "12:00:00".into(),
            source: Some("save-home".into()),
            runs: vec![crate::log::Run::plain("text")],
            kind,
        };
        let colour_of = |job: &LayoutJob, needle: &str| {
            let at = job.text.find(needle).unwrap();
            job.sections
                .iter()
                .find(|s| s.byte_range.contains(&egui::text::ByteIndex(at)))
                .unwrap()
                .format
                .color
        };
        let job = log_job(&line(crate::log::Kind::Good), &t);
        assert_eq!(colour_of(&job, "12:00"), TIME);
        assert_eq!(colour_of(&job, "save-home"), ICE);
        assert_eq!(colour_of(&job, "text"), GOOD);
        assert_eq!(colour_of(&log_job(&line(crate::log::Kind::Warn), &t), "text"), WARN);
    }

    fn shapes_of(menu: &Menu, tiles: usize, now: Instant) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(screen(1280.0, 720.0)),
            ..Default::default()
        };
        let icons: Vec<Option<Icon>> = (0..tiles).map(|_| None).collect();
        let mut out = None;
        for _ in 0..2 {
            let mut o = ctx.run_ui(input.clone(), |ui| {
                show(ui, menu, &icons, &Chrome::default(), now);
            });
            o.textures_delta.clear();
            out = Some(o);
        }
        out.unwrap().shapes
    }

    fn all_texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_running_tile_with_a_pending_update_shows_the_edge_and_the_marker() {
        let mut t0 = plain_tile("T0");
        t0.addon = true;
        let mut menu = Menu::new(
            Grid {
                tiles: vec![t0],
                banner: vec![],
            },
            false,
        );
        menu.set_addons(vec![crate::addons::AddonStatus {
            id: "T0".into(),
            ticked: true,
            state: crate::addons::AddonState::Pending,
            installed: Some("1".into()),
            pending: Some("2".into()),
            auto: true,
            message: None,
        }]);
        let now = Instant::now();
        menu.activate(0, now);
        assert!(menu.active(0) && menu.pending_update(0));
        let shapes = shapes_of(&menu, 1, now);
        assert!(
            shapes.iter().any(
                |c| matches!(&c.shape, egui::Shape::Rect(r) if r.fill == ICE && (r.rect.width() - 5.0).abs() < 0.01)
            ),
            "edge"
        );
        assert!(all_texts(&shapes).iter().any(|t| t == "update"), "marker");
    }

    #[test]
    fn a_reworded_tile_that_lost_keys_says_so_when_it_has_no_comment() {
        let mut t0 = plain_tile("T0");
        t0.origin = Origin::Reworded {
            ignored: vec!["Exec".into()],
        };
        let menu = Menu::new(
            Grid {
                tiles: vec![t0],
                banner: vec![],
            },
            false,
        );
        let texts = all_texts(&shapes_of(&menu, 1, Instant::now()));
        assert!(texts.iter().any(|t| t == "reworded; ignored: Exec"), "{texts:?}");
    }

    #[test]
    fn a_tile_with_two_copies_shows_the_edge_and_the_count() {
        let mut t1 = plain_tile("T1");
        t1.multiple = true;
        let mut menu = Menu::new(
            Grid {
                tiles: vec![plain_tile("T0"), t1, plain_tile("T2")],
                banner: vec![],
            },
            false,
        );
        let now = Instant::now();
        menu.activate(0, now);
        menu.activate(1, now);
        menu.activate(1, now);
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(screen(1280.0, 720.0)),
            ..Default::default()
        };
        let mut out = None;
        for _ in 0..2 {
            let mut o = ctx.run_ui(input.clone(), |ui| {
                show(ui, &menu, &[None, None, None], &Chrome::default(), now);
            });
            o.textures_delta.clear();
            out = Some(o);
        }
        let out = out.unwrap();
        let l = layout(screen(1280.0, 720.0), 3, 0, 0, 0);
        // The ice edge: a 5 wide bar at the left of the band, full height.
        let edges: Vec<Rect> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.fill == ICE && (r.rect.width() - 5.0).abs() < 0.01 => Some(r.rect),
                _ => None,
            })
            .collect();
        assert_eq!(edges.len(), 2, "an edge on each running tile only: {edges:?}");
        for e in &edges {
            assert!((e.height() - l.tile.y).abs() < 0.5, "{e:?}");
            assert!(
                (e.left() - l.grid.left()).abs() < 0.5
                    || (e.left() - (l.grid.left() + l.tile.x + TILE_GAP)).abs() < 0.5
            );
        }
        let texts: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t == "×2"), "{texts:?}");
        assert_eq!(
            texts.iter().filter(|t| t.starts_with('×')).count(),
            1,
            "one copy shows no count"
        );
    }

    #[test]
    fn after_sigkill_the_stopping_dialog_does_not_spin() {
        // Once SIGKILL is sent nothing is left to time; each remaining step
        // arrives as a job event. A zero repaint delay here redraws at full
        // speed until then, forever for a program stuck in D state.
        let mut menu = Menu::new(
            Grid {
                tiles: vec![plain_tile("T0")],
                banner: vec![],
            },
            false,
        );
        let now = Instant::now();
        menu.activate(0, now);
        menu.logout = Some(Logout::Stopping {
            kill_at: now,
            killed: true,
        });
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(screen(1280.0, 720.0)),
            ..Default::default()
        };
        // Time moves, so the dialog's own fade-in settles first.
        let mut delay = Duration::ZERO;
        for i in 0..4 {
            let mut frame = input.clone();
            frame.time = Some(i as f64);
            let mut o = ctx.run_ui(frame, |ui| {
                show(ui, &menu, &[None], &Chrome::default(), now + Duration::from_secs(1));
            });
            o.textures_delta.clear();
            delay = o.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        }
        assert!(!delay.is_zero(), "repaint delay {delay:?}");
    }

    fn folded_menu(n: usize) -> Menu {
        let mut menu = Menu::new(
            Grid {
                tiles: vec![plain_tile("T0")],
                banner: vec![],
            },
            false,
        );
        menu.folded = (0..n as u64)
            .map(|i| crate::windows::Folded {
                id: 100 + i,
                title: format!("ThinLinc session number {i} on a host with a very long name.example.com"),
                app_id: "x".into(),
            })
            .collect();
        menu
    }

    /// One frame of `show` with the given extra input events; returns the
    /// chips painted (rects of height `CHIP_H` filled with TILE or hover) and
    /// the event.
    fn strip_frame(
        ctx: &egui::Context,
        menu: &Menu,
        size: Vec2,
        events: Vec<egui::Event>,
    ) -> (Vec<Rect>, Option<UiEvent>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), size)),
            events,
            // Wheel scrolling is smoothed over time.
            time: Some(ctx.cumulative_pass_nr() as f64 * 0.1),
            ..Default::default()
        };
        let mut ev = None;
        let mut out = ctx.run_ui(input, |ui| {
            ev = show(ui, menu, &[None], &Chrome::default(), Instant::now());
        });
        out.textures_delta.clear();
        let chips = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r)
                    if (r.rect.height() - CHIP_H).abs() < 0.01 && (r.fill == TILE || r.fill == TILE_HOVER) =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .collect();
        (chips, ev)
    }

    fn click_at(p: egui::Pos2) -> Vec<egui::Event> {
        let b = |pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        vec![egui::Event::PointerMoved(p), b(true), b(false)]
    }

    #[test]
    fn no_folded_window_means_no_strip_and_the_log_reaches_the_bottom() {
        let l = layout(screen(1920.0, 1080.0), 8, 0, 0, 0);
        assert!(l.strip.is_none());
        assert!(1080.0 - l.log.bottom() <= GAP + 0.5);
    }

    #[test]
    fn a_folded_window_puts_the_strip_below_the_log() {
        let l = layout(screen(1920.0, 1080.0), 8, 2, 0, 0);
        let s = l.strip.unwrap();
        assert!(s.top() >= l.log.bottom() && s.bottom() <= 1080.0);
        assert!(s.height() >= CHIP_H);
    }

    #[test]
    fn a_chip_click_restores_that_window() {
        let menu = folded_menu(2);
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let size = vec2(1280.0, 720.0);
        let (chips, _) = strip_frame(&ctx, &menu, size, vec![]);
        let (chips2, _) = strip_frame(&ctx, &menu, size, vec![]);
        assert_eq!(chips.len(), 2, "{chips:?}");
        assert_eq!(chips, chips2);
        let (_, ev) = strip_frame(&ctx, &menu, size, click_at(chips[1].center()));
        assert_eq!(ev, Some(UiEvent::Unfold(101)));
    }

    #[test]
    fn ten_folded_windows_on_1280_by_720_stay_reachable() {
        let menu = folded_menu(10);
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let size = vec2(1280.0, 720.0);
        let strip = layout(screen(1280.0, 720.0), 1, 10, 0, 0).strip.unwrap();
        let over = strip.center();
        strip_frame(&ctx, &menu, size, vec![]);
        let (first, _) = strip_frame(&ctx, &menu, size, vec![]);
        // The first frames show the left end only: the last chip is out.
        let reachable = |c: &Rect| strip.contains_rect(*c);
        assert!(!first.iter().all(reachable), "ten long chips cannot fit one row");
        // Wheel and drag both move the row; the wheel is tried here.
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -4000.0),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
        };
        for _ in 0..12 {
            strip_frame(&ctx, &menu, size, vec![egui::Event::PointerMoved(over), wheel.clone()]);
        }
        let (chips, _) = strip_frame(&ctx, &menu, size, vec![egui::Event::PointerMoved(over)]);
        let last = chips.iter().max_by(|a, b| a.right().total_cmp(&b.right())).unwrap();
        assert!(strip.contains(last.center()), "last chip {last:?} not inside {strip:?}");
        assert!(last.right() <= strip.right() + 1.0);
        let (_, ev) = strip_frame(&ctx, &menu, size, click_at(last.center()));
        assert_eq!(ev, Some(UiEvent::Unfold(109)));
    }

    #[test]
    fn dragging_the_strip_scrolls_it_without_clicking_a_chip() {
        let menu = folded_menu(10);
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let size = vec2(1280.0, 720.0);
        let strip = layout(screen(1280.0, 720.0), 1, 10, 0, 0).strip.unwrap();
        strip_frame(&ctx, &menu, size, vec![]);
        let (before, _) = strip_frame(&ctx, &menu, size, vec![]);
        let from = before[0].center();
        let b = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        strip_frame(&ctx, &menu, size, vec![egui::Event::PointerMoved(from), b(from, true)]);
        let mut x = from.x;
        let mut clicked = None;
        while x > from.x - 600.0 {
            x -= 60.0;
            let (_, ev) = strip_frame(&ctx, &menu, size, vec![egui::Event::PointerMoved(pos2(x, from.y))]);
            clicked = clicked.or(ev);
        }
        let (_, ev) = strip_frame(&ctx, &menu, size, vec![b(pos2(x, from.y), false)]);
        clicked = clicked.or(ev);
        let (after, _) = strip_frame(&ctx, &menu, size, vec![]);
        assert!(
            after[0].left() < before[0].left() - 300.0,
            "{:?} -> {:?}",
            before[0],
            after[0]
        );
        assert!(clicked.is_none(), "a drag is not a click: {clicked:?}");
        assert!(strip.height() > 0.0);
    }

    /// The shapes of the Settings screen of `menu` on a `w` x `h` screen.
    fn settings_shapes(menu: &Menu, w: f32, h: f32) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(screen(w, h)),
            ..Default::default()
        };
        let icons: Vec<Option<Icon>> = menu.tiles.iter().map(|_| None).collect();
        let mut shapes = vec![];
        for _ in 0..2 {
            let mut o = ctx.run_ui(input.clone(), |ui| {
                show(ui, menu, &icons, &Chrome::default(), Instant::now());
            });
            o.textures_delta.clear();
            shapes = o.shapes;
        }
        shapes
    }

    /// The Settings menu of `four_addons()`, open.
    fn open_settings() -> Menu {
        let (mut menu, _) = four_addons();
        let settings = menu.tiles.iter().position(|t| t.name == "Settings").unwrap();
        menu.activate(settings, Instant::now());
        menu
    }

    fn text_rects(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, Rect)> {
        shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => {
                    // What is drawn: a cut-off line ends with the ellipsis glyph.
                    let drawn: String = t.galley.rows.iter().map(|r| r.text()).collect();
                    Some((drawn, t.galley.rect.translate(t.pos.to_vec2())))
                }
                _ => None,
            })
            .collect()
    }

    fn row_rects(shapes: &[egui::epaint::ClippedShape]) -> Vec<Rect> {
        shapes
            .iter()
            .filter_map(|c| match &c.shape {
                // The log below is TILE-filled too; a row is far lower.
                egui::Shape::Rect(r) if r.fill == TILE && r.rect.height() < 200.0 => Some(r.rect),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn settings_draws_nothing_rounded() {
        let shapes = settings_shapes(&open_settings(), 1280.0, 720.0);
        for c in &shapes {
            if let egui::Shape::Rect(r) = &c.shape {
                assert_eq!(r.corner_radius, egui::CornerRadius::ZERO, "rounded {:?}", r.rect);
            }
        }
    }

    #[test]
    fn the_settings_panel_is_as_wide_as_the_tile_group() {
        let rows = row_rects(&settings_shapes(&open_settings(), 1920.0, 1080.0));
        assert_eq!(rows.len(), 4, "{rows:?}");
        for r in rows {
            assert!((r.width() - GROUP_W).abs() < 1.0, "{r:?}");
            assert!((r.center().x - 960.0).abs() < 1.0, "{r:?}");
        }
    }

    /// The drawn face of the button whose label is `label`: the rect around
    /// it filled in the button's colour (the primary one for Update now, the
    /// secondary one for Install now).
    fn button_face(shapes: &[egui::epaint::ClippedShape], label: Rect, update: bool) -> Rect {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let p = Theme::current(&ctx).palette;
        let fill = if update { p.primary } else { p.secondary };
        shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.fill == fill && r.rect.contains_rect(label) => Some(r.rect),
                _ => None,
            })
            .expect("button face")
    }

    fn label_is_update(inside: &[&(String, Rect)]) -> bool {
        inside.iter().any(|(t, _)| t == "Update now")
    }

    #[test]
    fn a_row_lays_out_name_source_then_status_line_and_button_right() {
        let shapes = settings_shapes(&open_settings(), 1920.0, 1080.0);
        let texts = text_rects(&shapes);
        let rows = row_rects(&shapes);
        let mut buttons = 0;
        for row in rows {
            let inside: Vec<_> = texts.iter().filter(|(_, r)| row.contains_rect(*r)).collect();
            let find = |f: &dyn Fn(&str) -> bool| inside.iter().find(|(t, _)| f(t)).map(|(_, r)| *r);
            let name = find(&|t| ["ThinLinc", "OxuLnk", "Remmina", "Manual tool"].contains(&t)).expect("name");
            let source = find(&|t| t.starts_with("https://")).expect("source");
            let status = find(&|t| t.contains("installed") || t.contains("up to date")).expect("status");
            assert!(source.left() > name.right(), "{name:?} {source:?}");
            assert!((source.center().y - name.center().y).abs() < 2.0, "{name:?} {source:?}");
            assert!(status.top() >= name.bottom() - 0.5, "{name:?} {status:?}");
            if let Some(b) = find(&|t| t == "Install now" || t == "Update now") {
                buttons += 1;
                let face = button_face(&shapes, b, label_is_update(&inside));
                assert!(face.contains_rect(b), "{b:?} not inside {face:?}");
                assert!(
                    (face.right() - (row.right() - ROW_RIGHT as f32)).abs() < 2.0,
                    "{face:?} {row:?}"
                );
                assert!((b.center().y - row.center().y).abs() < 2.0, "{b:?} {row:?}");
            }
        }
        assert_eq!(buttons, 3);
    }

    /// `four_addons()` with the source of the tile `name` replaced by 300 `x`.
    fn long_source_shapes(name: &str) -> (Vec<egui::epaint::ClippedShape>, String) {
        let mut menu = open_settings();
        let i = menu.tiles.iter().position(|t| t.name == name).unwrap();
        let long = "x".repeat(300);
        menu.tiles[i].source = Some(long.clone());
        (settings_shapes(&menu, 1920.0, 1080.0), long)
    }

    #[test]
    fn a_row_without_a_button_reaches_the_right_padding() {
        // OxuLnk is installed and current: no button.
        let (shapes, long) = long_source_shapes("OxuLnk");
        let texts = text_rects(&shapes);
        let (text, src) = texts
            .iter()
            .find(|(t, _)| t.starts_with('x'))
            .expect("source not drawn");
        assert!(text.ends_with('…') && text.len() < long.len(), "{text} {src:?}");
        let row = row_rects(&shapes).into_iter().find(|r| r.contains_rect(*src)).unwrap();
        assert!(src.right() > row.right() - ROW_RIGHT as f32 - 8.0, "{src:?} {row:?}");
    }

    #[test]
    fn a_long_name_leaves_room_for_source_and_button() {
        let mut menu = open_settings();
        let i = menu.tiles.iter().position(|t| t.name == "ThinLinc").unwrap();
        menu.tiles[i].name = "N".repeat(300);
        menu.tiles[i].source = Some("https://example.org/client".into());
        let shapes = settings_shapes(&menu, 1920.0, 1080.0);
        let texts = text_rects(&shapes);
        let (name, name_r) = texts.iter().find(|(t, _)| t.starts_with("NN")).expect("name");
        let (_, src) = texts
            .iter()
            .find(|(t, _)| t.starts_with("https://example"))
            .expect("source");
        let (_, button) = texts.iter().find(|(t, _)| t == "Update now").expect("button");
        assert!(name.ends_with('…'), "{name}");
        assert!(name_r.right() <= src.left(), "{name_r:?} {src:?}");
        assert!(src.right() <= button.left(), "{src:?} {button:?}");
        let row = row_rects(&shapes).into_iter().find(|r| r.contains_rect(*src)).unwrap();
        assert!(row.contains_rect(*button) && row.contains_rect(*name_r), "{row:?}");
    }

    #[test]
    fn a_long_status_does_not_push_the_button_off() {
        let mut menu = open_settings();
        let st = crate::addons::AddonStatus {
            id: "30-remmina".into(),
            ticked: true,
            state: crate::addons::AddonState::InstallFailed,
            installed: None,
            pending: None,
            auto: true,
            message: Some("y".repeat(300)),
        };
        menu.set_addons(vec![st]);
        menu.retry = None;
        let shapes = settings_shapes(&menu, 1920.0, 1080.0);
        let texts = text_rects(&shapes);
        let (text, status) = texts
            .iter()
            .find(|(t, _)| t.starts_with("not installed · install failed: yyy"))
            .expect("status");
        assert!(text.ends_with('…'), "{text}");
        let row = row_rects(&shapes)
            .into_iter()
            .find(|r| r.contains_rect(*status))
            .unwrap();
        let (_, label) = texts
            .iter()
            .find(|(t, r)| t == "Install now" && row.contains_rect(*r))
            .expect("button");
        let face = button_face(&shapes, *label, false);
        assert!(row.contains_rect(face), "{face:?} {row:?}");
        assert!(status.right() <= face.left(), "{status:?} {face:?}");
    }

    #[test]
    fn a_long_source_does_not_push_the_button_off() {
        // ThinLinc has Update now.
        let (shapes, _) = long_source_shapes("ThinLinc");
        let texts = text_rects(&shapes);
        let (text, src) = texts.iter().find(|(t, _)| t.starts_with('x')).expect("source");
        let (_, button) = texts.iter().find(|(t, _)| t == "Update now").expect("button");
        assert!(text.ends_with('…'), "{text}");
        let row = row_rects(&shapes).into_iter().find(|r| r.contains_rect(*src)).unwrap();
        assert!(row.contains_rect(*button), "{button:?} {row:?}");
        assert!(src.right() <= button.left(), "{src:?} {button:?}");
    }

    #[test]
    fn shrinking_averages_the_area_each_pixel_covers() {
        let (b, w) = (Color32::BLACK, Color32::WHITE);
        // A 4 by 4 checkerboard of single pixels is grey at 2 by 2.
        let checker = ColorImage::new(
            [4, 4],
            (0..16).map(|i| if (i + i / 4) % 2 == 0 { w } else { b }).collect(),
        );
        let small = shrink(&checker, 2);
        assert_eq!(small.size, [2, 2]);
        assert!(
            small.pixels.iter().all(|c| c.r() == 128 && c.a() == 255),
            "{:?}",
            small.pixels
        );
        // The same size is the same image.
        assert_eq!(shrink(&checker, 4).pixels, checker.pixels);
        // 3 to 2: the middle source pixel is split between both.
        let row = ColorImage::new([3, 3], [w, b, b].repeat(3));
        let px = shrink(&row, 2).pixels;
        assert_eq!((px[0].r(), px[1].r()), (170, 0), "{px:?}");
    }

    #[test]
    fn every_icon_is_drawn_at_its_own_size_on_whole_pixels() {
        for ppp in [1.0f32, 1.25, 2.0] {
            let mut menu = folded_menu(1);
            menu.tiles[0].argv = vec!["/usr/bin/x".into()];
            let ctx = egui::Context::default();
            crate::brand::apply(&ctx);
            let side = render_side(ppp) as usize;
            let img = ColorImage::new([side, side], vec![Color32::WHITE; side * side]);
            let icon = Icon::new(&ctx, "t0", &img, ppp);
            let icons = vec![Some(icon.clone())];
            let mut input = egui::RawInput {
                screen_rect: Some(screen(1280.0, 720.0)),
                ..Default::default()
            };
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .native_pixels_per_point = Some(ppp);
            let mut out = ctx.run_ui(input, |ui| {
                show(ui, &menu, &icons, &Chrome::default(), Instant::now());
            });
            out.textures_delta.clear();
            let prims = ctx.tessellate(out.shapes, ppp);
            for (tex, points) in [(&icon.tile, ICON_BOX), (&icon.chip, CHIP_ICON)] {
                assert_eq!(tex.size(), [icon_px(points, ppp); 2], "texture at ppp {ppp}");
                let mesh = prims
                    .iter()
                    .find_map(|p| match &p.primitive {
                        egui::epaint::Primitive::Mesh(m) if m.texture_id == tex.id() => Some(m),
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("{points} pt icon drawn at ppp {ppp}"));
                let (mut lo, mut hi) = (pos2(f32::MAX, f32::MAX), pos2(f32::MIN, f32::MIN));
                for v in &mesh.vertices {
                    lo = lo.min(v.pos);
                    hi = hi.max(v.pos);
                }
                let (lo, hi) = (lo.to_vec2() * ppp, hi.to_vec2() * ppp);
                let whole = |v: f32| (v - v.round()).abs() < 1e-3;
                assert!(whole(lo.x) && whole(lo.y), "{points} pt icon at {lo:?} px, ppp {ppp}");
                let size = hi - lo;
                assert!(
                    (size.x - tex.size()[0] as f32).abs() < 1e-3 && (size.y - tex.size()[1] as f32).abs() < 1e-3,
                    "{points} pt icon drawn {size:?} px from a {:?} texture, ppp {ppp}",
                    tex.size()
                );
            }
        }
    }

    #[test]
    fn a_chip_takes_the_icon_of_the_tile_with_its_program() {
        let mut menu = folded_menu(1);
        menu.tiles[0].argv = vec!["/usr/bin/Foot".into()];
        assert_eq!(menu.tile_for_app("foot"), Some(0));
        assert_eq!(menu.tile_for_app("other"), None);
        assert_eq!(menu.tile_for_app(""), None);
    }

    #[test]
    fn the_dialog_names_the_way_the_menu_ends() {
        let anyway = |a| format!("{} anyway", capitalized(ending_words(a).0));
        assert_eq!(anyway(Action::Logout), "Log out anyway");
        assert_eq!(anyway(Action::Reboot), "Restart anyway");
        assert_eq!(anyway(Action::Poweroff), "Power off anyway");
        assert_eq!(capitalized(ending_words(Action::Poweroff).1), "Powering off");
    }

    /// Writes PPM files of the Settings screen and a dialog through the real
    /// rasteriser, for looking at. Run by hand:
    /// `TF_SHOT_DIR=<dir> cargo test --lib shot_ -- --ignored`.
    fn shot(menu: &Menu, name: &str, px: (u32, u32), s: f32) {
        let Some(dir) = std::env::var_os("TF_SHOT_DIR") else {
            return;
        };
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        ctx.set_zoom_factor(s);
        let input = egui::RawInput {
            screen_rect: Some(screen(px.0 as f32 / s, px.1 as f32 / s)),
            ..Default::default()
        };
        let icons: Vec<Option<Icon>> = menu.tiles.iter().map(|_| None).collect();
        let mut textures = crate::raster::Textures::default();
        let mut canvas = crate::raster::Canvas::new(px.0 as usize, px.1 as usize, crate::brand::BG);
        for frame in 0..12 {
            let mut input = input.clone();
            input.time = Some(frame as f64 * 0.1);
            let mut out = ctx.run_ui(input, |ui| {
                let chrome = Chrome {
                    header: crate::header::Header {
                        host: "thinfloe-a1b2c3".into(),
                        version: None,
                    },
                    clock: "Thu 30 Oct 14:05".into(),
                    covered: false,
                    logo: None,
                };
                show(ui, menu, &icons, &chrome, Instant::now());
            });
            textures.apply_set(&out.textures_delta);
            out.textures_delta.clear();
            let prims = ctx.tessellate(out.shapes, s);
            canvas.restore_background(canvas.bounds());
            canvas.draw(&prims, &textures, s, canvas.bounds());
        }
        let mut ppm = format!("P6\n{} {}\n255\n", px.0, px.1).into_bytes();
        for px in &canvas.pixels {
            ppm.extend([(px >> 16) as u8, (px >> 8) as u8, *px as u8]);
        }
        std::fs::write(std::path::Path::new(&dir).join(name), ppm).unwrap();
    }

    #[test]
    #[ignore]
    fn shot_settings_and_dialog() {
        let (mut menu, _) = four_addons();
        let settings = menu.tiles.iter().position(|t| t.name == "Settings").unwrap();
        menu.activate(settings, Instant::now());
        shot(&menu, "settings.ppm", (1280, 720), 1.0);
        let mut menu = Menu::new(addon_grid(), false);
        menu.logout = Some(Logout::Failed {
            message: "The home directory could not be saved: boot server unreachable.".into(),
        });
        shot(&menu, "dialog.ppm", (1280, 720), 1.0);
    }

    /// The shipped tiles, with the add-ons as installed (their source keys
    /// dropped so the tiles are drawn).
    fn shipped_grid() -> Grid {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../overlay/etc/thinfloe/menu");
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "desktop"))
            .collect();
        names.sort();
        let files: Vec<_> = names
            .iter()
            .map(|p| {
                let text: String = std::fs::read_to_string(p)
                    .unwrap()
                    .lines()
                    .filter(|l| !l.starts_with("X-TF-Source"))
                    .map(|l| format!("{l}\n"))
                    .collect();
                crate::tiles::SourceFile {
                    name: p.file_name().unwrap().to_string_lossy().into(),
                    entry: crate::desktop::parse(&text),
                }
            })
            .collect();
        Grid {
            tiles: crate::tiles::merge(&files, &[]),
            banner: vec![],
        }
    }

    #[test]
    #[ignore]
    fn shot_desktop_and_settings_at_densities() {
        let menu = Menu::new(shipped_grid(), false);
        shot(&menu, "desktop-1080.ppm", (1920, 1080), 1.0);
        shot(&menu, "desktop-4k-fallback.ppm", (3840, 2160), 2.0);
        shot(&menu, "desktop-21in-4k.ppm", (3840, 2160), 2.25);
        let mut folded = Menu::new(shipped_grid(), false);
        folded.folded = vec![
            crate::windows::Folded {
                id: 1,
                title: "Terminal".into(),
                app_id: "foot".into(),
            },
            crate::windows::Folded {
                id: 2,
                title: "ThinLinc Client".into(),
                app_id: "tlclient".into(),
            },
        ];
        shot(&folded, "desktop-1080-folded.ppm", (1920, 1080), 1.0);
        let bat = status_menu(
            vec![
                kdev("MX Mechanical", battery::DeviceKind::Keyboard, 100, "discharging"),
                kdev("MX Anywhere 3S", battery::DeviceKind::Mouse, 95, "charging"),
                dev("Pen", 7, "discharging"),
            ],
            Instant::now(),
        );
        shot(&bat, "desktop-1080-battery.ppm", (1920, 1080), 1.0);
        shot(&bat, "desktop-4k-battery.ppm", (3840, 2160), 2.0);
        let mut net = status_menu(
            vec![
                kdev("MX Mechanical", battery::DeviceKind::Keyboard, 100, "discharging"),
                kdev("MX Anywhere 3S", battery::DeviceKind::Mouse, 95, "charging"),
                dev("Pen", 7, "discharging"),
            ],
            Instant::now(),
        );
        net.set_network(net_snap(300), Instant::now(), std::time::SystemTime::now());
        net.set_hardware(crate::status::hardware::sample());
        shot(&net, "desktop-1080-status.ppm", (1920, 1080), 1.0);
        shot(&net, "desktop-4k-status.ppm", (3840, 2160), 2.0);
        shot(&net, "desktop-1366x768-status.ppm", (1366, 768), 1.0);
        shot(&net, "desktop-1280x720-status.ppm", (1280, 720), 1.0);
        shot(&net, "desktop-1440x900-status.ppm", (1440, 900), 1.0);
        let mut no_battery = status_menu(Vec::new(), Instant::now());
        no_battery.set_network(net_snap(300), Instant::now(), std::time::SystemTime::now());
        no_battery.set_hardware(crate::status::hardware::sample());
        shot(&no_battery, "desktop-1080-no-battery.ppm", (1920, 1080), 1.0);
        let (mut menu, _) = four_addons();
        let settings = menu.tiles.iter().position(|t| t.name == "Settings").unwrap();
        menu.activate(settings, Instant::now());
        shot(&menu, "settings-1080.ppm", (1920, 1080), 1.0);
        shot(&menu, "settings-21in-4k.ppm", (3840, 2160), 2.25);
        shot_elements();
    }

    /// One element as its SVG, and as a picture at twice its size.
    fn shot_element(name: &str, svg: &str, room: crate::status::Room) {
        let Some(dir) = std::env::var_os("TF_SHOT_DIR") else {
            return;
        };
        let dir = std::path::Path::new(&dir);
        std::fs::write(dir.join(format!("{name}.svg")), svg).unwrap();
        let px = [room.size.x as usize * 2, room.size.y as usize * 2];
        let img = crate::status::render::render(svg, px).unwrap();
        let mut ppm = format!("P6\n{} {}\n255\n", px[0], px[1]).into_bytes();
        for p in &img.pixels {
            ppm.extend([p.r(), p.g(), p.b()]);
        }
        std::fs::write(dir.join(format!("{name}.ppm")), ppm).unwrap();
    }

    /// Every status element by itself, with sample data.
    fn shot_elements() {
        use crate::network::{Info, Link, Since, Vpn};
        use crate::status::{Cells, battery as b, hardware as h, network as g, network_info as l};
        let room = |cells: &str, cell_h: f32| Cells::parse(cells).unwrap().room(vec2(220.0, cell_h), TILE_GAP);

        let r = room("0,0,1,1", TILE_H);
        let lines = battery::lines(&[
            kdev("MX Mechanical", battery::DeviceKind::Keyboard, 100, "discharging"),
            kdev("MX Anywhere 3S", battery::DeviceKind::Mouse, 60, "charging"),
            dev("Pen", 7, "discharging"),
        ]);
        shot_element("battery", &b::svg(&lines, r), r);

        let sample = h::sample();
        for (name, show, cells) in [
            ("temperatures", h::Show::Temperatures, "0,1,1,1"),
            ("cpu", h::Show::Cpu, "0,2,1,1"),
            ("memory", h::Show::Memory, "1,2,1,1"),
            ("system", h::Show::System, "2,2,1,1"),
        ] {
            for (size, cell_h) in [("76", TILE_H), ("64", TILE_H_MIN)] {
                let r = room(cells, cell_h);
                let per = b::per_page(r.size.y, &r.style());
                let svg = h::svg(&h::lines_in(&sample, show, per, r), r);
                shot_element(&format!("hardware-{name}-{size}"), &svg, r);
            }
        }

        let crowded = crate::hardware::Snapshot {
            fans: vec![12000, 1800, 900, 600],
            maker: Some("Gigabyte Technology".into()),
            model: Some("GB-BXi3-5010 Some Long Board Name".into()),
            ..sample.clone()
        };
        for (size, cell_h) in [("76", TILE_H), ("64", TILE_H_MIN)] {
            let r = room("2,2,1,1", cell_h);
            let per = b::per_page(r.size.y, &r.style());
            let svg = h::svg(&h::lines_in(&crowded, h::Show::System, per, r), r);
            shot_element(&format!("hardware-system-crowded-{size}"), &svg, r);
        }

        let r = room("1,0,2,2", TILE_H);
        shot_element("network-graph", &g::svg(&net_snap(300), r), r);
        let r = room("1,0,3,1", TILE_H);
        shot_element("network-graph-short", &g::svg(&net_snap(300), r), r);

        let since = Since {
            job: Some(1),
            label: "since 14:02".into(),
            base: (1_000_000_000, 600_000),
        };
        let r = room("3,0,1,2", TILE_H);
        shot_element("network-info", &l::svg(&l::sample(), &since, r), r);
        let mut three = l::sample();
        three.info.dns = vec!["192.168.1.1".into(), "9.9.9.9".into(), "1.1.1.1".into()];
        shot_element("network-info-three-dns", &l::svg(&three, &since, r), r);
        let mut warn = l::sample();
        warn.info = Info {
            iface: Some("wlp0s20f3".into()),
            default_route: false,
            link: Link::Wifi(Some(-61)),
            addressed: true,
            vpn: Vpn::Down,
            ..Info::default()
        };
        shot_element("network-info-warning", &l::svg(&warn, &since, r), r);
        let r = room("3,0,1,2", TILE_H_MIN);
        shot_element("network-info-small-cell", &l::svg(&l::sample(), &since, r), r);
    }
}
