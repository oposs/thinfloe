//! Drawing the monitor part of the Screen tab: the monitors as boxes in
//! their real proportions, the settings of the selected one, Apply, and
//! *Keep this arrangement?* over it all. Everything that decides something
//! lives in monitors.rs; this file only paints it and reports clicks.

use crate::brand::{self, role};
use crate::deskflow::statefile::DevState;
use crate::heads::{Mode, Rotation};
use crate::kanshi::{self, OutputSet, Place, Profile, Side};
use crate::monitors::{MonEvent, MonSource, MonTab};
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
/// The radius of a device's status dot, and its gap to the name.
const DOT_R: f32 = 4.0;
const DOT_GAP: f32 = 4.0;
/// Between the On tick box and *Show the menu here* on its row.
const MENU_BUTTON_GAP: f32 = 24.0;

const ROTATIONS: [Rotation; 4] = [Rotation::Normal, Rotation::R90, Rotation::R180, Rotation::R270];
const ROTATION_LABELS: [&str; 4] = ["Normal", "90°", "180°", "270°"];
const SIDES: [Side; 4] = [Side::LeftOf, Side::RightOf, Side::Above, Side::Below];
const SIDE_LABELS: [&str; 4] = ["Left of", "Right of", "Above", "Below"];
const HAND_LINE: &str = "The monitor arrangement is a kanshi file written by hand, which the menu keeps as it is.";
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

/// An attached Deskflow device as the arrangement draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct DevPlace {
    pub name: String,
    /// The monitor it sits beside, by criteria.
    pub output: String,
    pub side: Side,
    /// Its screen in its own pixels.
    pub size: (u32, u32),
    pub state: DevState,
    /// Its edge is not free: it gets no strip.
    pub inactive: bool,
}

/// Where a device's box goes in the drawing.
#[derive(Debug, Clone, PartialEq)]
pub struct DevBox {
    pub place: DevPlace,
    pub rect: Rect,
}

/// What a click in the monitor band asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum BandEvent {
    Mon(MonEvent),
    /// A device box, by name.
    Device(String),
}

/// The boxes of `layout` fitted into `area`, all at one scale. The
/// monitors that are on sit where the arrangement puts them; those that
/// are off follow in a row to their right, at the size of their mode.
pub fn boxes(mon: &MonTab, layout: &Profile, area: Rect) -> Vec<MonBox> {
    arranged(mon, layout, &[], area).0
}

/// `boxes`, and the devices beside the monitors that are on: one scale for
/// all, so a device's box shows its size against the monitors. A device
/// whose monitor is off or missing is not drawn.
pub fn arranged(mon: &MonTab, layout: &Profile, devices: &[DevPlace], area: Rect) -> (Vec<MonBox>, Vec<DevBox>) {
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
    let mut dev_rects = Vec::new();
    for d in devices {
        let Some(i) = layout.outputs.iter().position(|o| o.criteria == d.output) else {
            continue;
        };
        if placed[i].is_none() {
            continue;
        }
        let (x, y, w, h) = rects[i];
        let dw = i32::try_from(d.size.0).unwrap_or(i32::MAX / 4).min(i32::MAX / 4);
        let dh = i32::try_from(d.size.1).unwrap_or(i32::MAX / 4).min(i32::MAX / 4);
        let at = match d.side {
            Side::RightOf => (x + w, y),
            Side::LeftOf => (x - dw, y),
            Side::Above => (x, y - dh),
            Side::Below => (x, y + h),
        };
        // An inactive device whose box lies on a monitor (another monitor
        // took its edge) is not drawn: the monitor would cover it and take
        // its clicks. The band lists it instead.
        let overlaps = |(ax, ay, aw, ah): (i32, i32, i32, i32)| {
            let (ax, ay, aw, ah) = (i64::from(ax), i64::from(ay), i64::from(aw), i64::from(ah));
            rects
                .iter()
                .zip(&placed)
                .filter(|(_, p)| p.is_some())
                .any(|(&(bx, by, bw, bh), _)| {
                    let (bx, by, bw, bh) = (i64::from(bx), i64::from(by), i64::from(bw), i64::from(bh));
                    ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
                })
        };
        if d.inactive && overlaps((at.0, at.1, dw, dh)) {
            continue;
        }
        dev_rects.push((d, (at.0, at.1, dw, dh)));
    }
    // A device left of or above the monitors moves them right or down.
    let all = || rects.iter().chain(dev_rects.iter().map(|(_, r)| r));
    let min_x = all().map(|r| r.0).min().unwrap_or(0).min(0);
    let min_y = all().map(|r| r.1).min().unwrap_or(0).min(0);
    let total_w = all().map(|r| r.0 + r.2 - min_x).max().unwrap_or(0).max(1) as f32;
    let total_h = all().map(|r| r.1 + r.3 - min_y).max().unwrap_or(0).max(1) as f32;
    let scale = (area.width() / total_w).min(area.height() / total_h);
    let offset = vec2((area.width() - total_w * scale) / 2.0, 0.0);
    let to_area = |(x, y, w, h): (i32, i32, i32, i32)| {
        Rect::from_min_size(
            area.min + offset + vec2((x - min_x) as f32 * scale, (y - min_y) as f32 * scale),
            vec2(w as f32 * scale, h as f32 * scale),
        )
    };
    let mons = layout
        .outputs
        .iter()
        .zip(rects.iter())
        .map(|(o, r)| MonBox {
            criteria: o.criteria.clone(),
            enabled: o.enabled,
            rect: to_area(*r),
        })
        .collect();
    let devs = dev_rects
        .iter()
        .map(|(d, r)| DevBox {
            place: (*d).clone(),
            rect: to_area(*r),
        })
        .collect();
    (mons, devs)
}

