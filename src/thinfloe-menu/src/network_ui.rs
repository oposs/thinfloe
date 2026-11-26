//! Drawing the Settings screen's Network tab: the connections now, the
//! wired address, the saved WiFi networks and Apply, the FloeDrop band,
//! the Time band, and
//! the two dialogs
//! over it (add a network, confirm an Apply during a remote session).
//! Everything that decides something lives in network.rs; this file only
//! paints it and reports clicks.

use crate::brand::{self, role};
use crate::infopanel;
use crate::netfiles::FileState;
use crate::nettab;
use crate::network::{AddView, NetEvent, NetTab};
use crate::publicip;
use crate::share_state::{self, ShareEvent};
use crate::timeband::TimeEvent;
use crate::timetab;
use crate::ui::{self, GAP, ROW_LEFT, ROW_MARGIN_X, ROW_RIGHT, TILE_GAP};
use egui::{Rect, RichText, Sense, Ui};
use shadcn_egui::Theme;
use shadcn_egui::components::badge::{Badge, BadgeVariant};
use shadcn_egui::components::button::{Button, ButtonVariant};
use shadcn_egui::components::checkbox::Checkbox;
use shadcn_egui::components::field::{Field, field_label};
use shadcn_egui::components::icon::IconView;
use shadcn_egui::components::input::Input;
use shadcn_egui::components::item::{Item, ItemSize};
use shadcn_egui::components::select::Select;
use shadcn_egui::components::switch::Switch;
use shadcn_egui::icons::lucide;

/// The wired select: index 1 is a fixed address.
const WIRED_MODES: [&str; 2] = ["Automatic (DHCP)", "Fixed address"];
const PASSWORD_NOTE: &str = "The password is saved with the home directory. Anyone who has the stick can read it.";
/// Above and below the content of a band, as a Packages row.
const BAND_PAD: i8 = 12;
/// Before the buttons of a dialog, as the other dialogs.
const BUTTON_GAP: f32 = 8.0;

/// A dialog of the tab is open; the screen under it takes no clicks.
pub fn dialog_open(net: &NetTab) -> bool {
    net.add_view().is_some() || net.confirm_open()
}

