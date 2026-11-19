# Volume and Brightness Keys with an On-Screen Display Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two switches in Settings → Keyboard let the volume keys act on the machine's sound and the brightness keys on the monitors (DDC/CI), also while ThinLinc or OxuLnk holds the keyboard. An on-screen display on every monitor shows the result, and a microphone mark stays up while the microphone is muted.

**Architecture:** A new labwc patch (`0006`) adds a keybind attribute `allowWhenInhibited="yes"`. Bindings that carry it bypass `view_inhibits_actions()`, and the composer writes it only on the six media keys. `thinfloe-menu --osd` is a long-running helper of `thinfloe-session`. It draws `zwlr_layer_shell_v1` overlay surfaces with egui offscreen and the menu's CPU raster into `wl_shm` buffers, runs `wpctl` and `ddcutil` on worker threads, and takes actions from `thinfloe-menu --osd-key <action>` over a Unix datagram socket. When a switch changes live, the composer rebuilds a stable rc.xml source (`.source/rc.xml`). `labwc-scale` stays the only runtime writer of the composed `rc.xml`; it notices the source change and applies it again.

**Tech Stack:** C (labwc 0.9.3 patch), Rust 1.96 (thinfloe-menu: egui 0.37, shadcn-egui 0.8, wayland-client 0.31, wayland-protocols-wlr 0.3), POSIX sh (composer, labwc-scale, session), bash tests, podman headless labwc runtime rig.

**Spec:** [docs/superpowers/specs/2026-11-18-media-keys-osd-design.md](../specs/2026-11-18-media-keys-osd-design.md). Mockups (not in the repo): `/home/oetiker/checkouts/thinfloe/.superpowers/brainstorm/1964557-1795014818/content/osd-states-v3.html` and `mic-mark-v4.html`; the values below were taken from them.

## Global Constraints

- Both switches **off by default**. Labels exactly "Volume keys control this machine" and "Brightness keys control the monitors". Below each, in `MUTED`: "When on, these keys no longer reach the ThinLinc session or any other program."
- Keys: volume switch `XF86AudioRaiseVolume`, `XF86AudioLowerVolume`, `XF86AudioMute`, `XF86AudioMicMute`; brightness switch `XF86MonBrightnessUp`, `XF86MonBrightnessDown`. No other keybinding, ever. With both switches off, the composed rc.xml has zero `<keybind>`.
- Only bindings with `allowWhenInhibited="yes"` pass a grab or inhibitor, and only the composer writes that attribute, only on the six keys above.
- Volume: `wpctl`, `@DEFAULT_AUDIO_SINK@`, 5 % steps, `-l 1.0` (cap 100 %); mute toggles the sink; mic mute toggles `@DEFAULT_AUDIO_SOURCE@`. Volume keys change the level and leave the mute state alone; the panel then shows "Muted" with the new level.
- Brightness: `ddcutil` CLI, VCP feature `10`. A step is 10 % of each monitor's own maximum, `round(max / 10)`, clamped to `0..=max`. The value is read once per display, then set absolutely. While a set runs, only the latest target is kept.
- OSD panel (logical px, multiplied by the scale in `$XDG_RUNTIME_DIR/thinfloe-scale`, 1 when missing or invalid): 320 wide; bottom centre, 22 from the bottom edge; padding 12 top and bottom, 16 left and right; icon 22; gap 10 between icon and name; text Geo 19; bar 6 high, 9 below the row; 1 px `LINE` border; `TILE` fill; square corners. Height with bar 61, without bar 46. Hidden 1500 ms after the last key.
- Panel states (icon, icon colour, name, right text, bar): Volume `VOLUME_2` ICE "Volume" "45 %" in `MUTED`, ICE fill on `LINE` track; Muted `VOLUME_X` `MUTED` "Muted" "45 %" in `MUTED`, fill `BAR_OFF` `#3a4550`; Mic off `MIC_OFF` `WARN` "Microphone off", no bar; Mic on `MIC` ICE "Microphone on", no bar; Brightness `SUN` ICE "Brightness" "70 %", ICE bar; unsupported `SUN` `MUTED` "Brightness" "not supported" in `WARN`, no bar, on that monitor only. The percent text is `format!("{p} %")`.
- Mic mark: 30 × 30, 14 from the right edge, 34 from the top, `TILE` fill, 1 px `WARN` border, `MIC_OFF` 18 in `WARN`. It is on every monitor while the default source is muted, whatever the switches say.
- OSD surfaces: layer `overlay`, `keyboard_interactivity` none, exclusive zone −1, an empty input region. One panel and one mark surface per `wl_output`, made and dropped on hotplug.
- External commands go through env overrides so tests can fake them: `TF_WPCTL` (default `wpctl`), `TF_DDCUTIL` (`ddcutil`), `TF_OSD` (`/usr/bin/thinfloe-menu`, in `thinfloe-session`), `TF_OSD_KEY_CMD` (`/usr/bin/thinfloe-menu --osd-key`, in the composer), `TF_OSD_SOCKET` (`$XDG_RUNTIME_DIR/thinfloe-osd.sock`).
- Menu rules: CPU drawing only (no `wgpu`, `glow`, `eframe`); UI colours from theme roles and sizes from named `brand.rs` constants (`ui_code_has_no_style_literals` covers the new UI files); `config.toml` is written only by `store.rs`; readers before the menu use `generated/`.
- Build and test limits: cargo only as `systemd-run --user --scope -q -p MemoryMax=4G -- env CARGO_BUILD_JOBS=4 cargo …` (tests with `-- --test-threads=4`). Image gates are run back to back under the shared lock, because `artifacts/` is shared across checkouts:
  `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock sh -c 'make image VERSION=mediakeys && make test VERSION=mediakeys && make test-runtime VERSION=mediakeys'`
- Image size budget 503 MB (`test-image-size.sh`); `ddcutil` must fit, and a raise needs a reason in the commit message.
- No polkit. Never run `dhclient`. Firmware is not touched.
- Every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

**Stop points (architect decision):** Task 1 (grab bypass) and Task 2 (overlay above a fullscreen window) carry the risk. If either gate fails in a way the task cannot fix, stop and report to the owner; do not start the next task.

## Review Focus

1. **A held key.** labwc repeats a matched keybinding at the keyboard repeat rate (rc.xml: 500 ms delay, 30/s; `start_keybind_repeat` in `src/input/keyboard.c`). Holding brightness must not leave `ddcutil` runs that keep changing the monitor after the key is let go; at most one set may be running and one pending. Holding a mute key must toggle once. Test in Task 4 (`brightness_keeps_only_the_latest_target_while_a_set_runs`), and `onRelease="yes"` on the two mute keys in Task 7.
2. **No sound device.** On a thin client without a sink or source, `wpctl` fails. The OSD shows no panel, logs the failure once, and keeps no mark. Test in Task 4 (`a_failing_wpctl_is_an_error_not_a_level`) and Task 5 (`a_failed_audio_event_shows_no_panel`).
3. **A monitor that hangs on DDC/CI** (a dock). A `ddcutil` run is killed after 10 s; the next press works, and the panel shows the target. Test in Task 4 (`a_hanging_ddcutil_is_killed_and_the_next_set_runs`).
4. **No OSD process** (crashed, or not started yet). `--osd-key` never blocks labwc: volume and mute act without a display, brightness logs and does nothing (no coalescing without the process), and the exit is 0. Test in Task 5 (`osd_key_without_a_server_runs_wpctl_itself`, `osd_key_without_a_server_leaves_brightness_alone`).
5. **Hotplug while something shows.** A monitor removed while the mark is up must drop only its surfaces; the process stays up, and the other monitor keeps the mark. Test in Task 8 (runtime: `wlr-randr --output HEADLESS-2 --off` while muted).

---

## Amendment 2026-11-19: every speaker and microphone in sync (Tasks 9 and 10)

Spec: the section "Amendment 2026-11-19: every speaker and microphone in sync" of the spec; it wins over the earlier text. The old Task 9 (docs) is now Task 11.

Global constraints added by the amendment (every later task includes them):
- All audio goes through `pactl` (env `TF_PACTL`, default `pactl`); the menu no longer calls `wpctl`, and `TF_WPCTL` is gone.
- Switch on: the volume keys set every sink to the shared level `L` (5 % steps, clamp 0..=100); mute toggles the shared `M` on every sink; mic mute toggles the shared `Q` on every source. Monitor sources (`monitor_of_sink` set) are never touched.
- Switch off: every sink and source at 100 % and unmuted, at OSD start and when the switch is turned off.
- A device that appears (`pactl subscribe`: `new`) gets the shared state (switch on) or 100 %/unmuted (switch off) at once. Source volume is otherwise not changed while the switch is on.
- The OSD never reads `L`, `M`, `Q` back from the current default device after start. Changes by others are not copied to other devices. The mark follows the default source's real mute state.
- The 1 s `wpctl` mic poll is gone; `pactl subscribe` drives the mark. A `pactl subscribe` that ends is restarted after 1 s, doubling to at most 30 s, logged once; after a restart the shared state is applied to every device again.

Review focus added:
1. **A USB headset** brings a sink and a source in one burst, and WirePlumber may make both the new defaults: the shared level must not jump to the headset's 100 %, the new source must be muted when `Q` is set. Test in Task 9 (`a_new_default_device_does_not_become_the_shared_level`).
2. **A device removed while a set runs:** `pactl set-sink-volume <gone>` fails; the other devices are still set, and the action is not reported as a failure of the whole key. Test in Task 9 (`a_failing_set_on_one_device_still_sets_the_others`).
3. **The OSD's own sets** produce `change` events: they must not cause more sets (no feedback loop). Test in Task 9 (`change_events_only_refresh_the_mark`).
4. **No `pipewire-pulse`** (no sound server, or it restarts): `pactl subscribe` exits at once; no busy loop, one log line, and the state is applied again once it answers. Test in Task 10 (runtime log, and `the_watch_backs_off_and_resyncs` in `audio.rs`).
5. **Switch turned off during a session:** every device back to 100 % unmuted without a restart. Test in Task 10 (`a_reload_to_off_resets_every_device`, and the runtime live-switch case).

## File Structure

- `containers/ubuntu/patches/0006-keybind-allow-when-inhibited.patch`: new labwc patch (Task 1).
- `src/thinfloe-menu/src/osd/`: new module, one responsibility per file:
  - `mod.rs`: `Action`, the `--osd` and `--osd-key` entry points, the main loop.
  - `socket.rs`: datagram socket, bind and send.
  - `state.rs`: pure state; what each output shows and when it hides.
  - `audio.rs`: `wpctl` runner, parser, audio worker.
  - `ddc.rs`: `ddcutil` runner, parsers, per-display setter.
  - `surface.rs`: Wayland: outputs, layer surfaces, shm buffers.
  - `draw.rs`: egui offscreen render of the panel and the mark into a `raster::Canvas`.
