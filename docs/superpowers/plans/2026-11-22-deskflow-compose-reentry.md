# Deskflow: Per-Device Compose Key and Pointer Re-Entry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional per-device local `Multi_key` Compose resolver (off by default) and proportional pointer re-entry along the attached monitor edge to ThinFloe's Deskflow server.

**Architecture:** `DeviceCfg` gains a `compose: bool` field (default `false`) exposed as a switch in Settings -> Screen beside "Share the clipboard". `deskflow::keymap::Xkb` owns an `xkb_compose_state` loaded via `xkbcommon-dl`'s `xkbcommon_compose_option()` (with `"en_US.UTF-8"` fallback) and `deskflow::forward` intercepts only `Multi_key` (`0xFF20`) sequences when the active device has `compose = true`, emitting a paired `KeyDown`/`KeyUp` with lock-only modifiers and synthetic button `1`. On return from a client (`Track::Back { along }`), `deskflow::edge::reentry_point` maps `along` to strip-local and layout coordinates; `Capture::end_capture` sends `zwp_locked_pointer_v1::set_cursor_position_hint` + `surface.commit()` before destroying the lock (Approach 2A), and `Daemon::release` updates `hover.at` and resets resistance.

**Tech Stack:** Rust 1.96 (`thinfloe-menu`), `xkbcommon-dl` 0.4.2 (`XkbCommonCompose`), `wayland-protocols` (`zwp_locked_pointer_v1`), `egui` 0.37 / `shadcn-egui` 0.8 (`lyra`), Bash integration & Podman/`labwc 0.9.3`/`deskflow-core 1.26.0` runtime tests.

**Spec:** `docs/superpowers/specs/2026-11-22-deskflow-compose-reentry-design.md`

## Global Constraints

- Worktree: `/scratch/oetiker/claude-worktrees/thinfloe-deskflow-compose` (branch `feat/deskflow-compose-reentry`).
- No new Cargo dependencies (`xkbcommon-dl 0.4.2` already exposes `xkbcommon_compose_option()`).
- Max 4 cores locally:
  - Cargo tests: `flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock systemd-run --user --scope -q -p MemoryMax=6G -- env CARGO_BUILD_JOBS=4 cargo test -- --test-threads=4` (from `src/thinfloe-menu`).
  - Image/runtime targets: `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make <target> VERSION=deskflow-compose`.
