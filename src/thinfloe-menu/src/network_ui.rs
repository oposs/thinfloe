//! Drawing the Settings screen's Network tab: the connections now, the
//! wired address, the saved WiFi networks and Apply, and the two dialogs
//! over it (add a network, confirm an Apply during a remote session).
//! Everything that decides something lives in network.rs; this file only
//! paints it and reports clicks.

use crate::brand::{self, role};
use crate::netfiles::{self, FileState};
use crate::nettab;
use crate::network::{AddView, NetEvent, NetTab};
use crate::ui::{self, GAP, Icon, ROW_LEFT, ROW_MARGIN_X, ROW_RIGHT, TILE_GAP};
use egui::{RichText, Sense, Ui};
use shadcn_egui::Theme;
use shadcn_egui::components::badge::{Badge, BadgeVariant};
use shadcn_egui::components::button::{Button, ButtonVariant};
use shadcn_egui::components::checkbox::Checkbox;
use shadcn_egui::components::field::{Field, field_label};
use shadcn_egui::components::input::Input;
use shadcn_egui::components::item::{Item, ItemSize};
use shadcn_egui::components::select::Select;

/// The mark of a secured network.
pub const LOCK: &str = "\u{1f512}";
/// The wired select: index 1 is a fixed address.
const WIRED_MODES: [&str; 2] = ["Automatic (DHCP)", "Fixed address"];
const PASSWORD_NOTE: &str = "The password is saved with the home directory. Anyone who has the stick can read it.";
/// Above and below the content of a band, as a Packages row.
const BAND_PAD: i8 = 12;
/// Before the buttons of a dialog, as the other dialogs.
const BUTTON_GAP: f32 = 8.0;
/// The tab's scroll bar is drawn whole, also while the pointer is away.
const SCROLL_REST_OPACITY: f32 = 1.0;

/// A dialog of the tab is open; the screen under it takes no clicks.
pub fn dialog_open(net: &NetTab) -> bool {
    net.add_view().is_some() || net.confirm_open()
}

/// The Network tab, `width` wide, and its dialogs.
pub fn network_tab(ui: &mut Ui, net: &NetTab, _icons: &[Option<Icon>], width: f32) -> Option<NetEvent> {
    let t = Theme::current(ui.ctx());
    let mut event = None;
    // Apply and its error stand at the foot of the panel, so a page that
    // scrolls never hides them; the bands scroll in the room above.
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        if let Some(e) = net.error() {
            warn(ui, &t, e);
        }
        let label = if net.applying() { "Applying…" } else { "Apply" };
        if ui
            .add_enabled(net.dirty() && !net.applying(), Button::new(label))
            .clicked()
        {
            event = Some(NetEvent::Apply);
        }
        ui.add_space(GAP);
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            // A solid bar, drawn also at rest, as on the Upgrade screen:
            // egui's floating one is invisible until hovered.
            ui.spacing_mut().scroll = egui::style::ScrollStyle {
                dormant_background_opacity: SCROLL_REST_OPACITY,
                dormant_handle_opacity: SCROLL_REST_OPACITY,
                active_background_opacity: SCROLL_REST_OPACITY,
                active_handle_opacity: SCROLL_REST_OPACITY,
                ..egui::style::ScrollStyle::solid()
            };
            egui::ScrollArea::vertical()
                .id_salt("network")
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                .show(ui, |ui| {
                    let inner = ui.available_width().min(width) - ROW_MARGIN_X;
                    let between = ui.spacing().item_spacing.y;
                    ui.spacing_mut().item_spacing.y = TILE_GAP;
                    band(ui, &t, inner, between, "Connections now", |ui| connections(ui, &t, net));
                    // Nothing to edit before the files are read, nor while
                    // an Apply or a take-over runs.
                    if net.wifi_file().is_some() && net.wired_file().is_some() {
                        ui.add_enabled_ui(!net.applying() && !net.taking_over(), |ui| {
                            band(ui, &t, inner, between, "Wired", |ui| wired(ui, &t, net, &mut event));
                            band(ui, &t, inner, between, "WiFi networks", |ui| {
                                wifi(ui, &t, net, &mut event)
                            });
                        });
                    }
                });
        });
    });
    if let Some(view) = net.add_view() {
        if let Some(e) = add_dialog(ui, view) {
            event = Some(e);
        }
    } else if net.confirm_open()
        && let Some(e) = confirm_dialog(ui)
    {
        event = Some(e);
    }
    event
}

