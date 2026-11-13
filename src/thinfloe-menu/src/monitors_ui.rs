//! Drawing the monitor part of the Screen tab: the monitors as boxes in
//! their real proportions, the settings of the selected one, Apply, and
//! *Keep this arrangement?* over it all. Everything that decides something
//! lives in monitors.rs; this file only paints it and reports clicks.

use crate::brand::{self, role};
use crate::heads::{Mode, Rotation};
use crate::kanshi::{self, OutputSet, Place, Profile, Side};
use crate::monitors::{MonEvent, MonTab};
use crate::owned::Owner;
use crate::ui::{self, ROW_LEFT, ROW_MARGIN_X, ROW_RIGHT};
use egui::{Align, Color32, Layout, Rect, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, vec2};
use shadcn_egui::Theme;
use shadcn_egui::components::button::{Button, ButtonVariant};
use shadcn_egui::components::checkbox::Checkbox;
use shadcn_egui::components::icon::IconView;
use shadcn_egui::components::select::Select;
use shadcn_egui::icons::lucide;
use std::time::Instant;

/// The gap between two monitor boxes that touch on the desktop: each box
/// is drawn this much smaller than its place.
pub const BOX_GAP: f32 = 4.0;
/// The height of the area the boxes are drawn in.
const ARRANGE_H: f32 = 140.0;
/// Between a box's edge and its text.
const BOX_PAD: f32 = 8.0;
/// The size a box takes for a monitor that is off and reports no mode.
const NO_MODE: (i32, i32) = (1920, 1080);
/// Above and below the content of a band, as a Packages row.
const BAND_PAD: i8 = 12;
/// Before the buttons of a dialog, as the other dialogs.
const BUTTON_GAP: f32 = 8.0;
/// Between the On tick box and *Show the menu here* on its row.
const MENU_BUTTON_GAP: f32 = 24.0;

const ROTATIONS: [Rotation; 4] = [Rotation::Normal, Rotation::R90, Rotation::R180, Rotation::R270];
const ROTATION_LABELS: [&str; 4] = ["Normal", "90°", "180°", "270°"];
const SIDES: [Side; 4] = [Side::LeftOf, Side::RightOf, Side::Above, Side::Below];
const SIDE_LABELS: [&str; 4] = ["Left of", "Right of", "Above", "Below"];
const HAND_LINE: &str = "The monitor arrangement is set by hand in ~/.config/kanshi/config.";
const THINLINC_NOTE: &str = "ThinLinc uses the new arrangement from its next start.";

/// *Keep this arrangement?* is up; only it is drawn, over the frozen and
/// dimmed screen, and it takes every click.
pub fn dialog_open(mon: &MonTab) -> bool {
    mon.question_up()
}

/// A mode as the Resolution select lists it: `1920 × 1080 @ 60 Hz`, with
/// up to two decimals when the rate is not whole, and no rate for a
/// headless output, which reports none.
pub fn mode_label(m: &Mode) -> String {
    let size = format!("{} × {}", m.w, m.h);
    if m.refresh_mhz == 0 {
        return size;
    }
    // Hundredths of a hertz, rounded.
    let centi = (m.refresh_mhz + 5) / 10;
    match centi % 100 {
        0 => format!("{size} @ {} Hz", centi / 100),
        f if f % 10 == 0 => format!("{size} @ {}.{} Hz", centi / 100, f / 10),
        f => format!("{size} @ {}.{f:02} Hz", centi / 100),
    }
}

/// Where a monitor's box goes in the drawing.
#[derive(Debug, Clone, PartialEq)]
pub struct MonBox {
    pub criteria: String,
    pub enabled: bool,
    /// Its place in `area`; the box is drawn `BOX_GAP` smaller.
    pub rect: Rect,
}