/// The Network tab, `width` wide, and its dialogs.
pub fn network_tab(ui: &mut Ui, net: &NetTab, width: f32) -> Option<NetEvent> {
    let t = Theme::current(ui.ctx());
    let pending = pending_url(ui.ctx());
    let mut url = None;
    // Apply and its error follow the bands, as the Upgrade screen's footer;
    // pinned to the foot of the panel only when the bands scroll.
    let rect = Rect::from_min_max(ui.cursor().min, ui.max_rect().max);
    let (apply, edit) = ui::page_with_footer(
        ui,
        "network",
        rect,
        GAP,
        |ui| {
            let label = if net.applying() { "Applying…" } else { "Apply" };
            let clicked = ui
                .add_enabled(net.dirty() && !net.applying(), Button::new(label))
                .clicked();
            if let Some(e) = net.error() {
                warn(ui, &t, e);
            }
            clicked.then_some(NetEvent::Apply)
        },
        |ui| {
            let mut event = None;
            let inner = ui.available_width().min(width) - ROW_MARGIN_X;
            let between = ui.spacing().item_spacing.y;
            ui.spacing_mut().item_spacing.y = TILE_GAP;
            // Nothing to edit before the files are read, nor while
            // an Apply or a take-over runs.
            if net.wifi_file().is_some() && net.wired_file().is_some() {
                ui.add_enabled_ui(!net.applying() && !net.taking_over(), |ui| {
                    band(ui, &t, inner, between, "Wired", |ui| wired(ui, &t, net, &mut event));
                    band(ui, &t, inner, between, "WiFi networks", |ui| {
                        wifi(ui, &t, net, &mut event)
                    });
                    band(ui, &t, inner, between, "DNS", |ui| dns(ui, &t, net, &mut event));
                });
            }
            // The FloeDrop, Time and public IP bands are saved at once and
            // unrelated to the network files: they take clicks before the
            // files are read and while an Apply runs.
            band(ui, &t, inner, between, "FloeDrop", |ui| {
                share(ui, &t, net, pending.as_ref(), &mut event, &mut url)
            });
            band(ui, &t, inner, between, "Time", |ui| {
                time(ui, &t, net, pending.as_ref(), &mut event, &mut url)
            });
            band(ui, &t, inner, between, "Public IP address", |ui| {
                public_ip(ui, &t, net, pending.as_ref(), &mut event, &mut url)
            });
            // The facts close the tab, as on the Screen tab.
            band(ui, &t, inner, between, "Connections now", |ui| connections(ui, &t, net));
            // The footer's gap belongs to the page.
            ui.add_space(GAP);
            event
        },
    );
    let mut event = edit.or(apply);
    if let Some(view) = net.add_view() {
        if let Some(e) = add_dialog(ui, view) {
            event = Some(e);
        }
    } else if net.confirm_open()
        && let Some(e) = confirm_dialog(ui)
    {
        event = Some(e);
    }
    // A frame reports one event. The lookup address field reports its
    // own on top of a click elsewhere when that click took its focus
    // (Apply, the switch); the click is reported now and the field's event
    // on the next frame, so neither is lost.
    let url = merge_url(pending, url);
    let ctx = ui.ctx();
    // A pass egui throws away (a page or its footer changed its height)
    // reports nothing, and the pass after it gets no input: the field's
    // event waits for that one.
    if ctx.will_discard() {
        if let Some(url) = url {
            keep_pending_url(ctx, url);
        }
        return event;
    }
    forget_pending_url(ctx);
    match (event, url) {
        (None, url) => url,
        (Some(e), None) => Some(e),
        (Some(e), Some(url)) => {
            keep_pending_url(ctx, url);
            Some(e)
        }
    }
}

fn pending_url_id() -> egui::Id {
    egui::Id::unique("tf-net-public-ip-pending")
}

/// The lookup address event a frame could not report, kept for the frame
/// after it only: one found later (the tab was left, say) is stale.
fn keep_pending_url(ctx: &egui::Context, e: NetEvent) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(pending_url_id(), (pass, Some(e))));
    ctx.request_repaint();
}

/// The kept event, left in place: it goes only once a pass that is not
/// thrown away has reported it (`forget_pending_url`).
fn pending_url(ctx: &egui::Context) -> Option<NetEvent> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<(u64, Option<NetEvent>)>(pending_url_id()))
        .filter(|(at, _)| at + 1 == pass)
        .and_then(|(_, e)| e)
}

fn forget_pending_url(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<(u64, Option<NetEvent>)>(pending_url_id()));
}

/// The lookup address event of this frame on top of one kept from the
/// last: the newer text, committed if either was. Escape wins in either
/// order: egui reports the focus loss on the frame after it once more,
/// and that commit of the old text must not undo the cancel.
fn merge_url(kept: Option<NetEvent>, now: Option<NetEvent>) -> Option<NetEvent> {
    match (kept, now) {
        (kept, None) => kept,
        (Some(NetEvent::PublicIpUrlCancel), _) => Some(NetEvent::PublicIpUrlCancel),
        (Some(NetEvent::Time(TimeEvent::NtpCancel)), _) => Some(NetEvent::Time(TimeEvent::NtpCancel)),
        (Some(NetEvent::Share(ShareEvent::NameCancel)), _) => Some(NetEvent::Share(ShareEvent::NameCancel)),
        (
            Some(NetEvent::Share(ShareEvent::NameText { commit: was, .. })),
            Some(NetEvent::Share(ShareEvent::NameText { text, commit })),
        ) => Some(NetEvent::Share(ShareEvent::NameText {
            text,
            commit: was || commit,
        })),
        (
            Some(NetEvent::Time(TimeEvent::NtpText { commit: was, .. })),
            Some(NetEvent::Time(TimeEvent::NtpText { text, commit })),
        ) => Some(NetEvent::Time(TimeEvent::NtpText {
            text,
            commit: was || commit,
        })),
        (Some(NetEvent::PublicIpUrl { commit: was, .. }), Some(NetEvent::PublicIpUrl { text, commit })) => {
            Some(NetEvent::PublicIpUrl {
                text,
                commit: was || commit,
            })
        }
        (_, now) => now,
    }
}