/// A TILE band with its heading, as the rows of the Packages tab.
/// `between`: the item spacing inside it.
fn band(ui: &mut Ui, t: &Theme, width: f32, between: f32, title: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(t.palette().card)
        .inner_margin(egui::Margin {
            left: ROW_LEFT,
            right: ROW_RIGHT,
            top: BAND_PAD,
            bottom: BAND_PAD,
        })
        .show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = between;
            ui.label(
                RichText::new(title)
                    .font(brand::geo(brand::SECTION_PT))
                    .color(t.palette().foreground),
            );
            add(ui);
        });
}

fn muted(ui: &mut Ui, t: &Theme, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).color(t.palette().muted_foreground));
}

fn warn(ui: &mut Ui, t: &Theme, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).color(t.role(role::WARN)));
}

/// A line under a form's fields, in the label column's shadow.
fn note(ui: &mut Ui, indent: f32, text: RichText) {
    ui.horizontal(|ui| {
        ui.add_space(indent);
        ui.add(egui::Label::new(text).truncate());
    });
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

/// A form row with a text input; the new text when it changed.
fn text_row(ui: &mut Ui, t: &Theme, label: &str, value: &str, hint: &'static str, salt: &str) -> Option<String> {
    form_row(ui, t, label, |ui| {
        let mut text = value.to_string();
        let r = ui.add(
            Input::new(&mut text)
                .hint(hint)
                .max_width(brand::NET_FIELD_W)
                .id_salt(salt),
        );
        r.changed().then_some(text)
    })
}

/// One muted line per interface, or why there is none.
fn connections(ui: &mut Ui, t: &Theme, net: &NetTab) {
    for i in net.ifaces() {
        muted(ui, t, nettab::iface_line(i));
    }
    match net.status_error() {
        Some(e) => warn(ui, t, e),
        None if net.ifaces().is_empty() => muted(ui, t, "No network adapter found"),
        None => {}
    }
}

/// The wired address: the select and, for a fixed address, its fields.
/// A file written by hand shows them greyed.
fn wired(ui: &mut Ui, t: &Theme, net: &NetTab, event: &mut Option<NetEvent>) {
    let Some(file) = net.wired_file() else {
        return;
    };
    if net.netboot() {
        muted(ui, t, "A netbooted machine always uses DHCP.");
        return;
    }
    let form = net.wired_form();
    let editable = file.state != FileState::Hand;
    ui.add_enabled_ui(editable, |ui| {
        let mut idx = usize::from(form.fixed);
        let picked = form_row(ui, t, "Address", |ui| {
            ui.add(Select::new("tf-net-wired", &mut idx, &WIRED_MODES)).changed()
        });
        if picked {
            *event = Some(NetEvent::WiredMode(idx == 1));
        }
        if !form.fixed {
            return;
        }
        let mut typed = form.clone();
        let mut changed = false;
        let rows: [(&str, &str, &'static str, &str, &mut String); 3] = [
            (
                "Address",
                &form.address,
                "192.168.1.10/24",
                "tf-net-address",
                &mut typed.address,
            ),
            (
                "Gateway",
                &form.gateway,
                "192.168.1.1",
                "tf-net-gateway",
                &mut typed.gateway,
            ),
            (
                "DNS servers",
                &form.dns,
                "1.1.1.1, 9.9.9.9",
                "tf-net-dns",
                &mut typed.dns,
            ),
        ];
        for (label, value, hint, salt, slot) in rows {
            if let Some(text) = text_row(ui, t, label, value, hint, salt) {
                *slot = text;
                changed = true;
            }
        }
        if changed {
            *event = Some(NetEvent::WiredText {
                address: typed.address,
                gateway: typed.gateway,
                dns: typed.dns,
            });
        }
    });
    let indent = brand::FORM_LABEL_W + ui.spacing().item_spacing.x;
    if !editable {
        by_hand(
            ui,
            t,
            indent,
            netfiles::WIRED_FILE,
            file.manageable,
            "wired",
            net,
            event,
        );
    }
    if let Some(e) = &file.error {
        note(ui, indent, RichText::new(e).color(t.role(role::WARN)));
    }
}

/// The saved networks, and Add network. A file written by hand shows the
/// names only.
fn wifi(ui: &mut Ui, t: &Theme, net: &NetTab, event: &mut Option<NetEvent>) {
    let Some(file) = net.wifi_file() else {
        return;
    };
    let hand = file.state == FileState::Hand;
    for n in net.networks() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&n.ssid).color(t.palette().foreground));
            if hand {
                return;
            }
            if n.password.is_some() {
                muted(ui, t, LOCK);
            }
            if n.hidden {
                ui.add(Badge::new("Hidden").variant(BadgeVariant::Outline));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(Button::new("Forget").variant(ButtonVariant::Secondary))
                    .clicked()
                {
                    *event = Some(NetEvent::Forget(n.ssid.clone()));
                }
            });
        });
    }
    if net.adapter_changed() {
        warn(ui, t, "WiFi adapter changed");
    }
    if hand {
        by_hand(ui, t, 0.0, netfiles::WIFI_FILE, file.manageable, "wifi", net, event);
    } else {
        let adapter = !net.devices().is_empty();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(adapter, Button::new("Add network").variant(ButtonVariant::Secondary))
                .clicked()
            {
                *event = Some(NetEvent::OpenAdd);
            }
            if !adapter {
                muted(ui, t, "No WiFi adapter found");
            }
        });
    }
    if let Some(e) = &file.error {
        warn(ui, t, e);
    }
}

