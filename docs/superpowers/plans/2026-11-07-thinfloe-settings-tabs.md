# Settings tabs and power-save Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Settings screen gets a tab bar; today's add-on list becomes the Packages tab, and a new Screen tab sets when the screen blanks.

**Architecture:** A small `owned` module holds the file-ownership rule that Parts B, C and D share. A `screentab` module reads and writes `~/.config/thinfloe/screen`. The menu state machine (`menu.rs`) gets the tab and the new events; the file write runs in `app.rs`, which then signals the idle helper and queues a home save. `thinfloe-session` moves swayidle into a background helper loop that restarts swayidle on `SIGUSR1`.

**Tech Stack:** Rust (egui 0.37, shadcn-egui 0.8 `Tabs` and `Select`), POSIX sh (dash), bash test scripts.

**Spec:** `docs/superpowers/specs/2026-11-02-thinfloe-settings-and-writer-design.md`, §"Part B: Settings tabs and power-save" and §"Who owns which file".

## Global Constraints

- The owner header line, exact: `# Written by the ThinFloe menu. Edits here are replaced.`
- Hand-written file text, exact: `This is set by hand in ~/<file>`; button: `Let the menu manage it`; the rename goes to `<file>.by-hand`.
- Power-save choices: never, 1, 2, 5, 10, 15, 30, 60 minutes; default 5 minutes (300 s).
- Storage: `idle_timeout=<seconds>` in `~/.config/thinfloe/screen`, owned by the menu.
- `TF_IDLE_TIMEOUT` (and its old name `DBRRG_IDLE_TIMEOUT`) set after `~/.thinfloe-sessionrc` wins; the select is then disabled with `set in ~/.thinfloe-sessionrc`.
- The menu never parses a shell file. swayidle stays in `thinfloe-session`. `wlopm` blanks; never `wlr-randr --off`.
- After a change the menu queues a background home save (the Save-On-Exit queue). No save when this boot's home restore failed.
- Style: lyra, nothing rounded, no colour or size literal in UI code (the grep guard with its positive control stays clean), body font Ubuntu Light.
- Machine: at most 4 cores; menu tests under `systemd-run --user --scope -q -p MemoryMax=4G --`; builds `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=<tag> …` with `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests`, the same `VERSION` on every make call.

## Calls made in this plan (not in the spec; the owner may overrule)

1. **Network tab comes with Part C.** Part B ships the tabs Packages and Screen. An empty Network tab on `main` would only show a placeholder; `SettingsTab` is an enum, so Part C adds one variant.
2. **No tallest-tab measuring.** The Settings panel already fills the fixed area above the log, so its height does not change with the tab. OxuLnk's `tallest_tab` is not needed.
3. **The signal goes to an idle helper, not to the session shell.** dash runs a trap only after the foreground command ends, so a `SIGUSR1` to the session would wait until the menu exits. Running the menu in the background instead would make it, and every program it starts, ignore `SIGINT` (Ctrl-C in a terminal would stop working). So the session starts a background idle loop and exports its PID as `TF_IDLE_PID` (the spec named it `TF_SESSION_PID`).
4. **Layout of the screen:** a title row `Settings` with `Back` on the right, the tab bar under it at full width, then the tab body. The Packages body starts with the `last checked` line and `Check now`. Settings always opens on Packages.
5. **A value that is not in the list** (for example `idle_timeout=420`) shows as an extra entry (`7 minutes`; `450 seconds` when it is not whole minutes) at its sorted place, so the select never shows a value that is not in force. Valid values: 0 to 86400 seconds, digits only. Anything else counts as not set (default 300), in the menu and in the session alike.
6. **`<file>.by-hand` already there:** *Let the menu manage it* refuses with `~/<file>.by-hand exists already` on the error line and changes nothing.
7. **The file write runs on the UI thread**, like the retry marker: a few bytes into the home. Write to `<file>.tmp`, then rename.
8. **No `TF_IDLE_PID`, or the signal fails:** the file is still written and saved, and the log says `The new screen timeout applies from the next session.`