/// What the lookup address field reports: Escape cancels, Enter or a
/// focus loss commits the text, a change reports it.
fn url_field_event(changed: Option<String>, lost_focus: bool, escape: bool, shown: &str) -> Option<NetEvent> {
    if lost_focus && escape {
        return Some(NetEvent::PublicIpUrlCancel);
    }
    if changed.is_none() && !lost_focus {
        return None;
    }
    Some(NetEvent::PublicIpUrl {
        text: changed.unwrap_or_else(|| shown.to_string()),
        commit: lost_focus,
    })
}

/// `url_field_event` for the NTP servers field.
fn ntp_field_event(changed: Option<String>, lost_focus: bool, escape: bool, shown: &str) -> Option<NetEvent> {
    let e = match url_field_event(changed, lost_focus, escape, shown)? {
        NetEvent::PublicIpUrl { text, commit } => TimeEvent::NtpText { text, commit },
        _ => TimeEvent::NtpCancel,
    };
    Some(NetEvent::Time(e))
}

/// `url_field_event` for the FloeDrop name field.
fn name_field_event(changed: Option<String>, lost_focus: bool, escape: bool, shown: &str) -> Option<NetEvent> {
    let e = match url_field_event(changed, lost_focus, escape, shown)? {
        NetEvent::PublicIpUrl { text, commit } => ShareEvent::NameText { text, commit },
        _ => ShareEvent::NameCancel,
    };
    Some(NetEvent::Share(e))
}

/// The FloeDrop band: the switch, while it is on what the daemon says and
/// the folder, and the name phones show. Saved at once; the name field's
/// event goes to `field`, as the NTP servers field's does.
fn share(
    ui: &mut Ui,
    t: &Theme,
    net: &NetTab,
    pending: Option<&NetEvent>,
    event: &mut Option<NetEvent>,
    field: &mut Option<NetEvent>,
) {
    let band = net.share();
    let mut on = band.cfg().enabled;
    ui.horizontal(|ui| {
        if ui.add(Switch::new(&mut on)).changed() {
            *event = Some(NetEvent::Share(ShareEvent::Enable(on)));
        }
        ui.label(RichText::new(share_state::SWITCH_TEXT).color(t.palette().foreground));
    });
    if let Some(head) = band.head() {
        for (text, tone) in share_state::status_lines(&head)
            .into_iter()
            .chain(share_state::receiving_lines(band.receiving()))
        {
            ui.label(RichText::new(text).color(crate::upgrade_ui::tone_color(t, tone)));
        }
        muted(ui, t, format!("Folder: {}", share_state::FOLDER));
    }
    let shown = match pending {
        Some(NetEvent::Share(ShareEvent::NameText { text, .. })) => text.as_str(),
        _ => band.name_text(),
    };
    form_row(ui, t, share_state::NAME_LABEL, |ui| {
        let mut text = shown.to_string();
        let r = ui.add(
            Input::new(&mut text)
                .hint(band.placeholder())
                .max_width(brand::NET_FIELD_W)
                .id_salt("tf-net-share-name"),
        );
        let changed = r.changed().then_some(text);
        if let Some(e) = name_field_event(changed, r.lost_focus(), escape(ui), shown) {
            *field = Some(e);
        }
    });
    if let Some(e) = band.error() {
        warn(ui, t, e);
    }
}

