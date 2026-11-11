//! Drawing the Status tab of Settings: one row per status element with its
//! tick box, size select and Up and Down buttons, then the preview, which is
//! the status area itself, drawn by the code that draws it on the main
//! screen. What a click changes is decided in statustab.rs and menu.rs.

use crate::brand::{self, role};
use crate::menu::Menu;
use crate::status::{COLUMNS, Data};
use crate::statustab::{self, Row, StatusEvent};
use crate::ui::{self, GAP, ROW_LEFT, ROW_MARGIN_X, ROW_RIGHT, StatusPlace, TILE_GAP};
use egui::{Align, Layout, Rect, RichText, Sense, Ui, Vec2, vec2};
use shadcn_egui::Theme;
use shadcn_egui::components::button::{Button, ButtonSize, ButtonVariant};
use shadcn_egui::components::checkbox::Checkbox;
use shadcn_egui::components::select::Select;
use std::time::Instant;

/// Above and below the content of a row.
const ROW_PAD: i8 = 6;

/// Where the main screen puts the status area, for the preview.
#[derive(Debug, Clone, Copy)]
pub struct PreviewPlace {
    /// The size of one cell on the main screen.
    pub cell: Vec2,
    /// The screen is short: the elements take their short sizes.
    pub short: bool,
    /// A window covers the menu: the preview asks for no frames.
    pub covered: bool,
}

/// The Status tab in the rest of `ui`'s rectangle: the rows, and below them
/// Reset and the preview, pinned to the foot of the panel only when the rows
/// scroll.
pub fn status_tab(ui: &mut Ui, menu: &Menu, place: PreviewPlace, now: Instant) -> Option<StatusEvent> {
    let rect = Rect::from_min_max(ui.cursor().min, ui.max_rect().max);
    let data = menu.status_sources();
    let rows = statustab::rows(&menu.status, &data, place.short);
    let (footer, list) = ui::page_with_footer(
        ui,
        "status",
        rect,
        GAP,
        |ui| footer(ui, menu, &data, place, now),
        |ui| {
            ui.spacing_mut().item_spacing.y = TILE_GAP;
            let mut event = None;
            for row in &rows {
                if let Some(e) = row_band(ui, row) {
                    event = Some(e);
                }
            }
            // The footer's gap belongs to the page.
            ui.add_space(GAP);
            event
        },
    );
    list.or(footer)
}

/// One element: tick box, name, why it is not drawn, size select, Up, Down.
fn row_band(ui: &mut Ui, row: &Row) -> Option<StatusEvent> {
    let t = Theme::current(ui.ctx());
    let mut event = None;
    let inner_w = ui.available_width() - ROW_MARGIN_X;
    egui::Frame::new()
        .fill(t.palette().card)
        .inner_margin(egui::Margin {
            left: ROW_LEFT,
            right: ROW_RIGHT,
            top: ROW_PAD,
            bottom: ROW_PAD,
        })
        .show(ui, |ui| {
            ui.set_width(inner_w);
            ui.horizontal(|ui| {
                let mut on = row.shown;
                if ui.add(Checkbox::new(&mut on)).changed() {
                    event = Some(StatusEvent::Shown(row.id.into(), on));
                }
                ui.label(RichText::new(row.name).color(t.palette().foreground));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let small = |text: &str| Button::new(text).variant(ButtonVariant::Secondary).size(ButtonSize::Sm);
                    if ui.add_enabled(!row.last, small("Down")).clicked() {
                        event = Some(StatusEvent::Down(row.id.into()));
                    }
                    if ui.add_enabled(!row.first, small("Up")).clicked() {
                        event = Some(StatusEvent::Up(row.id.into()));
                    }
                    let labels: Vec<String> = row.sizes.iter().map(ToString::to_string).collect();
                    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                    let mut idx = row.sizes.iter().position(|&s| s == row.size).unwrap_or(0);
                    let id = format!("tf-status-size-{}", row.id);
                    let choose = row.sizes.len() > 1;
                    if ui.add_enabled(choose, Select::new(&id, &mut idx, &labels)).changed()
                        && let Some(&size) = row.sizes.get(idx)
                    {
                        event = Some(StatusEvent::Size(row.id.into(), size));
                    }
                    if let Some(note) = row.note {
                        ui.add(
                            egui::Label::new(RichText::new(note.text()).color(t.palette().muted_foreground)).truncate(),
                        );
                    }
                });
            });
        });
    event
}

/// Reset, a failed write, and the preview.
fn footer(ui: &mut Ui, menu: &Menu, data: &Data, place: PreviewPlace, now: Instant) -> Option<StatusEvent> {
    let t = Theme::current(ui.ctx());
    let mut event = None;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Preview")
                .font(brand::geo(brand::SECTION_PT))
                .color(t.palette().foreground),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add(Button::new("Reset to default").variant(ButtonVariant::Secondary))
                .clicked()
            {
                event = Some(StatusEvent::Reset);
            }
        });
    });
    if let Some(e) = &menu.status_error {
        ui.label(RichText::new(format!("The status layout could not be saved: {e}")).color(t.role(role::WARN)));
    }
    let rows = menu.status.rows(data, place.short);
    if rows == 0 {
        ui.label(RichText::new("Nothing to show.").color(t.palette().muted_foreground));
        return event;
    }
    // The cells of the main screen, so the picture is the one it shows.
    let span = |n: usize, size: f32| n as f32 * size + (n as f32 - 1.0) * TILE_GAP;
    let size = vec2(span(COLUMNS as usize, place.cell.x), span(rows, place.cell.y));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let at = StatusPlace {
        origin: rect.min,
        cell: place.cell,
        short: place.short,
    };
    ui::paint_status(ui, &menu.status, data, at, now, place.covered);
    event
}
