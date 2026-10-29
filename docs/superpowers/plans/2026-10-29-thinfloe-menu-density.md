# ThinFloe menu density Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The menu, the terminal and the labwc title bars size themselves by
the real density of the screen (1 point = 1 px of a 24" 1080p), with tiles
fixed at 220 x 76 points.

**Architecture:** A pure module `density.rs` turns an output's mode and
physical size into one scale `s`; `outputs.rs` reads `wl_output` on its own
Wayland connection; `app.rs` applies `s` as egui's zoom factor, renders icons
and logo at `s`, and writes `s` to `$XDG_RUNTIME_DIR/thinfloe-scale`.
`thinfloe-terminal` and a new helper `labwc-scale` read that file. All layout
constants in `ui.rs` stay in points; with the zoom they become pixels.

**Tech Stack:** Rust 1.96 (egui 0.36, egui-winit/winit 0.30, wayland-client
0.31), POSIX sh, labwc 0.9.3.

**Spec:** `docs/superpowers/specs/2026-10-29-thinfloe-menu-density-design.md`

## Global Constraints

- Tile 220 x 76 points, 4 columns, `TILE_GAP` 3; group width 889 points; `TILE_H_MIN` 64 stays.
- `s = ppi / 92`, ppi = mode width px / (physical width mm / 25.4).
- Fallback (no usable size, no file): `s = 2` when the shorter side of the mode is >= 1800 px, else `s = 1`.
- Untrustworthy size: either side 0; aspect of mm vs aspect of px differs by more than 10 %; ppi outside 50..800; **physical width under 200 mm** (see note below).
- Never below 1 from density alone; round to nearest 0.25; fit: largest 0.25 step with `889 * s <= 0.9 * window_width_px`, never below 0.5.
- labwc outputs stay at scale 1. No GPU crates. Zero labwc keybindings; `/etc/thinfloe/labwc/rc.xml` is never edited at runtime.
- Icons are re-rendered at `render_side(ppp)` for a new scale, never stretched.
- Scale file: `$XDG_RUNTIME_DIR/thinfloe-scale`, one number and a newline, written atomically (temp file + rename).
- Terminal font `14 * round(s)` px (at least 14), padding `12 * round(s)`.
- Title bar font `round(10 * s)` points; when anything fails, the title bars keep size 10.
- Machine: at most 4 cores (`cargo test -j4`, `CARGO_BUILD_JOBS=4`), podman, `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests`, image builds under `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=handover …`, no QEMU during a build.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

**Note, deviation from the spec (owner to confirm):** the spec says a TV that
reports 160 x 90 mm falls back through the aspect check. It does not: 160 x 90
has the 16:9 aspect of a 4K mode, and 3840 px / 160 mm is 609 ppi, inside
50..800, so `s` would be 6.5. The added rule "physical width under 200 mm is
not used" catches it (the smallest laptop screen in use, 11.6", is 256 mm
wide).

## Review Focus

1. A monitor or VM that reports 0 x 0 mm or an EDID aspect stored as cm (160 x 90): the menu must look as it does today (fallback), not tiny or huge. Tests in Task 1.
2. A monitor rotated by 90° (`wlr-randr --transform 90`): mode and mm are native, the window is narrow; `s` must come from the native size and the fit from the window width. Test in Task 1.
3. `XDG_RUNTIME_DIR` unset or the scale file unwritable, or a scale file with garbage (`abc`, `0`, `100`): the menu still draws; the terminal and the title bars use the fallback / size 10. Tests in Tasks 1, 7, 8.
4. Two monitors, menu placed on the second one: the first frame may use a guess, then the menu lays out once more at the scale of its real monitor. Test of `choose_output` in Task 1.
5. A long source address or a long failure message in a Settings row on the 889-point panel: the text is cut with "…" and the button stays fully visible. Test in Task 6.

---

### Task 1: The density rule (`density.rs`)

