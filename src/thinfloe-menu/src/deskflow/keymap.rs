//! Key translation for the Deskflow server: a Wayland keyboard event
//! (XKB keymap, evdev code, modifier state) becomes the key id, modifier mask
//! and button of a DKDL, DKUP or DKRP message.
//!
//! The numeric values are Deskflow's wire values (`src/lib/deskflow/KeyTypes.h`
//! at commit 80221f0). The rules for which keysym becomes a Unicode key id
//! and which a `kKey…` value follow Deskflow's keysym mapping
//! (`XDGKeyUtil::mapKeySymToKeyID`); only the rules and the numbers are
//! taken over. Where the table of Deskflow has no entry, libxkbcommon's
//! keysym-to-Unicode conversion decides, which also covers the keysyms
//! Deskflow's table misses (Unicode keysyms such as `U+0100_0000 + cp`).

use std::ffi::{CStr, c_char};
use std::fs::File;
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt;

use xkbcommon_dl::{
    XkbCommon, xkb_context, xkb_context_flags, xkb_keymap, xkb_keymap_compile_flags, xkb_keymap_format, xkb_state,
    xkb_state_component,
};

/// The largest keymap read from a compositor.
const KEYMAP_MAX: u32 = 1 << 20;

// KeyModifierMask bits, Deskflow commit 80221f0 (KeyTypes.h).
const MOD_SHIFT: u16 = 0x0001;
const MOD_CONTROL: u16 = 0x0002;
const MOD_ALT: u16 = 0x0004;
const MOD_SUPER: u16 = 0x0010;
const MOD_ALTGR: u16 = 0x0020;
const MOD_CAPS_LOCK: u16 = 0x1000;
const MOD_NUM_LOCK: u16 = 0x2000;
const MOD_SCROLL_LOCK: u16 = 0x4000;

