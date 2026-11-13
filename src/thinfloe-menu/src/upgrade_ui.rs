//! Drawing the Upgrade screen: the choice, the running write and its end.
//! Everything that decides something lives in upgrade.rs; this file only
//! paints it and reports clicks. Repaints come from app.rs (the drive poll
//! and every message of the write), never from a timer here.

use crate::brand::{self, role};
use crate::density::GROUP_W;
use crate::drives::{self, Drive, Mode};
use crate::menu::Menu;
use crate::settings::{self, Tone};
use crate::speed;
use crate::ui;
use crate::upgrade::{self, Outcome, Stage, StepMark, StepView, Upgrade, UpgradeEvent};
use egui::text::LayoutJob;
use egui::{Color32, FontId, Rect, Sense, Stroke, StrokeKind, TextFormat, Ui, UiBuilder, Vec2, vec2};
use shadcn_egui::Theme;
use shadcn_egui::components::button::{Button, ButtonVariant};
use shadcn_egui::components::checkbox::Checkbox;
use shadcn_egui::components::field::{Field, field_content, field_description, field_label};
use shadcn_egui::components::input::Input;
use shadcn_egui::components::item::{Item, ItemSize};
use shadcn_egui::components::progress::Progress;
use shadcn_egui::components::switch::Switch;

/// Around the panel, as on the Settings screen (`p-4`).
const PAD: f32 = 16.0;
/// Between the header and the page, and between two blocks (`gap-2`):
/// tight, so the four Choose blocks fit above the footer at 1080p.
const BLOCK_GAP: f32 = 8.0;
/// Inside a block (`p-3`).
const BLOCK_PAD: i8 = 12;
/// Between the lines of a block (`gap-1.5`).
const LINE_GAP: f32 = 6.0;
/// Between the two halves of a row of choices, and between the two
/// columns of the Choose page (`gap-2`).
const COL_GAP: f32 = 8.0;
/// Between two rows of a choice (`gap-1`).
const ROW_GAP: f32 = 4.0;
/// Between the heading of a block and the header's last-checked text (`gap-3`).
const HEADER_GAP: f32 = 12.0;
/// The side of a step's mark (`size-3`).
const MARK: f32 = 12.0;
/// The width of a step's mark border (`border`).
const MARK_BORDER: f32 = 1.0;
/// Between a step's mark and its label (`gap-3`).
const MARK_GAP: f32 = 12.0;
/// The height of a step's row (`h-7`).
const STEP_H: f32 = 28.0;
/// A drive no mode can use fades (`opacity-50`); its reason stays legible.
const REFUSED_OPACITY: f32 = 0.5;
/// The widest the address field and the typed-confirm field get (`max-w-md`).
const FIELD_W: f32 = 448.0;

/// The words of the steps' marks, for screen readers.
fn mark_word(m: StepMark) -> &'static str {
    match m {
        StepMark::Waiting => "waiting",
        StepMark::Running => "running",
        StepMark::Done => "done",
        StepMark::Failed => "failed",
    }
}

pub fn tone_color(t: &Theme, tone: Tone) -> Color32 {
    match tone {
        Tone::Plain => t.palette().foreground,
        Tone::Good => t.role(role::GOOD),
        Tone::Ice => t.palette().primary,
        Tone::Warn => t.role(role::WARN),
    }
}