**Files:**
- Create: `src/thinfloe-menu/src/density.rs`
- Modify: `src/thinfloe-menu/src/lib.rs` (add `pub mod density;`)

**Interfaces:**
- Produces:
  - `pub struct OutputInfo { pub name: Option<String>, pub mode_px: (u32, u32), pub phys_mm: (u32, u32) }` (derive `Clone, Debug, PartialEq`)
  - `pub const GROUP_W: f32 = 4.0 * 220.0 + 3.0 * 3.0;` (889)
  - `pub fn fallback_scale(mode_px: (u32, u32)) -> f32`
  - `pub fn density_scale(mode_px: (u32, u32), phys_mm: (u32, u32)) -> f32` (fallback inside, floor 1, rounded to 0.25)
  - `pub fn fit_scale(s: f32, window_w_px: u32) -> f32`
  - `pub fn choose_output<'a>(outputs: &'a [OutputInfo], monitor_name: Option<&str>, monitor_px: Option<(u32, u32)>) -> Option<&'a OutputInfo>`
  - `pub fn menu_scale(output: Option<&OutputInfo>, fallback_px: (u32, u32), window_w_px: u32) -> f32` — `density_scale` of the output (or `fallback_scale(fallback_px)` when `None`), then `fit_scale`.
  - `pub fn write_scale_file(path: &Path, s: f32) -> std::io::Result<()>` — writes `format!("{s}\n")` to `<path>.tmp`, renames.
  - `pub fn scale_file_path() -> Option<PathBuf>` — `TF_SCALE_FILE` if set, else `$XDG_RUNTIME_DIR/thinfloe-scale`, else `None`.

- [ ] **Step 1: Write the failing tests** (in `density.rs`, `mod tests`)

```rust
fn s(px: (u32, u32), mm: (u32, u32)) -> f32 { density_scale(px, mm) }

#[test] fn a_24_inch_1080p_is_one() { assert_eq!(s((1920, 1080), (531, 299)), 1.0); }
#[test] fn a_21_inch_4k_is_2_25() { assert_eq!(s((3840, 2160), (465, 261)), 2.25); }
#[test] fn a_43_inch_4k_is_one() { assert_eq!(s((3840, 2160), (952, 535)), 1.0); }
#[test] fn a_13_inch_8k_is_7_25() { assert_eq!(s((7680, 4320), (288, 162)), 7.25); }
#[test] fn a_32_inch_1080p_is_not_below_one() { assert_eq!(s((1920, 1080), (708, 398)), 1.0); }
#[test] fn zero_mm_falls_back() {
    assert_eq!(s((3840, 2160), (0, 0)), 2.0);
    assert_eq!(s((1920, 1080), (531, 0)), 1.0);
}
#[test] fn an_aspect_in_cm_falls_back() {          // EDID 16 x 9 cm
    assert_eq!(s((3840, 2160), (160, 90)), 2.0);
    assert_eq!(s((1920, 1080), (160, 90)), 1.0);
}
#[test] fn a_wrong_aspect_falls_back() { assert_eq!(s((1920, 1080), (600, 200)), 1.0); }
#[test] fn ppi_out_of_range_falls_back() {
    assert_eq!(s((1920, 1080), (1000, 563)), 1.0);   // 48.8 ppi
    assert_eq!(s((7680, 4320), (210, 118)), 2.0);    // 929 ppi
}
#[test] fn the_fit_shrinks_a_small_dense_screen() {
    let d = s((1920, 1080), (256, 144));             // 11.6", 2.0 by density
    assert_eq!(d, 2.0);
    assert_eq!(fit_scale(d, 1920), 1.75);
}
#[test] fn the_fit_leaves_a_13_inch_8k_alone() { assert_eq!(fit_scale(7.25, 7680), 7.25); }
#[test] fn the_fit_stops_at_one_half() { assert_eq!(fit_scale(1.0, 400), 0.5); }
#[test] fn a_rotated_monitor_takes_density_from_its_native_size_and_fits_its_width() {
    let out = OutputInfo { name: Some("DP-1".into()), mode_px: (3840, 2160), phys_mm: (465, 261) };
    assert_eq!(menu_scale(Some(&out), (3840, 2160), 2160), 2.0);
}
#[test] fn the_group_with_its_side_gaps_fits_every_result() {
    for w in [400u32, 800, 1024, 1366, 1920, 2560, 3840, 7680] {
        for d in [1.0f32, 1.75, 2.25, 4.0, 7.25] {
            let f = fit_scale(d, w);
            assert!(f == 0.5 || GROUP_W * f <= 0.9 * w as f32, "{w} {d} {f}");
            assert_eq!((f * 4.0).fract(), 0.0);
        }
    }
}
#[test] fn the_output_is_found_by_name_else_alone_else_by_size() {
    let a = OutputInfo { name: Some("DP-1".into()), mode_px: (1920, 1080), phys_mm: (531, 299) };
    let b = OutputInfo { name: Some("HDMI-A-1".into()), mode_px: (3840, 2160), phys_mm: (465, 261) };
    let both = [a.clone(), b.clone()];
    assert_eq!(choose_output(&both, Some("HDMI-A-1"), None), Some(&b));
    assert_eq!(choose_output(&both, None, Some((3840, 2160))), Some(&b));
    assert_eq!(choose_output(&both, None, None), None);
    assert_eq!(choose_output(&[a.clone()], None, None), Some(&a));
}
#[test] fn the_scale_file_holds_one_number() {
    let dir = crate::testdir::TestDir::new("density", "scale");   // derefs to &Path
    let p = dir.join("thinfloe-scale");
    write_scale_file(&p, 2.25).unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "2.25\n");
    write_scale_file(&p, 1.0).unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "1\n");
    assert!(!p.with_extension("tmp").exists());
}
#[test] fn an_unwritable_scale_file_is_an_error_not_a_panic() {
    assert!(write_scale_file(Path::new("/nonexistent-dir/thinfloe-scale"), 1.0).is_err());
}
```

