//! Drawing the grid, the Settings screen in its place, the log under them,
//! and the dialogs. Everything that decides something lives in menu.rs; this
//! file only paints it and reports clicks.

use crate::brand::{self, role};
use crate::density::GROUP_W;
use crate::header::{Header, header_text, until_next_minute};
use crate::icons::{self, IconJob, IconRoots};
use crate::log::{Ansi, Kind, Line, Log, Run};
use crate::menu::{Choice, Logout, Menu, Screen};
use crate::screentab;
use crate::settings::{self, RowButton, SettingsEvent, SettingsTab};
use crate::tiles::{Action, Origin, Tile};
use crate::upgrade::UpgradeEvent;
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align2, Color32, ColorImage, FontId, Rect, Sense, Stroke, StrokeKind, TextFormat, TextureHandle, Ui, UiBuilder,
    Vec2, pos2, vec2,
};
use shadcn_egui::Theme;
use shadcn_egui::components::button::{Button, ButtonVariant};
use shadcn_egui::components::select::Select;
use shadcn_egui::components::tabs::Tabs;
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
/// The log's rows while the Upgrade screen offers its choices: the page
/// needs the room, and nothing is written yet.
pub const LOG_CHOOSE_ROWS: usize = 4;
/// One log row, in points; the log draws one line per row.
pub const LOG_ROW: f32 = 18.0;
const LOG_PAD: f32 = 10.0;
const LOG_FONT: f32 = 13.0;

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
            // token-ok: raw pixels of a scaled icon
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
    let fg = crate::brand::theme().palette().foreground;
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
    Upgrade(UpgradeEvent),
    Eject(crate::menu::EjectEvent),
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

/// The log is small while the Upgrade screen offers its choices; it is
/// back to its size once a write starts, and on every other screen.
pub fn small_log(menu: &Menu) -> bool {
    menu.screen == Screen::Upgrade && menu.upgrade.stage == crate::upgrade::Stage::Choose
}

pub fn layout(screen: Rect, tiles: usize, folded: usize, status_rows: usize, planned_rows: usize) -> Layout {
    layout_with(screen, tiles, folded, status_rows, planned_rows, false)
}

/// The layout, with a log of `LOG_CHOOSE_ROWS` rows when `small_log`.
///
/// `status_rows` is the rows the status area uses now (0 to 3; 0 draws none);
/// `planned_rows` the rows it has when every element shows
/// (`status::Area::planned_rows`). The tiles and the top of the area are placed
/// by the planned rows, so they stay where they are when an element appears or
/// goes (the elements inside the area close up); with none planned the tiles
/// are centred alone.
pub fn layout_with(
    screen: Rect,
    tiles: usize,
    folded: usize,
    status_rows: usize,
    planned_rows: usize,
    small_log: bool,
) -> Layout {
    let small_h = LOG_CHOOSE_ROWS as f32 * LOG_ROW + 2.0 * LOG_PAD;
    let log_h = if small_log {
        small_h
    } else {
        (screen.height() * LOG_SHARE).max(LOG_MIN_ROWS as f32 * LOG_ROW + 2.0 * LOG_PAD)
    };
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
    let log_min = if small_log {
        small_h
    } else {
        LOG_MIN_ROWS as f32 * LOG_ROW + 2.0 * LOG_PAD
    };
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
    /// The frame's wall time, read once by the caller; `None`: now. The
    /// header's minute wake counts from it.
    pub wall: Option<std::time::SystemTime>,
}

/// Logo, name, host, version and clock along the top of `rect`.
fn show_header(ui: &mut Ui, chrome: &Chrome, rect: Rect) {
    let t = Theme::current(ui.ctx());
    let p = ui.painter();
    let fg = t.palette().foreground;
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
            // token-ok: white tints nothing: the logo texture is drawn as it is
            dim(Color32::WHITE),
        );
        x = logo.right() + 14.0;
    }
    let mid = top + LOGO_H / 2.0;
    // Name and byline form one block, centred on the logo.
    let name = p.layout_no_wrap("ThinFloe".into(), brand::geo(brand::NAME_PT), dim(fg));
    let by = p.layout_no_wrap(
        BYLINE.into(),
        brand::geo(brand::BYLINE_PT),
        t.palette().muted_foreground,
    );
    let y = mid - (name.size().y + by.size().y) / 2.0;
    let by_y = y + name.size().y;
    p.galley(pos2(x, y), name, fg);
    p.galley(pos2(x, by_y), by, t.palette().muted_foreground);
    p.text(
        pos2(rect.right() - HEADER_PAD_X, mid),
        Align2::RIGHT_CENTER,
        header_text(&chrome.header, &chrome.clock),
        brand::geo(brand::HEADER_PT),
        t.palette().muted_foreground,
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

/// What the dialogs wait for: the add-on jobs, the stick write or both.
fn root_waits(addons: bool, write: bool) -> &'static str {
    match (addons, write) {
        (true, true) => "the add-ons and the stick",
        (false, true) => "the stick",
        _ => "the add-ons",
    }
}

/// The root jobs that run: add-on jobs, and the stick write.
fn root_kinds(menu: &Menu) -> (bool, bool) {
    (!menu.addon_jobs().is_empty(), menu.upgrade.writing())
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
    ui.ctx().request_repaint_after(until_next_minute(
        chrome.wall.unwrap_or_else(std::time::SystemTime::now),
    ));
    let (verb, verbing) = ending_words(menu.ending);
    // No background fill here: the canvas restores the page colour itself,
    // and while the dialog is up it holds the frozen grid, which a fill
    // would erase. Only the dialog is drawn then.
    match &menu.logout {
        Some(Logout::Confirm) => {
            let mut chose = None;
            let root_jobs = menu.root_jobs();
            let kinds = root_kinds(menu);
            let title = match kinds {
                _ if !menu.jobs.is_empty() => "Programs are still running",
                (false, false) if menu.eject.running.is_some() => "A stick is being released",
                (false, true) => "A stick is being written",
                (true, true) => "Add-ons are being changed and a stick is written",
                _ => "Add-ons are being changed",
            };
            dialog(ui, title, |ui, t| {
                for job in &menu.jobs {
                    ui.label(egui::RichText::new(&job.name).color(t.palette().foreground));
                }
                for job in &root_jobs {
                    ui.label(egui::RichText::new(job).color(t.palette().foreground));
                }
                if !menu.jobs.is_empty() {
                    ui.label(
                        egui::RichText::new("They are stopped before the home directory is saved.")
                            .color(t.palette().muted_foreground),
                    );
                }
                if !root_jobs.is_empty() {
                    let what = match kinds {
                        (false, false) => "The stick is released",
                        (false, true) => "The stick is written",
                        (true, true) => "The add-on work and the write are finished",
                        _ => "The add-on work is finished",
                    };
                    ui.label(
                        egui::RichText::new(format!("{what} before you {verb}.")).color(t.palette().muted_foreground),
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
            let (addons, write) = root_kinds(menu);
            let what = if !addons && !write && menu.eject.running.is_some() {
                "the stick to be released"
            } else {
                root_waits(addons, write)
            };
            dialog(ui, &format!("Waiting for {what} before {verbing}"), |ui, t| {
                for job in menu.root_jobs() {
                    ui.label(egui::RichText::new(job).color(t.palette().muted_foreground));
                }
                // Stopping a fresh install after its first byte leaves a
                // stick that does not start; before it, the stick is as it was.
                if menu.ending != Action::Logout
                    && menu.upgrade.fresh_touched()
                    && let Some(drive) = menu.upgrade.job_drive()
                {
                    ui.label(
                        egui::RichText::new(format!(
                            "If you {verb} anyway, {drive} will not start until it is written again."
                        ))
                        .color(t.role(role::WARN)),
                    );
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
            let title = match root_kinds(menu) {
                (false, true) => "Stopping the write",
                (true, true) => "Stopping the add-on work and the write",
                _ => "Stopping the add-on work",
            };
            dialog(ui, title, |ui, t| {
                for job in menu.root_jobs() {
                    ui.label(egui::RichText::new(job).color(t.palette().muted_foreground));
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
                    ui.label(egui::RichText::new(&job.name).color(t.palette().muted_foreground));
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
                    egui::RichText::new("Waiting for the backup that is running.").color(t.palette().muted_foreground),
                );
            });
            return None;
        }
        Some(Logout::Saving { since }) => {
            dialog(ui, &format!("Saving your home directory before {verbing}"), |ui, t| {
                ui.label(
                    egui::RichText::new(elapsed(now.duration_since(*since)))
                        .size(brand::TIMER_PT)
                        .monospace()
                        .color(t.palette().ring),
                );
                ui.label(
                    egui::RichText::new("On a network-booted machine this can take a minute.")
                        .color(t.palette().muted_foreground),
                );
            });
            ui.ctx().request_repaint_after(Duration::from_secs(1));
            return None;
        }
        Some(Logout::Ejecting { since }) => {
            dialog(ui, &format!("Writing to {} …", menu.flushing_names()), |ui, t| {
                ui.label(
                    egui::RichText::new(elapsed(now.duration_since(*since)))
                        .size(brand::TIMER_PT)
                        .monospace()
                        .color(t.palette().ring),
                );
                ui.label(
                    egui::RichText::new(format!("The sticks are released before {verbing}."))
                        .color(t.palette().muted_foreground),
                );
            });
            ui.ctx().request_repaint_after(Duration::from_secs(1));
            return None;
        }
        Some(Logout::EjectFailed { message }) => {
            let mut chose = None;
            dialog(ui, "A stick was not released", |ui, t| {
                ui.label(egui::RichText::new(message).color(t.role(role::WARN)));
                ui.label(
                    egui::RichText::new(format!("If you {verb} now, data not yet written to it can be lost."))
                        .color(t.palette().muted_foreground),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(Button::new("Stay")).clicked() {
                        chose = Some(Choice::EjectWait);
                    }
                    if ui
                        .add(Button::new(format!("{} anyway", capitalized(verb))).variant(ButtonVariant::Destructive))
                        .clicked()
                    {
                        chose = Some(Choice::EjectGoOn);
                    }
                });
            });
            return chose.map(UiEvent::Chose);
        }
        Some(Logout::Leaving { until }) => {
            dialog(ui, "Home directory saved", |ui, t| {
                ui.label(egui::RichText::new(format!("{}.", capitalized(verbing))).color(t.palette().muted_foreground));
            });
            ui.ctx().request_repaint_after(until.saturating_duration_since(now));
            return None;
        }
        Some(Logout::Failed { message }) => {
            let mut chose = None;
            dialog(ui, "Your home directory was not saved", |ui, t| {
                ui.label(egui::RichText::new(message).color(t.role(role::WARN)));
                ui.label(
                    egui::RichText::new(format!("If you {verb} now, the changes since the last save are lost."))
                        .color(t.palette().muted_foreground),
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
    // The 15-second question comes before the retry question: it goes
    // back on its own when nobody answers.
    if crate::monitors_ui::dialog_open(&menu.monitors) {
        return crate::monitors_ui::keep_dialog(ui, &menu.monitors, now)
            .map(|e| UiEvent::Settings(SettingsEvent::Monitors(e)));
    }
    if let Some(r) = &menu.retry {
        let mut chose = None;
        let text = match &r.message {
            Some(m) => format!("{} could not be installed: {m}", r.name),
            None => format!("{} could not be installed.", r.name),
        };
        dialog(ui, "Add-on not installed", |ui, t| {
            ui.label(egui::RichText::new(text).color(t.role(role::WARN)));
            ui.label(egui::RichText::new("Settings shows it as well.").color(t.palette().muted_foreground));
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
    if menu.eject.panel_open {
        return crate::eject_ui::show(ui, menu).map(UiEvent::Eject);
    }
    show_screen(ui, menu, icons, chrome, now)
}

/// The screen without a dialog: the grid, Settings or Upgrade, with the
/// header, the log and the strip. A dialog freezes this, dimmed, behind it.
pub fn show_screen(ui: &mut Ui, menu: &Menu, icons: &[Option<Icon>], chrome: &Chrome, now: Instant) -> Option<UiEvent> {
    let shown = menu.visible_tiles();
    let sources = menu.status_sources();
    let short = short_screen(ui.max_rect().height());
    let l = layout_with(
        ui.max_rect(),
        shown.len(),
        menu.folded.len(),
        menu.status.rows(&sources, short),
        menu.status.planned_rows(short),
        small_log(menu),
    );
    show_header(ui, chrome, l.header);
    let unfold = l
        .strip
        .and_then(|r| show_strip(ui, menu, icons, r))
        .map(UiEvent::Unfold);
    if menu.screen == Screen::Settings {
        let preview = crate::statustab_ui::PreviewPlace {
            cell: l.tile,
            short,
            covered: chrome.covered,
        };
        let event = show_settings(ui, menu, icons, l.area, preview, now);
        show_log(ui, &menu.log, l.log);
        return event.map(UiEvent::Settings).or(unfold);
    }
    if menu.screen == Screen::Upgrade {
        let event = crate::upgrade_ui::show(ui, menu, l.area);
        show_log(ui, &menu.log, l.log);
        return event.map(UiEvent::Upgrade).or(unfold);
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
                            &menu.tile_name(index),
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
pub(crate) struct StatusPlace {
    pub(crate) origin: egui::Pos2,
    pub(crate) cell: Vec2,
    pub(crate) short: bool,
}

/// Draw each shown status element in `place`, each in a child `Ui` clipped
/// to its own cells, and ask for a frame when one changes on its own, unless
/// the menu is covered.
pub(crate) fn paint_status(
    ui: &mut Ui,
    area: &crate::status::Area,
    data: &crate::status::Data,
    place: StatusPlace,
    now: Instant,
    covered: bool,
) {
    let StatusPlace { origin, cell, short } = place;
    for (element, size, cells) in area.shown(data, short) {
        let rect = cells.rect(origin, cell, TILE_GAP);
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        element.draw(&mut child, rect, size, data, now);
    }
    if !covered && let Some(d) = area.wake(data, origin, cell, TILE_GAP, now, short) {
        ui.ctx().request_repaint_after(d);
    }
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
                        t.palette().foreground,
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
                    let fill = if resp.hovered() {
                        t.palette().accent
                    } else {
                        t.palette().card
                    };
                    p.rect_filled(rect, 0.0, fill);
                    p.rect_stroke(rect, 0.0, Stroke::new(1.0, t.palette().border), StrokeKind::Inside);
                    if resp.has_focus() {
                        p.rect_stroke(
                            rect.expand(2.0),
                            0.0,
                            Stroke::new(3.0, t.palette().ring),
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
                        // token-ok: white tints nothing: the icon is drawn as rendered
                        Some(icon) => draw_icon(p, &icon.chip, icon_rect.min, Color32::WHITE),
                        None => {
                            p.rect_filled(icon_rect, t.radius_sm(), t.palette().muted);
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
                                t.palette().foreground,
                            );
                        }
                    }
                    let y = rect.center().y - galley.size().y / 2.0;
                    ui.painter_at(rect)
                        .galley(pos2(icon_rect.right() + 10.0, y), galley, t.palette().foreground);
                    if resp.clicked() {
                        clicked = Some(f.id);
                    }
                }
            });
        });
    clicked
}

/// The left and right padding of a Settings row; the button ends `ROW_RIGHT` from its edge.
pub(crate) const ROW_LEFT: i8 = 22;
pub(crate) const ROW_RIGHT: i8 = 16;
pub(crate) const ROW_MARGIN_X: f32 = (ROW_LEFT + ROW_RIGHT) as f32;
/// The side of a row's icon on the Settings screen.
const ROW_ICON: f32 = 40.0;

/// The Settings screen in `area`: the title, the tab bar, and the open tab.
fn show_settings(
    ui: &mut Ui,
    menu: &Menu,
    icons: &[Option<Icon>],
    area: Rect,
    preview: crate::statustab_ui::PreviewPlace,
    now: Instant,
) -> Option<SettingsEvent> {
    let t = Theme::current(ui.ctx());
    let mut event = None;
    // The panel is as wide as the tile group of the desktop, centred.
    let inner = area.shrink(GAP);
    let width = GROUP_W.min(inner.width());
    let panel = Rect::from_center_size(inner.center(), vec2(width, inner.height()));
    let mut s_ui = ui.new_child(UiBuilder::new().max_rect(panel));
    // A Network or Screen dialog is modal: the screen under it takes no clicks.
    if menu.settings_tab == SettingsTab::Network && crate::network_ui::dialog_open(&menu.network)
        || menu.settings_tab == SettingsTab::Screen && screen_dialog_open(&menu.monitors, &menu.deskflow)
    {
        s_ui.disable();
    }
    s_ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Settings")
                .font(brand::geo(brand::SETTINGS_TITLE_PT))
                .color(t.palette().foreground),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(Button::new("Back").variant(ButtonVariant::Secondary)).clicked() {
                event = Some(SettingsEvent::Back);
            }
        });
    });
    s_ui.add_space(GAP);
    if let Some(e) = config_notes_band(&mut s_ui, menu, width) {
        event = Some(e);
    }
    let labels = SettingsTab::ALL.map(SettingsTab::label);
    let mut idx = SettingsTab::ALL
        .iter()
        .position(|&tab| tab == menu.settings_tab)
        .unwrap_or(0);
    if s_ui.add(Tabs::new(&mut idx, &labels).width(f32::INFINITY)).changed() {
        event = Some(SettingsEvent::Tab(SettingsTab::ALL[idx]));
    }
    s_ui.add_space(GAP);
    // A failed migration leaves the old settings in place: the tabs that
    // would change them show them and take no click.
    if menu.config_notes.legacy.is_some() && !matches!(menu.settings_tab, SettingsTab::Packages | SettingsTab::Machine)
    {
        s_ui.disable();
    }
    let body = match menu.settings_tab {
        SettingsTab::Packages => packages_tab(&mut s_ui, menu, icons, width),
        SettingsTab::Screen => screen_tab(&mut s_ui, menu, width, now),
        SettingsTab::Network => {
            crate::network_ui::network_tab(&mut s_ui, &menu.network, width).map(SettingsEvent::Network)
        }
        SettingsTab::Keyboard => {
            crate::keyboardtab_ui::keyboard_tab(&mut s_ui, &menu.keyboard, width).map(SettingsEvent::Keyboard)
        }
        SettingsTab::Sound => crate::soundtab_ui::sound_tab(
            &mut s_ui,
            &menu.sound,
            menu.keyboard.shown.volume_keys,
            menu.keyboard.volume_note.as_deref(),
            width,
            now,
        ),
        SettingsTab::Status => {
            crate::statustab_ui::status_tab(&mut s_ui, menu, preview, now).map(SettingsEvent::Status)
        }
        SettingsTab::Machine => crate::machine_ui::machine_tab(&mut s_ui, menu, width, now),
    };
    match (event, body) {
        // The tab click wins the frame, but the lookup address field's
        // last text must reach the menu, or leaving the tab commits an
        // older one.
        (
            Some(SettingsEvent::Tab(tab)),
            Some(SettingsEvent::Network(
                e @ (crate::network::NetEvent::PublicIpUrl { .. } | crate::network::NetEvent::PublicIpUrlCancel),
            )),
        ) => Some(SettingsEvent::TabAfterUrl(tab, e)),
        (event, body) => event.or(body),
    }
}

/// Padding above and below the notes band.
const NOTES_PAD: i8 = 12;

/// What the config directory has to say, as one muted band under the title:
/// the store's problems, the `environment` lines the menu overrides, the
/// migration report (with Dismiss) and the legacy note. Nothing, no band.
fn config_notes_band(ui: &mut Ui, menu: &Menu, width: f32) -> Option<SettingsEvent> {
    let notes = &menu.config_notes;
    if notes.broken.is_empty() && notes.dropped_env.is_empty() && notes.report.is_none() && notes.legacy.is_none() {
        return None;
    }
    let t = Theme::current(ui.ctx());
    let mut event = None;
    egui::Frame::new()
        .fill(t.palette().card)
        .inner_margin(egui::Margin {
            left: ROW_LEFT,
            right: ROW_RIGHT,
            top: NOTES_PAD,
            bottom: NOTES_PAD,
        })
        .show(ui, |ui| {
            // The frame's margins are part of the panel's width.
            ui.set_width(width - ROW_MARGIN_X);
            let muted = |text: &str| egui::RichText::new(text).color(t.palette().muted_foreground);
            for line in &notes.broken {
                ui.label(muted(line));
            }
            if !notes.dropped_env.is_empty() {
                ui.label(muted(
                    "These lines in ~/.config/thinfloe/environment are ignored because the menu sets them:",
                ));
                for line in &notes.dropped_env {
                    ui.label(muted(line));
                }
            }
            if let Some(report) = &notes.report {
                ui.horizontal(|ui| {
                    ui.label(muted(report));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(Button::new("Dismiss").variant(ButtonVariant::Secondary))
                            .clicked()
                        {
                            event = Some(SettingsEvent::DismissReport);
                        }
                    });
                });
            }
            if let Some(legacy) = &notes.legacy {
                ui.label(egui::RichText::new(legacy).color(t.role(role::WARN)));
            }
        });
    ui.add_space(GAP);
    event
}

/// The Packages tab: when the add-ons were last checked, and one row each.
fn packages_tab(ui: &mut Ui, menu: &Menu, icons: &[Option<Icon>], width: f32) -> Option<SettingsEvent> {
    let t = Theme::current(ui.ctx());
    let mut event = None;
    ui.horizontal(|ui| {
        // SAFETY: time(NULL) has no preconditions.
        let now = unsafe { libc::time(std::ptr::null_mut()) } as i64;
        ui.label(egui::RichText::new(settings::last_checked(menu.last_check, now)).color(t.palette().muted_foreground));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
    ui.add_space(GAP);
    egui::ScrollArea::vertical()
        .id_salt("settings")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let rows = settings::rows(menu);
            if rows.is_empty() {
                ui.label(egui::RichText::new("No add-ons are defined.").color(t.palette().muted_foreground));
            }
            ui.spacing_mut().item_spacing.y = TILE_GAP;
            let row_inner = ui.available_width().min(width) - ROW_MARGIN_X;
            for row in rows {
                // A TILE band; the ice edge on the left marks a waiting update.
                let framed = egui::Frame::new()
                    .fill(t.palette().card)
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
                    ui.painter().rect_filled(
                        Rect::from_min_max(r.min, pos2(r.min.x + 5.0, r.max.y)),
                        0.0,
                        t.palette().primary,
                    );
                }
                if let Some(e) = framed.inner {
                    event = Some(e);
                }
            }
        });
    event
}

/// A dialog of the Screen tab is open: a monitor's, or one of the
/// Deskflow band. Not *Keep this arrangement?*, which replaces the screen.
pub fn screen_dialog_open(mon: &crate::monitors::MonTab, df: &crate::deskflowtab::DeskflowTab) -> bool {
    mon.dialog_monitor().is_some() || crate::deskflowtab::dialog_open(df)
}

/// The Screen tab: when the screen turns off, the monitors, Deskflow, then
/// the graphics, and over them the open monitor or Deskflow dialog.
/// Apply follows the bands, as on the Network tab; pinned to the foot of
/// the panel only when the bands scroll.
fn screen_tab(ui: &mut Ui, menu: &Menu, width: f32, now: Instant) -> Option<SettingsEvent> {
    let rect = Rect::from_min_max(ui.cursor().min, ui.max_rect().max);
    let (apply, edit) = page_with_footer(
        ui,
        "screen",
        rect,
        GAP,
        |ui| crate::monitors_ui::monitors_footer(ui, &menu.monitors),
        |ui| {
            // The bands stand TILE_GAP apart, as on the Network tab; inside
            // them the spacing stays as it was.
            let between = ui.spacing().item_spacing.y;
            ui.spacing_mut().item_spacing.y = TILE_GAP;
            let band_w = ui.available_width().min(width);
            let idle = ui
                .scope(|ui| {
                    ui.spacing_mut().item_spacing.y = between;
                    idle_band(ui, menu)
                })
                .inner;
            let devices = crate::deskflow_ui::places(&menu.deskflow, &menu.monitors);
            let monitors = ui
                .scope(|ui| {
                    ui.spacing_mut().item_spacing.y = between;
                    crate::monitors_ui::monitors_band(
                        ui,
                        &menu.monitors,
                        &devices,
                        menu.deskflow.selected.as_deref(),
                        now,
                        band_w,
                    )
                })
                .inner
                .map(|e| match e {
                    crate::monitors_ui::BandEvent::Mon(e) => SettingsEvent::Monitors(e),
                    crate::monitors_ui::BandEvent::Device(name) => {
                        SettingsEvent::Deskflow(crate::deskflowtab::DeskflowEvent::Select(name))
                    }
                });
            let ipv4 = menu.netstat.as_ref().and_then(|s| s.info.ipv4).map(|(ip, _)| ip);
            let deskflow = ui
                .scope(|ui| {
                    ui.spacing_mut().item_spacing.y = between;
                    let t = Theme::current(ui.ctx());
                    crate::deskflow_ui::band(ui, &t, &menu.deskflow, &menu.monitors, ipv4, band_w)
                })
                .inner
                .map(SettingsEvent::Deskflow);
            // The facts close the tab, as on the Network tab: on top they
            // pushed the monitors' Position row out of a 1080 screen.
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = between;
                graphics_band(ui, menu.graphics.as_ref());
            });
            // The footer's gap belongs to the page.
            ui.add_space(GAP);
            idle.or(monitors).or(deskflow)
        },
    );
    // Drawn last, so it is on top; at most one is open.
    let monitor = crate::monitors_ui::monitor_dialog(ui, &menu.monitors).map(SettingsEvent::Monitors);
    let deskflow = crate::deskflow_ui::dialog(ui, &menu.deskflow, &menu.monitors).map(SettingsEvent::Deskflow);
    monitor.or(deskflow).or(edit).or(apply.map(SettingsEvent::Monitors))
}

/// When the screen turns off, in one band.
fn idle_band(ui: &mut Ui, menu: &Menu) -> Option<SettingsEvent> {
    let t = Theme::current(ui.ctx());
    let tab = &menu.screen_tab;
    let mut event = None;
    let values = screentab::options(tab.idle);
    let texts: Vec<String> = values.iter().map(|&v| screentab::label(v)).collect();
    let labels: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut idx = values.iter().position(|&v| v == tab.idle).unwrap_or(0);
    let inner_w = ui.available_width() - ROW_MARGIN_X;
    egui::Frame::new()
        .fill(t.palette().card)
        .inner_margin(egui::Margin {
            left: ROW_LEFT,
            right: ROW_RIGHT,
            top: 12,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.set_width(inner_w);
            ui.horizontal(|ui| {
                ui.scope(|ui| {
                    ui.set_min_width(brand::FORM_LABEL_W);
                    ui.set_max_width(brand::FORM_LABEL_W);
                    ui.label(egui::RichText::new("Turn the screen off after").color(t.palette().foreground));
                });
                if ui.add(Select::new("tf-screen-idle", &mut idx, &labels)).changed() {
                    event = values.get(idx).map(|&v| SettingsEvent::IdleTimeout(v));
                }
            });
            // The note stands under the select, in the label column's shadow.
            let indent = brand::FORM_LABEL_W + ui.spacing().item_spacing.x;
            let note = |ui: &mut Ui, text: egui::RichText| {
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    ui.add(egui::Label::new(text).truncate());
                });
            };
            if let Some(error) = &tab.error {
                note(ui, egui::RichText::new(error).color(t.role(role::WARN)));
            }
        });
    event
}

/// The GPUs with their drivers, and the video codecs VA-API offers;
/// `info` is `None` until the graphics job answers.
fn graphics_band(ui: &mut Ui, info: Option<&crate::graphics::Info>) {
    let t = Theme::current(ui.ctx());
    let w = ui.available_width();
    crate::infopanel::band(ui, &t, w, "Graphics", &crate::graphics::info_lines(info));
}

/// One add-on: tick box, icon, name, source, state, and its button.
fn settings_row(ui: &mut Ui, row: &settings::Row, icon: Option<&Icon>, t: &Theme) -> Option<SettingsEvent> {
    let mut event = None;
    ui.horizontal(|ui| {
        let mut ticked = row.ticked;
        let tick = ui.add_enabled_ui(!row.busy && !row.unusable, |ui| {
            shadcn_egui::components::checkbox::checkbox(ui, &mut ticked)
        });
        if tick.inner.changed() {
            event = Some(SettingsEvent::Tick {
                id: row.id.clone(),
                on: ticked,
            });
        }
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(ROW_ICON), Sense::hover());
        match icon {
            // token-ok: white tints nothing: the icon is drawn as rendered
            Some(icon) => draw_icon(ui.painter(), &icon.row, rect.min, Color32::WHITE),
            None => {
                ui.painter().rect_filled(rect, 0.0, t.palette().accent);
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
                    t.palette().foreground,
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
                                                .font(brand::geo(brand::ROW_NAME_PT))
                                                .color(t.palette().foreground),
                                        )
                                        .truncate(),
                                    );
                                });
                                if let Some(source) = &row.source {
                                    let job = LayoutJob::simple_singleline(
                                        source.clone(),
                                        FontId::monospace(brand::ROW_SOURCE_PT),
                                        t.palette().muted_foreground,
                                    );
                                    // A horizontal layout lets a label extend; cut it instead.
                                    ui.add(egui::Label::new(job).truncate());
                                }
                            },
                        );
                        // One line: the items joined by dots, each in its tone.
                        let font = FontId::proportional(brand::ROW_STATUS_PT);
                        let mut job = LayoutJob::default();
                        for (i, (text, tone)) in settings::status_items(row).into_iter().enumerate() {
                            if i > 0 {
                                job.append(
                                    " · ",
                                    0.0,
                                    egui::TextFormat::simple(font.clone(), t.palette().muted_foreground),
                                );
                            }
                            let color = match tone {
                                settings::Tone::Plain => t.palette().foreground,
                                settings::Tone::Good => t.role(role::GOOD),
                                settings::Tone::Ice => t.palette().primary,
                                settings::Tone::Warn => t.role(role::WARN),
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
        .fill(t.palette().card)
        .stroke(Stroke::new(1.0, t.palette().border))
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
                            .galley(pos2(row.left(), y), galley, t.palette().foreground);
                    }
                });
        });
}