## Review Focus

1. Two changes in quick succession: afterwards exactly one swayidle runs, with the last value (no orphan). → Task 3, `two signals leave one swayidle`.
2. `~/.config/thinfloe/` does not exist on a fresh home: the first change creates it. → Task 1, `write_creates_missing_dirs`.
3. A hand-edited menu file with a value outside the list or with garbage: the menu shows what the session uses. → Task 2, `parse_idle_*`, and Task 3, `garbage in the file means 300`.
4. Restore failed in this boot: a change writes the file but starts no save, and says why. → Task 4, `screen_change_does_not_save_after_failed_restore`.
5. The person ends the session while swayidle runs under the helper: both are gone after the session. → Task 3, `idle helper and swayidle end with the session`.

---

### Task 1: File ownership (`owned.rs`)

**Files:**
- Create: `src/thinfloe-menu/src/owned.rs`
- Modify: `src/thinfloe-menu/src/lib.rs` (add `pub mod owned;`)

**Interfaces:**
- Produces:
  - `pub const HEADER: &str` (the exact header line above)
  - `pub enum Owner { Missing, Menu, ByHand }` (`Debug, Clone, Copy, PartialEq, Eq`)
  - `pub fn owner(text: Option<&str>) -> Owner`: `None` is `Missing`; text whose first line equals `HEADER` is `Menu`; anything else, an empty file too, is `ByHand`. (Part C adds its template rule for `wifi.yaml` on top.)
  - `pub fn read(path: &Path) -> io::Result<Option<String>>`: `Ok(None)` when missing; uses `crate::bounded::read_bounded` with a 64 KiB cap.
  - `pub fn write(path: &Path, body: &str) -> io::Result<()>`: refuses with `io::ErrorKind::PermissionDenied` when `owner(read(path)?)` is `ByHand`; creates missing parent dirs; writes `HEADER\n` + `body` to `<path>.tmp`, `fsync`, rename.
  - `pub fn take_over(path: &Path) -> Result<Option<String>, String>`: renames `path` to `<path>.by-hand` and returns the old text; `Err("~/<name>.by-hand exists already")` when the target exists (the message uses the path relative to `$HOME`, through `pub fn home_relative(path: &Path) -> String`).

- [ ] **Step 1: Write the failing tests** in `owned.rs` `#[cfg(test)] mod tests`, using `crate::testdir`:
  - `owner_of_missing_menu_and_hand_files`: `owner(None) == Missing`; `owner(Some("# Written by the ThinFloe menu. Edits here are replaced.\nidle_timeout=60\n")) == Menu`; `owner(Some("idle_timeout=60\n")) == ByHand`; `owner(Some("")) == ByHand`; a header in line 2 is `ByHand`.
  - `write_creates_missing_dirs`: `write(dir/".config/thinfloe/screen", "idle_timeout=60\n")` succeeds and the file reads `HEADER + "\nidle_timeout=60\n"`; no `.tmp` file is left.
  - `write_refuses_a_hand_file`: a file `idle_timeout=60\n` stays byte-for-byte after `write` returns `PermissionDenied`.
  - `write_replaces_a_menu_file_edited_later`: header plus an extra line by hand is still replaced.
  - `take_over_renames_and_returns_text`, and `take_over_refuses_when_by_hand_exists` (both files unchanged, the error text ends in `.by-hand exists already`).
- [ ] **Step 2: Run, expect FAIL** (missing module): `cd src/thinfloe-menu && systemd-run --user --scope -q -p MemoryMax=4G -- cargo test --locked -j 4 owned::`
- [ ] **Step 3: Implement** the interface above.
- [ ] **Step 4: Run the same command, expect PASS.**
- [ ] **Step 5: Commit** `menu: a file the menu owns starts with its header line`.

### Task 2: The screen settings file (`screentab.rs`)

**Files:**
- Create: `src/thinfloe-menu/src/screentab.rs`
- Modify: `src/thinfloe-menu/src/lib.rs`

