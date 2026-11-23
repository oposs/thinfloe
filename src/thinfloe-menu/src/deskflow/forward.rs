//! The captured state of the Deskflow server: while a device has the
//! keyboard and the pointer, their events become its messages, and every
//! way back first releases on the device what was sent down (spec, Edge
//! capture and the way back, Keyboard). Pure: events and the time go in,
//! messages come out; the daemon (`run.rs`) sends them and ends the capture.

use std::time::{Duration, Instant};

use super::capture::CapEvent;
use super::edge::{Track, Tracker};
use super::keymap::{self, Xkb};
use super::proto::Msg;

/// Escape's keysym: with Ctrl, Alt and Shift down it releases.
const XK_ESCAPE: u32 = 0xff1b;
/// The microphone mute key stays on ThinFloe: Deskflow has no key id for
/// it, and the device's microphone is not the one at the desk.
const XF86_AUDIO_MIC_MUTE: u32 = 0x1008_ffb2;

// The escape chord's keys, linux/input-event-codes.h: what decides it
// when the keymap could not be read.
const KEY_ESC: u32 = 1;
const KEY_LEFTCTRL: u32 = 29;
const KEY_RIGHTCTRL: u32 = 97;
const KEY_LEFTALT: u32 = 56;
const KEY_RIGHTALT: u32 = 100;
const KEY_LEFTSHIFT: u32 = 42;
const KEY_RIGHTSHIFT: u32 = 54;

// Pointer buttons, linux/input-event-codes.h.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const BTN_SIDE: u32 = 0x113;
const BTN_EXTRA: u32 = 0x114;

/// Deskflow's id of a pointer button, from `src/lib/deskflow/MouseTypes.h`
/// at commit 80221f0: left 1, middle 2, right 3, the two extra buttons 4
/// and 5. (The comment in the protocol header swaps 2 and 3; the header
/// with the constants wins.) Other buttons have no id.
pub fn button_id(evdev: u32) -> Option<i8> {
    Some(match evdev {
        BTN_LEFT => 1,
        BTN_MIDDLE => 2,
        BTN_RIGHT => 3,
        BTN_SIDE => 4,
        BTN_EXTRA => 5,
        _ => return None,
    })
}

/// What the daemon does with the result of an event.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// A message for the captured device.
    Send(Msg),
    /// End the capture ([`release`] first).
    Release(&'static str),
    /// Pointer crossed back from the device; `along` is 0..1 along the edge.
    ReleaseBack { along: f64 },
    /// The microphone mute key was released: ThinFloe's microphone.
    LocalMicMute,
}

/// The keyboard as the compositor describes it: its keymap and modifier
/// state, and the key repeat it asks for.
#[derive(Default)]
pub struct Keyboard {
    xkb: Option<Xkb>,
    /// Repeats per second (0: none) and the delay before the first, in ms.
    rate: i32,
    delay: i32,
    /// The evdev codes of the keys down, as the key events tell.
    down: Vec<u32>,
}

impl Keyboard {
    /// Takes a keymap, modifier or repeat event; true when `ev` was one.
    pub fn update(&mut self, ev: &CapEvent) -> bool {
        match ev {
            CapEvent::Key { code, pressed } => {
                self.down.retain(|c| c != code);
                if *pressed {
                    self.down.push(*code);
                }
                return false;
            }
            CapEvent::Unlocked | CapEvent::KeyboardLeft => {
                self.down.clear();
                return false;
            }
            CapEvent::Keymap { fd, size } => {
                self.xkb = match Xkb::from_keymap_fd(fd, *size) {
                    Ok(x) => Some(x),
                    Err(e) => {
                        eprintln!("thinfloe-deskflow: keymap: {e}");
                        None
                    }
                };
            }
            CapEvent::Modifiers {
                depressed,
                latched,
                locked,
                group,
            } => {
                if let Some(x) = self.xkb.as_mut() {
                    x.update_mask(*depressed, *latched, *locked, *group);
                }
            }
            CapEvent::RepeatInfo { rate, delay } => (self.rate, self.delay) = (*rate, *delay),
            _ => return false,
        }
        true
    }

    /// The `KeyModifierMask` of the modifiers down now.
    pub fn mask(&self) -> u16 {
        self.xkb.as_ref().map_or(0, |x| x.mods().mask())
    }

    /// Only the lock-key modifier bits (`Caps`, `Num`, `Scroll`).
    fn locks_mask(&self) -> u16 {
        self.xkb.as_ref().map_or(0, |x| x.mods().locks_mask())
    }

    /// Whether a local compose sequence is currently in progress.
    pub fn is_composing(&self) -> bool {
        self.xkb.as_ref().is_some_and(|x| x.is_composing())
    }

    /// Resets any in-progress local compose sequence.
    pub fn reset_compose(&mut self) {
        if let Some(x) = self.xkb.as_mut() {
            x.compose_reset();
        }
    }

    /// The key id of a key in the current state; when its keysym has none
    /// (`XF86Switch_VT_1` under Ctrl+Alt), the id of its first-level keysym,
    /// as Deskflow does, so Ctrl+Alt+F1 reaches the device as F1.
    fn key_id(&self, code: u32) -> Option<u16> {
        let x = self.xkb.as_ref()?;
        keymap::key_id(x.keysym(code), x.utf32(code)).or_else(|| keymap::key_id(x.keysym_level0(code), 0))
    }