- `src/thinfloe-menu/src/brand.rs`: OSD sizes and the `BAR_OFF` role; `draw.rs` joins the style test.
- `src/thinfloe-menu/src/main.rs`: dispatch of `--osd`, `--osd-key`.
- `src/thinfloe-menu/src/{config,generated,keyboard,keyboardtab_ui,migrate}.rs`: the two settings.
- `overlay/etc/thinfloe/menu-managed-keys`: two keys.
- `overlay/usr/bin/thinfloe-compose-labwc-config`: `.source/` layout, keybind emission.
- `overlay/usr/libexec/thinfloe/labwc-scale`: re-applies when the source changes.
- `overlay/usr/bin/thinfloe-session`: starts and stops the OSD.
- `overlay/etc/modules-load.d/thinfloe-i2c.conf`: `i2c-dev`.
- `containers/ubuntu/Dockerfile`: `ddcutil`.
- `overlay/etc/thinfloe/labwc/rc.xml`: comment only.
- Tests: `test/runtime/test-labwc-runtime.sh`, `test/runtime/x11-probe.py`, `test/runtime/Dockerfile` (`wev`), `test/integration/test-labwc-config-merge.sh`, `test-labwc-scale.sh`, `test-session-lifecycle.sh`, `test-session-packages.sh`.
- Docs: `CLAUDE.md`, `docs/notes/history-of-constraints.md`, `docs/notes/session.md`, `docs/notes/menu.md`, new `docs/notes/media-keys.md`, `CHANGES.md`.

---

### Task 1: labwc patch 0006, bindings that pass a grab or inhibitor

**Files:**
- Create: `containers/ubuntu/patches/0006-keybind-allow-when-inhibited.patch`
- Modify: `test/runtime/x11-probe.py`, `test/runtime/test-labwc-runtime.sh` (new case at the end, before `exit $fail`), `test/runtime/Dockerfile` (add `wev`, used again in Task 8)
- Modify: `containers/ubuntu/Dockerfile` (comment block at the top: name 0006)

**Interfaces:**
- Produces: the rc.xml attribute `allowWhenInhibited="yes"` on `<keybind>` (labwc boolean, like `allowWhenLocked`). With it, the binding matches even while `view_inhibits_actions(active_view, …)` is true (X11 grab from patch 0002, inhibitor from 0004, user ToggleKeybinds). Without it, nothing changes.

The patch has the 0005 header style (Description, Author: ThinFloe maintainers, Origin: vendor, Forwarded: not-needed, Last-Update: 2026-11-18). It touches three files, none of them changed by 0001–0005, so it is diffed against the pristine source at `/scratch/oetiker/claude-tmp/labwc-src/labwc-0.9.3`. Copy that tree to `/scratch/oetiker/claude-tmp/labwc-0006/` and run `diff -u`; do not edit the pristine tree.
- `include/config/keybind.h`, `struct keybind`: `bool allow_when_inhibited;` after `allow_when_locked`.
- `src/config/rcxml.c`, `fill_keybind()`: `lab_xml_get_bool(node, "allowWhenInhibited", &keybind->allow_when_inhibited);` after the `allowWhenLocked` line.
- `src/input/keyboard.c`, `match_keybinding_for_sym()`: the inhibit check becomes `if (!keybind->allow_when_inhibited && view_inhibits_actions(server->active_view, &keybind->actions)) continue;`, with a `thinfloe:` comment saying the composer sets the attribute on the media keys only.

The description says why: ThinLinc (X11 grab) and OxuLnk (inhibitor) suspend every binding. The owner wants the volume and brightness keys to reach the machine anyway, and rc.xml stays the one place that says which keys pass.

- [ ] **Step 1: Extend `x11-probe.py`.** New option `--keys SECONDS`: after the window is mapped (and grabbed with `--grab`), it selects `KeyPressMask` and, for SECONDS, prints `KEY <keysym name>` for each KeyPress (`d.keycode_to_keysym(e.detail, 0)` then `XK.keysym_to_string`; for names it cannot map, print the number in hex). It then exits without the fullscreen step. The existing output stays the same without `--keys`.

- [ ] **Step 2: Write the failing runtime case `grab bypass`.** The container gets a config dir `/tmp/gb` with the shipped `themerc-override` and `environment` and a test `rc.xml` whose `<keyboard>` holds exactly:
  ```xml
  <keybind key="XF86AudioRaiseVolume" allowWhenInhibited="yes"><action name="Execute" command="sh -c 'echo pass >>/tmp/hits'" /></keybind>
  <keybind key="XF86AudioLowerVolume"><action name="Execute" command="sh -c 'echo plain >>/tmp/hits'" /></keybind>
  ```
  The session script starts `wtype -s 4000 -k XF86AudioRaiseVolume -s 300 -k XF86AudioLowerVolume &`, so a virtual keyboard exists before Xwayland starts (the old rig had no keyboard device at all). After 0.5 s it runs `python3 /usr/local/bin/x11-probe.py --grab --keys 7`, then prints `hits=$(tr '\n' ' ' </tmp/hits)`. labwc runs with `WAYLAND_DEBUG=1`.
  Assertions:
  - The grab was taken: the output matches `zwp_xwayland_keyboard_grab_manager_v1[@#][0-9]+\.grab_keyboard\(`. When it does not match, print `skip - the rig cannot drive an X11 keyboard grab; the grab bypass is an owner check on hardware` and skip the next two assertions (do not fail).
  - `hits=pass ` exactly: the attribute binding ran under the grab, and the plain one did not.
  - The probe printed `KEY XF86AudioLowerVolume` (the plain key reached the grabbing client) and no `KEY XF86AudioRaiseVolume`.
  - Control run, same rc.xml, probe without `--grab`: `hits=pass plain `. This proves the rig sends both keys, so the case cannot pass vacuously.

- [ ] **Step 3: Run it against the current image to see it fail.** Run the image gate line from Global Constraints once before writing the patch (the branch has only docs changes so far, so this image has the shipped labwc). Expected: FAIL on `hits=pass` (empty hits under the grab) when the grab is seen, or the skip line. Record which one: a skip means the inhibitor and grab paths are owner checks on hardware, and the commit message says so.

- [ ] **Step 4: Write the patch.** Then check that it applies on top of 0001–0005: copy the pristine tree again, then `for p in containers/ubuntu/patches/*.patch; do patch -p1 --dry-run <"$p" || exit 1; patch -p1 -s <"$p"; done`.

- [ ] **Step 5: Image gate.** Run `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock sh -c 'make image VERSION=mediakeys && make test VERSION=mediakeys && make test-runtime VERSION=mediakeys'`. Expected: `make test` all ok; runtime prints `ok` for the grab bypass case, or the skip line; the control run is `ok`; the #28 assertions (`labwc loads no default keybindings`, `Alt-Tab, Alt-Space and Super-a reach the focused window`) are still `ok`.
  **STOP** if the bypass assertion fails after the patch, and report to the owner.

- [ ] **Step 6: Commit**
  ```bash
  git add containers/ubuntu/patches/0006-keybind-allow-when-inhibited.patch containers/ubuntu/Dockerfile test/runtime/
  git commit -m "feat(labwc): keybindings marked allowWhenInhibited pass a keyboard grab"
  ```
  The message says whether the rig drove the grab, and that no rig can create a `zwp_keyboard_shortcuts_inhibitor_v1`. The OxuLnk path rests on the same code line and stays an owner check.

### Task 2: overlay surfaces above a fullscreen window (proof)

