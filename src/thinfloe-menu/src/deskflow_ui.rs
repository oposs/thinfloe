//! Drawing the Deskflow band of the Screen tab: the switch, this
//! computer's address and fingerprint, edge resistance, the devices that
//! want to attach, and the settings of the device selected in the
//! arrangement. Everything that decides something lives in deskflowtab.rs;
//! this file only paints it and reports clicks.

use crate::brand::{self, role};
use crate::config::DeviceCfg;
use crate::deskflow::ident;
use crate::deskflow::statefile::DevLine;
use crate::deskflowcfg::Resistance;
use crate::deskflowtab::{self, DeskflowEvent, DeskflowTab, FP_CHANGED};
use crate::kanshi::Side;
use crate::monitors::MonTab;
use crate::monitors_ui::DevPlace;
use crate::ui::{ROW_LEFT, ROW_MARGIN_X, ROW_RIGHT};
use egui::{FontId, RichText, Ui};
use shadcn_egui::Theme;
use shadcn_egui::components::button::{Button, ButtonVariant};
use shadcn_egui::components::select::Select;
use shadcn_egui::components::switch::Switch;

/// Above and below the content of a band, as the monitor band.
const BAND_PAD: i8 = 12;
/// Between the fingerprint picture and the text beside it.
const ART_GAP: f32 = 16.0;
/// Before a device's block, so two pending devices read apart.
const BLOCK_GAP: f32 = 8.0;
/// Bytes of the colon hex on one line: two lines for 32 bytes.
const HEX_BYTES_PER_LINE: usize = 16;

const LISTEN_TEXT: &str = "Let other computers use this keyboard and mouse (Deskflow)";
const CHECK_NOTE: &str = "Check this fingerprint on the other computer before you trust it there.";
const NOT_FREE: &str = "This edge is not free in the current arrangement.";
const NO_FREE_EDGE: &str = "No monitor edge is free in the current arrangement.";
const RESISTANCES: [Resistance; 4] = [Resistance::Off, Resistance::Low, Resistance::Medium, Resistance::High];
const RESISTANCE_LABELS: [&str; 4] = ["Off", "Low", "Medium", "High"];

/// The attached devices as the arrangement draws them.
pub fn places(tab: &DeskflowTab, mon: &MonTab) -> Vec<DevPlace> {
    let Some(layout) = mon.layout() else {
        return Vec::new();
    };
    tab.cfg
        .devices
        .iter()
        .map(|d| DevPlace {
            name: d.name.clone(),
            output: d.output.clone(),
            side: d.side,
            size: deskflowtab::device_size(&tab.state, d),
            state: deskflowtab::device_state(tab, d),
            inactive: deskflowtab::inactive(layout, mon.heads(), &tab.cfg.devices, d),
        })
        .collect()
}

/// The band, `width` wide as the panel, under the monitor band. `ipv4` is
/// the machine's address, for the *This computer* line.
pub fn band(
    ui: &mut Ui,
    t: &Theme,
    tab: &DeskflowTab,
    mon: &MonTab,
    ipv4: Option<std::net::Ipv4Addr>,
    width: f32,
) -> Option<DeskflowEvent> {
    let mut event = None;
    egui::Frame::new()
        .fill(t.palette().card)
        .inner_margin(egui::Margin {
            left: ROW_LEFT,
            right: ROW_RIGHT,
            top: BAND_PAD,
            bottom: BAND_PAD,
        })
        .show(ui, |ui| {
            ui.set_width(width - ROW_MARGIN_X);
            ui.label(
                RichText::new("Deskflow")
                    .font(brand::geo(brand::SECTION_PT))
                    .color(t.palette().foreground),
            );
            let mut on = tab.cfg.listen;
            ui.horizontal(|ui| {
                if ui.add(Switch::new(&mut on)).changed() {
                    event = Some(DeskflowEvent::Listen(on));
                }
                ui.label(RichText::new(LISTEN_TEXT).color(t.palette().foreground));
            });
            if let Some(line) = deskflowtab::stopped_line(tab) {
                warn(ui, t, &line);
            }
            if let Some(line) = deskflowtab::self_line(tab, ipv4) {
                ui.label(RichText::new(line).color(t.palette().foreground));
            }
            if let Some(fp) = deskflowtab::self_fingerprint(tab) {
                fingerprint(ui, t, fp, |ui| muted(ui, t, CHECK_NOTE));
            }
            let mut idx = RESISTANCES
                .iter()
                .position(|&r| r == tab.cfg.edge_resistance)
                .unwrap_or(0);
            form_row(ui, t, "Edge resistance", |ui| {
                if ui
                    .add(Select::new("tf-deskflow-resistance", &mut idx, &RESISTANCE_LABELS))
                    .changed()
                    && let Some(&r) = RESISTANCES.get(idx)
                {
                    event = Some(DeskflowEvent::Resistance(r));
                }
            });
            for line in deskflowtab::pending(&tab.state) {
                if let Some(e) = pending_row(ui, t, tab, mon, line) {
                    event = Some(e);
                }
            }
            for d in deskflowtab::band_devices(tab, mon.layout(), mon.heads()) {
                if let Some(e) = device_rows(ui, t, tab, mon, d) {
                    event = Some(e);
                }
            }
            if let Some(e) = &tab.error {
                warn(ui, t, e);
            }
        });
    event
}

