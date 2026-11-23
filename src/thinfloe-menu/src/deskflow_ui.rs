//! Drawing the Deskflow band of the Screen tab: the switch, this
//! computer's address with its Fingerprint… button and edge resistance,
//! then a row per client with its On, Clipboard and Compose switches and a
//! ⚙ that opens its dialog, and a row per computer that wants to attach.
//! The dialogs (a client's, *Check and attach…* and this computer's
//! fingerprint) are drawn by `dialog` over the tab. Everything that decides
//! something lives in deskflowtab.rs; this file only paints it and reports
//! clicks.

use crate::brand::{self, role};
use crate::config::DeviceCfg;
use crate::deskflow::ident;
use crate::deskflow::statefile::{DevLine, DevState};
use crate::deskflowcfg::Resistance;
use crate::deskflowtab::{self, DeskflowEvent, DeskflowTab, FP_CHANGED};
use crate::kanshi::Side;
use crate::monitors::MonTab;
use crate::monitors_ui::{DevPlace, column};
use crate::ui::{self, ROW_LEFT, ROW_MARGIN_X, ROW_RIGHT};
use egui::{Align, FontId, Layout, RichText, Sense, Ui, vec2};
use shadcn_egui::Theme;
use shadcn_egui::components::button::{Button, ButtonSize, ButtonVariant};
use shadcn_egui::components::select::Select;
use shadcn_egui::components::switch::Switch;
use shadcn_egui::icons::lucide;

/// Above and below the content of a band, as the monitor band.
const BAND_PAD: i8 = 12;
/// Between the fingerprint picture and the text beside it.
const ART_GAP: f32 = 16.0;
/// Before the buttons of a dialog, as the other dialogs.
const BUTTON_GAP: f32 = 8.0;
/// Above the list of clients, as above the list of monitors.
const LIST_GAP: f32 = 8.0;
/// Between two rows of the list of clients, as of the list of monitors.
const LIST_ROW_GAP: f32 = 2.0;
/// The state dot's column of the list of clients, and the dot's radius.
const DOT_W: f32 = 16.0;
const DOT_R: f32 = 4.0;
/// A switch column of the list of clients: room for its heading.
const CLIENT_SWITCH_W: f32 = 96.0;
/// The dialogs that show a fingerprint: its picture and two lines of hex
/// side by side.
const FP_DIALOG_W: f32 = 560.0;
/// Bytes of the colon hex on one line: two lines for 32 bytes.
const HEX_BYTES_PER_LINE: usize = 16;

const ON_TEXT: &str = "On";
const CLIPBOARD_TEXT: &str = "Share the clipboard";
const COMPOSE_TEXT: &str = "Handle Compose key on ThinFloe";
const LISTEN_TEXT: &str = "Let other computers use this keyboard and mouse (Deskflow)";
const CHECK_NOTE: &str = "Check this fingerprint on the other computer before you trust it there.";
const NOT_FREE: &str = "This edge is not free in the current arrangement.";
const NO_FREE_EDGE: &str = "No monitor edge is free in the current arrangement.";
const EDGE_PLACEHOLDER: &str = "Choose an edge";
const SELF_TITLE: &str = "Fingerprint of this computer";
const NO_SELF_FP: &str = "The fingerprint shows while Deskflow runs.";
const GONE: &str = "This computer is no longer attached.";
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
            enabled: d.enabled,
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
            if let Some(e) = self_and_resistance(ui, t, tab, ipv4) {
                event = Some(e);
            }
            if let Some(e) = client_rows(ui, t, tab, mon) {
                event = Some(e);
            }
            if let Some(e) = &tab.error {
                warn(ui, t, e);
            }
        });
    event
}

/// *This computer: …*, Fingerprint… and at the right end Edge
/// resistance, on one line; Edge resistance alone while the daemon shows
/// neither, so it can be set before Deskflow is on.
fn self_and_resistance(
    ui: &mut Ui,
    t: &Theme,
    tab: &DeskflowTab,
    ipv4: Option<std::net::Ipv4Addr>,
) -> Option<DeskflowEvent> {
    let line = deskflowtab::self_line(tab, ipv4);
    let has_fp = deskflowtab::self_fingerprint(tab).is_some();
    let mut event = None;
    if line.is_none() && !has_fp {
        return form_row(ui, t, "Edge resistance", |ui| resistance(ui, tab));
    }
    ui.horizontal(|ui| {
        if let Some(line) = line {
            ui.label(RichText::new(line).color(t.palette().foreground));
        }
        if has_fp
            && ui
                .add(Button::new("Fingerprint…").variant(ButtonVariant::Secondary))
                .clicked()
        {
            event = Some(DeskflowEvent::ShowFingerprint);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(e) = resistance(ui, tab) {
                event = Some(e);
            }
            ui.label(RichText::new("Edge resistance").color(t.palette().foreground));
        });
    });
    event
}