**Files:**
- Create: `src/thinfloe-menu/src/osd/mod.rs` (only `osd_main`'s `--probe` path for now), `src/thinfloe-menu/src/osd/surface.rs`
- Modify: `src/thinfloe-menu/src/main.rs`, `src/thinfloe-menu/src/lib.rs` (or wherever the modules are declared; follow the existing pattern), `test/runtime/x11-probe.py`, `test/runtime/test-labwc-runtime.sh`

**Interfaces:**
- Produces (`osd::surface`):
  ```rust
  pub struct Placement { pub anchor: Anchor, pub margin: (i32, i32, i32, i32) /* top right bottom left, px */, pub size: (u32, u32) /* px */ }
  pub enum SurfaceKind { Panel, Mark }
  pub struct Surfaces { /* wl connection, globals, outputs */ }
  impl Surfaces {
      pub fn connect() -> Result<Surfaces, String>;
      pub fn fd(&self) -> std::os::fd::BorrowedFd<'_>;          // for poll
      pub fn dispatch(&mut self) -> Result<OutputsChanged, String>; // read + dispatch pending, non-blocking
      pub fn outputs(&self) -> Vec<String>;                        // wl_output names, e.g. "HEADLESS-1"
      /// Show `pixels` (XRGB8888, row-major, width*height) for `kind` on `output`, creating the layer surface when needed.
      pub fn show(&mut self, output: &str, kind: SurfaceKind, place: &Placement, width: u32, pixels: &[u32]) -> Result<(), String>;
      pub fn hide(&mut self, output: &str, kind: SurfaceKind);
      pub fn flush(&mut self) -> Result<(), String>;
  }
  pub struct OutputsChanged(pub bool);
  ```
- Produces: `thinfloe-menu --osd --probe` maps a 320×61 panel in `TILE` with a 1 px `LINE` frame and a 30×30 mark in `TILE` with a `WARN` frame on every output, at the Global Constraints positions with scale 1. It holds them until SIGTERM, plain fills, no text. It stays as a test hook.

Surface rules: bind `wl_compositor` (v4), `wl_shm`, `zwlr_layer_shell_v1` (v4), and each `wl_output` (v4 for its `name` event). `get_layer_surface(surface, Some(output), Layer::Overlay, "thinfloe-osd")`, then `set_size`, `set_anchor` (panel `Bottom`, mark `Top | Right`), `set_margin`, `set_exclusive_zone(-1)`, `set_keyboard_interactivity(None)`, and `set_input_region(Some(&compositor.create_region()))` (empty region). Commit, wait for `configure`, `ack_configure`, then attach. Buffers: one `memfd_create` + `wl_shm_pool` + `wl_buffer` per draw, format `Xrgb8888`, destroyed on its `release`; `raster::Canvas::pixels` is already `0x00RRGGBB`. On `closed` or the output's `global_remove`, destroy that output's surfaces. An output whose name has not arrived yet gets nothing.

- [ ] **Step 1: Write the failing runtime case `overlay above fullscreen`.** Two outputs at 1280×720 (`WLR_HEADLESS_OUTPUTS=2`), shipped config (`LABWC_FULLSCREEN_SPAN_OUTPUTS=1`). `x11-probe.py` gets `--background RRGGBB` (window `background_pixel`) and `--hold SECONDS` (stay fullscreen that long after printing GEOMETRY), and `--buttons` (select `ButtonPressMask`, print `BUTTON x y` per press). Session script:
  `python3 x11-probe.py --background ff0000 --hold 12 --buttons &`; sleep 3; `wlrctl toplevel find app_id:thinfloe-probe state:active && echo focus_before=active`; `thinfloe-menu --osd --probe &`; sleep 2; `wlrctl toplevel find … state:active && echo focus_after=active`; `grim -t ppm /tmp/shot.ppm`; `wlrctl pointer move -5000 -5000; wlrctl pointer move 2531 49; wlrctl pointer click left` (the mark centre on output 2: x = 2560 − 14 − 15, y = 34 + 15); sleep 1; then a python snippet prints `px name=RRGGBB` for named points read from the PPM.
  Assertions:
  - GEOMETRY is `2560x720` (spanning; otherwise the rest says nothing about spanning).
  - The panel interior on both outputs is `0d1218`: points (640, 720−22−30) and (1920, 668). Its frame is `151b22`: (640−160, 668) is the left border of output 1.
  - The mark interior is `0d1218` and its border `c79a4a`: (1251, 49) and (2531, 49) interior; (1280−14−1, 49) border.
  - A control point (100, 100) is `ff0000`: the probe really is under the OSD.
  - `focus_before=active` and `focus_after=active`.
  - The probe printed `BUTTON 2531 49` (the spanning window starts at 0,0, so window and global coordinates agree): the click went through the mark.
  - A second run with a native fullscreen `foot -o colors.background=ff0000 --fullscreen sleep 12` on one output: the panel and mark pixels on that output are as above.

- [ ] **Step 2: Red.** The case needs `--osd --probe`, which does not exist yet; the Step 4 unit test is the red signal (`cargo test osd::surface` fails to compile).

- [ ] **Step 3: Implement `osd::surface` and `osd_main(args: &[OsString]) -> ExitCode` with `--probe` only**, plus the `main.rs` dispatch `if args.get(1).is_some_and(|a| a == osd::OSD_ARG) { return osd::osd_main(&args[2..]); }`. `pub const OSD_ARG: &str = "--osd";` `pub const OSD_KEY_ARG: &str = "--osd-key";`. `--osd` without `--probe` prints `thinfloe-menu --osd: not yet` and exits 2 (Task 5 replaces it). The main loop is `libc::poll` on the Wayland fd with a 1 s timeout; SIGTERM ends it with exit 0.

- [ ] **Step 4: Unit test the placement arithmetic.** In `surface.rs`:
  ```rust
  #[test] fn panel_and_mark_placement_follow_the_scale() {
      assert_eq!(panel_placement(1.0, true).size, (320, 61));
      assert_eq!(panel_placement(1.0, false).size, (320, 46));
      assert_eq!(panel_placement(2.0, true).size, (640, 122));
      assert_eq!(panel_placement(1.5, true).margin, (0, 0, 33, 0));
      assert_eq!(mark_placement(1.0).margin, (34, 14, 0, 0));
      assert_eq!(mark_placement(2.0).size, (60, 60));
  }
  ```
  Signatures: `pub fn panel_placement(scale: f32, bar: bool) -> Placement`, `pub fn mark_placement(scale: f32) -> Placement` (each value is `(v * scale).round()`). The constants live in `brand.rs` (see Task 3); add them here first: `OSD_W 320.0`, `OSD_BOTTOM 22.0`, `OSD_PAD_Y 12.0`, `OSD_PAD_X 16.0`, `OSD_ICON 22.0`, `OSD_GAP 10.0`, `OSD_TEXT_PT 19.0`, `OSD_BAR_H 6.0`, `OSD_BAR_GAP 9.0`, `MARK_SIDE 30.0`, `MARK_ICON 18.0`, `MARK_RIGHT 14.0`, `MARK_TOP 34.0`. Panel height = `2*OSD_PAD_Y + OSD_ICON (+ OSD_BAR_GAP + OSD_BAR_H)` + the 1 px border on each side counted inside: 12+22+12 = 46, + 9 + 6 = 61.

- [ ] **Step 5: Gates.** Run `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` (wrapped as above), then the image gate from Global Constraints. Expected: runtime `ok` for every assertion of Step 1.
  **STOP** if the overlay is below the fullscreen window, takes focus or eats the click, and report to the owner.

- [ ] **Step 6: Commit** `feat(menu): thinfloe-menu --osd maps overlay surfaces on every monitor`

### Task 3: drawing the panel and the mark

**Files:**
- Create: `src/thinfloe-menu/src/osd/draw.rs`
- Modify: `src/thinfloe-menu/src/brand.rs` (constants moved here from Task 2 if not already, `BAR_OFF`, style test list)

**Interfaces:**
- Consumes: `raster::{Canvas, Textures}`, `brand::{apply, geo, theme, role}`, `shadcn_egui::icons::lucide::{VOLUME_2, VOLUME_X, MIC, MIC_OFF, SUN}` with `IconView`, Task 2's `panel_placement`, `mark_placement`.
- Produces:
  ```rust
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub enum PanelView { Volume { percent: u8 }, Muted { percent: u8 }, MicOff, MicOn, Brightness { percent: u8 }, BrightnessUnsupported, BrightnessUnknown }
  impl PanelView { pub fn has_bar(&self) -> bool; }   // Volume, Muted, Brightness
  pub struct Painter { /* egui::Context with brand::apply, Textures */ }
  impl Painter {
      pub fn new() -> Painter;
      pub fn panel(&mut self, v: &PanelView, scale: f32) -> Canvas;
      pub fn mark(&mut self, scale: f32) -> Canvas;
  }
  ```
  `BrightnessUnknown` (detection not done yet): `SUN` ICE, "Brightness", no right text, no bar.
- `brand.rs`: `const BAR_OFF: Color32 = Color32::from_rgb(0x3a, 0x45, 0x50);`, `role::BAR_OFF: Role = Role::custom("tf-bar-off")` registered in `theme()`, and `pub(crate)` in the colour test module like the others.

Render: one persistent `egui::Context`. Each call runs `ctx.run` with `RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size_pt)), pixels_per_point: scale, .. }`. The content is laid out with `Area`/painter calls whose sizes come only from the `brand::OSD_*` constants and colours only from `theme().palette()` and roles. Then `tessellate`, `Textures::apply_set`, `Canvas::new(w, h, card)`, `Canvas::draw`, `Textures::apply_free`. Canvas size equals the Placement size.

- [ ] **Step 1: Write failing tests** in `draw.rs` (colours compared through `brand` role colours, not literals):
  ```rust
  #[test] fn volume_panel_has_the_size_and_an_ice_bar_to_the_level() {
      let c = Painter::new().panel(&PanelView::Volume { percent: 50 }, 1.0);
      assert_eq!((c.width, c.height), (320, 61));
      let bar_y = 61 - 1 - 12 - 3;               // middle of the 6 px bar
      assert_eq!(px(&c, 17 + 10, bar_y), ICE);    // inside the filled half
      assert_eq!(px(&c, 320 - 17 - 10, bar_y), LINE); // the empty track
      assert_eq!(px(&c, 0, 30), LINE);           // border
      assert_eq!(px(&c, 3, 3), TILE);            // fill
  }
  #[test] fn muted_bar_is_grey()  // fill pixel == BAR_OFF
  #[test] fn mic_off_has_no_bar_and_an_amber_icon() // height 46; some pixel inside the icon box (17..39, 12..34) is WARN, none is ICE
  #[test] fn unsupported_brightness_says_so_in_warn() // height 46; some pixel right of centre is WARN
  #[test] fn scale_two_doubles_the_panel() // (640, 122)
  #[test] fn mark_is_square_with_a_warn_border() // (30,30), px(0,15)==WARN, px(3,3)==TILE, some pixel in the centre 18×18 box is WARN
  #[test] fn percent_text_has_a_space_before_the_sign() { assert_eq!(percent_text(45), "45 %"); }
  ```
  `fn px(c: &Canvas, x: usize, y: usize) -> Color32` is a test helper.
- [ ] **Step 2: Run** `cargo test osd::draw` (wrapped). Expected: FAIL, missing items.
- [ ] **Step 3: Implement** `draw.rs` and the brand additions; add `("osd/draw.rs", include_str!("osd/draw.rs"))` to `ui_code_has_no_style_literals`. `--probe` (Task 2) now draws `Painter::panel(&PanelView::Volume { percent: 45 }, s)` and `Painter::mark(s)` instead of plain fills, so the Task 2 pixel points still hold: the interior points are away from text and icon.
- [ ] **Step 4: Run** `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`. Expected: PASS, the style test included.
- [ ] **Step 5: Commit** `feat(menu): draw the OSD panel and the microphone mark`

### Task 4: wpctl and ddcutil, with coalescing

**Files:**
- Create: `src/thinfloe-menu/src/osd/audio.rs`, `src/thinfloe-menu/src/osd/ddc.rs`

**Interfaces:**
- Produces (`osd::audio`):
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Level { pub percent: u8, pub muted: bool }
  pub fn parse_get_volume(out: &str) -> Option<Level>;
  pub struct Wpctl { cmd: Vec<String>, timeout: Duration }
  impl Wpctl {
      pub fn from_env() -> Wpctl;                                   // TF_WPCTL split on whitespace, default "wpctl"; timeout 3 s
      pub fn change(&self, delta_pct: i32) -> Result<Level, String>; // set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ {n}%+ | {n}%- ; then get-volume
      pub fn toggle_sink_mute(&self) -> Result<Level, String>;      // set-mute @DEFAULT_AUDIO_SINK@ toggle ; then get-volume
      pub fn toggle_source_mute(&self) -> Result<bool, String>;     // set-mute @DEFAULT_AUDIO_SOURCE@ toggle ; then source_muted
      pub fn source_muted(&self) -> Result<bool, String>;           // get-volume @DEFAULT_AUDIO_SOURCE@
  }
  pub enum AudioRequest { Step(i32), ToggleMute, ToggleMic }
  pub enum AudioEvent { Sink(Level), Mic(bool), Failed(String) }
  pub struct AudioWorker;
  impl AudioWorker {
      pub fn spawn(w: Wpctl, on_event: impl Fn(AudioEvent) + Send + 'static) -> AudioWorker;
      pub fn request(&self, r: AudioRequest); // never blocks
  }
  ```
  Coalescing: pending requests in a `Mutex<VecDeque>` + `Condvar`; a `Step` that follows a pending `Step` merges by adding the deltas (+5 per press); toggles are kept one by one.
- Produces (`osd::ddc`):
  ```rust
  #[derive(Clone, Debug, PartialEq, Eq)] pub struct DdcDisplay { pub bus: u32, pub connector: String, pub valid: bool }
  pub fn parse_detect(out: &str) -> Vec<DdcDisplay>;
  pub fn parse_getvcp(out: &str) -> Option<(u16, u16)>;          // (current, max)
  pub fn step(cur: u16, max: u16, up: bool) -> u16;
  pub fn percent(cur: u16, max: u16) -> u8;                       // round(cur*100/max), 0 when max == 0
  pub struct Ddcutil { cmd: Vec<String>, timeout: Duration }
  impl Ddcutil {
      pub fn from_env() -> Ddcutil;                               // TF_DDCUTIL, default "ddcutil"; timeout 10 s
      pub fn with_timeout(self, t: Duration) -> Ddcutil;          // tests
      pub fn detect(&self) -> Result<Vec<DdcDisplay>, String>;    // ddcutil detect --terse
      pub fn get(&self, bus: u32) -> Result<(u16, u16), String>;  // ddcutil --bus N --terse getvcp 10
      pub fn set(&self, bus: u32, v: u16) -> Result<(), String>;  // ddcutil --bus N --noverify setvcp 10 V
  }
  pub enum Bright { Unknown, Unsupported, Percent(u8) }
  pub enum DdcEvent { Detected(Vec<(String, Bright)>), Level { connector: String, percent: u8 } }
  pub struct Monitors;
  impl Monitors {
      pub fn spawn(d: Ddcutil, on_event: impl Fn(DdcEvent) + Send + Sync + 'static) -> Monitors;
      pub fn redetect(&self);      // detect off the caller's thread; then get for each valid display
      pub fn step(&self, up: bool); // every known valid display; Level events carry the new target at once
  }
  ```
  Per valid display, a setter thread with a single-slot target (`Mutex<Option<u16>>` + `Condvar`): `step` computes `step(target_or_current, max, up)`, stores it in the slot and emits `Level` with that percent. The setter takes the slot, runs `set`, and loops while a newer target is waiting. `step` before detection has finished does nothing. Commands run with the bounded child-process pattern of `jobs.rs`: stdout capped at 64 KiB, killed at the timeout, `default_signals`.

`parse_detect` reads blocks that start with `Display <n>` (valid) or `Invalid display` (invalid). In a block, `I2C bus:` gives `/dev/i2c-<bus>`, and `DRM connector:` or `DRM_connector:` gives `card<k>-<name>`; the connector is `<name>` with the `card<k>-` prefix removed. Blocks without a bus are skipped. **First check the real format** of the Ubuntu 26.04 `ddcutil`: run `podman run --rm ubuntu:26.04 sh -c 'apt-get update -q && apt-get install -yq ddcutil >/dev/null && ddcutil --version && man -P cat ddcutil 2>/dev/null | grep -i -A3 terse'`, or read the package's docs. Adjust the fixture below if the labels differ, and say so in the commit message.

- [ ] **Step 1: Write failing tests** (fake commands are shell scripts written into a `testdir::TestDir`; they log `$*` to a file):
  ```rust
  #[test] fn get_volume_parses_level_and_mute() {
      assert_eq!(parse_get_volume("Volume: 0.45\n"), Some(Level { percent: 45, muted: false }));
      assert_eq!(parse_get_volume("Volume: 0.45 [MUTED]\n"), Some(Level { percent: 45, muted: true }));
      assert_eq!(parse_get_volume("Volume: 1.00\n"), Some(Level { percent: 100, muted: false }));
      assert_eq!(parse_get_volume("Translate ID error\n"), None);
  }
  #[test] fn a_failing_wpctl_is_an_error_not_a_level()   // fake exits 1 → change(5) is Err
  #[test] fn change_runs_set_volume_with_the_cap_then_get_volume() // log == ["set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+", "get-volume @DEFAULT_AUDIO_SINK@"]
  #[test] fn steps_merge_while_wpctl_runs() // fake sleeps 0.3 s; 6 × request(Step(5)) within 50 ms → at most 2 set-volume lines, their sum 30%
  #[test] fn mic_toggle_targets_the_default_source() // log has "set-mute @DEFAULT_AUDIO_SOURCE@ toggle"
  #[test] fn detect_parses_connectors_and_invalid_displays() {
      let out = "Display 1\n   I2C bus:  /dev/i2c-6\n   DRM connector:           card1-DP-1\n   Monitor:   DEL:DELL U2720Q:ABC\n\nInvalid display\n   I2C bus:  /dev/i2c-7\n   DRM connector:           card1-HDMI-A-1\n   Monitor:   GSM:LG:XYZ\n";
      assert_eq!(parse_detect(out), vec![
          DdcDisplay { bus: 6, connector: "DP-1".into(), valid: true },
          DdcDisplay { bus: 7, connector: "HDMI-A-1".into(), valid: false }]);
  }
  #[test] fn getvcp_terse() { assert_eq!(parse_getvcp("VCP 10 C 60 100\n"), Some((60, 100))); assert_eq!(parse_getvcp("VCP 10 ERR\n"), None); }
  #[test] fn steps_are_a_tenth_of_the_maximum() {
      assert_eq!(step(60, 100, true), 70); assert_eq!(step(95, 100, true), 100);
      assert_eq!(step(5, 100, false), 0);  assert_eq!(step(100, 255, true), 126);
      assert_eq!(percent(126, 255), 49);   assert_eq!(percent(0, 0), 0);
  }
  #[test] fn brightness_keeps_only_the_latest_target_while_a_set_runs() // fake setvcp sleeps 0.3 s; current 50/100; 10 × step(true) within 100 ms → ≤ 2 setvcp lines, the last "… setvcp 10 100"; Level events end at 100
  #[test] fn a_hanging_ddcutil_is_killed_and_the_next_set_runs() // with_timeout(200 ms); fake setvcp sleeps 5 s on the first call only → the second step's set is logged within 1 s
  #[test] fn step_before_detection_does_nothing() // no setvcp in the log
  ```
- [ ] **Step 2: Run** `cargo test osd::audio osd::ddc` (wrapped). Expected: FAIL.
- [ ] **Step 3: Implement** both modules.
- [ ] **Step 4: Run** `cargo test`, `clippy`, `fmt`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(menu): volume through wpctl and brightness through ddcutil for the OSD`

