# Volume and brightness keys with an on-screen display

Date: 2026-11-18. Status: design, approved approach 1 (owner).
Builds on #28 (`0005-no-default-keybinds.patch`: labwc loads no default
keybindings, so every key reaches the focused program).

## Intent

ThinFloe runs mostly on desktop thin clients with external monitors, no
laptops. Since #28 the volume and brightness keys of a keyboard (the owner's
Logitech has them) go to the focused program, usually a fullscreen ThinLinc
session, and change nothing on the machine. The owner wants a choice in
Settings → Keyboard: these keys act on the machine itself, and a display that
shows above everything, a fullscreen ThinLinc session included, tells the
user what happened. A muted microphone stays marked on screen for as long as
it is muted, so nobody talks into a muted call without knowing.

## Goals

- Two switches in Settings → Keyboard, both **off by default**:
  - "Volume keys control this machine": `XF86AudioRaiseVolume`,
    `XF86AudioLowerVolume`, `XF86AudioMute`, `XF86AudioMicMute`.
  - "Brightness keys control the monitors": `XF86MonBrightnessUp`,
    `XF86MonBrightnessDown`, through DDC/CI on every connected monitor that
    answers.
  - Under each switch, a line saying that these keys then no longer reach
    the ThinLinc session (or any other program).
- A change takes effect at once in the running session, like the other
  Keyboard settings.
- The keys work **also while a window holds the keyboard**: a fullscreen
  ThinLinc session (X11 `XGrabKeyboard`, patch `0002`) and OxuLnk
  (`zwp_keyboard_shortcuts_inhibit_v1`, patch `0004`). Both patches make
  `view_inhibits_actions()` suspend every keybinding today; a new labwc patch
  lets exactly these media-key bindings through (owner, 2026-11-18). Every
  other key still reaches the session.
- Volume, mute and mic mute set with the keys **are kept across restarts**
  (owner, 2026-11-18): WirePlumber stores them in
  `~/.local/state/wireplumber/`, which travels in the saved home and wins over
  the 1.0 default of `50-thinfloe-volume.conf`. The mark shows a mic that
  comes back muted.
- Volume: default audio sink, 5 % steps, capped at 100 %; mute toggles the
  sink; mic mute toggles the default source.
