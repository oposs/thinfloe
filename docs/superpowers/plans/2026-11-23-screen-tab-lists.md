# Screen Tab Lists Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Screen tab's selection-dependent rows with one row per monitor and per Deskflow client, ⚙ dialogs for the rest, and a per-client On switch.

**Architecture:** The models (`monitors.rs`, `deskflowtab.rs`) keep deciding; "selected" now means "its dialog is open". Rows and dialogs are drawn by `monitors_ui.rs` and `deskflow_ui.rs` with `ui::dialog` (the Network tab's style: screen disabled underneath, Area on top). The daemon reads a new `enabled` flag from `generated/deskflow`.

**Tech Stack:** Rust, egui 0.37 with shadcn-egui (lyra), CPU rendering; the Deskflow daemon in `src/thinfloe-menu/src/deskflow/`.

**Spec:** `docs/superpowers/specs/2026-11-23-screen-tab-lists-design.md`

**Branch:** `feat/screen-tab-lists`, from `origin/main` @ `a498a3a` (v4.9.0, which carries the Compose switch). Its first commit is the Compose fix `f05936d`.

## Global Constraints

- UI colours from the theme, sizes from named constants in `brand.rs`; `ui_code_has_no_style_literals` must pass. shadcn-egui widgets, lyra, nothing rounded.
- Decisions in `monitors.rs` / `deskflowtab.rs`; `*_ui.rs` only paint and return events.
- The menu is the only writer of `[deskflow]`; the daemon reads only `generated/deskflow`. The state file and socket contract do not change.
- `enabled` defaults to `true` (serde default), so existing homes keep working clients.
- No new Cargo dependency.
- Cargo on this host: `CARGO_BUILD_JOBS=4`, `--test-threads=4`. Clippy: `make test-menu` (the old `-A clippy::chunks-exact-to-as-chunks` advice broke host clippy).
- Texts, verbatim: "Share the clipboard", "Handle Compose key on ThinFloe", "Let other computers use this keyboard and mouse (Deskflow)", "Check this fingerprint on the other computer before you trust it there.", "This edge is not free in the current arrangement.", "No monitor edge is free in the current arrangement.", "Fingerprint…", "Check and attach…", "Reject", "Attach", "Cancel", "Close", "Forget <name>", "Edge resistance", "Show the menu here", "Position", "Rotation", "Resolution", "On", "Clipboard", "Compose", column heads "Monitor", "Computer".

## Review Focus

1. A client turned off while its pointer is on it (config reload while active): the daemon must release back to ThinFloe at once, not wait for the client. Test in Task 1.
2. Opening one dialog while another is open (a box click with the client dialog up is impossible because the screen is disabled, but events from the model can arrive): exactly one dialog is open. Test in Task 3.
3. A monitor that is unplugged while its dialog is open: the dialog closes, no panic, no edit applies to a missing output. Test in Task 2.
4. A client forgotten from its own dialog: the dialog closes and the row goes. Test in Task 3.
5. A screen at 1920x1080 with two monitors, two clients and a pending client: the client rows are in view without scrolling (today's Position-row test adapted). Test in Task 5.

---

### Task 1: The per-client On switch, config to daemon

**Files:**
- Modify: `src/thinfloe-menu/src/config.rs` (`DeviceCfg`), `src/thinfloe-menu/src/deskflowcfg.rs` (render/parse), `src/thinfloe-menu/src/deskflowtab.rs` (`DeskflowEvent`, `apply`, re-attach in `Place`), `src/thinfloe-menu/src/deskflow/pair.rs:131` (new device: `enabled: true`), `src/thinfloe-menu/src/deskflow/run.rs` (`wanted_strips`, `shares_clipboard`, `update_strips`)
- Test: same files' `tests` modules

**Interfaces:**
- Produces: `DeviceCfg.enabled: bool` (`#[serde(default = "default_true")]`); `DeskflowEvent::Enabled { name: String, on: bool }` handled by `set_device_flag(.., |d| &mut d.enabled)`.

- [ ] **Step 1: Write the failing tests**
  - `deskflowcfg::tests::enabled_defaults_true_and_roundtrips`: a device text without `enabled` parses to `enabled == true`; `enabled = false` parses to false, survives `render` -> `parse`, and `generated`'s parse keeps it.
  - `deskflowtab::tests::on_switch_writes_device_and_survives_reattach`: `DeskflowEvent::Enabled { name: "mac", on: false }` returns `Some(DeskflowRequest::Save(cfg))` with `cfg.devices[mac].enabled == false`; re-attaching "mac" with a new fingerprint keeps `enabled == false` (as the compose test at `deskflowtab.rs:594`).
  - `run.rs` tests (next to the existing strip tests): an off device gets no strip but its connection stays in `conns` and trusted; `shares_clipboard` is false for it; with `Mode::Active` on that device, a reload that sets `enabled = false` leaves `Mode::Idle` and logs release reason `"the device is switched off"`.
- [ ] **Step 2: Run them, expect FAIL** (`cargo test --lib -- --test-threads=4 enabled on_switch switched_off` in `src/thinfloe-menu`).
- [ ] **Step 3: Implement.** In `wanted_strips`, skip an off device after the device lookup and push its id to a fourth list `off`; `update_strips` maps a captured id in `off` to the reason `"the device is switched off"`. `shares_clipboard` and `clip_live` require `d.enabled`. `free_edges` in `deskflowtab.rs` stays as it is (an off device keeps its edge).
- [ ] **Step 4: Run the tests, expect PASS**; then the whole `deskflow::` and `deskflowtab::` suites.
- [ ] **Step 5: Commit** `feat(deskflow): per-device On switch; an off device keeps its connection and its edge`.

### Task 2: Monitor model: per-monitor row events, selection is the dialog

**Files:**
- Modify: `src/thinfloe-menu/src/monitors.rs` (`MonEvent`, `event`, `edit`, `off_allowed`, `selected_mode`, line 525 initial selection), callers in `src/thinfloe-menu/src/monitors_ui.rs` and `src/thinfloe-menu/src/menu.rs` tests
- Test: `monitors.rs` tests

**Interfaces:**
- Produces:
  - `MonEvent::Enabled(String, bool)` and `MonEvent::Mode(String, usize)`: act on the monitor with these criteria, not on the selection.
  - `MonEvent::CloseDialog`: `selected = None`.
  - `MonTab::off_allowed_for(&self, criteria: &str) -> bool`, `MonTab::mode_of(&self, criteria: &str) -> Option<usize>`; `off_allowed()` and `selected_mode()` go (no other users after Task 4) or become wrappers if a test still needs them.
  - `MonTab::dialog_monitor(&self) -> Option<&str>` = today's `selected()` (rename for meaning; keep `selected()` if renaming widens the diff too much, the implementer's call).
  - `Rotation`, `Place`, `MenuHere` still act on the selected monitor (the dialog's).
- Rules: a fresh load leaves `selected = None` (line 525 today picks the first output; drop that). A reload whose draft no longer has the selected criteria sets `selected = None`.

- [ ] **Step 1: Write the failing tests**
  - `row_events_act_on_their_monitor_without_a_selection`: with `selected == None`, `Mode("DP-2", 0)` changes DP-2's mode, `Enabled("DP-2", false)` turns DP-2 off; `Enabled("DP-1", false)` on the menu's monitor changes nothing (same rule as today's tests at `monitors.rs:802-830`).
  - `a_fresh_load_opens_no_dialog`: after load, `dialog_monitor() == None`.
  - `close_dialog_clears_the_selection` and `an_unplugged_monitor_closes_its_dialog` (Review Focus 3).
  - Adapt today's tests that send `Select` then `Mode(i)` / `Enabled(b)` to the new shapes; their assertions stay.
- [ ] **Step 2: Run, expect FAIL** (`cargo test --lib monitors:: -- --test-threads=4`).
- [ ] **Step 3: Implement**; `edit` takes the target criteria from the event for `Mode`/`Enabled`, from `selected` for the rest.
- [ ] **Step 4: Run `monitors::` and `menu::` tests, expect PASS** (`monitors_ui.rs` compiles with the new event shapes; drawing changes come in Task 4).
- [ ] **Step 5: Commit** `refactor(monitors): row events name their monitor; the selection is the open dialog`.

### Task 3: Deskflow model: dialogs, and one dialog at a time

**Files:**
- Modify: `src/thinfloe-menu/src/deskflowtab.rs`, `src/thinfloe-menu/src/menu.rs` (`SettingsEvent::Monitors` / `::Deskflow` handling near line 919)
- Test: `deskflowtab.rs` and `menu.rs` tests

**Interfaces:**
- Produces:
  - `DeskflowTab.selected` keeps its type and now means "this client's dialog is open"; `DeskflowTab.attaching` means "the attach dialog is open".
  - `DeskflowTab.fingerprint_open: bool`; `DeskflowEvent::ShowFingerprint` sets it.
  - `DeskflowEvent::CloseDialog`: `selected = None`, `attaching = None`, `fingerprint_open = false`.
  - `pub fn dialog_open(tab: &DeskflowTab) -> bool` in `deskflowtab.rs`.
  - `band_devices` goes: every attached device has a row now.
  - In `menu.rs`: a `MonEvent::Select` also applies `DeskflowEvent::CloseDialog`; a `DeskflowEvent::Select`, `Attach` or `ShowFingerprint` also applies `MonEvent::CloseDialog`.

- [ ] **Step 1: Write the failing tests**
  - `deskflowtab::tests::close_dialog_closes_every_dialog`.
  - `deskflowtab::tests::forget_closes_its_dialog` (Review Focus 4): Select "mac", Forget "mac" -> `selected == None`, device gone.
  - `deskflowtab::tests::a_place_from_the_attach_dialog_closes_it`: Attach, Place -> `attaching == None`, device attached (today's behaviour, pinned).
  - `menu::tests::one_screen_dialog_at_a_time` (Review Focus 2): open the monitor dialog, then `DeskflowEvent::Select("laptop")` -> `monitors.dialog_monitor() == None`; then `MonEvent::Select("DP-1")` -> `deskflow.selected == None`. Adapt the test at `menu.rs:4741`.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run `deskflowtab::` and `menu::`, expect PASS.**
- [ ] **Step 5: Commit** `refactor(deskflow): the band's selection is its dialog; one Screen dialog at a time`.

### Task 4: Monitors band: the list and the monitor dialog

**Files:**
- Modify: `src/thinfloe-menu/src/monitors_ui.rs` (`monitors_band`, `arrangement`, replace `selected_rows` by `monitor_rows` and `monitor_dialog`), `src/thinfloe-menu/src/ui.rs` (`show_settings`: disable the screen while a Screen dialog is open; `screen_tab`: draw the open dialog last), `src/thinfloe-menu/src/brand.rs` (any new width constant)
- Test: `ui.rs` tests (the monitor tests' window near line 7343)

**Interfaces:**
- Consumes: Task 2's events and `off_allowed_for`, `mode_of`, `dialog_monitor`; Task 3's `deskflowtab::dialog_open`.
- Produces: `pub fn screen_dialog_open(mon: &MonTab, df: &DeskflowTab) -> bool` in `ui.rs` (`mon.dialog_monitor().is_some() || deskflowtab::dialog_open(df)`), used by `show_settings` like the Network check at line 923. `monitors_ui::dialog_open` (Keep / Go back) stays untouched.
- Layout: under the picture, a table: "Monitor" (name, "· Menu" on the menu's monitor in primary colour), "Resolution" (Select, id `tf-mon-mode-<criteria>`), "On" (Switch, disabled when `!off_allowed_for` and the monitor is on), a ⚙ button (lucide settings icon). Hand-written file (`MonSource::Kept`): rows drawn disabled.
- Monitor dialog (`ui::dialog`, title = monitor name): Position (today's control), Rotation (today's select), "Show the menu here" (today's rules), "Close". Escape closes (`MonEvent::CloseDialog`), as `network_ui.rs:744`.
- Picture: a monitor box click sends `MonEvent::Select` (opens the dialog); the open dialog's box keeps the 2 px primary stroke.

- [ ] **Step 1: Write the failing tests** in `ui.rs`:
  - `every_monitor_shows_its_resolution_and_on_without_a_click`: two monitors, nothing selected -> both names and two "Resolution" selects drawn, no "Rotation" text.
  - `a_monitor_box_click_opens_its_dialog_and_escape_closes_it`: click the DP-2 box -> `MonEvent::Select("DP-2")`; with DP-2 selected the frame draws "Rotation" and "Show the menu here"; Escape -> `MonEvent::CloseDialog`.
  - `the_screen_under_a_screen_dialog_takes_no_click`: with a dialog open, a click on the idle select yields no event.
  - Adapt the tests that find "Position" / "Rotation" rows inline (`ui.rs` ~7343-7500) to open the dialog first.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run `ui::` and `ui_code_has_no_style_literals`, expect PASS.**
- [ ] **Step 5: Commit** `feat(menu): Screen tab lists the monitors, the rest in a dialog per monitor`.

### Task 5: Deskflow band: client list, dialogs, dimmed off clients

**Files:**
- Modify: `src/thinfloe-menu/src/deskflow_ui.rs` (`band`, replace `pending_row` / `device_rows` by `client_rows`, add `client_dialog`, `attach_dialog`, `fingerprint_dialog`), `src/thinfloe-menu/src/monitors_ui.rs` (`DevPlace.enabled`, `device_box` dimmed), `src/thinfloe-menu/src/ui.rs` (`screen_tab` draws the open Deskflow dialog)
- Test: `ui.rs` tests, `deskflowtab.rs` if a helper moves there

**Interfaces:**
- Consumes: Task 1's `DeskflowEvent::Enabled`, Task 3's `ShowFingerprint`, `CloseDialog`, `dialog_open`, `selected`, `attaching`, `fingerprint_open`.
- Produces: `DevPlace { enabled: bool, .. }`.
- Layout, top to bottom: the Listen switch; the stopped warning; one line with `self_line` text, "Fingerprint…" (only with `self_fingerprint`), and "Edge resistance" select at the right end (its own line when `self_line` is `None`); the client table: state dot (`device_state`: Active/Connected in `GOOD`-role colour, Offline muted), name, three Switches (On, Clipboard, Compose), ⚙; an off client's name in `muted_foreground`; under a row whose edge is not free, the `NOT_FREE` warning. Pending clients: a row with "<name> wants to attach" (or today's changed-fingerprint warning text), "Check and attach…", "Reject". The tab error last.
- Dialogs (`ui::dialog`): client (title = name): Position, the three switches, the client's colon hex, "Forget <name>", "Close"; attach (title = name): random art, colon hex, check note, Position select of free edges (or `NO_FREE_EDGE`), "Attach" disabled until an edge is chosen, "Cancel"; this computer (title "Fingerprint of this computer"): random art, colon hex, check note, "Close". Escape sends `CloseDialog` (attach: `CancelAttach`).
- Attach flow: the Position select in the attach dialog only records the chosen edge in the dialog (egui temp memory keyed by name+fp); "Attach" sends today's `DeskflowEvent::Place`.

- [ ] **Step 1: Write the failing tests** in `ui.rs`:
  - `every_client_shows_on_clipboard_and_compose_without_a_click`: two attached clients, none selected -> two rows, six switches, the header texts "Computer", "On", "Clipboard", "Compose".
  - `a_client_switch_sends_its_event`: click the Compose switch of "mac" -> `DeskflowEvent::Compose { name: "mac", on: true }`; the On switch -> `Enabled`.
  - `a_pending_client_attaches_through_its_dialog`: "Check and attach…" -> `Attach`; with `attaching` set, the frame shows "Attach" disabled; after choosing an edge, "Attach" -> `Place`.
  - `the_client_rows_are_in_view_on_a_1080_screen` (Review Focus 5): two monitors, two clients, one pending, full Graphics band, 1920x1080 -> every client row's Compose switch inside the clip rect. Replaces `the_position_row_is_in_view_on_a_1080_screen` (`ui.rs:4098`), whose Position row now lives in a dialog.
  - `an_off_client_box_is_dimmed`: the device box text of an off client uses `muted_foreground`.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the whole crate: `cargo test -- --test-threads=4`, `cargo clippy`, `cargo fmt --check`, expect PASS.**
- [ ] **Step 5: Commit** `feat(menu): Deskflow clients as a list with On, Clipboard and Compose; dialogs for the rest`.

### Task 6: Docs, changelog, gates

**Files:**
- Modify: `docs/notes/deskflow.md` (the band, the On switch and what off does), `docs/notes/menu.md` (the Screen tab layout, the dialogs), `CHANGES.md` (house style: reader-facing, three sentences max per entry, no release header).

- [ ] **Step 1: Write the docs and the CHANGES entries.** CHANGES needs: the monitors and Deskflow computers are lists with their main switches always visible and the rest behind ⚙; the new On switch per computer ("Off: the mouse stops at that edge; the computer stays connected and keeps its place").
- [ ] **Step 2: Gates**, chained in one shell: `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test-runtime VERSION=screen-lists && flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test VERSION=screen-lists`. Expected: exit 0, no `not ok`, no `FAIL`.
- [ ] **Step 3: Commit** `docs(menu): Screen tab lists and the Deskflow On switch`.