/// The boxes of `layout` fitted into `area`, all at one scale. The
/// monitors that are on sit where the arrangement puts them; those that
/// are off follow in a row to their right, at the size of their mode.
pub fn boxes(mon: &MonTab, layout: &Profile, area: Rect) -> Vec<MonBox> {
    // An arrangement that cannot be placed shows its monitors in a row.
    let placed = kanshi::place_all(layout).unwrap_or_else(|_| vec![None; layout.outputs.len()]);
    let size_of = |o: &OutputSet| {
        if o.mode.w > 0 && o.mode.h > 0 {
            return kanshi::size(o);
        }
        mon.head(&o.criteria)
            .and_then(|h| h.modes.iter().find(|m| m.preferred).or_else(|| h.modes.first()))
            .map_or(NO_MODE, |m| (m.w as i32, m.h as i32))
    };
    // Desktop rects: x, y, w, h.
    let mut rects: Vec<(i32, i32, i32, i32)> = vec![(0, 0, 0, 0); layout.outputs.len()];
    for (i, o) in layout.outputs.iter().enumerate() {
        if let Some((x, y)) = placed[i] {
            let (w, h) = size_of(o);
            rects[i] = (x, y, w, h);
        }
    }
    let placed_right = rects
        .iter()
        .zip(&placed)
        .filter(|(_, p)| p.is_some())
        .map(|(r, _)| r.0 + r.2)
        .max();
    let widest = layout.outputs.iter().map(|o| size_of(o).0).max().unwrap_or(0);
    let gap = widest / 8;
    let mut cursor = placed_right.map_or(0, |r| r + gap);
    for (i, o) in layout.outputs.iter().enumerate() {
        if placed[i].is_none() {
            let (w, h) = size_of(o);
            rects[i] = (cursor, 0, w, h);
            cursor += w + gap;
        }
    }
    let total_w = rects.iter().map(|r| r.0 + r.2).max().unwrap_or(0).max(1) as f32;
    let total_h = rects.iter().map(|r| r.1 + r.3).max().unwrap_or(0).max(1) as f32;
    let scale = (area.width() / total_w).min(area.height() / total_h);
    let offset = vec2((area.width() - total_w * scale) / 2.0, 0.0);
    layout
        .outputs
        .iter()
        .zip(rects)
        .map(|(o, (x, y, w, h))| MonBox {
            criteria: o.criteria.clone(),
            enabled: o.enabled,
            rect: Rect::from_min_size(
                area.min + offset + vec2(x as f32 * scale, y as f32 * scale),
                vec2(w as f32 * scale, h as f32 * scale),
            ),
        })
        .collect()
}

/// The monitor band of the Screen tab, `width` wide as the panel, under
/// the screen timeout: the boxes and the selected monitor's settings.
/// Apply follows in `monitors_footer`. `now` keeps the countdown's
/// wake-up honest.
pub fn monitors_band(ui: &mut Ui, mon: &MonTab, now: Instant, width: f32) -> Option<MonEvent> {
    let t = Theme::current(ui.ctx());
    if let Some(d) = mon.next_wake(now) {
        ui.ctx().request_repaint_after(d);
    }
    let mut event = None;
    let inner = width - ROW_MARGIN_X;
    egui::Frame::new()
        .fill(t.palette().card)
        .inner_margin(egui::Margin {
            left: ROW_LEFT,
            right: ROW_RIGHT,
            top: BAND_PAD,
            bottom: BAND_PAD,
        })
        .show(ui, |ui| {
            ui.set_width(inner);
            ui.label(
                RichText::new("Monitors")
                    .font(brand::geo(brand::SECTION_PT))
                    .color(t.palette().foreground),
            );
            let Some(layout) = mon.layout() else {
                muted(ui, &t, "Looking for monitors…");
                return;
            };
            let hand = mon.owner() == Owner::ByHand;
            if let Some(c) = arrangement(ui, &t, mon, layout, !hand) {
                event = Some(MonEvent::Select(c));
            }
            if hand {
                muted(ui, &t, HAND_LINE);
                if ui
                    .add_enabled(
                        !mon.busy(),
                        Button::new("Let the menu manage it").variant(ButtonVariant::Secondary),
                    )
                    .clicked()
                {
                    event = Some(MonEvent::TakeOver);
                }
            } else if let Some(e) = selected_rows(ui, &t, mon, layout) {
                event = Some(e);
            }
        });
    event
}