**Interfaces:**
- Consumes: `owned::{Owner, owner, read}` from Task 1.
- Produces:
  - `pub const SCREEN_FILE: &str = ".config/thinfloe/screen";` (relative to `$HOME`)
  - `pub const DEFAULT_IDLE: u32 = 300;` `pub const MAX_IDLE: u32 = 86400;`
  - `pub const CHOICES: [u32; 8] = [0, 60, 120, 300, 600, 900, 1800, 3600];`
  - `pub enum IdleSource { Rc, Menu, ByHand, Unset }`
  - `pub struct ScreenTab { pub idle: u32, pub source: IdleSource, pub error: Option<String> }` (`Debug, Clone, PartialEq, Eq`)
  - `pub fn parse_idle(text: &str) -> Option<u32>`: the last line `idle_timeout=<digits>` with a value `<= MAX_IDLE`; anything else on that line makes it `None`.
  - `pub fn load(path: &Path, rc_set: bool) -> ScreenTab`: `rc_set` gives `source: Rc` (the value is not needed, the select is disabled). Else `owned::owner` decides `Menu`, `ByHand` or `Unset`; `idle` is `parse_idle` or `DEFAULT_IDLE`. A read error goes to `error`.
  - `pub fn body(idle: u32) -> String`: `"idle_timeout=<idle>\n"`.
  - `pub fn options(current: u32) -> Vec<u32>`: `CHOICES`, plus `current` at its sorted place when it is not in it (0 stays first).
  - `pub fn label(secs: u32) -> String`: `0` → `Never`, `60` → `1 minute`, `n*60` → `n minutes`, else `n seconds`.

- [ ] **Step 1: Write the failing tests:**
  - `parse_idle_takes_the_last_valid_line`: `"idle_timeout=60\nidle_timeout=120\n"` → `Some(120)`; `"idle_timeout=0"` → `Some(0)`.
  - `parse_idle_refuses_garbage`: `"idle_timeout=-5"`, `"idle_timeout=5m"`, `"idle_timeout=86401"`, `"idle_timeout= 60"`, `""` → `None`.
  - `load_without_file_is_default_unset`: `idle == 300`, `source == Unset`.
  - `load_with_rc_set_is_rc` even when a menu file exists.
  - `load_hand_file_shows_its_value`: `idle_timeout=900` without header → `idle == 900`, `ByHand`.
  - `options_insert_an_odd_value`: `options(420)` == `[0,60,120,300,420,600,900,1800,3600]`; `options(300)` == `CHOICES`.
  - `labels`: `label(0) == "Never"`, `label(60) == "1 minute"`, `label(300) == "5 minutes"`, `label(450) == "450 seconds"`.
- [ ] **Step 2: Run, expect FAIL:** `… cargo test --locked -j 4 screentab::`
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS.**
- [ ] **Step 5: Commit** `menu: read and write ~/.config/thinfloe/screen`.

### Task 3: The idle helper in `thinfloe-session`

**Files:**
- Modify: `overlay/usr/bin/thinfloe-session` (the screen blanking block, lines 57–88)
- Test: `test/integration/test-session-lifecycle.sh`

**Interfaces:**
- Produces for the menu (environment, exported before the menu starts):
  - `TF_IDLE_PID=<pid>` of the idle helper, only when the helper runs (swayidle and wlopm both found).
  - `TF_IDLE_TIMEOUT_RC=1` when `TF_IDLE_TIMEOUT` or `DBRRG_IDLE_TIMEOUT` is set after `.thinfloe-sessionrc`; unset otherwise.
