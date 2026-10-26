//! Drawing the grid, the Settings screen in its place, the log under them,
//! and the dialogs. Everything that decides something lives in menu.rs; this
//! file only paints it and reports clicks.

use crate::brand::{self, GOOD, ICE, MUTED, TILE, TILE_ACTIVE, TILE_HOVER, TIME};
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
/// The tile group takes this share of the screen width...
pub const GROUP_SHARE: f32 = 0.58;
/// ...but never less than this, so a name and a comment fit on a small
/// screen.
pub const GROUP_MIN: f32 = 4.0 * 220.0 + 3.0 * TILE_GAP;
/// A band is this tall, down to `TILE_H_MIN` when the screen is short.
pub const TILE_H: f32 = 76.0;
pub const TILE_H_MIN: f32 = 64.0;
/// The header: 16 above the 52 tall logo, 12 below.
pub const HEADER_H: f32 = 80.0;
const HEADER_PAD_X: f32 = 32.0;
const HEADER_PAD_TOP: f32 = 16.0;
pub const LOGO_H: f32 = 52.0;
const ICON_BOX: f32 = 44.0;
const EDGE_W: f32 = 5.0;
/// The log takes this share of the screen height, and at least
/// `LOG_MIN_ROWS` rows.
pub const LOG_SHARE: f32 = 0.3;
pub const LOG_MIN_ROWS: usize = 8;
/// One log row, in points; the log draws one line per row.
pub const LOG_ROW: f32 = 18.0;
const LOG_PAD: f32 = 10.0;
const LOG_FONT: f32 = 13.0;
pub const ICON_SIDE: u32 = 96;
pub use crate::brand::WARN;

/// Each tile's icon, rasterised in renderer processes as the menu draws it:
/// `None` when the tile names no icon that resolves, otherwise the path and
/// the image or why there is none. Both draw the letter.
pub fn render_icons(tiles: &[Tile], roots: &IconRoots) -> Vec<Option<(PathBuf, Result<ColorImage, String>)>> {
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
                    side: ICON_SIDE,
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

/// Rasterise every tile's icon once, at startup. `None` draws the letter.
pub fn load_icons(ctx: &egui::Context, tiles: &[Tile], roots: &IconRoots) -> Vec<Option<TextureHandle>> {
    render_icons(tiles, roots)
        .into_iter()
        .zip(tiles)
        .map(|(icon, tile)| match icon {
            Some((_, Ok(img))) => Some(ctx.load_texture(tile.file.clone(), img, egui::TextureOptions::LINEAR)),
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
pub fn load_icon(ctx: &egui::Context, tile: &Tile, roots: &IconRoots) -> Option<TextureHandle> {
    load_icons(ctx, std::slice::from_ref(tile), roots).pop().flatten()
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
}

pub fn layout(screen: Rect, tiles: usize) -> Layout {
    let log_h = (screen.height() * LOG_SHARE).max(LOG_MIN_ROWS as f32 * LOG_ROW + 2.0 * LOG_PAD);
    let log = Rect::from_min_max(
        pos2(screen.left() + GAP, screen.bottom() - GAP - log_h),
        pos2(screen.right() - GAP, screen.bottom() - GAP),
    );
    let header = Rect::from_min_size(screen.min, vec2(screen.width(), HEADER_H));
    let area = Rect::from_min_max(pos2(screen.left(), header.bottom()), pos2(screen.right(), log.top()));
    let cols = COLUMNS.min(tiles.max(1));
    let rows = tiles.max(1).div_ceil(COLUMNS);
    let group = (GROUP_SHARE * screen.width())
        .max(GROUP_MIN)
        .min(screen.width() - 2.0 * GAP);
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
    p.text(pos2(x, mid), Align2::LEFT_CENTER, "ThinFloe", brand::geo(34.0), dim(fg));
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
pub fn show(
    ui: &mut Ui,
    menu: &Menu,
    icons: &[Option<TextureHandle>],
    chrome: &Chrome,
    now: Instant,
) -> Option<UiEvent> {
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
    let l = layout(ui.max_rect(), shown.len());
    show_header(ui, chrome, l.header);
    if menu.screen == Screen::Settings {
        let event = show_settings(ui, menu, icons, l.area);
        show_log(ui, &menu.log, l.log);
        return event.map(UiEvent::Settings);
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
    clicked
}

/// The side of a row's icon on the Settings screen.
const ROW_ICON: f32 = 40.0;

/// The Settings screen in `area`: the add-ons, one row each.
fn show_settings(ui: &mut Ui, menu: &Menu, icons: &[Option<TextureHandle>], area: Rect) -> Option<SettingsEvent> {
    let t = Theme::current(ui.ctx());
    let mut event = None;
    let mut s_ui = ui.new_child(UiBuilder::new().max_rect(area.shrink(GAP)));
    s_ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Add-ons").size(20.0).color(t.palette.foreground));
        ui.add_space(12.0);
        // SAFETY: time(NULL) has no preconditions.
        let now = unsafe { libc::time(std::ptr::null_mut()) } as i64;
        ui.label(egui::RichText::new(settings::last_checked(menu.last_check, now)).color(t.palette.muted_foreground));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(Button::new("Back").variant(ButtonVariant::Secondary)).clicked() {
                event = Some(SettingsEvent::Back);
            }
            if ui.add_enabled(!menu.checking(), Button::new("Check now")).clicked() {
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
            for row in rows {
                egui::Frame::new()
                    .fill(t.palette.card)
                    .stroke(Stroke::new(1.0, t.palette.border))
                    .corner_radius(t.radius_md())
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if let Some(e) = settings_row(ui, &row, icons.get(row.tile).and_then(Option::as_ref), &t) {
                            event = Some(e);
                        }
                    });
                ui.add_space(8.0);
            }
        });
    event
}

/// One add-on: tick box, icon, name, source, state, and its button.
fn settings_row(ui: &mut Ui, row: &settings::Row, icon: Option<&TextureHandle>, t: &Theme) -> Option<SettingsEvent> {
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
            Some(tex) => {
                ui.painter().image(
                    tex.id(),
                    rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                ui.painter().rect_filled(rect, t.radius_sm(), t.palette.muted);
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
                    FontId::proportional(ROW_ICON * 0.5),
                    t.palette.foreground,
                );
            }
        }
        // The button first from the right, so the texts get what is left
        // and a long source is cut instead of pushing it off the row.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = match row.button {
                Some(RowButton::InstallNow) => Some("Install now"),
                Some(RowButton::UpdateNow) => Some("Update now"),
                None => None,
            };
            if let Some(label) = label
                && ui.add_enabled(!row.busy, Button::new(label)).clicked()
            {
                event = Some(match row.button {
                    Some(RowButton::UpdateNow) => SettingsEvent::UpdateNow(row.id.clone()),
                    _ => SettingsEvent::InstallNow(row.id.clone()),
                });
            }
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                let one_line = |ui: &mut Ui, text: &str, size: f32, color: Color32| {
                    let mut job = LayoutJob::simple_singleline(text.to_string(), FontId::proportional(size), color);
                    job.wrap = TextWrapping::truncate_at_width(ui.available_width());
                    ui.label(job);
                };
                one_line(ui, &row.name, 16.0, t.palette.foreground);
                if let Some(source) = &row.source {
                    one_line(ui, source, 13.0, t.palette.muted_foreground);
                }
                let state = if row.busy {
                    format!("{} · working…", row.state)
                } else {
                    row.state.clone()
                };
                one_line(ui, &state, 13.0, t.palette.foreground);
                if row.manual {
                    one_line(ui, settings::MANUAL_NOTE, 12.0, WARN);
                }
            });
        });
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