/// The modifier mask of a DKDL, DKUP or DKRP.
#[allow(clippy::too_many_arguments)]
pub fn mod_mask(
    shift: bool,
    ctrl: bool,
    alt: bool,
    logo: bool,
    altgr: bool,
    caps: bool,
    num: bool,
    scroll: bool,
) -> u16 {
    [
        (shift, MOD_SHIFT),
        (ctrl, MOD_CONTROL),
        (alt, MOD_ALT),
        (logo, MOD_SUPER),
        (altgr, MOD_ALTGR),
        (caps, MOD_CAPS_LOCK),
        (num, MOD_NUM_LOCK),
        (scroll, MOD_SCROLL_LOCK),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(0, |m, (_, bit)| m | bit)
}

/// The button of a key: the X11 keycode, which is the evdev code plus 8.
pub fn button(evdev: u32) -> u16 {
    (evdev + 8) as u16
}

/// A keysym's Deskflow key id; `None` for a key the client cannot be
/// given (no id, or beyond the 16 bit of the wire). `utf32` is the Unicode
/// character the key produces now (0 for none), used for keysyms that
/// have no `kKey…` value.
pub fn key_id(keysym: u32, utf32: u32) -> Option<u16> {
    let id: u32 = match keysym {
        // Latin-1 printable characters are their own id.
        0x20..=0x7e | 0xa0..=0xff => keysym,
        // ISO 9995 function keys and dead keys.
        0xfe00..=0xfeff => iso_key(keysym)?,
        // Miscellany: keysym - 0x1000.
        0xff00..=0xffff => keysym - 0x1000,
        // "Internet" keys.
        0x1008_ff00..=0x1008_ffff => internet_key(keysym & 0xff)?,
        _ => match utf32 {
            0x20..=0x7e | 0xa0..=0xd7ff | 0xe000..=0xffff => utf32,
            _ => return None,
        },
    };
    u16::try_from(id).ok().filter(|id| *id != 0)
}

fn iso_key(keysym: u32) -> Option<u32> {
    Some(match keysym {
        0xfe20 => 0xEE20, // ISO_Left_Tab: kKeyLeftTab
        0xfe03 => 0xEF7E, // ISO_Level3_Shift: kKeyAltGr
        0xfe11 => 0xfe11, // ISO_Level5_Shift: Deskflow passes the keysym on
        0xfe08 => 0xEE08, // ISO_Next_Group: kKeyNextGroup
        0xfe0a => 0xEE0A, // ISO_Prev_Group: kKeyPrevGroup
        // The dead keys are the combining characters.
        0xfe50 => 0x0300, // dead_grave
        0xfe51 => 0x0301, // dead_acute
        0xfe52 => 0x0302, // dead_circumflex
        0xfe53 => 0x0303, // dead_tilde
        0xfe54 => 0x0304, // dead_macron
        0xfe55 => 0x0306, // dead_breve
        0xfe56 => 0x0307, // dead_abovedot
        0xfe57 => 0x0308, // dead_diaeresis
        0xfe58 => 0x030a, // dead_abovering
        0xfe59 => 0x030b, // dead_doubleacute
        0xfe5a => 0x030c, // dead_caron
        0xfe5b => 0x0327, // dead_cedilla
        0xfe5c => 0x0328, // dead_ogonek
        _ => return None,
    })
}

/// The XF86 keysyms Deskflow forwards, by the low byte of the keysym
/// (0x1008ff__); the values are the 0xE0__ ids of KeyTypes.h at commit
/// 80221f0. XF86AudioMicMute (0xb2) has none.
fn internet_key(low: u32) -> Option<u32> {
    Some(match low {
        0x02 => 0xE0B9, // MonBrightnessUp: kKeyBrightnessUp
        0x03 => 0xE0B8, // MonBrightnessDown: kKeyBrightnessDown
        0x11 => 0xE0AE, // AudioLowerVolume: kKeyAudioDown
        0x12 => 0xE0AD, // AudioMute: kKeyAudioMute
        0x13 => 0xE0AF, // AudioRaiseVolume: kKeyAudioUp
        0x14 => 0xE0B3, // AudioPlay
        0x15 => 0xE0B2, // AudioStop
        0x16 => 0xE0B1, // AudioPrev
        0x17 => 0xE0B0, // AudioNext
        0x18 => 0xE0AC, // HomePage: kKeyWWWHome
        0x19 => 0xE0B4, // Mail: kKeyAppMail
        0x1b => 0xE0AA, // Search: kKeyWWWSearch
        0x26 => 0xE0A6, // Back: kKeyWWWBack
        0x27 => 0xE0A7, // Forward: kKeyWWWForward
        0x28 => 0xE0A9, // Stop: kKeyWWWStop
        0x29 => 0xE0A8, // Refresh: kKeyWWWRefresh
        0x2c => 0xE001, // Eject: kKeyEject
        0x30 => 0xE0AB, // Favorites: kKeyWWWFavorites
        0x32 => 0xE0B5, // AudioMedia: kKeyAppMedia
        0x40 => 0xE0B6, // Launch0: kKeyAppUser1
        0x41 => 0xE0B7, // Launch1: kKeyAppUser2
        0x4a => 0xE0C0, // kKeyMissionControl
        0x4b => 0xE0C1, // kKeyLaunchpad
        _ => return None,
    })
}

/// The modifiers in effect, as XKB reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
    pub altgr: bool,
    pub caps: bool,
    pub num: bool,
    pub scroll: bool,
}

impl Mods {
    /// The `KeyModifierMask` of these.
    pub fn mask(&self) -> u16 {
        mod_mask(
            self.shift,
            self.ctrl,
            self.alt,
            self.logo,
            self.altgr,
            self.caps,
            self.num,
            self.scroll,
        )
    }
}

/// The compositor's keymap and the modifier state, through libxkbcommon.
pub struct Xkb {
    lib: &'static XkbCommon,
    ctx: *mut xkb_context,
    keymap: *mut xkb_keymap,
    state: *mut xkb_state,
}

impl Xkb {
    /// Compiles the keymap the compositor sent (the fd's first `size` bytes).
    pub fn from_keymap_fd(fd: &OwnedFd, size: u32) -> Result<Xkb, String> {
        let lib = xkbcommon_dl::xkbcommon_option().ok_or("libxkbcommon is not available")?;
        if size == 0 || size > KEYMAP_MAX {
            return Err(format!("a keymap of {size} bytes"));
        }
        let file = File::from(fd.try_clone().map_err(|e| format!("dup: {e}"))?);
        let mut text = vec![0u8; size as usize];
        file.read_exact_at(&mut text, 0).map_err(|e| format!("read: {e}"))?;
        while text.last() == Some(&0) {
            text.pop();
        }
        text.push(0);
        let text = CStr::from_bytes_until_nul(&text).map_err(|e| e.to_string())?;
        Xkb::build(lib, |ctx| {
            // SAFETY: ctx is a live context; text is NUL-terminated.
            unsafe {
                (lib.xkb_keymap_new_from_string)(
                    ctx,
                    text.as_ptr(),
                    xkb_keymap_format::XKB_KEYMAP_FORMAT_TEXT_V1,
                    xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS,
                )
            }
        })
        .map_err(|e| {
            if e == NO_KEYMAP {
                "the keymap does not compile".into()
            } else {
                e
            }
        })
    }