/// The monitor band of the Screen tab, `width` wide as the panel, under
/// the screen timeout: the boxes and the selected monitor's settings.
/// Apply follows in `monitors_footer`. `now` keeps the countdown's
/// wake-up honest.
///
/// `devices` are drawn beside their monitors; `device` is the one
/// selected.
pub fn monitors_band(
    ui: &mut Ui,
    mon: &MonTab,
    devices: &[DevPlace],
    device: Option<&str>,
    now: Instant,
    width: f32,
) -> Option<BandEvent> {
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
            let hand = mon.source() == MonSource::Kept;
            event = arrangement(ui, &t, mon, layout, !hand, devices, device);
            if hand {
                muted(ui, &t, HAND_LINE);
                if ui
                    .add_enabled(
                        !mon.busy(),
                        Button::new("Let the menu manage it").variant(ButtonVariant::Secondary),
                    )
                    .clicked()
                {
                    event = Some(BandEvent::Mon(MonEvent::TakeOver));
                }
            } else if device.is_none()
                && let Some(e) = selected_rows(ui, &t, mon, layout)
            {
                event = Some(BandEvent::Mon(e));
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
        && mon.source() != MonSource::Kept
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

/// The boxes; returns what a click on one asks for. `pick`: a click on a
/// monitor selects it, else the monitors only show. A device box always
/// takes a click.
fn arrangement(
    ui: &mut Ui,
    t: &Theme,
    mon: &MonTab,
    layout: &Profile,
    pick: bool,
    devices: &[DevPlace],
    device: Option<&str>,
) -> Option<BandEvent> {
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), ARRANGE_H), Sense::hover());
    let mut clicked = None;
    let p = t.palette();
    let sense = if pick { Sense::click() } else { Sense::hover() };
    let (mons, devs) = arranged(mon, layout, devices, area);
    for d in &devs {
        if let Some(name) = device_box(ui, t, d, device == Some(d.place.name.as_str())) {
            clicked = Some(BandEvent::Device(name));
        }
    }
    for b in mons {
        let resp = ui.interact(b.rect, egui::Id::unique(("tf-mon-box", &b.criteria)), sense);
        if resp.clicked() {
            clicked = Some(BandEvent::Mon(MonEvent::Select(b.criteria.clone())));
        }
        let drawn = b.rect.shrink(BOX_GAP / 2.0);
        let selected = pick && device.is_none() && mon.dialog_monitor() == Some(b.criteria.as_str());
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

/// One device box: outlined, its name and a status dot. Returns its name
/// when clicked.
fn device_box(ui: &mut Ui, t: &Theme, d: &DevBox, selected: bool) -> Option<String> {
    let p = t.palette();
    let name = &d.place.name;
    let resp = ui.interact(d.rect, egui::Id::unique(("tf-dev-box", name)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, name));
    let drawn = d.rect.shrink(BOX_GAP / 2.0);
    let fill = if resp.hovered() { p.accent } else { Color32::TRANSPARENT };
    let stroke = if selected {
        Stroke::new(2.0, p.primary)
    } else {
        Stroke::new(1.0, p.border)
    };
    ui.painter().rect(drawn, 0.0, fill, stroke, StrokeKind::Inside);
    let dot = match d.place.state {
        DevState::Active => p.primary,
        DevState::Connected => t.role(role::GOOD),
        _ => p.muted_foreground,
    };
    let inside = drawn.shrink(BOX_PAD);
    if inside.width() > 0.0 && inside.height() > 0.0 {
        let centre = egui::pos2(inside.right() - DOT_R, inside.top() + DOT_R);
        ui.painter().circle_filled(centre, DOT_R, dot);
        let text_rect = Rect::from_min_max(
            inside.min,
            egui::pos2(inside.right() - 2.0 * DOT_R - DOT_GAP, inside.bottom()),
        );
        if text_rect.width() > 0.0 {
            let mut c = ui.new_child(
                UiBuilder::new()
                    .max_rect(text_rect)
                    .layout(Layout::top_down(Align::Min)),
            );
            c.set_clip_rect(text_rect.intersect(ui.clip_rect()));
            c.style_mut().interaction.selectable_labels = false;
            let colour = if d.place.inactive {
                p.muted_foreground
            } else {
                p.foreground
            };
            c.add(egui::Label::new(RichText::new(name).color(colour)).truncate());
        }
    }
    resp.clicked().then(|| name.clone())
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
    let sel = mon.dialog_monitor()?;
    let o = layout.outputs.iter().find(|o| o.criteria == sel)?;
    let editable = mon.editable();
    let mut event = None;

    // The menu's monitor, or the last one on, cannot be switched off; the
    // menu goes only on a monitor that is on. So both share a row.
    let mut on = o.enabled;
    let can_switch = if o.enabled { mon.off_allowed_for(sel) } else { editable };
    let can_menu = editable && o.enabled && mon.menu_choice() != Some(sel);
    form_row(ui, t, "On", |ui| {
        if ui.add_enabled(can_switch, Checkbox::new(&mut on)).changed() {
            event = Some(MonEvent::Enabled(sel.to_string(), on));
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
    let mut mode = mon.mode_of(sel).unwrap_or(usize::MAX);
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
        event = Some(MonEvent::Mode(sel.to_string(), mode));
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
    fn a_device_sits_beside_its_monitor_at_the_monitors_scale() {
        use crate::heads::Rotation;
        use crate::kanshi::OutputSet;
        let layout = Profile {
            outputs: vec![OutputSet {
                criteria: "DP-1".into(),
                enabled: true,
                mode: mode(1920, 1080, 60_000),
                rotation: Rotation::Normal,
                place: Place::Origin,
            }],
            menu_output: None,
        };
        let dev = |side, output: &str| DevPlace {
            name: "laptop".into(),
            output: output.into(),
            side,
            size: (960, 540),
            state: DevState::Connected,
            inactive: false,
        };
        let area = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(2880.0, 1080.0));
        let mon = MonTab::default();
        let (m, d) = arranged(&mon, &layout, &[dev(Side::RightOf, "DP-1")], area);
        assert_eq!(
            m[0].rect,
            Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(1920.0, 1080.0))
        );
        assert_eq!(
            d[0].rect,
            Rect::from_min_size(egui::pos2(1920.0, 0.0), vec2(960.0, 540.0))
        );
        // Left of the monitor: the monitor moves right.
        let (m, d) = arranged(&mon, &layout, &[dev(Side::LeftOf, "DP-1")], area);
        assert_eq!(d[0].rect.min, egui::pos2(0.0, 0.0));
        assert_eq!(m[0].rect.min, egui::pos2(960.0, 0.0));
        // A device whose edge another monitor took is not drawn over it.
        let mut docked = layout.clone();
        docked.outputs.push(OutputSet {
            criteria: "DP-2".into(),
            enabled: true,
            mode: mode(1920, 1080, 60_000),
            rotation: Rotation::Normal,
            place: Place::Next {
                to: "DP-1".into(),
                side: Side::RightOf,
            },
        });
        let mut busy = dev(Side::RightOf, "DP-1");
        busy.inactive = true;
        let (m, d) = arranged(&mon, &docked, &[busy.clone()], area);
        assert!(d.is_empty(), "the laptop box would lie inside DP-2");
        assert_eq!(m, boxes(&mon, &docked, area));
        // An inactive device that overlaps nothing is still drawn.
        let mut free = dev(Side::LeftOf, "DP-1");
        free.inactive = true;
        assert_eq!(arranged(&mon, &docked, &[free], area).1.len(), 1);
        // A device at a missing monitor is not drawn, and changes nothing.
        let (m, d) = arranged(&mon, &layout, &[dev(Side::LeftOf, "HDMI-1")], area);
        assert!(d.is_empty());
        assert_eq!(m, boxes(&mon, &layout, area));
    }

    #[test]
    fn going_back_counts_in_words() {
        assert_eq!(going_back(15), "Going back in 15 seconds.");
        assert_eq!(going_back(1), "Going back in 1 second.");
    }
}
