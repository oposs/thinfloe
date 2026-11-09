# ThinFloe Part D: Monitor Arrangement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Screen tab arranges the connected monitors (on/off, resolution, rotation, relative position, which one shows the menu), stored as a menu-owned kanshi config, applied at once with a 15-second revert.

**Architecture:** The menu reads the monitors through `zwlr_output_manager_v1` on a listener thread and never configures outputs itself. It writes `~/.config/kanshi/config` (one profile per set of monitors) and runs `kanshictl reload`; `thinfloe-session` starts kanshi when the file exists. The menu monitor goes through `$XDG_RUNTIME_DIR/thinfloe-menu-output`, which `labwc-scale` turns into a `MoveToOutput` window rule; the menu then replaces its window.

**Tech Stack:** Rust (egui 0.37, shadcn-egui 0.8, winit 0.30, wayland-client 0.31, wayland-protocols-wlr 0.3), POSIX sh (`labwc-scale`, `thinfloe-session`), kanshi, labwc 0.9.3, podman runtime tests.

**Spec:** `docs/superpowers/specs/2026-11-02-thinfloe-settings-and-writer-design.md`, sections "Part D: monitor arrangement", "Testing" (Part D line), "Settled after the builder check" items 7 and 8, "Settled after the pushback review" items 3, 4 and 8.

## Global Constraints

- labwc outputs **stay at scale 1**; no mirroring, no per-output scale (spec Non-goals).
- kanshi is the only one that sets monitors; the menu never sends a `zwlr_output_configuration_v1` (Settled 8).
- A monitor is identified as kanshi does it: make, model and serial; when the serial is missing, the output name (Settled 8).
- `labwc-scale` is the only program that writes the composed `rc.xml` (Pushback 4).
- The menu runs as tfuser; nothing in Part D needs root.
- The menu never overwrites a hand-written file: `~/.config/kanshi/config` goes through `owned.rs` (header line, ByHand never written, take-over to `config.by-hand`).
- A layout change never leaves a person with no visible menu (Anti-goal): the menu monitor cannot be switched off; at least one monitor stays on.
- Zero labwc keybindings, also in the generated `rc.xml`.
- UI: shadcn-egui 0.8, style lyra, nothing rounded, body font Ubuntu Light, Lucide icons only, no emoji in UI text; new UI files join the style guard list in `brand.rs` (~290).
- Every slow call (kanshictl, file writes, waiting for labwc) runs off the UI thread (`jobs.rs` pattern).
- English for code, comments and identifiers. CHANGES.md entries follow the house rules (reader is a user, three sentences at most).
- Machine: at most 4 cores (`-j 4`), podman, builds via `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=monitor-arrangement …`, `TMPDIR` under `/scratch/oetiker/claude-tmp/thinfloe-tests/<private dir>/`, cargo tests under `systemd-run --user --scope -q -p MemoryMax=4G --`.

## Review Focus

1. **Two identical monitors without a serial** (two "Dell U2419H", serial empty): kanshi criteria would collide. Expected: each falls back to its output name, so both stay distinct. Test: Task 3 `criteria_fall_back_to_the_name_when_make_model_serial_collide`.
2. **Refresh rates that are not whole numbers** (59 940 mHz): the mode string kanshi gets must select exactly the mode the person chose. Expected: `1920x1080@59.940Hz`. Test: Task 3 `mode_text_keeps_millihertz`.
3. **A monitor is unplugged while the 15-second question counts down.** Expected: the countdown goes on; Keep keeps the new file, a timeout or *Go back* restores `config.prev`; no panic on a vanished selection. Test: Task 6 `hotplug_during_the_countdown_still_reverts`.
4. **The menu monitor is unplugged** (laptop leaves the dock). Expected: the runtime file is emptied, labwc places the menu as today, and the menu does not replace its window in a loop. Test: Task 6 `a_missing_menu_monitor_clears_the_choice_once`.
5. **A hand-written kanshi config, and a `kanshi` line in `~/.thinfloe-sessionrc`.** Expected: the session does not start a second kanshi; the menu shows the file read-only and never writes it until *Let the menu manage it*. Tests: Task 5 `kanshi already running: session starts no second one`; Task 6 `a_hand_file_is_read_only`.

