# Peripheral Battery Tile Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show the keyboard and mouse batteries on a tile of `thinfloe-menu`, drop the ThinLinc export and the Settings switch.

**Architecture:** The reader `/usr/libexec/thinfloe/peripheral-battery` loses its subcommands and switch and always runs from its timer; it gains a lock so a click and the timer never talk to one receiver at once. The menu gets a module `battery.rs` (parse the file, tile text, log text), a shipped tile `70-battery.desktop` with `X-TF-Action=battery`, and turns the existing battery switch job into a read job: click, run the reader, read the file in the worker thread, log one line per device.

**Tech Stack:** Python 3 (helper, unittest), POSIX sh (hook), Rust 1.96.0 / egui 0.36.2 (menu), bash integration tests, systemd units.

**Spec:** `docs/superpowers/specs/2026-11-03-peripheral-battery-tile.md` (replaces `2026-11-03-peripheral-battery-switch.md`; the branch implements the latter, from commit 51604d6 onward).

**Note (after execution):** this plan records how the branch moved from an
earlier design (a ThinLinc local-drive export with a Settings switch) to the
tile. The specs and plans of that design were removed from the repository
later, and so were the test seams and the test of the `10-thinlinc` hook
(Task 2): the hook is main's again. The current design is the tile spec and
its follow-ups `...-tile-lines.md` and `...-on-connect.md`.

## Global Constraints

- Work in a separate worktree on the branch `peripheral-battery`.
- `git add` files by name only. Never `git add -A`, `git add .` or `git commit -a`: the `artifacts` symlink is modified locally and must never be committed.
- No ssh, no pushes. Temporary files under `$TMPDIR`.
- Code, comments, commit messages in English.
- Battery file: `/run/thinfloe/peripherals/battery`, line `name<TAB>percent<TAB>state<TAB>read_epoch`, `percent` 0..100 or `-`, `state` one of `discharging`, `charging`, `full`, `low`, `critical`, `unknown`, `not reachable`.
- Directories stay: `/run/thinfloe/peripherals` 0755 root, `battery` 0644; `/run/thinfloe/peripherals-state` 0700 root.
- Timer stays `OnBootSec=30s`, `OnUnitActiveSec=5min`, no `RandomizedDelaySec`.
- Click command: `sudo -n /usr/libexec/thinfloe/peripheral-battery` with no argument, own process group, log source `battery`.
- Tile name `95 %` (lowest percent of reachable devices) or `–` (U+2013); warn look at `low`, `critical` or percent `<= 15`.
- Device log line: `<name>: <percent> %, <state>, read HH:MM`.
- The battery job is never listed in or waited for by the end-of-session dialogs.
- The word "export" / "exported directory" for `/run/thinfloe/peripherals` disappears everywhere (CLAUDE.md, comments, tmpfiles comment).

### Commands

- **PY:** `python3 -m unittest discover -s test/unit`
- **HOOK:** `test/integration/test-thinlinc-hook.sh` (with `TMPDIR` set)
- **RT** (Rust test): `cd src/thinfloe-menu && RUSTUP_TOOLCHAIN=1.96.0 PATH=$HOME/.cargo/bin:$PATH systemd-run --user --scope -q -p MemoryMax=2G -- cargo test --locked -j 4`
- **RF** (fmt): `cd src/thinfloe-menu && RUSTUP_TOOLCHAIN=1.96.0 PATH=$HOME/.cargo/bin:$PATH cargo fmt --check`
- **RC** (clippy): `cd src/thinfloe-menu && RUSTUP_TOOLCHAIN=1.96.0 PATH=$HOME/.cargo/bin:$PATH systemd-run --user --scope -q -p MemoryMax=2G -- cargo clippy --locked --all-targets -- -D warnings`
- **IMG:** `make VERSION=pb rootfs` then `TF_UBUNTU_IMAGE=thinfloe-ubuntu:pb test/integration/test-session-packages.sh`

A single Rust test: append `<name>` after `-j 4` in RT.

## Decisions taken here (spec left them open)

