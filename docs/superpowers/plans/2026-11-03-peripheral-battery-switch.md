# Peripheral Battery Switch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A tick box in the menu's Settings screen switches the keyboard and
mouse battery file and its ThinLinc export on and off per machine; off by
default.

**Architecture:** The state is one file in tfuser's home,
`~/.config/thinfloe/peripheral-battery`, which travels with `save-home`. The
root helper `/usr/libexec/thinfloe/peripheral-battery` gains the subcommands
`enable`, `disable`, `thinlinc-config` and `status`, and takes over the
rewrite of `tlclient_mandatory.conf` from the sh hook. The service skips
through `ConditionPathExists=`. The menu runs `sudo -n
/usr/libexec/thinfloe/peripheral-battery enable|disable` as a job of its own
kind (`Job::Battery`), next to the add-on jobs but not part of them.

**Tech Stack:** Python 3 stdlib, POSIX sh, systemd, Rust 1.96.0 (egui,
`src/thinfloe-menu`).

**Spec:** `docs/superpowers/specs/2026-11-03-peripheral-battery-switch.md`
(read it with this plan). Previous plan:
`docs/superpowers/plans/2026-11-03-peripheral-battery.md`.

## Global Constraints

- Default off: no state file means no reading, no battery file, no NFS keys.
- State file: `~/.config/thinfloe/peripheral-battery`, content `on\n`, mode
  0644, owned by tfuser; written and removed only by the root helper. The
  helper finds the home with `pwd.getpwnam("tfuser").pw_dir`, never `$HOME`.
- Mandatory-config rewrite: temp file + rename in the same directory, every
  other line kept, mode 0644; the two keys are exactly
  `NFS_SERVER_ENABLED=1` and `NFS_EXPORTS=/run/thinfloe/peripherals,ro`.
  A failure there never fails the ThinLinc install.