/// The Upgrade screen in `area`.
pub fn show(ui: &mut Ui, menu: &Menu, area: Rect) -> Option<UpgradeEvent> {
    let t = Theme::current(ui.ctx());
    let u = &menu.upgrade;
    let mut event = None;
    // The panel is as wide as the tile group of the desktop, centred.
    let inner = area.shrink(PAD);
    let width = GROUP_W.min(inner.width());
    let panel = Rect::from_center_size(inner.center(), vec2(width, inner.height()));
    let mut p_ui = ui.new_child(UiBuilder::new().max_rect(panel));
    p_ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Upgrade")
                .font(brand::geo(brand::SETTINGS_TITLE_PT))
                .color(t.palette().foreground),
        );
        if let Some(job) = u.job_name() {
            ui.add_space(HEADER_GAP);
            ui.label(egui::RichText::new(job).color(t.palette().muted_foreground));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(Button::new("Back").variant(ButtonVariant::Secondary)).clicked() {
                event = Some(UpgradeEvent::Back);
            }
        });
    });
    let block_w = width - 2.0 * f32::from(BLOCK_PAD);
    p_ui.add_space(BLOCK_GAP);
    let page_top = p_ui.cursor().min.y;
    // The footer follows the page, one BLOCK_GAP below its last block (the
    // page's height includes that gap); pinned when the page overflows.
    let rect = Rect::from_min_max(egui::pos2(panel.left(), page_top), panel.max);
    let (footer, page) = ui::page_with_footer(
        ui,
        "upgrade",
        rect,
        BLOCK_GAP,
        |ui| match &u.stage {
            Stage::Choose => choose_footer(ui, u, &t, block_w),
            Stage::Running => running_footer(ui, u, &t, block_w),
            Stage::Finished(o) => finished_footer(ui, o, &t, block_w),
        },
        |ui| match &u.stage {
            Stage::Choose => choose(ui, menu, &t, block_w),
            Stage::Running => running(ui, u, &t, block_w),
            Stage::Finished(o) => finished(ui, u, o, &t, block_w),
        },
    );
    page.or(footer).or(event)
}

/// A TILE-coloured block, with a heading when `title` is given and a
/// muted note at the right end of the heading's line.
fn card<R>(
    ui: &mut Ui,
    t: &Theme,
    width: f32,
    title: Option<&str>,
    note: Option<&str>,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    egui::Frame::new()
        .fill(t.palette().card)
        .inner_margin(BLOCK_PAD)
        .show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = LINE_GAP;
            if let Some(title) = title {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(title)
                            .font(brand::geo(brand::SECTION_PT))
                            .color(t.palette().foreground),
                    );
                    if let Some(note) = note {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            muted(ui, t, note);
                        });
                    }
                });
            }
            add(ui)
        })
        .inner
}

/// A block of the scrolling page, with a heading.
fn block<R>(ui: &mut Ui, t: &Theme, width: f32, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    noted_block(ui, t, width, title, None, add)
}

/// A block with a note on its heading's line.
fn noted_block<R>(
    ui: &mut Ui,
    t: &Theme,
    width: f32,
    title: &str,
    note: Option<&str>,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let r = card(ui, t, width, Some(title), note, add);
    ui.add_space(BLOCK_GAP);
    r
}

/// Two halves side by side, `COL_GAP` apart: `add` draws half 0, then 1.
fn halves(ui: &mut Ui, mut add: impl FnMut(&mut Ui, usize)) {
    let w = ((ui.available_width() - COL_GAP) / 2.0).max(0.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = COL_GAP;
        for half in 0..2 {
            ui.vertical(|ui| {
                ui.set_width(w);
                add(ui, half);
            });
        }
    });
}

fn muted(ui: &mut Ui, t: &Theme, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).color(t.palette().muted_foreground));
}

fn warn(ui: &mut Ui, t: &Theme, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).color(t.role(role::WARN)));
}

/// A checkbox with its label, and a line under it.
fn check(ui: &mut Ui, on: bool, label: &str, line: Option<&str>) -> Option<bool> {
    let mut v = on;
    let changed = Field::new()
        .horizontal()
        .id_salt(label)
        .show(ui, |ui| {
            let c = ui.add(Checkbox::new(&mut v)).changed();
            field_content(ui, |ui| {
                field_label(ui, label);
                if let Some(line) = line {
                    field_description(ui, line);
                }
            });
            c
        })
        .inner;
    changed.then_some(v)
}

fn choose(ui: &mut Ui, menu: &Menu, t: &Theme, w: f32) -> Option<UpgradeEvent> {
    let u = &menu.upgrade;
    let mut event = None;
    // The heading shares the version line, to keep the block one row high.
    card(ui, t, w, None, None, |ui| {
        release(ui, u, t, &mut event);
    });
    ui.add_space(BLOCK_GAP);
    let outer = w + 2.0 * f32::from(BLOCK_PAD);
    if outer < brand::UPGRADE_TWO_COLUMNS_W {
        // One column: the drives one row each, the choices side by side.
        target_block(ui, u, t, w, false, &mut event);
        action_blocks(ui, menu, t, w, true, &mut event);
        return event;
    }
    // Two columns: the drives on the left, Action and Options on the right.
    let half = (outer - COL_GAP) / 2.0 - 2.0 * f32::from(BLOCK_PAD);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = COL_GAP;
        ui.vertical(|ui| {
            target_block(ui, u, t, half, true, &mut event);
        });
        ui.vertical(|ui| {
            action_blocks(ui, menu, t, half, false, &mut event);
        });
    });
    event
}

