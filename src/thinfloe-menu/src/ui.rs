//! Drawing the grid, the Settings screen in its place, the log under them,
//! and the dialogs. Everything that decides something lives in menu.rs; this
//! file only paints it and reports clicks.

use crate::brand::{self, role};
use crate::density::GROUP_W;
use crate::header::{Header, header_text, until_next_minute};
use crate::icons::{self, IconJob, IconRoots};
use crate::log::{Ansi, Kind, Line, Log, Run};
use crate::menu::{Choice, Logout, Menu, Screen};
use crate::settings::{self, RowButton, SettingsEvent};
use crate::tiles::{Action, Origin, Tile};
use crate::upgrade::UpgradeEvent;
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align2, Color32, ColorImage, FontId, Rect, Sense, Stroke, StrokeKind, TextFormat, TextureHandle, Ui, UiBuilder,
    Vec2, pos2, vec2,
};
use shadcn_egui::Theme;
use shadcn_egui::components::button::{Button, ButtonVariant};
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
}

/// Where the header, the grid and the log go on a screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// The strip along the top: logo, name, host and clock.
    pub header: Rect,
    /// The log, along the bottom.
    pub log: Rect,
    /// The space between header and log; the grid scrolls inside it.
    pub area: Rect,
    /// The grid itself, centred in `area`, or from its top when taller.
    pub grid: Rect,
    /// The size of one band.
    pub tile: Vec2,
    /// The row of folded-window chips at the bottom edge; `None` when no
    /// window is folded.
    pub strip: Option<Rect>,
}

pub fn layout(screen: Rect, tiles: usize, folded: usize) -> Layout {
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
    let fit_h = (area.height() - 2.0 * GAP - (rows as f32 - 1.0) * TILE_GAP) / rows as f32;
    let tile = vec2(tile_w, fit_h.clamp(TILE_H_MIN, TILE_H));
    let size = vec2(
        cols as f32 * tile.x + (cols as f32 - 1.0) * TILE_GAP,
        rows as f32 * tile.y + (rows as f32 - 1.0) * TILE_GAP,
    );
    let left = (screen.center().x - size.x / 2.0).max(area.left() + GAP.min(area.width() / 2.0));
    let top = area.top() + ((area.height() - size.y) / 2.0).max(GAP);
    Layout {
        header,
        log,
        area,
        grid: Rect::from_min_size(pos2(left, top), size),
        tile,
        strip,
    }
}

