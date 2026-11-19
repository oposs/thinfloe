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

## Amendment 2026-11-19: every speaker and microphone in sync

Added to `docs/superpowers/specs/2026-11-18-media-keys-osd-design.md` after
Tasks 1-7 were built. Where this section and the text above disagree, this
section wins.

### Why

A thin client often has more than one speaker and more than one microphone:
the monitor's speakers, a USB headset, a webcam microphone. The keys as first
designed change only the default sink and the default source. The other
devices stay where they were, and a microphone that is not the default stays
live while the mark says the microphone is off (owner, 2026-11-19).

### What the user gets

- **Volume switch on.** The volume keys set **every** speaker (sink) to the
  same level; mute mutes or unmutes every speaker; mic mute mutes or unmutes
  every microphone (source). Monitor sources (`*.monitor`, the copy of a
  sink's output) are not microphones and are left alone.
- **Volume switch off.** Every speaker and every microphone is at 100 % and
  unmuted: at session start, and again when the switch is turned off. The
  remote client (ThinLinc, OxuLnk) sets the loudness, as before this branch.
  This also clears a microphone left muted from an earlier session, which no
  key could unmute while the switch is off.
- **Hotplug** (owner's choice, 2026-11-19). A speaker or microphone plugged in
  during the session joins the current state at once: with the switch on, a
  new speaker gets the shared level and mute state, a new microphone the
  shared mic mute state; with the switch off, the new device gets 100 % and
  unmuted. A microphone plugged in during a muted call is muted before anyone
  can hear it (within the time PipeWire takes to report it, well under 1 s).
- The panel shows the shared level. The mark still follows the **default**
  source's real mute state, whoever set it (the source a remote client
  records from), and still shows when the switch is off.
- The levels set with the keys are still kept across restarts: WirePlumber
  stores each device's level, so all of them come back at the shared level.

### Approaches

1. **All audio through `pactl`** (chosen; owner left the method to the
   controller). One long-running `pactl subscribe` child of the OSD process
   reports new devices and changes at once; `pactl -f json list sinks` and
   `… sources` list the devices; `pactl set-sink-volume`, `set-sink-mute`,
   `set-source-volume` and `set-source-mute` set them by name. One tool,
   machine-readable output, events instead of a poll. `pactl` is in the
   image (`pulseaudio-utils`), it talks to `pipewire-pulse`, and WirePlumber
   stores what it sets just as it stores `wpctl` sets.
2. `wpctl` to set, `pactl subscribe` only as a trigger: two tools with two
   different device numberings; `wpctl` has no machine-readable listing
   (`wpctl status` is for people). More fragile than 1.
3. A WirePlumber Lua policy script: instant and independent of the OSD
   process, but the rule is split between Lua and Rust, it cannot be tested
   with fakes like the rest, and WirePlumber 0.5's Lua API moves.

Dropped: a 1 s poll of `wpctl status` (text for people; a new microphone is
live for up to 1 s).

### Decided

- Approach 1. `TF_WPCTL` is replaced by `TF_PACTL` (default `pactl`).
  `wpctl` is no longer called by the menu.
- The OSD process owns the **shared state**: speaker level `L` (percent),
  speaker mute `M`, microphone mute `Q`. It does not read the shared state
  again from whichever device is the default at a given moment: a headset
  plugged in often becomes the new default, and its 100 % must not become the
  shared level.
  - Start, switch on: `L` and `M` from the default sink, `Q` from the default
    source; then every sink gets `L`/`M` and every source gets `Q` (this
    brings devices that drifted apart back in line).
  - Start, or switch turned off: `L = 100`, `M = Q = false`; every sink and
    source gets volume 100 % and unmute.
  - Volume key (switch on): `L` = clamp(`L` ± 5, 0, 100), set on every sink;
    mute state untouched (unchanged rule: the panel shows "Muted" with the
    new level when `M` is set).
  - Mute key: `M = !M`, set on every sink. Mic mute key: `Q = !Q`, set on
    every source.
  - A new sink (`new` event): gets `L`/`M` (switch on) or 100 %/unmuted
    (switch off). A new source: gets `Q` (switch on) or 100 %/unmuted
    (switch off). Source volume is otherwise never changed while the switch
    is on.
- Changes made by others (the remote client setting the default sink, a
  user's `pactl`) are not copied to the other devices; the next key press
  sets every device from the shared state again. The OSD's own sets produce
  `change` events; only `new` events and the default source's mute drive
  actions, so there is no feedback loop.
- The mark is refreshed from the default source on `change` events of a
  source and of the server (the default device changed), coalesced. The 1 s
  `wpctl` mic poll is gone.
- The OSD learns the switch from `THINFLOE_VOLUME_KEYS` in
  `~/.config/thinfloe/generated/environment`: read at start, and read again
  when the menu, after a keyboard apply, sends the OSD a `reload` datagram
  on the existing socket.
- `--osd-key` without an OSD process (crashed, not started) does the same
  for all devices itself: it lists the sinks or sources and sets each,
  starting from the default device's current value, since it has no shared
  state. Brightness still does nothing without the process.
- `pactl subscribe` ends (pipewire-pulse restarted, `pactl` missing): the OSD
  logs it once, restarts the child after 1 s, doubling to at most 30 s, and
  on reconnect applies the shared state to every device again.
- No sound device at all: as before, no panel, one log line, no mark.

### Left to the builder

- The exact `pactl -f json` fields (names, `monitor_source`/`monitor_of_sink`,
  per-channel volume) as `pulseaudio-utils` in Ubuntu 26.04 prints them:
  check on the built image and pin them in a fixture.
- Whether `pactl`'s percent and WirePlumber's stored volume agree (both
  cubic); the 100 % default of `50-thinfloe-volume.conf` must read back as
  100 %.
- Coalescing of event bursts (a USB headset brings a sink and a source at
  once), and the restart back-off timing.

### Impact on the plan

- New Task 8b after Task 8: `audio.rs` (wpctl runner → pactl runner, JSON
  parser, subscribe child), `run.rs` (shared state, events instead of the mic
  poll, `reload`), `mod.rs`/`--osd-key` fallback, `jobs::keyboard_apply`
  sends `reload`, Task 8's runtime cases switch from a fake `wpctl` to a fake
  `pactl` with two sinks and two sources, plus a hotplug case (a third
  source appears while muted and is muted).
- Task 9 (docs, CHANGES): `docs/notes/session.md` (Audio volume) and the
  CHANGES entry say "every speaker and microphone".

### Owner checks on hardware

- Two speakers and two microphones (e.g. monitor speakers plus a USB headset):
  keys move both; a headset plugged in during a muted ThinLinc call is muted.