/// Action and Options for the chosen drive; `wide` puts each block's two
/// choices side by side, else they are stacked.
fn action_blocks(ui: &mut Ui, menu: &Menu, t: &Theme, w: f32, wide: bool, event: &mut Option<UpgradeEvent>) {
    let u = &menu.upgrade;
    let Some(d) = u.chosen_drive() else {
        return;
    };
    let modes = drives::modes(d, u.image_bytes());
    if modes.len() == 2 {
        block(ui, t, w, "Action", |ui| {
            ui.style_mut().interaction.selectable_labels = false;
            pair(ui, wide, |ui, i| {
                let Some((mode, allowed)) = modes.get(i) else { return };
                let title = match mode {
                    Mode::Upgrade => "Upgrade (A/B, active after a restart)",
                    Mode::Fresh => "Erase and install",
                };
                let mut item = Item::new()
                    .variant(brand::CHOICE_ROW)
                    .size(ItemSize::Sm)
                    .title(title)
                    .selected(u.mode == Some(*mode))
                    .sense(Sense::click())
                    .id_salt(("mode", title));
                if let Err(why) = allowed {
                    item = item.description(egui::RichText::new(why).color(t.role(role::WARN)));
                }
                let r = ui.add_enabled_ui(allowed.is_ok(), |ui| ui.add(item)).inner;
                if r.clicked() {
                    *event = Some(UpgradeEvent::Mode(*mode));
                }
            });
        });
    }
    if u.mode != Some(Mode::Fresh) {
        return;
    }
    block(ui, t, w, "Options", |ui| {
        let restore_failed = menu.restore_failed();
        pair(ui, wide, |ui, i| {
            if i == 0 {
                let copy = ui
                    .add_enabled_ui(!restore_failed, |ui| {
                        check(
                            ui,
                            u.copy_home && !restore_failed,
                            "Copy this machine's home directory",
                            None,
                        )
                    })
                    .inner;
                if let Some(on) = copy {
                    *event = Some(UpgradeEvent::CopyHome(on));
                }
            } else if let Some(on) = check(ui, u.verify, "Check the stick after writing", None) {
                *event = Some(UpgradeEvent::Verify(on));
            }
        });
        // The copy's note spans the block.
        muted(
            ui,
            t,
            if restore_failed {
                "This boot's home directory was not restored, so it cannot be copied."
            } else {
                "The copy leaves out the SSH host keys and the WireGuard key; the remote-access password travels with the home."
            },
        );
        if drives::internal(d) {
            let confirm = drives::confirm_text(d);
            warn(ui, t, format!("Type {confirm} to erase this drive."));
            let mut typed = u.typed.clone();
            let r = ui.add(
                Input::new(&mut typed)
                    .hint(confirm)
                    .max_width(FIELD_W)
                    .id_salt("upgrade-typed"),
            );
            if r.changed() {
                *event = Some(UpgradeEvent::Typed(typed));
            }
        }
    });
}

/// Two choices: side by side when `wide`, else one under the other.
fn pair(ui: &mut Ui, wide: bool, mut add: impl FnMut(&mut Ui, usize)) {
    if wide {
        halves(ui, add);
    } else {
        ui.spacing_mut().item_spacing.y = ROW_GAP;
        add(ui, 0);
        add(ui, 1);
    }
}

/// The summary and the confirm button, fixed below the page, in one row.
fn choose_footer(ui: &mut Ui, u: &Upgrade, t: &Theme, w: f32) -> Option<UpgradeEvent> {
    let mut event = None;
    let fresh = u.mode == Some(Mode::Fresh);
    card(ui, t, w, None, None, |ui| {
        let Some(summary) = u.summary() else {
            muted(ui, t, "Choose what to do with the stick.");
            return;
        };
        // In a row, so the layout below does not take the panel's height.
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = u.confirm_label().unwrap_or_default();
                let b = Button::new(label);
                let b = if fresh {
                    b.variant(ButtonVariant::Destructive)
                } else {
                    b
                };
                if ui.add_enabled(u.can_confirm(), b).clicked() {
                    event = Some(UpgradeEvent::Confirm);
                }
                ui.add_space(HEADER_GAP);
                ui.with_layout(
                    egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                    |ui| {
                        ui.label(egui::RichText::new(summary).color(t.palette().foreground));
                    },
                );
            })
        });
    });
    event
}