/// What follows the bands, as the Network tab's Apply: the ThinLinc note,
/// Apply, and the tab's error. Nothing to apply for a file written by
/// hand, nor before the monitors are known.
pub fn monitors_footer(ui: &mut Ui, mon: &MonTab) -> Option<MonEvent> {
    let t = Theme::current(ui.ctx());
    let mut event = None;
    if let Some(layout) = mon.layout()
        && mon.owner() != Owner::ByHand
    {
        if mon.thinlinc_note() {
            muted(ui, &t, THINLINC_NOTE);
        }
        // Neither a draft without a change nor one with every monitor off
        // is offered.
        let can = mon.editable() && !mon.busy() && mon.changed() && layout.outputs.iter().any(|o| o.enabled);
        if ui.add_enabled(can, Button::new("Apply")).clicked() {
            event = Some(MonEvent::Apply);
        }
    }
    if let Some(e) = mon.error() {
        warn(ui, &t, e);
    }
    event
}

fn muted(ui: &mut Ui, t: &Theme, text: &str) {
    ui.label(RichText::new(text).color(t.palette().muted_foreground));
}

fn warn(ui: &mut Ui, t: &Theme, text: &str) {
    ui.label(RichText::new(text).color(t.role(role::WARN)));
}

/// A form row: the label in its column, then what `add` draws.
fn form_row<R>(ui: &mut Ui, t: &Theme, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.set_min_width(brand::FORM_LABEL_W);
            ui.set_max_width(brand::FORM_LABEL_W);
            ui.label(RichText::new(label).color(t.palette().foreground));
        });
        add(ui)
    })
    .inner
}

/// The connector name of a monitor, else its criteria.
fn name_of<'a>(mon: &'a MonTab, criteria: &'a str) -> &'a str {
    mon.head(criteria).map_or(criteria, |h| h.name.as_str())
}

/// The boxes; returns the criteria of a box clicked. `pick`: a click
/// selects, else the boxes only show.
fn arrangement(ui: &mut Ui, t: &Theme, mon: &MonTab, layout: &Profile, pick: bool) -> Option<String> {
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), ARRANGE_H), Sense::hover());
    let mut clicked = None;
    let p = t.palette();
    let sense = if pick { Sense::click() } else { Sense::hover() };
    for b in boxes(mon, layout, area) {
        let resp = ui.interact(b.rect, egui::Id::unique(("tf-mon-box", &b.criteria)), sense);
        if resp.clicked() {
            clicked = Some(b.criteria.clone());
        }
        let drawn = b.rect.shrink(BOX_GAP / 2.0);
        let selected = pick && mon.selected() == Some(b.criteria.as_str());
        let fill = match (b.enabled, pick && resp.hovered()) {
            (true, true) => p.accent,
            (true, false) => p.muted,
            (false, _) => Color32::TRANSPARENT,
        };
        let stroke = if selected {
            Stroke::new(2.0, p.primary)
        } else {
            Stroke::new(1.0, p.border)
        };
        ui.painter().rect(drawn, 0.0, fill, stroke, StrokeKind::Inside);
        let text_color = if b.enabled { p.foreground } else { p.muted_foreground };
        let inside = drawn.shrink(BOX_PAD);
        if inside.width() <= 0.0 || inside.height() <= 0.0 {
            continue;
        }
        let mut c = ui.new_child(UiBuilder::new().max_rect(inside).layout(Layout::top_down(Align::Min)));
        c.set_clip_rect(inside.intersect(ui.clip_rect()));
        // A label must not take the click meant for its box.
        c.style_mut().interaction.selectable_labels = false;
        c.add(egui::Label::new(RichText::new(name_of(mon, &b.criteria)).color(text_color)).truncate());
        if let Some(h) = mon.head(&b.criteria) {
            let model = if h.model.is_empty() { &h.make } else { &h.model };
            if !model.is_empty() {
                c.add(
                    egui::Label::new(
                        RichText::new(model)
                            .size(brand::ROW_STATUS_PT)
                            .color(p.muted_foreground),
                    )
                    .truncate(),
                );
            }
        }
        if mon.menu_choice() == Some(b.criteria.as_str()) {
            c.horizontal(|ui| {
                ui.add(IconView::new(&lucide::MONITOR).color(p.primary));
                ui.add(egui::Label::new(RichText::new("Menu").color(p.primary)).truncate());
            });
        }
    }
    clicked
}