- [ ] **Step 2: Run, expect FAIL** — `cd src/thinfloe-menu && cargo test -j4 --lib density` → compile errors (module missing).

- [ ] **Step 3: Implement** the functions above. Order in `density_scale`: reject (either mm 0, width < 200 mm, aspect `|(mm_w/mm_h)/(px_w/px_h) - 1| > 0.10`, ppi outside 50..=800) → `fallback_scale`; else `((ppi / 92.0).max(1.0) * 4.0).round() / 4.0`. `fit_scale`: `max = 0.9 * w / GROUP_W`; if `s <= max` return `s`, else `((max * 4.0).floor() / 4.0).max(0.5)`. `choose_output`: by name, else the only output, else the first whose `mode_px == monitor_px`, else `None`.

- [ ] **Step 4: Run, expect PASS** — same command, all density tests green.

- [ ] **Step 5: Commit** — `git commit -m "menu: density rule for the menu scale"` (with the Co-Authored-By line).

---

### Task 2: Reading the outputs (`outputs.rs`)

**Files:**
- Create: `src/thinfloe-menu/src/outputs.rs`
- Modify: `src/thinfloe-menu/src/lib.rs`

**Interfaces:**
- Consumes: `density::OutputInfo`.
- Produces: `pub fn read() -> Result<Vec<OutputInfo>, String>` — own `Connection::connect_to_env()`, binds every `wl_output` global at version `1..=4`, two roundtrips, returns one `OutputInfo` per output that sent a current mode. No thread; the connection is dropped on return.
- Internal, pure and tested: `enum OutEvent { Geometry { mm: (i32, i32) }, Mode { current: bool, px: (i32, i32) }, Name(String), Done }` and `struct Collector` with `fn apply(&mut self, id: u32, e: OutEvent)` and `fn outputs(&self) -> Vec<OutputInfo>`, following the `Tracker`/`Event` split in `windows.rs`.