    /// Escape while Ctrl, Alt and Shift are down. Without a keymap the
    /// evdev codes decide, so the way back never depends on it.
    fn escape_chord(&self, code: u32) -> bool {
        let Some(x) = self.xkb.as_ref() else {
            let down = |a: u32, b: u32| self.down.iter().any(|c| *c == a || *c == b);
            return code == KEY_ESC
                && down(KEY_LEFTCTRL, KEY_RIGHTCTRL)
                && down(KEY_LEFTALT, KEY_RIGHTALT)
                && down(KEY_LEFTSHIFT, KEY_RIGHTSHIFT);
        };
        let m = x.mods();
        m.ctrl && m.alt && m.shift && (x.keysym(code) == XK_ESCAPE || x.keysym_level0(code) == XK_ESCAPE)
    }

    /// The key is the microphone mute key, at any level.
    fn mic_mute(&self, code: u32) -> bool {
        self.xkb
            .as_ref()
            .is_some_and(|x| x.keysym(code) == XF86_AUDIO_MIC_MUTE || x.keysym_level0(code) == XF86_AUDIO_MIC_MUTE)
    }

    /// The repeat period and delay for a key, when it repeats.
    fn repeat_of(&self, code: u32) -> Option<(Duration, Duration)> {
        let x = self.xkb.as_ref()?;
        if self.rate <= 0 || !x.repeats(code) {
            return None;
        }
        let every = Duration::from_secs(1) / self.rate.unsigned_abs();
        Some((
            every,
            Duration::from_millis(u64::from(self.delay.max(0).unsigned_abs())),
        ))
    }
}

/// The key that repeats: the last one pressed, while it is down.
#[derive(Debug)]
struct Repeat {
    id: u16,
    button: u16,
    next: Instant,
    every: Duration,
}

/// A device that has the keyboard and the pointer.
#[derive(Debug)]
pub struct Active {
    /// The connection's id.
    pub device: usize,
    pub tracker: Tracker,
    /// (key id, button) of each key sent down and not up yet.
    pub held: Vec<(u16, u16)>,
    /// The pointer buttons sent down.
    buttons: Vec<i8>,
    /// The position last sent.
    pos: (i16, i16),
    repeat: Option<Repeat>,
    /// The microphone mute key went down during the capture.
    mic_down: bool,
    /// Whether `Multi_key` compose sequences are handled locally.
    pub compose: bool,
}

impl Active {
    /// The pointer entered the device at `at` (the CINN position).
    pub fn new(device: usize, tracker: Tracker, at: (i16, i16), compose: bool) -> Active {
        Active {
            device,
            tracker,
            held: Vec::new(),
            buttons: Vec::new(),
            pos: at,
            repeat: None,
            mic_down: false,
            compose,
        }
    }

    /// When [`tick`] has the next repeat to send.
    pub fn deadline(&self) -> Option<Instant> {
        self.repeat.as_ref().map(|r| r.next)
    }
}

/// One event while captured. Keymap, modifier and repeat events update
/// `kb`; a key or button released that was never sent down is dropped.
pub fn on_cap_event(a: &mut Active, ev: &CapEvent, kb: &mut Keyboard, now: Instant) -> Vec<Action> {
    if kb.update(ev) {
        return Vec::new();
    }
    match *ev {
        CapEvent::Relative { dx, dy, .. } => {
            if !dx.is_finite() || !dy.is_finite() {
                return Vec::new();
            }
            match a.tracker.motion(dx, dy) {
                Track::Back { along } => vec![Action::ReleaseBack { along }],
                Track::Move(x, y) if (x, y) != a.pos => {
                    a.pos = (x, y);
                    vec![Action::Send(Msg::MouseMove { x, y })]
                }
                Track::Move(..) => Vec::new(),
            }
        }
        CapEvent::Button { button, pressed } => {
            let Some(id) = button_id(button) else {
                return Vec::new();
            };
            let down = a.buttons.iter().position(|b| *b == id);
            match (pressed, down) {
                (true, None) => {
                    a.buttons.push(id);
                    vec![Action::Send(Msg::MouseDown(id))]
                }
                (false, Some(i)) => {
                    a.buttons.remove(i);
                    vec![Action::Send(Msg::MouseUp(id))]
                }
                _ => Vec::new(),
            }
        }
        CapEvent::Axis { v120_x, v120_y } => {
            // Deskflow: positive is right and up; Wayland's vertical axis
            // is positive down.
            let clamp = |v: i32| v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
            vec![Action::Send(Msg::Wheel {
                dx: clamp(v120_x),
                dy: clamp(v120_y.saturating_neg()),
            })]
        }
        CapEvent::Key { code, pressed: true } => key_down(a, kb, code, now),
        CapEvent::Key { code, pressed: false } => key_up(a, kb, code),
        CapEvent::Unlocked => vec![Action::Release("unlocked")],
        CapEvent::KeyboardLeft => vec![Action::Release("keyboard left")],
        _ => Vec::new(),
    }
}

