//! The Sound tab: the volume switch, the volume slider and the mute switch.

use std::time::Instant;

use crate::keyboard::KbEvent;
use crate::keyboardtab_ui::{control_note, row};
use crate::settings::SettingsEvent;
use crate::soundtab::{self, SoundEvent, SoundTab};
use crate::ui::{ROW_LEFT, ROW_MARGIN_X, ROW_RIGHT};
use egui::{RichText, Ui};
use shadcn_egui::Theme;
use shadcn_egui::components::slider::Slider;
use shadcn_egui::components::switch::Switch;

/// Padding above and below the card.
const CARD_PAD: i8 = 12;

/// The tab's body; `switch_on` is the volume keys switch as the Keyboard
/// tab shows it, `kb_note` why its last apply failed, if it did.
pub fn sound_tab(
    ui: &mut Ui,
    tab: &SoundTab,
    switch_on: bool,
    kb_note: Option<&str>,
    width: f32,
    now: Instant,
) -> Option<SettingsEvent> {
    let t = Theme::current(ui.ctx());
    let mut event = None;
    let shown = tab.shown(now);
    let (enabled, why) = soundtab::controls(switch_on, shown);
    egui::ScrollArea::vertical()
        .id_salt("sound")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let inner_w = ui.available_width().min(width) - ROW_MARGIN_X;
            egui::Frame::new()
                .fill(t.palette().card)
                .inner_margin(egui::Margin {
                    left: ROW_LEFT,
                    right: ROW_RIGHT,
                    top: CARD_PAD,
                    bottom: CARD_PAD,
                })
                .show(ui, |ui| {
                    ui.set_width(inner_w);
                    let mut on = switch_on;
                    row(ui, &t, "Set the volume on this machine", |ui| {
                        if ui.add(Switch::new(&mut on)).changed() {
                            event = Some(SettingsEvent::Keyboard(KbEvent::VolumeKeys(on)));
                        }
                    });
                    control_note(ui, &t, soundtab::SWITCH_NOTE);
                    // The switch goes through the keyboard apply: its
                    // failure shows here too.
                    if let Some(note) = kb_note {
                        control_note(ui, &t, note);
                    }
                    let v = shown.unwrap_or(soundtab::Volume {
                        percent: 0,
                        muted: false,
                        on: false,
                    });
                    ui.add_enabled_ui(enabled, |ui| {
                        let mut level = f32::from(v.percent);
                        row(ui, &t, "Volume", |ui| {
                            ui.scope(|ui| {
                                ui.set_max_width(crate::brand::NET_FIELD_W);
                                let r = ui.add(Slider::new(&mut level, 0.0..=100.0).step(1.0).label("Volume"));
                                let done = r.drag_stopped() || r.clicked();
                                if r.changed() || done {
                                    event = Some(SettingsEvent::Sound(SoundEvent::Level {
                                        percent: level.round().clamp(0.0, 100.0) as u8,
                                        done,
                                    }));
                                }
                            });
                            ui.label(RichText::new(format!("{} %", level.round() as u8)).color(t.palette().foreground));
                        });
                        let mut muted = v.muted;
                        row(ui, &t, "Mute", |ui| {
                            if ui.add(Switch::new(&mut muted)).changed() {
                                event = Some(SettingsEvent::Sound(SoundEvent::Mute(muted)));
                            }
                        });
                    });
                    if let Some(why) = why {
                        control_note(ui, &t, why);
                    }
                });
        });
    event
}
