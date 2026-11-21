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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A message for the captured device.
    Send(Msg),
    /// End the capture ([`release`] first).
    Release(&'static str),
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
}

impl Active {
    /// The pointer entered the device at `at` (the CINN position).
    pub fn new(device: usize, tracker: Tracker, at: (i16, i16)) -> Active {
        Active {
            device,
            tracker,
            held: Vec::new(),
            buttons: Vec::new(),
            pos: at,
            repeat: None,
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
                Track::Back { .. } => vec![Action::Release("back")],
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

fn key_down(a: &mut Active, kb: &Keyboard, code: u32, now: Instant) -> Vec<Action> {
    if kb.escape_chord(code) {
        return vec![Action::Release("escape chord")];
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
pub fn release(a: &mut Active, kb: &Keyboard) -> Vec<Msg> {
    let mods = kb.mask();
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

    fn active() -> Active {
        Active::new(7, Tracker::new(Side::RightOf, (0, 540), (1920, 1080)), (0, 540))
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
        assert_eq!(release(&mut a, &kb), [Msg::Cout]);
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
        assert_eq!(release(&mut a, &kb), [Up(5), Up(2), Msg::Cout]);
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
}