- Behaviour: a shell function `tf_idle_loop`, run with `&` and added to `HELPER_PIDS`. Each round it finds the timeout (rc value when `TF_IDLE_TIMEOUT_RC=1`, else `idle_timeout=` from `$HOME/.config/thinfloe/screen` with the same rule as `parse_idle`, else 300), starts `swayidle -w timeout <t> 'wlopm --off \*' resume 'wlopm --on \*'` (or, for 0, `sleep 2147483647`) in the background and `wait`s for it. Trap `USR1`: set a flag and kill the child, so the next round re-reads the file. Trap `TERM`: kill the child and exit. A child that ended without `USR1` waits 5 s before the next round, so a crashing swayidle does not spin.
- The comment block keeps its explanation (wlopm vs wlr-randr, no locking, Xwayland) and says why the helper is a loop of its own (call 3 above).

- [ ] **Step 1: Write the failing tests** in `test-session-lifecycle.sh`. Replace the `swayidle` stub with one that writes `"$*"` to `$WORK/idle.<pid>` and `exec sleep 30`; a menu stub drives each case:
  - `default timeout is 300`: the stub menu waits until an `idle.*` file exists (max 2 s), copies its content and `$TF_IDLE_PID` and `${TF_IDLE_TIMEOUT_RC:-}` to `$WORK/seen`; expect `timeout 300` in the args, a numeric PID, an empty RC flag.
  - `the menu file sets the timeout`: `idle_timeout=600` in `$WORK/home/.config/thinfloe/screen` → `timeout 600`.
  - `garbage in the file means 300`: `idle_timeout=5m` → `timeout 300`.
  - `sessionrc wins`: `TF_IDLE_TIMEOUT=900` in `.thinfloe-sessionrc` plus the file at 600 → `timeout 900`, `TF_IDLE_TIMEOUT_RC=1`; same for `DBRRG_IDLE_TIMEOUT=900`.
  - `zero starts no swayidle`: file `idle_timeout=0` → no `idle.*` file after 1 s, `TF_IDLE_PID` still set.
  - `two signals leave one swayidle`: the stub menu writes 120, sends `kill -USR1 $TF_IDLE_PID`, writes 1800, sends it again, waits 1 s, then lists live PIDs of `idle.*` files; expect exactly one live, with `timeout 1800`.
  - `idle helper and swayidle end with the session`: after the session returns, neither `TF_IDLE_PID` nor any swayidle stub PID is alive (0.3 s grace, as the scale-helper case does).
  - Every existing case still passes (`sudo`, exit codes, scale helper, no saves).
- [ ] **Step 2: Run, expect the new cases to FAIL:** `test/integration/test-session-lifecycle.sh`
- [ ] **Step 3: Implement** the helper in `thinfloe-session`.
- [ ] **Step 4: Run, expect all `ok`, no `FAIL`.** Also `sh -n overlay/usr/bin/thinfloe-session` and, if installed, `shellcheck -s sh` on it with no new warnings.
- [ ] **Step 5: Commit** `session: swayidle restarts with a new timeout on SIGUSR1`.

### Task 4: Tabs and the Screen events in the menu state machine

**Files:**
- Modify: `src/thinfloe-menu/src/settings.rs` (`SettingsEvent`)
- Modify: `src/thinfloe-menu/src/menu.rs` (state, `settings()`, new methods, tests)

**Interfaces:**
- Consumes: `screentab::{ScreenTab, IdleSource, body}` (Task 2), `owned::HEADER` only through `screentab`.
- Produces:
  - `pub enum SettingsTab { Packages, Screen }` in `settings.rs`, with `pub const ALL: [SettingsTab; 2]` and `pub fn label(self) -> &'static str` (`Packages`, `Screen`).
  - New `SettingsEvent` variants: `Tab(SettingsTab)`, `IdleTimeout(u32)`, `ManageScreen`.
  - New `Effect` variants: `WriteScreen { idle: u32 }`, `TakeOverScreen`.
  - `Menu` fields: `pub settings_tab: SettingsTab` (set to `Packages` whenever the Settings tile opens the screen), `pub screen_tab: ScreenTab` (set by `main.rs` at start).
  - `pub fn screen_written(&mut self, result: Result<ScreenTab, String>, signalled: bool) -> Option<Effect>`: on `Ok` stores the tab, logs `Screen turns off after <label>.` (or `Screen never turns off.`), logs the `next session` line from call 8 when `!signalled`, and returns `request_save()`; after a failed restore it logs `SaveOutcome::RestoreFailed.message()` as `Kind::Warn` and returns `None`. On `Err` stores the message in `screen_tab.error` and logs it as `Kind::Warn`.