/// The Time band: the zone selects, the NTP servers field and the clock's
/// sync. Saved at once; the field's event goes to `field`, as the lookup
/// address's does, and `pending` is its event kept from the last frame.
fn time(
    ui: &mut Ui,
    t: &Theme,
    net: &NetTab,
    pending: Option<&NetEvent>,
    event: &mut Option<NetEvent>,
    field: &mut Option<NetEvent>,
) {
    let band = net.time();
    let st = &net.details().time;
    let indent = brand::FORM_LABEL_W + ui.spacing().item_spacing.x;
    let line = |ui: &mut Ui, text: &str| note(ui, indent, RichText::new(text).color(t.palette().muted_foreground));
    ui.add_enabled_ui(!band.busy(), |ui| {
        form_row(ui, t, "Time zone", |ui| {
            let regions = band.regions();
            // Over an unknown zone, until a region is picked, no entry is
            // chosen and the placeholder reads Automatic, so picking
            // Automatic is a change.
            let mut r = if band.unknown_zone().is_some() && !band.region_picked() {
                regions.len()
            } else {
                band.region_index()
            };
            let select = Select::new("tf-net-tz-region", &mut r, &regions).placeholder(crate::timeband::AUTOMATIC);
            if ui.add(select).changed() {
                *event = Some(NetEvent::Time(TimeEvent::Region(r)));
            }
            let cities = band.cities();
            if !cities.is_empty() {
                let mut c = band.city_index().unwrap_or(cities.len());
                let select = Select::new("tf-net-tz-city", &mut c, &cities).placeholder("Choose a city");
                if ui.add(select).changed() {
                    *event = Some(NetEvent::Time(TimeEvent::City(c)));
                }
            }
        });
    });
    if let Some(z) = band.unknown_zone() {
        ui.horizontal(|ui| {
            ui.add_space(indent);
            warn(
                ui,
                t,
                format!("The saved time zone {z} is not known here; the automatic zone is used."),
            );
        });
    }
    if net.status_known() {
        line(ui, &timetab::zone_line(st, jiff::Timestamp::now()));
    }
    if let Some(n) = timetab::session_tz_note(std::env::var("TZ").ok().as_deref()) {
        line(ui, &n);
    }
    let shown = match pending {
        Some(NetEvent::Time(TimeEvent::NtpText { text, .. })) => text.as_str(),
        _ => band.ntp_text(),
    };
    ui.add_enabled_ui(!band.busy(), |ui| {
        form_row(ui, t, "NTP servers", |ui| {
            let mut text = shown.to_string();
            let r = ui.add(
                Input::new(&mut text)
                    .hint("ntp.example.ch, 10.0.0.1")
                    .max_width(brand::NET_FIELD_W)
                    .id_salt("tf-net-ntp"),
            );
            let changed = r.changed().then_some(text);
            if let Some(e) = ntp_field_event(changed, r.lost_focus(), escape(ui), shown) {
                *field = Some(e);
            }
        });
    });
    match crate::timecfg::parse_servers(shown) {
        Err(e) => line(ui, &e),
        Ok(servers) if servers.is_empty() => line(ui, "Empty: the servers DHCP sends and the Ubuntu pool."),
        Ok(_) => {}
    }
    if band.busy() {
        line(ui, "Applying…");
    }
    if let Some(e) = band.error() {
        ui.horizontal(|ui| {
            ui.add_space(indent);
            warn(ui, t, e);
        });
    }
    if net.status_known() {
        form_row(ui, t, "Clock", |ui| muted(ui, t, timetab::clock_line(st)));
        if let Some(s) = timetab::servers_line(st) {
            line(ui, &s);
        }
    }
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

/// The interfaces as labelled rows, the DNS servers in use, or why there
/// is no list.
fn connections(ui: &mut Ui, t: &Theme, net: &NetTab) {
    let lines = nettab::info_lines(net.ifaces(), net.details(), net.status_known(), net.status_error());
    let w = infopanel::label_width(ui, &lines);
    infopanel::draw(ui, t, &lines, w);
}

/// The wired address: the select and, for a fixed address, its fields.
/// Kept text shows them greyed.
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
        let rows: [(&str, &str, &'static str, &str, &mut String); 2] = [
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
            });
        }
    });
    let indent = brand::FORM_LABEL_W + ui.spacing().item_spacing.x;
    if !editable {
        by_hand(ui, t, indent, file.manageable, "wired", net, event);
    }
    if let Some(e) = &file.error {
        note(ui, indent, RichText::new(e).color(t.role(role::WARN)));
    }
}