pub fn ansi_color(a: Ansi) -> Color32 {
    match a {
        Ansi::Basic(n) => brand::ANSI_16[(n & 15) as usize],
        Ansi::Indexed(n @ 0..=15) => brand::ANSI_16[n as usize],
        Ansi::Indexed(n @ 16..=231) => {
            let level = [0u8, 95, 135, 175, 215, 255];
            let i = n - 16;
            // token-ok: xterm's 256-colour cube, a program's own colour
            Color32::from_rgb(
                level[(i / 36) as usize],
                level[(i / 6 % 6) as usize],
                level[(i % 6) as usize],
            )
        }
        Ansi::Indexed(n) => {
            let g = 8 + 10 * (n - 232);
            // token-ok: xterm's grey ramp, a program's own colour
            Color32::from_rgb(g, g, g)
        }
        // token-ok: a program's own RGB colour
        Ansi::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    }
}

/// A run's colour: its own, brightened when bold the way terminals do, or
/// the line's colour by kind; bold without a colour is the strong text.
fn run_color(run: &Run, line_color: Color32, t: &Theme) -> Color32 {
    match (run.fg, run.bold) {
        (Some(Ansi::Basic(n)), true) if n < 8 => ansi_color(Ansi::Basic(n + 8)),
        (Some(a), _) => ansi_color(a),
        (None, true) => t.palette().foreground,
        (None, false) => line_color,
    }
}