- Rules in `settings()`: `IdleTimeout` returns `None` when the source is `Rc` or `ByHand`, or the value equals the current one; `ManageScreen` only when the source is `ByHand`. Both are refused during a logout, as now.

- [ ] **Step 1: Write the failing tests** in `menu.rs` tests:
  - `settings_opens_on_packages`: open Settings, `Tab(Screen)`, Back, open again → `settings_tab == Packages`.
  - `idle_change_asks_for_a_write`: source `Menu`, idle 300; `IdleTimeout(600)` → `Some(Effect::WriteScreen { idle: 600 })`; `IdleTimeout(300)` → `None`.
  - `idle_change_refused_when_rc_or_by_hand`.
  - `screen_written_queues_one_save`: two `screen_written(Ok(..), true)` while a save runs → `save.again == true`, one `Effect::Start(Job::Save)` in total.
  - `screen_change_does_not_save_after_failed_restore`: `Menu::new(grid, true)`; `screen_written(Ok(..), true)` → `None`, log has the restore warning.
  - `screen_written_without_signal_says_next_session`.
  - `screen_write_error_shows_on_the_tab`: `Err("disk full".into())` → `screen_tab.error == Some("disk full")`, no save.
- [ ] **Step 2: Run, expect FAIL:** `… cargo test --locked -j 4 --lib menu::`
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS**; the whole `cargo test --locked -j 4` stays green.
- [ ] **Step 5: Commit** `menu: Settings tabs and the screen timeout events`.

### Task 5: Wiring in `main.rs` and `app.rs`

**Files:**
- Modify: `src/thinfloe-menu/src/main.rs` (`build_menu`)
- Modify: `src/thinfloe-menu/src/app.rs` (`apply`, config)

**Interfaces:**
- Consumes: `screentab::load`, `owned::{write, take_over}`, `Menu::screen_written`, the `Effect` variants of Task 4, the environment of Task 3.
- Produces:
  - `pub fn idle_pid(v: Option<&OsStr>) -> Option<i32>` in `screentab.rs`: digits only, `> 1`.
  - `pub fn screen_path() -> PathBuf` in `main.rs`: `$HOME/.config/thinfloe/screen`, overridable by `TF_SCREEN_FILE` (for tests), the way `env_path` works.
  - `app.rs` keeps `screen_path: PathBuf`, `idle_pid: Option<i32>`, `rc_set: bool`.
  - `WriteScreen { idle }`: `owned::write(&screen_path, &screentab::body(idle))`; on success `libc::kill(pid, SIGUSR1) == 0` gives `signalled`; then `menu.screen_written(Ok(screentab::load(..)), signalled)` and apply the returned effect. `TakeOverScreen`: `owned::take_over`, then `WriteScreen` with the value read from the old text (`parse_idle`, else `DEFAULT_IDLE`).
  - `build_menu` sets `menu.screen_tab = screentab::load(&screen_path(), env TF_IDLE_TIMEOUT_RC == "1")`.

- [ ] **Step 1: Write the failing tests:** `idle_pid_parses_only_pids` (`"1234"` → `Some(1234)`, `"1"`, `"0"`, `"-5"`, `"12a"`, empty, missing → `None`) in `screentab.rs`; and, in the `app.rs` tests next to the existing ones, `write_screen_writes_signals_and_saves`: with `TF_SCREEN_FILE` in a test dir and `idle_pid` pointing at a `sleep 5` child that traps nothing (so the default action ends it), apply `WriteScreen { idle: 120 }` and expect the file content, the child ended by `SIGUSR1`, and `Job::Save` started. If `app.rs` has no harness that can take an `Effect` without a window, move the effect handling into a free function `fn apply_screen(effect, &paths, &mut Menu) -> Option<Effect>` and test that.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the whole crate's tests, expect PASS.**
- [ ] **Step 5: Commit** `menu: write the screen file, tell the idle helper, save the home`.

