# Status Area (egui elements, Status tab) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The menu's status area with elements written in Rust and drawn by egui, which the user ticks, sizes and orders in a Settings tab "Status".

**Architecture:** Import PR #6 (rebased as `9626a21` on branch `peripheral-battery`) in three topic commits, keep its data side (battery reader, samplers, redraw loop, grid geometry), then replace its SVG elements, `.conf` files and config parser with an egui `Element` trait, a pure packing function and a per-machine layout file edited from the Status tab.

**Tech Stack:** Rust (egui 0.37, shadcn-egui 0.8, winit, softbuffer), Python 3 (battery reader), systemd units, bash tests.

**Spec:** `docs/superpowers/specs/2026-11-10-status-area-design.md` (read it first; this plan argues from it).

## Global Constraints

- Worktree `/scratch/oetiker/claude-worktrees/thinfloe-status-area`, branch `status-area`, based on main `c449210`. Never push, merge, tag or publish.
- Source of the PR code: commit `9626a21` (branch `peripheral-battery`, worktree `/scratch/oetiker/claude-worktrees/thinfloe-peripheral-battery`, read-only).
- Max 4 cores: `cargo … -j4`, `CARGO_BUILD_JOBS=4`. Menu tests under `systemd-run --user --scope -q -p MemoryMax=4G --`. `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests/status-area/<task>` (create it; delete nothing outside it).
- Image builds: `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make …`, default `VERSION` only.
- The menu draws on the CPU, egui only; no GPU crates. Lyra style, nothing rounded. Zero keybindings.
- UI code takes colours from `crate::brand::theme()` and text sizes from named constants in `brand.rs`; `ui_code_has_no_style_literals` must cover `src/status/`.
- Cell = tile size at the current scale (220 x 76 pt base), gap `TILE_GAP`, 4 columns, at most 3 rows; short screen = window height under `ui::SHORT_SCREEN_H` (900 pt).
- Layout file `~/.config/thinfloe/status` (override `TF_STATUS_FILE`), lines `<id> <w>x<h>` / `-<id> <w>x<h>`, `#` comments, max 4 KiB and 64 lines, UTF-8.
- English in code, comments, docs. CHANGES.md entries for users (house rules in `~/.claude/CLAUDE.md`).
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- A hand-edited layout file with CRLF line ends, tabs, trailing spaces or a `+` in a size: each line parses or is skipped alone; the rest still counts (Task 2 test `layout_lines_tolerate_hand_editing`).
- A home archive from a newer image naming an unknown element: an older menu that rewrites the file keeps that line at its place (Task 2 test `unknown_lines_survive_a_rewrite`).
- A short screen with only tall elements that have no short size: the area is gone and the tiles are centred as without the feature (Task 3 test `a_short_screen_with_only_tall_elements_draws_no_area`).
- Long device names, many DNS servers, many sensors: nothing an element draws lands outside its rect (Task 3/4/5 test `<element>_draws_inside_its_rect_at_every_size`).
- Ticking and unticking quickly: no sampler thread is started twice; a resting sampler reads nothing (Task 6 test `samplers_start_once_and_rest_when_unticked`).

---

### Task 1: Import PR #6 in three topic commits

**Files:** everything `git diff c449210 9626a21 --stat` lists.