/// One log line: the time, then the program it came from, then the text.
fn log_job(line: &Line, t: &Theme) -> LayoutJob {
    let font = FontId::monospace(LOG_FONT);
    let text_color = match line.kind {
        Kind::Event => t.palette().foreground,
        Kind::Output => t.palette().muted_foreground,
        Kind::Warn => t.role(role::WARN),
        Kind::Good => t.role(role::GOOD),
    };
    let mut job = LayoutJob::default();
    job.append(&line.time, 0.0, TextFormat::simple(font.clone(), t.role(role::TIME)));
    let mut lead = 16.0;
    if let Some(source) = &line.source {
        job.append(source, 16.0, TextFormat::simple(font.clone(), t.palette().primary));
        job.append(" | ", 0.0, TextFormat::simple(font.clone(), t.role(role::TIME)));
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

fn paint_tile(ui: &Ui, rect: Rect, tile: &Tile, name: &str, icon: Option<&Icon>, state: TileState) {
    let t = Theme::current(ui.ctx());
    let p = ui.painter();
    let usable = tile.usable();
    // A tile that cannot be used fades; its reason stays at full strength.
    let fade = |c: Color32| if usable { c } else { c.gamma_multiply(0.35) };
    let fill = if state.hovered {
        t.palette().accent
    } else if state.active {
        t.role(role::TILE_ACTIVE)
    } else {
        t.palette().card
    };
    p.rect_filled(rect, 0.0, fade(fill));
    if state.active {
        p.rect_filled(
            Rect::from_min_size(rect.min, vec2(EDGE_W, rect.height())),
            0.0,
            t.palette().primary,
        );
    }
    if state.focused {
        p.rect_stroke(
            rect.expand(2.0),
            0.0,
            Stroke::new(3.0, t.palette().ring),
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
        // token-ok: white tints nothing: the icon is drawn as rendered, only faded
        Some(icon) => draw_icon(p, &icon.tile, icon_rect.min, fade(Color32::WHITE)),
        None => {
            p.rect_filled(icon_rect, t.radius_sm(), fade(t.palette().muted));
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
                fade(t.palette().foreground),
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
    line(name, brand::geo(brand::TILE_NAME_PT), fade(t.palette().foreground));
    // One second line: why the tile is grey, else its comment, else the
    // note about its file. `--check` has the long form.
    if let Some(why) = &tile.problem {
        line(why, FontId::proportional(brand::TILE_NOTE_PT), t.role(role::WARN));
    } else if let Some(c) = tile.comment.clone().or(tile.note.clone()).or_else(|| {
        // Only a reworded file that lost keys; plain "reworded" is no news.
        match &tile.origin {
            Origin::Reworded { ignored } if !ignored.is_empty() => tile.origin.reworded_text(),
            _ => None,
        }
    }) {
        line(
            &c,
            FontId::proportional(brand::TILE_NOTE_PT),
            fade(t.palette().muted_foreground),
        );
    }
    let heights: Vec<f32> = lines.iter().map(|g| g.size().y).collect();
    let block_h = heights.iter().sum::<f32>() + 2.0 * (lines.len() as f32 - 1.0);
    let mut y = snap(rect.center().y - block_h / 2.0);
    for (galley, h) in lines.into_iter().zip(heights) {
        // token-ok: fallback colour only: every section of these galleys has its own
        clipped.galley(pos2(text_x, y), galley, Color32::WHITE);
        y += h + 2.0;
    }
    if state.running > 1 {
        p.text(
            rect.right_top() + vec2(-9.0, 7.0),
            Align2::RIGHT_TOP,
            format!("×{}", state.running),
            brand::geo(brand::RUNNING_PT),
            t.palette().primary,
        );
    }
    // Small marks along the bottom right.
    let mut right = rect.right() - 9.0;
    let mut mark = |text: &str, color: Color32| {
        let r = p.text(
            pos2(right, rect.bottom() - 6.0),
            Align2::RIGHT_BOTTOM,
            text,
            FontId::proportional(brand::MARK_PT),
            color,
        );
        right = r.left() - 8.0;
    };
    if state.update {
        mark("update", t.palette().primary);
    }
    if tile.origin == Origin::User {
        mark("user", t.palette().muted_foreground);
    }
}

/// A page's scroll bar is drawn whole, also while the pointer is away.
const SCROLL_REST_OPACITY: f32 = 1.0;

/// A scrolling page above a footer in `rect`. The footer follows the page,
/// which ends with `gap` of its own. When the page does not fit, the
/// footer is pinned to the bottom of `rect` and the page scrolls above it,
/// `gap` apart, so the main buttons never scroll away. Both heights are
/// the previous pass's; a change discards the pass, so the frame is drawn
/// again at once with the right ones. `salt` names the page.
pub(crate) fn page_with_footer<F, P>(
    ui: &mut Ui,
    salt: &'static str,
    rect: Rect,
    gap: f32,
    footer: impl FnOnce(&mut Ui) -> F,
    page: impl FnOnce(&mut Ui) -> P,
) -> (F, P) {
    let h_id = egui::Id::unique((salt, "footer-h"));
    let c_id = egui::Id::unique((salt, "content-h"));
    let prev_h: f32 = ui.ctx().data(|d| d.get_temp(h_id)).unwrap_or(0.0);
    let prev_c: f32 = ui.ctx().data(|d| d.get_temp(c_id)).unwrap_or(f32::INFINITY);
    let room = (rect.bottom() - prev_h - rect.top()).max(0.0);
    let fits = prev_c <= room;
    let page_bottom = if fits {
        rect.top() + prev_c
    } else {
        rect.top() + (room - gap).max(0.0)
    };
    let footer_top = if fits { page_bottom } else { rect.bottom() - prev_h };
    let mut f_ui = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(
        pos2(rect.left(), footer_top),
        pos2(rect.right(), rect.bottom().max(footer_top)),
    )));
    let f = footer(&mut f_ui);
    let h = f_ui.min_rect().height().max(0.0);
    let mut s_ui =
        ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(rect.min, pos2(rect.right(), page_bottom))));
    // egui's default scroll style is floating, with the bar at zero
    // opacity until the pointer hovers it. A solid bar, drawn at full
    // opacity also at rest, in the theme's colours, and only when the
    // page overflows.
    s_ui.spacing_mut().scroll = egui::style::ScrollStyle {
        dormant_background_opacity: SCROLL_REST_OPACITY,
        dormant_handle_opacity: SCROLL_REST_OPACITY,
        active_background_opacity: SCROLL_REST_OPACITY,
        active_handle_opacity: SCROLL_REST_OPACITY,
        ..egui::style::ScrollStyle::solid()
    };
    let out = egui::ScrollArea::vertical()
        .id_salt(salt)
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(&mut s_ui, page);
    let c = out.content_size.y;
    if (h - prev_h).abs() > 0.5 || (c - prev_c).abs() > 0.5 {
        ui.ctx().data_mut(|d| {
            d.insert_temp(h_id, h);
            d.insert_temp(c_id, c);
        });
        ui.ctx().request_discard("a page or its footer changed its height");
    }
    (f, out.inner)
}

/// A centred card over the frozen grid.
pub(crate) fn dialog(ui: &Ui, title: &str, body: impl FnOnce(&mut Ui, &Theme)) {
    dialog_w(ui, title, DIALOG_W, body);
}

/// The width of a dialog's content (points).
const DIALOG_W: f32 = 460.0;

/// `dialog` with its content `width` wide.
pub(crate) fn dialog_w(ui: &Ui, title: &str, width: f32, body: impl FnOnce(&mut Ui, &Theme)) {
    let t = Theme::current(ui.ctx());
    egui::Area::new(egui::Id::unique("dialog"))
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(t.palette().card)
                .stroke(Stroke::new(1.0, t.palette().border))
                .inner_margin(24.0)
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.label(egui::RichText::new(title).font(brand::geo(brand::DIALOG_TITLE_PT)));
                    body(ui, &t);
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brand::rgb::*;

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

    fn status_menu(devices: Vec<battery::Device>, t0: Instant) -> Menu {
        let mut m = Menu::new(shipped_grid(), false);
        // A USB boot, where the Upgrade tile shows too: all shipped tiles.
        m.set_usb_boot(true);
        m.set_status_layout(crate::status::Area::shipped().layout);
        m.set_battery(devices, t0);
        // The app sets the frame's time before every frame.
        m.set_clock(std::time::SystemTime::now(), jiff::tz::TimeZone::UTC);
        m
    }

    /// `status_menu` with a network that has a route: the graph and the connection
    /// have something to show.
    fn net_menu(devices: Vec<battery::Device>, t0: Instant) -> Menu {
        let mut m = status_menu(devices, t0);
        m.set_network(crate::status::network::sample(120));
        m
    }

    /// A wall time well inside a minute (second 10), so no test depends on
    /// how close the real clock is to the next minute.
    fn fixed_wall() -> std::time::SystemTime {
        std::time::UNIX_EPOCH + Duration::from_secs(1_800_000_000 / 60 * 60 + 10)
    }

    /// The shapes of one 1280 x 720 frame, `at` the frame's time, and the
    /// repaint delay egui was asked for.
    fn status_frame(
        ctx: &egui::Context,
        menu: &Menu,
        at: Instant,
        covered: bool,
    ) -> (Vec<egui::epaint::ClippedShape>, Duration) {
        status_frame_sized(ctx, menu, at, covered, screen(1280.0, 720.0))
    }

    /// One frame of a screen `w` by `h`.
    fn status_frame_on(
        ctx: &egui::Context,
        menu: &Menu,
        at: Instant,
        (w, h): (f32, f32),
    ) -> Vec<egui::epaint::ClippedShape> {
        status_frame_sized(ctx, menu, at, false, screen(w, h)).0
    }

    fn status_frame_sized(
        ctx: &egui::Context,
        menu: &Menu,
        at: Instant,
        covered: bool,
        rect: Rect,
    ) -> (Vec<egui::epaint::ClippedShape>, Duration) {
        let input = egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        };
        let icons: Vec<Option<Icon>> = menu.tiles.iter().map(|_| None).collect();
        let chrome = Chrome {
            covered,
            wall: Some(fixed_wall()),
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

    fn five() -> Vec<battery::Device> {
        ["A", "B", "C", "D", "E"]
            .iter()
            .map(|n| dev(&format!("Dev {n}"), 50, "discharging"))
            .collect()
    }

    fn tile_name_top(shapes: &[egui::epaint::ClippedShape], name: &str) -> f32 {
        text_rect(shapes, name).expect("tile name drawn").min.y
    }

    /// Where text `t` is drawn, if it is.
    fn text_rect(shapes: &[egui::epaint::ClippedShape], t: &str) -> Option<Rect> {
        shapes.iter().find_map(|c| match &c.shape {
            egui::Shape::Text(s) if s.galley.text() == t => Some(s.visual_bounding_rect()),
            _ => None,
        })
    }

    /// The cell rectangle `cells` of the area in layout `l`.
    fn cell_rect(l: &Layout, col: u8, row: u8, w: u8, h: u8) -> Rect {
        crate::status::Cells { col, row, w, h }.rect(l.status.unwrap().min, l.tile, TILE_GAP)
    }

    #[test]
    fn the_battery_and_the_clock_stand_side_by_side_in_the_top_row() {
        let t0 = Instant::now();
        let menu = status_menu(vec![kdev("K", battery::DeviceKind::Keyboard, 80, "discharging")], t0);
        let shapes = status_frame_on(&new_ctx(), &menu, t0, (1920.0, 1080.0));
        // The network and hardware elements have nothing to show here, but
        // plan their rows.
        assert_eq!(menu.status.planned_rows(false), 3);
        let l = layout(screen(1920.0, 1080.0), menu.visible_tiles().len(), 0, 1, 3);
        let battery = cell_rect(&l, 0, 0, 1, 1);
        let clock = cell_rect(&l, 1, 0, 1, 1);
        let keyboard = text_rect(&shapes, "Keyboard").expect("the battery line");
        assert!(battery.contains_rect(keyboard), "{keyboard:?} in {battery:?}");
        let time = text_rect(&shapes, "Time").expect("the clock");
        assert!(clock.contains_rect(time), "{time:?} in {clock:?}");
        // No tile of its own any more.
        assert!(menu.tiles.iter().all(|t| t.name != "Battery"));
    }

    #[test]
    fn without_a_device_the_clock_closes_up_and_the_tiles_keep_their_place() {
        let t0 = Instant::now();
        let without_device = status_menu(Vec::new(), t0);
        let without_area = Menu::new(shipped_grid(), false);
        let full = (1920.0, 1080.0);
        let a = status_frame_on(&new_ctx(), &without_device, t0, full);
        let b = status_frame_on(&new_ctx(), &without_area, t0, full);
        let l = layout(screen(full.0, full.1), without_device.visible_tiles().len(), 0, 1, 3);
        assert!(text_rect(&a, "Keyboard").is_none());
        let time = text_rect(&a, "Time").expect("the clock");
        assert!(cell_rect(&l, 0, 0, 1, 1).contains_rect(time), "{time:?}");
        // The area plans its row, so the tiles stand higher than in a menu
        // without an area, which keeps its tiles centred alone.
        assert!(tile_name_top(&a, "Terminal") < tile_name_top(&b, "Terminal"));
        // With a device the grid stays where it was: no tile jump.
        let c = status_frame_on(&new_ctx(), &status_menu(five(), t0), t0, full);
        assert_eq!(tile_name_top(&c, "Terminal"), tile_name_top(&a, "Terminal"));
    }

    #[test]
    fn a_short_screen_with_only_tall_elements_draws_no_area() {
        // The clock has no short size: on a short screen it is like an
        // element too tall for the one row there.
        let t0 = Instant::now();
        let mut menu = status_menu(five(), t0);
        let layout_text = "-battery 1x1\nclock 1x1\n";
        let parsed = crate::status::layout::parse(layout_text.as_bytes(), &crate::status::offered);
        menu.set_status_layout(parsed);
        let short = short_screen(720.0);
        assert!(short);
        assert_eq!(menu.status.planned_rows(short), 0);
        assert_eq!(menu.status.rows(&menu.status_sources(), short), 0);
        let n = menu.visible_tiles().len();
        let l = layout(screen(1280.0, 720.0), n, 0, 0, menu.status.planned_rows(short));
        assert_eq!(l.status, None);
        assert_eq!(l, layout(screen(1280.0, 720.0), n, 0, 0, 0), "tiles as without area");
        let shapes = status_frame(&new_ctx(), &menu, t0, false).0;
        assert!(text_rect(&shapes, "Time").is_none());
        let none = status_frame(&new_ctx(), &Menu::new(shipped_grid(), false), t0, false).0;
        assert_eq!(tile_name_top(&shapes, "Terminal"), tile_name_top(&none, "Terminal"));
    }

    #[test]
    fn an_element_with_no_room_in_the_planned_rows_stays_out_and_off_the_log() {
        // Found by the final review: with no battery the graph went to row 0
        // and the connection to a third row the layout never reserved.
        let t0 = Instant::now();
        let mut menu = net_menu(Vec::new(), t0);
        let parsed = crate::status::layout::parse(
            b"clock 1x1\nbattery 1x1\nnetwork 2x2\nconnection 2x1\n-temperatures 1x1\n-cpu 1x1\n-memory 1x1\n",
            &crate::status::offered,
        );
        menu.set_status_layout(parsed);
        let src = menu.status_sources();
        let planned = menu.status.planned_rows(false);
        let rows = menu.status.rows(&src, false);
        assert_eq!(planned, 2);
        assert!(rows <= planned, "planned {planned}, shown {rows}");
        let n = menu.visible_tiles().len();
        for (w, h) in [(1920.0, 1080.0), (1440.0, 900.0), (1600.0, 1000.0)] {
            let l = layout(screen(w, h), n, 0, rows, planned);
            let status = l.status.expect("the area shows");
            assert!(
                status.bottom() <= l.log.top(),
                "{w}x{h}: {status:?} over the log at {}",
                l.log.top()
            );
        }
    }

    #[test]
    fn a_short_screen_gives_a_chosen_two_by_two_graph_its_short_size_and_drops_the_connection() {
        let t0 = Instant::now();
        let mut menu = net_menu(Vec::new(), t0);
        let parsed = crate::status::layout::parse(
            b"network 2x2\nconnection 1x2\n-battery 1x1\n-clock 1x1\n",
            &crate::status::offered,
        );
        menu.set_status_layout(parsed);
        let n = menu.visible_tiles().len();
        // A screen that is not short: the graph two rows high, the connection beside it.
        assert_eq!(menu.status.planned_rows(false), 2);
        let full = (1920.0, 1080.0);
        let shapes = status_frame_on(&new_ctx(), &menu, t0, full);
        assert!(text_rect(&shapes, "-5m").is_some() && text_rect(&shapes, "IPv4").is_some());
        // A short one: one row, the graph alone at 2 x 1 (the unit inside its
        // plot, no time axis), and the connection, which has no short size, left out.
        let short = short_screen(720.0);
        assert!(short);
        assert_eq!(menu.status.planned_rows(short), 1);
        assert_eq!(menu.status.rows(&menu.status_sources(), short), 1);
        let l = layout(screen(1280.0, 720.0), n, 0, 1, 1);
        let shapes = status_frame(&new_ctx(), &menu, t0, false).0;
        let graph = cell_rect(&l, 0, 0, 2, 1);
        let unit = text_rect(&shapes, "bit/s").expect("the graph is drawn");
        assert!(graph.contains_rect(unit), "{unit:?} in {graph:?}");
        assert!(text_rect(&shapes, "-5m").is_none(), "no time axis in one row");
        assert!(text_rect(&shapes, "IPv4").is_none() && text_rect(&shapes, "VPN").is_none());
        // Where the cell after the graph would be, nothing is drawn.
        let after = cell_rect(&l, 2, 0, 1, 1);
        assert!(
            shapes.iter().all(
                |c| !after.contains_rect(c.shape.visual_bounding_rect()) || matches!(c.shape, egui::Shape::Rect(_))
            )
        );
    }

    #[test]
    fn the_network_elements_ask_for_a_frame_a_second_only_while_shown() {
        let t0 = Instant::now();
        // No battery device and no clock: only the network elements ask.
        let mut menu = net_menu(Vec::new(), t0);
        let parsed = crate::status::layout::parse(
            b"network 2x2\nconnection 1x2\n-battery 1x1\n-clock 1x1\n",
            &crate::status::offered,
        );
        menu.set_status_layout(parsed);
        let settled = |covered| {
            let ctx = new_ctx();
            let mut delay = Duration::ZERO;
            for _ in 0..8 {
                delay = status_frame(&ctx, &menu, t0, covered).1;
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
    fn a_page_turn_is_asked_for_only_while_the_menu_is_shown() {
        let t0 = Instant::now();
        // The battery alone, so the clock's minute does not ask too.
        let mut menu = status_menu(five(), t0);
        let parsed = crate::status::layout::parse(b"battery 1x1\n-clock 1x1\n", &crate::status::offered);
        menu.set_status_layout(parsed);
        let at = t0 + Duration::from_millis(1500);
        // The fonts land in the first frames, which ask for the next at once.
        let settled = |menu: &Menu, covered| {
            let ctx = new_ctx();
            let mut delay = Duration::ZERO;
            for _ in 0..4 {
                delay = status_frame(&ctx, menu, at, covered).1;
            }
            delay
        };
        let delay = settled(&menu, false);
        assert!(
            delay <= Duration::from_millis(2500) && delay > Duration::ZERO,
            "{delay:?}"
        );
        let covered = settled(&menu, true);
        assert!(covered > Duration::from_millis(2500), "{covered:?}");
        menu.set_battery(five()[..2].to_vec(), t0);
        let calm = settled(&menu, false);
        assert!(calm > Duration::from_millis(2500), "nothing pages: no wake: {calm:?}");
    }

    /// The backgrounds the status elements draw: one card-coloured rectangle
    /// each, between the tiles and the log.
    fn element_rects(shapes: &[egui::epaint::ClippedShape], l: &Layout) -> Vec<Rect> {
        let card = crate::brand::theme().palette().card;
        let st = l.status.expect("an area");
        let mut out: Vec<Rect> = Vec::new();
        for c in shapes {
            if let egui::Shape::Rect(r) = &c.shape
                && r.fill == card
                && r.rect.min.y >= st.top() - 1.0
                && r.rect.max.y <= st.bottom() + 1.0
                && !out.contains(&r.rect)
            {
                out.push(r.rect);
            }
        }
        out
    }

    /// Whether an element's background lies on `cells` of the area in layout `l`.
    fn drawn_on(got: &[Rect], l: &Layout, cells: &str) -> bool {
        let want = cells.split(',').map(|n| n.parse::<u8>().unwrap()).collect::<Vec<_>>();
        let want = cell_rect(l, want[0], want[1], want[2], want[3]);
        got.iter()
            .any(|r| (r.min - want.min).length() < 1.0 && (r.size() - want.size()).length() < 1.0)
    }

    #[test]
    fn the_hardware_elements_stand_where_the_owner_put_them_and_are_gone_on_a_short_screen() {
        let t0 = Instant::now();
        let mut menu = net_menu(five(), t0);
        menu.set_hardware(crate::status::hardware::sample());
        let full = (1920.0, 1080.0);
        let n = menu.visible_tiles().len();
        let l = layout(screen(full.0, full.1), n, 0, 3, 3);
        assert_eq!(menu.status.rows(&menu.status_sources(), false), 3);
        let all = status_frame_on(&new_ctx(), &menu, t0, full);
        let got = element_rects(&all, &l);
        assert_eq!(got.len(), 7, "{got:?}");
        for cells in [
            "0,0,1,1", "1,0,2,2", "3,0,1,2", "0,1,1,1", "0,2,1,1", "1,2,1,1", "2,2,1,1",
        ] {
            assert!(drawn_on(&got, &l, cells), "{cells} in {got:?}");
        }
        // Their text is inside their cells.
        for (name, cells) in [("GPU", (0, 2)), ("CPU load", (1, 2)), ("RAM", (2, 2))] {
            let r = text_rect(&all, name).unwrap_or_else(|| panic!("{name}"));
            let cell = cell_rect(&l, cells.0, cells.1, 1, 1);
            assert!(cell.contains_rect(r), "{name} {r:?} in {cell:?}");
        }
        // No battery: the elements close up in first-fit order, the tiles do
        // not move.
        let mut no_battery = net_menu(Vec::new(), t0);
        no_battery.set_hardware(crate::status::hardware::sample());
        let shapes = status_frame_on(&new_ctx(), &no_battery, t0, full);
        let got = element_rects(&shapes, &l);
        assert_eq!(got.len(), 6, "{got:?}");
        for cells in ["0,0,2,2", "2,0,1,2", "3,0,1,1", "3,1,1,1", "0,2,1,1", "1,2,1,1"] {
            assert!(drawn_on(&got, &l, cells), "{cells} in {got:?}");
        }
        assert!(!drawn_on(&got, &l, "2,2,1,1"), "{got:?}");
        assert_eq!(tile_name_top(&shapes, "Terminal"), tile_name_top(&all, "Terminal"));
        // No network: the rest closes up into two rows, tiles unmoved.
        let mut no_net = status_menu(five(), t0);
        no_net.set_hardware(crate::status::hardware::sample());
        let shapes = status_frame_on(&new_ctx(), &no_net, t0, full);
        let got = element_rects(&shapes, &l);
        assert_eq!(got.len(), 5, "{got:?}");
        for cells in ["0,0,1,1", "1,0,1,1", "2,0,1,1", "3,0,1,1", "0,1,1,1"] {
            assert!(drawn_on(&got, &l, cells), "{cells} in {got:?}");
        }
        assert_eq!(tile_name_top(&shapes, "Terminal"), tile_name_top(&all, "Terminal"));
        // A VM: one row, in the top row of the same place, tiles unmoved.
        let mut vm = status_menu(Vec::new(), t0);
        vm.set_hardware(crate::hardware::Snapshot {
            temps: vec![],
            ..crate::status::hardware::sample()
        });
        assert_eq!(vm.status.rows(&vm.status_sources(), false), 1, "no blank row");
        let shapes = status_frame_on(&new_ctx(), &vm, t0, full);
        let got = element_rects(&shapes, &l);
        assert_eq!(got.len(), 4, "{got:?}");
        for cells in ["0,0,1,1", "1,0,1,1", "2,0,1,1", "3,0,1,1"] {
            assert!(drawn_on(&got, &l, cells), "{cells} in {got:?}");
        }
        assert_eq!(tile_name_top(&shapes, "Terminal"), tile_name_top(&all, "Terminal"));
        // Nothing yet from the sampler: the same place again.
        let shapes = status_frame_on(&new_ctx(), &net_menu(five(), t0), t0, full);
        assert_eq!(tile_name_top(&shapes, "Terminal"), tile_name_top(&all, "Terminal"));
        // A short screen: battery and the 2 x 1 graph, nothing else.
        let small = (1280.0, 720.0);
        let l = layout(screen(small.0, small.1), n, 0, 1, 1);
        let got = element_rects(&status_frame_on(&new_ctx(), &menu, t0, small), &l);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(menu.status.rows(&menu.status_sources(), true), 1);
        assert!(drawn_on(&got, &l, "1,0,2,1"), "{got:?}");
    }

    #[test]
    fn a_tall_temperature_element_draws_the_fan_line_below_the_sensors() {
        // At 1x2 the sample's five sensors leave a line free: the fan line
        // takes it, inside the element's cell.
        let t0 = Instant::now();
        let mut menu = net_menu(Vec::new(), t0);
        menu.set_hardware(crate::status::hardware::sample());
        let mut parsed = crate::status::layout::parse(
            b"temperatures 1x2\n-battery 1x1\n-network 2x2\n-connection 1x2\n-clock 1x1\n-cpu 1x1\n-memory 1x1\n",
            &crate::status::offered,
        );
        crate::status::layout::complete(&mut parsed, &crate::status::descriptors());
        menu.set_status_layout(parsed);
        let full = (1920.0, 1080.0);
        let shapes = status_frame_on(&new_ctx(), &menu, t0, full);
        let n = menu.visible_tiles().len();
        let l = layout(screen(full.0, full.1), n, 0, 2, 2);
        let cell = cell_rect(&l, 0, 0, 1, 2);
        let label = text_rect(&shapes, "Fan").expect("the fan label is drawn");
        let value = text_rect(&shapes, "2400 rpm").expect("the fan speed is drawn");
        let lowest_sensor = text_rect(&shapes, "Board").expect("the last sensor is drawn");
        assert!(
            cell.contains_rect(label) && cell.contains_rect(value),
            "{label:?} {value:?} in {cell:?}"
        );
        assert!(value.top() > lowest_sensor.top(), "the fans follow the sensors");
    }

    #[test]
    fn the_shipped_status_area_covers_no_tile_on_any_screen() {
        let t0 = Instant::now();
        let mut menu = net_menu(five(), t0);
        menu.set_hardware(crate::status::hardware::sample());
        let n = menu.visible_tiles().len();
        for (w, h, _) in SHIPPED_SCREENS {
            let short = short_screen(h);
            // Short: one row, the battery and the graph at 2 x 1. Otherwise
            // the graph is 2 x 2, the connection stands right of it and the
            // hardware elements fill the third row.
            let planned = menu.status.planned_rows(short);
            assert_eq!(planned, if short { 1 } else { 3 }, "{w}x{h}");
            assert_eq!(menu.status.rows(&menu.status_sources(), short), planned, "{w}x{h}");
            let l = layout(screen(w, h), n, 0, planned, planned);
            let st = l.status.unwrap();
            assert!(l.grid.bottom() + STATUS_GAP <= st.top() + 0.01, "{w}x{h}: {l:?}");
            assert!(l.grid.bottom() <= l.tiles.bottom() + 0.01, "{w}x{h}: no tile covered");
            assert!(st.bottom() <= l.log.top() - GAP + 0.01, "{w}x{h}: {st:?}");
            let shapes = status_frame_on(&new_ctx(), &menu, t0, (w, h));
            for name in ["Dev A", "Dev D", "bit/s"] {
                let r = text_rect(&shapes, name).unwrap_or_else(|| panic!("{w}x{h}: {name}"));
                assert!(st.contains_rect(r), "{w}x{h}: {r:?} in {st:?}");
            }
            // The connection and the clock only where the screen is not short.
            assert_eq!(text_rect(&shapes, "IPv4").is_some(), !short, "{w}x{h}");
            assert_eq!(text_rect(&shapes, "Time").is_some(), !short, "{w}x{h}");
            if let Some(r) = text_rect(&shapes, "IPv4") {
                assert!(st.contains_rect(r), "{w}x{h}: {r:?} in {st:?}");
            }
            // The hardware elements only where the screen is not short. The
            // sample's five sensors fill the temperature cell: no fan line.
            for name in ["CPU load", "RAM", "GPU"] {
                assert_eq!(text_rect(&shapes, name).is_some(), !short, "{w}x{h}: {name}");
                if let Some(r) = text_rect(&shapes, name) {
                    assert!(st.contains_rect(r), "{w}x{h}: {name} {r:?} in {st:?}");
                }
            }
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
            let mut m = Menu::new(grid, false);
            // A USB boot, where the Upgrade tile shows too.
            m.set_usb_boot(true);
            m.set_status_layout(crate::status::Area::shipped().layout);
            m
        };
        let mut menu = nine();
        menu.set_battery(vec![kdev("K", battery::DeviceKind::Keyboard, 80, "discharging")], t0);
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
        let (none_shown, _) = status_frame(&new_ctx(), &nine(), t0, false);
        assert_eq!(tile_name_top(&with, "Terminal"), tile_name_top(&none_shown, "Terminal"));
        let r = text_rect(&with, "Keyboard").unwrap();
        assert!(st.contains_rect(r), "{r:?} in {st:?}");
    }

    #[test]
    fn the_end_of_session_dialogs_draw_no_status_area() {
        let t0 = Instant::now();
        let mut menu = status_menu(five(), t0);
        let full = (1920.0, 1080.0);
        let shown = status_frame_on(&new_ctx(), &menu, t0, full);
        assert!(text_rect(&shown, "Dev A").is_some() && text_rect(&shown, "Time").is_some());
        menu.logout = Some(Logout::Failed { message: "x".into() });
        let shapes = status_frame_on(&new_ctx(), &menu, t0, full);
        assert!(text_rect(&shapes, "Dev A").is_none());
        assert!(text_rect(&shapes, "Time").is_none());
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

    /// The screens the layout is checked on, with the rows an area may plan
    /// there: one on a short screen, three at most on any other.
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
            "Settings",
            "Packages",
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

    #[test]
    fn the_eject_panel_lists_each_stick_with_size_model_and_buttons() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../overlay/etc/thinfloe/menu");
        let text = std::fs::read_to_string(dir.join("70-eject.desktop")).unwrap();
        let grid = Grid {
            tiles: crate::tiles::merge(
                &[crate::tiles::SourceFile {
                    name: "70-eject.desktop".into(),
                    entry: crate::desktop::parse(&text),
                }],
                &[],
            ),
            banner: vec![],
        };
        let mut menu = Menu::new(grid, false);
        let stick = |disk: &str, name: &str, model: &str, bytes: u64| crate::removable::Stick {
            disk: disk.into(),
            names: vec![name.into()],
            size_bytes: bytes,
            model: model.into(),
            kind: crate::removable::Kind::Stick,
        };
        menu.set_removable(vec![
            stick("sdb", "MYSTICK", "Cruzer", 16_000_000_000),
            stick("sdc", "CARD", "SD Reader", 7_800_000_000),
        ]);
        assert_eq!(menu.activate(0, Instant::now()), None);
        let texts = texts_of(&menu, &[None]);
        for want in [
            "MYSTICK",
            "16 GB",
            "Cruzer",
            "CARD",
            "7.8 GB",
            "SD Reader",
            "Eject all",
            "Close",
        ] {
            assert!(texts.iter().any(|t| t == want), "{want} not in {texts:?}");
        }
        assert_eq!(texts.iter().filter(|t| *t == "Eject").count(), 2, "{texts:?}");
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
            source: egui::MouseWheelSource::Wheel,
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
    fn the_settings_notes_are_one_band_under_the_title() {
        let mut menu = open_settings();
        menu.config_notes = crate::menu::ConfigNotes {
            broken: vec!["config.toml could not be read (bad); defaults are shown.".into()],
            dropped_env: vec!["XKB_DEFAULT_LAYOUT=de".into()],
            report: Some("Moved 3 files.".into()),
            legacy: Some("Your settings could not be moved.".into()),
        };
        let (w, h) = (1920.0, 1080.0);
        let shapes = settings_shapes(&menu, w, h);
        let texts = text_rects(&shapes);
        let find = |what: &str| {
            texts
                .iter()
                .find(|(t, _)| t == what)
                .unwrap_or_else(|| panic!("{what} not drawn in {texts:?}"))
                .1
        };
        let order = [
            find("Settings"),
            find("config.toml could not be read (bad); defaults are shown."),
            find("These lines in ~/.config/thinfloe/environment are ignored because the menu sets them:"),
            find("XKB_DEFAULT_LAYOUT=de"),
            find("Moved 3 files."),
            find("Dismiss"),
            find("Your settings could not be moved."),
            find("Packages"),
        ];
        // Title, broken, dropped heading, the line, the report, the legacy
        // note, then the tabs: top to bottom (Dismiss sits beside the report).
        for pair in order.windows(2) {
            if pair[1] == order[5] {
                continue;
            }
            assert!(pair[0].min.y <= pair[1].min.y + 1.0, "{order:?}");
        }
        assert!(order[6].min.y >= order[4].min.y, "legacy after the report");
        assert!(order[7].min.y > order[6].max.y - 1.0, "the notes are before the tabs");
        // All inside the panel, which is as wide as the tile group.
        let panel_left = (w - crate::density::GROUP_W) / 2.0;
        for r in &order {
            assert!(
                r.min.x >= panel_left - 1.0 && r.max.x <= panel_left + crate::density::GROUP_W + 1.0,
                "{r:?}"
            );
        }
        // No note, no band.
        let plain = text_rects(&settings_shapes(&open_settings(), w, h));
        assert!(!plain.iter().any(|(t, _)| t == "Dismiss"));
    }

    #[test]
    fn settings_draws_nothing_rounded() {
        // (name, menu, screen, texts that must be drawn): the Network
        // cases at 1080p, where every band is on screen, and each case
        // must show its widgets so the check cannot pass on an empty frame.
        type Case = (String, Menu, (f32, f32), &'static [&'static str]);
        let mut cases: Vec<Case> = crate::settings::SettingsTab::ALL
            .into_iter()
            .map(|tab| {
                let mut menu = open_settings();
                menu.settings_tab = tab;
                let (size, wants): ((f32, f32), &[&str]) = match tab {
                    crate::settings::SettingsTab::Network => {
                        menu.network = two_networks();
                        (
                            (1920.0, 1080.0),
                            &[
                                "Fixed address",
                                "192.168.1.10/24",
                                "Hidden",
                                "Forget",
                                "Add network",
                                "Apply",
                            ],
                        )
                    }
                    crate::settings::SettingsTab::Packages => ((1280.0, 720.0), &["Check now"]),
                    crate::settings::SettingsTab::Keyboard => {
                        // With the layout list loaded, as on a real machine.
                        menu.keyboard = keyboard_menu().keyboard;
                        (
                            (1280.0, 1080.0),
                            &[
                                "Layout",
                                "Variant",
                                "CapsLock acts as Ctrl",
                                "Num Lock on at start",
                                "Compose key",
                            ],
                        )
                    }
                    crate::settings::SettingsTab::Sound => {
                        menu.keyboard = keyboard_menu().keyboard;
                        let mut kb = menu.keyboard.shown.clone();
                        kb.volume_keys = true;
                        menu.keyboard = crate::keyboard::KeyboardTab::new(kb, Err("no list".into()));
                        menu.sound.set_file(
                            Some(crate::soundtab::Volume {
                                percent: 40,
                                muted: false,
                                on: true,
                            }),
                            Instant::now(),
                        );
                        (
                            (1280.0, 720.0),
                            &["Set the volume on this machine", "Volume", "40 %", "Mute"],
                        )
                    }
                    crate::settings::SettingsTab::Status => {
                        menu.set_status_layout(crate::status::Area::shipped().layout);
                        (
                            (1920.0, 1080.0),
                            &["Clock", "Up", "Down", "1x1", "Reset to default", "Preview"],
                        )
                    }
                    crate::settings::SettingsTab::Machine => {
                        let f = crate::machine::Facts {
                            maker: Some("HP".into()),
                            ..Default::default()
                        };
                        menu.machine.facts_done(f, Instant::now());
                        ((1280.0, 720.0), &["Maker", "HP", "Boot", "Copy"])
                    }
                    crate::settings::SettingsTab::Screen => {
                        menu.monitors = two_monitors();
                        (
                            (1920.0, 1080.0),
                            &[
                                "Turn the screen off after",
                                "Monitors",
                                "DP-1",
                                "Resolution",
                                "On",
                                "Apply",
                            ],
                        )
                    }
                };
                (format!("{tab:?}"), menu, size, wants)
            })
            .collect();
        cases.push((
            "monitor dialog".into(),
            monitor_settings(monitor_dialog("DP-1")),
            (1920.0, 1080.0),
            &["Position", "Rotation", "Show the menu here", "Close"],
        ));
        cases.push((
            "keep dialog".into(),
            monitor_settings(asking()),
            (1920.0, 1080.0),
            &["Keep this arrangement?", "Keep", "Go back"],
        ));
        cases.push((
            "add dialog".into(),
            network_settings(add_dialog_open()),
            (1920.0, 1080.0),
            &[
                "Office",
                "This security type is not supported",
                "Scan again",
                "Hidden network",
                "Password",
                "Add",
                "Cancel",
            ],
        ));
        for (tab, menu, (w, h), wants) in cases {
            let shapes = settings_shapes(&menu, w, h);
            let texts = all_texts(&shapes);
            for want in wants {
                assert!(texts.iter().any(|t| t == want), "{tab}: {want} not drawn in {texts:?}");
            }
            // Components such as Item draw their box inside a Shape::Vec.
            for r in shapes.iter().flat_map(|c| flat_rects(&c.shape)) {
                assert_eq!(r.corner_radius, egui::CornerRadius::ZERO, "{tab}: rounded {:?}", r.rect);
            }
        }
    }

    /// The Settings screen on its Status tab, with the shipped layout, two
    /// devices and a network: the battery, the graph, the connection and the clock
    /// have something to show, the hardware tiles have not.
    fn status_tab_menu() -> Menu {
        let t0 = Instant::now();
        let mut m = net_menu(
            vec![
                kdev("MX Mechanical", battery::DeviceKind::Keyboard, 100, "discharging"),
                kdev("MX Anywhere 3S", battery::DeviceKind::Mouse, 95, "charging"),
            ],
            t0,
        );
        m.set_clock(fixed_wall(), jiff::tz::TimeZone::UTC);
        m.screen = Screen::Settings;
        m.settings_tab = crate::settings::SettingsTab::Status;
        m
    }

    /// Settled frames of `menu` on a screen of `size`; then, per step, a
    /// click where the step points in the texts of the frame before (no
    /// click when it finds nothing), and the event that click reported.
    #[allow(clippy::type_complexity)]
    fn status_clicks(
        menu: &Menu,
        size: (f32, f32),
        steps: &[&dyn Fn(&[(String, Rect)]) -> Option<egui::Pos2>],
    ) -> Vec<Option<UiEvent>> {
        let ctx = new_ctx();
        let icons: Vec<Option<Icon>> = menu.tiles.iter().map(|_| None).collect();
        let chrome = Chrome {
            wall: Some(fixed_wall()),
            ..Chrome::default()
        };
        let frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen(size.0, size.1)),
                events,
                time: Some(ctx.cumulative_pass_nr() as f64 * 0.1),
                ..Default::default()
            };
            let mut ev = None;
            let mut out = ctx.run_ui(input, |ui| {
                ev = show(ui, menu, &icons, &chrome, Instant::now());
            });
            out.textures_delta.clear();
            (text_rects(&out.shapes), ev)
        };
        let mut texts = vec![];
        for _ in 0..4 {
            texts = frame(vec![]).0;
        }
        let mut events = vec![];
        for step in steps {
            let at = step(&texts);
            let (t, ev) = frame(at.map(click_at).unwrap_or_default());
            events.push(ev);
            texts = t;
            for _ in 0..3 {
                texts = frame(vec![]).0;
            }
        }
        events
    }

    /// The centre of `label` in the row whose element is named `name`.
    fn in_row(texts: &[(String, Rect)], name: &str, label: &str) -> Option<egui::Pos2> {
        let row = texts.iter().find(|(t, _)| t == name)?.1;
        texts
            .iter()
            .filter(|(t, r)| t == label && (r.center().y - row.center().y).abs() < 20.0)
            .map(|(_, r)| r.center())
            .next()
    }

    /// The tick box of the row named `name`: left of the name.
    fn tick_of(texts: &[(String, Rect)], name: &str) -> Option<egui::Pos2> {
        let row = texts.iter().find(|(t, _)| t == name)?.1;
        Some(pos2(row.left() - 16.0, row.center().y))
    }

    #[test]
    fn the_status_tab_has_a_row_per_element_with_tick_size_and_two_buttons() {
        let menu = status_tab_menu();
        // A screen tall enough that no row scrolls away.
        let shapes = settings_shapes(&menu, 1920.0, 2000.0);
        // The rows: what stands above the preview, whose elements draw
        // words of their own.
        let all = text_rects(&shapes);
        assert_eq!(all.iter().filter(|(t, _)| t == "Reset to default").count(), 1);
        let heading = all.iter().find(|(t, _)| t == "Preview").expect("a preview heading").1;
        let texts: Vec<(String, Rect)> = all.into_iter().filter(|(_, r)| r.top() < heading.top()).collect();
        let count = |want: &str| texts.iter().filter(|(t, _)| t == want).count();
        for e in crate::status::ELEMENTS {
            assert_eq!(count(e.name()), 1, "{} in {texts:?}", e.name());
            let size = e.sizes()[0].to_string();
            assert!(
                in_row(&texts, e.name(), &size).is_some(),
                "{}: no size select",
                e.name()
            );
            assert!(in_row(&texts, e.name(), "Up").is_some(), "{}: no Up", e.name());
            assert!(in_row(&texts, e.name(), "Down").is_some(), "{}: no Down", e.name());
            assert!(tick_of(&texts, e.name()).is_some());
        }
        assert_eq!(count("Up"), crate::status::ELEMENTS.len());
        assert_eq!(count("Down"), crate::status::ELEMENTS.len());
        // The rows are in the order of the file.
        let ys: Vec<f32> = crate::status::ELEMENTS
            .iter()
            .map(|e| texts.iter().find(|(t, _)| t == e.name()).unwrap().1.top())
            .collect();
        assert!(ys.windows(2).all(|w| w[0] < w[1]), "{ys:?}");
        assert!(heading.top() > *ys.last().unwrap(), "the preview stands below the list");
    }

    fn keyboard_menu() -> Menu {
        let mut menu = open_settings();
        menu.settings_tab = crate::settings::SettingsTab::Keyboard;
        let reg = crate::kbregistry::parse(include_str!("testdata/evdev-small.xml")).unwrap();
        menu.keyboard = crate::keyboard::KeyboardTab::new(crate::config::Keyboard::default(), Ok(reg));
        menu
    }

    #[test]
    fn the_keyboard_tab_shows_its_rows() {
        for size in [(1920.0, 1080.0), (1280.0, 900.0)] {
            let texts = text_rects(&settings_shapes(&keyboard_menu(), size.0, size.1));
            let at = |what: &str| {
                texts
                    .iter()
                    .find(|(t, _)| t == what)
                    .unwrap_or_else(|| panic!("{what} not drawn in {texts:?}"))
                    .1
            };
            let layout = at("Layout");
            let variant = at("Variant");
            let caps = at("CapsLock acts as Ctrl");
            let numlock = at("Num Lock on at start");
            let compose = at("Compose key");
            // One column of rows, top to bottom. At 1280x900 the test field
            // is below the fold and reached by scrolling.
            let mut ys = vec![layout, variant, caps, numlock, compose];
            if size.1 > 900.0 {
                ys.push(at("Try it here"));
            }
            let ys: Vec<f32> = ys.iter().map(|r| r.top()).collect();
            assert!(ys.windows(2).all(|w| w[0] < w[1]), "{ys:?}");
            // The labels share a left edge; the controls start one label
            // column further right.
            for r in [variant, caps, numlock, compose] {
                assert!((r.left() - layout.left()).abs() < 1.0);
            }
            for (shown, label) in [("English (US)", layout), ("Default", variant), ("Right Alt", compose)] {
                let r = at(shown);
                assert!(r.left() >= label.left() + brand::FORM_LABEL_W, "{shown}: {r:?}");
                assert!((r.center().y - label.center().y).abs() < 20.0, "{shown}");
            }
        }
    }

    #[test]
    fn the_keyboard_note_shows_an_error() {
        let mut menu = keyboard_menu();
        let none = text_rects(&settings_shapes(&menu, 1920.0, 1080.0));
        assert!(!none.iter().any(|(t, _)| t.contains("labwc")));
        menu.keyboard.note = Some("labwc --reconfigure failed (1)".into());
        let texts = text_rects(&settings_shapes(&menu, 1920.0, 1080.0));
        let note = texts
            .iter()
            .find(|(t, _)| t == "labwc --reconfigure failed (1)")
            .expect("the note is drawn")
            .1;
        // Directly under the card with the selects, above the test field.
        let compose = texts.iter().find(|(t, _)| t == "Compose key").unwrap().1;
        let try_it = texts.iter().find(|(t, _)| t == "Try it here").unwrap().1;
        assert!(
            note.top() > compose.bottom() && note.bottom() < try_it.top(),
            "{note:?}"
        );
    }

    #[test]
    fn a_long_keyboard_note_wraps_instead_of_being_cut() {
        let error = "labwc --reconfigure failed (1): the compositor refused the new keyboard settings \
                     and kept the old ones, so nothing changed on this machine until the next start";
        let mut menu = keyboard_menu();
        menu.keyboard.note = Some(error.into());
        let texts = text_rects(&settings_shapes(&menu, 1280.0, 1080.0));
        // One galley holding the whole text, on more than one row.
        let note = texts
            .iter()
            .find(|(t, _)| t == error)
            .unwrap_or_else(|| panic!("the note is cut: {texts:?}"))
            .1;
        let line = texts.iter().find(|(t, _)| t == "Layout").unwrap().1.height();
        assert!(note.height() > line * 1.5, "one line: {note:?}");
        // Card 2 follows below the note (it scrolls into view at this size).
        assert!(note.max.y < 1080.0 && note.max.x <= 1280.0, "{note:?}");
    }

    #[test]
    fn the_keyboard_tab_takes_no_click_under_a_legacy_home() {
        let click_caps = |menu: &Menu| {
            let at = |t: &[(String, Rect)]| {
                let r = t.iter().find(|(s, _)| s == "CapsLock acts as Ctrl")?.1;
                Some(pos2(r.left() + brand::FORM_LABEL_W + 14.0, r.center().y))
            };
            status_clicks(menu, (1280.0, 720.0), &[&at]).remove(0)
        };
        let menu = keyboard_menu();
        assert_eq!(
            click_caps(&menu),
            Some(UiEvent::Settings(SettingsEvent::Keyboard(
                crate::keyboard::KbEvent::CapsAsCtrl(false)
            )))
        );
        let mut legacy = keyboard_menu();
        legacy.config_notes.legacy = Some("Your settings could not be moved.".into());
        assert_eq!(click_caps(&legacy), None);
    }

    #[test]
    fn the_status_tab_reports_its_clicks() {
        use crate::statustab::StatusEvent;
        let menu = status_tab_menu();
        let s = |e| Some(UiEvent::Settings(SettingsEvent::Status(e)));
        let size = (1920.0, 2000.0);
        let events = status_clicks(
            &menu,
            size,
            &[
                &|t| in_row(t, "Network traffic", "Up"),
                &|t| in_row(t, "Memory", "Down"),
                &|t| tick_of(t, "Clock"),
                // The select shows the size; its list offers the others.
                &|t| in_row(t, "Keyboard and mouse batteries", "1x1"),
                &|t| t.iter().find(|(s, _)| s == "2x1").map(|(_, r)| r.center()),
                &|t| t.iter().find(|(s, _)| s == "Reset to default").map(|(_, r)| r.center()),
            ],
        );
        assert_eq!(
            events,
            [
                s(StatusEvent::Up("network".into())),
                // The bottom row's Down is off: no event.
                None,
                s(StatusEvent::Shown("clock".into(), false)),
                None,
                s(StatusEvent::Size("battery".into(), crate::status::Size { w: 2, h: 1 })),
                s(StatusEvent::Reset),
            ]
        );
        // The top row's Up is off too.
        let top = status_clicks(&menu, size, &[&|t| in_row(t, "Keyboard and mouse batteries", "Up")]);
        assert_eq!(top, [None]);
    }

    #[test]
    fn clicking_an_elements_name_ticks_its_box() {
        use crate::statustab::StatusEvent;
        let menu = status_tab_menu();
        let s = |e| Some(UiEvent::Settings(SettingsEvent::Status(e)));
        let name =
            |n: &'static str| move |t: &[(String, Rect)]| t.iter().find(|(s, _)| s == n).map(|(_, r)| r.center());
        let clock = name("Clock");
        let traffic = name("Network traffic");
        let events = status_clicks(&menu, (1920.0, 2000.0), &[&clock, &traffic]);
        assert_eq!(
            events,
            [
                s(StatusEvent::Shown("clock".into(), false)),
                s(StatusEvent::Shown("network".into(), false)),
            ]
        );
    }

    #[test]
    fn an_element_the_row_limit_leaves_out_says_no_room_in_its_row() {
        // statustab.rs's layout: with the battery read and empty, the connection
        // would need a third row, which was never reserved.
        let t0 = Instant::now();
        let mut menu = net_menu(vec![], t0);
        let text = "clock 1x1\nbattery 1x1\nnetwork 2x2\nconnection 2x1\n-temperatures 1x1\n-cpu 1x1\n-memory 1x1\n";
        let mut layout = crate::status::layout::parse(text.as_bytes(), &crate::status::offered);
        crate::status::layout::complete(&mut layout, &crate::status::descriptors());
        menu.set_status_layout(layout);
        menu.screen = Screen::Settings;
        menu.settings_tab = crate::settings::SettingsTab::Status;
        let sources = menu.status_sources();
        let placed: Vec<&str> = menu
            .status
            .shown(&sources, false)
            .iter()
            .map(|(e, _, _)| e.id())
            .collect();
        assert!(!placed.contains(&"connection"), "{placed:?}");
        let texts = text_rects(&settings_shapes(&menu, 1920.0, 1080.0));
        assert!(
            in_row(&texts, "Connection", "no room").is_some(),
            "no note in {texts:?}"
        );
    }

    #[test]
    fn status_notes_stay_in_their_row_on_a_narrow_screen() {
        // A short screen 600 wide, narrower than 1024 at scale 1.25 (819):
        // the hardware elements say "not on a small screen", the empty
        // battery "nothing to show on this machine", and a crowded list
        // "no room" for the rest.
        let t0 = Instant::now();
        let mut menu = status_menu(vec![], t0);
        let text = "network 3x1\nconnection 1x1\nclock 1x1\nbattery 1x1\ntemperatures 1x1\ncpu 1x1\nmemory 1x1\n";
        let mut layout = crate::status::layout::parse(text.as_bytes(), &crate::status::offered);
        crate::status::layout::complete(&mut layout, &crate::status::descriptors());
        menu.set_status_layout(layout);
        menu.set_network(crate::status::network::sample(120));
        menu.screen = Screen::Settings;
        menu.settings_tab = crate::settings::SettingsTab::Status;
        let sources = menu.status_sources();
        let notes = crate::statustab::rows(&menu.status, &sources, true)
            .iter()
            .filter(|r| r.note.is_some())
            .count();
        assert!(notes >= 6, "only {notes} rows have a note");
        let shapes = settings_shapes(&menu, 600.0, 899.0);
        let texts = text_rects(&shapes);
        let heading = text_rect(&shapes, "Preview").map_or(f32::INFINITY, |r| r.top());
        let list: Vec<&(String, Rect)> = texts.iter().filter(|(_, r)| r.bottom() < heading).collect();
        let note_texts = ["no room", "not on a small screen", "nothing to show on this machine"];
        let mut seen = 0;
        for (t, r) in &list {
            // A cut note ends with the ellipsis.
            if !note_texts
                .iter()
                .any(|n| t == n || (t.ends_with('…') && n.starts_with(t.trim_end_matches('…'))))
            {
                continue;
            }
            seen += 1;
            for (o, q) in &list {
                if (o, q) != (t, r) && (q.center().y - r.center().y).abs() < 12.0 {
                    assert!(
                        !r.shrink(0.5).intersects(q.shrink(0.5)),
                        "{t:?} {r:?} overlaps {o:?} {q:?}"
                    );
                }
            }
        }
        assert!(seen >= 3, "notes drawn: {seen} in {list:?}");
        // Nothing runs past the right edge of the screen either.
        for (t, r) in &list {
            assert!(r.right() <= 600.0, "{t:?} {r:?} runs past the screen");
        }
    }

    #[test]
    fn every_status_row_fits_at_1080p() {
        // 1080p is the common screen; there the list must not scroll.
        let menu = status_tab_menu();
        let shapes = settings_shapes(&menu, 1920.0, 1080.0);
        let heading = text_rect(&shapes, "Preview").expect("a preview heading");
        for e in crate::status::ELEMENTS {
            let shown = shapes.iter().any(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text() == e.name() => {
                    let r = t.galley.rect.translate(t.pos.to_vec2());
                    c.clip_rect.contains_rect(r) && r.bottom() < heading.top()
                }
                _ => false,
            });
            assert!(shown, "{} is not on screen above the preview", e.name());
        }
    }

    #[test]
    fn the_size_select_offers_only_the_sizes_of_its_element() {
        let menu = status_tab_menu();
        for e in crate::status::ELEMENTS {
            // The size texts with the element's list closed, then open: the
            // list adds one entry per size offered.
            let closed = std::cell::RefCell::new(Vec::new());
            let open = std::cell::RefCell::new(Vec::new());
            let sizes = |t: &[(String, Rect)]| -> Vec<String> {
                t.iter()
                    .filter(|(s, _)| crate::status::Size::parse(s).is_some())
                    .map(|(s, _)| s.clone())
                    .collect()
            };
            let current = e.sizes()[0].to_string();
            status_clicks(
                &menu,
                (1920.0, 2000.0),
                &[
                    &|t| {
                        *closed.borrow_mut() = sizes(t);
                        in_row(t, e.name(), &current)
                    },
                    &|t| {
                        *open.borrow_mut() = sizes(t);
                        None
                    },
                ],
            );
            let mut added = open.into_inner();
            for c in closed.into_inner() {
                if let Some(i) = added.iter().position(|a| *a == c) {
                    added.remove(i);
                }
            }
            added.sort();
            let mut want: Vec<String> = e.sizes().iter().map(|s| s.to_string()).collect();
            want.sort();
            // A single size has nothing to choose: its select does not open.
            if want.len() == 1 {
                want.clear();
            }
            assert_eq!(added, want, "{}", e.id());
        }
    }

    /// The element backgrounds in `region`, relative to the first one's
    /// top-left corner of the whole set.
    fn relative_cards(shapes: &[egui::epaint::ClippedShape], region: Rect) -> Vec<Rect> {
        let card = crate::brand::theme().palette().card;
        let mut out: Vec<Rect> = Vec::new();
        for c in shapes {
            if let egui::Shape::Rect(r) = &c.shape
                && r.fill == card
                && region.contains_rect(r.rect)
                && !out.contains(&r.rect)
            {
                out.push(r.rect);
            }
        }
        let min = out.iter().fold(pos2(f32::INFINITY, f32::INFINITY), |m, r| m.min(r.min));
        let mut rel: Vec<Rect> = out.iter().map(|r| r.translate(-min.to_vec2())).collect();
        rel.sort_by(|a, b| (a.min.y, a.min.x).partial_cmp(&(b.min.y, b.min.x)).unwrap());
        rel
    }

    #[test]
    fn the_preview_is_the_main_screens_status_area() {
        for (w, h) in [(1920.0, 1080.0), (1280.0, 720.0)] {
            let mut menu = status_tab_menu();
            let preview = settings_shapes(&menu, w, h);
            let heading = text_rect(&preview, "Preview").expect("a preview heading");
            let log = layout(screen(w, h), 0, 0, 0, 0).log;
            let below = Rect::from_min_max(pos2(0.0, heading.bottom()), pos2(w, log.top()));
            let got = relative_cards(&preview, below);
            menu.screen = Screen::Grid;
            let main = settings_shapes(&menu, w, h);
            let short = short_screen(h);
            let sources = menu.status_sources();
            let n = menu.visible_tiles().len();
            let l = layout(
                screen(w, h),
                n,
                0,
                menu.status.rows(&sources, short),
                menu.status.planned_rows(short),
            );
            let want = relative_cards(&main, l.status.unwrap().expand(1.0));
            assert!(!want.is_empty());
            assert_eq!(got.len(), want.len(), "{w}x{h}: {got:?} vs {want:?}");
            for (g, m) in got.iter().zip(&want) {
                assert!(
                    (g.min - m.min).length() < 0.5 && (g.size() - m.size()).length() < 0.5,
                    "{w}x{h}: {g:?} vs {m:?}"
                );
            }
        }
    }

    #[test]
    fn the_status_tab_fits_without_sideways_scrolling() {
        let menu = status_tab_menu();
        for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0)] {
            let shapes = settings_shapes(&menu, w, h);
            let panel_w = GROUP_W.min(w - 2.0 * GAP);
            let (left, right) = ((w - panel_w) / 2.0, (w + panel_w) / 2.0);
            let log = layout(screen(w, h), 0, 0, 0, 0).log;
            for c in &shapes {
                let b = c.shape.visual_bounding_rect();
                if !b.is_positive() || c.clip_rect.intersect(b).area() <= 0.0 {
                    continue;
                }
                let seen = c.clip_rect.intersect(b);
                // The log and the header span the screen; the tab lies in the panel.
                if seen.top() < HEADER_H || seen.top() >= log.top() - 0.5 {
                    continue;
                }
                assert!(
                    seen.left() >= left - 1.0 && seen.right() <= right + 1.0,
                    "{w}x{h}: {seen:?} outside {left}..{right}: {:?}",
                    c.shape
                );
            }
            // Reset and the preview stay on screen, above the log.
            let texts = text_rects(&shapes);
            for want in ["Reset to default", "Preview"] {
                let r = texts.iter().find(|(t, _)| t == want).map(|(_, r)| *r);
                assert!(r.is_some_and(|r| r.bottom() < log.top()), "{w}x{h}: {want} {r:?}");
            }
        }
    }

    #[test]
    fn a_row_says_why_its_element_is_not_drawn_and_a_failed_save_shows_in_warn() {
        let mut menu = status_tab_menu();
        menu.set_battery(Vec::new(), Instant::now());
        menu.status_error = Some("disk full".into());
        let shapes = settings_shapes(&menu, 1920.0, 2000.0);
        let texts = text_rects(&shapes);
        let note = in_row(
            &texts,
            "Keyboard and mouse batteries",
            "nothing to show on this machine",
        );
        assert!(note.is_some(), "{texts:?}");
        assert!(in_row(&texts, "Clock", "nothing to show on this machine").is_none());
        let colour = |want: &str| {
            shapes.iter().find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text().contains(want) => Some(t.galley.job.sections[0].format.color),
                _ => None,
            })
        };
        assert_eq!(colour("nothing to show"), Some(MUTED));
        assert_eq!(colour("disk full"), Some(WARN));
    }

    /// Every rect in `shape`, also those nested in a `Shape::Vec`.
    fn flat_rects(shape: &egui::Shape) -> Vec<egui::epaint::RectShape> {
        match shape {
            egui::Shape::Rect(r) => vec![r.clone()],
            egui::Shape::Vec(v) => v.iter().flat_map(flat_rects).collect(),
            _ => vec![],
        }
    }

    /// The Settings menu of `four_addons()` on its Screen tab, 5 minutes set.
    fn screen_settings() -> Menu {
        let mut menu = open_settings();
        menu.settings_tab = crate::settings::SettingsTab::Screen;
        menu.screen_tab = crate::screentab::ScreenTab { idle: 300, error: None };
        menu
    }

    fn settings_texts(menu: &Menu) -> Vec<String> {
        text_rects(&settings_shapes(menu, 1280.0, 720.0))
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    /// The This machine tab with its facts, a WiFi interface and a GPU read.
    fn machine_settings() -> Menu {
        let mut menu = open_settings();
        menu.settings_tab = crate::settings::SettingsTab::Machine;
        let f = crate::machine::Facts {
            maker: Some("HP".into()),
            kernel: Some("7.0.0-38-generic".into()),
            ..Default::default()
        };
        menu.machine.facts_done(f, Instant::now());
        let wifi = crate::nettab::Iface {
            name: "wlp2s0".into(),
            kind: crate::nettab::IfKind::Wifi,
            state: crate::nettab::IfState::Connected,
            addresses: vec!["10.0.0.7/24".into()],
            ssid: Some("Home".into()),
            signal_dbm: None,
            handshake_age_s: None,
        };
        let details = crate::netdetail::Details {
            ifaces: vec![crate::netdetail::IfDetail {
                name: "wlp2s0".into(),
                driver: Some("iwlwifi".into()),
                ..Default::default()
            }],
            dns_in_use: vec!["192.168.1.1".into()],
            ..Default::default()
        };
        menu.network.status_done(Ok(vec![wifi]), details);
        menu.graphics = Some(crate::graphics::Info {
            gpus: vec![crate::graphics::Gpu {
                card: "card1".into(),
                ids: (0x8086, 0x1616),
                name: None,
                driver: Some("i915".into()),
            }],
            vaapi: crate::graphics::Vaapi::NotAvailable("x".into()),
        });
        menu
    }

    /// On a 1920x1080 screen the last monitor's row is in view without
    /// a scroll, also with a full Graphics band (a GPU, its driver and
    /// three codecs). With the band on top the Position row of the time
    /// was not (2026-11-20).
    #[test]
    fn the_monitor_rows_are_in_view_on_a_1080_screen() {
        use crate::graphics::{Codec, Gpu, Info, Vaapi};
        let codec = |name| Codec {
            name,
            decode: true,
            encode: true,
        };
        let mut menu = monitor_settings(two_monitors());
        menu.graphics = Some(Info {
            gpus: vec![Gpu {
                card: "card1".into(),
                ids: (0x8086, 0x1616),
                name: Some("Intel Corporation HD Graphics 5500".into()),
                driver: Some("i915".into()),
            }],
            vaapi: Vaapi::Codecs(vec![codec("H.264"), codec("HEVC"), codec("VP9")]),
        });
        let shapes = settings_shapes(&menu, 1920.0, 1080.0);
        let side = shapes
            .iter()
            .find_map(|c| match &c.shape {
                // DP-1's Resolution select, in the last row.
                egui::Shape::Text(t) if t.galley.text() == "2560 × 1440 @ 60 Hz" => {
                    Some((t.galley.rect.translate(t.pos.to_vec2()), c.clip_rect))
                }
                _ => None,
            })
            .expect("the last row's Resolution select");
        let (text, clip) = side;
        // The select's box reaches about half a line past its text.
        let select = text.expand2(egui::vec2(0.0, text.height() / 2.0));
        assert!(
            clip.contains_rect(select),
            "select {select:?} outside the page {clip:?}"
        );
    }

    /// This machine shows what the Network and Screen tabs show, beside
    /// the machine's facts on a wide panel, below them on a narrow one.
    #[test]
    fn the_machine_tab_has_two_columns_when_wide() {
        // The first one below the tab bar, whose labels share the names.
        let rect_of = |rects: &[(String, Rect)], name: &str| {
            let tabs = rects
                .iter()
                .find(|(t, _)| t == "Packages")
                .map_or(0.0, |(_, r)| r.bottom());
            rects
                .iter()
                .find(|(t, r)| t == name && r.top() > tabs)
                .map(|(_, r)| *r)
                .unwrap_or_else(|| panic!("{name} not in {rects:?}"))
        };
        let menu = machine_settings();
        let wide = text_rects(&settings_shapes(&menu, 1920.0, 1080.0));
        for want in [
            "This machine",
            "Maker",
            "HP",
            "Network",
            "WiFi wlp2s0",
            "Driver",
            "iwlwifi",
            "DNS in use",
            "Graphics",
            "GPU",
            "8086:1616",
            "i915",
            "Video processing",
            "not available",
            "Copy",
        ] {
            rect_of(&wide, want);
        }
        let (m, n, g) = (
            rect_of(&wide, "This machine"),
            rect_of(&wide, "Network"),
            rect_of(&wide, "Graphics"),
        );
        assert!(n.left() > m.right(), "Network beside: {m:?} {n:?}");
        assert!((n.top() - m.top()).abs() < 1.0, "{m:?} {n:?}");
        assert!(g.top() > n.bottom() && (g.left() - n.left()).abs() < 1.0, "{n:?} {g:?}");
        // Copy under the short left column, beside the network.
        let copy = rect_of(&wide, "Copy");
        assert!(
            copy.top() > rect_of(&wide, "Manual tool").bottom() && copy.right() < n.left(),
            "{copy:?}"
        );
        let narrow = text_rects(&settings_shapes(&menu, 700.0, 1400.0));
        let (m, n) = (rect_of(&narrow, "This machine"), rect_of(&narrow, "Network"));
        assert!(n.top() > rect_of(&narrow, "Kernel").bottom(), "{m:?} {n:?}");
        assert!((n.left() - m.left()).abs() < 1.0, "{m:?} {n:?}");
        // One column: Copy after the last panel.
        assert!(rect_of(&narrow, "Copy").top() > rect_of(&narrow, "not available").bottom());
    }

    /// Until the machine's facts arrive, only their band waits: the
    /// network and the graphics, read already, show.
    #[test]
    fn the_machine_tab_shows_what_is_read_while_the_facts_are_waited_for() {
        let mut menu = machine_settings();
        menu.machine = crate::machine::MachineTab::default();
        let rects = text_rects(&settings_shapes(&menu, 1920.0, 1080.0));
        let texts: Vec<&str> = rects.iter().map(|(t, _)| t.as_str()).collect();
        for want in [
            "This machine",
            "Network",
            "WiFi wlp2s0",
            "iwlwifi",
            "Graphics",
            "i915",
            "Copy",
        ] {
            assert!(texts.contains(&want), "{want} not in {texts:?}");
        }
        assert!(!texts.contains(&"Kernel"), "{texts:?}");
        let reading: Vec<Rect> = rects.iter().filter(|(t, _)| t == "Reading…").map(|(_, r)| *r).collect();
        let at = |name: &str| rects.iter().rev().find(|(t, _)| t == name).map(|(_, r)| *r).unwrap();
        let (m, n) = (at("This machine"), at("Network"));
        assert_eq!(reading.len(), 1, "{texts:?}");
        assert!(
            reading[0].top() > m.bottom() && reading[0].right() < n.left(),
            "{reading:?}"
        );
    }

    /// Copy takes no click until the machine's facts are read.
    #[test]
    fn copy_takes_a_click_only_once_the_facts_are_read() {
        let copy = Some(UiEvent::Settings(SettingsEvent::MachineCopy));
        let mut menu = machine_settings();
        assert_eq!(click_text(&menu, 1920.0, 1080.0, "Copy"), copy);
        menu.machine = crate::machine::MachineTab::default();
        assert_ne!(click_text(&menu, 1920.0, 1080.0, "Copy"), copy);
    }

    /// A panel looks the same on every tab: the gap between the columns of
    /// This machine does not reach the label column inside them.
    #[test]
    fn a_panels_values_stand_as_far_from_their_labels_on_every_tab() {
        // From a label's left edge to its value's, in the WiFi group.
        let offset = |menu: &Menu| {
            let rects = text_rects(&settings_shapes(menu, 1920.0, 1600.0));
            let group = rects
                .iter()
                .find(|(t, _)| t == "WiFi wlp2s0")
                .map(|(_, r)| *r)
                .expect("the WiFi group");
            let below = |name: &str| {
                rects
                    .iter()
                    .filter(|(t, r)| t == name && r.top() > group.top())
                    .map(|(_, r)| *r)
                    .min_by(|a, b| a.top().total_cmp(&b.top()))
                    .unwrap_or_else(|| panic!("{name} not in {rects:?}"))
            };
            below("iwlwifi").left() - below("Driver").left()
        };
        let mut menu = machine_settings();
        let machine = offset(&menu);
        menu.settings_tab = crate::settings::SettingsTab::Network;
        let network = offset(&menu);
        assert!(
            (machine - network).abs() < 0.5,
            "This machine {machine}, Network {network}"
        );
    }

    /// The Graphics band closes the Screen tab: "Reading…" until the job
    /// answers, then each GPU and driver and the codecs under "Video
    /// processing", or why there are none, each value beside its label.
    #[test]
    fn the_graphics_band_closes_the_screen_tab() {
        use crate::graphics::{Codec, Gpu, Info, Vaapi};
        let texts = |menu: &Menu| -> Vec<String> {
            text_rects(&settings_shapes(menu, 1920.0, 1600.0))
                .into_iter()
                .map(|(t, _)| t)
                .collect()
        };
        let at = |texts: &[String], name: &str| {
            texts
                .iter()
                .position(|t| t == name)
                .unwrap_or_else(|| panic!("{name} not in {texts:?}"))
        };
        let mut menu = screen_settings();
        let got = texts(&menu);
        assert!(at(&got, "Turn the screen off after") < at(&got, "Graphics"), "{got:?}");
        assert!(at(&got, "Monitors") < at(&got, "Graphics"), "{got:?}");
        assert!(at(&got, "Graphics") < at(&got, "Reading…"), "{got:?}");
        menu.graphics = Some(Info {
            gpus: vec![Gpu {
                card: "card0".into(),
                ids: (0x8086, 0x1916),
                name: None,
                driver: Some("i915".into()),
            }],
            vaapi: Vaapi::Codecs(vec![Codec {
                name: "H.264",
                decode: true,
                encode: true,
            }]),
        });
        let got = texts(&menu);
        for want in [
            "GPU",
            "8086:1916",
            "Driver",
            "i915",
            "Video processing",
            "H.264",
            "decode, encode",
        ] {
            assert!(has(&got, want), "{want} not in {got:?}");
        }
        assert!(!has(&got, "Reading…"), "{got:?}");
        // A GPU without a driver link has no Driver row.
        menu.graphics = Some(Info {
            gpus: vec![Gpu {
                card: "card0".into(),
                ids: (0x8086, 0x1916),
                name: None,
                driver: None,
            }],
            vaapi: Vaapi::NotAvailable("vainfo lists no codec".into()),
        });
        let got = texts(&menu);
        assert!(has(&got, "GPU") && has(&got, "8086:1916"), "{got:?}");
        assert!(!has(&got, "Driver"), "{got:?}");
        menu.graphics = Some(Info {
            gpus: Vec::new(),
            vaapi: Vaapi::NotAvailable("vainfo exited with status 1".into()),
        });
        let got = texts(&menu);
        assert!(has(&got, "none found") && has(&got, "not available"), "{got:?}");
        assert!(at(&got, "GPU") < at(&got, "none found"), "{got:?}");
        // The cause is for the log, not the band.
        assert!(!got.iter().any(|t| t.contains("status 1")), "{got:?}");
    }

    #[test]
    fn the_sound_tab_says_turning_on_and_shows_a_failed_apply() {
        let mut menu = open_settings();
        menu.settings_tab = crate::settings::SettingsTab::Sound;
        let mut kb = menu.keyboard.shown.clone();
        kb.volume_keys = true;
        menu.keyboard = crate::keyboard::KeyboardTab::new(kb, Err("no list".into()));
        let off = crate::soundtab::Volume {
            percent: 100,
            muted: false,
            on: false,
        };
        menu.sound.set_file(Some(off), Instant::now());
        let texts = settings_texts(&menu);
        assert!(texts.iter().any(|t| t == crate::soundtab::TURNING_ON_NOTE), "{texts:?}");
        // A note of another keyboard setting stays on the Keyboard tab.
        menu.keyboard.note = Some(crate::keyboard::NUMLOCK_NEXT_START.into());
        let texts = settings_texts(&menu);
        assert!(
            !texts.iter().any(|t| t == crate::keyboard::NUMLOCK_NEXT_START),
            "{texts:?}"
        );
        menu.keyboard.volume_note = Some("The apply failed for a test.".into());
        let texts = settings_texts(&menu);
        assert!(texts.iter().any(|t| t == "The apply failed for a test."), "{texts:?}");
        menu.sound
            .set_file(Some(crate::soundtab::Volume { on: true, ..off }), Instant::now());
        let texts = settings_texts(&menu);
        assert!(
            !texts.iter().any(|t| t == crate::soundtab::TURNING_ON_NOTE),
            "{texts:?}"
        );
    }

    #[test]
    fn settings_shows_both_tabs() {
        for tab in crate::settings::SettingsTab::ALL {
            let mut menu = open_settings();
            menu.settings_tab = tab;
            assert_texts(
                &settings_shapes(&menu, 1280.0, 720.0),
                &["Settings", "Packages", "Screen", "Back"],
            );
        }
    }

    #[test]
    fn the_screen_tab_shows_the_timeout() {
        let texts = settings_texts(&screen_settings());
        for want in ["Turn the screen off after", "5 minutes"] {
            assert!(texts.iter().any(|t| t == want), "{want} not in {texts:?}");
        }
        // The add-on rows belong to the other tab.
        assert!(!texts.iter().any(|t| t == "ThinLinc" || t == "Check now"), "{texts:?}");
    }

    #[test]
    fn the_screen_tab_shows_the_stored_value() {
        for (idle, want) in [(0, "Never"), (900, "15 minutes"), (450, "450 seconds")] {
            let mut menu = screen_settings();
            menu.screen_tab = crate::screentab::from_config(&crate::config::ScreenCfg { idle_timeout: idle });
            let texts = settings_texts(&menu);
            assert!(texts.iter().any(|t| t == want), "{want} not in {texts:?}");
            assert!(!texts.iter().any(|t| t == "5 minutes"), "{texts:?}");
        }
    }

    #[test]
    fn the_screen_tab_has_no_sessionrc_or_take_over_notes() {
        let texts = settings_texts(&screen_settings());
        for gone in ["set in ~/.thinfloe-sessionrc", "Let the menu manage it"] {
            assert!(!texts.iter().any(|t| t == gone), "{gone} in {texts:?}");
        }
    }

    /// Clicks the texts `labels` of `menu`'s screen one after the other,
    /// each where the frame before drew it, and returns what each reported.
    /// A label that is not drawn is `None` in `drawn`.
    fn click_labels(menu: &Menu, labels: &[&str]) -> (Vec<Option<UiEvent>>, Vec<bool>) {
        click_labels_at(menu, labels, (1280.0, 720.0))
    }

    /// `click_labels` on a screen of `size` points.
    fn click_labels_at(menu: &Menu, labels: &[&str], size: (f32, f32)) -> (Vec<Option<UiEvent>>, Vec<bool>) {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen(size.0, size.1)),
                events,
                time: Some(ctx.cumulative_pass_nr() as f64 * 0.1),
                ..Default::default()
            };
            let mut ev = None;
            let mut out = ctx.run_ui(input, |ui| {
                ev = show(ui, menu, &[None], &Chrome::default(), Instant::now());
            });
            out.textures_delta.clear();
            (text_rects(&out.shapes), ev)
        };
        let mut texts = vec![];
        for _ in 0..3 {
            texts = frame(vec![]).0;
        }
        let (mut events, mut drawn) = (vec![], vec![]);
        for label in labels {
            let at = texts.iter().find(|(t, _)| t == label).map(|(_, r)| r.center());
            drawn.push(at.is_some());
            let Some(at) = at else {
                events.push(None);
                continue;
            };
            let (t, ev) = frame(click_at(at));
            events.push(ev);
            // Let a list that opened settle (its enter animation) before the next click.
            texts = t;
            for _ in 0..3 {
                texts = frame(vec![]).0;
            }
        }
        (events, drawn)
    }

    #[test]
    fn the_tabs_and_the_select_report_their_clicks() {
        use crate::settings::SettingsTab;
        let s = |e| Some(UiEvent::Settings(e));
        assert_eq!(
            click_labels(&open_settings(), &["Screen"]).0,
            [s(SettingsEvent::Tab(SettingsTab::Screen))]
        );
        assert_eq!(
            click_labels(&screen_settings(), &["Packages"]).0,
            [s(SettingsEvent::Tab(SettingsTab::Packages))]
        );
        // The open list offers the other times; a pick reports it.
        let (events, drawn) = click_labels(&screen_settings(), &["5 minutes", "10 minutes"]);
        assert_eq!(drawn, [true, true]);
        assert_eq!(events, [None, s(SettingsEvent::IdleTimeout(600))]);
    }

    #[test]
    fn the_screen_tab_shows_its_error_in_warn() {
        let mut menu = screen_settings();
        menu.screen_tab.error = Some("disk full".into());
        let shapes = settings_shapes(&menu, 1280.0, 720.0);
        let color = shapes.iter().find_map(|c| match &c.shape {
            egui::Shape::Text(t) if t.galley.text() == "disk full" => Some(t.galley.job.sections[0].format.color),
            _ => None,
        });
        assert_eq!(color, Some(WARN), "{:?}", all_texts(&shapes));
    }

    fn saved_net(ssid: &str, password: Option<&str>, hidden: bool) -> crate::netfiles::Net {
        crate::netfiles::Net {
            ssid: ssid.into(),
            hidden,
            password: password.map(String::from),
        }
    }

    fn fixed_wired() -> crate::netfiles::Wired {
        crate::netfiles::Wired::Static {
            address: "192.168.1.10".parse().unwrap(),
            prefix: 24,
            gateway: Some("192.168.1.1".parse().unwrap()),
            dns: vec!["1.1.1.1".parse().unwrap(), "9.9.9.9".parse().unwrap()],
        }
    }

    /// A Network tab with its files read: WiFi on adapter `wlp1`.
    fn net_tab(
        netboot: bool,
        wifi: (crate::netfiles::FileState, Vec<crate::netfiles::Net>, bool),
        wired: (crate::netfiles::FileState, crate::netfiles::Wired),
    ) -> crate::network::NetTab {
        let mut net = crate::network::NetTab::new(netboot);
        net.loaded(
            crate::netfiles::WifiFile {
                state: wifi.0,
                device: Some("wlp1".into()),
                networks: wifi.1,
                // In step with the wired servers, as the menu writes it.
                dns: wired.1.dns().to_vec(),
                manageable: wifi.2,
                error: None,
            },
            crate::netfiles::WiredFile {
                state: wired.0,
                wired: wired.1,
                manageable: true,
                error: None,
            },
            vec!["wlp1".into()],
        );
        net
    }

    /// A Network tab with the given file states and adapters; the menu
    /// file, when there is one, holds one saved network on `wlp1`.
    fn net_files(
        netboot: bool,
        wifi: crate::netfiles::FileState,
        wired: crate::netfiles::FileState,
        devices: &[&str],
    ) -> crate::network::NetTab {
        use crate::netfiles::FileState;
        let mut net = crate::network::NetTab::new(netboot);
        let networks = if wifi == FileState::Menu {
            vec![saved_net("Home", Some("secret123"), false)]
        } else {
            vec![]
        };
        net.loaded(
            crate::netfiles::WifiFile {
                state: wifi,
                device: (!devices.is_empty()).then(|| "wlp1".to_string()),
                networks,
                dns: Vec::new(),
                manageable: true,
                error: None,
            },
            crate::netfiles::WiredFile {
                state: wired,
                wired: crate::netfiles::Wired::dhcp(),
                manageable: true,
                error: None,
            },
            devices.iter().map(|d| d.to_string()).collect(),
        );
        net
    }

    /// Menu-made settings: two saved networks (one secured, one open and
    /// hidden) and a fixed wired address.
    fn two_networks() -> crate::network::NetTab {
        use crate::netfiles::FileState;
        net_tab(
            false,
            (
                FileState::Menu,
                vec![
                    saved_net("Home", Some("secret123"), false),
                    saved_net("Lab", None, true),
                ],
                true,
            ),
            (FileState::Menu, fixed_wired()),
        )
    }

    /// `two_networks()` with the add dialog open, the scan done.
    fn add_dialog_open() -> crate::network::NetTab {
        use crate::nettab::InRange;
        use crate::network::NetEvent;
        let mut net = two_networks();
        net.event(NetEvent::OpenAdd, false);
        let in_range = |ssid: &str, signal_dbm, secured, supported| InRange {
            ssid: ssid.into(),
            signal_dbm,
            secured,
            supported,
            name_ok: true,
        };
        net.scan_done(Ok(vec![
            in_range("Cafe", -75, false, true),
            in_range("Office", -55, true, true),
            in_range("Old Router", -65, true, false),
        ]));
        net
    }

    fn network_settings(net: crate::network::NetTab) -> Menu {
        let mut menu = open_settings();
        menu.settings_tab = crate::settings::SettingsTab::Network;
        menu.network = net;
        menu
    }

    /// The height of the screen the Network tab tests draw on: tall
    /// enough that every band fits, so none scrolls out of the test's
    /// view. With the Time band the page no longer fits on 1600 points;
    /// on 1080p and smaller screens it scrolls (see
    /// `every_network_control_scrolls_into_reach`).
    const NET_SCREEN_H: f32 = 2400.0;

    /// The Network tab's texts, drawn `NET_SCREEN_H` tall.
    fn net_texts(menu: &Menu) -> Vec<String> {
        text_rects(&settings_shapes(menu, 1920.0, NET_SCREEN_H))
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    fn net_clicks(menu: &Menu, labels: &[&str]) -> (Vec<Option<UiEvent>>, Vec<bool>) {
        click_labels_at(menu, labels, (1920.0, 1080.0))
    }

    fn has(texts: &[String], want: &str) -> bool {
        texts.iter().any(|t| t == want)
    }

    #[test]
    fn the_network_tab_lists_connections() {
        use crate::nettab::{IfKind, IfState, Iface};
        let mut net = two_networks();
        let lines = [
            Iface {
                name: "enp1s0".into(),
                kind: IfKind::Wired,
                state: IfState::Connected,
                addresses: vec!["192.168.1.10/24".into()],
                ssid: None,
                signal_dbm: None,
                handshake_age_s: None,
            },
            Iface {
                name: "wlp1".into(),
                kind: IfKind::Wifi,
                state: IfState::Connected,
                addresses: vec!["10.0.0.7/24".into()],
                ssid: Some("Home".into()),
                signal_dbm: Some(-50),
                handshake_age_s: None,
            },
        ];
        let details = crate::netdetail::Details {
            ifaces: vec![crate::netdetail::IfDetail {
                name: "wlp1".into(),
                mac: Some("3c:52:82:aa:bb:cc".into()),
                driver: Some("iwlwifi".into()),
                ..Default::default()
            }],
            dns_in_use: vec!["192.168.1.1".into()],
            ..Default::default()
        };
        net.status_done(Ok(lines.to_vec()), details);
        // The band closes the tab: a screen tall enough to show it whole.
        let tall = |menu: &Menu| -> Vec<String> {
            text_rects(&settings_shapes(menu, 1920.0, 2400.0))
                .into_iter()
                .map(|(t, _)| t)
                .collect()
        };
        let texts = tall(&network_settings(net));
        assert!(has(&texts, "Connections now"), "{texts:?}");
        // Every value beside its label, never a bare one joined by dots.
        for want in [
            "Wired enp1s0",
            "WiFi wlp1",
            "State",
            "connected",
            "Network",
            "Home",
            "Address",
            "10.0.0.7/24",
            "MAC address",
            "3c:52:82:aa:bb:cc",
            "Driver",
            "iwlwifi",
            "DNS in use",
            "192.168.1.1",
        ] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        assert!(!texts.iter().any(|t| t.contains(" · iwlwifi")), "{texts:?}");
        assert!(!has(&texts, "No network adapter found"), "{texts:?}");
        // Before the first status answer nothing is known yet.
        let texts = tall(&network_settings(two_networks()));
        assert!(!has(&texts, "DNS in use"), "{texts:?}");
        assert!(!has(&texts, "No network adapter found"), "{texts:?}");
        assert!(has(&texts, "Looking for network adapters…"), "{texts:?}");
        let mut net = two_networks();
        net.status_done(Ok(Vec::new()), Default::default());
        let texts = tall(&network_settings(net));
        assert!(has(&texts, "No network adapter found"), "{texts:?}");
    }

    #[test]
    fn the_network_tab_shows_fixed_address_fields() {
        use crate::netfiles::{FileState, Wired};
        use crate::network::NetEvent;
        let texts = net_texts(&network_settings(two_networks()));
        for want in ["Wired", "Fixed address", "Address", "Gateway", "DNS servers"] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        // The values as saved, in the inputs.
        for want in ["192.168.1.10/24", "192.168.1.1", "1.1.1.1, 9.9.9.9"] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        // DHCP: the select and the DNS servers, which replace those DHCP
        // gives when filled.
        let dhcp = || net_tab(false, (FileState::Menu, vec![], true), (FileState::Menu, Wired::dhcp()));
        let texts = net_texts(&network_settings(dhcp()));
        assert!(has(&texts, "Automatic (DHCP)"), "{texts:?}");
        assert!(has(&texts, "DNS servers"), "{texts:?}");
        assert!(!has(&texts, "Gateway"), "{texts:?}");
        assert!(has(&texts, "1.1.1.1, 9.9.9.9"), "placeholder not in {texts:?}");
        let saved = Wired::Dhcp {
            dns: vec!["8.8.8.8".parse().unwrap()],
        };
        let texts = net_texts(&network_settings(net_tab(
            false,
            (FileState::Menu, vec![], true),
            (FileState::Menu, saved),
        )));
        assert!(has(&texts, "8.8.8.8"), "{texts:?}");
        assert!(!has(&texts, WIFI_KEEPS_DNS), "{texts:?}");
        // Switched to fixed with nothing typed: the placeholders.
        let mut fresh = dhcp();
        fresh.event(NetEvent::WiredMode(true), false);
        let texts = net_texts(&network_settings(fresh));
        for want in ["192.168.1.10/24", "192.168.1.1", "1.1.1.1, 9.9.9.9"] {
            assert!(has(&texts, want), "placeholder {want} not in {texts:?}");
        }
        // The select reports the pick.
        let (events, drawn) = net_clicks(&network_settings(dhcp()), &["Automatic (DHCP)", "Fixed address"]);
        assert_eq!(drawn, [true, true]);
        assert_eq!(
            events,
            [
                None,
                Some(UiEvent::Settings(SettingsEvent::Network(NetEvent::WiredMode(true))))
            ]
        );
    }

    const WIFI_KEEPS_DNS: &str = "WiFi keeps the DNS servers from your old settings.";

    #[test]
    fn the_network_tab_says_a_hand_wifi_file_keeps_its_dns() {
        use crate::netfiles::{FileState, Wired};
        let net = net_tab(
            false,
            (FileState::Hand, vec![saved_net("Home", None, false)], true),
            (FileState::Menu, Wired::dhcp()),
        );
        let texts = net_texts(&network_settings(net));
        assert!(has(&texts, WIFI_KEEPS_DNS), "{texts:?}");
        // Not on netboot, where there is no DNS row.
        let net = net_tab(
            true,
            (FileState::Hand, vec![saved_net("Home", None, false)], true),
            (FileState::Menu, Wired::dhcp()),
        );
        let texts = net_texts(&network_settings(net));
        assert!(!has(&texts, WIFI_KEEPS_DNS), "{texts:?}");
    }

    #[test]
    fn the_network_tab_hides_fixed_on_netboot() {
        use crate::netfiles::{FileState, Wired};
        let net = net_tab(true, (FileState::Menu, vec![], true), (FileState::Menu, Wired::dhcp()));
        let texts = net_texts(&network_settings(net));
        assert!(has(&texts, "A netbooted machine always uses DHCP."), "{texts:?}");
        for gone in ["Automatic (DHCP)", "Fixed address", "Gateway", "DNS servers"] {
            assert!(!has(&texts, gone), "{gone} in {texts:?}");
        }
    }

    #[test]
    fn the_network_tab_shows_hand_wifi_read_only() {
        use crate::netfiles::{FileState, Wired};
        use crate::network::NetEvent;
        // Wired by DHCP: with a fixed address and the DNS note the bands
        // scroll on 1080p, and the button would sit under Apply.
        let hand = |manageable| {
            net_tab(
                false,
                (FileState::Hand, vec![saved_net("Home", None, false)], manageable),
                (FileState::Menu, Wired::dhcp()),
            )
        };
        let texts = net_texts(&network_settings(hand(true)));
        for want in [
            "WiFi networks",
            "Home",
            "Kept from your old settings; the menu cannot change it.",
            "Let the menu manage it",
        ] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        for gone in ["Forget", "Add network"] {
            assert!(!has(&texts, gone), "{gone} in {texts:?}");
        }
        let (events, _) = net_clicks(&network_settings(hand(true)), &["Let the menu manage it"]);
        assert_eq!(
            events,
            [Some(UiEvent::Settings(SettingsEvent::Network(NetEvent::TakeOver(
                "wifi"
            ))))]
        );
        let texts = net_texts(&network_settings(hand(false)));
        assert!(
            has(&texts, "Kept from your old settings; the menu cannot change it."),
            "{texts:?}"
        );
        assert!(!has(&texts, "Let the menu manage it"), "{texts:?}");
    }

    /// The DNS servers have a section of their own, below WiFi, in every
    /// wired mode; the wired band keeps the address alone.
    #[test]
    fn the_dns_section_stands_on_its_own() {
        use crate::netfiles::{FileState, Wired};
        use crate::network::NetEvent;
        let rect = |texts: &[(String, Rect)], want: &str| texts.iter().find(|(t, _)| t == want).map(|(_, r)| *r);
        let shapes = |net| text_rects(&settings_shapes(&network_settings(net), 1920.0, 1080.0));
        for wired in [Wired::dhcp(), fixed_wired()] {
            let texts = shapes(net_tab(
                false,
                (FileState::Menu, vec![], true),
                (FileState::Menu, wired),
            ));
            let heading = rect(&texts, "DNS").expect("DNS heading");
            let row = rect(&texts, "DNS servers").expect("DNS row");
            let wifi = rect(&texts, "WiFi networks").unwrap();
            assert!(wifi.top() < heading.top() && heading.top() < row.top(), "{texts:?}");
            assert!(rect(&texts, DNS_NOTE).is_some(), "{texts:?}");
        }
        // The row reports its own event.
        let net = net_tab(false, (FileState::Menu, vec![], true), (FileState::Menu, Wired::dhcp()));
        let mut menu = network_settings(net);
        menu.network.event(NetEvent::DnsText("8.8.8.8".into()), false);
        assert!(menu.network.dirty());
        assert!(has(&net_texts(&menu), "8.8.8.8"));
        // A kept (hand-written) wired setting: its servers, greyed, and why.
        let hand = Wired::Dhcp {
            dns: vec!["8.8.4.4".parse().unwrap()],
        };
        let texts = net_texts(&network_settings(net_tab(
            false,
            (FileState::Menu, vec![], true),
            (FileState::Hand, hand),
        )));
        for want in [
            "DNS servers",
            "8.8.4.4",
            "Kept from your old settings; the menu cannot change it.",
        ] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        // Netboot: the line alone.
        let texts = net_texts(&network_settings(net_tab(
            true,
            (FileState::Menu, vec![], true),
            (FileState::Menu, Wired::dhcp()),
        )));
        assert!(has(&texts, "DNS"), "{texts:?}");
        assert!(has(
            &texts,
            "A netbooted machine always uses the DNS servers DHCP sends."
        ));
        assert!(!has(&texts, "DNS servers") && !has(&texts, DNS_NOTE), "{texts:?}");
    }

    const DNS_NOTE: &str = "Used for wired and WiFi. Empty: the servers DHCP sends.";

    const PUBLIC_IP_NOTE: &str = "Only https:// addresses are used.";

    /// The public IP band follows DNS, takes a refused address with a
    /// note, and is drawn before the network files are read.
    #[test]
    fn the_public_ip_band_follows_dns() {
        use crate::network::NetEvent;
        let mut net = two_networks();
        let texts = net_texts(&network_settings(two_networks()));
        let at = |texts: &[String], name: &str| {
            texts
                .iter()
                .position(|t| t == name)
                .unwrap_or_else(|| panic!("{name} not in {texts:?}"))
        };
        assert!(at(&texts, "DNS") < at(&texts, "Public IP address"), "{texts:?}");
        assert!(has(&texts, "Show the public IP address"), "{texts:?}");
        assert!(has(&texts, "Lookup address"), "{texts:?}");
        assert!(has(&texts, crate::publicip::DEFAULT_URL), "the hint: {texts:?}");
        assert!(!has(&texts, PUBLIC_IP_NOTE), "{texts:?}");
        // One line: the label fits its column.
        let shapes = settings_shapes(&network_settings(two_networks()), 1920.0, NET_SCREEN_H);
        let rects = text_rects(&shapes);
        let height = |name: &str| rects.iter().find(|(t, _)| t == name).unwrap().1.height();
        assert!(height("Show the public IP address") < 1.5 * height("Lookup address"));
        net.event(
            NetEvent::PublicIpUrl {
                text: "http://ip.example.net".into(),
                commit: false,
            },
            false,
        );
        let texts = net_texts(&network_settings(net));
        assert!(has(&texts, PUBLIC_IP_NOTE), "{texts:?}");
        let texts = net_texts(&network_settings(crate::network::NetTab::new(false)));
        assert!(has(&texts, "Show the public IP address"), "{texts:?}");
    }

    /// WiFi with other servers than the wired setting: the DNS band says
    /// why Apply is on.
    #[test]
    fn the_dns_section_says_when_wifi_is_out_of_step() {
        use crate::netfiles::{FileState, Net, WifiFile, WiredFile};
        const OUT: &str = "WiFi uses other DNS servers. Apply sets the ones above.";
        let tab = |wifi_dns: Vec<std::net::Ipv4Addr>| {
            let mut net = crate::network::NetTab::new(false);
            net.loaded(
                WifiFile {
                    state: FileState::Menu,
                    device: Some("wlp1".into()),
                    networks: vec![Net {
                        ssid: "Home".into(),
                        hidden: false,
                        password: None,
                    }],
                    dns: wifi_dns,
                    manageable: true,
                    error: None,
                },
                WiredFile {
                    state: FileState::Menu,
                    wired: fixed_wired(),
                    manageable: true,
                    error: None,
                },
                vec!["wlp1".into()],
            );
            net
        };
        let net = tab(vec![]);
        assert!(net.dirty());
        let texts = net_texts(&network_settings(net));
        assert!(has(&texts, OUT), "{texts:?}");
        let texts = net_texts(&network_settings(tab(fixed_wired().dns().to_vec())));
        assert!(!has(&texts, OUT), "{texts:?}");
    }

    /// On 1080p the bands of a full Network tab outgrow the panel: a
    /// kept hand-written WiFi setting beside a fixed address, both take-over buttons and
    /// the DNS section. Every control scrolls into reach above the pinned
    /// Apply, and a click there lands.
    #[test]
    fn every_network_control_scrolls_into_reach() {
        use crate::netfiles::FileState;
        use crate::network::NetEvent;
        let menu = network_settings(net_tab(
            false,
            (FileState::Hand, vec![saved_net("Home", None, false)], true),
            (FileState::Menu, fixed_wired()),
        ));
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen(1920.0, 1080.0)),
                events,
                // Wheel scrolling is smoothed over time.
                time: Some(ctx.cumulative_pass_nr() as f64 * 0.1),
                ..Default::default()
            };
            let mut ev = None;
            let mut out = ctx.run_ui(input, |ui| {
                ev = show(ui, &menu, &[None], &Chrome::default(), Instant::now());
            });
            out.textures_delta.clear();
            (text_rects(&out.shapes), ev)
        };
        let find = |texts: &[(String, Rect)], want: &str| texts.iter().find(|(t, _)| t == want).map(|(_, r)| *r);
        let mut texts = vec![];
        for _ in 0..3 {
            texts = frame(vec![]).0;
        }
        let over = find(&texts, "Wired").expect("the Wired band").center();
        let top = find(&texts, "Network").expect("the tab bar").bottom();
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -40.0),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
            source: egui::MouseWheelSource::Wheel,
        };
        let wanted = [
            "Fixed address",
            "Gateway",
            "Let the menu manage it",
            "DNS servers",
            "Used for wired and WiFi. Empty: the servers DHCP sends.",
            "WiFi keeps the DNS servers from your old settings.",
            "Time zone",
            "NTP servers",
            "Lookup address",
        ];
        // Before any scrolling something is out of reach: the tab does not
        // fit, so the scrolling below is what the test checks.
        let in_reach = |texts: &[(String, Rect)], want: &str| {
            let apply = find(texts, "Apply").expect("Apply");
            find(texts, want).is_some_and(|r| r.top() > top && r.bottom() < apply.top())
        };
        assert!(
            !wanted.iter().all(|w| in_reach(&texts, w)),
            "the whole tab fits on 1080p: {texts:?}"
        );
        let mut seen = [false; 9];
        let mut clicked = None;
        for _ in 0..80 {
            let apply = find(&texts, "Apply").expect("Apply");
            for (i, want) in wanted.iter().enumerate() {
                if let Some(r) = find(&texts, want)
                    && r.top() > top
                    && r.bottom() < apply.top()
                {
                    seen[i] = true;
                    if *want == "Let the menu manage it" && clicked.is_none() {
                        clicked = Some(frame(click_at(r.center())).1);
                    }
                }
            }
            texts = frame(vec![egui::Event::PointerMoved(over), wheel.clone()]).0;
        }
        assert_eq!(seen, [true; 9], "{wanted:?}");
        assert_eq!(
            clicked,
            Some(Some(UiEvent::Settings(SettingsEvent::Network(NetEvent::TakeOver(
                "wifi"
            )))))
        );
    }

    #[test]
    fn the_network_tab_shows_a_hand_wired_file_read_only() {
        use crate::netfiles::FileState;
        use crate::network::NetEvent;
        let net = net_tab(false, (FileState::Menu, vec![], true), (FileState::Hand, fixed_wired()));
        let menu = network_settings(net);
        let texts = net_texts(&menu);
        for want in [
            "Fixed address",
            "192.168.1.10/24",
            "Kept from your old settings; the menu cannot change it.",
            "Let the menu manage it",
        ] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        // The select is greyed: no list opens.
        assert_eq!(
            net_clicks(&menu, &["Fixed address", "Automatic (DHCP)"]).1,
            [true, false]
        );
        assert_eq!(
            net_clicks(&menu, &["Let the menu manage it"]).0,
            [Some(UiEvent::Settings(SettingsEvent::Network(NetEvent::TakeOver(
                "wired"
            ))))]
        );
    }

    #[test]
    fn the_network_tab_reports_forget_add_and_apply() {
        use crate::network::NetEvent;
        let ev = |e| Some(UiEvent::Settings(SettingsEvent::Network(e)));
        let texts = net_texts(&network_settings(two_networks()));
        for want in ["Home", "Lab", "Hidden", "Add network", "Apply"] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        let (events, _) = net_clicks(&network_settings(two_networks()), &["Forget"]);
        assert_eq!(events, [ev(NetEvent::Forget("Home".into()))]);
        let (events, _) = net_clicks(&network_settings(two_networks()), &["Add network"]);
        assert_eq!(events, [ev(NetEvent::OpenAdd)]);
        // Nothing changed: Apply does nothing.
        let (events, _) = net_clicks(&network_settings(two_networks()), &["Apply"]);
        assert_eq!(events, [None]);
        let mut net = two_networks();
        net.event(NetEvent::Forget("Lab".into()), false);
        let menu = network_settings(net);
        let (events, _) = net_clicks(&menu, &["Apply"]);
        assert_eq!(events, [ev(NetEvent::Apply)]);
        // On a 720p screen the bands scroll; Apply stays in view.
        assert_eq!(click_labels(&menu, &["Apply"]).0, [ev(NetEvent::Apply)]);
        // No adapter: Add network is greyed and says why.
        let mut net = crate::network::NetTab::new(false);
        net.loaded(
            crate::netfiles::WifiFile {
                state: crate::netfiles::FileState::Missing,
                device: None,
                networks: vec![],
                dns: Vec::new(),
                manageable: true,
                error: None,
            },
            crate::netfiles::WiredFile {
                state: crate::netfiles::FileState::Missing,
                wired: crate::netfiles::Wired::dhcp(),
                manageable: true,
                error: None,
            },
            vec![],
        );
        let menu = network_settings(net);
        assert!(has(&net_texts(&menu), "No WiFi adapter found"));
        assert_eq!(net_clicks(&menu, &["Add network"]).0, [None]);
    }

    /// The fonts have no emoji: a lock drawn as text came out as a broken
    /// glyph on hardware. The marks are icons; the rows give the strength
    /// as a word and in dBm.
    #[test]
    fn network_rows_draw_no_emoji_and_give_the_dbm() {
        let saved = net_texts(&network_settings(two_networks()));
        let dialog = net_texts(&network_settings(add_dialog_open()));
        for t in saved.iter().chain(&dialog) {
            assert!(!t.chars().any(|c| u32::from(c) >= 0x1f000), "emoji in {t:?}");
        }
        assert!(has(&dialog, "strong · -55 dBm"), "{dialog:?}");
        assert!(has(&dialog, "weak · -75 dBm"), "{dialog:?}");
    }

    /// Apply sits below the bands, as the Upgrade screen's footer, and is
    /// pinned to the panel's foot only when the bands scroll.
    #[test]
    fn apply_follows_the_bands_on_a_tall_screen() {
        let shapes = settings_shapes(&network_settings(two_networks()), 1920.0, 2400.0);
        // "Connections now" is the last band.
        let band = card_of(&shapes, "Connections now");
        let apply = text_rects(&shapes)
            .into_iter()
            .rfind(|(t, _)| t == "Apply")
            .expect("Apply drawn")
            .1;
        let gap = apply.top() - band.bottom();
        assert!(gap > 0.0 && gap < 3.0 * GAP, "Apply {apply:?}, band {band:?}");
    }

    /// Ruling 21: one reason per cause on an unsupported row.
    #[test]
    fn unsupported_rows_say_why() {
        use crate::network::NetEvent;
        let mut net = two_networks();
        net.event(NetEvent::OpenAdd, false);
        let scan = r#"{"device":"w","networks":[
            {"ssid":"back\\slash","signal_dbm":-50,"secured":true,"supported":true},
            {"ssid":"Old Router","signal_dbm":-60,"secured":true,"supported":false},
            {"ssid":"Cafe","signal_dbm":-70,"secured":false,"supported":true}]}"#;
        net.scan_done(crate::nettab::parse_scan(scan));
        let texts = net_texts(&network_settings(net));
        let at = |name: &str| {
            texts
                .iter()
                .position(|t| t == name)
                .unwrap_or_else(|| panic!("{name} not in {texts:?}"))
        };
        assert_eq!(texts[at("back\\slash") + 1], "This name cannot be used.", "{texts:?}");
        assert_eq!(
            texts[at("Old Router") + 1],
            "This security type is not supported",
            "{texts:?}"
        );
        assert!(!has(&texts, "WEP is not supported"), "{texts:?}");
    }

    #[test]
    fn the_add_dialog_shows_the_password_note() {
        use crate::network::NetEvent;
        let note = "The password is saved with the home directory. Anyone who has the stick can read it.";
        let texts = net_texts(&network_settings(add_dialog_open()));
        for want in [
            "Office",
            "Cafe",
            "Old Router",
            "This security type is not supported",
            "Scan again",
            "Hidden network",
            "Password",
            note,
            "Add",
            "Cancel",
        ] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        // Strongest first.
        let at = |name: &str| texts.iter().position(|t| t == name).unwrap();
        assert!(
            at("Office") < at("Old Router") && at("Old Router") < at("Cafe"),
            "{texts:?}"
        );
        assert!(!has(&texts, "Network name"), "{texts:?}");
        // A picked open network asks for no password.
        let mut net = add_dialog_open();
        net.event(NetEvent::PickNetwork("Cafe".into()), false);
        let texts = net_texts(&network_settings(net));
        assert!(!has(&texts, "Password") && !has(&texts, note), "{texts:?}");
        // Hidden: the name is typed.
        let mut net = add_dialog_open();
        net.event(
            NetEvent::AddText {
                ssid: String::new(),
                password: String::new(),
                hidden: true,
            },
            false,
        );
        assert!(has(&net_texts(&network_settings(net)), "Network name"));
        // While the scan runs.
        let mut net = two_networks();
        net.event(NetEvent::OpenAdd, false);
        assert!(has(&net_texts(&network_settings(net)), "Looking for networks…"));
        // A click on a network picks it; one the menu cannot join does nothing.
        let ev = |e| Some(UiEvent::Settings(SettingsEvent::Network(e)));
        let menu = network_settings(add_dialog_open());
        assert_eq!(
            net_clicks(&menu, &["Office"]).0,
            [ev(NetEvent::PickNetwork("Office".into()))]
        );
        assert_eq!(net_clicks(&menu, &["Old Router"]).0, [None]);
        assert_eq!(net_clicks(&menu, &["Cancel"]).0, [ev(NetEvent::CloseAdd)]);
        assert_eq!(net_clicks(&menu, &["Scan again"]).0, [ev(NetEvent::Rescan)]);
        // The tab under the dialog takes no clicks.
        assert_eq!(net_clicks(&menu, &["Forget"]).0, [None]);
    }

    /// The shapes of `menu` at 1080p once the dialog's fade-in is over
    /// (no animation, as in the app), so its colours are the final ones.
    fn dialog_shapes(menu: &Menu) -> Vec<egui::epaint::ClippedShape> {
        upgrade_frame(menu, 1920.0, 1080.0, vec![]).0
    }

    /// The rects drawn with an ice border around the text `label`: the
    /// mark of a chosen row.
    fn ice_frames_around(shapes: &[egui::epaint::ClippedShape], label: &str) -> usize {
        let Some(at) = text_rects(shapes)
            .into_iter()
            .find(|(t, _)| t == label)
            .map(|(_, r)| r.center())
        else {
            panic!("{label} not drawn");
        };
        shapes
            .iter()
            .flat_map(|c| flat_rects(&c.shape))
            .filter(|r| r.stroke.color == ICE && r.stroke.width > 0.0 && r.rect.contains(at))
            .count()
    }

    #[test]
    fn the_add_dialog_shows_the_name_it_adds() {
        use crate::network::NetEvent;
        let ev = |e| Some(UiEvent::Settings(SettingsEvent::Network(e)));
        // Nothing picked, no name typed: Add is greyed and no row is chosen.
        let menu = network_settings(add_dialog_open());
        assert_eq!(net_clicks(&menu, &["Add"]).0, [None]);
        let shapes = dialog_shapes(&menu);
        assert_eq!(ice_frames_around(&shapes, "Office"), 0);
        // Picked: the row is drawn chosen, and Add acts.
        let mut net = add_dialog_open();
        net.event(NetEvent::PickNetwork("Office".into()), false);
        let menu = network_settings(net);
        let shapes = dialog_shapes(&menu);
        assert!(ice_frames_around(&shapes, "Office") > 0, "Office not drawn as chosen");
        assert_eq!(ice_frames_around(&shapes, "Cafe"), 0);
        assert_eq!(net_clicks(&menu, &["Add"]).0, [ev(NetEvent::Add)]);
        // Hidden ticked after the pick: the name is in the name field.
        let mut net = add_dialog_open();
        net.event(NetEvent::PickNetwork("Office".into()), false);
        net.event(
            NetEvent::AddText {
                ssid: "Office".into(),
                password: String::new(),
                hidden: true,
            },
            false,
        );
        let texts = net_texts(&network_settings(net));
        let field = texts.iter().position(|t| t == "Network name").expect("the name field");
        assert!(texts[field..].iter().any(|t| t == "Office"), "{texts:?}");
        // Unticked again: no row chosen, no name field, Add greyed.
        let mut net = add_dialog_open();
        net.event(NetEvent::PickNetwork("Office".into()), false);
        for hidden in [true, false] {
            net.event(
                NetEvent::AddText {
                    ssid: "Office".into(),
                    password: String::new(),
                    hidden,
                },
                false,
            );
        }
        let menu = network_settings(net);
        let shapes = dialog_shapes(&menu);
        assert_eq!(ice_frames_around(&shapes, "Office"), 0);
        assert!(!has(&net_texts(&menu), "Network name"));
        assert_eq!(net_clicks(&menu, &["Add"]).0, [None]);
    }

    #[test]
    fn a_picked_name_no_row_shows_is_in_the_name_field() {
        use crate::nettab::InRange;
        use crate::network::NetEvent;
        let ev = |e| Some(UiEvent::Settings(SettingsEvent::Network(e)));
        let mut net = office_picked_rescanning();
        // While the scan runs the list is not drawn: the name is in the field.
        let texts = net_texts(&network_settings(office_picked_rescanning()));
        let field = texts
            .iter()
            .position(|t| t == "Network name")
            .expect("the name field while scanning");
        assert!(texts[field..].iter().any(|t| t == "Office"), "{texts:?}");
        // Scan again, and Office is gone from the new list.
        net.scan_done(Ok(vec![InRange {
            ssid: "Cafe".into(),
            signal_dbm: -60,
            secured: false,
            supported: true,
            name_ok: true,
        }]));
        let menu = network_settings(net);
        let texts = net_texts(&menu);
        let field = texts.iter().position(|t| t == "Network name").expect("the name field");
        assert!(texts[field..].iter().any(|t| t == "Office"), "{texts:?}");
        assert_eq!(ice_frames_around(&dialog_shapes(&menu), "Cafe"), 0);
        // Add acts on that shown name.
        assert_eq!(net_clicks(&menu, &["Add"]).0, [ev(NetEvent::Add)]);
    }

    /// `add_dialog_open()` with Office picked and a rescan running.
    fn office_picked_rescanning() -> crate::network::NetTab {
        use crate::network::NetEvent;
        let mut net = add_dialog_open();
        net.event(NetEvent::PickNetwork("Office".into()), false);
        net.event(NetEvent::Rescan, false);
        net
    }

    #[test]
    fn the_apply_confirmation_names_the_remote_session() {
        use crate::network::NetEvent;
        let mut net = two_networks();
        net.event(NetEvent::Forget("Lab".into()), false);
        net.event(NetEvent::Apply, true);
        assert!(net.confirm_open());
        let menu = network_settings(net);
        let texts = net_texts(&menu);
        for want in [
            "The remote session may disconnect for a few seconds.",
            "Apply",
            "Cancel",
        ] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        let ev = |e| Some(UiEvent::Settings(SettingsEvent::Network(e)));
        assert_eq!(net_clicks(&menu, &["Cancel"]).0, [ev(NetEvent::CancelApply)]);
    }

    /// Frames of the Network tab of `menu` on one context, `NET_SCREEN_H`
    /// tall; each gets its events and reports its event.
    fn net_frames(menu: &Menu, frames: Vec<Vec<egui::Event>>) -> (Vec<(String, Rect)>, Vec<Option<UiEvent>>) {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        ctx.global_style_mut(|s| s.animation_time = 0.0);
        let mut texts = vec![];
        let mut events = vec![];
        for (i, input) in [vec![], vec![], vec![]].into_iter().chain(frames).enumerate() {
            let input = egui::RawInput {
                screen_rect: Some(screen(1920.0, NET_SCREEN_H)),
                events: input,
                time: Some(i as f64 * 0.1),
                ..Default::default()
            };
            let mut ev = None;
            let mut o = ctx.run_ui(input, |ui| {
                ev = show(ui, menu, &[None], &Chrome::default(), Instant::now());
            });
            o.textures_delta.clear();
            if texts.is_empty() {
                texts = text_rects(&o.shapes);
            }
            events.push(ev);
        }
        (texts, events.split_off(3))
    }

    /// `net_frames` on a menu that takes each Settings event, as the app
    /// does; the effects it answers with.
    fn net_frames_live(menu: &mut Menu, frames: Vec<Vec<egui::Event>>) -> Vec<Option<crate::menu::Effect>> {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        ctx.global_style_mut(|s| s.animation_time = 0.0);
        let mut effects = vec![];
        for (i, input) in frames.into_iter().enumerate() {
            let input = egui::RawInput {
                screen_rect: Some(screen(1920.0, NET_SCREEN_H)),
                events: input,
                time: Some(i as f64 * 0.1),
                ..Default::default()
            };
            let mut ev = None;
            let mut o = ctx.run_ui(input, |ui| {
                ev = show(ui, menu, &[None], &Chrome::default(), Instant::now());
            });
            o.textures_delta.clear();
            effects.push(match ev {
                Some(UiEvent::Settings(e)) => menu.settings(e),
                _ => None,
            });
        }
        effects
    }

    /// The Apply click that takes the focus from the lookup address field
    /// also clears the Apply error in the footer: the next frame's first
    /// pass is thrown away for the new height, and the field's commit
    /// still arrives.
    #[test]
    fn the_lookup_address_commit_survives_a_thrown_away_pass() {
        use crate::menu::{Effect, Job};
        use crate::network::NetEvent;
        let typed = "https://ip.example.net";
        let mut net = two_networks();
        net.event(NetEvent::Forget("Lab".into()), false);
        assert!(net.event(NetEvent::Apply, false).is_some());
        net.apply_done(Err(crate::jobs::ApplyFailed::new("no link".into())));
        net.event(
            NetEvent::PublicIpUrl {
                text: typed.into(),
                commit: false,
            },
            false,
        );
        let mut menu = network_settings(net);
        let (texts, _) = net_frames(&menu, vec![]);
        assert!(texts.iter().any(|(t, _)| t == "no link"), "{texts:?}");
        let at = |name: &str| texts.iter().find(|(t, _)| t == name).unwrap().1.center();
        let (field, apply) = (at(typed), at("Apply"));
        let effects = net_frames_live(
            &mut menu,
            vec![vec![], vec![], vec![], click_at(field), click_at(apply), vec![], vec![]],
        );
        assert!(
            matches!(effects[4], Some(Effect::Start(Job::NetApply { .. }))),
            "{effects:?}"
        );
        assert_eq!(menu.network.error(), None, "the footer lost its error line");
        assert_eq!(
            effects[5..],
            [
                Some(Effect::Start(Job::PublicIpSave(crate::publicip::PublicIpPrefs {
                    on: false,
                    url: typed.into(),
                }))),
                None,
            ]
        );
    }

    /// A lookup address commit kept from the frame before (it met an
    /// Apply click) and a tab click on one frame: the tab click carries it.
    /// (Text typed on the frame of a click never reaches the field: egui
    /// moves the focus first.)
    #[test]
    fn a_tab_click_carries_the_kept_lookup_address_event() {
        use crate::network::NetEvent;
        let typed = "https://ip.example.net";
        let mut net = two_networks();
        net.event(NetEvent::Forget("Lab".into()), false);
        net.event(
            NetEvent::PublicIpUrl {
                text: typed.into(),
                commit: false,
            },
            false,
        );
        let menu = network_settings(net);
        let (texts, _) = net_frames(&menu, vec![]);
        let at = |name: &str| texts.iter().find(|(t, _)| t == name).unwrap().1.center();
        let (field, apply, tab) = (at(typed), at("Apply"), at("Screen"));
        let (_, events) = net_frames(&menu, vec![click_at(field), click_at(apply), click_at(tab)]);
        assert_eq!(
            events[1..],
            [
                Some(UiEvent::Settings(SettingsEvent::Network(NetEvent::Apply))),
                Some(UiEvent::Settings(SettingsEvent::TabAfterUrl(
                    SettingsTab::Screen,
                    NetEvent::PublicIpUrl {
                        text: typed.into(),
                        commit: true,
                    }
                ))),
            ]
        );
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// A click on Apply that takes the focus from the lookup address field
    /// is reported, and the field's commit follows on the next frame.
    #[test]
    fn apply_and_the_lookup_address_commit_of_one_frame_both_arrive() {
        use crate::network::NetEvent;
        let mut net = two_networks();
        net.event(NetEvent::Forget("Lab".into()), false);
        let typed = "https://ip.example.net";
        net.event(
            NetEvent::PublicIpUrl {
                text: typed.into(),
                commit: false,
            },
            false,
        );
        let menu = network_settings(net);
        let (texts, _) = net_frames(&menu, vec![]);
        let at = |name: &str| texts.iter().find(|(t, _)| t == name).unwrap().1.center();
        let (field, apply) = (at(typed), at("Apply"));
        let (_, events) = net_frames(&menu, vec![click_at(field), click_at(apply), vec![], vec![]]);
        let ev = |e| Some(UiEvent::Settings(SettingsEvent::Network(e)));
        assert_eq!(
            events,
            [
                None,
                ev(NetEvent::Apply),
                ev(NetEvent::PublicIpUrl {
                    text: typed.into(),
                    commit: true,
                }),
                None,
            ]
        );
    }

    /// The Time band sits between DNS and the public IP band, and its NTP
    /// field commits like the lookup address.
    #[test]
    fn the_time_band_follows_dns() {
        let texts = net_texts(&network_settings(two_networks()));
        let at = |name: &str| {
            texts
                .iter()
                .position(|t| t == name)
                .unwrap_or_else(|| panic!("{name} not in {texts:?}"))
        };
        assert!(
            at("DNS") < at("Time") && at("Time") < at("Public IP address"),
            "{texts:?}"
        );
        for t in [
            "Time zone",
            "Automatic",
            "NTP servers",
            "Empty: the servers DHCP sends and the Ubuntu pool.",
        ] {
            assert!(has(&texts, t), "{t}: {texts:?}");
        }
    }

    /// The FloeDrop band sits between DNS and Time; its status and folder
    /// show only while the switch is on.
    #[test]
    fn the_floedrop_band_follows_dns() {
        use crate::share_state::Head;
        let texts = net_texts(&network_settings(two_networks()));
        let at = |name: &str| {
            texts
                .iter()
                .position(|t| t == name)
                .unwrap_or_else(|| panic!("{name} not in {texts:?}"))
        };
        assert!(at("DNS") < at("FloeDrop") && at("FloeDrop") < at("Time"), "{texts:?}");
        assert!(has(&texts, crate::share_state::SWITCH_TEXT), "{texts:?}");
        assert!(has(&texts, crate::share_state::NAME_LABEL), "{texts:?}");
        assert!(!has(&texts, "Folder: /media/tfuser/mediashare"), "off: {texts:?}");
        let mut net = two_networks();
        net.share_mut().set_host("thinfloe-56db68".into());
        let start = std::time::SystemTime::now();
        net.share_mut().set_cfg(crate::config::ShareCfg {
            enabled: true,
            name: String::new(),
        });
        net.share_mut().set_daemon(Some(start), None);
        net.share_mut().set_file(Some((
            Head::Ready {
                iface: "wlx1".into(),
                mode: "exclusive".into(),
                channel: 6,
                flags: vec!["no-ble".into()],
            },
            start,
        )));
        let texts = net_texts(&network_settings(net));
        for t in [
            "Ready on wlx1 (exclusive, channel 6). Phones must be set to AirDrop 'Everyone for 10 Minutes'.",
            "Sending needs Bluetooth; this machine has none.",
            "Folder: /media/tfuser/mediashare",
            "FloeDrop 56db68",
        ] {
            assert!(has(&texts, t), "{t}: {texts:?}");
        }
    }

    /// A saved zone the image does not list is named in the band.
    #[test]
    fn the_time_band_names_an_unknown_zone() {
        let mut net = two_networks();
        net.time_mut()
            .set_zones(crate::timecfg::parse_zone_tab("CH\t+0\tEurope/Zurich\n"));
        net.time_mut().set_cfg(crate::config::TimeCfg {
            zone: Some("Europe/Gone".into()),
            ntp: Vec::new(),
        });
        let texts = net_texts(&network_settings(net));
        assert!(has(&texts, "Automatic"), "{texts:?}");
        assert!(
            has(
                &texts,
                "The saved time zone Europe/Gone is not known here; the automatic zone is used."
            ),
            "{texts:?}"
        );
    }

    /// A region picked over an unknown zone shows in the region select.
    #[test]
    fn a_region_picked_over_an_unknown_zone_shows() {
        use crate::timeband::TimeEvent;
        let mut net = two_networks();
        net.time_mut()
            .set_zones(crate::timecfg::parse_zone_tab("CH\t+0\tEurope/Zurich\n"));
        net.time_mut().set_cfg(crate::config::TimeCfg {
            zone: Some("Europe/Gone".into()),
            ntp: Vec::new(),
        });
        assert_eq!(net.time_mut().event(TimeEvent::Region(2)), None);
        let texts = net_texts(&network_settings(net));
        assert!(has(&texts, "Europe"), "{texts:?}");
        assert!(has(&texts, "Choose a city"), "{texts:?}");
        assert!(!has(&texts, "Automatic"), "{texts:?}");
    }

    /// Over an unknown zone the region select shows Automatic, and picking
    /// Automatic in it is a change the band can save.
    #[test]
    fn automatic_can_be_picked_over_an_unknown_zone() {
        use crate::network::NetEvent;
        use crate::timeband::TimeEvent;
        let mut net = two_networks();
        net.time_mut()
            .set_zones(crate::timecfg::parse_zone_tab("CH\t+0\tEurope/Zurich\n"));
        net.time_mut().set_cfg(crate::config::TimeCfg {
            zone: Some("Europe/Gone".into()),
            ntp: Vec::new(),
        });
        let menu = network_settings(net);
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        ctx.global_style_mut(|s| s.animation_time = 0.0);
        let mut i = 0;
        let mut frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen(1920.0, NET_SCREEN_H)),
                events,
                time: Some(i as f64 * 0.1),
                ..Default::default()
            };
            i += 1;
            let mut ev = None;
            let mut o = ctx.run_ui(input, |ui| {
                ev = show(ui, &menu, &[None], &Chrome::default(), Instant::now());
            });
            o.textures_delta.clear();
            (text_rects(&o.shapes), ev)
        };
        let mut texts = vec![];
        for _ in 0..3 {
            texts = frame(vec![]).0;
        }
        let autos = |t: &[(String, Rect)]| -> Vec<Rect> {
            t.iter().filter(|(s, _)| s == "Automatic").map(|(_, r)| *r).collect()
        };
        let button = autos(&texts)[0].center();
        frame(click_at(button));
        let mut open = vec![];
        for _ in 0..2 {
            open = frame(vec![]).0;
        }
        let items = autos(&open);
        assert!(items.len() >= 2, "no popup: {open:?}");
        let item = items
            .iter()
            .find(|r| (r.center() - button).length() > 1.0)
            .unwrap()
            .center();
        let mut events = vec![frame(click_at(item)).1];
        events.push(frame(vec![]).1);
        assert!(
            events.contains(&Some(UiEvent::Settings(SettingsEvent::Network(NetEvent::Time(
                TimeEvent::Region(0)
            ))))),
            "{events:?}"
        );
    }

    /// The note on an empty server field goes once servers are typed.
    #[test]
    fn the_empty_servers_note_only_on_an_empty_field() {
        use crate::network::NetEvent;
        use crate::timeband::TimeEvent;
        let mut net = two_networks();
        net.event(
            NetEvent::Time(TimeEvent::NtpText {
                text: "ntp.example.ch".into(),
                commit: false,
            }),
            false,
        );
        let texts = net_texts(&network_settings(net));
        assert!(has(&texts, "ntp.example.ch"), "{texts:?}");
        assert!(
            !has(&texts, "Empty: the servers DHCP sends and the Ubuntu pool."),
            "{texts:?}"
        );
    }

    /// The clock line follows a status answer.
    #[test]
    fn the_time_band_shows_the_clock() {
        let mut net = two_networks();
        let details = crate::netdetail::Details {
            time: crate::timetab::TimeStatus {
                zone: "UTC".into(),
                answered: true,
                ..Default::default()
            },
            ..Default::default()
        };
        net.status_done(Ok(Vec::new()), details);
        let texts = net_texts(&network_settings(net));
        assert!(has(&texts, "Not synchronised yet."), "{texts:?}");
        assert!(has(&texts, "UTC: DHCP sends no time zone."), "{texts:?}");
        assert!(has(&texts, "Servers: none."), "{texts:?}");
    }

    /// Typing servers, then clicking the public IP switch: the switch is
    /// reported, and the field's commit on the next frame, as with Apply
    /// and the lookup address.
    #[test]
    fn ntp_field_and_switch_in_one_frame() {
        use crate::network::NetEvent;
        use crate::timeband::TimeEvent;
        let mut net = two_networks();
        net.event(
            NetEvent::Time(TimeEvent::NtpText {
                text: "ntp.example.ch".into(),
                commit: false,
            }),
            false,
        );
        let menu = network_settings(net);
        let (texts, _) = net_frames(&menu, vec![]);
        let at = |name: &str| texts.iter().find(|(t, _)| t == name).unwrap().1;
        let field = at("ntp.example.ch").center();
        // The switch sits in the value column, where the lookup address
        // field below it starts, on the line of its label.
        let label = at("Show the public IP address");
        let column = at(crate::publicip::DEFAULT_URL).left();
        let switch = egui::pos2(column + 4.0, label.center().y);
        let (_, events) = net_frames(&menu, vec![click_at(field), click_at(switch), vec![], vec![]]);
        let ev = |e| Some(UiEvent::Settings(SettingsEvent::Network(e)));
        assert_eq!(
            events,
            [
                None,
                ev(NetEvent::PublicIp(true)),
                ev(NetEvent::Time(TimeEvent::NtpText {
                    text: "ntp.example.ch".into(),
                    commit: true,
                })),
                None,
            ]
        );
    }

    /// Text and Enter in one frame: one event carries both.
    #[test]
    fn typing_and_enter_in_one_frame_commit_the_text() {
        use crate::network::NetEvent;
        let menu = network_settings(two_networks());
        let (texts, _) = net_frames(&menu, vec![]);
        let field = texts
            .iter()
            .find(|(t, _)| t == crate::publicip::DEFAULT_URL)
            .unwrap()
            .1
            .center();
        let typed = vec![
            egui::Event::Text("https://ip.example.net".into()),
            key(egui::Key::Enter),
        ];
        let (_, events) = net_frames(&menu, vec![click_at(field), typed]);
        assert_eq!(
            events,
            [
                None,
                Some(UiEvent::Settings(SettingsEvent::Network(NetEvent::PublicIpUrl {
                    text: "https://ip.example.net".into(),
                    commit: true,
                })))
            ]
        );
    }

    /// Escape in the lookup address field cancels it.
    #[test]
    fn escape_in_the_lookup_address_cancels_it() {
        use crate::network::NetEvent;
        let menu = network_settings(two_networks());
        let (texts, _) = net_frames(&menu, vec![]);
        let field = texts
            .iter()
            .find(|(t, _)| t == crate::publicip::DEFAULT_URL)
            .unwrap()
            .1
            .center();
        let (_, events) = net_frames(&menu, vec![click_at(field), vec![key(egui::Key::Escape)]]);
        assert_eq!(
            events,
            [
                None,
                Some(UiEvent::Settings(SettingsEvent::Network(NetEvent::PublicIpUrlCancel)))
            ]
        );
    }

    /// Escape closes a Network dialog as Cancel does.
    #[test]
    fn escape_closes_the_network_dialogs() {
        use crate::network::NetEvent;
        let escape = || {
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]
        };
        let ev = |e| Some(UiEvent::Settings(SettingsEvent::Network(e)));
        let menu = network_settings(add_dialog_open());
        assert_eq!(upgrade_frame(&menu, 1920.0, 1080.0, escape()).1, ev(NetEvent::CloseAdd));
        let mut net = two_networks();
        net.event(NetEvent::Forget("Lab".into()), false);
        net.event(NetEvent::Apply, true);
        let menu = network_settings(net);
        assert_eq!(
            upgrade_frame(&menu, 1920.0, 1080.0, escape()).1,
            ev(NetEvent::CancelApply)
        );
        // Without a dialog, Escape does nothing on the tab.
        let menu = network_settings(two_networks());
        assert_eq!(upgrade_frame(&menu, 1920.0, 1080.0, escape()).1, None);
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
        let p = *Theme::current(&ctx).palette();
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

    // The Upgrade screen. The menu is driven through its own events, so the
    // screen draws what the state machine holds after real job results.

    const IMAGE: u64 = 3_150_000_000;

    fn drive(device: &str, model: &str, size: u64) -> crate::drives::Drive {
        crate::drives::Drive {
            device: device.into(),
            model: model.into(),
            size_bytes: size,
            serial: Some(format!("SN{device}")),
            transport: Some("usb".into()),
            removable: true,
            mounted: false,
            automounted: false,
            is_boot: false,
            is_thinfloe: false,
            has_tl_old: false,
            has_tl_new: false,
            examined: true,
            id: format!("sn-{device}:{size}"),
            fits: Some(true),
            version: None,
        }
    }

    /// The boot stick, a second ThinFloe stick and a stick too small.
    fn three_drives() -> Vec<crate::drives::Drive> {
        let boot = crate::drives::Drive {
            is_boot: true,
            mounted: true,
            is_thinfloe: true,
            has_tl_old: true,
            version: Some("4.0.0".into()),
            ..drive("/dev/sda", "Kingston DataTraveler", 32_000_000_000)
        };
        let other = crate::drives::Drive {
            is_thinfloe: true,
            version: Some("3.9.2".into()),
            ..drive("/dev/sdb", "SanDisk Cruzer", 16_008_609_792)
        };
        let small = crate::drives::Drive {
            fits: Some(false),
            ..drive("/dev/sdc", "Intenso Rainbow", 2_000_000_000)
        };
        vec![boot, other, small]
    }

    fn internal_disk() -> crate::drives::Drive {
        crate::drives::Drive {
            transport: Some("nvme".into()),
            removable: false,
            ..drive("/dev/nvme0n1", "Samsung SSD 980", 500_107_862_016)
        }
    }

    /// The Upgrade screen open with `release` checked and `drives` listed.
    fn upgrade_open(
        release: crate::release::Release,
        drives: Vec<crate::drives::Drive>,
        all: bool,
        now: Instant,
    ) -> Menu {
        use crate::jobs::JobResult;
        let mut menu = Menu::new(shipped_grid(), false);
        menu.set_usb_boot(true);
        menu.set_installed(None);
        let tile = menu.tiles.iter().position(|t| t.action == Action::Upgrade).unwrap();
        menu.activate(tile, now);
        assert_eq!(menu.screen, Screen::Upgrade);
        menu.finished(
            JobResult::Release {
                custom: false,
                result: Ok(release),
            },
            now,
        );
        if all {
            menu.upgrade_event(UpgradeEvent::Advanced(true), now);
            menu.upgrade_event(UpgradeEvent::ShowAll(true), now);
        }
        menu.tick(now);
        menu.finished(
            JobResult::Drives {
                all,
                result: Ok(drives),
            },
            now,
        );
        menu
    }

    fn unknown_release() -> crate::release::Release {
        crate::release::Release {
            latest: None,
            image_bytes: Some(IMAGE),
            ..Default::default()
        }
    }

    /// The second stick chosen, to be erased.
    fn erase_chosen() -> Menu {
        let now = Instant::now();
        let mut menu = upgrade_open(unknown_release(), three_drives(), false, now);
        menu.upgrade_event(UpgradeEvent::Choose("/dev/sdb".into()), now);
        menu.upgrade_event(UpgradeEvent::Mode(crate::drives::Mode::Fresh), now);
        assert!(menu.upgrade.can_confirm());
        menu
    }

    /// A fresh write of the second stick, in `write` at `done` of `total`,
    /// with a rate of 12 MB/s over ten seconds.
    fn writing_at(done: u64, total: Option<u64>) -> Menu {
        use crate::jobs::JobResult;
        use crate::upgrade_proto::Msg;
        let now = Instant::now();
        let mut menu = erase_chosen();
        menu.upgrade_event(UpgradeEvent::Confirm, now);
        assert!(menu.upgrade.writing());
        menu.finished(JobResult::UpgradeStarted { pgid: 4242 }, now);
        menu.finished(
            JobResult::UpgradeMsg(Msg::Step {
                id: "write".into(),
                text: "Downloading and writing".into(),
            }),
            now,
        );
        menu.finished(JobResult::UpgradeMsg(Msg::Touched), now);
        let msg = |d| JobResult::UpgradeMsg(Msg::Progress { done: d, total });
        menu.finished(msg(done - 120_000_000), now);
        menu.finished(msg(done), now + Duration::from_secs(10));
        menu
    }

    /// One frame pair of `show` on a `w` x `h` screen, the second with `events`.
    fn upgrade_frame(
        menu: &Menu,
        w: f32,
        h: f32,
        events: Vec<egui::Event>,
    ) -> (Vec<egui::epaint::ClippedShape>, Option<UiEvent>) {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        // As the app: no animation, a bar is where its value puts it.
        ctx.global_style_mut(|s| s.animation_time = 0.0);
        let icons: Vec<Option<Icon>> = menu.tiles.iter().map(|_| None).collect();
        let mut shapes = vec![];
        let mut ev = None;
        for (i, events) in [vec![], vec![], events].into_iter().enumerate() {
            let input = egui::RawInput {
                screen_rect: Some(screen(w, h)),
                events,
                time: Some(i as f64 * 0.1),
                ..Default::default()
            };
            let mut o = ctx.run_ui(input, |ui| {
                ev = show(ui, menu, &icons, &Chrome::default(), Instant::now());
            });
            o.textures_delta.clear();
            shapes = o.shapes;
        }
        (shapes, ev)
    }

    fn upgrade_shapes(menu: &Menu, w: f32, h: f32) -> Vec<egui::epaint::ClippedShape> {
        upgrade_frame(menu, w, h, vec![]).0
    }

    fn assert_texts(shapes: &[egui::epaint::ClippedShape], wants: &[&str]) {
        let texts = all_texts(shapes);
        for want in wants {
            assert!(texts.iter().any(|t| t.contains(want)), "{want:?} not in {texts:?}");
        }
    }

    #[test]
    fn the_choose_stage_draws_release_target_and_summary() {
        let shapes = upgrade_shapes(&erase_chosen(), 1280.0, 2000.0);
        assert_texts(
            &shapes,
            &[
                "Upgrade",
                "version unknown",
                "this machine's stick",
                "too small (needs 3.2 GB)",
                "Erase SanDisk Cruzer, 16 GB, and install the newest ThinFloe",
                "Upgrade (A/B, active after a restart)",
                "Erase and install",
                "Copy this machine's home directory",
                "Check the stick after writing",
            ],
        );
    }

    #[test]
    fn an_invalid_address_says_so_beside_the_release() {
        let now = Instant::now();
        let mut menu = upgrade_open(unknown_release(), three_drives(), false, now);
        menu.upgrade_event(UpgradeEvent::Advanced(true), now);
        menu.upgrade_event(UpgradeEvent::Source("ftp://x".into()), now);
        let texts = all_texts(&upgrade_shapes(&menu, 1280.0, 2000.0));
        assert!(texts.iter().any(|t| t == "· address not valid"), "{texts:?}");
        assert!(!texts.iter().any(|t| t.contains("not checked yet")), "{texts:?}");
    }

    /// The fill of a destructive button's face, from a button drawn alone.
    fn destructive_fill() -> Color32 {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let mut o = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.add(Button::new("x").variant(ButtonVariant::Destructive));
        });
        o.textures_delta.clear();
        let label = text_rects(&o.shapes)[0].1;
        o.shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.rect.contains_rect(label) && r.fill != Color32::TRANSPARENT => Some(r.fill),
                _ => None,
            })
            .expect("a destructive face")
    }

    fn face_of(shapes: &[egui::epaint::ClippedShape], label: &str) -> Color32 {
        // The last one drawn: the screen's title can read like a button.
        let rect = text_rects(shapes)
            .into_iter()
            .rfind(|(t, _)| t == label)
            .unwrap_or_else(|| panic!("{label:?} not drawn"))
            .1;
        // The smallest filled rect under the label: the button, not the
        // one-row footer card around it.
        shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r)
                    if r.rect.contains_rect(rect) && r.rect.height() < 60.0 && r.fill != Color32::TRANSPARENT =>
                {
                    Some((r.rect.area(), r.fill))
                }
                _ => None,
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, fill)| fill)
            .unwrap_or_else(|| panic!("no face under {label:?}"))
    }

    #[test]
    fn the_erase_button_is_destructive_and_names_the_drive() {
        let red = destructive_fill();
        let shapes = upgrade_shapes(&erase_chosen(), 1280.0, 2000.0);
        assert_eq!(face_of(&shapes, "Erase SanDisk Cruzer 16 GB"), red);
        // Upgrade is no erase: not red.
        let mut menu = erase_chosen();
        menu.upgrade_event(UpgradeEvent::Mode(crate::drives::Mode::Upgrade), Instant::now());
        let shapes = upgrade_shapes(&menu, 1280.0, 2000.0);
        assert_ne!(face_of(&shapes, "Upgrade"), red);
    }

    #[test]
    fn a_refused_row_cannot_be_chosen() {
        let menu = upgrade_open(unknown_release(), three_drives(), false, Instant::now());
        let shapes = upgrade_shapes(&menu, 1280.0, 2000.0);
        let centre = |title: &str| {
            text_rects(&shapes)
                .into_iter()
                .find(|(t, _)| t.starts_with(title))
                .unwrap_or_else(|| panic!("{title:?} not drawn"))
                .1
                .center()
        };
        let (_, ev) = upgrade_frame(&menu, 1280.0, 2000.0, click_at(centre("Intenso Rainbow")));
        assert!(!matches!(ev, Some(UiEvent::Upgrade(UpgradeEvent::Choose(_)))), "{ev:?}");
        // The same click on a usable row chooses it.
        let (_, ev) = upgrade_frame(&menu, 1280.0, 2000.0, click_at(centre("SanDisk Cruzer")));
        assert_eq!(ev, Some(UiEvent::Upgrade(UpgradeEvent::Choose("/dev/sdb".into()))));
    }

    /// The Upgrade screen with 4.5.0 checked and the second stick (3.9.2)
    /// chosen for an upgrade.
    fn notes_menu() -> Menu {
        let now = Instant::now();
        let release = crate::release::Release {
            latest: Some("4.5.0".into()),
            ..unknown_release()
        };
        let mut menu = upgrade_open(release, three_drives(), false, now);
        menu.upgrade_event(UpgradeEvent::Choose("/dev/sdb".into()), now);
        menu.upgrade_event(UpgradeEvent::Mode(crate::drives::Mode::Upgrade), now);
        menu
    }

    /// `notes_menu` with What's new pressed and `result` answered.
    fn notes_answered(result: Result<Option<String>, String>) -> Menu {
        let now = Instant::now();
        let mut menu = notes_menu();
        menu.upgrade_event(UpgradeEvent::WhatsNew, now);
        menu.finished(
            crate::jobs::JobResult::Notes {
                from: Some("3.9.2".into()),
                to: "4.5.0".into(),
                only: false,
                result,
            },
            now,
        );
        menu
    }

    fn click_text(menu: &Menu, w: f32, h: f32, label: &str) -> Option<UiEvent> {
        let shapes = upgrade_shapes(menu, w, h);
        let at = text_rects(&shapes)
            .into_iter()
            .rfind(|(t, _)| t == label)
            .unwrap_or_else(|| panic!("{label:?} not drawn"))
            .1
            .center();
        upgrade_frame(menu, w, h, click_at(at)).1
    }

    #[test]
    fn whats_new_button_sends_event() {
        let menu = notes_menu();
        assert_eq!(
            click_text(&menu, 1280.0, 2000.0, "What's new"),
            Some(UiEvent::Upgrade(UpgradeEvent::WhatsNew))
        );
        // While the notes load, the button takes no click.
        let mut menu = notes_menu();
        menu.upgrade_event(UpgradeEvent::WhatsNew, Instant::now());
        assert_ne!(
            click_text(&menu, 1280.0, 2000.0, "What's new"),
            Some(UiEvent::Upgrade(UpgradeEvent::WhatsNew))
        );
    }

    #[test]
    fn whats_new_button_hidden_for_custom_source() {
        let now = Instant::now();
        let mut menu = notes_menu();
        menu.upgrade_event(UpgradeEvent::Advanced(true), now);
        menu.upgrade_event(UpgradeEvent::Source("https://example.org/r".into()), now);
        let texts = all_texts(&upgrade_shapes(&menu, 1280.0, 2000.0));
        assert!(!texts.iter().any(|t| t == "What's new"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "Check now"), "{texts:?}");
    }

    #[test]
    fn notes_dialog_shows_heading_and_bullet() {
        let mut menu = notes_menu();
        menu.upgrade_event(UpgradeEvent::WhatsNew, Instant::now());
        assert_texts(
            &upgrade_shapes(&menu, 1280.0, 2000.0),
            &["What's new from 3.9.2 to 4.5.0", "Loading…"],
        );
        let menu = notes_answered(Ok(Some("## 4.5.0 - 2026-12-01\n### New\n- One `thing` more\n".into())));
        assert_texts(
            &upgrade_shapes(&menu, 1280.0, 2000.0),
            &["What's new from 3.9.2 to 4.5.0", "4.5.0 · New", "• One thing more"],
        );
        assert_eq!(
            click_text(&menu, 1280.0, 2000.0, "Close"),
            Some(UiEvent::Upgrade(UpgradeEvent::CloseNotes))
        );
        // The screen under the dialog takes no clicks.
        assert_eq!(click_text(&menu, 1280.0, 2000.0, "Check now"), None);
        let escape = vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        assert_eq!(
            upgrade_frame(&menu, 1280.0, 2000.0, escape.clone()).1,
            Some(UiEvent::Upgrade(UpgradeEvent::CloseNotes))
        );
        // Without the dialog, Escape does nothing on the screen.
        assert_eq!(upgrade_frame(&notes_menu(), 1280.0, 2000.0, escape).1, None);
    }

    #[test]
    fn notes_titles_for_fresh_and_unknown_versions() {
        let now = Instant::now();
        let mut menu = notes_menu();
        menu.upgrade_event(UpgradeEvent::Mode(crate::drives::Mode::Fresh), now);
        menu.upgrade_event(UpgradeEvent::WhatsNew, now);
        assert_texts(&upgrade_shapes(&menu, 1280.0, 2000.0), &["What's new in 4.5.0"]);
        let mut drives = three_drives();
        drives[1].version = None;
        let release = crate::release::Release {
            latest: Some("4.5.0".into()),
            ..unknown_release()
        };
        let mut menu = upgrade_open(release, drives, false, now);
        menu.upgrade_event(UpgradeEvent::Choose("/dev/sdb".into()), now);
        menu.upgrade_event(UpgradeEvent::Mode(crate::drives::Mode::Upgrade), now);
        menu.upgrade_event(UpgradeEvent::WhatsNew, now);
        menu.finished(
            crate::jobs::JobResult::Notes {
                from: None,
                to: "4.5.0".into(),
                only: false,
                result: Ok(Some("## 4.5.0\n- a\n".into())),
            },
            now,
        );
        assert_texts(
            &upgrade_shapes(&menu, 1280.0, 2000.0),
            &[
                "What's new up to 4.5.0",
                "The version on this stick is not known: all releases up to 4.5.0 are shown.",
            ],
        );
    }

    #[test]
    fn notes_empty_range_text() {
        let menu = notes_answered(Ok(None));
        assert_texts(
            &upgrade_shapes(&menu, 1280.0, 2000.0),
            &["No changes between 3.9.2 and 4.5.0."],
        );
    }

    #[test]
    fn notes_failed_text() {
        let mut menu = notes_answered(Err("could not fetch the release notes: timeout".into()));
        assert_texts(
            &upgrade_shapes(&menu, 1280.0, 2000.0),
            &[
                "What's new from 3.9.2 to 4.5.0",
                "The release notes could not be fetched",
                "could not fetch the release notes: timeout",
            ],
        );
        // The stick is pulled behind the dialog: the title keeps the range
        // that failed.
        let later = Instant::now() + std::time::Duration::from_secs(5);
        menu.tick(later);
        menu.finished(
            crate::jobs::JobResult::Drives {
                all: false,
                result: Ok(vec![]),
            },
            later,
        );
        assert!(menu.upgrade.chosen_drive().is_none());
        assert_texts(
            &upgrade_shapes(&menu, 1280.0, 2000.0),
            &["What's new from 3.9.2 to 4.5.0"],
        );
    }

    #[test]
    fn notes_dialog_fits_1280x720() {
        let mut text = String::from("## 4.5.0 - 2026-12-01\n### New\n");
        for i in 0..200 {
            text.push_str(&format!(
                "- Change number {i} of the release, with a line long enough to wrap once.\n"
            ));
        }
        let menu = notes_answered(Ok(Some(text)));
        let shapes = upgrade_shapes(&menu, 1280.0, 720.0);
        let rects = text_rects(&shapes);
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 720.0));
        for label in ["What's new from 3.9.2 to 4.5.0", "Close"] {
            let r = rects
                .iter()
                .rfind(|(t, _)| t == label)
                .unwrap_or_else(|| panic!("{label:?} not drawn"))
                .1;
            assert!(screen.contains_rect(r), "{label:?} at {r:?}");
        }
        // It scrolls: the last bullet is not drawn.
        assert!(!rects.iter().any(|(t, _)| t.contains("number 199")));
        assert_eq!(
            click_text(&menu, 1280.0, 720.0, "Close"),
            Some(UiEvent::Upgrade(UpgradeEvent::CloseNotes))
        );
    }

    /// The track of a progress bar: its fill and height, from one drawn alone.
    fn progress_track() -> (Color32, f32) {
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let mut o = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.add(shadcn_egui::components::progress::Progress::new(0.0));
        });
        o.textures_delta.clear();
        o.shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.fill != Color32::TRANSPARENT => Some((r.fill, r.rect.height())),
                _ => None,
            })
            .expect("a progress track")
    }

    /// The tracks of the progress bars drawn, with how much of each is filled.
    fn bars(shapes: &[egui::epaint::ClippedShape]) -> Vec<(Rect, f32)> {
        let (track, h) = progress_track();
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let primary = Theme::current(&ctx).palette().primary;
        let tracks: Vec<Rect> = shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.fill == track && (r.rect.height() - h).abs() < 0.01 => Some(r.rect),
                _ => None,
            })
            .collect();
        tracks
            .into_iter()
            .map(|t| {
                let filled = shapes
                    .iter()
                    .filter_map(|c| {
                        // The indicator is a rect, or a path cut to the track.
                        let r = match &c.shape {
                            egui::Shape::Rect(r) if r.fill == primary => r.rect,
                            egui::Shape::Path(p) if p.fill == primary => c.shape.visual_bounding_rect(),
                            _ => return None,
                        };
                        ((r.center().y - t.center().y).abs() < 0.5)
                            .then(|| r.intersect(c.clip_rect).intersect(t).width().max(0.0))
                    })
                    .fold(0.0, f32::max);
                (t, filled / t.width())
            })
            .collect()
    }

    #[test]
    fn the_running_stage_shows_bar_rate_and_time_left() {
        let total = 3_100_000_000;
        let menu = writing_at(total * 42 / 100, Some(total));
        let shapes = upgrade_shapes(&menu, 1280.0, 2000.0);
        let bars = bars(&shapes);
        assert_eq!(bars.len(), 1, "{bars:?}");
        assert!((bars[0].1 - 0.42).abs() < 0.01, "{bars:?}");
        assert_texts(
            &shapes,
            &["1.3 GB of 3.1 GB", "12.0 MB/s", "about 3 min left", "Cancel"],
        );
    }

    #[test]
    fn no_bar_without_a_total() {
        for total in [None, Some(1_000_000_000)] {
            // No total, or one the bytes are past: no bar, no time left.
            let menu = writing_at(1_302_000_000, total);
            let shapes = upgrade_shapes(&menu, 1280.0, 2000.0);
            assert!(bars(&shapes).is_empty(), "{total:?}");
            assert_texts(&shapes, &["1.3 GB written", "12.0 MB/s"]);
            let texts = all_texts(&shapes);
            assert!(
                !texts.iter().any(|t| t.contains("left") || t.contains('%')),
                "{texts:?}"
            );
        }
    }

    #[test]
    fn the_dialog_names_the_write_and_warns_on_fresh() {
        let mut menu = writing_at(1_000_000_000, Some(3_100_000_000));
        // Restart from the Upgrade screen: the dialog draws over it.
        assert_eq!(menu.screen, Screen::Upgrade);
        menu.ending = Action::Reboot;
        menu.logout = Some(Logout::Confirm);
        let shapes = upgrade_shapes(&menu, 1280.0, 720.0);
        assert_texts(&shapes, &["A stick is being written", "Writing SanDisk Cruzer"]);
        menu.logout = Some(Logout::WaitAddons);
        let shapes = upgrade_shapes(&menu, 1280.0, 720.0);
        assert_texts(
            &shapes,
            &[
                "Waiting for the stick before restarting",
                "Writing SanDisk Cruzer",
                "If you restart anyway, SanDisk Cruzer will not start until it is written again.",
                "Restart anyway",
            ],
        );
        // Log out only waits, and says nothing about "anyway".
        menu.ending = Action::Logout;
        let texts = all_texts(&upgrade_shapes(&menu, 1280.0, 720.0));
        assert!(!texts.iter().any(|t| t.contains("anyway")), "{texts:?}");
    }

    #[test]
    fn the_dialog_does_not_warn_before_the_first_byte() {
        use crate::jobs::JobResult;
        use crate::upgrade_proto::Msg;
        let now = Instant::now();
        let mut menu = erase_chosen();
        menu.upgrade_event(UpgradeEvent::Confirm, now);
        menu.finished(JobResult::UpgradeStarted { pgid: 4242 }, now);
        menu.finished(
            JobResult::UpgradeMsg(Msg::Step {
                id: "write".into(),
                text: "Downloading and writing".into(),
            }),
            now,
        );
        menu.ending = Action::Reboot;
        menu.logout = Some(Logout::WaitAddons);
        let texts = all_texts(&upgrade_shapes(&menu, 1280.0, 720.0));
        assert!(texts.iter().any(|t| t == "Restart anyway"), "{texts:?}");
        assert!(
            !texts.iter().any(|t| t.contains("will not start")),
            "no byte was written yet: {texts:?}"
        );
    }

    /// Where `label` is drawn on a `w` x `h` screen (points), and whether
    /// it is fully visible: inside its clip and above the log.
    fn visible_button(menu: &Menu, w: f32, h: f32, label: &str) -> (Rect, bool) {
        let shapes = upgrade_shapes(menu, w, h);
        let (rect, clip) = shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text() == label => {
                    Some((t.galley.rect.translate(t.pos.to_vec2()), c.clip_rect))
                }
                _ => None,
            })
            .next_back()
            .unwrap_or_else(|| panic!("{label:?} not drawn"));
        let log = layout_with(screen(w, h), menu.visible_tiles().len(), 0, 0, 0, small_log(menu)).log;
        (rect, clip.contains_rect(rect) && rect.bottom() < log.top())
    }

    #[test]
    fn the_main_buttons_stay_visible_above_the_log() {
        let total = 3_100_000_000;
        let mut asked = writing_at(total * 42 / 100, Some(total));
        asked.upgrade_event(UpgradeEvent::Cancel, Instant::now());
        // 1080p, and a 21 inch 4K screen at scale 2.25, in points.
        for (w, h) in [(1920.0, 1080.0), (3840.0 / 2.25, 2160.0 / 2.25)] {
            for (menu, label) in [
                (&erase_chosen(), "Erase SanDisk Cruzer 16 GB"),
                (&writing_at(total * 42 / 100, Some(total)), "Cancel"),
                (&asked, "Stop"),
                (&asked, "Keep writing"),
            ] {
                let (rect, ok) = visible_button(menu, w, h, label);
                assert!(ok, "{label:?} at {rect:?} on {w}x{h} is not visible");
            }
        }
    }

    /// The internal-drive fixture of the shots: Advanced on with an
    /// address, Show all, four drives, the SSD chosen and its name typed.
    fn internal_chosen() -> Menu {
        let now = Instant::now();
        let mut drives = three_drives();
        drives.push(internal_disk());
        let mut m = upgrade_open(unknown_release(), drives, true, now);
        m.upgrade_event(UpgradeEvent::Source("ftp://example.org/thinfloe".into()), now);
        m.upgrade_event(UpgradeEvent::Choose("/dev/nvme0n1".into()), now);
        m.upgrade_event(UpgradeEvent::Typed("Samsung SSD".into()), now);
        m
    }

    #[test]
    fn the_choose_blocks_fit_above_the_footer() {
        let note = "The copy leaves out the SSH host keys and the WireGuard key; the remote-access password travels with the home.";
        let common = [
            "Release",
            "Installed",
            "Check now",
            "Target",
            "Intenso Rainbow, 2.0 GB",
            "Options",
            "Copy this machine's home directory",
            "Check the stick after writing",
            note,
        ];
        // 1080p, and a 21 inch 4K screen at scale 2.25, in points.
        for (w, h) in [(1920.0, 1080.0), (3840.0 / 2.25, 2160.0 / 2.25)] {
            for (menu, extra, summary) in [
                (
                    erase_chosen(),
                    // An internal drive offers only Erase, so no Action block.
                    vec![
                        "Action",
                        "Erase and install",
                        "Upgrade (A/B, active after a restart)",
                        "Erase SanDisk Cruzer 16 GB",
                    ],
                    "Erase SanDisk Cruzer, 16 GB, and install the newest ThinFloe",
                ),
                (
                    internal_chosen(),
                    vec![
                        "Samsung SSD 980, 500 GB",
                        "Show all drives",
                        "Erase Samsung SSD 980 500 GB",
                    ],
                    "Erase Samsung SSD 980, 500 GB, and install the newest ThinFloe",
                ),
            ] {
                for label in common.iter().chain(extra.iter()) {
                    let (rect, ok) = visible_button(&menu, w, h, label);
                    assert!(ok, "{label:?} at {rect:?} on {w}x{h} is not visible");
                }
                let (summary, _) = visible_button(&menu, w, h, summary);
                for label in [note, "Intenso Rainbow, 2.0 GB"] {
                    let (r, _) = visible_button(&menu, w, h, label);
                    assert!(
                        r.bottom() < summary.top(),
                        "{label:?} at {r:?} runs into the footer on {w}x{h}"
                    );
                }
            }
        }
    }

    /// The block a label is drawn in: the smallest filled rect around it
    /// that is wider than a button.
    fn card_of(shapes: &[egui::epaint::ClippedShape], label: &str) -> Rect {
        let rect = text_rects(shapes)
            .into_iter()
            .rfind(|(t, _)| t == label)
            .unwrap_or_else(|| panic!("{label:?} not drawn"))
            .1;
        shapes
            .iter()
            .filter_map(|c| match &c.shape {
                // Wider than any button: a block, not a button's face.
                egui::Shape::Rect(r)
                    if r.rect.contains_rect(rect) && r.fill != Color32::TRANSPARENT && r.rect.width() > 400.0 =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .min_by(|a, b| a.area().total_cmp(&b.area()))
            .unwrap_or_else(|| panic!("no block around {label:?}"))
    }

    #[test]
    fn the_footer_follows_the_page_and_is_pinned_only_on_overflow() {
        let summary = "Erase SanDisk Cruzer, 16 GB, and install the newest ThinFloe";
        // Fits: below the lowest block by the gap between two blocks.
        let shapes = upgrade_shapes(&erase_chosen(), 1920.0, 1080.0);
        let gap = card_of(&shapes, "Target").top() - card_of(&shapes, "Release").bottom();
        assert!(gap > 0.0, "{gap}");
        let footer = card_of(&shapes, summary);
        let last = ["Target", "Options"]
            .iter()
            .map(|l| card_of(&shapes, l).bottom())
            .fold(f32::MIN, f32::max);
        assert!(
            (footer.top() - (last + gap)).abs() < 1.0,
            "footer {footer:?}, page ends {last}"
        );
        let total = 3_100_000_000;
        let running = writing_at(total / 2, Some(total));
        let shapes = upgrade_shapes(&running, 1920.0, 1080.0);
        let footer = card_of(&shapes, "Cancel");
        let steps = card_of(&shapes, "Downloading and writing");
        assert!(
            (footer.top() - (steps.bottom() + gap)).abs() < 1.0,
            "{footer:?} {steps:?}"
        );
        // Overflows: pinned to the bottom of the panel, button in view.
        let (w, h) = (1280.0, 600.0);
        let menu = internal_chosen();
        let shapes = upgrade_shapes(&menu, w, h);
        let label = "Erase Samsung SSD 980, 500 GB, and install the newest ThinFloe";
        let footer = card_of(&shapes, label);
        let log = layout_with(screen(w, h), menu.visible_tiles().len(), 0, 0, 0, small_log(&menu)).log;
        assert!((footer.bottom() - (log.top() - 16.0)).abs() < 1.0, "{footer:?} {log:?}");
        let (rect, ok) = visible_button(&menu, w, h, "Erase Samsung SSD 980 500 GB");
        assert!(ok, "{rect:?}");
    }

    #[test]
    fn a_finished_boot_upgrade_offers_restart_and_another_stick() {
        use crate::jobs::JobResult;
        use crate::upgrade_proto::Msg;
        let now = Instant::now();
        let mut done = upgrade_open(unknown_release(), three_drives(), false, now);
        done.upgrade_event(UpgradeEvent::Choose("/dev/sda".into()), now);
        done.upgrade_event(UpgradeEvent::Confirm, now);
        done.finished(JobResult::UpgradeStarted { pgid: 4242 }, now);
        done.finished(
            JobResult::UpgradeMsg(Msg::Result {
                ok: true,
                reason: String::new(),
            }),
            now,
        );
        done.finished(JobResult::UpgradeEnded { status: Ok(Some(0)) }, now);
        let shapes = upgrade_shapes(&done, 1920.0, 1080.0);
        assert_texts(&shapes, &["Restart now", "Write another stick", "Back"]);
        let (rect, ok) = visible_button(&done, 1920.0, 1080.0, "Write another stick");
        assert!(ok, "{rect:?}");
    }

    #[test]
    fn the_log_is_small_only_while_choosing() {
        let rows = |m: &Menu| {
            let l = layout_with(screen(1920.0, 1080.0), m.visible_tiles().len(), 0, 0, 0, small_log(m));
            ((l.log.height() - 2.0 * LOG_PAD) / LOG_ROW).round() as usize
        };
        let choosing = erase_chosen();
        assert_eq!(rows(&choosing), LOG_CHOOSE_ROWS);
        let total = 3_100_000_000;
        let running = writing_at(total / 2, Some(total));
        assert!(rows(&running) >= LOG_MIN_ROWS, "{}", rows(&running));
        let mut desktop = erase_chosen();
        desktop.upgrade_event(UpgradeEvent::Back, Instant::now());
        assert_eq!(rows(&desktop), rows(&running));
    }

    #[test]
    fn the_target_says_it_looks_until_the_first_list() {
        let now = Instant::now();
        let mut menu = Menu::new(shipped_grid(), false);
        menu.set_usb_boot(true);
        let tile = menu.tiles.iter().position(|t| t.action == Action::Upgrade).unwrap();
        menu.activate(tile, now);
        let texts = all_texts(&upgrade_shapes(&menu, 1280.0, 2000.0));
        assert!(texts.iter().any(|t| t == "Looking for drives…"), "{texts:?}");
        assert!(!texts.iter().any(|t| t.contains("No USB stick")), "{texts:?}");
        let empty = upgrade_open(unknown_release(), vec![], false, now);
        assert_texts(
            &upgrade_shapes(&empty, 1280.0, 2000.0),
            &["No USB stick found. Plug one in."],
        );
        let all = upgrade_open(unknown_release(), vec![], true, now);
        assert_texts(
            &upgrade_shapes(&all, 1280.0, 2000.0),
            &["No drive found. Plug a stick in."],
        );
    }

    #[test]
    fn the_waiting_titles_name_what_the_dialog_lists() {
        assert_eq!(root_waits(false, true), "the stick");
        assert_eq!(root_waits(true, false), "the add-ons");
        assert_eq!(root_waits(true, true), "the add-ons and the stick");
    }

    #[test]
    fn the_upgrade_screen_draws_nothing_rounded() {
        let now = Instant::now();
        let mut internal = upgrade_open(unknown_release(), vec![internal_disk()], true, now);
        internal.upgrade_event(UpgradeEvent::Choose("/dev/nvme0n1".into()), now);
        for menu in [erase_chosen(), internal, writing_at(1_302_000_000, Some(3_100_000_000))] {
            let shapes = upgrade_shapes(&menu, 1280.0, 2000.0);
            assert!(!all_texts(&shapes).is_empty());
            for c in &shapes {
                if let egui::Shape::Rect(r) = &c.shape {
                    assert_eq!(r.corner_radius, egui::CornerRadius::ZERO, "rounded {:?}", r.rect);
                }
            }
        }
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
        let mut canvas = crate::raster::Canvas::new(px.0 as usize, px.1 as usize, crate::brand::rgb::BG);
        // As the real header: the logo at the screen's pixel density (as
        // app.rs loads it) and the release the build writes.
        let logo = ctx.load_texture(
            "thinfloe-logo",
            crate::brand::logo((LOGO_H * s).round() as u32),
            egui::TextureOptions::LINEAR,
        );
        let version = include_str!("../../../RELEASE").trim().to_string();
        let chrome = Chrome {
            header: crate::header::Header {
                host: "thinfloe-a1b2c3".into(),
                version: Some(version),
            },
            clock: "Thu 30 Oct 14:05".into(),
            logo: Some(logo),
            covered: false,
            wall: None,
        };
        // As the app: a dialog stands on the screen under it, frozen and
        // dimmed.
        let passes: &[bool] = if menu.dialog() { &[false, true] } else { &[true] };
        let mut frame = 0;
        for &with_dialog in passes {
            if with_dialog && passes.len() > 1 {
                canvas.freeze_dimmed(crate::app::DIM_KEEP);
            }
            for _ in 0..12 {
                let mut input = input.clone();
                input.time = Some(frame as f64 * 0.1);
                frame += 1;
                let mut out = ctx.run_ui(input, |ui| {
                    if with_dialog {
                        show(ui, menu, &icons, &chrome, Instant::now());
                    } else {
                        show_screen(ui, menu, &icons, &chrome, Instant::now());
                    }
                });
                textures.apply_set(&out.textures_delta);
                out.textures_delta.clear();
                let prims = ctx.tessellate(out.shapes, s);
                canvas.restore_background(canvas.bounds());
                canvas.draw(&prims, &textures, s, canvas.bounds());
            }
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
        net.set_network(crate::status::network::sample(300));
        shot(&net, "desktop-1080-status.ppm", (1920, 1080), 1.0);
        shot(&net, "desktop-4k-status.ppm", (3840, 2160), 2.0);
        shot(&net, "desktop-1366x768-status.ppm", (1366, 768), 1.0);
        shot(&net, "desktop-1280x720-status.ppm", (1280, 720), 1.0);
        shot(&net, "desktop-1440x900-status.ppm", (1440, 900), 1.0);
        let mut no_battery = status_menu(Vec::new(), Instant::now());
        no_battery.set_network(crate::status::network::sample(300));
        shot(&no_battery, "desktop-1080-no-battery.ppm", (1920, 1080), 1.0);
        let (mut menu, _) = four_addons();
        let settings = menu.tiles.iter().position(|t| t.name == "Settings").unwrap();
        menu.activate(settings, Instant::now());
        shot(&menu, "settings-1080.ppm", (1920, 1080), 1.0);
        shot(&menu, "settings-21in-4k.ppm", (3840, 2160), 2.25);
        use crate::screentab::ScreenTab;
        menu.settings_tab = crate::settings::SettingsTab::Screen;
        menu.screen_tab = ScreenTab { idle: 300, error: None };
        shot(&menu, "settings-screen-1080.ppm", (1920, 1080), 1.0);
        menu.screen_tab = ScreenTab {
            idle: 420,
            error: Some("config.toml could not be written: No space left on device (os error 28)".into()),
        };
        shot(&menu, "settings-screen-error-1080.ppm", (1920, 1080), 1.0);
        shot(&menu, "settings-screen-21in-4k.ppm", (3840, 2160), 2.25);
        // The Network tab: menu-made settings, two networks, a fixed address.
        let mut net = two_networks();
        net.status_done(
            Ok(vec![crate::nettab::Iface {
                name: "enp1s0".into(),
                kind: crate::nettab::IfKind::Wired,
                state: crate::nettab::IfState::Connected,
                addresses: vec!["192.168.1.10/24".into()],
                ssid: None,
                signal_dbm: None,
                handshake_age_s: None,
            }]),
            Default::default(),
        );
        menu.settings_tab = crate::settings::SettingsTab::Network;
        menu.network = net;
        shot(&menu, "settings-network-1080.ppm", (1920, 1080), 1.0);
        shot(&menu, "settings-network-21in-4k.ppm", (3840, 2160), 2.25);
        shot(&menu, "settings-network-4k.ppm", (3840, 2160), 2.0);
        menu.network = add_dialog_open();
        shot(&menu, "settings-network-add-1080.ppm", (1920, 1080), 1.0);
        {
            use crate::netfiles::FileState;
            use crate::network::NetEvent;
            let one = (1920, 1080);
            // Both files written by hand.
            let mut m = open_settings();
            m.settings_tab = crate::settings::SettingsTab::Network;
            m.network = net_files(false, FileState::Hand, FileState::Hand, &["wlp1"]);
            shot(&m, "settings-network-hand-1080.ppm", one, 1.0);
            // Netboot: always DHCP.
            m.network = net_files(true, FileState::Menu, FileState::Menu, &["wlp1"]);
            shot(&m, "settings-network-netboot-1080.ppm", one, 1.0);
            // No adapter at all.
            m.network = net_files(false, FileState::Missing, FileState::Missing, &[]);
            m.network.status_done(Ok(Vec::new()), Default::default());
            shot(&m, "settings-network-noadapter-1080.ppm", one, 1.0);
            // A saved network whose adapter is gone.
            m.network = net_files(false, FileState::Menu, FileState::Menu, &["wlp9"]);
            shot(&m, "settings-network-adapter-changed-1080.ppm", one, 1.0);
            // The Apply confirmation (remote session).
            m.network = two_networks();
            m.network.event(NetEvent::Forget("Lab".into()), true);
            m.network.event(NetEvent::Apply, true);
            shot(&m, "settings-network-confirm-1080.ppm", one, 1.0);
            // The add dialog with a hidden network typed.
            m.network = add_dialog_open();
            m.network.event(
                NetEvent::AddText {
                    ssid: "Guest".into(),
                    password: "hunter2hunter2".into(),
                    hidden: true,
                },
                false,
            );
            shot(&m, "settings-network-add-hidden-1080.ppm", one, 1.0);
            // An Apply that failed.
            m.network = two_networks();
            m.network.event(NetEvent::Forget("Lab".into()), false);
            m.network.event(NetEvent::Apply, false);
            m.network.apply_started(7);
            m.network.apply_done(Err(crate::jobs::ApplyFailed {
                reason: "netplan apply failed: Invalid YAML in /etc/netplan/90-thinfloe.yaml".into(),
                home_changed: false,
            }));
            shot(&m, "settings-network-error-1080.ppm", one, 1.0);
        }
        // The Status tab: the shipped layout, two devices and a network.
        let status = status_tab_menu();
        shot(&status, "settings-status-1080.ppm", (1920, 1080), 1.0);
        shot(&status, "settings-status-1280x720.ppm", (1280, 720), 1.0);
        let mut newer = Menu::new(shipped_grid(), false);
        newer.set_usb_boot(true);
        newer.set_installed(Some("4.0.0".into()));
        newer.upgrade.release_file(crate::release::Release {
            latest: Some("4.1.0".into()),
            ..Default::default()
        });
        shot(&newer, "desktop-1080-upgrade-marker.ppm", (1920, 1080), 1.0);
        shot_elements();
    }

    #[test]
    #[ignore]
    fn shot_upgrade_screens() {
        use crate::jobs::JobResult;
        use crate::upgrade_proto::Msg;
        let now = Instant::now();
        let release = crate::release::Release {
            latest: Some("4.1.0".into()),
            image_bytes: Some(IMAGE),
            checked: Some(1_793_000_000),
            ..Default::default()
        };
        let mut menu = upgrade_open(release.clone(), three_drives(), false, now);
        menu.set_installed(Some("4.0.0".into()));
        menu.upgrade_event(UpgradeEvent::Choose("/dev/sdb".into()), now);
        menu.upgrade_event(UpgradeEvent::Mode(crate::drives::Mode::Fresh), now);
        shot(&menu, "upgrade-choose-1080.ppm", (1920, 1080), 1.0);
        shot(&menu, "upgrade-choose-21in-4k.ppm", (3840, 2160), 2.25);
        // The whole page at once, for looking at what scrolls at 1080p.
        shot(&menu, "upgrade-choose-tall.ppm", (1920, 2600), 1.0);

        let mut drives = three_drives();
        drives.push(internal_disk());
        let mut internal = upgrade_open(release.clone(), drives, true, now);
        internal.upgrade_event(UpgradeEvent::Source("ftp://example.org/thinfloe".into()), now);
        internal.upgrade_event(UpgradeEvent::Choose("/dev/nvme0n1".into()), now);
        internal.upgrade_event(UpgradeEvent::Typed("Samsung SSD".into()), now);
        shot(&internal, "upgrade-choose-internal-1080.ppm", (1920, 1080), 1.0);
        shot(&internal, "upgrade-choose-internal-tall.ppm", (1920, 2600), 1.0);
        // Too tall for the screen: the footer is pinned, the page scrolls.
        shot(&internal, "upgrade-choose-overflow.ppm", (1280, 600), 1.0);

        let total = 3_100_000_000;
        let running = writing_at(total * 42 / 100, Some(total));
        shot(&running, "upgrade-running-1080.ppm", (1920, 1080), 1.0);
        shot(
            &writing_at(1_302_000_000, None),
            "upgrade-running-unknown-1080.ppm",
            (1920, 1080),
            1.0,
        );
        let mut ask = writing_at(total * 42 / 100, Some(total));
        ask.upgrade_event(UpgradeEvent::Cancel, now);
        shot(&ask, "upgrade-cancel-ask-1080.ppm", (1920, 1080), 1.0);

        let mut done = upgrade_open(release.clone(), three_drives(), false, now);
        done.upgrade_event(UpgradeEvent::Choose("/dev/sda".into()), now);
        done.upgrade_event(UpgradeEvent::Confirm, now);
        done.finished(JobResult::UpgradeStarted { pgid: 4242 }, now);
        for (id, text) in [
            ("save", "Saving the home directory"),
            ("download", "Downloading the new release"),
            ("check", "Checking the files"),
            ("switch", "Copying the new release to the stick"),
        ] {
            done.finished(
                JobResult::UpgradeMsg(Msg::Step {
                    id: id.into(),
                    text: text.into(),
                }),
                now,
            );
            done.finished(JobResult::UpgradeMsg(Msg::Done { id: id.into() }), now);
        }
        done.finished(
            JobResult::UpgradeMsg(Msg::Result {
                ok: true,
                reason: String::new(),
            }),
            now,
        );
        done.finished(JobResult::UpgradeEnded { status: Ok(Some(0)) }, now);
        shot(&done, "upgrade-done-boot-1080.ppm", (1920, 1080), 1.0);

        let mut failed = writing_at(total * 42 / 100, Some(total));
        failed.finished(
            JobResult::UpgradeMsg(Msg::Result {
                ok: false,
                reason: "the stick was removed while it was written".into(),
            }),
            now,
        );
        failed.finished(JobResult::UpgradeEnded { status: Ok(Some(1)) }, now);
        shot(&failed, "upgrade-failed-1080.ppm", (1920, 1080), 1.0);

        let mut dialog = writing_at(total * 42 / 100, Some(total));
        dialog.ending = Action::Reboot;
        dialog.logout = Some(Logout::WaitAddons);
        shot(&dialog, "dialog-writing-1080.ppm", (1920, 1080), 1.0);
    }

    // ---- The monitors of the Screen tab ----

    use crate::monitors::{MonEvent, MonTab};

    /// A monitor without a serial, so its criteria are its name, with its
    /// own size as the current mode and 1280x720 at 59.94 Hz besides.
    fn mon_head(name: &str, model: &str, x: i32, w: u32, h: u32) -> crate::heads::Head {
        use crate::heads::{Mode, Rotation};
        crate::heads::Head {
            name: name.into(),
            make: "Dell".into(),
            model: model.into(),
            serial: String::new(),
            phys_mm: (600, 340),
            enabled: true,
            modes: vec![
                Mode {
                    w,
                    h,
                    refresh_mhz: 60_000,
                    preferred: true,
                },
                Mode {
                    w: 1280,
                    h: 720,
                    refresh_mhz: 59_940,
                    preferred: false,
                },
            ],
            current: Some(0),
            pos: (x, 0),
            rotation: Rotation::Normal,
        }
    }

    fn mon_heads() -> Vec<crate::heads::Head> {
        vec![
            mon_head("HDMI-A-1", "P2419H", 0, 1920, 1080),
            mon_head("DP-1", "U2720Q", 1920, 2560, 1440),
        ]
    }

    /// HDMI-A-1 (1920x1080) with DP-1 (2560x1440) right of it, no file yet;
    /// no dialog open.
    fn two_monitors() -> MonTab {
        let mut mon = MonTab::default();
        mon.heads_changed(mon_heads());
        mon.loaded(Ok(None), crate::monitors::MonSource::Missing);
        mon
    }

    /// `two_monitors` with the dialog of `criteria` open.
    fn monitor_dialog(criteria: &str) -> MonTab {
        let mut mon = two_monitors();
        mon.event(MonEvent::Select(criteria.into()), Instant::now());
        mon
    }

    /// The On switch in the row of monitor name.
    fn switch_in_row(shapes: &[egui::epaint::ClippedShape], name: &str) -> Rect {
        switch_in_column(shapes, "On", name)
    }

    /// The switch in the column under heading on the row of name: the
    /// smallest shape a little right of the heading's left edge, on the
    /// row. The heading is the nearest one above the row, so the client
    /// list's On is not taken for the monitor list's.
    fn switch_in_column(shapes: &[egui::epaint::ClippedShape], heading: &str, name: &str) -> Rect {
        let rects = text_rects(shapes);
        let (head, row) = rects
            .iter()
            .filter(|(t, _)| t == heading)
            .flat_map(|(_, h)| {
                rects
                    .iter()
                    .filter(move |(t, r)| t == name && r.top() > h.bottom())
                    .map(move |(_, r)| (*h, *r))
            })
            .min_by(|a, b| (a.1.top() - a.0.bottom()).total_cmp(&(b.1.top() - b.0.bottom())))
            .unwrap_or_else(|| panic!("no row of {name} under {heading} in {:?}", all_texts(shapes)));
        let at = pos2(head.left() + 4.0, row.center().y);
        shapes
            .iter()
            .filter(|c| !matches!(c.shape, egui::Shape::Text(_)))
            .map(|c| c.shape.visual_bounding_rect())
            .filter(|r| r.contains(at))
            .min_by(|a, b| a.area().total_cmp(&b.area()))
            .unwrap_or_else(|| panic!("no switch under {heading} in the row of {name}"))
    }

    fn escape_key() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    #[test]
    fn every_monitor_shows_its_resolution_and_on_without_a_click() {
        let texts = mon_texts(&monitor_settings(two_monitors()));
        for want in [
            "Monitor",
            "Resolution",
            "On",
            "HDMI-A-1",
            "DP-1",
            "1920 × 1080 @ 60 Hz",
            "2560 × 1440 @ 60 Hz",
        ] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        for gone in ["Rotation", "Position", "Show the menu here"] {
            assert!(!has(&texts, gone), "{gone} in {texts:?}");
        }
    }

    #[test]
    fn a_monitor_box_click_opens_its_dialog_and_escape_closes_it() {
        let menu = monitor_settings(two_monitors());
        assert_eq!(mon_clicks(&menu, &["DP-1"]), [mon_ev(MonEvent::Select("DP-1".into()))]);
        let menu = monitor_settings(monitor_dialog("DP-1"));
        let texts = mon_texts(&menu);
        for want in ["Position", "Rotation", "Show the menu here", "Close"] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        assert_eq!(mon_clicks(&menu, &["Close"]), [mon_ev(MonEvent::CloseDialog)]);
        assert_eq!(
            upgrade_frame(&menu, MON_WINDOW.0, MON_WINDOW.1, escape_key()).1,
            mon_ev(MonEvent::CloseDialog)
        );
    }

    #[test]
    fn the_screen_under_a_screen_dialog_takes_no_click() {
        // Without a dialog the idle select takes the click.
        let open = monitor_settings(two_monitors());
        let idle = crate::screentab::label(open.screen_tab.idle);
        let other = crate::screentab::options(open.screen_tab.idle)
            .into_iter()
            .map(crate::screentab::label)
            .find(|l| *l != idle)
            .unwrap();
        let (events, _) = click_labels_at(&open, &[&idle, &other], MON_WINDOW);
        assert!(events[1].is_some(), "{events:?}");
        let mut deskflow = monitor_settings(two_monitors());
        deskflow.deskflow.selected = Some("macbook".into());
        for menu in [monitor_settings(monitor_dialog("DP-1")), deskflow] {
            let (events, _) = click_labels_at(&menu, &[&idle, &other], MON_WINDOW);
            assert_eq!(events, [None, None]);
        }
    }

    const DF_FP_MAC: &str = "v2:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DF_FP_PC: &str = "v2:sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const DF_FP_LNX: &str = "v2:sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const DF_FP_SELF: &str = "v2:sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

    fn df_device(name: &str, fp: &str, output: &str, side: crate::kanshi::Side) -> crate::config::DeviceCfg {
        crate::config::DeviceCfg {
            name: name.into(),
            fingerprint: fp.into(),
            output: output.into(),
            side,
            clipboard: true,
            compose: false,
            enabled: true,
        }
    }

    /// The daemon runs with "mac" right of DP-1 and "pc" left of HDMI-A-1
    /// attached; no dialog open.
    fn two_clients() -> crate::deskflowtab::DeskflowTab {
        use crate::deskflow::statefile::Head;
        use crate::kanshi::Side;
        let mut df = crate::deskflowtab::DeskflowTab::default();
        df.cfg.listen = true;
        df.cfg.devices = vec![
            df_device("mac", DF_FP_MAC, "DP-1", Side::RightOf),
            df_device("pc", DF_FP_PC, "HDMI-A-1", Side::LeftOf),
        ];
        df.running = true;
        df.state.head = Head::SelfInfo {
            fingerprint: DF_FP_SELF.into(),
            listen: "0.0.0.0:24800".into(),
        };
        df
    }

    /// The tab of df, and "lnx" wants to attach.
    fn with_lnx_pending(mut df: crate::deskflowtab::DeskflowTab) -> crate::deskflowtab::DeskflowTab {
        df.state.devices.push(crate::deskflow::statefile::DevLine {
            name: "lnx".into(),
            fingerprint: DF_FP_LNX.into(),
            state: crate::deskflow::statefile::DevState::Pending,
            since: 1,
            width: 0,
            height: 0,
            flags: vec![],
        });
        df
    }

    /// The two monitors' Screen tab with the Deskflow band of df.
    fn deskflow_settings(df: crate::deskflowtab::DeskflowTab) -> Menu {
        let mut menu = monitor_settings(two_monitors());
        menu.deskflow = df;
        menu
    }

    fn df_ev(e: crate::deskflowtab::DeskflowEvent) -> Option<UiEvent> {
        Some(UiEvent::Settings(SettingsEvent::Deskflow(e)))
    }

    /// The Deskflow tests' window: tall enough that the band is in view
    /// without a scroll.
    const DF_WINDOW: (f32, f32) = (1920.0, 2000.0);

    fn df_shapes(menu: &Menu) -> Vec<egui::epaint::ClippedShape> {
        settings_shapes(menu, DF_WINDOW.0, DF_WINDOW.1)
    }

    fn df_texts(menu: &Menu) -> Vec<String> {
        all_texts(&df_shapes(menu))
    }

    fn df_clicks(menu: &Menu, labels: &[&str]) -> Vec<Option<UiEvent>> {
        let (events, drawn) = click_labels_at(menu, labels, DF_WINDOW);
        assert!(drawn.iter().all(|d| *d), "{labels:?} drawn: {drawn:?}");
        events
    }

    fn df_frame(menu: &Menu, events: Vec<egui::Event>) -> Option<UiEvent> {
        upgrade_frame(menu, DF_WINDOW.0, DF_WINDOW.1, events).1
    }

    #[test]
    fn every_client_shows_on_clipboard_and_compose_without_a_click() {
        let shapes = df_shapes(&deskflow_settings(two_clients()));
        let texts = all_texts(&shapes);
        for want in ["Computer", "On", "Clipboard", "Compose", "mac", "pc"] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        // Two rows of three switches.
        let mut switches = vec![];
        for name in ["mac", "pc"] {
            for heading in ["On", "Clipboard", "Compose"] {
                switches.push(switch_in_column(&shapes, heading, name));
            }
        }
        for (i, a) in switches.iter().enumerate() {
            for b in &switches[i + 1..] {
                assert!(!a.intersects(*b), "two switches overlap: {a:?} {b:?}");
            }
        }
        // The rest is in the dialog, which is closed.
        for gone in ["Position", "Forget mac", "Share the clipboard", "Close"] {
            assert!(!has(&texts, gone), "{gone} in {texts:?}");
        }
    }

    #[test]
    fn a_client_switch_sends_its_event() {
        use crate::deskflowtab::DeskflowEvent;
        let menu = deskflow_settings(two_clients());
        let shapes = df_shapes(&menu);
        let click = |heading: &str| {
            let switch = switch_in_column(&shapes, heading, "mac");
            df_frame(&menu, click_at(switch.center()))
        };
        assert_eq!(
            click("Compose"),
            df_ev(DeskflowEvent::Compose {
                name: "mac".into(),
                on: true
            })
        );
        assert_eq!(
            click("Clipboard"),
            df_ev(DeskflowEvent::Clipboard {
                name: "mac".into(),
                on: false
            })
        );
        assert_eq!(
            click("On"),
            df_ev(DeskflowEvent::Enabled {
                name: "mac".into(),
                on: false
            })
        );
        // The gear right of the switches opens the client's dialog.
        let compose = switch_in_column(&shapes, "Compose", "mac");
        let gear = shapes
            .iter()
            .filter(|c| !matches!(c.shape, egui::Shape::Text(_)))
            .map(|c| c.shape.visual_bounding_rect())
            .filter(|r| r.left() > compose.right() && r.y_range().contains(compose.center().y))
            .min_by(|a, b| a.left().total_cmp(&b.left()))
            .expect("the gear of mac");
        assert_eq!(
            df_frame(&menu, click_at(gear.center())),
            df_ev(DeskflowEvent::Select("mac".into()))
        );
    }

    #[test]
    fn a_pending_client_attaches_through_its_dialog() {
        use crate::deskflowtab::{self, DeskflowEvent};
        use crate::kanshi::Side;
        let menu = deskflow_settings(with_lnx_pending(two_clients()));
        assert!(has(&df_texts(&menu), "lnx wants to attach"));
        assert_eq!(
            df_clicks(&menu, &["Check and attach…"]),
            [df_ev(DeskflowEvent::Attach {
                name: "lnx".into(),
                fp: DF_FP_LNX.into()
            })]
        );
        let mut df = with_lnx_pending(two_clients());
        deskflowtab::apply(
            &mut df,
            DeskflowEvent::Attach {
                name: "lnx".into(),
                fp: DF_FP_LNX.into(),
            },
        );
        let menu = deskflow_settings(df);
        let texts = df_texts(&menu);
        for want in [
            "Check this fingerprint on the other computer before you trust it there.",
            "Attach",
            "Cancel",
        ] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        // No edge chosen yet: Attach does nothing.
        assert_eq!(df_clicks(&menu, &["Attach"]), [None]);
        let events = df_clicks(&menu, &["Choose an edge", "Above DP-1", "Attach"]);
        assert_eq!(events[..2], [None, None], "choosing an edge only marks it");
        assert_eq!(
            events[2],
            df_ev(DeskflowEvent::Place {
                name: "lnx".into(),
                output: "DP-1".into(),
                side: Side::Above
            })
        );
        assert_eq!(df_clicks(&menu, &["Cancel"]), [df_ev(DeskflowEvent::CancelAttach)]);
        assert_eq!(df_frame(&menu, escape_key()), df_ev(DeskflowEvent::CancelAttach));
    }

    /// Whenever a Deskflow dialog counts as open (and the tab under it is
    /// disabled), one is drawn with a button that closes it, and Escape
    /// closes it too: also for a client or a fingerprint that is gone.
    #[test]
    fn every_open_deskflow_dialog_is_drawn_with_its_close_button() {
        use crate::deskflowtab::{self, DeskflowEvent};
        let open = |ev: DeskflowEvent, df: crate::deskflowtab::DeskflowTab| {
            let mut df = df;
            deskflowtab::apply(&mut df, ev);
            assert!(deskflowtab::dialog_open(&df));
            df
        };
        let mut stopped = two_clients();
        stopped.running = false;
        let cases = [
            (
                open(DeskflowEvent::Select("mac".into()), two_clients()),
                vec!["mac", "Position", "Share the clipboard", "Forget mac"],
                "Close",
                DeskflowEvent::CloseDialog,
            ),
            (
                open(DeskflowEvent::Select("gone".into()), two_clients()),
                vec!["gone"],
                "Close",
                DeskflowEvent::CloseDialog,
            ),
            (
                open(
                    DeskflowEvent::Attach {
                        name: "lnx".into(),
                        fp: DF_FP_LNX.into(),
                    },
                    with_lnx_pending(two_clients()),
                ),
                vec!["lnx", "Attach"],
                "Cancel",
                DeskflowEvent::CancelAttach,
            ),
            (
                open(DeskflowEvent::ShowFingerprint, two_clients()),
                vec![
                    "Fingerprint of this computer",
                    "Check this fingerprint on the other computer before you trust it there.",
                ],
                "Close",
                DeskflowEvent::CloseDialog,
            ),
            (
                open(DeskflowEvent::ShowFingerprint, stopped),
                vec!["Fingerprint of this computer"],
                "Close",
                DeskflowEvent::CloseDialog,
            ),
        ];
        for (df, wants, close, closes) in cases {
            let menu = deskflow_settings(df);
            let texts = df_texts(&menu);
            for want in wants {
                assert!(has(&texts, want), "{want} not in {texts:?}");
            }
            assert_eq!(df_clicks(&menu, &[close]), [df_ev(closes.clone())], "{texts:?}");
            assert_eq!(df_frame(&menu, escape_key()), df_ev(closes));
        }
        // The Fingerprint… button opens this computer's dialog.
        assert_eq!(
            df_clicks(&deskflow_settings(two_clients()), &["Fingerprint…"]),
            [df_ev(DeskflowEvent::ShowFingerprint)]
        );
    }

    /// On a 1920x1080 screen, with two monitors, two clients, a pending
    /// client and a full Graphics band, the page scrolls; scrolled to the
    /// Deskflow band, all of its rows are in view at once. The page shows
    /// about 450 points there, which the Monitors band above nearly fills,
    /// so the rows cannot be in view without a scroll.
    #[test]
    fn the_client_rows_are_in_view_on_a_1080_screen() {
        use crate::graphics::{Codec, Gpu, Info, Vaapi};
        let codec = |name| Codec {
            name,
            decode: true,
            encode: true,
        };
        let mut menu = deskflow_settings(with_lnx_pending(two_clients()));
        menu.graphics = Some(Info {
            gpus: vec![Gpu {
                card: "card1".into(),
                ids: (0x8086, 0x1616),
                name: Some("Intel Corporation HD Graphics 5500".into()),
                driver: Some("i915".into()),
            }],
            vaapi: Vaapi::Codecs(vec![codec("H.264"), codec("HEVC"), codec("VP9")]),
        });
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        ctx.global_style_mut(|s| s.animation_time = 0.0);
        let icons: Vec<Option<Icon>> = menu.tiles.iter().map(|_| None).collect();
        let frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen(1920.0, 1080.0)),
                events,
                ..Default::default()
            };
            let mut o = ctx.run_ui(input, |ui| {
                show(ui, &menu, &icons, &Chrome::default(), Instant::now());
            });
            o.textures_delta.clear();
            o.shapes
        };
        frame(vec![]);
        let shapes = frame(vec![]);
        let title = |shapes: &[egui::epaint::ClippedShape]| {
            shapes
                .iter()
                .find_map(|c| match &c.shape {
                    egui::Shape::Text(t) if t.galley.text() == "Deskflow" => {
                        Some((t.galley.rect.translate(t.pos.to_vec2()), c.clip_rect))
                    }
                    _ => None,
                })
                .expect("the Deskflow title")
        };
        let (at, page) = title(&shapes);
        // Scroll the band's title to the top of the page.
        let over = page.center();
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, page.top() - at.top()),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
            source: egui::MouseWheelSource::Wheel,
        };
        frame(vec![egui::Event::PointerMoved(over), wheel]);
        // The scroll animates over a few frames.
        let mut shapes = vec![];
        for _ in 0..30 {
            shapes = frame(vec![egui::Event::PointerMoved(over)]);
        }
        let (at, _) = title(&shapes);
        assert!(page.contains_rect(at), "the title {at:?} outside {page:?}");
        for name in ["mac", "pc"] {
            let switch = switch_in_column(&shapes, "Compose", name);
            assert!(
                page.contains_rect(switch),
                "{name}'s Compose {switch:?} outside {page:?}"
            );
        }
        let reject = text_rects(&shapes)
            .into_iter()
            .find(|(t, _)| t == "Reject")
            .expect("the pending row's Reject")
            .1;
        assert!(page.contains_rect(reject), "Reject {reject:?} outside {page:?}");
    }

    #[test]
    fn an_off_client_box_is_dimmed() {
        use crate::deskflow::statefile::{DevLine, DevState};
        let mut df = two_clients();
        df.cfg.devices[0].enabled = false;
        // Both connected: only switching mac off dims its dot.
        for (name, fp) in [("mac", DF_FP_MAC), ("pc", DF_FP_PC)] {
            df.state.devices.push(DevLine {
                name: name.into(),
                fingerprint: fp.into(),
                state: DevState::Connected,
                since: 1,
                width: 1920,
                height: 1080,
                flags: vec![],
            });
        }
        let shapes = df_shapes(&deskflow_settings(df));
        // Each name is drawn first in its box, then in its row of the list.
        let colours = |want: &str| -> Vec<Color32> {
            shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) if t.galley.text() == want => Some(t.galley.job.sections[0].format.color),
                    _ => None,
                })
                .collect()
        };
        let (mac, pc) = (colours("mac"), colours("pc"));
        assert_eq!(mac.len(), 2, "mac in its box and its row");
        assert_eq!(mac, [MUTED, MUTED], "box and row name of an off client are muted");
        assert_ne!(pc[0], MUTED);
        assert_ne!(pc[1], MUTED);
        // The dot in the box: muted for the off client, as the row's for the on one.
        let dot_in = |label: &str| {
            let b = box_around(&shapes, label);
            shapes
                .iter()
                .find_map(|c| match &c.shape {
                    egui::Shape::Circle(d) if b.contains(d.center) => Some(d.fill),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no dot in the box of {label}"))
        };
        assert_eq!(dot_in("mac"), MUTED);
        assert_ne!(dot_in("pc"), MUTED, "a connected client's dot is not muted");
    }

    /// Without a saved choice the monitor the menu is on now carries the
    /// Menu marks, in its box and in its row.
    #[test]
    fn the_menu_marks_follow_the_menu_without_a_choice() {
        let mut mon = two_monitors();
        assert_eq!(mon.menu_choice(), None);
        let texts = mon_texts(&monitor_settings(mon.clone()));
        assert!(!has(&texts, "· Menu"), "no mark while the menu's monitor is unknown");
        mon.menu_is_on(Some("HDMI-A-1"));
        let shapes = mon_shapes(&monitor_settings(mon));
        let rects = text_rects(&shapes);
        let at = |want: &str| {
            rects
                .iter()
                .find(|(t, _)| t == want)
                .unwrap_or_else(|| panic!("{want} not drawn"))
                .1
        };
        assert!(box_around(&shapes, "HDMI-A-1").contains(at("Menu").center()));
        // The row mark sits on the row of HDMI-A-1 (its second label).
        let row = rects
            .iter()
            .filter(|(t, _)| t == "HDMI-A-1")
            .nth(1)
            .expect("the row of HDMI-A-1")
            .1;
        assert!(row.y_range().contains(at("· Menu").center().y));
    }

    /// The attach dialog closed some other way than Attach, Cancel or
    /// Escape (here: a frame without it), it opens again with no edge chosen.
    #[test]
    fn the_attach_dialog_opens_again_with_no_edge_chosen() {
        use crate::deskflowtab::{self, DeskflowEvent};
        let pending = with_lnx_pending(two_clients());
        let mut attaching = pending.clone();
        deskflowtab::apply(
            &mut attaching,
            DeskflowEvent::Attach {
                name: "lnx".into(),
                fp: DF_FP_LNX.into(),
            },
        );
        let (open, closed) = (deskflow_settings(attaching), deskflow_settings(pending));
        let ctx = egui::Context::default();
        crate::brand::apply(&ctx);
        let frame = |menu: &Menu, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen(DF_WINDOW.0, DF_WINDOW.1)),
                events,
                time: Some(ctx.cumulative_pass_nr() as f64 * 0.1),
                ..Default::default()
            };
            let icons: Vec<Option<Icon>> = menu.tiles.iter().map(|_| None).collect();
            let mut out = ctx.run_ui(input, |ui| {
                show(ui, menu, &icons, &Chrome::default(), Instant::now());
            });
            out.textures_delta.clear();
            text_rects(&out.shapes)
        };
        let settle = |menu: &Menu| {
            let mut texts = vec![];
            for _ in 0..3 {
                texts = frame(menu, vec![]);
            }
            texts
        };
        let shown = |texts: &[(String, Rect)], want: &str| texts.iter().any(|(t, _)| t == want);
        let mut texts = settle(&open);
        for label in ["Choose an edge", "Above DP-1"] {
            let at = texts
                .iter()
                .find(|(t, _)| t == label)
                .unwrap_or_else(|| panic!("{label} not drawn"))
                .1
                .center();
            frame(&open, click_at(at));
            texts = settle(&open);
        }
        assert!(shown(&texts, "Above DP-1") && !shown(&texts, "Choose an edge"));
        settle(&closed);
        let texts = settle(&open);
        assert!(shown(&texts, "Choose an edge"), "no edge chosen on reopening");
        assert!(!shown(&texts, "Above DP-1"));
    }

    fn monitor_settings(mon: MonTab) -> Menu {
        let mut menu = screen_settings();
        menu.monitors = mon;
        menu
    }

    fn mon_ev(e: MonEvent) -> Option<UiEvent> {
        Some(UiEvent::Settings(SettingsEvent::Monitors(e)))
    }

    /// The monitor tests' window: a 1080 screen, on which the Position row
    /// must be in view without a scroll.
    const MON_WINDOW: (f32, f32) = (1920.0, 1080.0);

    fn mon_shapes(menu: &Menu) -> Vec<egui::epaint::ClippedShape> {
        settings_shapes(menu, MON_WINDOW.0, MON_WINDOW.1)
    }

    fn mon_texts(menu: &Menu) -> Vec<String> {
        all_texts(&mon_shapes(menu))
    }

    fn mon_clicks(menu: &Menu, labels: &[&str]) -> Vec<Option<UiEvent>> {
        let (events, drawn) = click_labels_at(menu, labels, MON_WINDOW);
        assert!(drawn.iter().all(|d| *d), "{labels:?} drawn: {drawn:?}");
        events
    }

    /// The smallest rect around the first `label` drawn: its monitor box.
    fn box_around(shapes: &[egui::epaint::ClippedShape], label: &str) -> Rect {
        let at = text_rects(shapes)
            .into_iter()
            .find(|(t, _)| t == label)
            .unwrap_or_else(|| panic!("{label} not in {:?}", all_texts(shapes)))
            .1
            .center();
        shapes
            .iter()
            .flat_map(|c| flat_rects(&c.shape))
            .map(|r| r.rect)
            .filter(|r| r.contains(at))
            .min_by(|a, b| a.area().total_cmp(&b.area()))
            .unwrap()
    }

    #[test]
    fn monitor_boxes_have_the_monitors_proportions() {
        let shapes = mon_shapes(&monitor_settings(two_monitors()));
        let small = box_around(&shapes, "HDMI-A-1");
        let big = box_around(&shapes, "DP-1");
        // Each box is drawn BOX_GAP smaller than its place, so they never touch.
        let gap = crate::monitors_ui::BOX_GAP;
        let wide = (big.width() + gap) / (small.width() + gap);
        let tall = (big.height() + gap) / (small.height() + gap);
        assert!((wide - 4.0 / 3.0).abs() < 0.01, "width ratio {wide}: {small:?} {big:?}");
        assert!(
            (tall - 4.0 / 3.0).abs() < 0.01,
            "height ratio {tall}: {small:?} {big:?}"
        );
        assert!(
            big.left() >= small.right(),
            "DP-1 is right of HDMI-A-1: {small:?} {big:?}"
        );
        assert!(
            (big.top() - small.top()).abs() < 0.5,
            "aligned at the top: {small:?} {big:?}"
        );
        let texts = all_texts(&shapes);
        for want in ["Monitors", "P2419H", "U2720Q", "On", "Resolution", "Apply"] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        assert!(has(&texts, "1920 × 1080 @ 60 Hz"), "{texts:?}");
    }

    #[test]
    fn a_click_on_a_monitor_box_selects_it() {
        let menu = monitor_settings(two_monitors());
        assert_eq!(mon_clicks(&menu, &["DP-1"]), [mon_ev(MonEvent::Select("DP-1".into()))]);
    }

    #[test]
    fn the_menu_monitor_cannot_be_switched_off() {
        let now = Instant::now();
        let mut mon = monitor_dialog("HDMI-A-1");
        mon.event(MonEvent::MenuHere, now);
        mon.event(MonEvent::CloseDialog, now);
        let menu = monitor_settings(mon);
        let shapes = mon_shapes(&menu);
        // The box of the menu monitor says so, and so does its row.
        let menu_word = text_rects(&shapes)
            .into_iter()
            .find(|(t, _)| t == "Menu")
            .expect("the Menu mark")
            .1;
        assert!(box_around(&shapes, "HDMI-A-1").contains(menu_word.center()));
        assert!(has(&all_texts(&shapes), "· Menu"));
        let switch = switch_in_row(&shapes, "HDMI-A-1");
        assert_eq!(upgrade_frame(&menu, 1920.0, 1080.0, click_at(switch.center())).1, None);
        // The other monitor can go off, from its row.
        let switch = switch_in_row(&shapes, "DP-1");
        assert_eq!(
            upgrade_frame(&menu, 1920.0, 1080.0, click_at(switch.center())).1,
            mon_ev(MonEvent::Enabled("DP-1".into(), false))
        );
    }

    #[test]
    fn show_the_menu_here_and_apply_report_their_clicks() {
        let menu = monitor_settings(monitor_dialog("HDMI-A-1"));
        assert_eq!(mon_clicks(&menu, &["Show the menu here"]), [mon_ev(MonEvent::MenuHere)]);
        let mut mon = monitor_dialog("HDMI-A-1");
        mon.event(MonEvent::MenuHere, Instant::now());
        mon.event(MonEvent::CloseDialog, Instant::now());
        let menu = monitor_settings(mon);
        assert_eq!(mon_clicks(&menu, &["Apply"]), [mon_ev(MonEvent::Apply)]);
    }

    #[test]
    fn apply_needs_a_change_and_a_position_change_is_one() {
        // Nothing changed yet: Apply is off.
        assert_eq!(mon_clicks(&monitor_settings(two_monitors()), &["Apply"]), [None]);
        // The Position row of the dialog: the side select shows where the
        // monitor sits; its list offers the other sides.
        let menu = monitor_settings(monitor_dialog("HDMI-A-1"));
        let texts = mon_texts(&menu);
        let current = ["Left of", "Right of", "Above", "Below"]
            .into_iter()
            .find(|l| has(&texts, l))
            .expect("a side in the Position row");
        let other = if current == "Above" { "Below" } else { "Above" };
        let events = mon_clicks(&menu, &[current, other]);
        assert_eq!(events[0], None, "opening the list is no change");
        let Some(Some(UiEvent::Settings(SettingsEvent::Monitors(MonEvent::Place(side, to))))) = events.get(1).cloned()
        else {
            panic!("no Place in {events:?}");
        };
        assert_eq!(
            side,
            if other == "Above" {
                crate::kanshi::Side::Above
            } else {
                crate::kanshi::Side::Below
            }
        );
        let mut mon = monitor_dialog("HDMI-A-1");
        mon.event(MonEvent::Place(side, to), Instant::now());
        mon.event(MonEvent::CloseDialog, Instant::now());
        let menu = monitor_settings(mon);
        assert_eq!(mon_clicks(&menu, &["Apply"]), [mon_ev(MonEvent::Apply)]);
    }

    #[test]
    fn apply_is_off_while_busy_or_with_every_monitor_off() {
        let now = Instant::now();
        // Keep pressed before the Apply job ended: the job runs, no question.
        let mut mon = two_monitors();
        mon.event(MonEvent::Apply, now);
        mon.event(MonEvent::Keep, now);
        assert!(mon.busy() && !crate::monitors_ui::dialog_open(&mon));
        assert_eq!(mon_clicks(&monitor_settings(mon), &["Apply"]), [None]);
        // Every monitor reported off: nothing to apply.
        let mut mon = MonTab::default();
        let mut heads = mon_heads();
        for h in &mut heads {
            h.enabled = false;
        }
        mon.heads_changed(heads);
        mon.loaded(Ok(None), crate::monitors::MonSource::Missing);
        assert!(mon.editable());
        assert_eq!(mon_clicks(&monitor_settings(mon), &["Apply"]), [None]);
    }

    /// Two monitors, applied: *Keep this arrangement?* counts down.
    fn asking() -> MonTab {
        let mut mon = two_monitors();
        mon.event(MonEvent::Apply, Instant::now());
        mon
    }

    #[test]
    fn the_keep_dialog_counts_down_and_answers() {
        let menu = monitor_settings(asking());
        assert!(menu.dialog(), "the question freezes and dims the screen behind it");
        let texts = mon_texts(&menu);
        for want in ["Keep this arrangement?", "Going back in 15 seconds.", "Keep", "Go back"] {
            assert!(has(&texts, want), "{want} not in {texts:?}");
        }
        // Only the dialog is drawn: the tab under it takes no clicks.
        assert!(!has(&texts, "Show the menu here"), "{texts:?}");
        assert_eq!(mon_clicks(&menu, &["Keep"]), [mon_ev(MonEvent::Keep)]);
        assert_eq!(mon_clicks(&menu, &["Go back"]), [mon_ev(MonEvent::GoBack)]);
    }

    #[test]
    fn escape_goes_back() {
        let escape = vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        let menu = monitor_settings(asking());
        assert_eq!(
            upgrade_frame(&menu, 1920.0, 1080.0, escape.clone()).1,
            mon_ev(MonEvent::GoBack)
        );
        // Without the question, Escape does nothing on the tab.
        let menu = monitor_settings(two_monitors());
        assert_eq!(upgrade_frame(&menu, 1920.0, 1080.0, escape).1, None);
    }

    #[test]
    fn the_screen_tab_says_when_thinlinc_takes_the_arrangement() {
        let note = "ThinLinc uses the new arrangement from its next start.";
        let mut mon = two_monitors();
        assert!(!has(&mon_texts(&monitor_settings(mon.clone())), note));
        mon.set_thinlinc(true);
        let texts = mon_texts(&monitor_settings(mon));
        assert!(has(&texts, note), "{texts:?}");
    }

    #[test]
    fn a_hand_written_kanshi_file_is_shown_with_the_take_over() {
        let mut mon = MonTab::default();
        mon.heads_changed(mon_heads());
        mon.loaded(Ok(None), crate::monitors::MonSource::Kept);
        let menu = monitor_settings(mon);
        let texts = mon_texts(&menu);
        let line = "The monitor arrangement is a kanshi file written by hand, which the menu keeps as it is.";
        assert!(has(&texts, line), "{texts:?}");
        // The monitors and their rows are shown, but nothing is offered
        // to change them.
        assert!(has(&texts, "DP-1"), "{texts:?}");
        assert!(has(&texts, "Resolution"), "{texts:?}");
        for gone in ["Apply", "Show the menu here"] {
            assert!(!has(&texts, gone), "{gone} in {texts:?}");
        }
        let switch = switch_in_row(&mon_shapes(&menu), "DP-1");
        assert_eq!(upgrade_frame(&menu, 1920.0, 1080.0, click_at(switch.center())).1, None);
        assert_eq!(mon_clicks(&menu, &["DP-1"]), [None], "a box opens no dialog");
        assert_eq!(
            mon_clicks(&menu, &["Let the menu manage it"]),
            [mon_ev(MonEvent::TakeOver)]
        );
    }

    #[test]
    fn monitor_errors_are_shown_in_warn() {
        let now = Instant::now();
        let refused = "An earlier monitor arrangement is not confirmed yet; nothing was changed.";
        let mut not_applied = two_monitors();
        not_applied.event(MonEvent::Apply, now);
        not_applied.applied(Err(refused.into()));
        let mut not_kept = asking();
        not_kept.applied(Ok(()));
        not_kept.event(MonEvent::Keep, now);
        not_kept.kept(Err("disk full".into()));
        for (mon, want) in [
            (not_applied, format!("The arrangement was not applied: {refused}")),
            (
                not_kept,
                "The arrangement could not be confirmed: disk full. It goes back at the next login.".to_string(),
            ),
        ] {
            assert_eq!(mon.error(), Some(want.as_str()));
            let shapes = mon_shapes(&monitor_settings(mon));
            let color = shapes.iter().find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text() == want => Some(t.galley.job.sections[0].format.color),
                _ => None,
            });
            assert_eq!(color, Some(WARN), "{want} in {:?}", all_texts(&shapes));
        }
    }

    #[test]
    #[ignore]
    fn shot_screen_monitors() {
        // DP-1 carrying the menu, then with its dialog open.
        let mut two = monitor_dialog("DP-1");
        two.event(MonEvent::MenuHere, Instant::now());
        shot(
            &monitor_settings(two.clone()),
            "screen-monitors-dialog.ppm",
            (1920, 1080),
            1.0,
        );
        two.event(MonEvent::CloseDialog, Instant::now());
        shot(&monitor_settings(two), "screen-monitors-two.ppm", (1920, 1080), 1.0);
        let mut keep = asking();
        keep.set_thinlinc(true);
        shot(&monitor_settings(keep), "screen-monitors-keep.ppm", (1920, 1080), 1.0);
        let mut hand = MonTab::default();
        hand.heads_changed(mon_heads());
        hand.loaded(Ok(None), crate::monitors::MonSource::Kept);
        shot(&monitor_settings(hand), "screen-monitors-hand.ppm", (1920, 1080), 1.0);
        // The Deskflow band with two clients, one off, and one that wants
        // to attach; then the dialogs.
        let mut df = with_lnx_pending(two_clients());
        df.cfg.devices[1].enabled = false;
        shot(&deskflow_settings(df.clone()), "screen-deskflow.ppm", (1920, 1400), 1.0);
        for (ev, name) in [
            (
                crate::deskflowtab::DeskflowEvent::Select("mac".into()),
                "screen-deskflow-client.ppm",
            ),
            (
                crate::deskflowtab::DeskflowEvent::Attach {
                    name: "lnx".into(),
                    fp: DF_FP_LNX.into(),
                },
                "screen-deskflow-attach.ppm",
            ),
            (
                crate::deskflowtab::DeskflowEvent::ShowFingerprint,
                "screen-deskflow-self.ppm",
            ),
        ] {
            let mut open = df.clone();
            crate::deskflowtab::apply(&mut open, ev);
            shot(&deskflow_settings(open), name, (1920, 1080), 1.0);
        }
    }

    /// The Network and Screen tabs as the web page shows them, at the size
    /// and scale of its pictures from a running system (site/README.md).
    #[test]
    #[ignore]
    fn shot_page_pictures() {
        let mut two = monitor_dialog("DP-1");
        two.event(MonEvent::MenuHere, Instant::now());
        two.event(MonEvent::CloseDialog, Instant::now());
        shot(&monitor_settings(two), "page-screen.ppm", (2880, 1920), 2.0);
        let mut menu = open_settings();
        menu.settings_tab = crate::settings::SettingsTab::Network;
        menu.network = two_networks();
        menu.network.status_done(
            Ok(vec![crate::nettab::Iface {
                name: "enp1s0".into(),
                kind: crate::nettab::IfKind::Wired,
                state: crate::nettab::IfState::Connected,
                addresses: vec!["192.168.1.10/24".into()],
                ssid: None,
                signal_dbm: None,
                handshake_age_s: None,
            }]),
            Default::default(),
        );
        shot(&menu, "page-network.ppm", (2880, 1920), 2.0);
    }

    /// One element by itself at twice its size, cells `cell_h` high.
    fn shot_element(
        name: &str,
        element: &dyn crate::status::Element,
        size: crate::status::Size,
        cell_h: f32,
        data: &crate::status::Data,
    ) {
        let Some(dir) = std::env::var_os("TF_SHOT_DIR") else {
            return;
        };
        let s = 2.0;
        let rect = crate::status::Cells {
            col: 0,
            row: 0,
            w: size.w,
            h: size.h,
        }
        .rect(pos2(0.0, 0.0), vec2(220.0, cell_h), TILE_GAP);
        let px = ((rect.width() * s) as usize, (rect.height() * s) as usize);
        let ctx = new_ctx();
        ctx.set_zoom_factor(s);
        let mut textures = crate::raster::Textures::default();
        let mut canvas = crate::raster::Canvas::new(px.0, px.1, crate::brand::rgb::BG);
        for _ in 0..4 {
            let input = egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
                child.set_clip_rect(rect);
                element.draw(&mut child, rect, size, data, Instant::now());
            });
            textures.apply_set(&out.textures_delta);
            out.textures_delta.clear();
            let prims = ctx.tessellate(out.shapes, s);
            canvas.restore_background(canvas.bounds());
            canvas.draw(&prims, &textures, s, canvas.bounds());
        }
        let mut ppm = format!("P6\n{} {}\n255\n", px.0, px.1).into_bytes();
        for p in &canvas.pixels {
            ppm.extend([(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.ppm")), ppm).unwrap();
    }

    /// Every status element by itself, at each size it offers, with sample
    /// data.
    fn shot_elements() {
        use crate::status::{Data, ELEMENTS};
        let devices = vec![
            kdev("MX Mechanical", battery::DeviceKind::Keyboard, 100, "discharging"),
            kdev("MX Anywhere 3S", battery::DeviceKind::Mouse, 60, "charging"),
            dev("Pen", 7, "discharging"),
        ];
        let net = crate::status::network::sample(300);
        let hardware = crate::status::hardware::sample();
        let data = Data {
            battery: &devices,
            battery_since: Some(Instant::now()),
            network: Some(&net),
            hardware: Some(&hardware),
            wall: Some(std::time::SystemTime::now()),
            zone: None,
            public_ip: None,
        };
        for e in ELEMENTS {
            for size in e.sizes() {
                for h in [TILE_H, TILE_H_MIN] {
                    shot_element(&format!("{}-{size}-{h}", e.id()), *e, *size, h, &data);
                }
            }
        }
    }
}