fn opposite(s: Side) -> Side {
    match s {
        Side::LeftOf => Side::RightOf,
        Side::RightOf => Side::LeftOf,
        Side::Above => Side::Below,
        Side::Below => Side::Above,
    }
}

/// Where the selected monitor sits, as the Position row shows it: its own
/// place, or for the one the others are placed from, the place the first
/// monitor beside it implies.
fn shown_place(layout: &Profile, sel: &str) -> (Option<Side>, Option<String>) {
    let Some(o) = layout.outputs.iter().find(|o| o.criteria == sel) else {
        return (None, None);
    };
    match &o.place {
        Place::Next { to, side } => (Some(*side), Some(to.clone())),
        Place::Origin => layout
            .outputs
            .iter()
            .find_map(|x| match &x.place {
                Place::Next { to, side } if to == sel => Some((Some(opposite(*side)), Some(x.criteria.clone()))),
                _ => None,
            })
            .unwrap_or((None, None)),
    }
}

/// On with *Show the menu here*, Resolution, Rotation and Position of the
/// selected monitor.
fn selected_rows(ui: &mut Ui, t: &Theme, mon: &MonTab, layout: &Profile) -> Option<MonEvent> {
    let sel = mon.selected()?;
    let o = layout.outputs.iter().find(|o| o.criteria == sel)?;
    let editable = mon.editable();
    let mut event = None;

    // The menu's monitor, or the last one on, cannot be switched off; the
    // menu goes only on a monitor that is on. So both share a row.
    let mut on = o.enabled;
    let can_switch = if o.enabled { mon.off_allowed() } else { editable };
    let can_menu = editable && o.enabled && mon.menu_choice() != Some(sel);
    form_row(ui, t, "On", |ui| {
        if ui.add_enabled(can_switch, Checkbox::new(&mut on)).changed() {
            event = Some(MonEvent::Enabled(on));
        }
        ui.add_space(MENU_BUTTON_GAP);
        if ui
            .add_enabled(
                can_menu,
                Button::new("Show the menu here").variant(ButtonVariant::Secondary),
            )
            .clicked()
        {
            event = Some(MonEvent::MenuHere);
        }
    });

    let modes: Vec<String> = mon
        .head(sel)
        .map_or(Vec::new(), |h| h.modes.iter().map(mode_label).collect());
    let mode_refs: Vec<&str> = modes.iter().map(String::as_str).collect();
    // A mode the monitor does not list (a file from elsewhere) shows as text.
    let mut mode = mon.selected_mode().unwrap_or(usize::MAX);
    let placeholder = if o.mode.w > 0 {
        mode_label(&o.mode)
    } else {
        String::new()
    };
    let changed = form_row(ui, t, "Resolution", |ui| {
        ui.add_enabled(
            editable && o.enabled && !modes.is_empty(),
            Select::new("tf-mon-mode", &mut mode, &mode_refs).placeholder(placeholder),
        )
        .changed()
    });
    if changed && mode < modes.len() {
        event = Some(MonEvent::Mode(mode));
    }

    let mut rot = ROTATIONS.iter().position(|&r| r == o.rotation).unwrap_or(0);
    let changed = form_row(ui, t, "Rotation", |ui| {
        ui.add_enabled(
            editable && o.enabled,
            Select::new("tf-mon-rotation", &mut rot, &ROTATION_LABELS),
        )
        .changed()
    });
    if changed {
        event = Some(MonEvent::Rotation(ROTATIONS[rot]));
    }

    if layout.outputs.len() > 1 {
        let others: Vec<&OutputSet> = layout
            .outputs
            .iter()
            .filter(|x| x.enabled && x.criteria != sel)
            .collect();
        let other_names: Vec<&str> = others.iter().map(|x| name_of(mon, &x.criteria)).collect();
        let (side, to) = shown_place(layout, sel);
        let mut side_i = side
            .and_then(|s| SIDES.iter().position(|&x| x == s))
            .unwrap_or(usize::MAX);
        let mut to_i = to
            .and_then(|c| others.iter().position(|x| x.criteria == c))
            .unwrap_or(usize::MAX);
        let can = editable && o.enabled && !others.is_empty();
        let changed = form_row(ui, t, "Position", |ui| {
            let a = ui
                .add_enabled(can, Select::new("tf-mon-side", &mut side_i, &SIDE_LABELS))
                .changed();
            let b = ui
                .add_enabled(can, Select::new("tf-mon-to", &mut to_i, &other_names))
                .changed();
            a || b
        });
        if changed {
            // The half not chosen yet: right of, and the first other monitor.
            let side = SIDES.get(side_i).copied().unwrap_or(Side::RightOf);
            if let Some(to) = others.get(to_i).or(others.first()) {
                event = Some(MonEvent::Place(side, to.criteria.clone()));
            }
        }
    }
    event
}