- Brightness: 10 % steps (of each monitor's own maximum), per monitor.
- On-screen display (OSD), style B of the mockups, on **every monitor**:
  panel bottom centre, 320 px wide (logical, before scale), menu tile colour
  behind, `LINE` border, square corners, Geo font, Lucide icon in ice blue,
  name left, percent right in `MUTED`, a 6 px bar below. Gone about 1.5 s
  after the last key press. States:
  1. Volume, `volume-2` icon, percent, ice bar.
  2. Muted, `volume-x` icon in `MUTED`, the level kept, bar grey.
  3. Microphone off, `mic-off` icon in `WARN` (amber), no bar.
  4. Microphone on, `mic` icon, no bar.
  5. Brightness, `sun` icon, that monitor's own percent and bar.
  6. Brightness on a monitor that does not answer DDC/CI: `sun` in `MUTED`,
     "not supported" in `WARN`, no bar, on that monitor only.
- Microphone mark: while the default source is muted, a 30 px square at the
  **top right** of every monitor (14 px from the right edge, 34 px from the
  top, so below a remote desktop's title strip): tile colour, 1 px `WARN`
  border, `mic-off` in `WARN`. It follows the real mute state of the source,
  whoever set it, and shows also when the volume switch is off.
- The OSD and the mark never take keyboard focus and never take a click
  (empty input region): a click on the corner reaches the session below.

## Non-goals

- Laptop backlight (`/sys/class/backlight`, `brightnessctl`, logind
  `SetBrightness`): no laptops in use.
- A brightness or volume slider in the menu, a status-area element for the
  mute state, media keys for play/pause/next.
- Any other keybinding. The rule stays: no keys bound unless one of the two
  switches is on, and then exactly the keys listed above.

## Anti-goals (a failure even if it works)

- With both switches off, any key fails to reach the focused program
  (regression of #28).
- The OSD or the mark steals focus, eats a click or a key, or sits below a
  fullscreen window.
- Holding a brightness key queues `ddcutil` runs that keep changing the
  brightness seconds after the key was let go.
- A crash of the OSD process takes the menu down, or the other way round, or
  keeps the session from ending (`thinfloe-session` exit codes unchanged).

## Constraints from the project

- CLAUDE.md "labwc must have zero keybindings" and its guards
  (`test-session-packages.sh`, `test/runtime/test-labwc-runtime.sh` with
  `wtype`) change to "no keybindings unless a switch is on; then exactly the
  listed keys". The runtime test keeps its #28 assertions with both switches
  off and gains: with the volume switch on, `XF86AudioRaiseVolume` does not
  reach `foot` and runs the OSD command; with it off, it reaches `foot`.
  Update the story in `docs/notes/history-of-constraints.md`.
- `config.toml` is written only by the menu (`store.rs`); readers before the
  menu use `generated/`. New `[keyboard]` fields `volume_keys` and
  `brightness_keys`, both `false` when missing (old files keep working).
- The menu reaches labwc through `generated/environment` and the composer.
  The two values become managed keys (`THINFLOE_VOLUME_KEYS`,
  `THINFLOE_BRIGHTNESS_KEYS`, `on`/`off`) in
  `overlay/etc/thinfloe/menu-managed-keys`, like `THINFLOE_NUMLOCK`.
- Keybindings live in `rc.xml`. Today the composer links `rc.xml` to the
  system file or to a `.numlock/` copy, and once the session runs
  `labwc-scale` is the only writer of the composed `rc.xml`, always from the
  source it saw when it started. **The builder must make a live change of a
  switch reach labwc** without breaking the scale or Num Lock handling and
  keep one runtime writer. (Unlike `<numlock>`, labwc applies keybindings on
  `--reconfigure`.) Guards: `test-labwc-config-merge.sh`,
  `test-labwc-scale.sh`, `test-labwc-runtime.sh`.
- labwc is a local rebuild with patches in `containers/ubuntu/patches/`; the
  grab bypass is a new patch there (a `Makefile` prerequisite through
  `PATCH_FILES`). Its form (a keybind attribute or a fixed action list) is the
  builder's call, but it must not let any binding other than the media keys
  past a grab or an inhibitor. Test it with an X11 client that calls
  `XGrabKeyboard` (`python3-xlib` is in the runtime test image) if the
  headless rig can drive the grab path; if not, say so, and it becomes an
  owner check on hardware.
- `docs/notes/session.md` (Audio volume) says the image never stores a
  volume; it changes to: stored once the keys set it.
- labwc 0.9.3 order (`src/output.c`, top to bottom): session lock, window
  switcher, menu, layer popups, **overlay**, top, views, bottom, background;
  `desktop_update_top_layer_visibility` hides only the top layer for a
  fullscreen view. So the OSD uses `zwlr_layer_shell_v1` layer **overlay**.
  The runtime test proves it above a fullscreen window (also one spanning
  outputs, `LABWC_FULLSCREEN_SPAN_OUTPUTS`).
- The menu draws on the CPU: no `wgpu`, `glow`, `eframe`. UI colours and text
  sizes from `brand.rs` (`ui_code_has_no_style_literals`).
- Machine limits for the build: 4 cores, `systemd-run ... MemoryMax`.
- Image size budget (`test-image-size.sh`, 503 MB, now about 493 MB):
  `ddcutil` and its dependencies must fit; a raise needs a reason.
- No polkit in the image.

## Approaches

1. **OSD process inside the menu binary, `thinfloe-menu --osd`** (chosen by
   the owner). `thinfloe-session` starts it next to the menu; a key runs
   `thinfloe-menu --osd-key <action>`, which hands the action to the running
   process. That process changes volume (`wpctl`) and brightness
   (`ddcutil`), draws the OSD and keeps the mark. Reuses the menu's CPU
   raster, `brand.rs`, the Geo font and the shipped Lucide icons, falls
   under the style test, one binary, one owner of the state; held keys are
   collapsed in one place. A crash of either process leaves the other.
2. Separate crate `thinfloe-osd`: independent, but copies brand, fonts and
   icons outside the style test and adds a second Rust build.
3. Shell scripts bound in labwc plus a draw-only program (wob-like):
   simplest parts, but held brightness keys queue slow `ddcutil` runs and the
   mute mark needs a second watcher.

## Decided

- Approach 1; OSD style B; mono Lucide icons in the menu's colours; mark top
  right, kept while muted; two switches, off by default; DDC/CI only for
  brightness; the keys pass a ThinLinc or OxuLnk keyboard grab (new labwc
  patch); volume and mute states are kept across restarts.
- My calls (owner may change): OSD and mark on every monitor; 5 % / 10 %
  steps; 1.5 s; volume cap 100 %; mark shown regardless of the volume
  switch.

## Left to the builder

- How `--osd-key` reaches the OSD process (a socket in `$XDG_RUNTIME_DIR` is
  the obvious way), and what it does when no OSD process runs (act without a
  display and log is acceptable).
- The layer-shell surfaces: one per output, created and dropped on output
  hotplug; the OSD's size follows the menu's scale file
  `$XDG_RUNTIME_DIR/thinfloe-scale`.
- Watching the source mute state for the mark (poll `wpctl` about once a
  second, or PipeWire events).
- DDC/CI: `ddcutil` CLI, display found once per output by its DRM connector
  name (it matches the Wayland output name), detection off the drawing
  thread and again on hotplug; read the value once, then set absolute
  values; while a set runs, only the latest target is kept. `i2c-dev`
  loaded at boot (`modules-load.d`); access for `tfuser` (the `ddcutil`
  package's udev rule with `uaccess`, to be checked on the image).
- Whether labwc repeats a held keybinding (if not, one step per press).
- Where a monitor that does not answer DDC/CI is logged once per session so
  support finds it.
- Tests: the runtime test above (grim pixel checks of the overlay above a
  fullscreen window, mark position, click-through); unit tests for the
  rc.xml composition, config defaults and migration, the OSD state machine
  (coalescing, timeout, mute states) with fake `wpctl` and `ddcutil`.
- CHANGES.md: the #28 entry says volume and brightness keys now go to the
  focused program; the new entry under New says they can be switched to
  control the machine.

## Owner checks on hardware

- Brightness on real monitors through DDC/CI (QEMU has no DDC), also
  through the docks in use.
- Mic mute with a ThinLinc call: the remote side hears nothing.
- Volume keys inside a fullscreen ThinLinc session (the grab bypass), and
  that Alt-Tab still goes to ThinLinc there.