- UI code takes colours from the theme and text sizes from `brand.rs` (`ui_code_has_no_style_literals` enforces this); `shadcn-egui` `lyra` style, nothing rounded.
- Per-device `compose` defaults to `false`; absent in TOML reads as `false`; preserved on fingerprint re-attach.
- Local compose intercepts `Multi_key` (`0xFF20`) only; dead keys (`0xFE50..=0xFE5C`) continue to travel as Deskflow dead-key IDs.
- Synthetic `KeyDown`/`KeyUp` button is `1` (`COMPOSED_BUTTON`: non-zero, `< 8` so never colliding with `evdev + 8`, `< 0x200` so Deskflow `KeyState::fakeKeyUp`'s unmasked `m_serverKeys[serverID] = 0` stays in bounds).
- Approach 2A (`set_cursor_position_hint` + `surface.commit()` before `lock.destroy()`) must be verified on `labwc 0.9.3` in `test/runtime/test-labwc-runtime.sh`; switch to fallback 2B (`zwlr_virtual_pointer_v1`) only if `labwc 0.9.3` ignores the hint on layer-shell surfaces.
- Verify `/usr/share/X11/locale/en_US.UTF-8/Compose` is in the ThinFloe rootfs with an integration guard in `test/integration/test-session-packages.sh`.
- Update `docs/notes/deskflow.md` and `CHANGES.md` in house style (`writing-style` skill).

## Review Focus

1. **Modifiers pressed or released mid-compose:** Shift pressed during `Compose + " + a` must shift `'` to `"` inside XKB without being sent to the client, and the composed `KeyDown`/`KeyUp` must carry only lock modifiers (`Caps`/`Num`/`Scroll`); conversely, a modifier pressed *before* `Multi_key` and released *during* compose is already in `Active.held` and must still emit its `KeyUp` so it never sticks on the client. (Pinned in Task 3.)
2. **Aborted or interrupted compose sequences:** An invalid sequence (`Compose + x + z`) must swallow the invalid key, reset compose state, and forward the next normal key cleanly; `Ctrl+Alt+Shift+Escape`, capture release, disconnect, keymap reload, or toggling `compose = false` mid-sequence must reset compose state and never leave compose stuck. (Pinned in Tasks 2, 3, 4.)
3. **Dead keys and non-BMP / multi-codepoint compose outputs:** Dead keys (`dead_acute`, `dead_circumflex`, `0xFE50..=0xFE5C`) with `compose = true` must bypass local compose and forward as Deskflow dead-key IDs; a compose sequence whose output is outside `key_id`'s BMP range or multi-codepoint must reset compose state and send nothing. (Pinned in Tasks 2, 3.)
4. **Locale fallback and missing compose table:** Unset, empty, `"C"`, `"POSIX"`, or `"C.UTF-8"` locale must fall back to `"en_US.UTF-8"`; if compose table creation still returns null, `Xkb` must still load and forward non-compose keys normally. (Pinned in Task 2.)
5. **Re-entry edge bounds and non-bounce on the 1 px strip:** `reentry_point` for `along` at `0.0`, `1.0`, out-of-range, or `NaN` across all four sides and offset outputs must stay inside strip surface bounds (`0..w`, `0..h`); returning onto the strip must reset resistance so inward/sideways/zero motion never bounces back, while a later outward push enters at the updated `hover.at`. (Pinned in Tasks 3, 4, 5.)

---

## File Structure

- `src/thinfloe-menu/src/config.rs` — add `compose: bool` (`#[serde(default)]`) to `DeviceCfg`.
- `src/thinfloe-menu/src/deskflowcfg.rs` — test `compose` default (`false`) and TOML roundtrip.
- `src/thinfloe-menu/src/deskflowtab.rs` — add `DeskflowEvent::Compose { name, on }`, preserve `compose` on fingerprint re-attach.
- `src/thinfloe-menu/src/deskflow_ui.rs` — add `"Handle Compose key on ThinFloe"` switch beside `"Share the clipboard"` in `device_rows`.
- `src/thinfloe-menu/src/{app.rs,convert.rs,store.rs,deskflow/pair.rs}` — add `compose: false` to existing `DeviceCfg` test literals.
- `src/thinfloe-menu/src/deskflow/keymap.rs` — add `XK_MULTI_KEY`, `COMPOSED_BUTTON`, `Mods::locks_mask()`, locale resolution, and `XkbCompose` owned by `Xkb`.
- `src/thinfloe-menu/src/deskflow/edge.rs` — add `reentry_point(out, side, along)`.
- `src/thinfloe-menu/src/deskflow/forward.rs` — add `Active.compose`, `Action::ReleaseBack { along }`, `Multi_key`-only compose interception in `key_down`, compose reset in `release(&mut Active, &mut Keyboard)`.
- `src/thinfloe-menu/src/deskflow/capture.rs` — extend `Capture::end_capture(&mut self, hint: Option<(f64, f64)>) -> bool` to call `set_cursor_position_hint` + `surface.commit()` before `lock.destroy()`.
- `src/thinfloe-menu/src/deskflow/run.rs` — wire `DeviceCfg.compose` into `Active` and live `reload()`, wire `Action::ReleaseBack { along }` through `edge::reentry_point`, `capture.end_capture(hint)`, and `hover.at`.
- `src/thinfloe-menu/src/deskflow/mod.rs` — pass `None` to `cap.end_capture(None)` in `probe_capture`.
- `test/integration/test-session-packages.sh` — guard `/usr/share/X11/locale/en_US.UTF-8/Compose`, `compose.dir`, `locale.dir`.
- `test/runtime/test-labwc-runtime.sh` — runtime test for local Compose (`ä`, dead key, `compose = false`) and proportional pointer re-entry on `labwc 0.9.3` + stock `deskflow-core 1.26.0`.
- `docs/notes/deskflow.md`, `CHANGES.md` — document per-device Compose and pointer re-entry in house style.

---

### Task 1: Per-device `compose` config, tab state, and Settings UI

**Files:**
- Modify: `src/thinfloe-menu/src/config.rs:477-493`
- Modify: `src/thinfloe-menu/src/deskflowcfg.rs:122-215`
- Modify: `src/thinfloe-menu/src/deskflowtab.rs:66-192,407-580`
- Modify: `src/thinfloe-menu/src/deskflow_ui.rs:32-38,190-224`
- Modify (test literals): `src/thinfloe-menu/src/app.rs:2687`, `src/thinfloe-menu/src/convert.rs:738`, `src/thinfloe-menu/src/store.rs:494,573`, `src/thinfloe-menu/src/deskflow/pair.rs:125`

**Interfaces:**
- Produces:
  - `config::DeviceCfg { ..., #[serde(default)] pub compose: bool }` (defaults to `false`)
  - `deskflowtab::DeskflowEvent::Compose { name: String, on: bool }`
  - `deskflow_ui::COMPOSE_TEXT = "Handle Compose key on ThinFloe"`

- [ ] **Step 1: Write failing tests in `deskflowcfg.rs` and `deskflowtab.rs`**

In `src/thinfloe-menu/src/deskflowcfg.rs` (extend `toml_roundtrip_matches_spec_block` or add `compose_defaults_false_and_roundtrips`):
```rust
#[test]
fn compose_defaults_false_and_roundtrips() {
    let absent = "[[deskflow.device]]\nname = \"a\"\nfingerprint = \"x\"\noutput = \"o\"\nside = \"above\"\n";
    let c = crate::config::parse(absent).unwrap();
    assert!(!c.deskflow.devices[0].compose);

    let on = format!("{absent}compose = true\n");
    let c = crate::config::parse(&on).unwrap();
    assert!(c.deskflow.devices[0].compose);
    let again = crate::config::parse(&crate::config::to_text(&c)).unwrap();
    assert!(again.deskflow.devices[0].compose);

    let g = parse(&render(&c.deskflow, false)).unwrap();
    assert!(g.deskflow.devices[0].compose);
}
```

In `src/thinfloe-menu/src/deskflowtab.rs`:
```rust
#[test]
fn compose_switch_writes_device_and_survives_reattach() {
    let mut tab = DeskflowTab::default();
    tab.cfg.devices = vec![
        device("laptop", FP_A, "DP-1", Side::LeftOf),
        device("mac", FP_B, "DP-2", Side::RightOf),
    ];
    let r = apply(&mut tab, DeskflowEvent::Compose { name: "mac".into(), on: true });
    let Some(DeskflowRequest::Save(cfg)) = r else { panic!("no save: {r:?}"); };
    assert!(!cfg.devices[0].compose);
    assert!(cfg.devices[1].compose);
    assert_eq!(apply(&mut tab, DeskflowEvent::Compose { name: "mac".into(), on: true }), None);

    // Re-attaching with a new fingerprint preserves compose = true.
    apply(&mut tab, DeskflowEvent::Attach { name: "mac".into(), fp: FP_A.into() });
    let r = apply(&mut tab, DeskflowEvent::Place { name: "mac".into(), output: "DP-2".into(), side: Side::RightOf });
    let Some(DeskflowRequest::Save(cfg)) = r else { panic!("no save: {r:?}"); };
    assert!(cfg.devices.iter().find(|d| d.name == "mac").unwrap().compose);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run (from `src/thinfloe-menu`):
`flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock systemd-run --user --scope -q -p MemoryMax=6G -- env CARGO_BUILD_JOBS=4 cargo test -- --test-threads=4 deskflowcfg deskflowtab`
Expected: FAIL to compile (`no field compose on type DeviceCfg`, `no variant named Compose`).

- [ ] **Step 3: Implement `DeviceCfg.compose`, `DeskflowEvent::Compose`, and UI switch**

1. In `src/thinfloe-menu/src/config.rs`, add `#[serde(default)] pub compose: bool` to `DeviceCfg` after `clipboard`.
2. Update existing `DeviceCfg { ... }` test literals in `deskflowcfg.rs`, `deskflowtab.rs`, `deskflow/pair.rs`, `app.rs`, `convert.rs`, `store.rs` with `compose: false`.
3. In `src/thinfloe-menu/src/deskflowtab.rs`:
   - Add `DeskflowEvent::Compose { name: String, on: bool }`.
   - In `apply`'s `DeskflowEvent::Place` attaching branch, preserve `compose` from any existing device with the same `name` (`is_some_and(|d| d.compose)`).
   - Add `DeskflowEvent::Compose { name, on }` arm matching `DeskflowEvent::Clipboard`.
4. In `src/thinfloe-menu/src/deskflow_ui.rs`:
   - Add `const COMPOSE_TEXT: &str = "Handle Compose key on ThinFloe";`.
   - In `device_rows`, immediately after the `CLIPBOARD_TEXT` switch row, add a `ui.horizontal` with `Switch::new(&mut compose)` emitting `DeskflowEvent::Compose { name: d.name.clone(), on: compose }` and `ui.label(RichText::new(COMPOSE_TEXT).color(t.palette().foreground))`.

- [ ] **Step 4: Run menu tests and style literal guard**

Run (from `src/thinfloe-menu`):
`flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock systemd-run --user --scope -q -p MemoryMax=6G -- env CARGO_BUILD_JOBS=4 cargo test -- --test-threads=4 deskflowcfg deskflowtab ui_code_has_no_style_literals`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/config.rs src/thinfloe-menu/src/deskflowcfg.rs \
  src/thinfloe-menu/src/deskflowtab.rs src/thinfloe-menu/src/deskflow_ui.rs \
  src/thinfloe-menu/src/app.rs src/thinfloe-menu/src/convert.rs \
  src/thinfloe-menu/src/store.rs src/thinfloe-menu/src/deskflow/pair.rs
git commit -m "feat(deskflow): add per-device compose setting and Screen tab switch"
```

---

### Task 2: Local `libxkbcommon` Compose state in `deskflow/keymap.rs`

**Files:**
- Modify: `src/thinfloe-menu/src/deskflow/keymap.rs`

**Interfaces:**
- Produces:
  - `pub const XK_MULTI_KEY: u32 = 0xff20;`
  - `pub const COMPOSED_BUTTON: u16 = 1;`
  - `pub fn Mods::locks_mask(&self) -> u16` (returns `self.mask() & (MOD_CAPS_LOCK | MOD_NUM_LOCK | MOD_SCROLL_LOCK)`)
  - `pub enum ComposeOutcome { Idle, Composing, Composed(Option<u16>), Cancelled }`
  - `pub fn Xkb::is_composing(&self) -> bool`
  - `pub fn Xkb::compose_feed(&mut self, keysym: u32) -> ComposeOutcome`
  - `pub fn Xkb::compose_reset(&mut self)`
  - `pub(crate) fn compose_locale(lc_all: Option<&str>, lc_ctype: Option<&str>, lang: Option<&str>) -> &str` (returns first usable non-empty locale not equal to `"C"`, `"POSIX"`, or starting with `"C."`, else `"en_US.UTF-8"`)

- [ ] **Step 1: Write failing unit tests in `src/thinfloe-menu/src/deskflow/keymap.rs`**

```rust
#[test]
fn locks_mask_keeps_only_lock_modifiers() {
    let m = Mods {
        shift: true, ctrl: true, alt: true, logo: true, altgr: true,
        caps: true, num: true, scroll: false,
    };
    assert_eq!(m.locks_mask(), 0x1000 | 0x2000);
    assert_eq!(Mods::default().locks_mask(), 0);
}

#[test]
fn compose_locale_falls_back_to_en_us_utf8() {
    assert_eq!(compose_locale(None, None, None), "en_US.UTF-8");
    assert_eq!(compose_locale(Some(""), Some("C"), Some("POSIX")), "en_US.UTF-8");
    assert_eq!(compose_locale(None, None, Some("C.UTF-8")), "en_US.UTF-8");
    assert_eq!(compose_locale(None, Some("de_CH.UTF-8"), Some("en_GB.UTF-8")), "de_CH.UTF-8");
    assert_eq!(compose_locale(Some("fr_FR.UTF-8"), Some("de_CH.UTF-8"), None), "fr_FR.UTF-8");
}

#[test]
fn xkb_compose_resolves_multi_key_sequences_and_resets() {
    let mut x = Xkb::from_layout("us").expect("libxkbcommon + xkeyboard-config");
    assert!(!x.is_composing());
    // Multi_key + " + a => ä (U+00E4)
    assert_eq!(x.compose_feed(XK_MULTI_KEY), ComposeOutcome::Composing);
    assert!(x.is_composing());
    // Modifier mid-sequence is ignored by libxkbcommon and keeps composing.
    assert_eq!(x.compose_feed(0xffe1), ComposeOutcome::Composing); // Shift_L
    assert_eq!(x.compose_feed(0x0022), ComposeOutcome::Composing); // quotedbl
    assert_eq!(x.compose_feed(0x0061), ComposeOutcome::Composed(Some(0x00e4)));
    assert!(!x.is_composing());

    // Multi_key + = + e => € (U+20AC)
    assert_eq!(x.compose_feed(XK_MULTI_KEY), ComposeOutcome::Composing);
    assert_eq!(x.compose_feed(0x003d), ComposeOutcome::Composing);
    assert_eq!(x.compose_feed(0x0065), ComposeOutcome::Composed(Some(0x20ac)));

    // Multi_key + - + - + - => — (U+2014, where keysym 0x0aa9 != utf32)
    assert_eq!(x.compose_feed(XK_MULTI_KEY), ComposeOutcome::Composing);
    assert_eq!(x.compose_feed(0x002d), ComposeOutcome::Composing);
    assert_eq!(x.compose_feed(0x002d), ComposeOutcome::Composing);
    assert_eq!(x.compose_feed(0x002d), ComposeOutcome::Composed(Some(0x2014)));

    // Multi_key + . + . => … (U+2026)
    assert_eq!(x.compose_feed(XK_MULTI_KEY), ComposeOutcome::Composing);
    assert_eq!(x.compose_feed(0x002e), ComposeOutcome::Composing);
    assert_eq!(x.compose_feed(0x002e), ComposeOutcome::Composed(Some(0x2026)));

    // Invalid sequence cancels and resets cleanly.
    assert_eq!(x.compose_feed(XK_MULTI_KEY), ComposeOutcome::Composing);
    assert_eq!(x.compose_feed(0x0078), ComposeOutcome::Composing); // x
    assert_eq!(x.compose_feed(0x007a), ComposeOutcome::Cancelled); // z
    assert!(!x.is_composing());

    // Explicit reset clears in-progress compose.
    assert_eq!(x.compose_feed(XK_MULTI_KEY), ComposeOutcome::Composing);
    x.compose_reset();
    assert!(!x.is_composing());
}
```

Also add a unit test `utf8_to_single_utf32_rejects_multi_char_and_empty` pinning the helper that converts `xkb_compose_state_get_utf8` output into `u32` (returns `0` for empty or multi-codepoint strings; `key_id(0, 0x1f600) == None` already rejects non-BMP).

- [ ] **Step 2: Run tests to verify they fail**

Run (from `src/thinfloe-menu`):
`flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock systemd-run --user --scope -q -p MemoryMax=6G -- env CARGO_BUILD_JOBS=4 cargo test -- --test-threads=4 deskflow::keymap`
Expected: FAIL to compile (`XK_MULTI_KEY`, `locks_mask`, `compose_locale`, `compose_feed` not found).

- [ ] **Step 3: Implement `XkbCompose` and helpers in `src/thinfloe-menu/src/deskflow/keymap.rs`**

- Export `pub const XK_MULTI_KEY: u32 = 0xff20;` and `pub const COMPOSED_BUTTON: u16 = 1;`.
- Add `Mods::locks_mask(&self) -> u16`.
- Add `compose_locale(lc_all, lc_ctype, lang) -> &str` returning `"en_US.UTF-8"` when unset, empty, `"C"`, `"POSIX"`, or starting with `"C."`.
- Add private `Compose { lib: &'static XkbCommonCompose, table: *mut xkb_compose_table, state: *mut xkb_compose_state }` built inside `Xkb::build` using `xkbcommon_dl::xkbcommon_compose_option()`:
  - Call `xkb_compose_table_new_from_locale(ctx, locale, XKB_COMPOSE_COMPILE_NO_FLAGS)`. If NULL and `locale != "en_US.UTF-8"`, retry with `"en_US.UTF-8"`.
  - If loading fails (missing table or missing symbols), log once via `eprintln!` and set `compose: None` so `Xkb` still works.
  - In `Drop for Xkb`: unref `compose.state` and `compose.table` BEFORE `xkb_context_unref(self.ctx)`.
- Implement `is_composing`, `compose_reset`, and `compose_feed`:
  - If `self.compose` is `None`, return `ComposeOutcome::Idle`.
  - Call `xkb_compose_state_feed(state, keysym)`, then `xkb_compose_state_get_status(state)`:
    - `XKB_COMPOSE_NOTHING` -> `ComposeOutcome::Idle`
    - `XKB_COMPOSE_COMPOSING` -> `ComposeOutcome::Composing`
    - `XKB_COMPOSE_CANCELLED` -> `xkb_compose_state_reset(state)`; return `ComposeOutcome::Cancelled`
    - `XKB_COMPOSE_COMPOSED` -> read `sym = xkb_compose_state_get_one_sym(state)` and UTF-8 bytes via `xkb_compose_state_get_utf8(state, buf, buf.len())`; call `xkb_compose_state_reset(state)`; convert single-scalar UTF-8 to `utf32` (if UTF-8 has >1 scalar return `Composed(None)` — multi-codepoint output is unsupported by Deskflow `KeyID`; if UTF-8 is empty fall back to `xkb_keysym_to_utf32(sym)`); return `ComposeOutcome::Composed(key_id(sym, utf32))`.

- [ ] **Step 4: Run tests to verify they pass**

Run (from `src/thinfloe-menu`):
`flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock systemd-run --user --scope -q -p MemoryMax=6G -- env CARGO_BUILD_JOBS=4 cargo test -- --test-threads=4 deskflow::keymap`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/deskflow/keymap.rs
git commit -m "feat(deskflow): add libxkbcommon compose state to keymap"
```

---

### Task 3: Compose interception and pointer re-entry geometry (`edge.rs`, `forward.rs`)

**Files:**
- Modify: `src/thinfloe-menu/src/deskflow/edge.rs`
- Modify: `src/thinfloe-menu/src/deskflow/forward.rs`

**Interfaces:**
- Consumes (from Task 2):
  - `keymap::XK_MULTI_KEY`, `keymap::COMPOSED_BUTTON`, `keymap::ComposeOutcome`
  - `Xkb::is_composing(&self) -> bool`, `Xkb::compose_feed(&mut self, keysym: u32) -> ComposeOutcome`, `Xkb::compose_reset(&mut self)`
  - `Mods::locks_mask(&self) -> u16`
- Produces:
  - `pub fn edge::reentry_point(out: Rect, side: Side, along: f64) -> ((f64, f64), (f64, f64))` returning `((surface_x, surface_y), (layout_x, layout_y))`
  - `Action::ReleaseBack { along: f64 }` (and `Action` derives `Debug, Clone, PartialEq`)
  - `Active::new(device: usize, tracker: Tracker, at: (i16, i16), compose: bool) -> Active`
  - `pub Active.compose: bool`
  - `pub fn Keyboard::is_composing(&self) -> bool`
  - `pub fn Keyboard::reset_compose(&mut self)`
  - `pub fn forward::release(a: &mut Active, kb: &mut Keyboard) -> Vec<Msg>`

- [ ] **Step 1: Write failing tests in `edge.rs` and `forward.rs`**

In `src/thinfloe-menu/src/deskflow/edge.rs`:
```rust
#[test]
fn reentry_maps_proportionally_and_clamps_inside_strip() {
    let o = r(1280, 100, 1920, 1080);
    assert_eq!(reentry_point(o, Side::RightOf, 0.5), ((0.0, 540.0), (3199.0, 640.0)));
    assert_eq!(reentry_point(o, Side::LeftOf, 0.25), ((0.0, 270.0), (1280.0, 370.0)));
    assert_eq!(reentry_point(o, Side::Below, 0.5), ((960.0, 0.0), (2240.0, 1179.0)));
    assert_eq!(reentry_point(o, Side::Above, 1.0), ((1919.0, 0.0), (3199.0, 100.0)));
    // Clamping, non-finite and degenerate output never leave strip bounds.
    assert_eq!(reentry_point(o, Side::RightOf, -0.5), ((0.0, 0.0), (3199.0, 100.0)));
    assert_eq!(reentry_point(o, Side::RightOf, 1.5), ((0.0, 1079.0), (3199.0, 1179.0)));
    assert_eq!(reentry_point(o, Side::RightOf, f64::NAN), ((0.0, 540.0), (3199.0, 640.0)));
    assert_eq!(reentry_point(r(0, 0, 0, 0), Side::RightOf, 0.5), ((0.0, 0.0), (0.0, 0.0)));
}
```

In `src/thinfloe-menu/src/deskflow/forward.rs`, add a helper `us_compose()` building an `Xkb` with `"us"` and options `"compose:ralt"` (extend `#[cfg(test)] Xkb::from_layout_options(layout, options)` in `keymap.rs` if needed — 3 lines, no production change) and `active_compose(compose: bool) -> Active`. Write these tests:
1. `compose_off_forwards_multi_key_raw`: with `compose = false`, pressing `KEY_RIGHTALT` (100, `Multi_key`) sends `KeyDown { id: 0xEF20, .. }` and `KeyUp { id: 0xEF20, .. }`.
2. `compose_on_resolves_multi_key_with_shift_mid_sequence_and_locks_only`: with `compose = true` and Caps Lock on (`locked = 2`), `Multi_key` down/up, `Shift` down + `mods(SHIFT, locked=2)`, `'` (`KEY_APOSTROPHE = 40`, shifted `"`) down/up, `Shift` up + `mods(0, locked=2)`, `a` down/up emits ONLY `[KeyDown { id: 0x00e4, mods: 0x1000, button: COMPOSED_BUTTON }, KeyUp { id: 0x00e4, mods: 0x1000, button: COMPOSED_BUTTON }]`; `a.held` is empty; no Shift or `Multi_key` messages sent.
3. `modifier_held_before_compose_still_releases_during_compose`: with `compose = true`, `LeftCtrl` pressed *before* `Multi_key` (sent down), then `Multi_key` down, then `LeftCtrl` up sends `KeyUp` for `LeftCtrl` so it does not stick.
4. `dead_keys_bypass_local_compose`: on a `"de"` layout with `compose = true`, pressing the dead acute key (`KEY_EQUAL = 13` on `de`) forwards `KeyDown { id: 0x0301, .. }` and does NOT enter compose.
5. `invalid_compose_cancels_and_next_key_forwards`: `Multi_key`, `x`, `z` emits nothing; following `a` emits normal `KeyDown`/`KeyUp` for `0x61`.
6. `escape_chord_and_release_reset_compose_state`: mid-compose (`Multi_key` pressed), `Ctrl+Alt+Shift+Escape` returns `Action::Release("escape chord")` and `release(&mut a, &mut kb)` resets `kb`; a following `a` on a fresh `Active` forwards normally without being eaten by stale compose state.
7. `motion_crossing_back_carries_along_fraction`: `Tracker::motion` crossing back at `y = 810` of `1080` returns `Action::ReleaseBack { along }` with `(along - 0.75).abs() < 1e-9`.

- [ ] **Step 2: Run tests to verify they fail**

Run (from `src/thinfloe-menu`):
`flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock systemd-run --user --scope -q -p MemoryMax=6G -- env CARGO_BUILD_JOBS=4 cargo test -- --test-threads=4 deskflow::edge deskflow::forward`
Expected: FAIL to compile (`reentry_point`, `ReleaseBack`, `Active::new` arity).

- [ ] **Step 3: Implement `edge::reentry_point` and `forward` compose + `ReleaseBack`**

1. In `src/thinfloe-menu/src/deskflow/edge.rs`, implement `reentry_point(out: Rect, side: Side, along: f64) -> ((f64, f64), (f64, f64))`:
   - `let f = if along.is_finite() { along.clamp(0.0, 1.0) } else { 0.5 };`
   - `let strip = strip_rect(out, side);`
   - `let (sx, sy) = match side`:
     - `Side::RightOf | Side::LeftOf => (0.0, (f * f64::from(out.h.max(0))).clamp(0.0, f64::from(out.h.saturating_sub(1).max(0))))`
     - `Side::Above | Side::Below => ((f * f64::from(out.w.max(0))).clamp(0.0, f64::from(out.w.saturating_sub(1).max(0))), 0.0)`
   - Return `((sx, sy), (f64::from(strip.x) + sx, f64::from(strip.y) + sy))`.
2. In `src/thinfloe-menu/src/deskflow/forward.rs`:
   - Change `Action` to `#[derive(Debug, Clone, PartialEq)]` and add `ReleaseBack { along: f64 }`.
   - Change `Track::Back { along } => vec![Action::ReleaseBack { along }]`.
   - Add `pub compose: bool` to `Active` and `compose: bool` parameter to `Active::new`.
   - Add `pub fn Keyboard::is_composing(&self) -> bool`, `pub fn Keyboard::reset_compose(&mut self)`, and `fn Keyboard::locks_mask(&self) -> u16`.
   - In `key_down(a: &mut Active, kb: &mut Keyboard, code: u32, now: Instant)`:
     - Check `escape_chord(code)` and `mic_mute(code)` first (unchanged — release always wins).
     - If `a.compose` and `let Some(x) = kb.xkb.as_mut()`:
       - Let `sym = x.keysym(code)`.
       - If `x.is_composing() || sym == keymap::XK_MULTI_KEY`:
         - Clear `a.repeat = None`.
         - If `sym == 0`: return `Vec::new()`.
         - Match `x.compose_feed(sym)`:
           - `ComposeOutcome::Composing | ComposeOutcome::Cancelled | ComposeOutcome::Idle` -> return `Vec::new()`.
           - `ComposeOutcome::Composed(Some(id))` -> let `mods = kb.locks_mask();` return `vec![Action::Send(Msg::KeyDown { id, mods, button: keymap::COMPOSED_BUTTON }), Action::Send(Msg::KeyUp { id, mods, button: keymap::COMPOSED_BUTTON })]`.
           - `ComposeOutcome::Composed(None)` -> return `Vec::new()`.
   - Change `forward::release(a: &mut Active, kb: &mut Keyboard) -> Vec<Msg>` to call `kb.reset_compose()`.
   - Update existing callers in `forward.rs` tests and `run.rs` (temporarily or in Task 4 — note `run.rs` must compile in this crate's `cargo test`, so pass `false` / `&mut self.kb` / handle `Action::ReleaseBack { .. } => self.release("back")` minimally in `run.rs` here if needed, then complete full wiring in Task 4).

- [ ] **Step 4: Run tests to verify they pass**

Run (from `src/thinfloe-menu`):
`flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock systemd-run --user --scope -q -p MemoryMax=6G -- env CARGO_BUILD_JOBS=4 cargo test -- --test-threads=4 deskflow::edge deskflow::forward`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/deskflow/keymap.rs src/thinfloe-menu/src/deskflow/edge.rs \
  src/thinfloe-menu/src/deskflow/forward.rs src/thinfloe-menu/src/deskflow/run.rs
git commit -m "feat(deskflow): intercept Multi_key compose and compute pointer re-entry"
```

---

### Task 4: Wayland cursor position hint and daemon wiring (`capture.rs`, `run.rs`, `mod.rs`)

**Files:**
- Modify: `src/thinfloe-menu/src/deskflow/capture.rs:469-518,879-883`
- Modify: `src/thinfloe-menu/src/deskflow/run.rs:710-762,823-890,1071-1114,1298-1744`
- Modify: `src/thinfloe-menu/src/deskflow/mod.rs:220-230`

**Interfaces:**
- Consumes (from Tasks 1, 3):
  - `DeviceCfg.compose`
  - `edge::reentry_point(out: Rect, side: Side, along: f64) -> ((f64, f64), (f64, f64))`
  - `Action::ReleaseBack { along: f64 }`, `Active::new(..., compose: bool)`, `forward::release(&mut Active, &mut Keyboard)`
- Produces:
  - `pub fn Capture::end_capture(&mut self, hint: Option<(f64, f64)>) -> bool`
  - `Daemon::uses_compose(&self, c: &Conn) -> bool`
  - `Daemon::release(&mut self, reason: &str)` and `Daemon::release_at(&mut self, reason: &str, back_along: Option<f64>)`

- [ ] **Step 1: Write failing unit tests in `src/thinfloe-menu/src/deskflow/run.rs`**

Add unit tests exercising `Daemon` directly (without Wayland) for:
1. `reentry_updates_hover_position_and_resets_resistance`:
   - Set up `Daemon` with one output `HEADLESS-1` (`0, 0, 1280, 720`), one `Strip` (`RightOf`, `dev: (1920, 1080)`), `hover` at `(1279.0, 100.0)` with non-zero push, and `Mode::Active` for that device.
   - Call `d.forward(vec![Action::ReleaseBack { along: 0.75 }])`.
   - Assert `matches!(d.mode, Mode::Idle)`.
   - Assert `d.hover.as_ref().unwrap().at == (1279.0, 540.0)`.
   - Assert inward motion `(-10.0, 0.0)` and sideways motion `(0.0, 50.0)` on `hover.push` do not trigger capture.
2. `compose_flag_flows_on_enter_and_live_reload_resets_compose`:
   - Configure device with `compose = true` in `d.cfg.devices`, enter device, assert `Active.compose == true`.
   - Start a compose sequence on `d.kb`, update `generated/deskflow` to `compose = false`, call `d.reload()`.
   - Assert `Active.compose == false` and `!d.kb.is_composing()`.

- [ ] **Step 2: Run tests to verify they fail**

Run (from `src/thinfloe-menu`):
`flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock systemd-run --user --scope -q -p MemoryMax=6G -- env CARGO_BUILD_JOBS=4 cargo test -- --test-threads=4 deskflow::run`
Expected: FAIL to compile / FAIL assertions.

- [ ] **Step 3: Implement cursor position hint in `capture.rs` and wiring in `run.rs` / `mod.rs`**

1. In `src/thinfloe-menu/src/deskflow/capture.rs`:
   - Update `Capture::end_capture(&mut self, hint: Option<(f64, f64)>) -> bool` and `State::end_capture(&mut self, hint: Option<(f64, f64)>) -> bool`.
   - Before `lock.destroy()`: if `let Some((sx, sy)) = hint` and `self.locked` and `let Some(st) = self.strips.iter().find(|x| x.spec.id == id)`, call `lock.set_cursor_position_hint(sx, sy); st.surface.commit();`.
   - Pass `None` in `remove_strip` and `zwp_locked_pointer_v1::Event::Unlocked`.
2. In `src/thinfloe-menu/src/deskflow/mod.rs`:
   - Pass `None` to `cap.end_capture(None)` in `probe_capture`.
3. In `src/thinfloe-menu/src/deskflow/run.rs`:
   - Add `fn uses_compose(&self, c: &Conn) -> bool` mirroring `shares_clipboard` (`d.compose`).
   - In `enter_device`: pass `self.uses_compose(&self.conns[i])` to `Active::new(...)`.
   - In `forward`: handle `Action::ReleaseBack { along } => self.release_at("back", Some(along))` and `Action::Release(why) => self.release(why)`.
   - Split `release(&mut self, reason: &str)` to delegate to `release_at(reason, None)`:
     - Before replacing `self.mode`, if `back_along` is `Some(along)` and `self.mode` is `Mode::Active(ref a)`, look up the strip (`s.spec.id == a.device as u32`) and compute `edge::reentry_point(s.out, s.side, along)` -> `(surface_hint, layout_at)`.
     - Call `forward::release(&mut a, &mut self.kb)` and send the release messages.
     - Call `c.end_capture(surface_hint)` (incrementing `self.own_unlocks` when true).
     - Update `self.hover`: if `let Some(h) = self.hover.as_mut()`, reset `h.push.reset()`, and if `let Some(at) = layout_at`, set `h.at = at`.
   - In `reload()`: after `self.cfg = cfg`, if `Mode::Active(ref mut a) = self.mode`, recompute `a.compose` from the matching connection's `uses_compose`; if `!a.compose`, call `self.kb.reset_compose()`.

- [ ] **Step 4: Run full menu crate test suite + clippy + fmt**

Run (from `src/thinfloe-menu`):
`flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock systemd-run --user --scope -q -p MemoryMax=6G -- env CARGO_BUILD_JOBS=4 bash -c 'cargo test -- --test-threads=4 && cargo clippy --all-targets -- -D warnings && cargo fmt --check'`
Expected: PASS with 0 warnings and clean formatting.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/deskflow/capture.rs src/thinfloe-menu/src/deskflow/run.rs \
  src/thinfloe-menu/src/deskflow/mod.rs
git commit -m "feat(deskflow): wire per-device compose and cursor position hint on re-entry"
```

---

### Task 5: Rootfs Compose guard, runtime verification (Compose + labwc 2A), and docs

**Files:**
- Modify: `test/integration/test-session-packages.sh`
- Modify: `test/runtime/test-labwc-runtime.sh`
- Modify: `docs/notes/deskflow.md`
- Modify: `CHANGES.md`

**Interfaces:**
- Verifies end-to-end against rebuilt image (`make image VERSION=deskflow-compose`, `make test`, `make test-runtime`).

- [ ] **Step 1: Add rootfs Compose table guard in `test/integration/test-session-packages.sh`**

Near the other X11/session `present` checks (around line 95, beside `libXrandr.so.2`), add:
```bash
# Deskflow per-device local Compose uses libxkbcommon's X11 Compose table.
present "X11 en_US.UTF-8 Compose table" 'usr/share/X11/locale/en_US\.UTF-8/Compose$'
present "X11 Compose directory index"   'usr/share/X11/locale/compose\.dir$'
present "X11 locale directory index"    'usr/share/X11/locale/locale\.dir$'
```

- [ ] **Step 2: Add Deskflow Compose + pointer re-entry runtime test in `test/runtime/test-labwc-runtime.sh`**

Add `run_deskflow_compose_reentry_case` (modeled on `run_deskflow_forward_case` / `run_deskflow_media_case`) running `labwc` on 1 output (`HEADLESS-1`, `1280x720`) with `XKB_DEFAULT_LAYOUT=de` in the composed environment (so Xwayland and `deskflow-core 1.26.0`'s X11 keymap can inject `adiaeresis` `ä` and `dead_acute`), `x11-probe.py --background ff0000 --buttons --hold 90` fullscreen on `HEADLESS-1` (or separate `--record` + `--buttons` probes), and stock `deskflow-core 1.26.0` client attached `right-of` `HEADLESS-1` with `edge_resistance = "low"`:

1. **Compose on (`compose = true`):**
   - Move pointer to `(1279, 100)`, push right onto client (`state active`), warm up with `wtype -s 300 x -s 500` twice.
   - Send `wtype -s 300 -k Multi_key -s 100 -k quotedbl -s 100 a -s 500`.
   - Assert RECORD sees `KEY adiaeresis` and `KEYUP adiaeresis`, and does NOT see `KEY Multi_key`.
   - Send `wtype -s 300 -k dead_acute -s 500`.
   - Assert RECORD sees `KEY dead_acute` (dead keys bypass local compose).
2. **Pointer re-entry (Approach 2A verification):**
   - While still active on the client (entered at `y = 100`), move pointer down `wlrctl pointer move 0 500` (client `y = 600`), then move left back across the edge (`wlrctl pointer move -100 0` until `released (back)`).
   - Verify no bounce: move `wlrctl pointer move 0 10` (sideways) and `wlrctl pointer move -100 0` (inward); assert device is still `connected` (not `active`).
   - Click left (`wlrctl pointer click left`): assert `x11-probe.py --buttons` reports `BUTTON <x> <y>` with `y` in `550..=670` (proving `labwc 0.9.3` honored `set_cursor_position_hint` at `~600` instead of staying at `~100`).
   - Move back onto the edge (`wlrctl pointer move 5000 0`, keeping `y ~ 610`), wait, push right onto client; assert client entry `MOTION 0 <y>` has `y` in `550..=670`.
3. **Compose off (`compose = false`):**
   - Reload config with `compose = false`, send `wtype -s 300 -k Multi_key -s 500`.
   - Assert RECORD sees `KEY Multi_key`.

**Decision gate for Approach 2A vs fallback 2B:** If step 2's click lands near `y = 100` (proving `labwc 0.9.3` ignores `zwp_locked_pointer_v1::set_cursor_position_hint` on layer-shell surfaces), STOP and switch to fallback 2B (`zwlr_virtual_pointer_v1` absolute warp in `capture.rs`) before proceeding.

- [ ] **Step 3: Build image and run integration + runtime tests**

Run (from worktree root):
`flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make image VERSION=deskflow-compose && flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test VERSION=deskflow-compose && flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test-runtime VERSION=deskflow-compose`
Expected: all unit, integration (including Compose table guards), and runtime tests (including `run_deskflow_compose_reentry_case`) PASS.

- [ ] **Step 4: Update `docs/notes/deskflow.md` and `CHANGES.md`**

Load the `writing-style` skill before editing prose:
- `docs/notes/deskflow.md`:
  - Under **Settings → Screen**, note the per-device `"Handle Compose key on ThinFloe"` switch (`compose = true/false`, default `false`, preserved on fingerprint re-attach).
  - Under **Capture and the way back**, document proportional pointer re-entry (`set_cursor_position_hint` + `hover.at` update) and local `Multi_key` Compose (with dead keys forwarded unchanged, and client-layout/BMP limits).
  - Under **Known limits**, keep existing limits accurate.
- `CHANGES.md`:
  - Under `## [Unreleased]`, add concise entries under `### New` (per-device `"Handle Compose key on ThinFloe"` switch in Settings → Screen) and `### Changed` (proportional pointer re-entry along the monitor edge).

- [ ] **Step 5: Verify no doc/test regressions and commit**

Run `bash -n test/integration/test-session-packages.sh test/runtime/test-labwc-runtime.sh && shellcheck test/integration/test-session-packages.sh test/runtime/test-labwc-runtime.sh`.
```bash
git add test/integration/test-session-packages.sh test/runtime/test-labwc-runtime.sh \
  docs/notes/deskflow.md CHANGES.md
git commit -m "test, docs(deskflow): verify Compose table, local Compose and pointer re-entry"
```