fn release(ui: &mut Ui, u: &Upgrade, t: &Theme, event: &mut Option<UpgradeEvent>) {
    let unknown = "version unknown";
    let rel = u.target_release();
    let latest = rel.and_then(|r| r.latest.as_deref()).unwrap_or(unknown);
    // SAFETY: time(NULL) has no preconditions.
    let now = unsafe { libc::time(std::ptr::null_mut()) } as i64;
    // An address that cannot be checked was never checked: say why.
    let checked = if u.source_invalid() {
        "address not valid".to_string()
    } else {
        settings::last_checked(rel.and_then(|r| r.checked), now)
    };
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Release")
                .font(brand::geo(brand::SECTION_PT))
                .color(t.palette().foreground),
        );
        ui.add_space(HEADER_GAP);
        muted(ui, t, "Installed");
        ui.label(egui::RichText::new(u.installed.as_deref().unwrap_or(unknown)).color(t.palette().foreground));
        ui.add_space(HEADER_GAP);
        muted(ui, t, "Newest");
        ui.label(egui::RichText::new(latest).color(t.palette().foreground));
        muted(ui, t, format!("· {checked}"));
        ui.add_space(HEADER_GAP);
        // Not a Field: a horizontal Field's label takes all the room left,
        // and Check now would be pushed out of the block.
        let mut advanced = u.advanced;
        let switched = ui.add(Switch::new(&mut advanced)).changed();
        let named = ui
            .add(egui::Label::new("Advanced").selectable(false).sense(Sense::click()))
            .clicked();
        if named {
            advanced = !advanced;
        }
        if switched || named {
            *event = Some(UpgradeEvent::Advanced(advanced));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(
                    !u.checking && !u.source_invalid(),
                    Button::new("Check now").variant(ButtonVariant::Secondary),
                )
                .clicked()
            {
                *event = Some(UpgradeEvent::CheckNow);
            }
        });
    });
    if let Some(e) = rel.and_then(|r| r.error.as_deref()) {
        warn(ui, t, format!("The check failed: {e}"));
    }
    let entered = u.source_text.trim();
    if !u.advanced {
        // Off, the address keeps its value but is not used: say so, never
        // hide it.
        if !entered.is_empty() {
            muted(ui, t, format!("Not used while Advanced is off: {entered}"));
        }
        return;
    }
    let bad = u.source_invalid();
    let mut text = u.source_text.clone();
    let mut f = Field::new().label("Release address").invalid(bad).id_salt("source");
    if bad {
        f = f.error("Only http:// and https:// addresses.");
    }
    f.show(ui, |ui| {
        let r = ui.add(
            Input::new(&mut text)
                .hint("address of the release")
                .invalid(bad)
                .max_width(FIELD_W)
                .id_salt("upgrade-source"),
        );
        if r.changed() {
            *event = Some(UpgradeEvent::Source(text.clone()));
        }
    });
    if entered.is_empty() {
        muted(ui, t, "Empty: the default release is used.");
    }
}

/// The reason a drive cannot be used at all: every mode is refused.
fn refusal(d: &Drive, image_bytes: Option<u64>) -> Option<String> {
    let modes = drives::modes(d, image_bytes);
    if modes.iter().any(|(_, r)| r.is_ok()) {
        return None;
    }
    modes.into_iter().find_map(|(_, r)| r.err())
}

/// The Target block: its heading with Show all drives at the right end of
/// the heading's line, the drive list, and what the list does.
fn target_block(ui: &mut Ui, u: &Upgrade, t: &Theme, w: f32, narrow: bool, event: &mut Option<UpgradeEvent>) {
    const HINT: &str = "Internal drives too. Erasing one needs its name and size typed in.";
    card(ui, t, w, None, None, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Target")
                    .font(brand::geo(brand::SECTION_PT))
                    .color(t.palette().foreground),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Not a Field, for the reason given at Advanced. Right to
                // left: the label first, then its box to the left of it.
                let mut on = u.show_all;
                let named = ui
                    .add(
                        egui::Label::new("Show all drives")
                            .selectable(false)
                            .sense(Sense::click()),
                    )
                    .on_hover_text(HINT)
                    .clicked();
                let ticked = ui.add(Checkbox::new(&mut on)).on_hover_text(HINT).changed();
                if named {
                    on = !on;
                }
                if named || ticked {
                    *event = Some(UpgradeEvent::ShowAll(on));
                }
            });
        });
        target(ui, u, t, narrow, event);
        muted(ui, t, "The list follows as sticks are plugged in and pulled out.");
        if u.show_all {
            muted(ui, t, HINT);
        }
    });
    ui.add_space(BLOCK_GAP);
}