/// A device that wants to attach: its name, its fingerprint, and Attach…
/// and Reject, or the Position control after Attach….
fn pending_row(ui: &mut Ui, t: &Theme, tab: &DeskflowTab, mon: &MonTab, line: &DevLine) -> Option<DeskflowEvent> {
    let mut event = None;
    ui.add_space(BLOCK_GAP);
    let name = &line.name;
    if line.flags.iter().any(|f| f == FP_CHANGED) {
        warn(
            ui,
            t,
            &format!("{name} has a new fingerprint. Attach it again only if you expect this."),
        );
    } else {
        ui.label(RichText::new(format!("{name} wants to attach")).color(t.palette().foreground));
    }
    let attaching = tab
        .attaching
        .as_ref()
        .is_some_and(|(n, f)| n == name && *f == line.fingerprint);
    fingerprint(ui, t, &line.fingerprint, |ui| {
        if attaching {
            if let Some((output, side)) = position(ui, t, tab, mon, name, None) {
                event = Some(DeskflowEvent::Place {
                    name: name.clone(),
                    output,
                    side,
                });
            }
            if ui
                .add(Button::new("Cancel").variant(ButtonVariant::Secondary))
                .clicked()
            {
                event = Some(DeskflowEvent::CancelAttach);
            }
            return;
        }
        ui.horizontal(|ui| {
            if ui.add(Button::new("Attach…")).clicked() {
                event = Some(DeskflowEvent::Attach {
                    name: name.clone(),
                    fp: line.fingerprint.clone(),
                });
            }
            if ui
                .add(Button::new("Reject").variant(ButtonVariant::Secondary))
                .clicked()
            {
                event = Some(DeskflowEvent::Reject {
                    name: name.clone(),
                    fp: line.fingerprint.clone(),
                });
            }
        });
    });
    event
}

/// An attached device: Position and Forget, with the note when its edge is
/// not free.
fn device_rows(ui: &mut Ui, t: &Theme, tab: &DeskflowTab, mon: &MonTab, d: &DeviceCfg) -> Option<DeskflowEvent> {
    let mut event = None;
    ui.add_space(BLOCK_GAP);
    ui.label(RichText::new(&d.name).color(t.palette().foreground));
    if let Some((output, side)) = position(ui, t, tab, mon, &d.name, Some(d)) {
        event = Some(DeskflowEvent::Place {
            name: d.name.clone(),
            output,
            side,
        });
    }
    let inactive = mon
        .layout()
        .is_none_or(|l| deskflowtab::inactive(l, mon.heads(), &tab.cfg.devices, d));
    if inactive {
        warn(ui, t, NOT_FREE);
    }
    if ui
        .add(Button::new("Forget").variant(ButtonVariant::Secondary))
        .clicked()
    {
        event = Some(DeskflowEvent::Forget { name: d.name.clone() });
    }
    event
}

/// The Position control: one select of the free edges ("Right of DP-1").
/// `current` is an attached device, whose own edge shows even when it is
/// not free. The edge chosen, by criteria.
fn position(
    ui: &mut Ui,
    t: &Theme,
    tab: &DeskflowTab,
    mon: &MonTab,
    name: &str,
    current: Option<&DeviceCfg>,
) -> Option<(String, Side)> {
    let free = mon.layout().map_or(Vec::new(), |l| {
        deskflowtab::free_edges(l, mon.heads(), &tab.cfg.devices, current.map(|d| d.name.as_str()))
    });
    if free.is_empty() && current.is_none() {
        muted(ui, t, NO_FREE_EDGE);
        return None;
    }
    let monitor = |c: &str| mon.head(c).map_or(c.to_string(), |h| h.name.clone());
    let labels: Vec<String> = free
        .iter()
        .map(|(c, s)| deskflowtab::edge_label(*s, &monitor(c)))
        .collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let mut idx = current
        .and_then(|d| free.iter().position(|(c, s)| *c == d.output && *s == d.side))
        .unwrap_or(usize::MAX);
    let placeholder = current.map_or(String::new(), |d| deskflowtab::edge_label(d.side, &monitor(&d.output)));
    let id = format!("tf-deskflow-position-{name}");
    let changed = form_row(ui, t, "Position", |ui| {
        ui.add_enabled(
            !free.is_empty(),
            Select::new(&id, &mut idx, &refs).placeholder(placeholder),
        )
        .changed()
    });
    if changed { free.get(idx).cloned() } else { None }
}

/// A fingerprint as Deskflow shows it: the random-art picture, and beside
/// it the colon hex and what `beside` adds.
fn fingerprint(ui: &mut Ui, t: &Theme, fp: &str, beside: impl FnOnce(&mut Ui)) {
    let mono = FontId::monospace(brand::ROW_STATUS_PT);
    ui.horizontal_top(|ui| {
        ui.label(
            RichText::new(ident::random_art(fp).join("\n"))
                .font(mono.clone())
                .color(t.palette().foreground),
        );
        ui.add_space(ART_GAP);
        ui.vertical(|ui| {
            let hex = ident::colon_hex(fp);
            let pairs: Vec<&str> = hex.split(':').collect();
            let lines: Vec<String> = pairs.chunks(HEX_BYTES_PER_LINE).map(|c| c.join(":")).collect();
            ui.label(
                RichText::new(lines.join("\n"))
                    .font(mono)
                    .color(t.palette().muted_foreground),
            );
            beside(ui);
        });
    });
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