fn paint_tile(ui: &Ui, rect: Rect, tile: &Tile, icon: Option<&TextureHandle>, state: TileState) {
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
        Some(tex) => {
            p.image(
                tex.id(),
                icon_rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                fade(Color32::WHITE),
            );
        }
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
                .fill(t.palette.card)
                .stroke(Stroke::new(1.0, t.palette.border))
                .corner_radius(t.radius_lg())
                .inner_margin(24.0)
                .show(ui, |ui| {
                    ui.set_width(460.0);
                    ui.label(egui::RichText::new(title).size(18.0));
                    body(ui, &t);
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn eight_tiles_on_full_hd_are_two_rows_of_bands_at_58_percent() {
        let l = layout(screen(1920.0, 1080.0), 8);
        assert!((l.grid.width() - 0.58 * 1920.0).abs() < 1.0, "{:?}", l.grid);
        assert!((l.grid.center().x - 960.0).abs() < 1.0);
        assert!(l.tile.x > l.tile.y * 2.5, "bands, not squares: {:?}", l.tile);
        assert!(l.tile.y >= 64.0);
        assert!(l.grid.top() > l.header.bottom() && l.grid.bottom() < l.log.top());
        assert!(l.log.height() >= 0.3 * 1080.0 - 2.0 * GAP, "{:?}", l.log);
        assert!(l.log.bottom() <= 1080.0 && l.log.left() >= 0.0 && l.log.right() <= 1920.0);
    }

    #[test]
    fn nine_tiles_fit_between_header_and_log_on_1280_by_720() {
        let l = layout(screen(1280.0, 720.0), 9);
        assert!(
            l.grid.top() > l.header.bottom() && l.grid.bottom() < l.log.top(),
            "{l:?}"
        );
        assert!(l.grid.left() >= 0.0 && l.grid.right() <= 1280.0, "{l:?}");
        assert!(l.tile.x > l.tile.y * 2.5, "{:?}", l.tile);
    }

    fn texts_of(menu: &Menu, icons: &[Option<TextureHandle>]) -> Vec<String> {
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
            "not installed · install failed: HTTP 404",
            "updates only by hand",
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
    fn the_log_keeps_its_minimum_rows_on_a_short_screen() {
        let l = layout(screen(1280.0, 480.0), 6);
        assert!(l.log.height() >= LOG_MIN_ROWS as f32 * LOG_ROW, "{:?}", l.log);
    }

    #[test]
    fn many_tiles_start_at_the_top_and_overflow_downwards() {
        let l = layout(screen(1920.0, 1080.0), 38);
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
        let l = layout(screen(1280.0, 720.0), names.len());
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
        let icons: Vec<Option<TextureHandle>> = (0..tiles).map(|_| None).collect();
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
        let l = layout(screen(1280.0, 720.0), 3);
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

    #[test]
    fn the_dialog_names_the_way_the_menu_ends() {
        let anyway = |a| format!("{} anyway", capitalized(ending_words(a).0));
        assert_eq!(anyway(Action::Logout), "Log out anyway");
        assert_eq!(anyway(Action::Reboot), "Restart anyway");
        assert_eq!(anyway(Action::Poweroff), "Power off anyway");
        assert_eq!(capitalized(ending_words(Action::Poweroff).1), "Powering off");
    }
}