/// The Edge resistance select.
fn resistance(ui: &mut Ui, tab: &DeskflowTab) -> Option<DeskflowEvent> {
    let mut idx = RESISTANCES
        .iter()
        .position(|&r| r == tab.cfg.edge_resistance)
        .unwrap_or(0);
    if ui
        .add(Select::new("tf-deskflow-resistance", &mut idx, &RESISTANCE_LABELS))
        .changed()
    {
        return RESISTANCES.get(idx).map(|&r| DeskflowEvent::Resistance(r));
    }
    None
}

/// What a switch of a client sets.
type SwitchEvent = fn(String, bool) -> DeskflowEvent;

/// A client's On, Clipboard and Compose: the state, the dialog's label,
/// and the event a toggle sends.
fn switches(d: &DeviceCfg) -> [(bool, &'static str, SwitchEvent); 3] {
    [
        (d.enabled, ON_TEXT, |name, on| DeskflowEvent::Enabled { name, on }),
        (d.clipboard, CLIPBOARD_TEXT, |name, on| DeskflowEvent::Clipboard {
            name,
            on,
        }),
        (d.compose, COMPOSE_TEXT, |name, on| DeskflowEvent::Compose { name, on }),
    ]
}

/// The list of clients: a heading, a row per attached client, then a row
/// per computer that wants to attach.
fn client_rows(ui: &mut Ui, t: &Theme, tab: &DeskflowTab, mon: &MonTab) -> Option<DeskflowEvent> {
    let pending = deskflowtab::pending(tab);
    if tab.cfg.devices.is_empty() && pending.is_empty() {
        return None;
    }
    let p = t.palette();
    let mut event = None;
    ui.add_space(LIST_GAP);
    if !tab.cfg.devices.is_empty() {
        ui.horizontal(|ui| {
            let heading = |text: &str| RichText::new(text).size(brand::ROW_STATUS_PT).color(p.muted_foreground);
            // The names start where the monitor names do; the dots stand in
            // their own column before them.
            column(ui, DOT_W, |_| ());
            column(ui, brand::FORM_LABEL_W - DOT_W, |ui| ui.label(heading("Computer")));
            for text in ["On", "Clipboard", "Compose"] {
                column(ui, CLIENT_SWITCH_W, |ui| ui.label(heading(text)));
            }
        });
    }
    for d in &tab.cfg.devices {
        if let Some(e) = client_row(ui, t, tab, mon, d) {
            event = Some(e);
        }
    }
    for line in pending {
        if let Some(e) = pending_row(ui, t, line) {
            event = Some(e);
        }
    }
    event
}

/// An attached client: its state dot, its name (muted while it is off),
/// On, Clipboard, Compose and ⚙; the note under it when its edge is not
/// free.
fn client_row(ui: &mut Ui, t: &Theme, tab: &DeskflowTab, mon: &MonTab, d: &DeviceCfg) -> Option<DeskflowEvent> {
    let p = t.palette();
    let mut event = None;
    ui.add_space(LIST_ROW_GAP);
    ui.horizontal(|ui| {
        column(ui, DOT_W, |ui| state_dot(ui, t, deskflowtab::device_state(tab, d)));
        let colour = if d.enabled { p.foreground } else { p.muted_foreground };
        column(ui, brand::FORM_LABEL_W - DOT_W, |ui| {
            ui.add(egui::Label::new(RichText::new(&d.name).color(colour)).truncate());
        });
        for (on, _, make) in switches(d) {
            let mut on = on;
            if column(ui, CLIENT_SWITCH_W, |ui| ui.add(Switch::new(&mut on)).changed()) {
                event = Some(make(d.name.clone(), on));
            }
        }
        let gear = Button::new(format!("Settings of {}", d.name))
            .icon(lucide::SETTINGS)
            .size(ButtonSize::Icon)
            .variant(ButtonVariant::Secondary);
        if ui.add(gear).clicked() {
            event = Some(DeskflowEvent::Select(d.name.clone()));
        }
    });
    if edge_not_free(tab, mon, d) {
        warn(ui, t, NOT_FREE);
    }
    event
}

/// The edge of `d` is not free, or the monitors are not known yet.
fn edge_not_free(tab: &DeskflowTab, mon: &MonTab, d: &DeviceCfg) -> bool {
    mon.layout()
        .is_none_or(|l| deskflowtab::inactive(l, mon.heads(), &tab.cfg.devices, d))
}

/// The dot of a client's state: connected or active in the good colour,
/// offline muted.
fn state_dot(ui: &mut Ui, t: &Theme, state: DevState) {
    let (rect, _) = ui.allocate_exact_size(vec2(DOT_W, DOT_W), Sense::hover());
    let colour = match state {
        DevState::Active | DevState::Connected => t.role(role::GOOD),
        _ => t.palette().muted_foreground,
    };
    ui.painter().circle_filled(rect.center(), DOT_R, colour);
}

/// A computer that wants to attach: "! <name> wants to attach" (or the
/// changed-fingerprint warning), Check and attach… and Reject.
fn pending_row(ui: &mut Ui, t: &Theme, line: &DevLine) -> Option<DeskflowEvent> {
    let mut event = None;
    let name = &line.name;
    ui.add_space(LIST_ROW_GAP);
    ui.horizontal(|ui| {
        column(ui, DOT_W, |ui| ui.label(RichText::new("!").color(t.role(role::WARN))));
        if line.flags.iter().any(|f| f == FP_CHANGED) {
            warn(
                ui,
                t,
                &format!("{name} has a new fingerprint. Attach it again only if you expect this."),
            );
        } else {
            ui.label(RichText::new(format!("{name} wants to attach")).color(t.palette().foreground));
        }
        if ui.add(Button::new("Check and attach…")).clicked() {
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
    event
}

/// The open dialog of the band, over the tab: a client's, *Check and
/// attach…* or this computer's fingerprint. Every state in which
/// `deskflowtab::dialog_open` holds draws one, with a button and Escape
/// that close it. Nothing while none is open.
pub fn dialog(ui: &Ui, tab: &DeskflowTab, mon: &MonTab) -> Option<DeskflowEvent> {
    if let Some(name) = &tab.selected {
        return client_dialog(ui, tab, mon, name);
    }
    if let Some((name, fp)) = &tab.attaching {
        return attach_dialog(ui, tab, mon, name, fp);
    }
    if tab.fingerprint_open {
        return fingerprint_dialog(ui, tab);
    }
    None
}

fn escaped(ui: &Ui) -> bool {
    ui.input(|i| i.key_pressed(egui::Key::Escape))
}

/// A client's dialog: Position, On, Clipboard, Compose, its fingerprint,
/// Forget and Close. A client the configuration no longer holds shows that
/// it is gone, and Close.
fn client_dialog(ui: &Ui, tab: &DeskflowTab, mon: &MonTab, name: &str) -> Option<DeskflowEvent> {
    let d = tab.cfg.devices.iter().find(|d| d.name == name);
    let mut event = None;
    ui::dialog(ui, name, |ui, t| {
        let Some(d) = d else {
            muted(ui, t, GONE);
            ui.add_space(BUTTON_GAP);
            if ui.add(Button::new("Close")).clicked() {
                event = Some(DeskflowEvent::CloseDialog);
            }
            return;
        };
        if let Some((output, side)) = position(ui, t, tab, mon, d) {
            event = Some(DeskflowEvent::Place {
                name: d.name.clone(),
                output,
                side,
            });
        }
        if edge_not_free(tab, mon, d) {
            warn(ui, t, NOT_FREE);
        }
        for (on, label, make) in switches(d) {
            if let Some(on) = switch_row(ui, t, on, label) {
                event = Some(make(d.name.clone(), on));
            }
        }
        hex(ui, t, &d.fingerprint);
        if ui
            .add(Button::new(format!("Forget {}", d.name)).variant(ButtonVariant::Secondary))
            .clicked()
        {
            event = Some(DeskflowEvent::Forget { name: d.name.clone() });
        }
        ui.add_space(BUTTON_GAP);
        if ui.add(Button::new("Close")).clicked() {
            event = Some(DeskflowEvent::CloseDialog);
        }
    });
    event.or_else(|| escaped(ui).then_some(DeskflowEvent::CloseDialog))
}

/// *Check and attach…*: the computer's fingerprint, the check note, and a
/// Position select of the free edges. The edge chosen stays in the dialog
/// until Attach places the computer there.
fn attach_dialog(ui: &Ui, tab: &DeskflowTab, mon: &MonTab, name: &str, fp: &str) -> Option<DeskflowEvent> {
    let free = free_edges(tab, mon, name);
    let key = egui::Id::unique(("tf-deskflow-attach-edge", name, fp));
    let mut chosen: Option<(String, Side)> = ui
        .data(|d| d.get_temp::<(String, Side)>(key))
        .filter(|e| free.contains(e));
    let mut event = None;
    ui::dialog_w(ui, name, FP_DIALOG_W, |ui, t| {
        fingerprint(ui, t, fp, |ui| muted(ui, t, CHECK_NOTE));
        if free.is_empty() {
            muted(ui, t, NO_FREE_EDGE);
        } else {
            let labels = edge_labels(mon, &free);
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let mut idx = chosen
                .as_ref()
                .and_then(|c| free.iter().position(|e| e == c))
                .unwrap_or(usize::MAX);
            let id = format!("tf-deskflow-attach-{name}");
            let changed = form_row(ui, t, "Position", |ui| {
                ui.add(Select::new(&id, &mut idx, &refs).placeholder(EDGE_PLACEHOLDER))
                    .changed()
            });
            if changed && let Some(e) = free.get(idx) {
                chosen = Some(e.clone());
                ui.data_mut(|d| d.insert_temp(key, e.clone()));
            }
        }
        ui.add_space(BUTTON_GAP);
        ui.horizontal(|ui| {
            if ui.add_enabled(chosen.is_some(), Button::new("Attach")).clicked()
                && let Some((output, side)) = chosen.clone()
            {
                event = Some(DeskflowEvent::Place {
                    name: name.to_string(),
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
        });
    });
    let event = event.or_else(|| escaped(ui).then_some(DeskflowEvent::CancelAttach));
    if event.is_some() {
        ui.data_mut(|d| d.remove::<(String, Side)>(key));
    }
    event
}

/// This computer's fingerprint, to check on the other computer.
fn fingerprint_dialog(ui: &Ui, tab: &DeskflowTab) -> Option<DeskflowEvent> {
    let mut event = None;
    ui::dialog_w(ui, SELF_TITLE, FP_DIALOG_W, |ui, t| {
        match deskflowtab::self_fingerprint(tab) {
            Some(fp) => fingerprint(ui, t, fp, |ui| muted(ui, t, CHECK_NOTE)),
            None => muted(ui, t, NO_SELF_FP),
        }
        ui.add_space(BUTTON_GAP);
        if ui.add(Button::new("Close")).clicked() {
            event = Some(DeskflowEvent::CloseDialog);
        }
    });
    event.or_else(|| escaped(ui).then_some(DeskflowEvent::CloseDialog))
}

/// A labeled switch row; returns the new state when toggled.
fn switch_row(ui: &mut Ui, t: &Theme, mut on: bool, label: &str) -> Option<bool> {
    let mut next = None;
    ui.horizontal(|ui| {
        if ui.add(Switch::new(&mut on)).changed() {
            next = Some(on);
        }
        ui.label(RichText::new(label).color(t.palette().foreground));
    });
    next
}

/// The edges the computer `name` can take, by criteria.
fn free_edges(tab: &DeskflowTab, mon: &MonTab, name: &str) -> Vec<(String, Side)> {
    mon.layout().map_or(Vec::new(), |l| {
        deskflowtab::position_edges(l, mon.heads(), &tab.cfg.devices, name)
    })
}

/// "Right of DP-1" for each edge.
fn edge_labels(mon: &MonTab, edges: &[(String, Side)]) -> Vec<String> {
    edges
        .iter()
        .map(|(c, s)| deskflowtab::edge_label(*s, monitor_name(mon, c)))
        .collect()
}

fn monitor_name<'a>(mon: &'a MonTab, criteria: &'a str) -> &'a str {
    mon.head(criteria).map_or(criteria, |h| h.name.as_str())
}

/// The Position control of an attached client: one select of the free
/// edges ("Right of DP-1"); its own edge shows even when it is not free.
/// The edge chosen, by criteria.
fn position(ui: &mut Ui, t: &Theme, tab: &DeskflowTab, mon: &MonTab, d: &DeviceCfg) -> Option<(String, Side)> {
    let free = free_edges(tab, mon, &d.name);
    let labels = edge_labels(mon, &free);
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let mut idx = free
        .iter()
        .position(|(c, s)| *c == d.output && *s == d.side)
        .unwrap_or(usize::MAX);
    let placeholder = deskflowtab::edge_label(d.side, monitor_name(mon, &d.output));
    let id = format!("tf-deskflow-position-{}", d.name);
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
                .font(mono)
                .color(t.palette().foreground),
        );
        ui.add_space(ART_GAP);
        ui.vertical(|ui| {
            hex(ui, t, fp);
            beside(ui);
        });
    });
}

/// The colon hex of a fingerprint, on two lines.
fn hex(ui: &mut Ui, t: &Theme, fp: &str) {
    let hex = ident::colon_hex(fp);
    let pairs: Vec<&str> = hex.split(':').collect();
    let lines: Vec<String> = pairs.chunks(HEX_BYTES_PER_LINE).map(|c| c.join(":")).collect();
    ui.label(
        RichText::new(lines.join("\n"))
            .font(FontId::monospace(brand::ROW_STATUS_PT))
            .color(t.palette().muted_foreground),
    );
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