- [ ] **Step 1: Failing tests** for `Collector`:
  - `a_current_mode_and_geometry_make_one_output`: Geometry (531, 299), Mode{current:false,(1024,768)}, Mode{current:true,(1920,1080)}, Name("DP-1"), Done → `[OutputInfo{name:Some("DP-1"), mode_px:(1920,1080), phys_mm:(531,299)}]`.
  - `an_output_without_a_current_mode_is_left_out`.
  - `negative_mm_count_as_zero` (Geometry (-1, -1) → phys_mm (0, 0)).
  - `a_version_3_output_has_no_name` (no Name event → `name: None`).
- [ ] **Step 2: Run** `cargo test -j4 --lib outputs` → FAIL.
- [ ] **Step 3: Implement** `Collector` and `read()`; `Dispatch<wl_output::WlOutput, u32>` maps `Geometry{physical_width, physical_height, ..}`, `Mode{flags, width, height}` (current = `flags` contains `Mode::Current`), `Name{name}`, `Done` into `OutEvent`. Registry dispatch as in `windows.rs`.
- [ ] **Step 4: Run** → PASS. Also `cargo build -j4` clean, `cargo clippy -j4 -- -D warnings` clean.
- [ ] **Step 5: Commit** `menu: read the outputs' physical size`.

---

### Task 3: Apply the scale in the app

**Files:**
- Modify: `src/thinfloe-menu/src/app.rs` (`Live`, `resumed`, `redraw`, `window_event` `Resized`, `refresh_addons`)