fn key_down(a: &mut Active, kb: &mut Keyboard, code: u32, now: Instant) -> Vec<Action> {
    if kb.escape_chord(code) {
        return vec![Action::Release("escape chord")];
    }
    if kb.mic_mute(code) {
        a.mic_down = true;
        return Vec::new();
    }
    if a.compose
        && let Some(x) = kb.xkb.as_mut()
    {
        let sym = x.keysym(code);
        // Lock keys always reach the client, mid-sequence too, so its Caps
        // Lock and Num Lock follow ThinFloe's; the compose state never sees
        // them. Other modifiers pressed mid-sequence stay held back, press
        // and release (`key_up` sends only what was sent down), and the
        // composed key goes with the lock bits alone.
        if (x.is_composing() || sym == keymap::XK_MULTI_KEY) && !keymap::is_lock_key(sym) {
            // The repeat stops only for a key the sequence takes; a key
            // that goes on to the client leaves it as with the switch off.
            if sym == 0 {
                a.repeat = None;
                return Vec::new();
            }
            match x.compose_feed(sym) {
                // No table, or Multi_key starts no sequence in it: the key
                // goes to the client as with the switch off.
                keymap::ComposeOutcome::Idle => {}
                keymap::ComposeOutcome::Composing
                | keymap::ComposeOutcome::Cancelled
                | keymap::ComposeOutcome::Composed(None) => {
                    a.repeat = None;
                    return Vec::new();
                }
                keymap::ComposeOutcome::Composed(Some(id)) => {
                    a.repeat = None;
                    let mods = kb.locks_mask();
                    return vec![
                        Action::Send(Msg::KeyDown {
                            id,
                            mods,
                            button: keymap::COMPOSED_BUTTON,
                        }),
                        Action::Send(Msg::KeyUp {
                            id,
                            mods,
                            button: keymap::COMPOSED_BUTTON,
                        }),
                    ];
                }
            };
        }
    }
    let button = keymap::button(code);
    let Some(id) = kb.key_id(code) else {
        return Vec::new();
    };
    if a.held.iter().any(|h| h.1 == button) {
        return Vec::new();
    }
    a.held.push((id, button));
    // A key that does not repeat (a modifier) leaves the repeat of the
    // key held, as the X server does.
    if let Some((every, delay)) = kb.repeat_of(code) {
        a.repeat = Some(Repeat {
            id,
            button,
            next: now + delay,
            every,
        });
    }
    vec![Action::Send(Msg::KeyDown {
        id,
        mods: kb.mask(),
        button,
    })]
}

fn key_up(a: &mut Active, kb: &Keyboard, code: u32) -> Vec<Action> {
    // On the release, as labwc's binding (onRelease); a press before the
    // capture was ThinFloe's.
    if kb.mic_mute(code) {
        return if std::mem::take(&mut a.mic_down) {
            vec![Action::LocalMicMute]
        } else {
            Vec::new()
        };
    }
    let button = keymap::button(code);
    let Some(i) = a.held.iter().position(|h| h.1 == button) else {
        return Vec::new();
    };
    let (id, _) = a.held.remove(i);
    if a.repeat.as_ref().is_some_and(|r| r.button == button) {
        a.repeat = None;
    }
    vec![Action::Send(Msg::KeyUp {
        id,
        mods: kb.mask(),
        button,
    })]
}

/// Time passes: the repeats due for the key held, as one DKRP with their
/// count, as Deskflow sends them.
pub fn tick(a: &mut Active, kb: &Keyboard, now: Instant) -> Vec<Action> {
    let Some(r) = a.repeat.as_mut() else {
        return Vec::new();
    };
    if now < r.next {
        return Vec::new();
    }
    let late = now.duration_since(r.next).as_nanos() / r.every.as_nanos().max(1);
    let count = u16::try_from(late + 1).unwrap_or(u16::MAX);
    r.next += r.every * u32::from(count);
    vec![Action::Send(Msg::KeyRepeat {
        id: r.id,
        mods: kb.mask(),
        count,
        button: r.button,
    })]
}