    /// A keymap for the layout name from the system's XKB data (tests).
    #[cfg(test)]
    pub(crate) fn from_layout(layout: &str) -> Result<Xkb, String> {
        let lib = xkbcommon_dl::xkbcommon_option().ok_or("libxkbcommon is not available")?;
        let layout = std::ffi::CString::new(layout).map_err(|e| e.to_string())?;
        let names = xkbcommon_dl::xkb_rule_names {
            rules: std::ptr::null(),
            model: std::ptr::null(),
            layout: layout.as_ptr(),
            variant: std::ptr::null(),
            options: std::ptr::null(),
        };
        Xkb::build(lib, |ctx| {
            // SAFETY: ctx is live; names points to NUL-terminated strings.
            unsafe {
                (lib.xkb_keymap_new_from_names)(ctx, &names, xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS)
            }
        })
        .map_err(|e| {
            if e == NO_KEYMAP {
                format!("no keymap for layout {}", layout.to_string_lossy())
            } else {
                e
            }
        })
    }

    fn build(lib: &'static XkbCommon, make: impl FnOnce(*mut xkb_context) -> *mut xkb_keymap) -> Result<Xkb, String> {
        // SAFETY: plain libxkbcommon calls on pointers checked for NULL; the
        // three objects are owned by the Xkb and freed in Drop.
        unsafe {
            let ctx = (lib.xkb_context_new)(xkb_context_flags::XKB_CONTEXT_NO_FLAGS);
            if ctx.is_null() {
                return Err("xkb_context_new failed".into());
            }
            let keymap = make(ctx);
            if keymap.is_null() {
                (lib.xkb_context_unref)(ctx);
                return Err(NO_KEYMAP.into());
            }
            let state = (lib.xkb_state_new)(keymap);
            if state.is_null() {
                (lib.xkb_keymap_unref)(keymap);
                (lib.xkb_context_unref)(ctx);
                return Err("xkb_state_new failed".into());
            }
            Ok(Xkb {
                lib,
                ctx,
                keymap,
                state,
            })
        }
    }

    /// Takes the modifier state of a `wl_keyboard.modifiers` event.
    pub fn update_mask(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        // SAFETY: state is valid until Drop.
        unsafe {
            (self.lib.xkb_state_update_mask)(self.state, depressed, latched, locked, 0, 0, group);
        }
    }

    /// The keysym the key gives in the current state; 0 for none.
    pub fn keysym(&self, evdev: u32) -> u32 {
        // SAFETY: state is valid until Drop.
        unsafe { (self.lib.xkb_state_key_get_one_sym)(self.state, evdev + 8) }
    }

    /// The keysym on the key's first level in its current layout, whatever
    /// the modifiers; 0 for none. Ctrl+Alt+F1 gives `XF86Switch_VT_1` in
    /// the current state and `F1` here.
    pub fn keysym_level0(&self, evdev: u32) -> u32 {
        // SAFETY: state and keymap are valid until Drop; syms points into
        // the keymap and is read only when the count says it holds one.
        unsafe {
            let layout = (self.lib.xkb_state_key_get_layout)(self.state, evdev + 8);
            let mut syms: *const u32 = std::ptr::null();
            let n = (self.lib.xkb_keymap_key_get_syms_by_level)(self.keymap, evdev + 8, layout, 0, &mut syms);
            if n == 1 && !syms.is_null() { *syms } else { 0 }
        }
    }

    /// Whether the keymap lets the key repeat (modifiers do not).
    pub fn repeats(&self, evdev: u32) -> bool {
        // SAFETY: keymap is valid until Drop.
        unsafe { (self.lib.xkb_keymap_key_repeats)(self.keymap, evdev + 8) > 0 }
    }

    /// The Unicode character the key gives in the current state; 0 for none.
    pub fn utf32(&self, evdev: u32) -> u32 {
        // SAFETY: state is valid until Drop.
        unsafe { (self.lib.xkb_state_key_get_utf32)(self.state, evdev + 8) }
    }

    /// The effective modifiers.
    pub fn mods(&self) -> Mods {
        let on = |names: &[&[u8]]| names.iter().any(|n| self.mod_active(n));
        Mods {
            shift: on(&[b"Shift\0"]),
            ctrl: on(&[b"Control\0"]),
            alt: on(&[b"Mod1\0", b"Alt\0"]),
            logo: on(&[b"Mod4\0", b"Super\0", b"Hyper\0"]),
            altgr: on(&[b"Mod5\0", b"LevelThree\0"]),
            caps: on(&[b"Lock\0"]),
            num: on(&[b"Mod2\0", b"NumLock\0"]),
            scroll: on(&[b"ScrollLock\0"]),
        }
    }