- [ ] **Step 1:** In the worktree, `git checkout 9626a21 -- scripts/lib/common.sh .dockerignore vendor/.gitignore overlay/usr/bin/thinfloe-addon` plus the `dialog` line removal in `containers/ubuntu/Dockerfile` only (edit by hand; the PR's other Dockerfile line belongs to step 2). Remove `vendor/.gitignore` if the PR deleted it, and `containers/ubuntu/patches/.gitkeep` and the `Makefile` `BUILD_DATE` line the same way. Run `bash test/integration/test-container-inputs.sh` and `shellcheck scripts/lib/common.sh`. Commit `build: drop unused helpers, the dialog package and stale build-context entries`.
- [ ] **Step 2:** `git checkout 9626a21 --` the battery reader and its system parts: `overlay/usr/libexec/thinfloe/peripheral-battery`, `overlay/etc/systemd/system/thinfloe-peripheral-battery*.{service,timer}`, `overlay/etc/udev/rules.d/90-thinfloe-peripheral-battery.rules`, `overlay/usr/lib/tmpfiles.d/thinfloe-peripherals.conf`, `test/unit/test_peripheral_battery.py`, the `systemctl enable thinfloe-peripheral-battery…` Dockerfile line, and the battery-unit hunks of `test/integration/test-session-packages.sh`. Run `make test-unit` (expect all pass). Commit `battery: a root reader for keyboard and mouse batteries`.
- [ ] **Step 3:** `git checkout 9626a21 -- .` for the rest (menu crate, `overlay/etc/thinfloe/status/`, runtime test, docs, CHANGES, README, CLAUDE.md, remaining session-packages hunks). `git diff 9626a21 --stat` must be empty.
- [ ] **Step 4:** Verify: `cd src/thinfloe-menu && systemd-run --user --scope -q -p MemoryMax=4G -- cargo test -j4` (expect PASS, note the count), `cargo clippy -j4 --all-targets -- -D warnings`, `cargo fmt --check`.
- [ ] **Step 5:** Baseline pictures: run the ignored `shot_elements` test (`cargo test -j4 shot_elements -- --ignored`) and copy the PPMs to `$TMPDIR/baseline/` (convert with `pnmtopng`). Later tasks compare against them.
- [ ] **Step 6:** Commit `menu: a status area below the tiles (PR #6, SVG elements)`.

### Task 2: Layout file and packing (pure)

**Files:**
- Create: `src/thinfloe-menu/src/status/layout.rs`
- Modify: `src/thinfloe-menu/src/status/mod.rs` (add `pub mod layout;`, add `Size`)

**Interfaces:**
- Produces:
  - `pub struct Size { pub w: u8, pub h: u8 }` in `status/mod.rs`, `Size::parse(&str) -> Option<Size>` for `WxH` (1..=4 x 1..=3), `Display` as `WxH`.
  - `pub struct Choice { pub id: String, pub size: Size, pub shown: bool, pub known: bool }` (an unknown id: `known == false`, written back verbatim, never drawn).
  - `pub struct Layout { pub choices: Vec<Choice>, pub notes: Vec<String>, pub refused: bool }`.
  - `pub fn parse(text: &[u8], known: &dyn Fn(&str) -> Option<&'static [Size]>) -> Layout` (`known` returns the offered sizes of an id).
  - `pub fn default_layout(elements: &[&dyn Element]) -> Layout`, `pub fn complete(layout: &mut Layout, elements: &[&dyn Element])` (appends elements the file does not name: shown, default size).
  - `pub fn to_text(layout: &Layout) -> String` (header comment line `# thinfloe status area, written by the menu`, then one line per choice).
  - `pub fn pack(items: &[Size], short: bool, short_sizes: &[Option<Size>]) -> Vec<Option<Cells>>` (one result per item, `None` = left out).

- [ ] **Step 1: Tests** in `layout.rs`:
  - `the_default_list_packs_to_the_pr6_layout`: sizes `[1x1, 2x2, 1x2, 1x1, 1x1, 1x1, 1x1, 1x1]` pack to cells `(0,0,1,1) (1,0,2,2) (3,0,1,2) (0,1,1,1) (0,2,1,1) (1,2,1,1) (2,2,1,1) (3,2,1,1)`.
  - `packing_is_first_fit_row_by_row`: `[2x1, 3x1, 2x1]` → `(0,0,2,1)`, `(0,1,3,1)`, `(2,0,2,1)`.
  - `an_item_that_fits_nowhere_is_left_out`: `[4x3, 1x1]` → `Some(0,0,4,3)`, `None`.
  - `a_short_screen_has_one_row`: `[1x1 (short 1x1), 2x2 (short 3x1), 1x2 (short none)]`, short → `(0,0,1,1)`, `(1,0,3,1)`, `None`.
  - `missing_or_empty_file_is_the_default`; `hidden_lines_keep_place_and_size` (`-clock 1x1` → `shown: false`, round-trips through `to_text`).
  - `layout_lines_tolerate_hand_editing`: CRLF, tabs, trailing spaces, `#` after content, `+1x1`, `1x1x1`, blank lines → each bad line is one note, the good lines count.
  - `unknown_lines_survive_a_rewrite`: `battery 1x1\nups 2x1\nclock 1x1` → `to_text` keeps `ups 2x1` between battery and clock.
  - `a_size_not_offered_falls_back_to_the_default`; `a_second_line_for_an_id_is_ignored`.
  - `an_oversized_or_binary_file_is_refused`: 4097 bytes, 65 lines, invalid UTF-8 → default list, `refused == true`, one note.
  - `new_elements_are_appended_shown` (via `complete`).
- [ ] **Step 2:** Run `cargo test -j4 status::layout` → FAIL (not defined).
- [ ] **Step 3:** Implement. `pack` keeps a 4 x rows occupancy grid (rows = 1 on short, else 3); each item takes the first `(row, col)` in row-major order where its rectangle is free.
- [ ] **Step 4:** Run `cargo test -j4 status::layout` → PASS.
- [ ] **Step 5:** Commit `menu: the status layout file and its packing`.

### Task 3: The egui element trait, wired in, with battery and clock

**Files:**
- Modify: `src/thinfloe-menu/src/status/mod.rs` (new trait, `ELEMENTS`, `Area` on top of `layout`), `status/battery.rs`, `status/clock.rs`, `src/ui.rs` (`paint_status`, layout calls, drop `status_texture`/`StatusTexture`/`STATUS_TEXTURE`), `src/menu.rs` (`status: Area` from a `Layout`), `src/main.rs` (`--check`), `src/brand.rs` (status text-size constants; palette private again)
- Delete: `status/render.rs`, `status/style.rs` (move what egui needs, e.g. the per-cell `LARGE`/`SMALL` sizes, into named `brand.rs` constants), `status/config.rs`, `overlay/etc/thinfloe/status/`, resvg `text` feature in `Cargo.toml`
- Network and hardware modules: unlisted from `ELEMENTS` in this task (their SVG code is removed in Tasks 4 and 5); the crate must still build (gate them with `#[allow(dead_code)]` only if needed, removed again in Task 5).

**Interfaces:**
- Produces (in `status/mod.rs`; `Sources` is renamed `Data`, same fields):
```rust
pub enum Source { Battery, Network, Hardware }
pub trait Element: Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn sizes(&self) -> &'static [Size];          // first = default
    fn short_size(&self) -> Option<Size>;
    fn sources(&self) -> &'static [Source];
    fn has_content(&self, data: &Data) -> bool;
    fn draw(&self, ui: &mut egui::Ui, rect: egui::Rect, size: Size, data: &Data, now: Instant);
    fn next_change(&self, data: &Data, rect: egui::Rect, now: Instant) -> Option<Duration>;
    fn refresh(&self) -> Option<Duration>;       // periodic redraw, None = on data change
}
pub static ELEMENTS: &[&dyn Element];
pub fn element(id: &str) -> Option<&'static dyn Element>;
```
  - `Area { layout: Layout }` with `shown(&self, data, short) -> Vec<(&'static dyn Element, Size, Cells)>` (pack of shown choices that have content), `planned_rows(short)` (pack of all shown choices, content ignored), `rows(data, short)`, `wake(data, origin, cell, gap, now, short) -> Option<Duration>`, `needs(source) -> bool` (some shown choice's element lists it).
  - `paint_status` draws each element into a child `Ui` at its rect with `set_clip_rect(rect)`.
- [ ] **Step 1: Tests:** `battery_draws_inside_its_rect_at_every_size` and `clock_draws_inside_its_rect_at_every_size` (render a frame with sample data, every shape's bounding rect within the element rect; use 10 devices with 60-character names for battery); `battery_pages_turn_every_4_s` (`next_change`); `a_short_screen_with_only_tall_elements_draws_no_area` (ui layout: `status` is `None`, tiles as without area); `ui_code_has_no_style_literals` extended to `src/status/`; port the PR's area tests (`tiles_and_status_area_are_centred_together…`, `the_shipped_status_area_covers_no_tile_on_any_screen`, `the_end_of_session_dialogs_draw_no_status_area`) to the new `Area`.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement. Battery keeps its rules (red below 10 % or `critical`, `–` muted when unknown or not reachable, kind names, paging every 4 s); clock keeps the time and date in the machine's zone, redrawn at the minute (`header::until_next_minute`). Fonts: Geo for labels via the theme's font families, body font for numbers.
- [ ] **Step 4:** Run the full menu test suite, clippy, fmt → PASS. Shots of battery and clock next to the Task 1 baseline: same facts and colours.
- [ ] **Step 5:** Commit `menu: status elements draw with egui; battery and clock`.

### Task 4: Network graph and network details in egui

**Files:** `status/network.rs`, `status/network_info.rs`, `status/mod.rs` (`ELEMENTS`), `src/brand.rs` if a size constant is needed.

- [ ] **Step 1: Tests:** `network_draws_inside_its_rect_at_every_size`, `network_info_draws_inside_its_rect_at_every_size` (6 DNS servers, a 40-character interface name); `the_graph_scales_to_the_peak` (the highest point of the rx line touches the top of the plot area within 1 pt); `short_network_is_the_graph_alone` (at 3x1 no legend rows outside the plot); port the PR's network element tests that check facts, not SVG text.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement with `egui::Shape::line` for rx and tx, axis labels as egui text, sizes `[2x2, 3x1, 2x1]` short `3x1`, info sizes `[1x2, 2x1]` short none, `refresh() = Some(1 s)`, sources `[Network]`. Data stays `netstat::Snapshot` and `Data::network_since`.
- [ ] **Step 4:** Tests, clippy, fmt → PASS; shots vs baseline.
- [ ] **Step 5:** Commit `menu: the network status elements draw with egui`.

### Task 5: Hardware elements in egui

**Files:** `status/hardware.rs` (split into four elements `temperatures`, `cpu`, `memory`, `system` sharing helpers), `status/mod.rs`.

- [ ] **Step 1: Tests:** `<id>_draws_inside_its_rect_at_every_size` for all four (12 sensors, 8 fans, a 60-character model name); port the PR's rules: CPU temperature always kept, least important sensor dropped first, hot/critical colours, fans that do not fit left out.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement; sizes per the spec table, `refresh() = Some(5 s)`, sources `[Hardware]`. Remove every leftover SVG helper and any `dead_code` allowance from Task 3. `rg -n 'svg|resvg' src/status` finds nothing.
- [ ] **Step 4:** Tests, clippy, fmt → PASS; shots vs baseline; `cargo tree -e features | rg 'resvg.*text'` is empty.
- [ ] **Step 5:** Commit `menu: the hardware status elements draw with egui`.

### Task 6: The layout file in the running menu, samplers on demand

**Files:** `src/app.rs` (file watch, sampler start/rest), `src/menu.rs`, `src/main.rs` (`TF_STATUS_FILE`, `--check`), `src/netstat.rs` (`on: Arc<AtomicBool>` like `hardware_on`), `src/jobs.rs`.

**Interfaces:**
- Produces: `Menu::set_status_layout(Layout)`; `Job::StatusSave(Option<String>)` (`None` removes the file) and `JobResult::StatusSaved(Result<(), String>)`; app helper `fn sync_samplers(&mut self)` called after every layout change and every cover change.
- [ ] **Step 1: Tests:** `samplers_start_once_and_rest_when_unticked` (app-level unit test with a counting spawn function: tick, untick, tick → 1 spawn, `on` flag false then true); `a_changed_layout_file_is_read_again`; `check_names_skipped_layout_lines` (`thinfloe-menu --check` prints each note); `a_refused_file_is_not_rewritten_until_a_change` (no `StatusSave` effect at load).
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement. A sampler starts on the first `needs()` and is never started again; its `on` flag is `needs() && !covered` (hardware also `!short`, as in PR #6). Default path `$HOME/.config/thinfloe/status`; the save job writes `<file>.new` then renames, creating `~/.config/thinfloe/` if missing.
- [ ] **Step 4:** Tests, clippy, fmt → PASS; `bash test/runtime/test-labwc-runtime.sh` (via `make test-runtime`) still reports the covered/uncovered lines.
- [ ] **Step 5:** Commit `menu: the status layout comes from the home and samplers run on demand`.

### Task 7: The Status tab

**Files:** `src/settings.rs` (`SettingsTab::Status` after `Screen`), create `src/statustab.rs` (state and events) and `src/statustab_ui.rs` (drawing), `src/menu.rs` (event routing), `src/ui.rs` (tab dispatch).

**Interfaces:**
- Produces: `pub enum StatusEvent { Shown(String, bool), Size(String, Size), Up(String), Down(String), Reset }`; `Menu::status_event(StatusEvent) -> Option<Effect>` returning `Effect::Start(Job::StatusSave(..))`.
- [ ] **Step 1: Tests** (state machine in `statustab.rs`/`menu.rs`): tick hides and shows in place; size select offers only `sizes()`; `Up` on the first row and `Down` on the last do nothing; unknown lines are never listed and survive every event; `Reset` gives `StatusSave(None)` and the default list; a failed save shows its error and keeps the on-screen state; a row whose element is left out shows "no room", one without content "nothing to show on this machine". UI tests in `ui.rs`: one row per element of `ELEMENTS` with tick box, size select and two buttons; clicks produce the events; the preview area is drawn below the list and equals the main-screen packing for the window's short/normal state; the tab fits at 1280x720 and 1920x1080 without horizontal scroll.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement with shadcn-egui widgets already used by the other tabs (`Select`, checkbox, buttons). Preview: the same `paint_status` at the real cell size, scaled by egui zoom of a child only when the tab is narrower than `GROUP_W`.
- [ ] **Step 4:** Tests, clippy, fmt → PASS. Add a shot of the tab at 1080p to the ignored shot test.
- [ ] **Step 5:** Commit `menu: a Status tab in Settings chooses the status area`.

### Task 8: Docs and gates

**Files:** `CLAUDE.md`, `CHANGES.md`, `README.md`, `docs/superpowers/specs/2026-11-03-status-area.md` (a note at the top: superseded by the 2026-11-10 design for drawing, configuration and elements).

- [ ] **Step 1:** CLAUDE.md: replace the PR's 337 lines with at most 40: the `Element` trait and how to add one (pointer to `status/mod.rs`), the layout file and its rules, the battery reader's file format, units and limits; move protocol details into comments of `peripheral-battery` where they are not already. CHANGES.md: user entries for the status area (from the PR, without the SVG wording) and one for the Status tab; no release header. README: the status area paragraph mentions the Status tab.
- [ ] **Step 2:** Gates: menu tests, clippy, fmt; `make test` (expect rc 0); `make test-runtime`; `make test-unit`; `bash test/integration/test-session-lifecycle.sh`; `shellcheck` on changed shell files; `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make image`; `make qemu-smoke` and `make qemu-smoke-addons`. Keep logs in `$TMPDIR/gates/`.
- [ ] **Step 3:** Commit `docs: the status area and its Status tab`.