/// *Going back in N seconds.*
fn going_back(n: u32) -> String {
    if n == 1 {
        "Going back in 1 second.".to_string()
    } else {
        format!("Going back in {n} seconds.")
    }
}

/// *Keep this arrangement?* with its countdown. Escape goes back.
pub fn keep_dialog(ui: &Ui, mon: &MonTab, now: Instant) -> Option<MonEvent> {
    let n = mon.countdown(now)?;
    if let Some(d) = mon.next_wake(now) {
        ui.ctx().request_repaint_after(d);
    }
    let mut event = None;
    ui::dialog(ui, "Keep this arrangement?", |ui, t| {
        ui.label(RichText::new(going_back(n)).color(t.palette().foreground));
        if mon.thinlinc_note() {
            ui.label(RichText::new(THINLINC_NOTE).color(t.palette().muted_foreground));
        }
        ui.add_space(BUTTON_GAP);
        ui.horizontal(|ui| {
            if ui.add(Button::new("Keep")).clicked() {
                event = Some(MonEvent::Keep);
            }
            if ui
                .add(Button::new("Go back").variant(ButtonVariant::Secondary))
                .clicked()
            {
                event = Some(MonEvent::GoBack);
            }
        });
    });
    event.or_else(|| {
        ui.input(|i| i.key_pressed(egui::Key::Escape))
            .then_some(MonEvent::GoBack)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(w: u32, h: u32, mhz: u32) -> Mode {
        Mode {
            w,
            h,
            refresh_mhz: mhz,
            preferred: false,
        }
    }

    #[test]
    fn mode_labels_read_as_the_spec_writes_them() {
        assert_eq!(mode_label(&mode(1920, 1080, 60_000)), "1920 × 1080 @ 60 Hz");
        assert_eq!(mode_label(&mode(1920, 1080, 59_940)), "1920 × 1080 @ 59.94 Hz");
        assert_eq!(mode_label(&mode(1920, 1080, 59_951)), "1920 × 1080 @ 59.95 Hz");
        assert_eq!(mode_label(&mode(1280, 720, 59_900)), "1280 × 720 @ 59.9 Hz");
        assert_eq!(mode_label(&mode(1280, 1024, 75_025)), "1280 × 1024 @ 75.03 Hz");
        assert_eq!(mode_label(&mode(1920, 1080, 59_999)), "1920 × 1080 @ 60 Hz");
        assert_eq!(mode_label(&mode(1920, 1080, 0)), "1920 × 1080");
    }

    #[test]
    fn going_back_counts_in_words() {
        assert_eq!(going_back(15), "Going back in 15 seconds.");
        assert_eq!(going_back(1), "Going back in 1 second.");
    }
}