### Task 5: the OSD process, the key client and the session

**Files:**
- Create: `src/thinfloe-menu/src/osd/socket.rs`, `src/thinfloe-menu/src/osd/state.rs`
- Modify: `src/thinfloe-menu/src/osd/mod.rs`, `src/thinfloe-menu/src/main.rs`, `overlay/usr/bin/thinfloe-session`, `test/integration/test-session-lifecycle.sh`

**Interfaces:**
- Consumes: Tasks 2–4 (`Surfaces`, `Painter`, `PanelView`, `AudioWorker`, `Monitors`, `Bright`).
- Produces:
  ```rust
  // osd/mod.rs
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum Action { VolumeUp, VolumeDown, VolumeMute, MicMute, BrightnessUp, BrightnessDown }
  impl Action { pub const ALL: [Action; 6]; pub fn parse(s: &str) -> Option<Action>; pub fn as_str(self) -> &'static str; }
  // names: volume-up volume-down volume-mute mic-mute brightness-up brightness-down
  pub fn socket_path() -> Option<PathBuf>;  // TF_OSD_SOCKET, else $XDG_RUNTIME_DIR/thinfloe-osd.sock, else None
  pub fn osd_main(args: &[OsString]) -> ExitCode;
  pub fn osd_key_main(args: &[OsString]) -> ExitCode;
  // osd/socket.rs
  pub enum SendError { NoServer, Io(std::io::Error) }
  pub fn bind(path: &Path) -> std::io::Result<UnixDatagram>;   // removes a stale file first; mode 0600; non-blocking
  pub fn send(path: &Path, a: Action) -> Result<(), SendError>; // one datagram "<name>\n"; ENOENT/ECONNREFUSED → NoServer; non-blocking
  pub fn decode(buf: &[u8]) -> Option<Action>;
  // osd/state.rs
  pub struct OsdState { /* panel: Option<(Shown, Instant)>, mic_muted, bright: BTreeMap<String, Bright> */ }
  pub enum Shown { Sink(Level), Mic(bool), Brightness }
  impl OsdState {
      pub const HIDE_AFTER: Duration = Duration::from_millis(1500);
      pub fn new() -> OsdState;
      pub fn show(&mut self, s: Shown, now: Instant);
      pub fn set_bright(&mut self, connector: &str, b: Bright);
      pub fn set_mic(&mut self, muted: bool) -> bool;          // true when changed
      pub fn panel_for(&self, output: &str, now: Instant) -> Option<PanelView>;
      pub fn mark(&self) -> bool;
      pub fn next_deadline(&self) -> Option<Instant>;
      pub fn expire(&mut self, now: Instant) -> bool;          // true when the panel went away
  }
  ```
  `panel_for`: `Sink(l)` → `Volume`/`Muted { percent }` on every output. `Mic(m)` → `MicOff`/`MicOn`. `Brightness` → per output: `Percent(p)` → `Brightness { percent: p }`, `Unsupported` → `BrightnessUnsupported`, missing or `Unknown` → `BrightnessUnknown`. Nothing once `now >= shown + HIDE_AFTER`.

Main loop (`osd_main` without `--probe`): `Surfaces::connect()` (on failure, log `thinfloe-menu --osd: <why>` and exit 1). Bind the socket (on failure, log and exit 1). Spawn `AudioWorker` and `Monitors`; their events go to an `mpsc` channel, and each one writes a byte to a `UnixStream::pair` wake end. `poll` waits on the Wayland fd, the socket, the wake fd, with the timeout `min(next_deadline, next mic poll)`. Mic poll: a thread of its own calls `Wpctl::source_muted` every 1 s and sends the answer as `AudioEvent::Mic` (an error counts as not muted). On an action: Volume* → `AudioWorker::request`; then `show(Sink(level))` when the `Sink` event arrives. Mic → `ToggleMic`, `show(Mic(m))` on the event, and `set_mic(m)`. Brightness* → `Monitors::step`, `show(Brightness)`. On `Level` → `set_bright`. On `AudioEvent::Failed(why)` → log once per distinct message, no panel. On `Detected` → `set_bright` for each; each connector reported `Unsupported` is logged once per process as `thinfloe-menu --osd: monitor <name> does not answer DDC/CI; the brightness keys leave it alone`. The same happens for an output that `ddcutil detect` does not list at all. After `OutputsChanged(true)`, wait 2 s, then `Monitors::redetect()`. After each change, redraw: for every output, `show`/`hide` the panel (`Painter::panel`, placement from `has_bar`) and the mark. The scale is read from `$XDG_RUNTIME_DIR/thinfloe-scale` (`TF_SCALE_FILE`) at each redraw: one number 0.5 to 8, the `labwc-scale` rule, else 1. SIGTERM: remove the socket file, exit 0.

`osd_key_main`: an unknown or missing action → stderr `thinfloe-menu --osd-key: unknown action '<x>'`, exit 2. `send` OK → exit 0. On `NoServer`: volume actions run `Wpctl::from_env()` directly (one call, no display) and log `thinfloe-menu --osd-key: no OSD running; <action> done without a display`. Brightness logs `… no OSD running; brightness not changed` and does nothing. Exit 0 in both cases. Another `Io` error → log, exit 0. It must not build the menu (no `build_menu`, no egui).