- No `Before=` in any unit (CLAUDE.md, "A multi-user.target unit must never
  declare Before= on a sysinit.target unit").
- Menu row texts, verbatim: name `Keyboard and mouse battery`; status `off`,
  or `on · in ThinLinc as ~/thindrives/peripherals/battery`; extra item
  `takes effect when ThinLinc starts again`; log source `battery`.
- All code, comments and commit messages in English; no mention of the
  assistant or its vendor anywhere; no `Co-Authored-By`.
- `git add` files by name, never `git add -A` or `.`: the worktree's
  `artifacts` symlink is repointed locally and must never be committed.
- Temporary files under `/scratch/moetiker/tmp`. No ssh, no push.
- Test commands (use exactly these):
  - Python: `python3 -m unittest discover -s test/unit`
    (one file: `python3 -m unittest discover -s test/unit -p test_peripheral_battery.py -v`)
  - Hook: `TMPDIR=/scratch/moetiker/tmp test/integration/test-thinlinc-hook.sh`
  - Rust: `cd src/thinfloe-menu && RUSTUP_TOOLCHAIN=1.96.0 PATH=$HOME/.cargo/bin:$PATH systemd-run --user --scope -q -p MemoryMax=2G -- cargo test --locked -j 4`
    (the mise cargo shim is broken on this host)
  - Image: `make VERSION=pb rootfs`, then
    `TF_UBUNTU_IMAGE=thinfloe-ubuntu:pb test/integration/test-session-packages.sh`

## Decision: end-of-session dialogs do not wait for a battery job

Add-on jobs are waited for at Log out / Restart / Power off (`AddonRun`,
`Logout::WaitAddons`, `addon_jobs()`), because a dpkg cut short stays
half-installed for the rest of the boot. A battery job is kept out of that
machinery (its own `battery_run` field, not an `AddonRun`), and the dialogs
neither list, wait for nor stop it. Why:

1. It is short. `enable` writes one file, rewrites one config file and
   queues the service with `systemctl start --no-block`; `disable` removes
   files and runs `systemctl stop` (which waits for a read in progress, at
   most `RUN_BUDGET` 15 s). The state file is changed first, in
   milliseconds, so the logout save that follows archives the new state.
2. Cut short at any point it heals: the boot rebuilds `/run`, and
   `thinfloe-addon boot` reinstalls ThinLinc on every boot, whose hook runs
   `peripheral-battery thinlinc-config`, which makes the mandatory config
   match the state file.
3. It runs under sudo in a process group of its own, so the menu exiting
   (Log out) does not end it; on Restart / Power off systemd ends it.
4. Putting it into `addon_runs` would be wrong, not just larger:
   `addon_busy(id)` matches any argument against the add-on id, and
   `enable` is a valid add-on id (`enable.desktop`).

Consequence, recorded in CLAUDE.md: a `Logout::Confirm` question appears
only for programs and add-on jobs, never for a battery job.

## Review Focus

- `~/.config` or `~/.config/thinfloe` a symlink (restored from a home
  archive): the root helper must not write through it; it exits 1 with a
  reason and changes nothing outside the home. Test in Task 1.
- Switched off while the timer's read is running: the read must not write
  the battery file back after `disable` removed it (`disable` stops the
  service before it removes the file). Test in Task 1 (order of calls).
- Switched on after a boot whose 30 s timer run was skipped by the
  condition: `OnUnitActiveSec` has no activation to count from, so the
  timer may never fire again; `enable` starting the service gives it one.
  Test in Task 1 (`enable` calls `systemctl start --no-block`); on hardware
  in Task 5.
- A ThinLinc install while switched off on a stick whose old hook left the
  two keys: `thinlinc-config` must remove them, not just skip. Test in
  Task 2.
- A click on the tick box while its job runs, or a second click before the
  state was read again: no second job, and the row shows the state file,
  not the click. Test in Task 3.

---

### Task 1: Helper subcommands

**Files:**
- Modify: `overlay/usr/libexec/thinfloe/peripheral-battery` (docstring, new
  functions, `main(argv)`)
- Test: `test/unit/test_peripheral_battery.py` (new class `SwitchTests`)

**Interfaces:**
- Produces (used by Task 2 and the menu through sudo):
  - CLI: `peripheral-battery` (no argument: one reading run, unchanged),
    `enable`, `disable`, `thinlinc-config`, `status`. Exit 0 ok, 1 failure
    with `thinfloe: battery: <what>: <why>` on stderr (existing `log()`),
    2 unknown subcommand (usage on stderr).
  - Environment overrides for tests: `TF_BATTERY_HOME` (tfuser's home),
    `TF_THINLINC_DIR` (default `/opt/thinlinc`), `TF_SYSTEMCTL` (default
    `systemctl`), plus the existing `TF_BATTERY_DIR`, `TF_BATTERY_STATE`.
  - Constants: `SWITCH_DIRS = (".config", "thinfloe")`,
    `SWITCH_FILE = "peripheral-battery"`,
    `SERVICE = "thinfloe-peripheral-battery.service"`,
    `EXPORT_LINES = ("NFS_SERVER_ENABLED=1", "NFS_EXPORTS=/run/thinfloe/peripherals,ro")`.
  - Python functions:
    `switch_on() -> bool`, `set_switch(on: bool) -> None`,
    `thinlinc_config(on: bool) -> None` (raises `OSError` with a reason),
    `main(argv: Sequence[str] = ()) -> int`.

- [ ] **Step 1: Write the failing tests** in `SwitchTests`. `setUp`: a temp
  dir under `$TMPDIR` with `home/`, `tl/etc/tlclient_mandatory.conf`
  (content `PRINTER_ENABLED=0\n`), `peripherals/battery`,
  `state/power-supply`, and a stub `systemctl` (sh script appending `"$@"`
  as one line to `calls`); patch `os.environ` with `TF_BATTERY_HOME`,
  `TF_THINLINC_DIR`, `TF_SYSTEMCTL`, `TF_BATTERY_DIR`, `TF_BATTERY_STATE`.
  `sw = home/.config/thinfloe/peripheral-battery`, `conf =
  tl/etc/tlclient_mandatory.conf`.
  - `test_default_is_off`: `main(["status"]) == 0`, stdout `off\n`, `sw`
    absent.
  - `test_enable_writes_switch_config_and_starts_service`: rc 0; `sw` reads
    `on\n`, mode `0o644`; `conf` is `PRINTER_ENABLED=0\nNFS_SERVER_ENABLED=1\nNFS_EXPORTS=/run/thinfloe/peripherals,ro\n`
    and mode `0o644`; `calls` is `start --no-block thinfloe-peripheral-battery.service\n`;
    `main(["status"])` prints `on\n`.
  - `test_enable_without_thinlinc_etc_still_switches_on`: `tl/etc` removed;
    rc 0, `sw` exists, `tl/etc` still absent.
  - `test_disable_removes_switch_keys_and_battery_file`: enable, then
    `disable` rc 0; `sw`, `peripherals/battery`, `state/power-supply` gone;
    `conf` is `PRINTER_ENABLED=0\n`; last line of `calls` is
    `stop thinfloe-peripheral-battery.service`.
  - `test_disable_stops_the_service_before_removing_the_file`: the stub
    `systemctl` also records whether `peripherals/battery` still exists when
    it is called with `stop`; assert it did (the stop came first).
  - `test_enable_and_disable_twice_change_nothing`: after each second run the
    bytes of `sw`/`conf` equal those after the first, rc 0 both times, no
    `.*` temp file left in `tl/etc` or `home/.config/thinfloe`.
  - `test_thinlinc_config_follows_the_switch`: `conf` holding
    `NFS_SERVER_ENABLED=0\nNFS_EXPORTS=/media,rw\nX=1\n`; with the switch off
    `thinlinc-config` leaves `X=1\n`; after `set_switch(True)` it gives
    `X=1\nNFS_SERVER_ENABLED=1\nNFS_EXPORTS=/run/thinfloe/peripherals,ro\n`.
    With the switch off and no `conf`, no `conf` is created.
  - `test_unusable_thinlinc_etc_fails_with_a_reason`: `tl/etc` is a regular
    file; `thinlinc-config` with the switch on returns 1 and stderr starts
    with `thinfloe: battery: `.
  - `test_systemctl_failure_keeps_the_switch_and_exits_1`: stub exits 3;
    `enable` rc 1, `sw` exists.
  - `test_symlinked_config_dir_is_not_followed`: `home/.config` a symlink
    to `outside/`; `enable` rc 1, `outside/` stays empty.
  - `test_unknown_subcommand_exits_2`: `main(["bogus"]) == 2`.
  - `test_no_argument_still_reads`: the existing `MainTests` call
    `mod.main()` and must keep passing (no change to them).

- [ ] **Step 2: Run to see them fail**

  Run: `python3 -m unittest discover -s test/unit -p test_peripheral_battery.py -v`
  Expected: the `SwitchTests` fail (`main` takes no argument / no
  subcommands); the old tests pass.

- [ ] **Step 3: Implement.** Rename today's `main()` body to
  `read_batteries() -> int`; `main(argv=())` dispatches, empty `argv`
  calling `read_batteries()`. The `__main__` block becomes
  `sys.exit(main(sys.argv[1:]))` (the default `()` matters: under
  `unittest discover` `sys.argv` holds `discover`).
  - The home is `TF_BATTERY_HOME` or `pwd.getpwnam("tfuser").pw_dir`.
    Reach `.config/thinfloe` the way `_tick_dir_fd` in
    `overlay/usr/bin/thinfloe-addon` does: `os.open` with `O_DIRECTORY |
    O_NOFOLLOW` and `dir_fd`, `mkdir` 0755 when missing. `chown` new
    directories and the switch file to tfuser only when `os.geteuid() == 0`.
  - `set_switch(True)`: `peripheral-battery.new` with `O_EXCL | O_NOFOLLOW`,
    `on\n`, fchmod 0644, fsync, `os.replace` with `dir_fd`.
    `set_switch(False)`: unlink, missing is fine; does not create dirs.
  - `thinlinc_config(on)`: `etc = TL/etc`; not existing: return; existing
    but not a directory: raise. Read the file (missing = empty), drop lines
    starting `NFS_SERVER_ENABLED=` or `NFS_EXPORTS=`, append
    `EXPORT_LINES` when on. Off and no file: return without writing.
    Write `etc/.tlclient_mandatory.conf.<pid>`, chmod 0644, `os.replace`;
    remove the temp file on failure.
  - `enable`: `set_switch(True)`, `thinlinc_config(True)`,
    `[systemctl, "start", "--no-block", SERVICE]`. Each step runs even when
    an earlier one failed, except that nothing follows a failed
    `set_switch`; every failure is logged; rc 1 if any failed.
  - `disable`: `set_switch(False)`, `[systemctl, "stop", SERVICE]`,
    `thinlinc_config(False)`, unlink `TF_BATTERY_DIR/battery` and
    `TF_BATTERY_STATE/power-supply` (missing is fine). Same failure rule,
    all steps attempted.
  - `thinlinc-config`: `thinlinc_config(switch_on())`.
  - Extend the module docstring with the subcommands and the state file.

- [ ] **Step 4: Run the tests**

  Run: `python3 -m unittest discover -s test/unit`
  Expected: OK (all files, `test_addon.py` included).

- [ ] **Step 5: Commit**

```bash
git add overlay/usr/libexec/thinfloe/peripheral-battery test/unit/test_peripheral_battery.py
git commit -m "peripherals: switch the battery file on and off"
```

### Task 2: Hook, service condition, image

**Files:**
- Modify: `overlay/usr/libexec/thinfloe/addon-hooks/10-thinlinc`
- Modify: `overlay/etc/systemd/system/thinfloe-peripheral-battery.service`
- Test: `test/integration/test-thinlinc-hook.sh`,
  `test/integration/test-session-packages.sh` (battery block near line 906)

**Interfaces:**
- Consumes: `peripheral-battery thinlinc-config`, `TF_BATTERY_HOME`,
  `TF_THINLINC_DIR` (Task 1).
- Produces: hook override `TF_BATTERY_HELPER` (default
  `/usr/libexec/thinfloe/peripheral-battery`).

- [ ] **Step 1: Change the hook test.** `run()` also passes
  `TF_BATTERY_HOME="$work/home"` and
  `TF_BATTERY_HELPER="$REPO/overlay/usr/libexec/thinfloe/peripheral-battery"`;
  helpers `on()` (creates `$work/home/.config/thinfloe/peripheral-battery`
  with `on`) and `off()` (removes it). Cases:
  - existing cases that expect `$EXPECTED` run with `on`;
  - `ok "switched off, an empty mandatory file stays empty"`;
  - `ok "switched off, keys an earlier hook wrote are removed, others kept"`
    (file `PRINTER_ENABLED=0\n` + `$EXPECTED`, result `PRINTER_ENABLED=0`);
  - the `noetc` case now expects rc 0 and **no** `etc/` created (ThinLinc
    without etc/ has nothing to configure);
  - `badetc` (etc/ is a file) with `on`: rc 0, output contains
    `thinfloe: battery export not configured: `, shim installed;
  - a missing helper (`TF_BATTERY_HELPER=$work/none`) with `on`: rc 0, the
    message is printed, shim installed.

- [ ] **Step 2: Run it to fail**

  Run: `TMPDIR=/scratch/moetiker/tmp test/integration/test-thinlinc-hook.sh`
  Expected: FAIL on the switched-off and `noetc` cases.

- [ ] **Step 3: Implement.** In the hook, replace the body of
  `export_battery` with `"$HELPER" thinlinc-config`, where
  `HELPER=${TF_BATTERY_HELPER:-/usr/libexec/thinfloe/peripheral-battery}`;
  keep the error capture, the `thinfloe: battery export not configured:`
  message, the `logger` call and `exit 0`. Update the header comment (the
  export follows the Settings switch; `TF_BATTERY_HOME` and
  `TF_BATTERY_HELPER` are for the test). In the service add, under
  `[Unit]`,
  `ConditionPathExists=/home/tfuser/.config/thinfloe/peripheral-battery`
  with a comment: the Settings switch; off skips every timer run; `enable`
  starts the service, which also gives `OnUnitActiveSec` a start to count
  from.

- [ ] **Step 4: Run the hook test**

  Run: `TMPDIR=/scratch/moetiker/tmp test/integration/test-thinlinc-hook.sh`
  Expected: `PASSED - 10-thinlinc hook`.

- [ ] **Step 5: Guard the condition.** In `test-session-packages.sh`, next
  to the `ExecStart=` check: `ok - the battery service runs only when
  switched on` if `grep -qx
  'ConditionPathExists=/home/tfuser/.config/thinfloe/peripheral-battery'`
  matches in `$PBU/thinfloe-peripheral-battery.service`, else FAIL with
  `fail=1`. Also assert the image has no
  `home/tfuser/.config/thinfloe/peripheral-battery` (default off).

- [ ] **Step 6: Build and run**

  Run: `make VERSION=pb rootfs && TF_UBUNTU_IMAGE=thinfloe-ubuntu:pb test/integration/test-session-packages.sh`
  Expected: all `ok`, the two new lines among them.

- [ ] **Step 7: Commit**

```bash
git add overlay/usr/libexec/thinfloe/addon-hooks/10-thinlinc \
        overlay/etc/systemd/system/thinfloe-peripheral-battery.service \
        test/integration/test-thinlinc-hook.sh test/integration/test-session-packages.sh
git commit -m "thinlinc: export the battery file only when switched on"
```

### Task 3: Menu state machine and job

**Files:**
- Modify: `src/thinfloe-menu/src/jobs.rs`, `src/thinfloe-menu/src/menu.rs`,
  `src/thinfloe-menu/src/settings.rs` (the event only),
  `src/thinfloe-menu/src/app.rs` (`start`, `user_event`),
  `src/thinfloe-menu/src/main.rs` (`Paths`)
- Test: unit tests in `jobs.rs` and `menu.rs`

**Interfaces:**
- Produces:
  - `settings.rs`: `SettingsEvent::Battery(bool)` (the tick box; `true` =
    switch on).
  - `jobs.rs`: `pub const BATTERY_SOURCE: &str = "battery";`
    `Paths.battery_cmd: Vec<String>`;
    `pub fn default_battery_cmd() -> Vec<String>` =
    `["sudo", "-n", "/usr/libexec/thinfloe/peripheral-battery"]`;
    `pub fn battery(cmd: &[String], on: bool, feed: &Arc<Feed>) -> JobResult`
    (appends `enable` or `disable`, `process_group(0)`, stdout and stderr
    logged under `BATTERY_SOURCE`, i.e. `run_logged(.., quiet: false, ..)`);
    `JobResult::Battery { on: bool, status: Result<Option<i32>, String> }`.
    No started-result: the group id is never needed (see the Decision).
  - `menu.rs`: `Job::Battery { on: bool }`; fields `pub battery_on: bool`
    (the state file exists, set by the app), `battery_run: Option<bool>`
    (the job's direction), `pub battery_changed: bool` (a job switched it
    successfully in this menu run); `pub fn battery_busy(&self) -> bool`.

- [ ] **Step 1: Write the failing tests.**
  - `jobs.rs` `battery_logs_both_streams_as_battery_and_passes_the_verb`:
    with `sh("echo \"out $0\"; echo err >&2; exit 0")`, `battery(&cmd, true,
    ..)` gives `JobResult::Battery { on: true, status: Ok(Some(0)) }` and the
    feed holds `(Some("battery"), "out enable")` and
    `(Some("battery"), "err")` (order between the streams not asserted);
    `on: false` passes `disable`; `default_battery_cmd()` equals the
    array above.
  - `menu.rs` (use `settings_menu` unless a test says otherwise):
    - `battery_tick_starts_one_job`: `battery_on = false`;
      `settings(SettingsEvent::Battery(true)) ==
      Some(Effect::Start(Job::Battery { on: true }))`; `battery_busy()`;
      a second `Battery(true)` and a `Battery(false)` give `None`.
    - `battery_tick_matching_the_state_does_nothing`: `battery_on = true`,
      `Battery(true)` gives `None`.
    - `battery_job_end_frees_the_row_and_notes_the_change`:
      `finished(JobResult::Battery { on: true, status: Ok(Some(0)) })` is
      `None`, `!battery_busy()`, `battery_changed`; the log's last line is
      `(Kind::Event, "Keyboard and mouse battery switched on.")`.
    - `a_failed_battery_job_is_a_warning`: status `Ok(Some(1))` logs
      `(Kind::Warn, "Switching the keyboard and mouse battery on failed (status 1).")`,
      `Ok(None)` `... failed (killed by a signal).`, `Err(e)` logs
      `"{e}."` as Warn; `battery_changed` stays false.
    - `the_end_of_session_question_ignores_a_battery_job`: on
      `Menu::new(grid(), false)` (index 3 is Log out) with
      `m.screen = Screen::Settings`, start a battery job, set `m.screen =
      Screen::Grid`; activating Log out (`m.activate(3, now)`) returns
      `Some(Effect::Start(Job::Save))` (no `Logout::Confirm`), and a later
      `JobResult::Battery` leaves `logout` unchanged.
    - `no_battery_job_during_a_logout`: with `logout` set,
      `Battery(true)` gives `None`.

- [ ] **Step 2: Run to fail**

  Run: `cd src/thinfloe-menu && RUSTUP_TOOLCHAIN=1.96.0 PATH=$HOME/.cargo/bin:$PATH systemd-run --user --scope -q -p MemoryMax=2G -- cargo test --locked -j 4`
  Expected: compile errors for the missing names.

- [ ] **Step 3: Implement.** In `Menu::settings`, handle
  `SettingsEvent::Battery(on)` before the `(verb, id)` match: `None` when
  `battery_run.is_some()` or `on == battery_on`, else set `battery_run =
  Some(on)` and return `Effect::Start(Job::Battery { on })`. In `finished`,
  `JobResult::Battery` clears `battery_run`, logs as in the tests, sets
  `battery_changed` on `Ok(Some(0))`, returns `None` and touches no logout
  state. `activate` (Log out/Restart/Power off) and every `addon_runs`
  check stay as they are. Add the Decision's reasons, in two lines, to the
  doc comment of `battery_run` and to the module doc ("A battery job is
  neither waited for nor stopped"). In `app.rs::start`, `Job::Battery { on
  }` runs `jobs::battery(&paths.battery_cmd, on, &feed)`; copy
  `battery_cmd` into the thread's `Paths` like `addon_cmd`. In
  `main.rs::build_menu`, `battery_cmd: jobs::default_battery_cmd()`; fix
  the `Paths` literals in the `jobs.rs` tests.

- [ ] **Step 4: Run the tests**

  Run: the Rust command above. Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/jobs.rs src/thinfloe-menu/src/menu.rs \
        src/thinfloe-menu/src/settings.rs src/thinfloe-menu/src/app.rs \
        src/thinfloe-menu/src/main.rs
git commit -m "menu: run the battery switch as a job of its own"
```

### Task 4: The Settings row and reading the switch

**Files:**
- Modify: `src/thinfloe-menu/src/settings.rs`, `src/thinfloe-menu/src/ui.rs`
  (`show_settings`, `settings_row`), `src/thinfloe-menu/src/app.rs`
  (`Config`, redraw, `user_event`), `src/thinfloe-menu/src/main.rs`
- Test: unit tests in `settings.rs` and `ui.rs`

**Interfaces:**
- Consumes: `SettingsEvent::Battery`, `Menu::battery_on`,
  `Menu::battery_busy()`, `Menu::battery_changed`, `JobResult::Battery`
  (Task 3).
- Produces:
  - `Row.tile` becomes `Option<usize>` (`None` for the battery row; the row
    draws the letter `K`); every add-on row uses `Some(i)`.
  - `pub const BATTERY_NAME: &str = "Keyboard and mouse battery";`
    `pub const BATTERY_ON: &str = "on · in ThinLinc as ~/thindrives/peripherals/battery";`
    `pub const BATTERY_NOTE: &str = "takes effect when ThinLinc starts again";`
  - `pub fn battery_row(menu: &Menu) -> Row`: `tile: None`, `id: ""`,
    `name: BATTERY_NAME`, `source: None`, `ticked: menu.battery_on`,
    `state`: `"off"` or `BATTERY_ON`, plus `" · " + BATTERY_NOTE` when
    `menu.battery_changed`; `button: None`, `busy: menu.battery_busy()`,
    the other flags false.
  - `status_items`: an item equal to `BATTERY_NOTE` gets `Tone::Ice`.
  - `pub fn battery_switch_path() -> PathBuf` and the pure
    `pub fn battery_switch_path_from(over: Option<OsString>, home:
    Option<OsString>) -> PathBuf`: `TF_BATTERY_SWITCH`, else
    `$HOME/.config/thinfloe/peripheral-battery`, else
    `/home/tfuser/.config/thinfloe/peripheral-battery` (the menu runs as
    tfuser; same fallback as `user_dir()` in `main.rs`).
  - `app.rs`: `Config.battery_switch: PathBuf`;
    `fn refresh_battery(&mut self)` sets
    `cfg.menu.battery_on = cfg.battery_switch.exists()`.
  - `ui.rs`: `settings_row(ui, row, icon, t, tick: impl FnOnce(bool) ->
    SettingsEvent)`.

- [ ] **Step 1: Write the failing tests.**
  - `settings.rs` `battery_row_says_off_on_and_the_note`: on
    `Menu::new(Grid { tiles: vec![], banner: vec![] }, false)` with
    `screen = Screen::Settings`: `battery_on = false` gives state `off`,
    `ticked == false`, `tile == None`; `battery_on = true` gives
    `BATTERY_ON`; with `battery_changed` the `status_items` end with
    `(BATTERY_NOTE, Tone::Ice)`; after `settings(SettingsEvent::Battery(false))`
    the row is `busy` and the last item is `working…`.
  - `settings.rs` `battery_switch_path_prefers_the_override_then_home`:
    tests the pure `battery_switch_path_from(over: Option<OsString>, home:
    Option<OsString>) -> PathBuf` that `battery_switch_path()` calls with
    `var_os("TF_BATTERY_SWITCH")` and `var_os("HOME")` (no `set_var`: it
    is `unsafe` in edition 2024 and races the other test threads):
    `(Some("/x"), Some("/h"))` gives `/x`; `(None, Some("/h"))` gives
    `/h/.config/thinfloe/peripheral-battery`; `(None, None)` gives
    `/home/tfuser/.config/thinfloe/peripheral-battery`.
  - `ui.rs` `the_settings_screen_ends_with_the_battery_row`: `texts_of` an
    open Settings menu (`addon_grid()`, Settings activated) with
    `battery_on = true` contains `Keyboard and mouse battery` and the
    status line `on · in ThinLinc as ~/thindrives/peripherals/battery`
    (one text, as the add-on status line is in
    `the_settings_screen_draws_a_row_per_addon`); with
    `battery_changed` it is
    `on · in ThinLinc as ~/thindrives/peripherals/battery · takes effect when ThinLinc starts again`.
    On a menu with no add-on tiles (`m.screen = Screen::Settings` set
    directly), `No add-ons are defined.` and
    `Keyboard and mouse battery` are both drawn.
  - `ui.rs` `the_settings_panel_is_as_wide_as_the_tile_group`: now 5 rows
    (four add-ons plus battery); the last row's top is below the fourth's.
    Update any other row count the change breaks, only by +1.

- [ ] **Step 2: Run to fail**

  Run: the Rust command (Global Constraints). Expected: compile errors.

- [ ] **Step 3: Implement.** In `show_settings`, after the add-on loop
  (inside the same `ScrollArea`), draw `settings::battery_row(menu)` with
  the same `Frame` and call `settings_row(.., None, &t,
  SettingsEvent::Battery)`; add-on rows pass `|on| SettingsEvent::Tick {
  id: row.id.clone(), on }`. Icons: `row.tile.and_then(|i|
  icons.get(i)).and_then(Option::as_ref)`. In `app.rs`: call
  `refresh_battery()` after a `UiEvent::Tile` whose effect left
  `menu.screen == Screen::Settings`, and in `user_event` after a
  `JobResult::Battery` (next to the `addon` refresh). In `main.rs`: fill
  `battery_switch` and set `menu.battery_on` in `build_menu` from it.

- [ ] **Step 4: Run the tests**

  Run: the Rust command. Expected: all pass. Also
  `cargo clippy --locked -- -D warnings` with the same prefix, clean.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/settings.rs src/thinfloe-menu/src/ui.rs \
        src/thinfloe-menu/src/app.rs src/thinfloe-menu/src/main.rs
git commit -m "menu: a Settings row switches the keyboard and mouse battery"
```

### Task 5: Documentation and whole-branch check

**Files:**
- Modify: `CLAUDE.md` (the "Peripheral battery" bullet under Customizing
  the System; the Add-ons "The menu." bullet)

- [ ] **Step 1: Edit CLAUDE.md.** In the "Peripheral battery" bullet, say:
  off by default; the Settings row `Keyboard and mouse battery` runs `sudo
  -n /usr/libexec/thinfloe/peripheral-battery enable|disable`; the state
  file `~/.config/thinfloe/peripheral-battery` (`on`, travels with
  `save-home`, written only by the helper); on = the service runs
  (`ConditionPathExists=`), the battery file exists and the hook writes
  the two NFS keys; off = none of it, and `disable` removes the keys and
  the file; the subcommands and `thinlinc-config` called by the hook;
  a change reaches ThinLinc when the client starts again. Replace "The
  ThinLinc add-on hook ... writes" with the switch-dependent wording, keep
  the exposure sentence but start it with "When switched on,". In the
  Add-ons "The menu." bullet, one sentence: the battery job is neither
  listed nor waited for at Log out / Restart / Power off, with the reason
  from the Decision above. Under the not-verified list, add: the switch on
  hardware, and that the timer fires again after `enable` on a boot whose
  first run was skipped.

- [ ] **Step 2: Run every offline test**

  Run: `python3 -m unittest discover -s test/unit`,
  `TMPDIR=/scratch/moetiker/tmp test/integration/test-thinlinc-hook.sh`,
  the Rust command, and `make VERSION=pb rootfs && TF_UBUNTU_IMAGE=thinfloe-ubuntu:pb test/integration/test-session-packages.sh`.
  Expected: all pass. `git status --short` shows ` M artifacts` and
  nothing else uncommitted after the commit below.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: the Settings switch for the peripheral battery"
```

- [ ] **Step 4: Hardware check (owner, not an agent: needs ssh).** On a
  machine with the built image: `status` says `off`, no battery file, the
  mandatory file has no NFS keys; tick the row; `systemctl list-timers
  thinfloe-peripheral-battery.timer` shows a next run, the file appears,
  ThinLinc started afterwards shows `~/thindrives/peripherals/battery`;
  untick; the file and keys are gone.