**Interfaces:**
- Consumes: `outputs::read`, `density::{choose_output, menu_scale, write_scale_file, scale_file_path}`, `ui::{load_icons, load_icon, LOGO_H}`.
- Produces: `Live` gains `scale: f32` (the menu's `s`), `monitor: Option<String>` (name the scale was computed for), `outputs: Vec<OutputInfo>`; `fn icon_ppp(&self) -> f32 = scale * window.scale_factor()`. A method `App::apply_scale(&mut self)` that recomputes `s`, and when it changed: `self.ctx.set_zoom_factor(s)`, reloads every icon and the logo (`logo_px = round(LOGO_H * icon_ppp)`) at the new ppp, writes the scale file (error: one `eprintln!("thinfloe-menu: cannot write the scale file: {e}")`, no exit), and, under `TF_MENU_DEBUG`, logs `thinfloe-menu: scale <s> on <name or ?> (<w>x<h> px, <w>x<h> mm)`.

- [ ] **Step 1: Failing test** in `app.rs` `mod tests`: `the_zoom_factor_is_the_pixels_per_point` — `ctx.set_zoom_factor(2.25)`, `ctx.run_ui(RawInput{screen_rect: 3840x2160 px.., ..})`, assert `out.pixels_per_point == 2.25` and the screen rect seen inside `run_ui` is `3840/2.25` points wide. (Proves egui applies the zoom in the same pass; if it applies one pass late, set it before the first `run_ui` and note that in a comment.)
- [ ] **Step 2: Run** → FAIL (test missing → write it, then it may pass at once; that is fine, it pins the egui behaviour the app relies on).
- [ ] **Step 3: Implement.** In `resumed`, before the first `request_redraw`: `outputs::read()` (Err → empty list, logged under debug), monitor = `window.current_monitor()` name and size, `fallback_px` = monitor size, else the largest output mode, else (1920, 1080); window width = `window.inner_size().width` when > 0, else the chosen output's mode width. Call `apply_scale` so the first frame uses `s`. In `redraw`, before `run_ui`: when `window.current_monitor().and_then(|m| m.name())` differs from `live.monitor`, re-read the outputs and `apply_scale` (the "lay out again once"). In `Resized`: `apply_scale` (the fit depends on the width). `refresh_addons` and every `load_icon(s)` call use `icon_ppp()` instead of `window.scale_factor()`.
- [ ] **Step 4: Run** `cargo test -j4` (all) → PASS; `cargo clippy -j4 -- -D warnings` clean.
- [ ] **Step 5: Commit** `menu: draw at the scale of the screen's density`.

---

### Task 4: Fixed tile group; the strip takes from the log

**Files:**
- Modify: `src/thinfloe-menu/src/ui.rs` (`layout`, constants `GROUP_SHARE`/`GROUP_MIN` removed, use `density::GROUP_W`; tests)

**Interfaces:**
- Produces: `layout(screen, tiles, folded) -> Layout` unchanged signature; `Layout.tile.x == 220.0` whenever `screen.width() >= GROUP_W + 2.0 * GAP`.

- [ ] **Step 1: Failing tests** (replace `eight_tiles_on_full_hd_are_two_rows_of_bands_at_58_percent`):
  - `tiles_are_220_by_76_on_every_screen`: for screens (in points) 1920x1080, 3840x2160, 1280x720, 1707x960 (3840x2160 at 2.25): `l.tile == vec2(220.0, 76.0)` (1280x720 with 8 tiles: height may be less, ≥ 64, width 220), `l.grid.width() == GROUP_W`, grid centred (±1).
  - `the_group_never_exceeds_the_screen`: 900x600 → `l.grid.left() >= 0 && l.grid.right() <= 900`.
  - `folding_a_window_takes_only_from_the_log`: 1920x1080, 8 tiles: `layout(..,0).area == layout(..,1).area`, `log.top()` equal, `log.bottom()` of folded = strip top − `STRIP_GAP`.
  - `the_log_keeps_its_minimum_and_the_area_gives_the_rest`: 1024x600, 1 folded: `log.height() >= LOG_MIN_ROWS as f32 * LOG_ROW + 2.0 * LOG_PAD`, `area.bottom()` of folded < unfolded.
  - Keep `nine_tiles_fit_between_header_and_log_on_1280_by_720` and the strip tests passing.
- [ ] **Step 2: Run** `cargo test -j4 --lib ui::tests` → new tests FAIL.
- [ ] **Step 3: Implement.** Group = `GROUP_W.min(screen.width() - 2.0 * GAP)`. Log: compute the unfolded log rect first (`log_h` as today, bottom `screen.bottom() - GAP`); with a strip, keep `log.top()` and move only its bottom up to `strip.top() - STRIP_GAP`; when that leaves less than the minimum log height, set the log to the minimum height ending at `strip.top() - STRIP_GAP` (its top moves up, the area shrinks).
- [ ] **Step 4: Run** → PASS (whole `ui::tests`).
- [ ] **Step 5: Commit** `menu: tiles keep 220 x 76 points; the strip takes its room from the log`.

---

### Task 5: Header margins and "by oetiker.ai"

**Files:**
- Modify: `src/thinfloe-menu/src/ui.rs` (`HEADER_PAD_TOP` 16 → 32, `HEADER_H` to fit, `show_header`)

**Interfaces:**
- Produces: `pub const BYLINE: &str = "by oetiker.ai";` in `ui.rs`.

- [ ] **Step 1: Failing test** `the_header_has_32_points_margin_and_the_byline`: run `show` on 1920x1080 with `Chrome::default()`; find the text shapes "ThinFloe" and `BYLINE`: byline left == wordmark left (±0.5), byline top ≥ wordmark bottom − 2, byline bottom ≤ `layout(..).header.bottom()`, byline font family is Geo at 14 (check `galley.job.sections[0].format.font_id == brand::geo(14.0)`), colour `MUTED`; wordmark left == 32 when no logo is drawn; header text on the right ends at `1920 − 32` (±0.5).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.** Logo top at `rect.top() + 32`. Wordmark and byline as one block centred on the logo's height: wordmark Geo 34 above, byline Geo 14 `MUTED` below, left-aligned with it. `HEADER_H = 32 + LOGO_H + 16` (100), unless the block needs more.
- [ ] **Step 4: Run** `cargo test -j4 --lib ui::tests` → PASS.
- [ ] **Step 5: Commit** `menu: header 32 points from the edges, with "by oetiker.ai"`.

---

### Task 6: Settings panel and row layout D

**Files:**
- Modify: `src/thinfloe-menu/src/ui.rs` (`show_settings`, `settings_row`, remove `STATE_WIDTH`, `ROW_BUTTON_W`)
- Modify: `src/thinfloe-menu/src/settings.rs` (status items)

**Interfaces:**
- Produces in `settings.rs`: `pub enum Tone { Plain, Good, Ice, Warn }` and `pub fn status_items(row: &Row) -> Vec<(String, Tone)>` — `row.state` split on `" · "` (with `" · working…"` appended as `Plain` when `busy`), each item `Good` when it is `"up to date"`, `Ice` when it ends with `"ready for next boot"`, else `Plain`; then `(MANUAL_NOTE, Warn)` when `row.manual`. `ui.rs` maps `Tone` to `t.palette.foreground`, `GOOD`, `ICE`, `WARN`.

- [ ] **Step 1: Failing tests.**
  - `settings.rs`: `status_items_keep_their_colours` — "1.4.2 installed · 1.5.0 ready for next boot" → `[("1.4.2 installed", Plain), ("1.5.0 ready for next boot", Ice)]`; "2.0 · up to date" with `manual` → `[("2.0", Plain), ("up to date", Good), ("updates only by hand", Warn)]`.
  - `ui.rs` (replace `settings_buttons_and_states_line_up_and_nothing_is_rounded`, keep its no-rounding assertion): with `four_addons()` on 1920x1080:
    - `the_settings_panel_is_as_wide_as_the_tile_group`: every row frame rect (the `TILE` fills) is `GROUP_W` wide and centred at x 960 (±1).
    - `a_row_lays_out_name_source_then_status_line_and_button_right`: per row the source text starts right of the name text and on its line (same centre y ±2); the status line's top is below the name's bottom; the button label's right is the row's right minus 16 (±2) and its centre y is the row centre (±2).
    - `a_row_without_a_button_reaches_the_right_padding`: on a row with `button == None` and a source of 300 `x` characters, the source text's right edge is > row right − 16 − 8 and ends with "…".
    - `a_long_source_does_not_push_the_button_off`: on a row with a button and a 300-character source, the button label lies fully inside the row, and the source ends with "…" left of the button.
- [ ] **Step 2: Run** `cargo test -j4 --lib settings ui::tests` → FAIL.
- [ ] **Step 3: Implement.** Panel width `GROUP_W.min(inner.width())`. Row: tick box, icon, then a `right_to_left` layout that adds the button only when there is one (no reserved slot), then a top-down block: line 1 a horizontal of name (Geo 20) + source (monospace 11, `MUTED`, truncated to what is left); line 2 one `LayoutJob` with the items from `status_items` joined by `" · "` in `MUTED`, each item in its tone colour, 12 px proportional, truncated.
- [ ] **Step 4: Run** → PASS, and `cargo test -j4` whole crate.
- [ ] **Step 5: Commit** `menu: Settings as wide as the tiles, one status line per add-on`.

---

### Task 7: `thinfloe-terminal` follows the menu's scale

**Files:**
- Modify: `overlay/usr/bin/thinfloe-terminal`
- Modify: `test/integration/test-terminal.sh`

**Interfaces:**
- Consumes: `$XDG_RUNTIME_DIR/thinfloe-scale` (or `TF_SCALE_FILE`), a decimal number.

- [ ] **Step 1: Failing tests** in `test-terminal.sh` (the `run` helper gains a scale-file argument; `TF_SCALE_FILE` points into `$work`):
  - file `2.25` → pixelsize 28, pad 24 (whatever wlr-randr says, e.g. a 1080p mode).
  - file `2.5` → 42 / 36; file `7.25` → 98 / 84; file `1` on a 4K mode → 14 / 12; file `0.5` → 14 / 12.
  - no file, 4K mode → 28 / 24 (today's rule); no file, wlr-randr fails → 14 / 12.
  - file `abc`, empty, `0`, `100` → the fallback rule as if no file (`100` is outside 0.5..8, so refused).
  Note: `expect` computes pad as `px*12/14`, which holds for every case above.
- [ ] **Step 2: Run** `test/integration/test-terminal.sh` → FAIL.
- [ ] **Step 3: Implement.** Read the file with `awk` (one number, 0.5 ≤ s ≤ 8, else empty); `n = int(s + 0.5)`, at least 1; `px = 14 * n`, `pad = 12 * n`. Without a usable file, today's wlr-randr rule. Update the comment at the top.
- [ ] **Step 4: Run** → all `ok`, no `FAIL`.
- [ ] **Step 5: Commit** `terminal: font size from the menu's scale`.

---

### Task 8: labwc title bars follow the scale

**Files:**
- Create: `overlay/usr/libexec/thinfloe/labwc-scale` (mode 755)
- Create: `test/integration/test-labwc-scale.sh` (mode 755)
- Modify: `overlay/usr/bin/thinfloe-session` (remove a stale scale file before the menu; start the helper in the background, its PID into `HELPER_PIDS`)
- Modify: `Makefile` (`test:` target runs the new test next to `test-labwc-config-merge.sh`)
- Modify: `test/integration/test-session-lifecycle.sh` if its stubs need `TF_LABWC_SCALE` (override path for the helper, as `TF_MENU`)

**Interfaces:**
- Consumes: scale file (as Task 7, same 0.5..8 check), the composed config dir `$XDG_RUNTIME_DIR/thinfloe-labwc` made by `thinfloe-compose-labwc-config` (holds symlinks `rc.xml`, `themerc-override` to `/etc/thinfloe/labwc`).
- Produces: `labwc-scale [--wait SECONDS]` (default 15). Env overrides for tests: `TF_SCALE_FILE`, `TF_LABWC_DIR` (default `$XDG_RUNTIME_DIR/thinfloe-labwc`), `TF_LABWC` (default `labwc`).

Behaviour: wait (poll every 0.2 s) up to `--wait` s for a usable scale file; exit 0 doing nothing when none arrives, when `TF_LABWC_DIR` does not exist (session fell back to `/etc/thinfloe/labwc`), or when `s == 1`. Otherwise replace the `rc.xml` symlink in `TF_LABWC_DIR` by a copy in which only the `size="10"` of each `<font place=…>` line inside `<theme>` becomes `size="<round(10*s)>"`; replace the `themerc-override` symlink by a copy where the title bar padding and button keys hold `round(value * s)` (rewrite the key's line in place; append a key the file lacks, with labwc's default as base). Each file: temp file in the same dir, then `mv`. Then `"$TF_LABWC" --reconfigure`. Any failure: log `thinfloe: title bar scale not applied: <why>` to stderr, restore the symlinks, exit 0.

- [ ] **Step 1: Check labwc 0.9.3** in the built image: `podman run --rm thinfloe-ubuntu:handover man labwc-theme` (or the source package's `docs/labwc-theme.5.scd`) — which keys set title bar padding, button width/height and spacing, their defaults, and whether `<font size>` is points. Write the key list with defaults into the script's header comment. If the image has no man page, read `docs/labwc-theme.5.scd` from the `labwc_0.9.3` source the Dockerfile builds.
- [ ] **Step 2: Failing tests** in `test-labwc-scale.sh` (style of `test-labwc-config-merge.sh`: `ok`/`FAIL` lines, `$TMPDIR` work dir, a stub `labwc` that records its arguments; the composed dir is made by the real `thinfloe-compose-labwc-config` from `overlay/etc/thinfloe/labwc`):
  - `2` → both font lines `size="20"`; padding/button keys doubled; stub called with `--reconfigure` once.
  - `2.25` → `size="23"`.
  - the `<keyboard>` … `</keyboard>` section of the generated `rc.xml` is byte-identical to the system file's, and it has no `<keybind` and no `<default`.
  - everything outside the `<font` lines is identical to the system `rc.xml` (`diff` after removing the font lines).
  - `1` → `rc.xml` is still a symlink; no reconfigure.
  - no file, `--wait 1` → nothing changed, no reconfigure, exit 0.
  - garbage file (`abc`, `100`) → nothing changed, exit 0.
  - `TF_LABWC_DIR` missing → exit 0, no reconfigure.
  - stub `labwc` failing → the symlinks are restored, exit 0, stderr has `thinfloe: title bar scale not applied`.
- [ ] **Step 3: Run** → FAIL.
- [ ] **Step 4: Implement** the script; in `thinfloe-session`, before `"$TF_MENU"`: `rm -f "${XDG_RUNTIME_DIR:-/tmp}/thinfloe-scale"`, then `"${TF_LABWC_SCALE:-/usr/libexec/thinfloe/labwc-scale}" >>"${TF_SESSION_LOG:-/dev/null}" 2>&1 &` with its PID added to `HELPER_PIDS`.
- [ ] **Step 5: Run** `test-labwc-scale.sh`, `test-session-lifecycle.sh`, `test-labwc-config-merge.sh` → all `ok`.
- [ ] **Step 6: Commit** `session: labwc title bars follow the menu's scale`.

---

### Task 9: Pictures, docs, image and gates

**Files:**
- Modify: `src/thinfloe-menu/src/ui.rs` (`shot` takes size and scale)
- Modify: `CHANGES.md`, `CLAUDE.md`

- [ ] **Step 1:** Change the test helper to `fn shot(menu: &Menu, name: &str, px: (u32, u32), s: f32)`: `ctx.set_zoom_factor(s)`, screen rect in points `px / s`, canvas `px`, tessellate and draw at `s`. Add ignored test `shot_desktop_and_settings_at_densities` writing `desktop-1080.ppm` (1920x1080, 1.0), `desktop-4k-fallback.ppm` (3840x2160, 2.0), `desktop-21in-4k.ppm` (3840x2160, 2.25), `settings-1080.ppm`, `settings-21in-4k.ppm`, plus one with two folded windows. Run `TF_SHOT_DIR=/scratch/oetiker/claude-tmp/thinfloe-tests/shots cargo test -j4 --lib shot_ -- --ignored`, convert to PNG (`pnmtopng` or `magick`), look at each.
- [ ] **Step 2: CHANGES.md** under `[Unreleased]` / `### Changed`: one entry for the menu (tiles keep their shape and look the same size on every screen; a 4K screen no longer gets stretched tiles; a small dense laptop screen shrinks the menu to fit), one for the Settings rows (one status line, button at the right), one for the header ("by oetiker.ai", larger margin), one for the terminal and title bars following the screen. Replace the existing terminal sentence "on a 4K screen the font and margin are twice as large" so the two entries do not contradict each other.
- [ ] **Step 3: CLAUDE.md**: under "Reaching a terminal" the new rule (scale file, `14 * round(s)`), under the look/session bullets the density rule, the scale file, `labwc-scale` and its tests; under "Menu limits" what is not verified on hardware (density on real EDIDs, the title bars).
- [ ] **Step 4: Commit** `docs: menu density`.
- [ ] **Step 5: Build** — `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=handover image` (timeout 600000, poll until it ends).
- [ ] **Step 6: Gates** — `make VERSION=handover test` (exit 0, no `FAIL`, count `^ok` lines and compare with 646 + the new ones), `make VERSION=handover test-runtime`, then `make qemu-smoke`, `qemu-smoke-upgrade`, `qemu-smoke-addons`, `qemu-smoke-netboot` (one at a time, never during a build). Each must end with PASSED.
- [ ] **Step 7:** Report to the owner with the screenshots and ask before publishing for the hardware test.