1. **Concurrency lock in the helper.** The spec has the click run the reader directly while the timer also runs it. Two runs would interleave HID++ requests on one hidraw node and share the fixed temporary file `.battery.new`. The helper therefore takes an exclusive `fcntl.flock` on `$TF_BATTERY_STATE/lock` for the whole run; a second run waits, then reads again.
2. **File watch:** reuse the menu's existing 1 s add-on watcher thread (`watch_addons` in `app.rs`, `addons::stamp`), not a new 10 s poll. A `stat` a second is the cost already paid for the add-on files, and the thread already pauses while the menu is covered.
3. **The worker reads the file**, not the app after the job: `jobs::battery` runs the reader, then `battery::read`s the file and returns the devices in `JobResult::Battery`, so `Menu::finished` can log the device lines and is unit-testable without the app.
4. **Warn counts reachable devices only** (an unreachable device's percent is stale; the name also uses reachable devices only).
5. **Tile file `70-battery.desktop`:** after Settings, before Restart/Power off, so its appearing moves only those two.
6. **Comment layout:** entries `<name cut to 14 chars + …> <percent|–>[ ⚡]` joined with ` · `, packed by measured width into at most 2 comment lines (76 pt tile: name + 2 lines of 11 px fit, also at the 64 pt minimum); the last line is cut with the painter's ellipsis.
7. **Start note** `Reading the keyboard and mouse batteries.` on click, like every tile start; with no device after the job, the line `No keyboard or mouse reports a battery.` (source `battery`).
8. **Percent without value** in the log line: `<name>: –, <state>, read HH:MM`.

## Review Focus

1. A click while the timer's run is in progress: the second run must wait for the first, never interleave on the receiver (Task 1, `test_run_holds_the_lock_around_the_receivers`).
2. Battery file missing, a symlink, a FIFO, over 16 KiB, not UTF-8, or emptied while the menu runs: the tile hides, nothing blocks or panics (Task 3 read tests, Task 4 `a_job_that_finds_no_device_hides_the_tile_and_says_so`).
3. Hostile device names (control characters, bidi overrides, U+2028, 200 characters): one line on the tile and in the log, cut (Task 3 `names_are_sanitised_and_cut`).
4. A click while a dialog is up or while the job still runs starts nothing; the end-of-session question ignores the job (Task 4 tests).
5. A user file `70-battery.desktop` rewording the tile, or a user file of another name with `X-TF-Action=battery`: the reword keeps the action and the data still names the tile; the user tile is broken (Task 4 `user_file_cannot_use_battery_action`, Task 5 `battery_data_names_the_tile_over_a_reword`).

---

### Task 1: Helper back to the reading run, with a lock

**Files:**
- Modify: `overlay/usr/libexec/thinfloe/peripheral-battery`
- Modify: `overlay/etc/systemd/system/thinfloe-peripheral-battery.service`
- Modify: `overlay/usr/lib/tmpfiles.d/thinfloe-peripherals.conf`
- Test: `test/unit/test_peripheral_battery.py`

**Interfaces:**
- Produces: `main(argv) -> int`: no argument = `read_batteries()`; any argument prints `usage: peripheral-battery` to stderr and returns 2. `read_batteries()` unchanged otherwise, minus the switch check, plus the lock.

- [ ] **Step 1: Change the tests**

Delete the class `SwitchTests` (all its tests). In `MainTests.setUp` delete the `p4` patch of `switch_on`. Add to `MainTests`:

```python
def test_any_argument_is_a_usage_error(self):
    with mock.patch("sys.stderr", new_callable=io.StringIO) as err:
        self.assertEqual(mod.main(["enable"]), 2)
    self.assertIn("usage: peripheral-battery", err.getvalue())
    self.assertFalse((self.out / "battery").exists())

def test_run_holds_the_lock_around_the_receivers(self):
    calls = []
    real = mod.fcntl.flock
    def flock(fd, op):
        calls.append((os.readlink(f"/proc/self/fd/{fd}"), op))
        return real(fd, op)
    def transport(node):
        calls.append(("open", node))
        return FakeReceiver(...)   # same receiver test_main_writes_file_and_name_cache uses
    with mock.patch.object(mod.fcntl, "flock", flock), \
         mock.patch.object(mod, "open_transport", transport):
        self.assertEqual(mod.main(), 0)
    self.assertEqual(calls[0], (str(self.state / "lock"), mod.fcntl.LOCK_EX))
    self.assertEqual(calls[1][0], "open")
    self.assertEqual(stat.S_IMODE((self.state / "lock").stat().st_mode), 0o600)
```

(Build the receiver exactly as `test_main_writes_file_and_name_cache` does.)

- [ ] **Step 2: Run PY, expect the two new tests to fail** (`main(["enable"])` returns via `enable`, no `fcntl` attribute).

- [ ] **Step 3: Implement**

In the helper delete: `SWITCH_DIRS`, `SWITCH_FILE`, `SERVICE`, `EXPORT_LINES`, `MANDATORY`, `_tfuser`, `_home`, `_switch_dir_fd`, `switch_on`, `set_switch`, `thinlinc_config`, `_step`, `_systemctl`, `_unlink_quiet`, `enable`, `disable`, the `# --- switch` section, `import pwd`, `import subprocess`, and `if not switch_on(): return 0` in `read_batteries`. `main(argv)` keeps its signature with the rule above.

Add `import fcntl`. In `read_batteries`, after `state` is known and before `load_names`: open `state / "lock"` with `os.open(..., O_RDWR | O_CREAT | O_CLOEXEC | O_NOFOLLOW, 0o600)`, `fcntl.flock(fd, fcntl.LOCK_EX)` (blocking), hold it to the end of the function (`try/finally: os.close(fd)`). A failure to open or lock is logged with `log(str(lockpath), why)` and returns 1 (no reading without the lock).

Docstring: remove the sentence about the ThinLinc client exporting the directory and the whole subcommand/switch block. Say instead: the menu's battery tile reads the file, and a click runs this helper once more through `sudo -n`; a run holds `$TF_BATTERY_STATE/lock`, so a click and the timer take turns; any argument is a usage error (exit 2).

Service: delete the three comment lines and `ConditionPathExists=`; keep the "Started by ... timer only" and no-`Before=` comment.

tmpfiles comment line 1–2 becomes: `# peripheral-battery writes the battery file here; thinfloe-menu (tfuser) reads it.` Line for the state dir: `# Its temporary file, the device name cache and the lock, root only.`

- [ ] **Step 4: Run PY**, expect all tests to pass; `grep -n "thinlinc\|switch\|NFS\|export" overlay/usr/libexec/thinfloe/peripheral-battery overlay/usr/lib/tmpfiles.d/thinfloe-peripherals.conf overlay/etc/systemd/system/thinfloe-peripheral-battery.service` prints nothing (the word `export` may appear only in unrelated code; check each hit).

- [ ] **Step 5: Commit**

```bash
git add overlay/usr/libexec/thinfloe/peripheral-battery overlay/etc/systemd/system/thinfloe-peripheral-battery.service overlay/usr/lib/tmpfiles.d/thinfloe-peripherals.conf test/unit/test_peripheral_battery.py
git commit -m "peripherals: always read the batteries, one run at a time"
```

---

### Task 2: The ThinLinc hook installs the shim only

**Files:**
- Modify: `overlay/usr/libexec/thinfloe/addon-hooks/10-thinlinc`
- Modify: `test/integration/test-thinlinc-hook.sh`

- [ ] **Step 1: Rewrite the test** to these cases only (keep `tree`, `run` without `TF_BATTERY_*`, `ok`/`bad`; drop `EXPECTED`, `HELPER`, `on`, `off`):
  1. the shim replaces the client's `pulseaudio`, mode executable, rc 0;
  2. without the client's `pulseaudio`: rc 0, nothing created at that path;
  3. a failing shim copy (`TF_SHIM` = missing file) fails the hook (rc ≠ 0);
  4. the hook never touches `tlclient_mandatory.conf`: a tree whose `etc/tlclient_mandatory.conf` holds `PRINTER_ENABLED=0\nNFS_SERVER_ENABLED=0\n` is byte-identical after the run (`cmp` against a copy), and a tree without the file still has none.
  Header comment: the hook puts the PipeWire shim over the client's PulseAudio; nothing else.

- [ ] **Step 2: Run HOOK**, expect case 4 to fail (the current hook calls `thinlinc-config`).

- [ ] **Step 3: Replace the hook** with the 17d5202 version (`git show 17d5202:overlay/usr/libexec/thinfloe/addon-hooks/10-thinlinc`), keeping the test overrides:

```sh
#!/bin/sh
# Hook run by thinfloe-addon after it installs the ThinLinc client. The client starts
# its own PulseAudio 6.0 from /opt/thinlinc/lib/tlclient/pulseaudio; the
# package puts that daemon there, so put the PipeWire shim over it.
# TF_THINLINC_DIR and TF_SHIM exist for test/integration/test-thinlinc-hook.sh.
set -e
TL=${TF_THINLINC_DIR:-/opt/thinlinc}
SHIM=${TF_SHIM:-/usr/libexec/thinfloe/tlclient-pulseaudio}
f=$TL/lib/tlclient/pulseaudio
[ -e "$f" ] || exit 0
install -m 0755 "$SHIM" "$f"
```

- [ ] **Step 4: Run HOOK**, expect `PASSED - 10-thinlinc hook`. The `Makefile` line running the test stays.

- [ ] **Step 5: Commit**

```bash
git add overlay/usr/libexec/thinfloe/addon-hooks/10-thinlinc test/integration/test-thinlinc-hook.sh
git commit -m "thinlinc: the hook installs the PipeWire shim only"
```

---

### Task 3: `battery.rs` – the file, the tile text and the log text

**Files:**
- Create: `src/thinfloe-menu/src/battery.rs`
- Modify: `src/thinfloe-menu/src/lib.rs` (add `pub mod battery;` in order)

**Interfaces:**
- Produces:
  - `pub const FILE: &str = "/run/thinfloe/peripherals/battery";`
  - `pub const MAX_BYTES: u64 = 16 * 1024; pub const MAX_LINES: usize = 64; pub const MAX_NAME: usize = 64; pub const SHORT_NAME: usize = 14; pub const WARN_PERCENT: u8 = 15;`
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub struct Device { pub name: String, pub percent: Option<u8>, pub state: String, pub read_epoch: u64 }` with `pub fn reachable(&self) -> bool` (state ≠ `not reachable`) and `pub fn low(&self) -> bool` (reachable and (state `low`|`critical` or percent ≤ `WARN_PERCENT`)).
  - `pub fn parse(text: &str) -> Vec<Device>` – split on `\n` only; exactly 4 tab fields; field 4 all ASCII digits (fits u64); field 2 `-` or 1–3 digits ≤ 100; first `MAX_LINES` good lines; name through `sanitise`.
  - `pub fn sanitise(name: &str) -> String` – same rule as the helper's `sanitise`: drop NUL, replace every `char::is_control()` and the format/separator characters U+200B–U+200F, U+202A–U+202E, U+2028, U+2029, U+2060–U+2064, U+2066–U+2069, U+FEFF with `?`, trim, first `MAX_NAME` chars, empty → `unknown device`.
  - `pub fn read(path: &Path) -> Vec<Device>` – `crate::bounded::read_bounded(path, MAX_BYTES, false)`, `String::from_utf8_lossy`, `parse`; any error → empty.
  - `pub struct TileText { pub name: String, pub entries: Vec<String>, pub warn: bool }` (`Debug, Clone, PartialEq, Eq`) and `pub fn tile_text(devices: &[Device]) -> TileText` – name `format!("{p} %")` of the lowest percent among reachable devices, else `–`; one entry per device in file order: `short(name)` + ` ` + (percent, or `–` when none or not reachable) + (` ⚡` when state `charging`); `short` cuts to `SHORT_NAME` chars and appends `…` only when it cut; `warn` = any `low()`.
  - `pub const CHARGING_MARK: &str = "⚡";`
  - `pub fn log_text(d: &Device) -> String` – `<name>: <p> %, <state>, read HH:MM`, or `<name>: –, <state>, read HH:MM` without a percent; HH:MM from `crate::log::local_hms(d.read_epoch as i64)`.
  - `#[derive(Debug, Default)] pub struct Watcher` with `pub fn tick(&mut self, path: &Path) -> bool` – like `addons::Watcher`: first tick true, then true only when `addons::stamp(path)` changed.

- [ ] **Step 1: Write the tests** (in `battery.rs` `mod tests`, using `crate::testdir::TestDir`):
  - `parse_takes_good_lines_only`: input with a good line `MX Mechanical\t100\tcharging\t1000`, `Mouse\t-\tnot reachable\t900`, and bad ones (3 fields, 5 fields, `x\t101\tfull\t1`, `x\t9a\tfull\t1`, `x\t5\tfull\t-1`, empty line, `\r`-ended good line kept with name sanitised) → exactly the expected `Device`s.
  - `names_are_sanitised_and_cut`: `"A\u{202e}B\u{2028}C\x1b"` → `"A?B?C?"`; 200 × `W` → 64 chars; `"  "` → `unknown device`.
  - `read_is_bounded_and_refuses_odd_files`: missing → empty; symlink to a good file → empty; a FIFO (`libc::mkfifo`) → empty without blocking; 16 KiB + 1 → empty; invalid UTF-8 in a name → the line kept with U+FFFD; 100 good lines → 64 devices.
  - `tile_text_names_the_lowest_reachable_percent`: devices 95 discharging, 100 charging, unreachable 5 → name `95 %`, entries `["MX Anywhere 3S… 95", "MX Mechanical 100 ⚡", "Mouse –"]` (first device named `MX Anywhere 3S Mouse`), `warn` false; all unreachable → name `–`; reachable with `-` only → `–`.
  - `warn_at_low_critical_or_15_percent`: 15 discharging → warn; 16 → no; state `low` at 50 → warn; `critical` → warn; unreachable 5 → no.
  - `log_text_says_percent_state_and_time`: with `TZ=UTC` set through the same mechanism `log.rs` tests use, epoch 3660 → `MX Mechanical: 100 %, charging, read 01:01`; no percent → `Mouse: –, not reachable, read 01:01`.
  - `watcher_fires_first_then_on_change`: first `tick` true, second false, after rewriting the file with other content true, after removing it true, then false.

- [ ] **Step 2: Run RT** `battery::`, expect compile failure (module missing).

- [ ] **Step 3: Implement** the interface above. Make `log::local_hms` reachable (it is `pub fn` already).

- [ ] **Step 4: Run RT, RF, RC**, all pass.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/battery.rs src/thinfloe-menu/src/lib.rs
git commit -m "menu: read the battery file and phrase it for a tile"
```

---

### Task 4: Settings switch out, battery tile and read job in

**Files:**
- Modify (restore to `17d5202`): `src/thinfloe-menu/src/settings.rs`, `src/thinfloe-menu/src/ui.rs` – their only change on this branch is the switch: `git checkout 17d5202 -- src/thinfloe-menu/src/settings.rs src/thinfloe-menu/src/ui.rs`. This removes `SettingsEvent::Battery`, `Row.tile: Option<usize>` (back to `usize`), `BATTERY_NAME`, `BATTERY_ON`, `BATTERY_NOTE`, `battery_row`, `battery_switch_on`, `battery_switch_path`, `battery_switch_path_from`, the Settings battery row and their tests.
- Modify: `src/thinfloe-menu/src/tiles.rs`, `menu.rs`, `jobs.rs`, `app.rs`, `main.rs`
- Create: `overlay/etc/thinfloe/menu/70-battery.desktop`, `src/thinfloe-menu/icons/battery.svg`

**Interfaces:**
- Consumes: Task 3 `battery::{Device, read, log_text, Watcher, FILE}`.
- Produces:
  - `tiles::Action::Battery` (parsed from `X-TF-Action=battery`; `exit_code` → `None`).
  - Reused, changed: `menu::Job::Battery` (unit variant, was `Battery { on }`); `jobs::JobResult::Battery { status: Result<Option<i32>, String>, devices: Vec<battery::Device> }` (was `{ on, status }`); `jobs::battery(cmd: &[String], file: &Path, feed: &Arc<Feed>) -> JobResult` (was `(cmd, on, feed)`; no verb argument, still `process_group(0)` and `run_logged(.., BATTERY_SOURCE, feed, false, |_| {})`, then `battery::read(file)`); `jobs::BATTERY_SOURCE`, `jobs::default_battery_cmd()` unchanged; `jobs::Paths` gains `battery_file: PathBuf` beside `battery_cmd`.
  - `Menu`: drop `battery_on`, `battery_changed`; `battery_run: Option<bool>` becomes `battery_run: bool`; new `pub battery: Vec<battery::Device>`; `pub fn set_battery(&mut self, devices: Vec<Device>)`; `battery_busy()` kept.
  - `app::Wake::Battery`; `app::Config.battery_switch` replaced by `battery_file: PathBuf`; `App::refresh_battery` now does `self.cfg.menu.set_battery(battery::read(&self.cfg.battery_file))`.
  - `main.rs`: `fn battery_file() -> PathBuf` = `env_path("TF_BATTERY_FILE", battery::FILE)`.

- [ ] **Step 1: Restore settings.rs and ui.rs** (command above), then write the failing tests.

tiles.rs:
  - `battery_tile_parses`: read `overlay/etc/thinfloe/menu/70-battery.desktop` like `settings_tile_parses`; `action == Action::Battery`, `icon == Some("battery")`, usable, not addon/multiple/save_on_exit, `Action::Battery.exit_code() == None`, `icons/battery.svg` is a file.
  - `user_file_cannot_use_battery_action`: as `user_file_cannot_use_settings_action`, problem text `61-mine.desktop: a tile of your own may only use the run action, not 'battery'`; a reword of `70-battery.desktop` keeps `Action::Battery` and the new Name.

menu.rs (replace the six `battery_*` tests of the switch; add a `battery_menu()` helper: `grid()` plus the shipped `70-battery.desktop` text tile, returning `(Menu, index)`; a `dev(name, percent, state)` helper):
  - `the_battery_tile_shows_only_with_a_device`: no devices → `!visible(i)`, `!can_activate(i)`; `set_battery(vec![dev(..)])` → visible and activatable.
  - `a_click_starts_one_read_job`: `activate(i)` → `Some(Effect::Start(Job::Battery))`, `battery_busy()`, `active(i)`, last log line `ev("Reading the keyboard and mouse batteries.")`; second `activate(i)` → `None`.
  - `the_job_end_logs_one_line_per_device`: after `finished(JobResult::Battery { status: Ok(Some(0)), devices: [100 charging, 10 discharging] })` → `None`, not busy, `m.battery` equals those devices, the last two log lines have source `Some("battery")`, texts `battery::log_text` of each, kinds `Event` and `Warn` (the 10 % one).
  - `a_failed_read_job_warns_and_still_logs_the_devices`: `Ok(Some(1))` → warn `peripheral-battery exited with status 1.` then the device lines; `Ok(None)` → `peripheral-battery was killed by a signal.`; `Err("sudo could not be started: x")` → `sudo could not be started: x.`.
  - `a_job_that_finds_no_device_hides_the_tile_and_says_so`: devices empty → line `No keyboard or mouse reports a battery.` source `battery`, `!visible(i)`, not busy.
  - `no_read_job_behind_a_dialog`: `m.logout = Some(Logout::Confirm)` → `activate(i)` is `None`.
  - keep `the_end_of_session_question_ignores_a_battery_job`, rewritten to start the job by `activate(i)` and end it with `JobResult::Battery { status: Ok(Some(0)), devices }`.

jobs.rs: replace `battery_logs_both_streams_as_battery_and_passes_the_verb` with `battery_logs_both_streams_and_returns_the_file`: command `sh("echo \"out $#\"; echo err >&2")` (asserts no argument: `out 0`), a `TestDir` file holding one good line → `JobResult::Battery { status: Ok(Some(0)), devices: [that device] }`, feed lines `(battery, err)`, `(battery, out 0)`; `default_battery_cmd()` unchanged. Every `Paths { .. }` literal in the tests gains `battery_file`.

- [ ] **Step 2: Run RT**, expect compile errors / failures for the new names.

- [ ] **Step 3: Implement**
  - tiles.rs: `Action::Battery` parsed from `"battery"`; add to the `None` arm of `exit_code`. `tile_from` already refuses non-run actions in user files.
  - menu.rs: module doc line about the battery job → "A battery read job is neither waited for nor stopped: it only reads, and the shutdown ends it anyway." `Job::Battery` doc → `sudo -n /usr/libexec/thinfloe/peripheral-battery`, one reading run. `visible`: `Action::Battery` tile visible iff `!self.battery.is_empty()` (checked before the addon rule; a battery tile is never an addon). `can_activate`: `Action::Battery => !self.battery_run`. `active`: also true for the battery tile while `battery_run`. `activate`: `Action::Battery` sets `battery_run`, notes the start line, returns `Effect::Start(Job::Battery)`. Remove the `SettingsEvent::Battery` arm of `settings()`. `finished(JobResult::Battery { status, devices })`: clear `battery_run`; warn line by status as in the tests; `set_battery(devices)`; then per device push a `log::Line { time: log::local_time(), source: Some(BATTERY_SOURCE.into()), runs: vec![Run::plain(battery::log_text(d))], kind: if d.low() { Warn } else { Event } }`, or the no-device line (kind `Event`). Logout code paths do not look at `battery_run` (unchanged).
  - jobs.rs: new signature and result as in Interfaces; doc comment: runs the reader once, both streams logged as `battery`, then reads the file; no group id is reported because the end-of-session dialogs neither wait for nor stop it.
  - app.rs: `start()` passes `battery_file` in `Paths` and calls `Job::Battery => jobs::battery(&paths.battery_cmd, &paths.battery_file, &feed)`. Remove the "Opening Settings" `refresh_battery` call and the `use crate::settings` it needed (if unused). In `user_event` keep `let battery = matches!(result, JobResult::Battery { .. })` out: the result carries the devices, so drop that refresh. Add `Wake::Battery => self.refresh_battery()`. `watch_addons` gets the battery path too: its thread keeps a `battery::Watcher` and sends `Wake::Battery` when its `tick` is true (same 1 s pace, same `shown` pause); update its doc comment.
  - main.rs: `build_menu` sets `menu.set_battery(battery::read(&battery_file()))` instead of `battery_on`; `Paths` gets `battery_file: battery_file()`; `Config` gets `battery_file: battery_file()` instead of `battery_switch`.
  - `overlay/etc/thinfloe/menu/70-battery.desktop`:

    ```ini
    [Desktop Entry]
    Type=Application
    Name=Battery
    Comment=Keyboard and mouse
    Icon=battery
    X-TF-Action=battery
    ```

  - `src/thinfloe-menu/icons/battery.svg`: Lucide `battery` from the same release as the others. Fetch `https://unpkg.com/lucide-static@1.49.0/icons/battery.svg` if reachable and keep it byte for byte with the `<!-- @license lucide-static v1.49.0 - ISC -->` first line the other icons carry; otherwise write it in the layout of `icons/usb.svg` with class `lucide lucide-battery` and the body `<path d="M 22 14 L 22 10" />` and `<rect x="2" y="6" width="16" height="12" rx="2" />`. `every_shipped_icon_passes_the_guards_and_draws` covers it; the Dockerfile installs `icons/*.svg` already.

- [ ] **Step 4: Run RT, RF, RC**, all pass; `grep -rn "battery_on\|battery_changed\|battery_switch\|SettingsEvent::Battery" src/thinfloe-menu/src` prints nothing.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/settings.rs src/thinfloe-menu/src/ui.rs src/thinfloe-menu/src/tiles.rs src/thinfloe-menu/src/menu.rs src/thinfloe-menu/src/jobs.rs src/thinfloe-menu/src/app.rs src/thinfloe-menu/src/main.rs src/thinfloe-menu/icons/battery.svg overlay/etc/thinfloe/menu/70-battery.desktop
git commit -m "menu: a battery tile reads the batteries on a click"
```

---

### Task 5: Draw the battery tile

**Files:**
- Modify: `src/thinfloe-menu/src/ui.rs`

**Interfaces:**
- Consumes: `battery::{tile_text, TileText, CHARGING_MARK}`, `Menu.battery`, `Action::Battery`.
- Produces: `fn pack(entries: &[String], fits: impl Fn(&str) -> bool, max_lines: usize) -> Vec<String>` (private to ui.rs); `paint_tile(ui, rect, tile, icon, state, text: Option<&battery::TileText>)`.

- [ ] **Step 1: Write the tests** (ui.rs `mod tests`):
  - `pack_fills_lines_greedily`: entries `["aaaa","bbbb","cccc"]`, max 2 lines. With `fits = |s| s.chars().count() <= 11` → `["aaaa · bbbb", "cccc"]`. With `<= 10` → `["aaaa", "bbbb · cccc"]` (the overflow joins the last line). One entry → one line; no entries → empty.
  - `the_battery_tile_draws_the_data_inside_its_tile`: a menu with the shipped `70-battery.desktop` tile and three devices (one 200-char name) rendered as in `tile_text_stays_on_one_line_inside_its_tile`; the text shapes inside the tile are `95 %` plus 1 or 2 comment lines, each one row, each inside its clip, clip height ≤ the tile height.
  - `a_low_battery_draws_the_name_in_warn`: device at 10 % → the `10 %` text shape's colour is `WARN`; at 50 % it is the foreground colour (use the existing `colour_of`-style helper).
  - `battery_data_names_the_tile_over_a_reword`: the tile's `name` changed to `Akku` (as a reword would) with one device → `Akku` is not drawn, `95 %` is.
  - `the_charging_mark_has_a_glyph`: the 11 px proportional font after `brand::apply` has a glyph for `CHARGING_MARK` (egui's `has_glyphs` on the context's fonts in 0.36.2). If this fails, set `CHARGING_MARK` to `+` in `battery.rs` and its test, not a new font.

- [ ] **Step 2: Run RT**, expect failures (no `pack`, plain tile name drawn).

- [ ] **Step 3: Implement**
  - Grid loop: `let text = (tile.action == Action::Battery).then(|| battery::tile_text(&menu.battery));` passed to `paint_tile`.
  - `paint_tile`: with `Some(text)` and no `tile.problem`, the first line is `text.name` in `WARN` when `text.warn`, else the usual foreground; the comment lines are `pack(&text.entries, fits, 2)` where `fits` measures the 11 px proportional galley width against `text_w` (`painter.layout_no_wrap(..).size().x <= text_w`), each drawn like the comment (`MUTED`, truncated with ellipsis). Without `text`, unchanged. Vertical centring already sums the lines.
  - `pack` joins entries with ` · `; when a new line would exceed `max_lines`, the rest goes onto the last line.
  - Add a battery state to the ignored shot test `shot_desktop_and_settings_at_densities`: `desktop-1080-battery.ppm` with two devices, one low.

- [ ] **Step 4: Run RT, RF, RC**, all pass. Optionally run the ignored shot test and look at the picture (`... cargo test --locked -j 4 shot_desktop -- --ignored`).

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/ui.rs src/thinfloe-menu/src/battery.rs
git commit -m "menu: the battery tile shows each device and warns when one runs low"
```

(`battery.rs` only if the charging mark changed.)

---

### Task 6: Image test and documentation

**Files:**
- Modify: `test/integration/test-session-packages.sh`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Change the image test**
  - Add `70-battery` to the `for t in ...` tile list (keep order) and `battery` to the Lucide icon list.
  - In the battery block: comment says the timer refreshes the file that the menu's battery tile reads (no ThinLinc). Replace the `ConditionPathExists` ok-check with a FAIL when `^ConditionPathExists=` appears in the service (`ok   - the battery service runs on every timer firing`). Delete the "ships no battery switch file" check. Extract `etc/thinfloe/menu/70-battery.desktop` with the other files and require `grep -qx 'X-TF-Action=battery'` and `grep -qx 'Icon=battery'` on it. Rename "the exported directory is 0755" to "the battery directory is 0755, the state directory 0700".
  - `thinfloe-menu --check` already runs over every shipped tile, so the icon must render there.

- [ ] **Step 2: Run IMG**, expect `PASSED`. (A build takes a while; run it once here, not per task.)

- [ ] **Step 3: Rewrite CLAUDE.md**
  - Replace the whole "Peripheral battery: off by default ..." bullet (Customizing the System) with one that says: the reader (`/usr/libexec/thinfloe/peripheral-battery`, root, timer 30 s after boot and every 5 min) writes `/run/thinfloe/peripherals/battery` (format and states as now; keep the HID++/Bolt, register `b5`/`50+n`, timeout, `not reachable`, name cache, Solaar paragraphs); a run holds `/run/thinfloe/peripherals-state/lock`, so a click and the timer take turns; any argument is exit 2. The menu tile `70-battery.desktop` (`X-TF-Action=battery`, honoured only in shipped files; a user file of that name may reword Name, Comment, Icon, but the drawn name and comment come from the file while a device is listed) shows when the file lists a device: name = lowest reachable percent or `–`, comment = one entry per device (name cut to 14 characters, `–` for not reachable, `⚡` while charging) on at most two lines, the name in the warn colour at `low`, `critical` or 15 % or less. The menu reads the file at start, on every change (the 1 s add-on watcher) and after a click; a click runs `sudo -n` the reader in its own process group (output under `battery`) and then logs `<name>: <percent> %, <state>, read HH:MM` per device. Limitation: the menu, and so the tile, is hidden while a fullscreen window (the ThinLinc client) covers it. Guards: `test/unit/test_peripheral_battery.py`, `test-session-packages.sh`, `battery.rs` and the menu/ui tests. Keep "Confirmed on hardware" only for what was confirmed (both devices read in 0.5 s); drop the ThinLinc export confirmation and the export/server/`tl-mount-localdrives`/`NFS_*` text; **Not verified on hardware:** a boot of the built image, the tile and the click, the `not reachable` line written by the timer.
  - In the Add-ons "The menu" paragraph, replace the "battery switch job (Settings, `battery`) ..." sentence with: "The battery read job (the battery tile) is neither listed nor waited for at Log out, Restart or Power off: it only reads, and the shutdown ends it."
  - "Known Limitations > Menu limits": add "The battery tile is not seen while a fullscreen window covers the menu."
  - Add `test/integration/test-thinlinc-hook.sh` (shim only) to the ThinLinc hook mention under Add-ons "Post-install hooks" if the guards there list tests; otherwise leave it.
  - `grep -n "thindrives\|NFS_\|peripheral-battery enable\|thinlinc-config\|battery switch\|exported directory" CLAUDE.md` prints nothing.

- [ ] **Step 4: Run PY, HOOK, RT** once more (all green).

- [ ] **Step 5: Commit**

```bash
git add test/integration/test-session-packages.sh CLAUDE.md
git commit -m "docs: the battery tile replaces the ThinLinc export"
```

After Task 6 the branch is ready for a new PR against `main` (the owner opens it; this plan pushes nothing).