/// What the header shows besides the grid: the names, the clock and the
/// logo texture. The logo is `None` in tests that draw without one.
#[derive(Default)]
pub struct Chrome {
    pub header: Header,
    /// `HH:MM`; the app refreshes it before every frame.
    pub clock: String,
    pub logo: Option<TextureHandle>,
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
    ui.ctx()
        .request_repaint_after(until_next_minute(std::time::SystemTime::now()));
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
            let what = root_waits(addons, write);
            dialog(ui, &format!("Waiting for {what} before {verbing}"), |ui, t| {
                for job in menu.root_jobs() {
                    ui.label(egui::RichText::new(job).color(t.palette().muted_foreground));
                }
                // Stopping a fresh install leaves a stick that does not start.
                if menu.ending != Action::Logout
                    && menu.upgrade.fresh_running()
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
    let shown = menu.visible_tiles();
    let l = layout(ui.max_rect(), shown.len(), menu.folded.len());
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
    if menu.screen == Screen::Upgrade {
        let event = crate::upgrade_ui::show(ui, menu, l.area);
        show_log(ui, &menu.log, l.log);
        return event.map(UiEvent::Upgrade).or(unfold);
    }
    let mut clicked = None;
    let mut grid_ui = ui.new_child(UiBuilder::new().max_rect(l.area));
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
    show_log(ui, &menu.log, l.log);
    clicked.or(unfold)
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
                .font(brand::geo(brand::SETTINGS_TITLE_PT))
                .color(t.palette().foreground),
        );
        ui.add_space(12.0);
        // SAFETY: time(NULL) has no preconditions.
        let now = unsafe { libc::time(std::ptr::null_mut()) } as i64;
        ui.label(egui::RichText::new(settings::last_checked(menu.last_check, now)).color(t.palette().muted_foreground));
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

/// A centred card over the frozen grid.
fn dialog(ui: &Ui, title: &str, body: impl FnOnce(&mut Ui, &Theme)) {
    let t = Theme::current(ui.ctx());
    egui::Area::new(egui::Id::unique("dialog"))
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(t.palette().card)
                .stroke(Stroke::new(1.0, t.palette().border))
                .inner_margin(24.0)
                .show(ui, |ui| {
                    ui.set_width(460.0);
                    ui.label(egui::RichText::new(title).font(brand::geo(brand::DIALOG_TITLE_PT)));
                    body(ui, &t);
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brand::rgb::*;

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
        let header = layout(screen(1920.0, 1080.0), 0, 0).header;
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
            let l = layout(screen(w, h), 8, 0);
            assert_eq!(l.tile, vec2(220.0, 76.0), "{w}x{h}: {:?}", l.tile);
            assert!((l.grid.width() - GROUP_W).abs() < 0.5, "{w}x{h}: {:?}", l.grid);
            assert!((l.grid.center().x - w / 2.0).abs() < 1.0, "{w}x{h}: {:?}", l.grid);
            assert!(l.grid.top() > l.header.bottom() && l.grid.bottom() < l.log.top());
            assert!(l.log.bottom() <= h && l.log.left() >= 0.0 && l.log.right() <= w);
        }
    }

    #[test]
    fn the_group_never_exceeds_the_screen() {
        let l = layout(screen(900.0, 600.0), 8, 0);
        assert!(l.grid.left() >= 0.0 && l.grid.right() <= 900.0, "{:?}", l.grid);
    }

    #[test]
    fn folding_a_window_takes_only_from_the_log() {
        let open = layout(screen(1920.0, 1080.0), 8, 0);
        let folded = layout(screen(1920.0, 1080.0), 8, 1);
        assert_eq!(open.area, folded.area);
        assert_eq!(open.grid, folded.grid);
        assert_eq!(open.log.top(), folded.log.top());
        assert_eq!(folded.log.bottom(), folded.strip.unwrap().top() - STRIP_GAP);
    }

    #[test]
    fn the_log_keeps_its_minimum_and_the_area_gives_the_rest() {
        // 0.3 * 600 = 180 is 16 above the minimum, less than STRIP_H + STRIP_GAP.
        let min = LOG_MIN_ROWS as f32 * LOG_ROW + 2.0 * LOG_PAD;
        let open = layout(screen(1024.0, 600.0), 8, 0);
        let folded = layout(screen(1024.0, 600.0), 8, 1);
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
        let l = layout(screen(1280.0, 720.0), 9, 0);
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
        let l = layout(screen(1280.0, 480.0), 6, 0);
        assert!(l.log.height() >= LOG_MIN_ROWS as f32 * LOG_ROW, "{:?}", l.log);
    }

    #[test]
    fn many_tiles_start_at_the_top_and_overflow_downwards() {
        let l = layout(screen(1920.0, 1080.0), 38, 0);
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
        let l = layout(screen(1280.0, 720.0), names.len(), 0);
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
        let l = layout(screen(1280.0, 720.0), 3, 0);
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
        let l = layout(screen(1920.0, 1080.0), 8, 0);
        assert!(l.strip.is_none());
        assert!(1080.0 - l.log.bottom() <= GAP + 0.5);
    }

    #[test]
    fn a_folded_window_puts_the_strip_below_the_log() {
        let l = layout(screen(1920.0, 1080.0), 8, 2);
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
        let strip = layout(screen(1280.0, 720.0), 1, 10).strip.unwrap();
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
        let strip = layout(screen(1280.0, 720.0), 1, 10).strip.unwrap();
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
        shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Rect(r)
                    if r.rect.contains_rect(rect) && r.rect.height() < 60.0 && r.fill != Color32::TRANSPARENT =>
                {
                    Some(r.fill)
                }
                _ => None,
            })
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
        let log = layout(screen(w, h), menu.visible_tiles().len(), 0).log;
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
        let (mut menu, _) = four_addons();
        let settings = menu.tiles.iter().position(|t| t.name == "Settings").unwrap();
        menu.activate(settings, Instant::now());
        shot(&menu, "settings-1080.ppm", (1920, 1080), 1.0);
        shot(&menu, "settings-21in-4k.ppm", (3840, 2160), 2.25);
        let mut newer = Menu::new(shipped_grid(), false);
        newer.set_usb_boot(true);
        newer.set_installed(Some("4.0.0".into()));
        newer.upgrade.release_file(crate::release::Release {
            latest: Some("4.1.0".into()),
            ..Default::default()
        });
        shot(&newer, "desktop-1080-upgrade-marker.ppm", (1920, 1080), 1.0);
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
            ("switch", "Switching to the new release"),
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
}