`thinfloe-session`: after the `labwc-scale` start (line ~368):
```sh
# The volume and brightness keys and the microphone mark: thinfloe-menu --osd
# (docs/notes/media-keys.md). Started always, the mark shows with the keys off.
TF_OSD="${TF_OSD:-/usr/bin/thinfloe-menu}"
if [ -x "$TF_OSD" ]; then
    "$TF_OSD" --osd >>"${TF_SESSION_LOG:-/dev/null}" 2>&1 &
    HELPER_PIDS="$HELPER_PIDS $!"
fi
```
No respawn. The menu's exit codes and the save behaviour are unchanged.

- [ ] **Step 1: Write failing tests:**
  ```rust
  #[test] fn actions_round_trip() { for a in Action::ALL { assert_eq!(Action::parse(a.as_str()), Some(a)); } assert_eq!(Action::parse("volume"), None); }
  #[test] fn a_datagram_reaches_the_bound_socket()   // bind in a TestDir, send(VolumeUp), recv → decode == Some(VolumeUp)
  #[test] fn send_without_a_server_is_no_server()     // missing path → NoServer; a stale file nobody binds → NoServer
  #[test] fn bind_replaces_a_stale_socket_file()
  #[test] fn the_panel_hides_1500_ms_after_the_last_key() // show at t0, show again at t0+1 s → panel_for at t0+2.4 s is Some, at t0+2.5 s None; expire(t0+2.5 s) == true
  #[test] fn muted_sink_shows_muted_with_the_level()  // Sink(Level{45,true}) → Muted{percent:45}
  #[test] fn brightness_is_per_output()                // DP-1 Percent(70), HDMI-A-1 Unsupported, DP-2 missing → Brightness{70}, BrightnessUnsupported, BrightnessUnknown
  #[test] fn the_mark_follows_the_source_mute()        // set_mic(true) → mark() and returns true; set_mic(true) again → false
  #[test] fn a_failed_audio_event_shows_no_panel()     // tested at the event handler: fn on_audio(&mut OsdState, AudioEvent, Instant, &mut Logged) leaves panel_for None
  #[test] fn osd_key_without_a_server_runs_wpctl_itself() // TF_OSD_SOCKET to a missing path, TF_WPCTL a logging fake; run the built binary (env!("CARGO_BIN_EXE_thinfloe-menu")) with --osd-key volume-up → exit 0, log has "set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+"
  #[test] fn osd_key_without_a_server_leaves_brightness_alone() // TF_DDCUTIL logging fake → log empty, exit 0, stderr has "brightness not changed"
  #[test] fn osd_key_rejects_an_unknown_action() // exit 2
  ```
  `test-session-lifecycle.sh`, new case: `TF_OSD` is a stub that writes `$*` and its pid to `$WORK/osd.args` and sleeps. After a normal logout (menu stub exit 0), assert `osd.args` holds `--osd`, that pid is gone (killed by the EXIT trap), and the session status and save behaviour of the existing exit-0 case hold. Second case: `TF_OSD=/nonexistent` changes nothing (session exits as before). The existing cases set `TF_OSD=/nonexistent` in the environment they share, so a host that has `/usr/bin/thinfloe-menu` does not start it.
- [ ] **Step 2: Run** `cargo test osd` and `bash test/integration/test-session-lifecycle.sh`. Expected: FAIL.
- [ ] **Step 3: Implement** `socket.rs`, `state.rs`, the main loop, `osd_key_main`, the `main.rs` dispatch for `--osd-key` (`return osd::osd_key_main(&args[2..]);` before `build_menu`), and the session lines.
- [ ] **Step 4: Run** `cargo test`, `clippy -- -D warnings`, `fmt --check`, `bash test/integration/test-session-lifecycle.sh`, and `shellcheck overlay/usr/bin/thinfloe-session` if the repo uses it (check `make test-unit`). Expected: PASS.
- [ ] **Step 5: Commit** `feat(menu): the OSD process takes key actions over a socket; the session starts it`

### Task 6: the two switches in Settings → Keyboard

**Files:**
- Modify: `src/thinfloe-menu/src/config.rs` (`Keyboard`, `DEFAULT_MANAGED`), `generated.rs` (`render` "environment"), `keyboard.rs` (`KbEvent`), `keyboardtab_ui.rs` (`pickers`), `migrate.rs:777` (struct literal), `overlay/etc/thinfloe/menu-managed-keys`

**Interfaces:**
- Produces: `Keyboard { …, pub volume_keys: bool, pub brightness_keys: bool }`, both `false` in `Default` (the struct already has `#[serde(default)]`, so an old `config.toml` loads). `KbEvent::VolumeKeys(bool)`, `KbEvent::BrightnessKeys(bool)`. `generated/environment` gains `THINFLOE_VOLUME_KEYS=on|off` and `THINFLOE_BRIGHTNESS_KEYS=on|off` after `THINFLOE_NUMLOCK`. `DEFAULT_MANAGED` has 7 keys, the shipped `menu-managed-keys` lists the same 7.
- The live path is unchanged: `jobs::keyboard_apply` → composer `--env-only` → `labwc --reconfigure`. The composer (Task 7) turns the values into keybindings. `KbOutcome::Live` gives no note for these switches (they act on reconfigure), unlike Num Lock.

- [ ] **Step 1: Write failing tests:**
  ```rust
  // config.rs
  #[test] fn media_key_switches_default_off_and_old_files_load() {
      let c: ThinfloeConfig = toml::from_str("[keyboard]\nlayout = \"de\"\n").unwrap();
      assert!(!c.keyboard.volume_keys && !c.keyboard.brightness_keys);
      assert!(DEFAULT_MANAGED.contains(&"THINFLOE_VOLUME_KEYS") && DEFAULT_MANAGED.contains(&"THINFLOE_BRIGHTNESS_KEYS"));
  }
  // generated.rs: update the existing exact-text test to end "…THINFLOE_NUMLOCK=on\nTHINFLOE_VOLUME_KEYS=off\nTHINFLOE_BRIGHTNESS_KEYS=off\n"
  #[test] fn media_key_switches_reach_the_environment() // volume_keys = true → contains "\nTHINFLOE_VOLUME_KEYS=on\n" and "\nTHINFLOE_BRIGHTNESS_KEYS=off\n"
  // keyboard.rs
  #[test] fn a_media_key_switch_applies_live_without_a_note() // event(VolumeKeys(true)) → Some(kb with volume_keys); done(&kb, Ok(Live)) → note None
  ```
  `managed_keys_come_from_the_file_or_the_default` already compares the shipped file with `DEFAULT_MANAGED`; it fails until the file has both lines.
- [ ] **Step 2: Run** `cargo test` (wrapped). Expected: FAIL.
- [ ] **Step 3: Implement.** In `pickers`, after the Num Lock row and before "Compose key": a row "Volume keys control this machine" with a `Switch`, then the explanatory line below it as `egui::Label::new(RichText::new(MEDIA_KEYS_NOTE).color(t.palette().muted_foreground)).wrap()`, indented to start in the control column (`ui.horizontal` with `ui.add_space(brand::FORM_LABEL_W)`). The brightness row and its line follow the same way. `pub const MEDIA_KEYS_NOTE: &str = "When on, these keys no longer reach the ThinLinc session or any other program.";` in `keyboard.rs`. Two lines in `menu-managed-keys`: `THINFLOE_VOLUME_KEYS`, `THINFLOE_BRIGHTNESS_KEYS`.
- [ ] **Step 4: Run** `cargo test`, `clippy`, `fmt`, and `make test-unit` (wrapped with the same `systemd-run … env CARGO_BUILD_JOBS=4`). Expected: PASS, the style test included.
- [ ] **Step 5: Commit** `feat(menu): switches for the volume and brightness keys on the Keyboard tab`

### Task 7: the composer emits the keys; labwc-scale follows a source change

**Files:**
- Modify: `overlay/usr/bin/thinfloe-compose-labwc-config`, `overlay/usr/libexec/thinfloe/labwc-scale`, `overlay/etc/thinfloe/labwc/rc.xml` (comment), `test/integration/test-labwc-config-merge.sh`, `test/integration/test-labwc-scale.sh`