---

## File Structure

- Create `src/thinfloe-menu/src/heads.rs` — `zwlr_output_manager_v1` reader: a pure `HeadCollector` and a listener thread `Heads::connect`.
- Create `src/thinfloe-menu/src/kanshi.rs` — pure layout model: criteria, placement → positions, overlap check, profile text, parsing the menu's own file.
- Create `src/thinfloe-menu/src/monitors.rs` — `MonTab` state machine for the Screen tab's monitor part (events → requests, countdown, hotplug, menu monitor).
- Create `src/thinfloe-menu/src/monitors_ui.rs` — drawing the boxes, the controls and the *Keep this arrangement?* dialog.
- Create `test/runtime/kanshi-two.conf` — the profile text both the unit test and the runtime test use.
- Modify `src/thinfloe-menu/src/{lib.rs,app.rs,menu.rs,jobs.rs,settings.rs,ui.rs,brand.rs}`.
- Modify `overlay/usr/libexec/thinfloe/labwc-scale`, `overlay/usr/bin/thinfloe-session`, `overlay/home/tfuser/.thinfloe-sessionrc` (comment only).
- Modify `test/runtime/test-labwc-runtime.sh`, `test/integration/test-labwc-scale.sh`, `test/integration/test-session-lifecycle.sh`.
- Modify `CHANGES.md`, `CLAUDE.md`.

---

### Task 1: Gate check — MoveToOutput, a new window, kanshi in the image

The spec makes this the first task, and it decides whether Part D goes on (Settled 7). **If any check fails, stop and report to the controller; do not work around it.**

**Files:**
- Modify: `test/runtime/test-labwc-runtime.sh`
- Create: `test/runtime/kanshi-two.conf`

**Interfaces:**
- Produces: `test/runtime/kanshi-two.conf` (Task 3 asserts its generated text equals this file byte for byte).

- [ ] **Step 1: Write the kanshi fixture** `test/runtime/kanshi-two.conf`:

```
# Written by the ThinFloe menu. Edits here are replaced.

profile {
	output HEADLESS-1 enable mode 1280x720@60.000Hz position 0,0 transform normal scale 1
	output HEADLESS-2 enable mode 1024x768@60.000Hz position 1280,0 transform normal scale 1
}
```