/// The messages that end a capture on the device: DKUP for every key sent
/// down, DMUP for every button, then COUT.
pub fn release(a: &mut Active, kb: &mut Keyboard) -> Vec<Msg> {
    let mods = kb.mask();
    kb.reset_compose();
    let mut out: Vec<Msg> = a
        .held
        .drain(..)
        .rev()
        .map(|(id, button)| Msg::KeyUp { id, mods, button })
        .collect();
    out.extend(a.buttons.drain(..).rev().map(Msg::MouseUp));
    a.repeat = None;
    out.push(Msg::Cout);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kanshi::Side;

    const KEY_A: u32 = 30;
    const KEY_F1: u32 = 59;
    // Real modifier bits of an XKB keymap.
    const SHIFT: u32 = 1;
    const CONTROL: u32 = 4;
    const MOD1: u32 = 8;

    fn us() -> Keyboard {
        Keyboard {
            xkb: Some(
                Xkb::from_layout("us").expect("libxkbcommon and the xkeyboard-config data are needed for this test"),
            ),
            rate: 25,
            delay: 600,
            down: Vec::new(),
        }
    }

    fn us_compose() -> Keyboard {
        Keyboard {
            xkb: Some(
                Xkb::from_layout_options("us", Some("compose:ralt"))
                    .expect("libxkbcommon and the xkeyboard-config data are needed for this test"),
            ),
            rate: 25,
            delay: 600,
            down: Vec::new(),
        }
    }

    fn de_compose() -> Keyboard {
        Keyboard {
            xkb: Some(
                Xkb::from_layout("de").expect("libxkbcommon and the xkeyboard-config data are needed for this test"),
            ),
            rate: 25,
            delay: 600,
            down: Vec::new(),
        }
    }

    fn active() -> Active {
        active_compose(false)
    }

    fn active_compose(compose: bool) -> Active {
        Active::new(
            7,
            Tracker::new(Side::RightOf, (0, 540), (1920, 1080)),
            (0, 540),
            compose,
        )
    }

    fn key(code: u32, pressed: bool) -> CapEvent {
        CapEvent::Key { code, pressed }
    }

    fn mods(depressed: u32) -> CapEvent {
        CapEvent::Modifiers {
            depressed,
            latched: 0,
            locked: 0,
            group: 0,
        }
    }

    /// Feeds the events; a Release ends the run with the release messages.
    fn run(a: &mut Active, kb: &mut Keyboard, evs: &[CapEvent]) -> (Vec<Msg>, Option<&'static str>) {
        let now = Instant::now();
        let mut out = Vec::new();
        for ev in evs {
            for act in on_cap_event(a, ev, kb, now) {
                match act {
                    Action::Send(m) => out.push(m),
                    Action::Release(why) => {
                        out.extend(release(a, kb));
                        return (out, Some(why));
                    }
                    Action::ReleaseBack { .. } => {
                        out.extend(release(a, kb));
                        return (out, Some("back"));
                    }
                    Action::LocalMicMute => {}
                }
            }
        }
        (out, None)
    }

    fn down(id: u16, mods: u16, code: u32) -> Msg {
        Msg::KeyDown {
            id,
            mods,
            button: keymap::button(code),
        }
    }

    fn up(id: u16, mods: u16, code: u32) -> Msg {
        Msg::KeyUp {
            id,
            mods,
            button: keymap::button(code),
        }
    }

    #[test]
    fn mic_mute_release_is_local_and_not_sent() {
        // linux/input-event-codes.h
        const KEY_VOLUMEUP: u32 = 115;
        const KEY_MICMUTE: u32 = 248;
        let (mut a, mut kb) = (active(), us());
        let now = Instant::now();
        assert_eq!(on_cap_event(&mut a, &key(KEY_MICMUTE, true), &mut kb, now), Vec::new());
        assert_eq!(
            on_cap_event(&mut a, &key(KEY_MICMUTE, false), &mut kb, now),
            vec![Action::LocalMicMute]
        );
        // A release whose press the capture did not see does nothing.
        assert_eq!(on_cap_event(&mut a, &key(KEY_MICMUTE, false), &mut kb, now), Vec::new());
        // The volume keys go to the device: kKeyAudioUp.
        let (out, why) = run(&mut a, &mut kb, &[key(KEY_VOLUMEUP, true), key(KEY_VOLUMEUP, false)]);
        assert_eq!(why, None);
        assert_eq!(out, vec![down(0xE0AF, 0, KEY_VOLUMEUP), up(0xE0AF, 0, KEY_VOLUMEUP)]);
        assert_eq!(release(&mut a, &mut kb), vec![Msg::Cout]);
    }

    #[test]
    fn release_sends_up_for_held_keys() {
        let (mut a, mut kb) = (active(), us());
        let evs = [
            key(KEY_A, true),
            key(KEY_LEFTCTRL, true),
            mods(CONTROL),
            key(KEY_LEFTALT, true),
            mods(CONTROL | MOD1),
            key(KEY_LEFTSHIFT, true),
            mods(CONTROL | MOD1 | SHIFT),
            key(KEY_ESC, true),
        ];
        let (out, why) = run(&mut a, &mut kb, &evs);
        assert_eq!(why, Some("escape chord"));
        // Ctrl, Alt and Shift: kKeyControl_L, kKeyAlt_L, kKeyShift_L.
        let all = 0x0001 | 0x0002 | 0x0004;
        assert_eq!(
            out,
            [
                down(0x61, 0, KEY_A),
                down(0xEFE3, 0, KEY_LEFTCTRL),
                down(0xEFE9, 0x0002, KEY_LEFTALT),
                down(0xEFE1, 0x0006, KEY_LEFTSHIFT),
                up(0xEFE1, all, KEY_LEFTSHIFT),
                up(0xEFE9, all, KEY_LEFTALT),
                up(0xEFE3, all, KEY_LEFTCTRL),
                up(0x61, all, KEY_A),
                Msg::Cout,
            ]
        );
        assert!(
            !out.iter()
                .any(|m| matches!(m, Msg::KeyDown { id: 0xEF1B, .. } | Msg::KeyUp { id: 0xEF1B, .. }))
        );
        assert!(a.held.is_empty());
    }

    #[test]
    fn a_key_never_sent_down_is_never_sent_up() {
        let (mut a, mut kb) = (active(), us());
        // Pressed before the lock (held back by the capture): only its
        // release arrives.
        let (out, _) = run(&mut a, &mut kb, &[key(KEY_A, false)]);
        assert_eq!(out, []);
        // A key that has no id goes neither down nor up.
        const KEY_MICMUTE: u32 = 248;
        let (out, _) = run(&mut a, &mut kb, &[key(KEY_MICMUTE, true), key(KEY_MICMUTE, false)]);
        assert_eq!(out, []);
        let (out, _) = run(
            &mut a,
            &mut kb,
            &[key(KEY_A, true), key(KEY_A, false), key(KEY_A, false)],
        );
        assert_eq!(out, [down(0x61, 0, KEY_A), up(0x61, 0, KEY_A)]);
        assert_eq!(release(&mut a, &mut kb), [Msg::Cout]);
    }

    #[test]
    fn the_key_up_carries_the_id_of_its_key_down() {
        let (mut a, mut kb) = (active(), us());
        let evs = [
            key(KEY_LEFTSHIFT, true),
            mods(SHIFT),
            key(KEY_A, true),
            key(KEY_LEFTSHIFT, false),
            mods(0),
            key(KEY_A, false),
        ];
        let (out, _) = run(&mut a, &mut kb, &evs);
        assert_eq!(
            out,
            [
                down(0xEFE1, 0, KEY_LEFTSHIFT),
                down(0x41, 0x0001, KEY_A),
                up(0xEFE1, 0x0001, KEY_LEFTSHIFT),
                up(0x41, 0, KEY_A),
            ]
        );
    }

    #[test]
    fn ctrl_alt_f1_reaches_the_device_as_f1() {
        let (mut a, mut kb) = (active(), us());
        let (out, _) = run(&mut a, &mut kb, &[mods(CONTROL | MOD1), key(KEY_F1, true)]);
        assert_eq!(out, [down(0xEFBE, 0x0006, KEY_F1)]);
    }

    #[test]
    fn escape_alone_or_with_two_modifiers_is_forwarded() {
        let (mut a, mut kb) = (active(), us());
        let (out, why) = run(
            &mut a,
            &mut kb,
            &[key(KEY_ESC, true), mods(CONTROL | MOD1), key(KEY_ESC, true)],
        );
        assert_eq!(why, None);
        assert_eq!(out, [down(0xEF1B, 0, KEY_ESC)]);
    }

    #[test]
    fn buttons_have_deskflows_ids_and_are_released() {
        let (mut a, mut kb) = (active(), us());
        let b = |button, pressed| CapEvent::Button { button, pressed };
        let evs = [
            b(BTN_LEFT, true),
            b(BTN_LEFT, false),
            b(BTN_MIDDLE, true),
            b(BTN_RIGHT, true),
            b(BTN_RIGHT, false),
            b(BTN_SIDE, true),
            b(BTN_SIDE, false),
            b(BTN_EXTRA, true),
            b(0x115, true),
            b(BTN_LEFT, false),
        ];
        let (out, _) = run(&mut a, &mut kb, &evs);
        use Msg::{MouseDown as Dn, MouseUp as Up};
        assert_eq!(out, [Dn(1), Up(1), Dn(2), Dn(3), Up(3), Dn(4), Up(4), Dn(5)]);
        assert_eq!(release(&mut a, &mut kb), [Up(5), Up(2), Msg::Cout]);
    }

    #[test]
    fn the_wheel_is_positive_up_and_right() {
        let (mut a, mut kb) = (active(), us());
        let (out, _) = run(
            &mut a,
            &mut kb,
            &[
                CapEvent::Axis { v120_x: 0, v120_y: 120 },
                CapEvent::Axis {
                    v120_x: -240,
                    v120_y: -60,
                },
                CapEvent::Axis {
                    v120_x: i32::MAX,
                    v120_y: i32::MIN,
                },
            ],
        );
        assert_eq!(
            out,
            [
                Msg::Wheel { dx: 0, dy: -120 },
                Msg::Wheel { dx: -240, dy: 60 },
                Msg::Wheel {
                    dx: i16::MAX,
                    dy: i16::MAX
                },
            ]
        );
    }

    fn rel(dx: f64, dy: f64) -> CapEvent {
        CapEvent::Relative {
            dx,
            dy,
            dx_unaccel: dx / 2.0,
            dy_unaccel: dy / 2.0,
        }
    }

    #[test]
    fn motion_moves_the_device_pointer_until_it_crosses_back() {
        let (mut a, mut kb) = (active(), us());
        let evs = [
            rel(10.0, 5.0),
            rel(0.0, 0.0),
            rel(f64::NAN, 1.0),
            rel(1.0, f64::INFINITY),
            rel(-5.0, 0.0),
            rel(-6.0, 0.0),
            rel(10.0, 0.0),
        ];
        let (out, why) = run(&mut a, &mut kb, &evs);
        assert_eq!(why, Some("back"));
        assert_eq!(
            out,
            [
                Msg::MouseMove { x: 10, y: 545 },
                Msg::MouseMove { x: 5, y: 545 },
                Msg::Cout
            ]
        );
    }

    #[test]
    fn the_lock_going_or_the_keyboard_leaving_releases() {
        for (ev, why) in [
            (CapEvent::Unlocked, "unlocked"),
            (CapEvent::KeyboardLeft, "keyboard left"),
        ] {
            let (mut a, mut kb) = (active(), us());
            let (out, w) = run(&mut a, &mut kb, &[key(KEY_A, true), ev]);
            assert_eq!(w, Some(why));
            assert_eq!(out, [down(0x61, 0, KEY_A), up(0x61, 0, KEY_A), Msg::Cout]);
        }
    }

    #[test]
    fn a_held_key_repeats_at_the_rate_the_compositor_gives() {
        let (mut a, mut kb) = (active(), us());
        assert!(kb.update(&CapEvent::RepeatInfo { rate: 20, delay: 500 }));
        let t = Instant::now();
        on_cap_event(&mut a, &key(KEY_A, true), &mut kb, t);
        let ms = Duration::from_millis;
        assert_eq!(a.deadline(), Some(t + ms(500)));
        assert_eq!(tick(&mut a, &kb, t + ms(499)), []);
        let rp = |count| {
            vec![Action::Send(Msg::KeyRepeat {
                id: 0x61,
                mods: 0,
                count,
                button: keymap::button(KEY_A),
            })]
        };
        assert_eq!(tick(&mut a, &kb, t + ms(500)), rp(1));
        assert_eq!(tick(&mut a, &kb, t + ms(520)), []);
        // A late loop sends the repeats it missed as one count.
        assert_eq!(tick(&mut a, &kb, t + ms(710)), rp(4));
        assert_eq!(a.deadline(), Some(t + ms(750)));
        on_cap_event(&mut a, &key(KEY_A, false), &mut kb, t + ms(720));
        assert_eq!(a.deadline(), None);
        // Modifiers do not repeat, nor anything at rate 0.
        on_cap_event(&mut a, &key(KEY_LEFTSHIFT, true), &mut kb, t);
        assert_eq!(a.deadline(), None);
        kb.update(&CapEvent::RepeatInfo { rate: 0, delay: 500 });
        on_cap_event(&mut a, &key(KEY_A, true), &mut kb, t);
        assert_eq!(a.deadline(), None);
    }

    #[test]
    fn without_a_keymap_the_escape_chord_still_releases() {
        for (c, al, sh) in [
            (KEY_LEFTCTRL, KEY_LEFTALT, KEY_LEFTSHIFT),
            (KEY_RIGHTCTRL, KEY_RIGHTALT, KEY_RIGHTSHIFT),
        ] {
            let mut a = active();
            let mut kb = Keyboard::default();
            let evs = [key(c, true), key(al, true), key(sh, true), key(KEY_ESC, true)];
            let (out, why) = run(&mut a, &mut kb, &evs);
            assert_eq!(why, Some("escape chord"));
            assert_eq!(out, [Msg::Cout]);
        }
        // Two of the three, or a modifier let go, is no chord.
        let mut a = active();
        let mut kb = Keyboard::default();
        let evs = [
            key(KEY_LEFTCTRL, true),
            key(KEY_LEFTALT, true),
            key(KEY_ESC, true),
            key(KEY_LEFTSHIFT, true),
            key(KEY_LEFTSHIFT, false),
            key(KEY_ESC, true),
        ];
        assert_eq!(run(&mut a, &mut kb, &evs).1, None);
    }

    #[test]
    fn a_modifier_pressed_does_not_stop_the_repeat_of_a_held_key() {
        const KEY_RIGHT: u32 = 106;
        let (mut a, mut kb) = (active(), us());
        let t = Instant::now();
        let ms = Duration::from_millis;
        on_cap_event(&mut a, &key(KEY_RIGHT, true), &mut kb, t);
        assert_eq!(a.deadline(), Some(t + ms(600)));
        on_cap_event(&mut a, &key(KEY_LEFTSHIFT, true), &mut kb, t + ms(100));
        on_cap_event(&mut a, &mods(SHIFT), &mut kb, t + ms(100));
        assert_eq!(a.deadline(), Some(t + ms(600)));
        match tick(&mut a, &kb, t + ms(600)).as_slice() {
            [
                Action::Send(Msg::KeyRepeat {
                    mods, button, count: 1, ..
                }),
            ] => {
                assert_eq!(*button, keymap::button(KEY_RIGHT));
                assert_eq!(*mods, 0x0001);
            }
            o => panic!("{o:?}"),
        }
        // Another key that repeats takes the repeat over.
        on_cap_event(&mut a, &key(KEY_A, true), &mut kb, t + ms(700));
        assert_eq!(a.deadline(), Some(t + ms(1300)));
    }

    #[test]
    fn without_a_keymap_no_key_is_sent() {
        let mut a = active();
        let mut kb = Keyboard::default();
        let (out, _) = run(&mut a, &mut kb, &[key(KEY_A, true), mods(SHIFT), key(KEY_A, false)]);
        assert_eq!(out, []);
    }

    fn mods_locked(depressed: u32, locked: u32) -> CapEvent {
        CapEvent::Modifiers {
            depressed,
            latched: 0,
            locked,
            group: 0,
        }
    }

    #[test]
    fn compose_off_forwards_multi_key_raw() {
        let (mut a, mut kb) = (active_compose(false), us_compose());
        let (out, why) = run(&mut a, &mut kb, &[key(KEY_RIGHTALT, true), key(KEY_RIGHTALT, false)]);
        assert_eq!(why, None);
        assert_eq!(out, [down(0xEF20, 0, KEY_RIGHTALT), up(0xEF20, 0, KEY_RIGHTALT)]);
        assert!(!kb.is_composing());
        // With the switch off nothing reads the Compose table.
        assert!(!kb.xkb.as_ref().unwrap().compose_loaded());
    }

    /// The switch is on but no Compose table loaded: Multi_key reaches the
    /// client as with the switch off, instead of vanishing.
    #[test]
    fn compose_on_without_a_table_forwards_multi_key_raw() {
        let (mut a, mut kb) = (active_compose(true), us_compose());
        kb.xkb.as_mut().unwrap().drop_compose_for_test();
        let (out, why) = run(&mut a, &mut kb, &[key(KEY_RIGHTALT, true), key(KEY_RIGHTALT, false)]);
        assert_eq!(why, None);
        assert_eq!(out, [down(0xEF20, 0, KEY_RIGHTALT), up(0xEF20, 0, KEY_RIGHTALT)]);
    }

    /// A keymap with a and A, and Multi_key on Right Alt on a key that does
    /// not repeat (stock XKB options all let the Compose key repeat).
    const MULTI_NO_REPEAT: &str = "xkb_keymap {
        xkb_keycodes { minimum = 8; maximum = 255; <AC01> = 38; <RALT> = 108; };
        xkb_types { include \"complete\" };
        xkb_compat { include \"complete\" };
        xkb_symbols {
            key <AC01> { [ a, A ] };
            key <RALT> { repeat = False, [ Multi_key ] };
        };
    };";

    /// The switch is on but no Compose table loaded: Multi_key leaves the
    /// repeat of a key held exactly as with the switch off.
    #[test]
    fn compose_on_without_a_table_keeps_the_repeat_of_a_held_key() {
        let t = Instant::now();
        let ms = Duration::from_millis;
        let deadline = |compose: bool| {
            let mut x = Xkb::from_keymap_text(MULTI_NO_REPEAT).expect("libxkbcommon and the xkeyboard-config data");
            x.drop_compose_for_test();
            assert_eq!(x.keysym(KEY_RIGHTALT), keymap::XK_MULTI_KEY);
            assert!(!x.repeats(KEY_RIGHTALT));
            let mut kb = Keyboard {
                xkb: Some(x),
                rate: 25,
                delay: 600,
                down: Vec::new(),
            };
            let mut a = active_compose(compose);
            on_cap_event(&mut a, &key(KEY_A, true), &mut kb, t);
            assert_eq!(a.deadline(), Some(t + ms(600)));
            let out = on_cap_event(&mut a, &key(KEY_RIGHTALT, true), &mut kb, t + ms(100));
            assert_eq!(out, [Action::Send(down(0xEF20, 0, KEY_RIGHTALT))]);
            a.deadline()
        };
        assert_eq!(deadline(false), Some(t + ms(600)));
        assert_eq!(deadline(true), Some(t + ms(600)));
    }

    #[test]
    fn compose_on_resolves_multi_key_with_shift_mid_sequence_and_locks_only() {
        const KEY_APOSTROPHE: u32 = 40;
        let (mut a, mut kb) = (active_compose(true), us_compose());
        let evs = [
            mods_locked(0, 2),
            key(KEY_RIGHTALT, true),
            key(KEY_RIGHTALT, false),
            key(KEY_LEFTSHIFT, true),
            mods_locked(SHIFT, 2),
            key(KEY_APOSTROPHE, true),
            key(KEY_APOSTROPHE, false),
            key(KEY_LEFTSHIFT, false),
            mods_locked(0, 2),
            key(KEY_A, true),
            key(KEY_A, false),
        ];
        let (out, why) = run(&mut a, &mut kb, &evs);
        assert_eq!(why, None);
        // Caps Lock (locked = 2) shifts KEY_A to 'A', so Multi_key + " + A composes to Ä (0x00c4)
        // with only the lock mask (0x1000) and without Shift or Multi_key.
        assert_eq!(
            out,
            [
                Msg::KeyDown {
                    id: 0x00c4,
                    mods: 0x1000,
                    button: keymap::COMPOSED_BUTTON,
                },
                Msg::KeyUp {
                    id: 0x00c4,
                    mods: 0x1000,
                    button: keymap::COMPOSED_BUTTON,
                },
            ]
        );
        assert!(a.held.is_empty());
        assert!(!kb.is_composing());
    }

    /// Caps Lock pressed mid-sequence reaches the client, so its Caps Lock
    /// follows ThinFloe's; Shift stays held back, press and release, and
    /// the sequence still completes.
    #[test]
    fn caps_lock_mid_sequence_reaches_the_client_and_the_sequence_completes() {
        const KEY_CAPSLOCK: u32 = 58;
        const KEY_APOSTROPHE: u32 = 40;
        let (mut a, mut kb) = (active_compose(true), us_compose());
        let evs = [
            key(KEY_RIGHTALT, true),
            key(KEY_RIGHTALT, false),
            key(KEY_CAPSLOCK, true),
            mods_locked(0, 2),
            key(KEY_CAPSLOCK, false),
            key(KEY_LEFTSHIFT, true),
            mods_locked(SHIFT, 2),
            key(KEY_APOSTROPHE, true),
            key(KEY_APOSTROPHE, false),
            key(KEY_LEFTSHIFT, false),
            mods_locked(0, 2),
            key(KEY_A, true),
            key(KEY_A, false),
        ];
        let (out, why) = run(&mut a, &mut kb, &evs);
        assert_eq!(why, None);
        assert_eq!(
            out,
            [
                down(0xEFE5, 0, KEY_CAPSLOCK),
                up(0xEFE5, 0x1000, KEY_CAPSLOCK),
                Msg::KeyDown {
                    id: 0x00c4,
                    mods: 0x1000,
                    button: keymap::COMPOSED_BUTTON,
                },
                Msg::KeyUp {
                    id: 0x00c4,
                    mods: 0x1000,
                    button: keymap::COMPOSED_BUTTON,
                },
            ]
        );
        assert!(a.held.is_empty());
        assert!(!kb.is_composing());
    }

    /// Shift pressed mid-sequence and released after it never reaches the
    /// client: its release is not sent alone.
    #[test]
    fn a_modifier_held_back_mid_sequence_is_not_released_after_it() {
        const KEY_APOSTROPHE: u32 = 40;
        let (mut a, mut kb) = (active_compose(true), us_compose());
        let evs = [
            key(KEY_RIGHTALT, true),
            key(KEY_RIGHTALT, false),
            key(KEY_LEFTSHIFT, true),
            mods(SHIFT),
            key(KEY_APOSTROPHE, true),
            key(KEY_APOSTROPHE, false),
            key(KEY_A, true),
            key(KEY_A, false),
            key(KEY_LEFTSHIFT, false),
            mods(0),
        ];
        let (out, why) = run(&mut a, &mut kb, &evs);
        assert_eq!(why, None);
        assert_eq!(
            out,
            [
                Msg::KeyDown {
                    id: 0x00c4,
                    mods: 0,
                    button: keymap::COMPOSED_BUTTON,
                },
                Msg::KeyUp {
                    id: 0x00c4,
                    mods: 0,
                    button: keymap::COMPOSED_BUTTON,
                },
            ]
        );
        assert!(a.held.is_empty());
    }

    #[test]
    fn modifier_held_before_compose_still_releases_during_compose() {
        let (mut a, mut kb) = (active_compose(true), us_compose());
        let evs = [
            key(KEY_LEFTCTRL, true),
            mods(CONTROL),
            key(KEY_RIGHTALT, true),
            key(KEY_RIGHTALT, false),
            key(KEY_LEFTCTRL, false),
            mods(0),
        ];
        let (out, why) = run(&mut a, &mut kb, &evs);
        assert_eq!(why, None);
        assert_eq!(out, [down(0xEFE3, 0, KEY_LEFTCTRL), up(0xEFE3, 0x0002, KEY_LEFTCTRL)]);
        assert!(a.held.is_empty());
    }

    #[test]
    fn dead_keys_bypass_local_compose() {
        const KEY_EQUAL: u32 = 13;
        let (mut a, mut kb) = (active_compose(true), de_compose());
        let (out, why) = run(&mut a, &mut kb, &[key(KEY_EQUAL, true), key(KEY_EQUAL, false)]);
        assert_eq!(why, None);
        assert_eq!(out, [down(0x0301, 0, KEY_EQUAL), up(0x0301, 0, KEY_EQUAL)]);
        assert!(!kb.is_composing());
    }

    #[test]
    fn invalid_compose_cancels_and_next_key_forwards() {
        const KEY_X: u32 = 45;
        const KEY_Z: u32 = 44;
        let (mut a, mut kb) = (active_compose(true), us_compose());
        let evs = [
            key(KEY_RIGHTALT, true),
            key(KEY_RIGHTALT, false),
            key(KEY_X, true),
            key(KEY_X, false),
            key(KEY_Z, true),
            key(KEY_Z, false),
            key(KEY_A, true),
            key(KEY_A, false),
        ];
        let (out, why) = run(&mut a, &mut kb, &evs);
        assert_eq!(why, None);
        assert_eq!(out, [down(0x61, 0, KEY_A), up(0x61, 0, KEY_A)]);
        assert!(!kb.is_composing());
    }

    #[test]
    fn escape_chord_and_release_reset_compose_state() {
        let (mut a, mut kb) = (active_compose(true), us_compose());
        let now = Instant::now();
        assert_eq!(on_cap_event(&mut a, &key(KEY_RIGHTALT, true), &mut kb, now), []);
        assert_eq!(on_cap_event(&mut a, &key(KEY_RIGHTALT, false), &mut kb, now), []);
        assert!(kb.is_composing());
        let evs = [
            key(KEY_LEFTCTRL, true),
            mods(CONTROL),
            key(KEY_LEFTALT, true),
            mods(CONTROL | MOD1),
            key(KEY_LEFTSHIFT, true),
            mods(CONTROL | MOD1 | SHIFT),
        ];
        for ev in &evs {
            assert_eq!(on_cap_event(&mut a, ev, &mut kb, now), []);
        }
        assert_eq!(
            on_cap_event(&mut a, &key(KEY_ESC, true), &mut kb, now),
            [Action::Release("escape chord")]
        );
        assert_eq!(release(&mut a, &mut kb), [Msg::Cout]);
        assert!(!kb.is_composing());
        // Release modifiers in kb before pressing 'a' on a fresh Active.
        for ev in [
            key(KEY_LEFTSHIFT, false),
            key(KEY_LEFTALT, false),
            key(KEY_LEFTCTRL, false),
            mods(0),
        ] {
            on_cap_event(&mut a, &ev, &mut kb, now);
        }
        let mut fresh = active_compose(true);
        let (out, why) = run(&mut fresh, &mut kb, &[key(KEY_A, true), key(KEY_A, false)]);
        assert_eq!(why, None);
        assert_eq!(out, [down(0x61, 0, KEY_A), up(0x61, 0, KEY_A)]);
    }

    #[test]
    fn motion_crossing_back_carries_along_fraction() {
        let mut a = Active::new(7, Tracker::new(Side::RightOf, (0, 810), (1920, 1080)), (0, 810), false);
        let mut kb = us();
        let acts = on_cap_event(&mut a, &rel(-1.0, 0.0), &mut kb, Instant::now());
        match acts.as_slice() {
            [Action::ReleaseBack { along }] => assert!((*along - 0.75).abs() < 1e-9, "along={along}"),
            other => panic!("expected ReleaseBack, got {other:?}"),
        }
    }
}