    /// Whether the modifier is active; a name the keymap lacks is not.
    fn mod_active(&self, name: &[u8]) -> bool {
        debug_assert_eq!(name.last(), Some(&0));
        // SAFETY: name is NUL-terminated; state is valid until Drop.
        unsafe {
            (self.lib.xkb_state_mod_name_is_active)(
                self.state,
                name.as_ptr().cast::<c_char>(),
                xkb_state_component::XKB_STATE_MODS_EFFECTIVE,
            ) > 0
        }
    }
}

const NO_KEYMAP: &str = "the keymap does not build";

impl Drop for Xkb {
    fn drop(&mut self) {
        // SAFETY: all three were made in build and are freed only here.
        unsafe {
            (self.lib.xkb_state_unref)(self.state);
            (self.lib.xkb_keymap_unref)(self.keymap);
            (self.lib.xkb_context_unref)(self.ctx);
        }
    }
}

/// The name of a keysym, for logs; its hex value when libxkbcommon has none.
pub fn keysym_name(sym: u32) -> String {
    let Some(lib) = xkbcommon_dl::xkbcommon_option() else {
        return format!("0x{sym:x}");
    };
    let mut buf = [0 as c_char; 64];
    // SAFETY: the buffer and its length match; libxkbcommon NUL-terminates.
    let n = unsafe { (lib.xkb_keysym_get_name)(sym, buf.as_mut_ptr(), buf.len()) };
    if n <= 0 {
        return format!("0x{sym:x}");
    }
    // SAFETY: a NUL-terminated string in buf.
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_Y: u32 = 21;
    const KEY_A: u32 = 30;

    #[test]
    fn latin_letters_are_unicode() {
        assert_eq!(key_id(0x61, 0x61), Some(0x61));
        assert_eq!(key_id(0x41, 0x41), Some(0x41));
        assert_eq!(key_id(0x20, 0x20), Some(0x20));
    }

    #[test]
    fn umlaut() {
        assert_eq!(key_id(0xe4, 0xe4), Some(0xE4));
    }

    #[test]
    fn other_scripts_and_the_euro_sign_are_unicode() {
        assert_eq!(key_id(0x20ac, 0x20ac), Some(0x20AC)); // EuroSign
        assert_eq!(key_id(0x6c1, 0x0430), Some(0x0430)); // Cyrillic_a
        assert_eq!(key_id(0x0100_20ac, 0x20ac), Some(0x20AC)); // a Unicode keysym
        assert_eq!(key_id(0x0101_f600, 0x1f600), None); // beyond the BMP
        assert_eq!(key_id(0x6c1, 0), None); // no character
        assert_eq!(key_id(0x6c1, 0x01), None); // a control character
    }

    #[test]
    fn return_escape_f1_match_keytypes() {
        assert_eq!(key_id(0xff0d, 0x0d), Some(0xEF0D)); // kKeyReturn
        assert_eq!(key_id(0xff1b, 0x1b), Some(0xEF1B)); // kKeyEscape
        assert_eq!(key_id(0xffbe, 0), Some(0xEFBE)); // kKeyF1
        assert_eq!(key_id(0xff08, 0x08), Some(0xEF08)); // kKeyBackSpace
        assert_eq!(key_id(0xff09, 0x09), Some(0xEF09)); // kKeyTab
        assert_eq!(key_id(0xffff, 0x7f), Some(0xEFFF)); // kKeyDelete
        assert_eq!(key_id(0xff51, 0), Some(0xEF51)); // kKeyLeft
        assert_eq!(key_id(0xff8d, 0x0d), Some(0xEF8D)); // kKeyKP_Enter
        assert_eq!(key_id(0xffb5, 0x35), Some(0xEFB5)); // kKeyKP_5
        assert_eq!(key_id(0xffe1, 0), Some(0xEFE1)); // kKeyShift_L
        assert_eq!(key_id(0xffeb, 0), Some(0xEFEB)); // kKeySuper_L
        assert_eq!(key_id(0xfe20, 0), Some(0xEE20)); // ISO_Left_Tab: kKeyLeftTab
        assert_eq!(key_id(0xfe03, 0), Some(0xEF7E)); // ISO_Level3_Shift: kKeyAltGr
        assert_eq!(key_id(0xfe57, 0), Some(0x0308)); // dead_diaeresis
        assert_eq!(key_id(0xfeff, 0), None);
    }

    #[test]
    fn media_keys_match_keytypes() {
        assert_eq!(key_id(0x1008_ff12, 0), Some(0xE0AD)); // AudioMute
        assert_eq!(key_id(0x1008_ff11, 0), Some(0xE0AE)); // AudioLowerVolume
        assert_eq!(key_id(0x1008_ff13, 0), Some(0xE0AF)); // AudioRaiseVolume
        assert_eq!(key_id(0x1008_ff03, 0), Some(0xE0B8)); // MonBrightnessDown
        assert_eq!(key_id(0x1008_ff02, 0), Some(0xE0B9)); // MonBrightnessUp
    }

    #[test]
    fn mic_mute_is_none() {
        assert_eq!(key_id(0x1008_ffb2, 0), None);
    }

    #[test]
    fn mod_mask_bits() {
        assert_eq!(mod_mask(true, false, false, false, false, false, false, false), 0x0001);
        assert_eq!(mod_mask(false, true, false, false, false, false, false, false), 0x0002);
        assert_eq!(mod_mask(false, false, true, false, false, false, false, false), 0x0004);
        assert_eq!(mod_mask(false, false, false, true, false, false, false, false), 0x0010);
        assert_eq!(mod_mask(false, false, false, false, true, false, false, false), 0x0020);
        assert_eq!(mod_mask(false, false, false, false, false, true, false, false), 0x1000);
        assert_eq!(mod_mask(false, false, false, false, false, false, true, false), 0x2000);
        assert_eq!(mod_mask(false, false, false, false, false, false, false, true), 0x4000);
        assert_eq!(mod_mask(true, true, true, true, true, true, true, true), 0x7037);
        assert_eq!(mod_mask(false, false, false, false, false, false, false, false), 0);
    }

    #[test]
    fn button_is_x11_keycode() {
        assert_eq!(button(30), 38);
    }

    fn de() -> Xkb {
        Xkb::from_layout("de").expect("libxkbcommon and the xkeyboard-config data are needed for this test")
    }

    #[test]
    fn xkb_de_layout_y_is_z() {
        let x = de();
        assert_eq!(x.keysym(KEY_Y), 0x7a);
        assert_eq!(x.utf32(KEY_Y), 0x7a);
        assert_eq!(key_id(x.keysym(KEY_Y), x.utf32(KEY_Y)), Some(0x7a));
    }

    #[test]
    fn xkb_modifier_state_changes_keysym_and_mask() {
        let mut x = de();
        assert_eq!(x.keysym(KEY_A), 0x61);
        assert_eq!(x.mods().mask(), 0);
        x.update_mask(1, 0, 0, 0); // Shift
        assert_eq!(x.keysym(KEY_A), 0x41);
        assert!(x.mods().shift);
        assert_eq!(x.mods().mask(), 0x0001);
        x.update_mask(0, 0, 2, 0); // Lock (Caps Lock)
        assert!(x.mods().caps);
        assert_eq!(x.mods().mask(), 0x1000);
        x.update_mask(0, 0, 0, 0);
        assert_eq!(x.mods(), Mods::default());
    }

    #[test]
    fn level0_ignores_the_modifiers_and_only_keys_repeat() {
        const KEY_F1: u32 = 59;
        const KEY_LEFTCTRL: u32 = 29;
        let mut x =
            Xkb::from_layout("us").expect("libxkbcommon and the xkeyboard-config data are needed for this test");
        x.update_mask(4 | 8, 0, 0, 0); // Control, Mod1
        assert_eq!(x.keysym(KEY_F1), 0x1008_fe01); // XF86Switch_VT_1
        assert_eq!(key_id(x.keysym(KEY_F1), x.utf32(KEY_F1)), None);
        assert_eq!(x.keysym_level0(KEY_F1), 0xffbe); // F1
        x.update_mask(1, 0, 0, 0); // Shift
        assert_eq!(x.keysym(KEY_A), 0x41);
        assert_eq!(x.keysym_level0(KEY_A), 0x61);
        assert!(x.repeats(KEY_A));
        assert!(!x.repeats(KEY_LEFTCTRL));
    }

    #[test]
    fn a_bad_keymap_is_an_error() {
        let file = File::open("/dev/null").unwrap();
        assert!(Xkb::from_keymap_fd(&OwnedFd::from(file), 0).is_err());
    }
}