### Task 6: The tab bar and the Screen tab on screen

**Files:**
- Modify: `src/thinfloe-menu/src/ui.rs` (`show_settings`, new `packages_tab`, `screen_tab`, tests, shot tests)
- Modify: `src/thinfloe-menu/src/brand.rs` only if a new size or width constant is needed (for example the form label width)
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `SettingsTab`, `SettingsEvent` variants (Task 4), `screentab::{options, label, IdleSource}`.
- Produces: `fn packages_tab(ui, menu, icons, width) -> Option<SettingsEvent>` (today's body, moved unchanged), `fn screen_tab(ui, menu) -> Option<SettingsEvent>`.
- Screen tab content: one form row, label `Turn the screen off after` (fixed label width from `brand.rs`), a `Select` over `options(idle)` with `label()` texts. Disabled with the muted line `set in ~/.thinfloe-sessionrc` when the source is `Rc`. For `ByHand`: the select disabled, the line `This is set by hand in ~/.config/thinfloe/screen`, and a secondary button `Let the menu manage it` (→ `ManageScreen`). `screen_tab.error` shows as one warn-coloured line under the row.

- [ ] **Step 1: Write the failing tests** next to the Settings tests in `ui.rs`:
  - `settings_shows_both_tabs`: the text shapes contain `Packages` and `Screen`, and `Back`.
  - `the_screen_tab_shows_the_timeout`: `settings_tab = Screen`, idle 300 → `Turn the screen off after` and `5 minutes` are drawn.
  - `the_screen_tab_says_sessionrc`: source `Rc` → `set in ~/.thinfloe-sessionrc` is drawn.
  - `the_screen_tab_offers_take_over`: source `ByHand` → the hand line and `Let the menu manage it`.
  - `settings_draws_nothing_rounded` covers both tabs (loop over `SettingsTab::ALL`); `the_settings_screen_draws_a_row_per_addon` still passes.
  - Extend `shot_desktop_and_settings_at_densities` to also render the Screen tab.
- [ ] **Step 2: Run, expect FAIL:** `… cargo test --locked -j 4 --bin thinfloe-menu -- settings` (main.rs tests are in `--bin thinfloe-menu`; check where the `ui.rs` tests land and use that target).
- [ ] **Step 3: Implement.** Tab bar: `Tabs::new(&mut idx, &labels).width(f32::INFINITY)` under the title row; a change returns `SettingsEvent::Tab`.
- [ ] **Step 4: Run, expect PASS**; run the literal grep guard with its positive control.
- [ ] **Step 5: Add to `CHANGES.md` under `### New`:** `The Settings screen has two tabs: Packages, the add-on list, and Screen, where "Turn the screen off after" sets when the screen blanks (never, or 1 to 60 minutes; 5 by default). A change applies at once and is saved with the home directory; TF_IDLE_TIMEOUT in ~/.thinfloe-sessionrc still wins and then shows there instead, and a ~/.config/thinfloe/screen written by hand is shown but not changed until you choose "Let the menu manage it".`
- [ ] **Step 6: Commit** `menu: Settings tabs Packages and Screen`.

### Task 7: Acceptance

- [ ] **Step 1:** Render the shots (`TF_SHOT_DIR` under `/scratch/oetiker/claude-tmp/thinfloe-tests/b-shots/`), convert to PNG, check the mtimes, look at every picture yourself: tab bar, Packages unchanged, Screen tab in all three source states, at 1080p, 4K and 21-inch 4K.
- [ ] **Step 2:** `make VERSION=<tag> test-unit`, `make VERSION=<tag> test`, `make VERSION=<tag> test-runtime`, through the build lock. Expected: all green; the test count rises by the new tests only. Never while a subagent edits the tree.
- [ ] **Step 3:** Show the owner the pictures; ask before the fast-forward merge to `main` and the push.