(Headless outputs have no serial, so the criteria are output names. If the image's kanshi rejects any of this syntax, the check below fails: report the exact error.)

- [ ] **Step 2: Add case `kanshi applies a profile`** — 2 headless outputs, the image's `kanshi -c /fixture/kanshi-two.conf &`, wait up to 5 s, then `wlr-randr --json` must show HEADLESS-2 at x=1280 with mode 1024x768. Also `command -v kanshictl` and `kanshictl reload` (exit 0) after editing the copy to `position 0,768` for HEADLESS-2 → HEADLESS-2 at y=768. Print `kanshi --version` in the log.

- [ ] **Step 3: Add case `menu maps on the MoveToOutput output`** — copy `/etc/thinfloe/labwc` to a temp dir in the container, add `<action name="MoveToOutput" output="HEADLESS-2"/>` before `ToggleAlwaysOnBottom` in the `thinfloe-menu` rule, set HEADLESS-2 to 1024x768 with `wlr-randr` before the menu starts, run the menu with `TF_MENU_DEBUG=1`; the log line `thinfloe-menu: configure 1024x768` proves the maximized window is on HEADLESS-2 (HEADLESS-1 stays 1280x720).

- [ ] **Step 4: Add case `MoveToOutput to a missing output`** — same, with `output="HEADLESS-9"`: the menu must still map (configure 1280x720 or 1024x768), labwc must not crash.

- [ ] **Step 5: Run** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=monitor-arrangement test-runtime` (after `make rootfs`). Expected: the three new cases print `ok   -`. A new winit window *after a change* is checked in Task 7, once the menu can replace its window.

- [ ] **Step 6: Commit** `runtime test: kanshi profile, MoveToOutput window rule (Part D gate)`.

---

### Task 2: Output-manager reader (`heads.rs`)

**Files:**
- Create: `src/thinfloe-menu/src/heads.rs`; Modify: `src/thinfloe-menu/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub struct Mode { pub w: u32, pub h: u32, pub refresh_mhz: u32, pub preferred: bool }`
  - `pub enum Rotation { Normal, R90, R180, R270 }` (flipped transforms read as their unflipped rotation)
  - `pub struct Head { pub name: String, pub make: String, pub model: String, pub serial: String, pub phys_mm: (u32, u32), pub enabled: bool, pub modes: Vec<Mode>, pub current: Option<usize>, pub pos: (i32, i32), pub rotation: Rotation }` (all derive `Debug, Clone, PartialEq, Eq`)
  - `pub enum HeadEvent { Name(String), Make(String), Model(String), Serial(String), PhysMm(i32, i32), Enabled(bool), Position(i32, i32), Transform(i32), Mode { id: u32 }, ModeSize { id: u32, w: i32, h: i32 }, ModeRefresh { id: u32, mhz: i32 }, ModePreferred { id: u32 }, CurrentMode { id: u32 }, Finished }`
  - `pub struct HeadCollector` with `pub fn head(&mut self, head: u32, e: HeadEvent)`, `pub fn done(&mut self) -> Vec<Head>` (snapshot, heads sorted by name)
  - `pub struct Heads` with `pub fn connect(on_change: impl Fn() + Send + 'static) -> Result<Heads, String>` and `pub fn snapshot(&self) -> Vec<Head>`

- [ ] **Step 1: Write failing collector tests** in `heads.rs`:
  - `a_head_with_modes_and_a_current_mode` — events for one head with two modes, current = second → `done()` gives one `Head` with `modes.len()==2`, `current==Some(1)`, `refresh_mhz==59940` on it.
  - `a_finished_head_is_dropped` — `Finished` removes the head from the next snapshot.
  - `a_mode_finished_before_current_is_ignored` — `CurrentMode` with an unknown id leaves `current==None`.
  - `transforms_map_to_rotations` — transform 1 → R90, 5 (flipped-90) → R90, 2 → R180.
  - `heads_are_sorted_by_name` — HDMI-A-1 before eDP-1 (byte order).
- [ ] **Step 2:** `cargo test --locked -j 4 --lib heads::` → FAIL (not defined).
- [ ] **Step 3: Implement** the collector, then `Heads::connect` following `windows.rs:245` `Windows::connect`: own `Connection::connect_to_env()`, bind `zwlr_output_manager_v1` (version `min(4)`), two roundtrips, then a thread with `blocking_dispatch`; on the manager's `done` the snapshot goes into `Arc<Mutex<Vec<Head>>>` and `on_change()` runs. No manager global → `Err("the compositor offers no output management")`.
- [ ] **Step 4:** tests PASS; `cargo clippy --locked -j 4 --all-targets` clean.
- [ ] **Step 5: Commit** `menu: read monitors through zwlr_output_manager_v1`.

---

### Task 3: Layout model and kanshi text (`kanshi.rs`)

**Files:**
- Create: `src/thinfloe-menu/src/kanshi.rs`; Modify: `lib.rs`

**Interfaces:**
- Consumes: `heads::{Head, Mode, Rotation}`.
- Produces:
  - `pub fn criteria(head: &Head, all: &[Head]) -> String` — `"<make> <model> <serial>"`; the output name when the serial is empty or when two heads give the same string.
  - `pub enum Side { LeftOf, RightOf, Above, Below }`
  - `pub enum Place { Origin, Next { to: String /* criteria */, side: Side } }` — left/right align the tops, above/below align the left edges (spec).
  - `pub struct OutputSet { pub criteria: String, pub enabled: bool, pub mode: Mode, pub rotation: Rotation, pub place: Place }`
  - `pub struct Profile { pub outputs: Vec<OutputSet>, pub menu_output: Option<String> /* criteria */ }`
  - `pub fn positions(p: &Profile) -> Result<Vec<(i32, i32)>, String>` — absolute positions (rotated sizes for R90/R270), shifted so the smallest x and y are 0; disabled outputs get none and an output placed next to one is placed next to that one's own anchor; overlap → `Err("<A> and <B> overlap.")`.
  - `pub fn set_place(p: &mut Profile, who: &str, place: Place)` — when `to` depends (through the chain) on `who`, `to` takes `who`'s old place first, so the chain stays a tree.
  - `pub fn mode_text(m: &Mode) -> String` — `"{w}x{h}@{hz}.{mhz:03}Hz"`.
  - `pub fn render(profiles: &[Profile]) -> String` — `owned::HEADER` is added by `owned::write`; the body is profiles in the fixture's format. Placement and menu monitor are kept in comment lines the menu reads back: `\t# thinfloe place <criteria> <side> <to>` and `\t# thinfloe menu <criteria>` (criteria quoted with `"` when they contain a space, as kanshi's own lines).
  - `pub fn parse(text: &str) -> Result<Vec<Profile>, String>` — reads the menu's own file only (`render` output, header included); anything else is an error.
  - `pub fn profile_for(profiles: &[Profile], heads: &[Head]) -> Option<usize>` — the profile whose criteria set equals the connected set.
  - `pub fn from_heads(heads: &[Head]) -> Profile` — the current arrangement: first head by x then y is `Origin`; each other head is placed next to the head it touches (right of before below), else `RightOf` the previous one; mode = current or preferred or first.

- [ ] **Step 1: Write failing tests:**
  - `renders_the_runtime_fixture` — a two-head profile (HEADLESS-1 1280x720@60000, HEADLESS-2 1024x768@60000 `RightOf`) renders, with `owned::HEADER` + `"\n\n"` in front, exactly `include_str!("../../../test/runtime/kanshi-two.conf")` with the two `# thinfloe` comment lines inserted — adjust the fixture in this task so the two are equal and Task 1's case still passes.
  - `mode_text_keeps_millihertz` — 59 940 → `1920x1080@59.940Hz`; 60 000 → `…@60.000Hz`.
  - `criteria_fall_back_to_the_name_when_make_model_serial_collide` — two heads, same make/model, empty serial → `DP-1`, `DP-2`; with serials → `"Dell Inc. U2419H ABC"`.
  - `right_of_aligns_tops_below_aligns_left` — 1920x1080 origin, 1280x1024 `RightOf` → (1920,0); `Below` → (0,1080).
  - `rotation_swaps_the_size` — R90 origin 1920x1080 then `RightOf` → (1080,0).
  - `left_of_is_shifted_to_zero` — B `LeftOf` A(1920 wide), B 1280 wide → A (1280,0), B (0,0).
  - `overlap_is_refused` — two heads both `RightOf` the origin → `Err` naming both.
  - `set_place_swaps_a_cycle` — A origin, B right of A; `set_place(A, RightOf B)` → B `Origin`, A `RightOf` B.
  - `a_disabled_anchor_passes_its_place_on` — C right of B, B right of A, B disabled → C at A's right edge.
  - `parse_round_trips_render` and `parse_refuses_a_hand_file` (`profile { output * enable }` without header → `Err`).
  - `profile_for_matches_the_connected_set_only` — profile {A,B} is not chosen for {A}.
  - `from_heads_reads_side_by_side_and_stacked` — (0,0)+(1920,0) → `RightOf`; (0,0)+(0,1080) → `Below`.
- [ ] **Step 2:** run `--lib kanshi::` → FAIL.
- [ ] **Step 3: Implement** to the signatures above.
- [ ] **Step 4:** PASS, clippy clean; Task 1's runtime case still green with the updated fixture (kanshi ignores the comment lines).
- [ ] **Step 5: Commit** `menu: kanshi profiles from a monitor layout`.

---

### Task 4: `labwc-scale` writes the menu output into `rc.xml`

**Files:**
- Modify: `overlay/usr/libexec/thinfloe/labwc-scale`; Test: `test/integration/test-labwc-scale.sh`

**Interfaces:**
- Produces (contract with Task 7): the menu writes the wanted output name (connector name, one line, may be empty) to `${TF_MENU_OUTPUT_FILE:-$XDG_RUNTIME_DIR/thinfloe-menu-output}`; `labwc-scale` reads it each poll, and after `labwc --reconfigure` writes the same name to `<that file>.applied` (atomic, temp + `mv`). Empty or missing file = no choice.

- [ ] **Step 1: Write failing cases** in `test-labwc-scale.sh` (stub labwc as today):
  - `output only, scale 1` — file says `HDMI-A-1` → composed `rc.xml` is a regular file whose `thinfloe-menu` rule holds `<action name="MoveToOutput" output="HDMI-A-1"/>` before `ToggleAlwaysOnBottom`; fonts unscaled; `.applied` says `HDMI-A-1`; the stub saw `--reconfigure`.
  - `output and scale 2` — both changes in one `rc.xml`.
  - `output cleared, scale 1` — the links come back (`[ -L rc.xml ]`), `.applied` is empty.
  - `no scale file at all` — the output file alone still works (today the script exits when `--wait` runs out; it must keep polling the output file).
  - `name with XML characters` (`a"<b`) → refused: no rule, a line on stderr, `.applied` not written.
  - every case: generated `rc.xml` has no `<keybind` and no `<default */>`.
- [ ] **Step 2:** `bash test/integration/test-labwc-scale.sh` → the new cases FAIL.
- [ ] **Step 3: Implement**: compose when `scale != 1 || output != ""`, restore links only when both are default; the output name must match `^[A-Za-z0-9._-]{1,64}$`.
- [ ] **Step 4:** PASS; `shellcheck -s sh` on the script clean.
- [ ] **Step 5: Commit** `labwc-scale: put the menu on the chosen output`.

---

### Task 5: `thinfloe-session` starts kanshi and undoes an unconfirmed layout

**Files:**
- Modify: `overlay/usr/bin/thinfloe-session`, `overlay/home/tfuser/.thinfloe-sessionrc` (comment: the menu manages kanshi now; a hand line still works); Test: `test/integration/test-session-lifecycle.sh`

**Interfaces:**
- Produces (contract with Task 6): marker `${XDG_RUNTIME_DIR}/thinfloe-layout-unconfirmed`, contents `prev` (restore `~/.config/kanshi/config.prev` over `config`) or `none` (remove `config`, there was none before).

- [ ] **Step 1: Write failing cases** (fake `kanshi` on `PATH` that records its argv and stays alive):
  - `kanshi started when the config exists` — after `.thinfloe-sessionrc`, once, as `kanshi -c "$HOME/.config/kanshi/config"`, in `HELPER_PIDS`.
  - `no config: no kanshi`.
  - `kanshi already running: session starts no second one` — sessionrc starts the fake kanshi → one instance (`pgrep -u "$(id -u)" -x kanshi`).
  - `unconfirmed layout is undone: prev` / `…: none` — marker present → `config` equals the old `config.prev` (or is gone), `config.prev` and the marker removed, before kanshi starts.
- [ ] **Step 2:** `bash test/integration/test-session-lifecycle.sh` → FAIL.
- [ ] **Step 3: Implement** just before labwc-scale starts (thinfloe-session:~268); keep the signal contract described in `main.md` §4 untouched (kanshi joins `HELPER_PIDS`, nothing else).
- [ ] **Step 4:** PASS; shellcheck clean.
- [ ] **Step 5: Commit** `session: start kanshi for the menu's monitor layout`.

---

### Task 6: Screen-tab monitor state (`monitors.rs`) and its jobs

**Files:**
- Create: `src/thinfloe-menu/src/monitors.rs`; Modify: `menu.rs`, `jobs.rs`, `settings.rs`, `app.rs` (wiring `Heads` → `Wake::Heads`)

**Interfaces:**
- Consumes: `heads::Head`, `kanshi::*`, `owned::*`, Task 5 marker contract, Task 4 output-file contract.
- Produces:
  - `pub enum MonEvent { Select(String), Enabled(bool), Mode(usize), Rotation(Rotation), Place(Side, String), MenuHere, Apply, Keep, GoBack, TakeOver }` (`String` = criteria); `SettingsEvent::Monitors(MonEvent)`.
  - `pub enum MonRequest { Load, Apply { text: String, menu_output: Option<String> }, Keep, Revert, TakeOver }` → `Job::MonLoad | MonApply{..} | MonKeep | MonRevert | MonTakeOver` → `JobResult::MonLoaded(Result<Option<String>, String> /* file text */, FileOwner) | MonApplied(Result<(), String>) | MonKept | MonReverted(Result<(), String>) | MonTakenOver(Result<(), String>)`.
  - `pub struct MonTab` with `pub fn heads_changed(&mut self, heads: Vec<Head>) -> Option<MonRequest>`, `pub fn event(&mut self, e: MonEvent, now: Instant) -> Option<MonRequest>`, `pub fn tick(&mut self, now: Instant) -> Option<MonRequest>` (countdown), `pub fn menu_output(&self) -> Option<&str>` (connector name wanted now), `pub fn countdown(&self, now: Instant) -> Option<u32>` (seconds left), plus read accessors the UI needs.
  - `pub const KEEP_SECS: u64 = 15;`
- Job behaviour (`jobs.rs`): **MonApply** — read old bytes; write `config.prev` (or nothing) and the marker (`prev`/`none`); `owned::write` the config; `kanshictl reload`, and when that fails because kanshi does not run, start `kanshi -c <config>` in its own session; any failure → put the old file back (`owned::put_back`), remove marker/prev, `Err(reason)`. **MonKeep** — remove marker and `config.prev`. **MonRevert** — put `config.prev` back (or remove), reload, remove marker.

- [ ] **Step 1: Write failing tests** in `monitors.rs`:
  - `first_heads_choose_the_saved_profile_else_the_current_layout`.
  - `the_menu_monitor_cannot_be_switched_off` — `Enabled(false)` on it is ignored and `off_allowed()` is false; the last enabled monitor neither.
  - `apply_sends_the_rendered_text_and_starts_the_countdown` — `countdown(now)==Some(15)`, `countdown(now+10s)==Some(5)`.
  - `no_answer_reverts_after_15_seconds` — `tick(now+15s)==Some(MonRequest::Revert)`; `Keep` before that → `MonRequest::Keep` and no revert.
  - `hotplug_during_the_countdown_still_reverts` (Review Focus 3).
  - `a_missing_menu_monitor_clears_the_choice_once` (Review Focus 4) — `menu_output()` becomes `None`; a second identical `heads_changed` gives no request and no change.
  - `a_hand_file_is_read_only` (Review Focus 5) — `MonLoaded(Ok(Some(text)), ByHand)` → every edit event ignored, `TakeOver` → `MonRequest::TakeOver`.
  - `overlap_shows_an_error_and_sends_nothing`.
  - jobs (in `jobs.rs` tests, temp home, fake `kanshictl` via an env override as `TF_NETWORK_CMD` does): `mon_apply_writes_prev_marker_and_config`, `mon_apply_failure_puts_the_old_file_back`, `mon_revert_restores_prev`, `mon_keep_removes_marker_and_prev`.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: Implement**; while ThinLinc runs (`addon_id == "10-thinlinc"` among running jobs, as `remote_running` in `menu.rs:299`), `MonTab::thinlinc_note()` is true.
- [ ] **Step 4:** whole crate PASS, clippy clean.
- [ ] **Step 5: Commit** `menu: monitor layout state, apply and 15-second revert`.

---

### Task 7: The menu moves to its monitor by replacing its window

**Files:**
- Modify: `src/thinfloe-menu/src/app.rs`; Test: `test/runtime/test-labwc-runtime.sh`

**Interfaces:**
- Consumes: `MonTab::menu_output()`, Task 4 contract.
- Produces: `App::replace_window(&mut self, event_loop)` — the body of `resumed` (app.rs:823) moves into `fn create_live(...) -> Live`; replacing drops the old `Live` and creates a new one; jobs, log and menu state are untouched. `TF_MENU_DEBUG_MOVE=<name>`: at start the menu acts as if *Show the menu here* chose `<name>` (test hook, documented in the debug section of `main.rs`).

- [ ] **Step 1: Write failing runtime case** `menu moves to HEADLESS-2 after a change`: composed config dir + `labwc-scale` as the session starts them, HEADLESS-2 1024x768, menu with `TF_MENU_DEBUG=1 TF_MENU_DEBUG_MOVE=HEADLESS-2` → first `configure 1280x720`, then `configure 1024x768`, and `.applied` says `HEADLESS-2`. A second case: `TF_MENU_OUTPUT_FILE` never answered (labwc-scale not started) → the menu replaces its window after 3 s and logs `The menu could not be moved to HEADLESS-2.`
- [ ] **Step 2:** `make … test-runtime` → FAIL.
- [ ] **Step 3: Implement**: when `menu_output()` changes, write the output file (atomic) and note the time; each tick, read `.applied`; replace the window when it equals the wanted name or 3 s have passed (then the log line above); only once per change.
- [ ] **Step 4:** PASS, and `cargo test` still green.
- [ ] **Step 5: Commit** `menu: replace the window to move to the chosen monitor`.

---

### Task 8: Drawing the monitors (`monitors_ui.rs`)

**Files:**
- Create: `src/thinfloe-menu/src/monitors_ui.rs`; Modify: `ui.rs` (`screen_tab`, ui.rs:884), `brand.rs` (style guard list)

**Interfaces:**
- Consumes: `MonTab` accessors, `MonEvent`.
- Produces: `pub fn monitors_band(ui: &mut Ui, mon: &MonTab, now: Instant, width: f32) -> Option<MonEvent>` below the idle-timeout card; `pub fn dialog_open(mon: &MonTab) -> bool`.

Copy (exact): band title `Monitors`; rows `On`, `Resolution`, `Rotation`, `Position`; rotation choices `Normal`, `90°`, `180°`, `270°`; sides `Left of`, `Right of`, `Above`, `Below`; button `Show the menu here`; buttons `Apply`; dialog title `Keep this arrangement?`, body `Going back in N seconds.`, buttons `Keep`, `Go back`; ThinLinc note `ThinLinc uses the new arrangement from its next start.`; hand file `The monitor arrangement is set by hand in ~/.config/kanshi/config.` with `Let the menu manage it`. Each box: output name and model, the menu monitor marked with the Lucide `MONITOR` icon and `Menu`. Mode labels `1920 × 1080 @ 60 Hz` (`@ 59.94 Hz` when not whole).

- [ ] **Step 1: Write failing UI tests** in `ui.rs` (pattern: `the_network_tab_lists_connections`, `escape_closes_the_network_dialogs`): boxes in the right proportions (2560x1440 box is 4/3 the width of a 1920x1080 one at the same scale); a click on a box sends `Select`; `On` is disabled for the menu monitor; the dialog shows the countdown and `Keep`/`Go back` send their events; Escape = `Go back`; ThinLinc note shown; hand-file view; no rounded shape (the existing check covers the new file).
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** PASS; shots `TF_SHOT_DIR=<dir> cargo test --locked -j 4 --lib shot_ -- --ignored` gain `screen-monitors-two`, `screen-monitors-keep`, `screen-monitors-hand`; look at them yourself (`pnmtopng`).
- [ ] **Step 5: Commit** `menu: Screen tab arranges the monitors`.

---

### Task 9: Docs

**Files:** `CHANGES.md` (Unreleased → New; extend the Screen-tab entry or add one entry of at most three sentences), `CLAUDE.md` (kanshi now started by the session and managed by the menu; the output file contract), `.thinfloe-sessionrc` comment if not done in Task 5.

- [ ] **Step 1:** write; **Step 2:** commit `docs: monitor arrangement`.

---

### Task 10: Acceptance

- [ ] `make VERSION=monitor-arrangement test` rc=0 and `make … test-runtime` rc=0 through the build lock; counts reported with the delta against `main`.
- [ ] Shots looked at; owner tests on hardware (two real monitors, hotplug, DPMS wake — spec "Not verified").