/// The hand line, and the take-over button when the menu can take the
/// file over without losing a setting.
#[allow(clippy::too_many_arguments)]
fn by_hand(
    ui: &mut Ui,
    t: &Theme,
    indent: f32,
    file: &str,
    manageable: bool,
    which: &'static str,
    net: &NetTab,
    event: &mut Option<NetEvent>,
) {
    let text = format!("This is set by hand in ~/{file}");
    note(ui, indent, RichText::new(text).color(t.palette().muted_foreground));
    if !manageable {
        return;
    }
    ui.horizontal(|ui| {
        ui.add_space(indent);
        if ui
            .add_enabled(
                !net.taking_over(),
                Button::new("Let the menu manage it").variant(ButtonVariant::Secondary),
            )
            .clicked()
        {
            *event = Some(NetEvent::TakeOver(which));
        }
    });
}

/// The add dialog: the networks in range, or a hidden network's name,
/// and the password.
fn add_dialog(ui: &Ui, view: &AddView) -> Option<NetEvent> {
    let mut event = None;
    let text = |ssid: String, password: String, hidden: bool| NetEvent::AddText { ssid, password, hidden };
    ui::dialog(ui, "Add network", |ui, t| {
        if view.scanning {
            muted(ui, t, "Looking for networks…");
        } else {
            egui::ScrollArea::vertical()
                .id_salt("in-range")
                .max_height(brand::NET_LIST_H)
                .show(ui, |ui| {
                    // A selectable name would take the click meant for its row.
                    ui.style_mut().interaction.selectable_labels = false;
                    for n in &view.in_range {
                        let line = if !n.supported {
                            RichText::new("WEP is not supported").color(t.role(role::WARN))
                        } else if n.secured {
                            RichText::new(format!("{LOCK} {}", nettab::signal_word(n.signal_dbm)))
                                .color(t.palette().muted_foreground)
                        } else {
                            RichText::new(nettab::signal_word(n.signal_dbm)).color(t.palette().muted_foreground)
                        };
                        let item = Item::new()
                            .variant(brand::CHOICE_ROW)
                            .size(ItemSize::Sm)
                            .title(n.ssid.as_str())
                            .description(line)
                            .selected(view.picked.is_some() && view.ssid == n.ssid)
                            .sense(Sense::click())
                            .id_salt(("in-range", &n.ssid));
                        if ui.add_enabled_ui(n.supported, |ui| ui.add(item)).inner.clicked() {
                            event = Some(NetEvent::PickNetwork(n.ssid.clone()));
                        }
                    }
                });
        }
        if let Some(e) = &view.scan_error {
            warn(ui, t, e);
        }
        if ui
            .add_enabled(
                !view.scanning,
                Button::new("Scan again").variant(ButtonVariant::Secondary),
            )
            .clicked()
        {
            event = Some(NetEvent::Rescan);
        }
        let mut hidden = view.hidden;
        let ticked = Field::new()
            .horizontal()
            .id_salt("tf-net-hidden")
            .show(ui, |ui| {
                let c = ui.add(Checkbox::new(&mut hidden)).changed();
                field_label(ui, "Hidden network");
                c
            })
            .inner;
        if ticked {
            event = Some(text(view.ssid.clone(), view.password.clone(), hidden));
        }
        // The name Add acts on is always on screen: a chosen row, or the
        // name field.
        if view.hidden || (view.picked.is_none() && !view.ssid.is_empty()) {
            let mut ssid = view.ssid.clone();
            let typed = Field::new()
                .label("Network name")
                .id_salt("tf-net-ssid-field")
                .show(ui, |ui| {
                    ui.add(Input::new(&mut ssid).max_width(f32::INFINITY).id_salt("tf-net-ssid"))
                        .changed()
                })
                .inner;
            if typed {
                event = Some(text(ssid, view.password.clone(), view.hidden));
            }
        }
        if !view.open_network() {
            let mut password = view.password.clone();
            let typed = Field::new()
                .label("Password")
                .id_salt("tf-net-password-field")
                .show(ui, |ui| {
                    ui.add(
                        Input::new(&mut password)
                            .password(true)
                            .max_width(f32::INFINITY)
                            .id_salt("tf-net-password"),
                    )
                    .changed()
                })
                .inner;
            if typed {
                event = Some(text(view.ssid.clone(), password, view.hidden));
            }
            muted(ui, t, PASSWORD_NOTE);
        }
        if let Some(e) = &view.error {
            warn(ui, t, e);
        }
        ui.add_space(BUTTON_GAP);
        ui.horizontal(|ui| {
            if ui.add_enabled(!view.ssid.is_empty(), Button::new("Add")).clicked() {
                event = Some(NetEvent::Add);
            }
            if ui
                .add(Button::new("Cancel").variant(ButtonVariant::Secondary))
                .clicked()
            {
                event = Some(NetEvent::CloseAdd);
            }
        });
    });
    event
}

/// Apply while ThinLinc or OxuLnk runs: the session may drop for a moment.
fn confirm_dialog(ui: &Ui) -> Option<NetEvent> {
    let mut event = None;
    ui::dialog(ui, "A remote session is running", |ui, t| {
        ui.label(RichText::new("The remote session may disconnect for a few seconds.").color(t.palette().foreground));
        ui.add_space(BUTTON_GAP);
        ui.horizontal(|ui| {
            if ui.add(Button::new("Apply")).clicked() {
                event = Some(NetEvent::ConfirmApply);
            }
            if ui
                .add(Button::new("Cancel").variant(ButtonVariant::Secondary))
                .clicked()
            {
                event = Some(NetEvent::CancelApply);
            }
        });
    });
    event
}