/// The machine's DNS servers, for wired and WiFi in every wired mode. They
/// are kept with the wired settings, so kept wired text shows them greyed.
fn dns(ui: &mut Ui, t: &Theme, net: &NetTab, event: &mut Option<NetEvent>) {
    let Some(file) = net.wired_file() else {
        return;
    };
    if net.netboot() {
        muted(ui, t, "A netbooted machine always uses the DNS servers DHCP sends.");
        return;
    }
    let editable = file.state != FileState::Hand;
    ui.add_enabled_ui(editable, |ui| {
        if let Some(text) = text_row(ui, t, "DNS servers", &net.dns_text(), "1.1.1.1, 9.9.9.9", "tf-net-dns") {
            *event = Some(NetEvent::DnsText(text));
        }
    });
    let indent = brand::FORM_LABEL_W + ui.spacing().item_spacing.x;
    let line = |ui: &mut Ui, text: &str| note(ui, indent, RichText::new(text).color(t.palette().muted_foreground));
    line(ui, "Used for wired and WiFi. Empty: the servers DHCP sends.");
    if !editable {
        line(ui, "Kept from your old settings; the menu cannot change it.");
    }
    // Why Apply is on when nothing was typed.
    if net.wifi_dns_out_of_step() {
        line(ui, "WiFi uses other DNS servers. Apply sets the ones above.");
    }
    // The menu writes the servers into the WiFi settings too, never into
    // kept text.
    if net.wifi_file().is_some_and(|w| w.state == FileState::Hand) {
        line(ui, "WiFi keeps the DNS servers from your old settings.");
    }
}