/// The drive list; `narrow` puts each drive's details under its name.
fn target(ui: &mut Ui, u: &Upgrade, t: &Theme, narrow: bool, event: &mut Option<UpgradeEvent>) {
    if let Some(e) = &u.drives_error {
        warn(ui, t, format!("The drives could not be listed: {e}"));
    }
    if u.drives.is_empty() && u.drives_error.is_none() {
        let text = if !u.listed() {
            "Looking for drives…"
        } else if u.show_all {
            "No drive found. Plug a stick in."
        } else {
            "No USB stick found. Plug one in."
        };
        muted(ui, t, text);
    }
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = ROW_GAP;
        // A selectable title would take the click meant for its row.
        ui.style_mut().interaction.selectable_labels = false;
        for d in &u.drives {
            let why = refusal(d, u.image_bytes());
            let font = FontId::proportional(brand::ROW_STATUS_PT);
            let mut job = LayoutJob::default();
            job.append(
                &format!("{} · {}", d.device, drives::tag(d)),
                0.0,
                TextFormat::simple(font.clone(), t.palette().muted_foreground),
            );
            if let Some(why) = &why {
                job.append(&format!(" · {why}"), 0.0, TextFormat::simple(font, t.role(role::WARN)));
            }
            let chosen = u.chosen.as_ref().is_some_and(|c| c.id == d.id && c.device == d.device);
            let item = Item::new()
                .variant(brand::CHOICE_ROW)
                .size(ItemSize::Sm)
                .title(format!("{}, {}", upgrade::name(d), drives::format_size(d.size_bytes)))
                .selected(chosen)
                .sense(Sense::click())
                .id_salt(("drive", &d.device));
            // Wide: one line, the device and its state at the row's end.
            // Narrow: they go under the name, where they may wrap.
            let r = ui
                .add_enabled_ui(why.is_none(), |ui| {
                    if why.is_some() {
                        ui.multiply_opacity(REFUSED_OPACITY);
                    }
                    if narrow {
                        ui.add(item.description(job))
                    } else {
                        item.show(ui, |ui| ui.label(job)).response
                    }
                })
                .inner;
            if r.clicked() {
                *event = Some(UpgradeEvent::Choose(d.device.clone()));
            }
        }
    });
}

/// A step's mark: a square, empty while waiting, filled in the colour of
/// its state.
fn paint_mark(ui: &mut Ui, t: &Theme, label: &str, mark: StepMark) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(MARK), Sense::hover());
    let p = ui.painter();
    match mark {
        StepMark::Waiting => {
            p.rect_stroke(
                rect,
                0.0,
                Stroke::new(MARK_BORDER, t.palette().muted_foreground),
                StrokeKind::Inside,
            );
        }
        StepMark::Running => {
            p.rect_filled(rect, 0.0, t.palette().primary);
        }
        StepMark::Done => {
            p.rect_filled(rect, 0.0, t.role(role::GOOD));
        }
        StepMark::Failed => {
            p.rect_filled(rect, 0.0, t.role(role::WARN));
        }
    }
    let word = mark_word(mark);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{label}: {word}")));
}