**Interfaces:**
- Consumes: Task 6's `THINFLOE_VOLUME_KEYS` / `THINFLOE_BRIGHTNESS_KEYS` in the merged environment (values `on`, `"on"`, `'on'` count as on, the same quoting rule as `THINFLOE_NUMLOCK`); Task 1's `allowWhenInhibited`; Task 5's `--osd-key` actions.
- Produces, the composed directory layout (decided):
  - `rc.xml` → link to `$OUT_DIR/.source/rc.xml`, always, from the full compose on. This is the stable path `labwc-scale` records as its source at start.
  - `.source/base.xml` is written only by the full compose (session start). It is a link to `$SYSTEM_DIR/rc.xml`, or, with `THINFLOE_NUMLOCK=off`, a real copy that says `<numlock>off</numlock>` (today's `.numlock/rc.xml` logic, moved here; `.numlock/` is gone). So Num Lock still follows only at the next start.
  - `.source/rc.xml` is written by the full compose and by every `--env-only`. With both switches off it is a link to `$OUT_DIR/.source/base.xml`. Otherwise it is a real file: `base.xml` with the bindings inserted on the lines before `</keyboard>` (temp file in `.source/`, then `mv -f`). If it cannot be written, stderr says `thinfloe-compose-labwc-config: cannot write the key bindings; the volume and brightness keys stay off`, it becomes the link to `base.xml`, and the exit status is unchanged.
  - `--env-only` on a directory without `.source/base.xml` rewrites only `environment`, as before, and says `…: no .source/base.xml; the key switches follow at the next start` on stderr, exit 0.
  - Bindings, exact text; the command prefix is `${TF_OSD_KEY_CMD:-/usr/bin/thinfloe-menu --osd-key}`:
    ```xml
        <!-- thinfloe-compose-labwc-config: THINFLOE_VOLUME_KEYS=on -->
        <keybind key="XF86AudioRaiseVolume" allowWhenInhibited="yes"><action name="Execute" command="/usr/bin/thinfloe-menu --osd-key volume-up" /></keybind>
        <keybind key="XF86AudioLowerVolume" allowWhenInhibited="yes"><action name="Execute" command="/usr/bin/thinfloe-menu --osd-key volume-down" /></keybind>
        <keybind key="XF86AudioMute" allowWhenInhibited="yes" onRelease="yes"><action name="Execute" command="/usr/bin/thinfloe-menu --osd-key volume-mute" /></keybind>
        <keybind key="XF86AudioMicMute" allowWhenInhibited="yes" onRelease="yes"><action name="Execute" command="/usr/bin/thinfloe-menu --osd-key mic-mute" /></keybind>
        <!-- thinfloe-compose-labwc-config: THINFLOE_BRIGHTNESS_KEYS=on -->
        <keybind key="XF86MonBrightnessUp" allowWhenInhibited="yes"><action name="Execute" command="/usr/bin/thinfloe-menu --osd-key brightness-up" /></keybind>
        <keybind key="XF86MonBrightnessDown" allowWhenInhibited="yes"><action name="Execute" command="/usr/bin/thinfloe-menu --osd-key brightness-down" /></keybind>
    ```
    `onRelease="yes"` on the two mute keys makes a held mute key toggle once: labwc repeats a held binding, but an on-release binding runs only on the release.
- `labwc-scale`: `rc_link` is read at start as today, and now names `.source/rc.xml`. The poll compares `"$scale|$out|$(cksum <"$rc_link" 2>/dev/null)"` with the last state. A change of the source content is a change like a new scale, so `attempt` rebuilds the scaled copy from the new source and reconfigures. With the links in place (scale 1, no output), `handle` does nothing, and the menu's own `labwc --reconfigure` has already read the new source through the links. `labwc-scale` is still the only program that writes `$dir/rc.xml` once the session runs. The composer writes only `.source/`. Update the header comment to say this.

- [ ] **Step 1: Write failing tests** in `test-labwc-config-merge.sh` (keep the existing style, `ok`/`bad`):
  - Both off: `readlink "$OUT/rc.xml"` is `$OUT/.source/rc.xml`, `readlink -f "$OUT/rc.xml"` is the system rc.xml, and the file has no `<keybind`. Replaces "rc.xml should be a symlink into the system dir".
  - `THINFLOE_VOLUME_KEYS=on` in the generated env: exactly 4 `<keybind`, keys exactly the four volume keys, each with `allowWhenInhibited="yes"`, and the two mute keys with `onRelease="yes"`. They are inside `<keyboard>`. The file without the inserted lines and the comment is equal to the system rc.xml.
  - `THINFLOE_BRIGHTNESS_KEYS=on` alone: exactly the 2 brightness keys. Both on: 6, and `TF_OSD_KEY_CMD=/x/osd` makes every command start `/x/osd `.
  - Quoted `"on"` counts as on; `yes`, `1` and empty count as off.
  - Num Lock off and volume on: `.source/base.xml` is a real file with `<numlock>off</numlock>`; `.source/rc.xml` has it and the 4 keys. The existing Num Lock cases are rewritten to check `.source/base.xml` and `readlink -f` instead of `.numlock/`.
  - `--env-only` after switching volume on: `.source/rc.xml` now has the 4 keys, `rc.xml` is still the same link, and `.source/base.xml` is unchanged (also when the new env says Num Lock off: the existing "`--env-only` leaves rc.xml alone" becomes "`--env-only` leaves base.xml alone"). Switching off again makes `.source/rc.xml` the link to `base.xml` again.
  - `--env-only` with an unwritable `.source/` (chmod 500): `.source/rc.xml` unchanged, the stderr line, exit 0. Skip this when running as root.
  In `test-labwc-scale.sh`: a dir laid out as the composer does (`rc.xml` → `.source/rc.xml` → a file), scale 2, poll 0.2 s, a `TF_LABWC` stub that counts `--reconfigure`. Rewrite `.source/rc.xml` with an added `<keybind key="XF86AudioMute"…/>` (temp + mv). Within 1 s, `$dir/rc.xml` is a real file holding that keybind and `size="20"`, and the stub has one more reconfigure. Second case: at scale 1 with no output, a source change causes no reconfigure, and `rc.xml` stays the link.
- [ ] **Step 2: Run** `bash test/integration/test-labwc-config-merge.sh; bash test/integration/test-labwc-scale.sh`. Expected: FAIL on the new cases.
- [ ] **Step 3: Implement** the composer (`build_source_rc` used by both modes, `env_value` for the two keys, `awk` insertion before the `</keyboard>` line), `labwc-scale`, and the rc.xml comment. The comment now says: no keybindings in this file; the composer adds the media keys when a switch in Settings → Keyboard is on (CLAUDE.md, labwc has no keybindings unless a key switch is on).
- [ ] **Step 4: Run** both scripts, `bash test/integration/test-session-lifecycle.sh`, and `shellcheck` on the two changed scripts. Expected: all ok.
- [ ] **Step 5: Commit** `feat(session): the composer binds the media keys when a switch is on; labwc-scale follows`

### Task 8: ddcutil in the image and the end-to-end runtime test

**Files:**
- Modify: `containers/ubuntu/Dockerfile` (`ddcutil` in the session package `apt-get install` block, with a comment: brightness keys, DDC/CI)
- Create: `overlay/etc/modules-load.d/thinfloe-i2c.conf` (one line `i2c-dev`, with a comment line)
- Possibly create: `overlay/usr/lib/udev/rules.d/60-thinfloe-i2c.rules` (only when Step 1 finds no `uaccess` rule in the package)
- Modify: `test/integration/test-session-packages.sh` (package checks), `test/runtime/test-labwc-runtime.sh`, `test/integration/test-image-size.sh` (only when the budget is exceeded)

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: Find the package's udev rule.** `podman run --rm ubuntu:26.04 sh -c 'apt-get update -q && apt-get install -yq --no-install-recommends ddcutil >/dev/null && dpkg -L ddcutil | grep -i udev && cat $(dpkg -L ddcutil | grep rules.d/)'`. If a rule with `TAG+="uaccess"` for `i2c-dev` exists, use it. If not, ship `60-thinfloe-i2c.rules`: `SUBSYSTEM=="i2c-dev", KERNEL=="i2c-[0-9]*", TAG+="uaccess"`. Record which one in the commit message.
- [ ] **Step 2: Write failing image checks** in `test-session-packages.sh`: `usr/bin/ddcutil` present, and `ddcutil`'s `NEEDED` libraries are all in the image (follow the existing library check pattern for add-ons if there is one). `etc/modules-load.d/thinfloe-i2c.conf` contains `^i2c-dev$`. The kernel has i2c-dev: `kernel/drivers/i2c/i2c-dev.ko*` under `usr/lib/modules/*/`, or `i2c-dev` in `modules.builtin`. A rule file under `usr/lib/udev/rules.d/` has `uaccess` on a line that names `i2c`. `wpctl` is present.
- [ ] **Step 3: Write failing runtime cases** in `test-labwc-runtime.sh` (container env `TF_WPCTL=/tmp/fake-wpctl`, a script that logs `$*` to `/tmp/wpctl.log` and answers `get-volume @DEFAULT_AUDIO_SINK@` with `Volume: 0.45` and `get-volume @DEFAULT_AUDIO_SOURCE@` with the contents of `/tmp/mic`, default `Volume: 1.00`):
  - Switch off (generated env `THINFLOE_VOLUME_KEYS=off`, compose with the image's composer into `/tmp/xdg/thinfloe-labwc`, labwc `-C` that): a focused `wev -f wl_keyboard:key` shows `sym: XF86AudioRaiseVolume` after `wtype -k XF86AudioRaiseVolume`, and `/tmp/wpctl.log` has no `set-volume`. (The #28 cases stay as they are.)
  - Switch on: `wev` shows no `XF86AudioRaiseVolume` and does show a following `x`. `/tmp/wpctl.log` has `set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+`. With `thinfloe-menu --osd &` running and an X probe fullscreen with background `ff0000` (two outputs, spanning), a `grim -t ppm` taken 0.5 s after the key shows the panel interior `0d1218` at (640, 668) and (1920, 668). The ICE bar is at 45 %: pixel (480 + 17 + 50, 720 − 22 − 1 − 12 − 3) is `7cc4ff`. A shot 2.5 s after the key shows `ff0000` there.
  - Live switch: start with the switch off, then run the image's composer `--env-only` with `THINFLOE_VOLUME_KEYS=on` and `labwc --reconfigure`. The key no longer reaches `wev`.
  - Mic mark: `echo 'Volume: 1.00 [MUTED]' >/tmp/mic`; within 2 s, `grim` shows the mark border `c79a4a` at (1265, 49) and (2545, 49). Then `wlr-randr --output HEADLESS-2 --off`; after 1 s the OSD pid is alive (`kill -0`) and (1265, 49) still shows the mark. Then `echo 'Volume: 1.00' >/tmp/mic`; within 2 s (1265, 49) is `ff0000`.
  - Brightness without DDC (`TF_DDCUTIL` fake: `detect --terse` prints nothing, exit 1): with the brightness switch on, `wtype -k XF86MonBrightnessUp` puts "not supported" on both outputs: some pixel in the right half of the panel row is `c79a4a`. The OSD's stderr has `does not answer DDC/CI` once per output.
- [ ] **Step 4: Image gate.** Run the flock line from Global Constraints. Expected: all `ok`. Note the `test-image-size.sh` sizes and the delta against 493.0 MB in the commit message. Over 503 MB: raise `MAX_SUM` by the measured need, rounded up to a whole MB, and give the reason in the commit message.
- [ ] **Step 5: Commit** `feat(image): ddcutil and i2c-dev for the brightness keys; runtime test of the media keys`

### Task 8a: perl's module tree out of the image (owner, 2026-11-19)

`ddcutil` hard-depends on `i2c-tools`, which depends on `perl` only for its `decode-*` scripts; that brought about 9 MB and made Task 8 raise `MAX_SQSH` to 401 MB. The owner chose to cut it.

**Files:**
- Modify: `containers/ubuntu/slim/slim-rootfs.sh`, `test/integration/test-image-size.sh` (`MAX_SQSH` back to 399 MB), `test/integration/test-session-packages.sh` (guard)

- [ ] **Step 1: Find what perl brings and who uses it.** In the built rootfs (Task 8 image): `dpkg -L perl libperl5.* perl-modules-5.*` and their sizes; every file in the image with a `#!` line naming perl (`/usr/bin/perl`, `/usr/bin/env perl`), with the package it belongs to; what `perl-base` provides (it stays: it is Essential). Record the list in the report.
- [ ] **Step 2: Write the failing guard** in `test-session-packages.sh`: the image has no `usr/share/perl/5.*` tree and no `libperl.so*`, `usr/bin/perl` still runs `-e 'use strict; use warnings; print "ok"'` inside the rootfs container, and every perl-shebang file in the image is on an explicit list in the test (each line with the package and why it is dead or works with `perl-base` alone). An unknown perl script fails the test and names itself.
- [ ] **Step 3: Implement** the cut in `slim-rootfs.sh` next to the other cuts, deleting the `perl`, `libperl5.*` and `perl-modules-5.*` payload paths (not `perl-base`'s), with a comment: why (i2c-tools' decode scripts, ~9 MB), and that the guard lists the perl scripts left. The slim RUN stays the last in the Dockerfile. `MAX_SQSH` back to 399 MB, its comment updated.
- [ ] **Step 4: Image gate** (Global Constraints line). Expected: all `ok`; `ramroot.sqsh` back under 399 MB; note the sizes in the commit message.
- [ ] **Step 5: Commit** `feat(image): drop perl's module tree that i2c-tools pulled in`

### Task 9: the shared audio state over every device (`pactl`)

**Files:**
- Create: `src/thinfloe-menu/src/osd/sync.rs` (pure: the shared state and what to set)
- Modify: `src/thinfloe-menu/src/osd/audio.rs` (`Wpctl` → `Pactl`, JSON and event parsers, the worker owns `Sync`), `src/thinfloe-menu/src/osd/mod.rs` (`mod sync;`, `act_without_osd`), `src/thinfloe-menu/src/osd/run.rs` (only to compile against the new worker; the loop changes are Task 10), `src/thinfloe-menu/tests/osd_key.rs` (fake `pactl` instead of fake `wpctl`)

**Interfaces:**
- Produces in `audio.rs`:
  - `pub enum Kind { Sink, Source }`
  - `pub struct Device { pub kind: Kind, pub index: u32, pub name: String, pub percent: u8, pub muted: bool }`
  - `pub fn parse_devices(kind: Kind, json: &str) -> Result<Vec<Device>, String>`: the array `pactl -f json list sinks|sources` prints; `percent` is the rounded mean of the channels' `value_percent` (`"45%"`); a source whose `monitor_of_sink` is a sink name (not `"n/a"`, not null, not missing) is skipped.
  - `pub enum PaEvent { New(Kind, u32), Change(Kind, u32), Remove(Kind, u32), Server }` and `pub fn parse_event(line: &str) -> Option<PaEvent>` for `pactl subscribe` lines: `Event 'new' on sink #57`, `Event 'change' on source #12`, `Event 'remove' on sink #57`, `Event 'change' on server #…`/`Event 'change' on server`; every other facility (`sink-input`, `client`, `card`, …) is `None`.
  - `pub struct Pactl` with `from_env()` (`TF_PACTL`), `list(Kind) -> Result<Vec<Device>, String>`, `default_name(Kind) -> Result<String, String>` (`get-default-sink` / `get-default-source`, trimmed), `set_volume(Kind, &str, u8) -> Result<(), String>` (`set-sink-volume NAME 45%` / `set-source-volume`), `set_mute(Kind, &str, bool) -> Result<(), String>` (`set-sink-mute NAME 1|0` / `set-source-mute`), `subscribe_args() -> Vec<String>` (the argv for Task 10's watcher). Timeout per call stays `TIMEOUT` (3 s), through `run_tool`.
  - `AudioRequest`: `Step(i32)`, `ToggleMute`, `ToggleMic`, `Device(PaEvent)`, `Mode(Mode)`, `Resync`. Consecutive `Step`s merge (as now); consecutive `Device(Change…|Server)` merge into one mark refresh.
  - `AudioEvent`: `Sink(Level)` (the shared `L`/`M` after a key), `Mic(bool)` (the shared `Q` after the mic key), `Mark(bool)` (the default source's real mute after a refresh), `Failed(String)`. `Level` stays as it is.
  - `AudioWorker::spawn(p: Pactl, mode: Mode, on_event: impl Fn(AudioEvent) + Send + 'static) -> AudioWorker`; at start the worker runs `Sync::start` and sends `Mark` once.
- Produces in `sync.rs`:
  - `pub enum Mode { Keys, Off }`
  - `pub enum Set { Volume(Kind, String, u8), Mute(Kind, String, bool) }`
  - `pub struct Sync` (private fields `mode`, `level: u8`, `sink_muted: bool`, `mic_muted: bool`) with
    `pub fn start(mode: Mode, sinks: &[Device], sources: &[Device], default_sink: Option<&str>, default_source: Option<&str>) -> (Sync, Vec<Set>)`,
    `pub fn step(&mut self, delta: i32, sinks: &[Device]) -> Vec<Set>`,
    `pub fn toggle_mute(&mut self, sinks: &[Device]) -> Vec<Set>`,
    `pub fn toggle_mic(&mut self, sources: &[Device]) -> Vec<Set>`,
    `pub fn on_new(&self, d: &Device) -> Vec<Set>`,
    `pub fn set_mode(&mut self, mode: Mode, sinks: &[Device], sources: &[Device]) -> Vec<Set>`,
    `pub fn reapply(&self, sinks: &[Device], sources: &[Device]) -> Vec<Set>`,
    `pub fn level(&self) -> Level`, `pub fn mic_muted(&self) -> bool`.
  - Rules: a `Set` is produced only where the device's value differs. `start` with `Keys` takes `L`/`M` from the default sink (or 100/false when there is none) and `Q` from the default source (or false); with `Off`, `L = 100`, `M = Q = false`, and every sink and source gets volume 100 and unmute. Keys act in either mode (they are bound only when the switch is on); `Mode` decides `start`, `set_mode` and `on_new`. `on_new` in `Keys`: a sink gets `L` and `M`, a source gets `Q` (its volume untouched); in `Off`: volume 100 and unmute.

- [ ] **Step 1: Write failing tests.**
  - `sync.rs`: `start_takes_the_default_and_aligns_the_rest` (two sinks 40 %/100 %, default the first → `L = 40`, one `Set::Volume(Sink, second, 40)`); `off_resets_every_device_to_full_and_unmuted` (sinks 40 % muted, sources muted → volume 100 and unmute on all, no `Set` for a device already at 100 % unmuted); `step_moves_every_sink_from_the_shared_level` (`L = 40`, sinks at 40 and 70, `step(5)` → both to 45; `step(+5)` at 98 → 100; `step(-5)` at 3 → 0); `toggle_mic_sets_every_source` (two sources, one muted by someone else, `Q = false` → `toggle_mic` gives `Q = true` and a mute `Set` only for the unmuted one); `a_new_default_device_does_not_become_the_shared_level` (start with `L = 30`, then `on_new` for a headset sink at 100 % → `Set::Volume(Sink, headset, 30)`, `level().percent == 30`; a new source with `Q` set → `Set::Mute(Source, mic, true)`); `on_new_in_off_mode_gives_full_and_unmuted`; `set_mode_off_resets_and_back_to_keys_keeps_full` (after `Off`, `set_mode(Keys)` takes `L = 100`, `M = Q = false`, no `Set`).
  - `audio.rs`: `parse_devices_reads_name_index_percent_mute` (fixture: two sinks, stereo channels `"45%"`/`"47%"` → 46); `parse_devices_skips_monitor_sources` (a source with `monitor_of_sink: "alsa_output.x"` skipped, one with `"n/a"` kept); `parse_event_reads_new_change_remove_server` (the four line forms, plus `Event 'change' on sink-input #3` → `None`); `a_failing_set_on_one_device_still_sets_the_others` (fake `pactl` that fails `set-sink-volume gone 45%` and logs every call: a `Step` with sinks `gone` and `kept` still calls `set-sink-volume kept 45%`, the event is `Sink(Level { percent: 45, .. })`, and the failure is in the log, not an `AudioEvent::Failed`); `change_events_only_refresh_the_mark` (worker gets `Device(Change(Sink, 1))`, `Device(Change(Source, 2))`, `Device(Server)` in a burst → no `set-` call in the fake's log, one `get-default-source` and one `list sources`, one `Mark` event); `a_failing_pactl_is_an_error_not_a_level` (rename of the wpctl test: `list sinks` fails → `Failed`). Keep the existing `Step` merge test, adapted.
  - `tests/osd_key.rs`: `osd_key_without_a_server_runs_pactl_itself` (fake `pactl` with two sinks, default the first at 40 %: `--osd-key volume-up` → `set-sink-volume <both> 45%`, exit 0); keep `osd_key_without_a_server_leaves_brightness_alone`.
  The fake `pactl` in tests answers `-f json list sinks|sources` from files in its temp dir, `get-default-*` from a file, logs `$*`. Put fixtures in `src/thinfloe-menu/tests/fixtures/pactl-*.json` (also used by `audio.rs` unit tests through `include_str!`). Take the JSON shape from the real `pactl` in the image (Ubuntu 26.04 `pulseaudio-utils`): run `podman run --rm --entrypoint pactl localhost/<rootfs image>:mediakeys -f json list sinks` cannot list without a server, so start `pipewire`, `wireplumber` and `pipewire-pulse` in the rootfs container with a `null` sink (`pactl load-module module-null-sink`) and save that output; say in the report where the fixture came from.
- [ ] **Step 2: Run** `cargo test osd::` and `cargo test --test osd_key` (wrapped). Expected: FAIL (missing items).
- [ ] **Step 3: Implement** `sync.rs`, the `audio.rs` changes and `act_without_osd` (no process: list, take the default device's value, compute, set every device of that kind; brightness unchanged). Remove `Wpctl`, `parse_get_volume` and every `TF_WPCTL` use; the loop in `run.rs` keeps its mic poll for now but calls the default source through `Pactl` (Task 10 replaces it), and spawns the worker with `Mode::Keys` (Task 10 reads the switch).
- [ ] **Step 4: Run** `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `make test-unit` (all wrapped). Expected: PASS, output clean.
- [ ] **Step 5: Commit** `feat(menu): the audio keys set every speaker and microphone through pactl`

### Task 10: events instead of the mic poll; the switch reaches the OSD; runtime test

**Files:**
- Modify: `src/thinfloe-menu/src/osd/audio.rs` (the watcher), `src/thinfloe-menu/src/osd/run.rs` (watch thread, `Reload`), `src/thinfloe-menu/src/osd/mod.rs` (`Action::Reload`), `src/thinfloe-menu/src/jobs.rs` (`keyboard_apply` sends `reload`), `test/runtime/test-labwc-runtime.sh` (fake `pactl`; hotplug and reset cases)

**Interfaces:**
- Consumes: Task 9's `Pactl`, `PaEvent`, `AudioRequest`, `AudioEvent::Mark`, `Mode`.
- Produces: `Action::Reload` (`as_str` `"reload"`, accepted by `--osd-key` and the socket; not bound by the composer). `pub fn watch(p: &Pactl, on: impl Fn(Watch) + Send + 'static)` in `audio.rs`, run on its own thread: `pub enum Watch { Event(PaEvent), Restarted }`. `pub fn keys_mode(env_text: &str) -> Mode` in `sync.rs`: `THINFLOE_VOLUME_KEYS` with the composer's quoting rule (`on`, `"on"`, `'on'` → `Keys`; anything else or missing → `Off`; the last assignment wins).
- The watcher: `pactl subscribe` as a child (stdout piped, stderr to the OSD's stderr, its own process group like the other tools), each line through `parse_event`. On EOF or a spawn error: log `thinfloe-menu --osd: pactl subscribe ended; retrying in N s` once per distinct N, sleep (1, 2, 4, … capped at 30 s; back to 1 s after a run that lasted 30 s), start again, then send `Watch::Restarted` (the loop turns it into `AudioRequest::Resync`). The child is killed and reaped when the OSD stops (SIGTERM path), never left behind.
- The loop: `Watch::Event(e)` → `AudioRequest::Device(e)`; `AudioEvent::Mark(m)` → `state.set_mic(m)` without a panel; `Action::Reload` → read the generated environment again (the file `store.dir.generated_file("environment")` names; the OSD finds it the way the menu's `Paths` does) and send `AudioRequest::Mode(keys_mode(..))`. At start, the mode comes from the same file. The 1 s mic poll thread and `MIC_POLL` are removed.
- `keyboard_apply`: after a `Live` outcome, `osd::socket::send(&path, Action::Reload)`; `NoServer` is ignored, another error is logged to the menu's log; the outcome is unchanged.

- [ ] **Step 1: Write failing tests.**
  - `audio.rs`: `the_watch_backs_off_and_resyncs` (fake `pactl subscribe` that prints one `Event 'new' on source #3` line and exits: the callback sees `Event(New(Source, 3))`, then `Restarted`; with a fake that exits at once, the gaps between starts are 1 s then 2 s, within 200 ms; run with a shortened base delay through a `#[cfg(test)]` constructor, not by waiting 30 s).
  - `sync.rs`: `keys_mode_reads_the_switch` (`THINFLOE_VOLUME_KEYS=on`, `="on"`, `='on'` → `Keys`; `=off`, `=yes`, empty, missing → `Off`; `…=on\n…=off` → `Off`).
  - `run.rs`: `a_mark_event_sets_the_mark_without_a_panel`; `a_reload_to_off_resets_every_device` (through the worker with a fake `pactl`: start in `Keys` with a sink at 40 %, `Mode(Off)` → `set-sink-volume <sink> 100%` and the mutes cleared).
  - `jobs.rs`: `keyboard_apply_tells_the_osd_to_reload` (a bound test socket at `TF_OSD_SOCKET` receives `reload` after a live apply; without a socket the outcome is still `Live`). Keep socket paths short: the TestDir name for any socket test at most 12 characters (UNIX paths stop at 108 bytes, and a long `TMPDIR` plus a 7-digit PID gets close).
  - `test-labwc-runtime.sh`: replace the fake `wpctl` (`TF_WPCTL`, `/tmp/wpctl.log`) of Task 8's cases with a fake `pactl` (`TF_PACTL=/tmp/fake-pactl`): it logs `$*` to `/tmp/pactl.log`, answers `-f json list sinks|sources` from `/tmp/pa/sinks.json` / `sources.json` (two sinks at 45 %, two sources unmuted, plus one monitor source), `get-default-*` from `/tmp/pa/default-*`, and `subscribe` with `tail -n +1 -f /tmp/pa/events` (and it updates the JSON files on `set-*` calls, so reads see the new values). Task 8's expectations change from `set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+` to `set-sink-volume <sink1> 50%` and `set-sink-volume <sink2> 50%`; the mic mark cases now mute through the files plus an `Event 'change' on source #N` line instead of `/tmp/mic`. New cases: **hotplug while muted** (mic key → both sources muted; then add a third source, unmuted, and append `Event 'new' on source #N`: within 1 s `/tmp/pactl.log` has `set-source-mute <third> 1`, and the monitor source has no `set-` call at all); **live switch off** (switch on, a volume key, then the composer `--env-only` with `THINFLOE_VOLUME_KEYS=off`, `labwc --reconfigure` and `thinfloe-menu --osd-key reload`: within 1 s every sink and source gets `100%` and `0`).
- [ ] **Step 2: Run** the cargo tests (wrapped). Expected: FAIL.
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run** `cargo test`, `clippy`, `fmt`, `make test-unit` (wrapped), then the image gate line from Global Constraints. Expected: all `ok`, the new runtime lines included.
- [ ] **Step 5: Commit** `feat(menu): the OSD follows audio devices through pactl subscribe and the switch through reload`

### Task 11: docs and the keybinding guard

**Files:**
- Modify: `CLAUDE.md`, `docs/notes/history-of-constraints.md`, `docs/notes/session.md`, `docs/notes/menu.md`, `test/integration/test-session-packages.sh`, `CHANGES.md`
- Create: `docs/notes/media-keys.md`

- [ ] **Step 1: The guard.** In `test-session-packages.sh`, the existing "rc.xml registers no keybindings" stays: the system file has none. Add: run the image's composer (unsquashfs `usr/bin/thinfloe-compose-labwc-config`, `etc/thinfloe/labwc/`, `etc/thinfloe/menu-managed-keys`) with both switches off → the composed `rc.xml` has zero `<keybind`; both on → exactly 6. The key set is exactly the six names, every one with `allowWhenInhibited="yes"`, and no `allowWhenInhibited` anywhere else. Expected first run: PASS (Task 7 built it); confirm the check can fail by temporarily dropping one key from the composer in a scratch copy, not in the tree.
- [ ] **Step 2: CLAUDE.md.** Rename the constraint "labwc must have zero keybindings" to "labwc has no keybindings unless a key switch is on". Text: the system `rc.xml` has a `<keyboard>` section with no `<default />` and no `<keybind>`. The composer adds bindings only when a switch in Settings → Keyboard is on, and then exactly the six media keys with `allowWhenInhibited="yes"` (patch 0006), which pass a ThinLinc grab or an OxuLnk inhibitor; every other key reaches the focused program. Adding any other binding is a deliberate decision followed by `make test-runtime`. Guards: `test-session-packages.sh`, `test-labwc-config-merge.sh`, `test-labwc-runtime.sh`. Update its story link anchor. In "Where code lives", add `OSD: src/thinfloe-menu/src/osd/ (thinfloe-menu --osd, --osd-key)`. In "Rules that are easy to break → Menu", add one line: the OSD never takes focus or a click (layer overlay, empty input region, keyboard interactivity none), and only the composer writes `allowWhenInhibited`. In the labwc-rebuild constraint, name `0006` next to the other patches. Under "Notes", add `[media-keys.md](docs/notes/media-keys.md): the volume and brightness keys, the OSD and the microphone mark, DDC/CI, tests and owner checks.`
- [ ] **Step 3: history-of-constraints.md.** Rename the heading to match, and fix the anchor at the labwc-rebuild section (`#labwc-must-have-zero-keybindings`, about line 342). Append a paragraph: 2026-11-18, the owner wanted the keys back on the machine as an opt-in; patch 0006; the composer and `.source/` layout; whether the rig drove the X11 grab (from Task 1's result).
- [ ] **Step 4: session.md (Audio volume).** "the image has no volume control at the machine" becomes: with the volume switch on, the keys set the sink volume and mute through `wpctl`. WirePlumber stores them in `~/.local/state/wireplumber/`, they travel with the home, and they win over the 1.0 default at the next start. With the switch off, nothing is stored, as before. In the labwc config merge part, replace `.numlock/` with `.source/` (base.xml, rc.xml).
- [ ] **Step 5: menu.md (Keyboard tab).** Mention the two switches and their note; replace the `.numlock/` sentence with the `.source/base.xml` one.
- [ ] **Step 6: media-keys.md.** Sections: what the switches do; the keybinding path (composer, `allowWhenInhibited`, `onRelease` on the mute keys, labwc's key repeat); live switching (`.source/`, `labwc-scale` cksum); the OSD process (socket, actions, fallback without the process, scale file, surfaces per output, hotplug); audio (`wpctl` commands, mic poll 1 s, volume keys do not unmute); brightness (`ddcutil` commands, detection by connector, latest-target setter, 10 s timeout, the log line for a monitor without DDC/CI, i2c-dev and the udev rule); tests (which file covers what); owner checks on hardware (the three from the spec, plus the grab bypass if the rig skipped it, plus `uaccess` on the i2c device for tfuser). Note sizes and colours by their `brand.rs` names.
- [ ] **Step 7: CHANGES.md.** Under `[Unreleased]` → `### New`, first entry:
  > Settings → Keyboard has two new switches, both off by default: with them on, the volume keys (up, down, mute, microphone mute) change this machine's sound and the brightness keys change the brightness of every monitor that supports DDC/CI, also inside a fullscreen ThinLinc session. A panel at the bottom of each monitor shows the new level, and a microphone sign stays at the top right of every monitor while the microphone is muted. Volume and mute set with the keys are kept across restarts.

  In `### Fixed`, the #28 entry's last sentence becomes: "The volume and brightness keys therefore go to the focused program too, unless they are given back to the machine in Settings → Keyboard." Keep the entry's issue tag as it is.
- [ ] **Step 8: Gate.** `bash test/integration/test-session-packages.sh` against the Task 8 image (the image is not rebuilt; docs are no image input), `systemd-run … make test-unit`. Expected: all ok.
- [ ] **Step 9: Commit** `docs: the volume and brightness keys, the OSD and the keybinding rule`


#### Amendments from the audio change (2026-11-19)


The old Task 9's text below applies, with these changes from the audio amendment:
- Step 4 (session.md): "the keys set the sink volume and mute through `wpctl`" becomes "the keys set every speaker's volume and mute and every microphone's mute through `pactl`; with the switch off every speaker and microphone is set to 100 % and unmuted at session start and when the switch is turned off".
- Step 6 (media-keys.md, audio section): `pactl` commands, the shared state and why it is not read back from the default device, hotplug through `pactl subscribe` (back-off), `reload`, monitor sources left alone; no mic poll. Owner checks: two speakers and two microphones; a headset plugged in during a muted ThinLinc call is muted.
- Step 7 (CHANGES): the New entry's first sentence says the volume keys change "the volume of every speaker and the mute of every microphone of this machine"; add a sentence: "A speaker or microphone plugged in later takes the same setting, and with the switch off every speaker and microphone is kept at full volume and unmuted."

---

## Self-Review

- **Spec coverage:** switches and copy (T6); live change (T6, T7); keys through a grab or inhibitor (T1; the inhibitor is an owner check, see the T1 commit); volume kept across restarts (WirePlumber; T9 docs, T8 runtime log); 5 %/10 %/100 % cap (T4); OSD style B, every monitor, states 1–6 (T3, T5, T8); mic mark position and its following the real mute state, also with the switch off (T3, T5, T8); no focus, no click (T2); non-goals (no other bindings, guarded in T7 and T9); anti-goals: #28 regression (T1, T8 keep the #28 cases), held brightness (T4), crash isolation (separate process, T5 session, no respawn), stealing focus (T2); constraints: patches as Makefile prerequisites (the `PATCH_FILES` wildcard picks up 0006), CPU drawing (egui offscreen into `raster::Canvas`), style test (T3), `config.toml` via the store (T6), managed keys (T6), one runtime writer (T7), layer overlay (T2), size budget (T8), no polkit (`uaccess` udev, T8). The spec's "left to the builder" items are decided above: socket, fallback, surfaces, mic poll, DDC detection, labwc repeat (yes; `onRelease` for the mute keys), the log line, the tests, CHANGES.
- **Type consistency:** `PanelView` (T3) is used by `OsdState::panel_for` (T5); `Bright` (T4) by `OsdState::set_bright` (T5); `Level` (T4) in `Shown::Sink` (T5); `Action` names (T5) match the composer commands (T7); `panel_placement`/`mark_placement` (T2) are used by `Painter` (T3).