/// The public IP switch and its lookup address. The address is saved when
/// the field is left or takes Enter, and Escape puts the saved one back;
/// one the menu does not use is named below it and not saved. The field's
/// event goes to `url`, apart from the clicks in `event`. `pending`: its
/// event from the last frame, not reported yet, whose text is shown.
fn public_ip(
    ui: &mut Ui,
    t: &Theme,
    net: &NetTab,
    pending: Option<&NetEvent>,
    event: &mut Option<NetEvent>,
    url: &mut Option<NetEvent>,
) {
    let mut on = net.public_ip().on;
    form_row(ui, t, "Show the public IP address", |ui| {
        if ui.add(Switch::new(&mut on)).changed() {
            *event = Some(NetEvent::PublicIp(on));
        }
    });
    let shown = match pending {
        Some(NetEvent::PublicIpUrl { text, .. }) => text.as_str(),
        _ => net.public_ip_url_text(),
    };
    form_row(ui, t, "Lookup address", |ui| {
        let mut text = shown.to_string();
        let r = ui.add(
            Input::new(&mut text)
                .hint(publicip::DEFAULT_URL)
                .max_width(brand::NET_FIELD_W)
                .id_salt("tf-net-public-ip-url"),
        );
        let changed = r.changed().then_some(text);
        // The Time band's field may have reported into the same slot.
        if let Some(e) = url_field_event(changed, r.lost_focus(), escape(ui), shown) {
            *url = Some(e);
        }
    });
    let refused = !shown.is_empty() && !publicip::url_ok(shown);
    if refused {
        let indent = brand::FORM_LABEL_W + ui.spacing().item_spacing.x;
        note(
            ui,
            indent,
            RichText::new("Only https:// addresses are used.").color(t.palette().muted_foreground),
        );
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
                ui.add(IconView::new(&lucide::LOCK).color(t.palette().muted_foreground));
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
        by_hand(ui, t, 0.0, file.manageable, "wifi", net, event);
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
fn by_hand(
    ui: &mut Ui,
    t: &Theme,
    indent: f32,
    manageable: bool,
    which: &'static str,
    net: &NetTab,
    event: &mut Option<NetEvent>,
) {
    note(
        ui,
        indent,
        RichText::new("Kept from your old settings; the menu cannot change it.").color(t.palette().muted_foreground),
    );
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

/// The strength mark of a network in range: more arcs, more signal.
fn signal_icon(dbm: i32) -> shadcn_egui::Icon {
    match nettab::signal_word(dbm) {
        "strong" => lucide::WIFI,
        "good" => lucide::WIFI_HIGH,
        _ => lucide::WIFI_LOW,
    }
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
                        let line = if let Some(why) = n.unsupported_reason() {
                            RichText::new(why).color(t.role(role::WARN))
                        } else {
                            RichText::new(nettab::signal_line(n.signal_dbm)).color(t.palette().muted_foreground)
                        };
                        let item = Item::new()
                            .variant(brand::CHOICE_ROW)
                            .size(ItemSize::Sm)
                            .icon(signal_icon(n.signal_dbm))
                            .title(n.ssid.as_str())
                            .description(line)
                            .selected(view.picked_row_shown() && view.ssid == n.ssid)
                            .sense(Sense::click())
                            .id_salt(("in-range", &n.ssid));
                        let lock = |ui: &mut Ui| {
                            if n.secured {
                                ui.add(IconView::new(&lucide::LOCK).color(t.palette().muted_foreground));
                            }
                        };
                        if ui
                            .add_enabled_ui(n.supported, |ui| item.show(ui, lock).response)
                            .inner
                            .clicked()
                        {
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
        if view.name_field_shown() {
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
    event.or_else(|| escape(ui).then_some(NetEvent::CloseAdd))
}

/// Escape was pressed this frame: a dialog closes as with Cancel.
fn escape(ui: &Ui) -> bool {
    ui.input(|i| i.key_pressed(egui::Key::Escape))
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
    event.or_else(|| escape(ui).then_some(NetEvent::CancelApply))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str, commit: bool) -> Option<NetEvent> {
        Some(NetEvent::PublicIpUrl {
            text: text.into(),
            commit,
        })
    }

    #[test]
    fn a_kept_lookup_address_event_merges_with_the_next() {
        let cancel = Some(NetEvent::PublicIpUrlCancel);
        assert_eq!(merge_url(None, None), None);
        assert_eq!(merge_url(url("a", true), None), url("a", true));
        assert_eq!(merge_url(None, url("a", false)), url("a", false));
        // The newer text, and a commit is never dropped.
        assert_eq!(merge_url(url("a", true), url("ab", false)), url("ab", true));
        // Escape wins in either order.
        assert_eq!(merge_url(cancel.clone(), url("a", true)), cancel);
        assert_eq!(merge_url(url("a", true), cancel.clone()), cancel);
    }

    #[test]
    fn the_lookup_address_field_reports_change_commit_and_cancel() {
        assert_eq!(url_field_event(None, false, false, "a"), None);
        assert_eq!(url_field_event(Some("ab".into()), false, false, "a"), url("ab", false));
        assert_eq!(url_field_event(Some("ab".into()), true, false, "a"), url("ab", true));
        assert_eq!(url_field_event(None, true, false, "a"), url("a", true));
        assert_eq!(
            url_field_event(None, true, true, "a"),
            Some(NetEvent::PublicIpUrlCancel)
        );
        // Escape without leaving the field is not a cancel.
        assert_eq!(url_field_event(None, false, true, "a"), None);
    }
}