fn step_row(ui: &mut Ui, u: &Upgrade, t: &Theme, s: &StepView, mark: StepMark) {
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), STEP_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = MARK_GAP;
            paint_mark(ui, t, &s.label, mark);
            let color = match mark {
                StepMark::Waiting => t.palette().muted_foreground,
                StepMark::Failed => t.role(role::WARN),
                _ => t.palette().foreground,
            };
            ui.label(egui::RichText::new(&s.label).color(color));
        },
    );
    if mark != StepMark::Running || u.current_step() != Some(s.id.as_str()) {
        return;
    }
    let Some((done, total)) = u.progress else {
        return;
    };
    ui.horizontal(|ui| {
        ui.add_space(MARK + MARK_GAP);
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            let fraction = speed::fraction(done, total);
            if let Some(f) = fraction {
                ui.add(Progress::new(f));
            }
            let mut parts = vec![speed::bytes_text(done, total)];
            if let Some(rate) = u.speed.rate() {
                parts.push(speed::rate_text(rate));
            }
            // A time left only with a bar: a total that is missing or
            // passed gives none.
            if fraction.is_some()
                && let Some(eta) = u.speed.eta(done, total)
            {
                parts.push(speed::eta_text(eta));
            }
            muted(ui, t, parts.join(" · "));
        });
    });
}

fn running(ui: &mut Ui, u: &Upgrade, t: &Theme, w: f32) -> Option<UpgradeEvent> {
    let title = u.summary().unwrap_or_else(|| "Writing".to_string());
    block(ui, t, w, &title, |ui| {
        for s in &u.steps {
            step_row(ui, u, t, s, s.mark);
        }
    });
    None
}

/// Cancel, or the question about stopping, fixed below the steps.
fn running_footer(ui: &mut Ui, u: &Upgrade, t: &Theme, w: f32) -> Option<UpgradeEvent> {
    let cancellable = matches!(u.current_step(), Some("write" | "download" | "verify"));
    if !u.cancelling && !u.cancel_asked && !cancellable {
        return None;
    }
    let mut event = None;
    card(ui, t, w, None, None, |ui| {
        if u.cancelling {
            muted(ui, t, "Cancelling…");
        } else if u.cancel_asked {
            warn(ui, t, u.cancel_question());
            ui.horizontal(|ui| {
                if ui
                    .add(Button::new("Keep writing").variant(ButtonVariant::Secondary))
                    .clicked()
                {
                    event = Some(UpgradeEvent::CancelNo);
                }
                if ui
                    .add(Button::new("Stop").variant(ButtonVariant::Destructive))
                    .clicked()
                {
                    event = Some(UpgradeEvent::CancelYes);
                }
            });
        } else if ui
            .add(Button::new("Cancel").variant(ButtonVariant::Secondary))
            .clicked()
        {
            event = Some(UpgradeEvent::Cancel);
        }
    });
    event
}

fn finished(ui: &mut Ui, u: &Upgrade, o: &Outcome, t: &Theme, w: f32) -> Option<UpgradeEvent> {
    let ok = matches!(o, Outcome::Done { .. });
    let (text, tone) = u.outcome_text();
    block(ui, t, w, if ok { "Done" } else { "Not done" }, |ui| {
        ui.label(egui::RichText::new(text).color(tone_color(t, tone)));
        for s in &u.steps {
            // A success leaves no step open: a step the tool skipped (a
            // conditional one) is left out, one it began without `@done`
            // is drawn done.
            let mark = match (ok, s.mark) {
                (true, StepMark::Waiting) => continue,
                (true, StepMark::Running) => StepMark::Done,
                (_, m) => m,
            };
            step_row(ui, u, t, s, mark);
        }
    });
    None
}

/// Restart now, Try again and Back, fixed below the outcome.
fn finished_footer(ui: &mut Ui, o: &Outcome, t: &Theme, w: f32) -> Option<UpgradeEvent> {
    let ok = matches!(o, Outcome::Done { .. });
    let mut event = None;
    card(ui, t, w, None, None, |ui| {
        ui.horizontal(|ui| {
            if matches!(o, Outcome::Done { boot_stick: true, .. }) {
                if ui.add(Button::new("Restart now")).clicked() {
                    event = Some(UpgradeEvent::RestartNow);
                }
                // Opening the screen again keeps this outcome for Restart
                // now, so another stick needs its own way back to Choose.
                if ui
                    .add(Button::new("Write another stick").variant(ButtonVariant::Secondary))
                    .clicked()
                {
                    event = Some(UpgradeEvent::WriteAnother);
                }
            }
            if !ok && ui.add(Button::new("Try again")).clicked() {
                event = Some(UpgradeEvent::TryAgain);
            }
            if ui.add(Button::new("Back").variant(ButtonVariant::Secondary)).clicked() {
                event = Some(UpgradeEvent::Back);
            }
        });
    });
    event
}
