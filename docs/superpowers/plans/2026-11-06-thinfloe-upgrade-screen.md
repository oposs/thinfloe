# Upgrade screen in the menu (Part A2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Upgrade tile opens an Upgrade screen in `thinfloe-menu` where the person picks release, drive, action and options on one page, confirms once, and then watches each step of `thinfloe-upgrade write` with a bar, MB/s and time left; a daily check marks the tile when a newer release exists.

**Architecture:** The root tool stays `overlay/usr/bin/thinfloe-upgrade` (A1, merged). It gains `drives --no-size` (so the 2 s poll sends no network request) and `check` (release version and image size, written to `/run/thinfloe/upgrade/status`, run by a timer). The menu gets four pure modules (`upgrade_proto.rs` for the `@` lines, `drives.rs` for the drive list and its rules, `release.rs` for the status file and version order, `speed.rs` for rate and time left), a state machine `upgrade.rs` with no egui in it, three job runners in `jobs.rs`, and the drawing in `upgrade_ui.rs`. The write job joins the end-of-session list next to the add-on jobs in `menu.rs`.

**Tech Stack:** Rust 1.96, egui 0.37, shadcn-egui 0.8.0 (`Button`, `Item`, `Progress`, `Input`, `checkbox`, `Badge`), softbuffer CPU raster (unchanged), `serde_json` 1 (new, `Value` only, no derive); Python 3 stdlib for the tool, `unittest`; systemd timer units.

**Spec:** `docs/superpowers/specs/2026-11-02-thinfloe-settings-and-writer-design.md`, §"Part A: the image writer" (113-236), §Testing (386), §"Settled after the builder check" 1, 5, 10, §"Settled after the pushback review" 1, 2, 5, 10, 11, §"Left to the builder", §"Not verified". The interface contract is the top docstring of `overlay/usr/bin/thinfloe-upgrade`. Part 5 contract: `docs/superpowers/specs/2026-10-21-thinfloe-design.md` §"Part 5: GitHub releases".

## Global Constraints

- Reuse the Part 0 worktree, which keeps its built image in `artifacts/` and its build cache: `git worktree move /scratch/oetiker/claude-worktrees/thinfloe-menu-shadcn /scratch/oetiker/claude-worktrees/thinfloe-upgrade-screen`, then there `git switch -c feature/upgrade-screen main` after Part 0's follow-up (`fix/menu-review-minors`) is merged. The owner merges to `main` without a pull request.
- Every `make` call in that worktree passes `VERSION=upgrade-screen` and runs as `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=upgrade-screen …` with `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests`. Its `artifacts` is already a real directory (`artifacts.link` holds the shared link, `skip-worktree` is set); undo that before the worktree is removed: `rm -r artifacts && mv artifacts.link artifacts && git update-index --no-skip-worktree artifacts`.
- At most 4 cores: `cargo … -j4`, `CARGO_BUILD_JOBS=4`, `BUILD_JOBS=4`. Menu tests run as `cd src/thinfloe-menu && systemd-run --user --scope -q -p MemoryMax=4G -- cargo test --locked -j4 --lib <filter>`. Python tests run with `python3 -B` (a `__pycache__` under `overlay/usr/bin` makes the image stale).
- No temp data in `/tmp` on the dev host. Menu tests use `crate::testdir::TestDir`.
- The menu runs as tfuser and calls the tool only as `sudo -n /usr/bin/thinfloe-upgrade <args>`, each call in a process group of its own (`process_group(0)`), like `jobs::addon`. No root code in the menu process.
- The menu draws on the CPU: no `wgpu`, `glow`, `eframe`; no `Spinner` (it re-rasterises its texture every frame); no animation. Every colour from `Theme::current(ctx)`, every size a named constant; `upgrade_ui.rs` joins the file list of `brand::tests::ui_code_has_no_style_literals`.
- Zero labwc keybindings stay. Nothing in this plan adds one.
- `X-TF-Action=upgrade` is honoured only in shipped tile files, like `settings`. The tile keeps the file name `50-upgrade-image.desktop`.
- The contract points the tool's docstring states, verbatim in effect: `@result` wins over the exit code; a run with no `@result` is failed; `@result ok` may carry no reason; exit 130 after `@done write` or `@done verify` of a fresh install may leave a good, unwiped stick; a cancel during `switch` ends with `@result ok` and the screen says the cancel came too late; only the first signal counts; only one write runs at a time (exit 2 for a second); a row with `examined: false` says nothing about `is_thinfloe`, `has_tl_old`, `has_tl_new`.
- The menu never sends SIGKILL to a write job. A SIGKILL reaches only `sudo`; the tool keeps running and holding `/run/thinfloe/upgrade.lock`.
- Drive list: USB or removable drives by default; others only with "Show all drives", and erasing one needs its model and size typed back. A drive with a mounted partition is never offered, except the boot stick for Upgrade.
- The menu passes `--source` only when the person entered an address under Advanced; otherwise the tool's own default applies (it moves to the GitHub release in Part 5).
- The Upgrade tile is hidden on netboot (open question 4 may change this).
- Changelog entries in `CHANGES.md` under `[Unreleased]`, for users, at most three sentences, no release header (rules in `~/.claude/CLAUDE.md`). `CLAUDE.md` is updated in the task that changes the behaviour it describes.
- Prose in code comments and docs follows the house style: plain English, no em dashes.

## Review Focus

1. **The stick is pulled out while it is written.** The tool ends with `@result failed <reason>` and the next drive poll no longer lists the stick. The screen must show that reason with Try again, and the choice of the vanished drive must be gone, not left selectable for a second write. → Task 7, tests `a_pulled_stick_shows_the_tools_reason` and `a_drive_that_disappears_clears_the_choice`.
2. **Two sticks of one model swap device nodes between choosing and confirming.** The choice is tied to the drive's `id`, not to `/dev/sdX`; a poll that shows the chosen device with another id clears the choice, and `write` always carries `--expect-id`. → Task 7, test `a_device_with_a_new_id_is_not_the_chosen_drive`.
3. **Restart, Power off or Log out while a stick is written.** The dialog lists "Writing SanDisk Cruzer" and waits; "Restart anyway" on a fresh install shows that the stick will not start until it is written again and sends SIGTERM (never SIGKILL); Log out only waits. → Task 8, test `restart_waits_for_the_write_and_anyway_warns_on_fresh`.
4. **A progress total that is missing or wrong** (`@progress N -`, or `done > total` from a bad frame header). No percentage and no bar then, only bytes written and MB/s, no time left; never a bar past 100 %. → Task 5, test `unknown_or_overflowing_total_shows_no_percentage`.
5. **The `@result` line arrives just before the tool exits.** A success must never be shown as "ended without a result" because the exit was seen before the last stdout line. → Task 6, test `a_result_printed_just_before_exit_is_never_lost`.

---

### Task 1: Tool: `drives --no-size` and `check`

**Files:**
- Modify: `overlay/usr/bin/thinfloe-upgrade` (docstring, constants, `_batch_parser`, `_batch_run`, new `release_check`, `write_status`, `read_installed_version`; `main` dispatches `check`)
- Test: `test/unit/test_upgrade_batch.py` (new classes `TestDrivesNoSize`, `TestCheck`, `TestWriteArgsFixture`)
- Create: `test/unit/upgrade-write-args.tsv` (shared with Task 7)
- Modify: `CLAUDE.md` (the `thinfloe-upgrade drives … write …` bullet in "Persistent Home Directory": `--no-size`, `check`, the status file)

**Interfaces:**
- Produces (CLI): `thinfloe-upgrade drives [--all] [--source URL] [--no-size]`: with `--no-size` no request is sent and every row has `"fits": null`. `thinfloe-upgrade check [--source URL]`: prints `key=value` lines on stdout (`latest`, `image_bytes`, `checked`, `source`, `error`; a missing value is an empty string) and exits 0 when both requests were answered, 1 when either failed (the lines are still printed, `error` says which), 2 on misuse. Only when `--source` is absent or equals `DEFAULT_URL` does it also write the same lines atomically (`status.new`, `os.replace`) to `UPGRADE_STATUS = "/run/thinfloe/upgrade/status"`, directory mode 0755, file 0644, plus `installed=` from `/etc/thinfloe/version` for SSH readers.
- Produces (constants): `VERSION_ASSET = "version"` (open question 1), `UPGRADE_STATUS`, `INSTALLED_VERSION_FILE = "/etc/thinfloe/version"`, `VERSION_RE = r"[0-9A-Za-z.+-]{1,32}"`.
- Produces (fixture): `upgrade-write-args.tsv`, one argv per line, tab-separated, `#` comments; each line is a `write` call the menu may make (Task 7 writes the same lines from `write_args`).

- [ ] **Step 1: Write the failing tests**
  - `TestDrivesNoSize.test_no_size_sends_no_request`: patch `image_size` to raise; `_batch_run` with `["drives", "--no-size"]` and a patched `enumerate_drives` returning one drive prints JSON whose row has `"fits": None`.
  - `TestCheck.test_check_reads_version_and_size`: patch the curl call of the version fetch to return `b"4.1.0\n"` and `image_size` to return `3148873728`; `check` prints `latest=4.1.0` and `image_bytes=3148873728`, `error=` empty, exit 0, and `UPGRADE_STATUS` (pointed into a temp dir) holds the same lines plus `installed=`.
  - `test_missing_version_file_is_unknown_not_failed`: version fetch exits 22 (HTTP 404); `latest=` empty, `error=no version file at the source`, exit 1, status file written.
  - `test_hostile_version_text_is_refused`: bodies `b"4.1.0; rm -rf /\n"`, `b"\x1b[31m4\n"`, 4 KiB of `9` give `latest=` empty and `error=version file not usable`.
  - `test_custom_source_does_not_write_the_status`: `check --source http://127.0.0.1:1/x` leaves `UPGRADE_STATUS` absent.
  - `test_status_write_is_atomic`: after a run no `status.new` is left; a read during `os.replace` (patch it to assert the old file still parses) sees a whole file.
  - `TestWriteArgsFixture.test_every_fixture_line_parses`: every non-comment line of `upgrade-write-args.tsv` parses with `_batch_parser()` without `UsageError`, and `args.command == "write"`.
- [ ] **Step 2:** `python3 -B -m unittest test.unit.test_upgrade_batch -v` → FAIL (`--no-size` unknown, no `check`).
- [ ] **Step 3: Implement.** The version fetch is `curl -fsSL --max-filesize 256 --max-time 30 -- <source>/version`, run with `**SPAWN`; the first line, stripped, must fullmatch `VERSION_RE`. The status file values are single-line (newlines replaced by spaces, cut to 200 characters). `check` takes no lock: it touches no drive. Fixture lines to write now:
  ```
  write	--device	/dev/sdb	--mode	fresh	--expect-id	sn-AA1:16008609792:3f2a	--verify
  write	--device	/dev/sdb	--mode	fresh	--expect-id	nosn:16008609792:3f2a	--copy-home	--verify
  write	--device	/dev/sda	--mode	upgrade	--expect-id	sn-BOOT:31914983424:9c01
  write	--device	/dev/nvme0n1	--mode	fresh	--expect-id	sn-X:512110190592:77aa	--allow-internal
  write	--device	/dev/sdc	--mode	upgrade	--expect-id	sn-C:8004304896:0011	--source	https://example.org/rel
  ```
- [ ] **Step 4:** Same command → PASS; `make test-unit` → PASS.
- [ ] **Step 5:** Update the `CLAUDE.md` bullet. Commit `upgrade: drives --no-size and a release check for the menu`.

### Task 2: The daily release check

**Files:**
- Create: `overlay/etc/systemd/system/thinfloe-upgrade-check.service`, `overlay/etc/systemd/system/thinfloe-upgrade-check.timer`
- Modify: `containers/ubuntu/Dockerfile` (`systemctl enable thinfloe-upgrade-check.timer` next to the add-on timers, line ~393)
- Modify: `test/integration/test-session-packages.sh` (present: the timer's `timers.target.wants` link; the service has no `Before=`)
- Modify: `CLAUDE.md` (a sentence in "Add-ons"-style prose under the `thinfloe-upgrade` bullets: the timer, its schedule, USB only)

**Interfaces:**
- Consumes: `thinfloe-upgrade check` (Task 1).
- Produces: `/run/thinfloe/upgrade/status` refreshed on USB boots, which the menu watches (Task 9).

- [ ] **Step 1: Write the failing test.** In `test-session-packages.sh`: `present "upgrade check timer enabled" 'etc/systemd/system/timers\.target\.wants/thinfloe-upgrade-check\.timer$'` and a check that `thinfloe-upgrade-check.service` contains `ConditionPathIsMountPoint=/run/thinfloe/storage/efi` and no `Before=` line.
- [ ] **Step 2:** The assertions fail on the current image (run only the new lines by hand against `unsquashfs -l` of the last image, or wait for the image build in Task 11).
- [ ] **Step 3: Write the units.** Service: `Type=oneshot`, `ExecStart=/usr/bin/thinfloe-upgrade check`, `Wants=network-online.target`, `After=network-online.target`, `ConditionPathIsMountPoint=/run/thinfloe/storage/efi` (netboot has no ESP there), `Restart=on-failure`, `RestartSec=1h` (the retry the spec left to the builder). Timer: `OnBootSec=5min`, `OnUnitActiveSec=1d`, `RandomizedDelaySec=1h`, `WantedBy=timers.target` (schedule: open question 2). A comment in the timer says why there is a boot firing: the status lives in `/run`, so a machine that runs less than a day would otherwise never see a marker.
- [ ] **Step 4:** Assertions pass after the image build (Task 11 re-runs `make test`); `scripts/check-boot-smoke.sh` finds no ordering cycle in `make qemu-smoke`.
- [ ] **Step 5:** Commit `upgrade: check for a new release after boot and once a day`.

### Task 3: Progress lines and release status in the menu

**Files:**
- Create: `src/thinfloe-menu/src/upgrade_proto.rs`, `src/thinfloe-menu/src/release.rs`
- Modify: `src/thinfloe-menu/src/lib.rs` (both modules)

**Interfaces:**
- Produces (`upgrade_proto.rs`):
  - `pub enum Msg { Step { id: String, text: String }, Progress { done: u64, total: Option<u64> }, Done { id: String }, Result { ok: bool, reason: String } }` (derive `Debug, Clone, PartialEq, Eq`)
  - `pub fn parse(line: &str) -> Option<Msg>`: an id is 1 to 32 of `[a-z0-9_-]`; text and reason are cut to `TEXT_MAX = 300` characters; total `-` or `0` is `None`; anything else (unknown verb, bad number, missing id) is `None` and the caller logs the line as output.
- Produces (`release.rs`):
  - `pub const STATUS_PATH: &str = "/run/thinfloe/upgrade/status";`, `STATUS_BYTES: u64 = 4096`
  - `pub struct Release { pub latest: Option<String>, pub image_bytes: Option<u64>, pub checked: Option<u64>, pub source: Option<String>, pub error: Option<String> }` (derive `Debug, Clone, Default, PartialEq, Eq`)
  - `pub fn parse_status(text: &str) -> Release`, `pub fn read_status(path: &Path) -> Release` (bounded with `bounded::read_bounded`; a missing file is `Release::default()`)
  - `pub fn newer(latest: &str, installed: &str) -> bool` (semantic versions: `MAJOR.MINOR.PATCH[-pre][+build]`, build ignored, a pre-release sorts below its release, pre-release identifiers compared numerically when both are digits; either side unparsable is `false`)
  - `pub fn update_available<'a>(r: &'a Release, installed: Option<&str>) -> Option<&'a str>`
  - `pub struct Watcher` with `pub fn tick(&mut self, path: &Path) -> bool` (true on the first tick and whenever size or mtime changed)

- [ ] **Step 1: Write the failing tests.**
  - `upgrade_proto`: `parse("@step write Downloading and writing")` → `Step { id: "write", text: "Downloading and writing" }`; `parse("@progress 10 100")` → `Progress { done: 10, total: Some(100) }`; `parse("@progress 10 -")` → total `None`; `parse("@done verify")`; `parse("@result ok")` → `Result { ok: true, reason: "" }`; `parse("@result failed the stick was removed")`; `None` for `"@step"`, `"@step WRITE x"`, `"@progress x 1"`, `"@result maybe"`, `"plain text"`, `"@progress 18446744073709551616 -"`; a 2000-character reason is cut to 300.
  - `release`: `parse_status("latest=4.1.0\nimage_bytes=3148873728\nchecked=1762412345\nsource=https://x\nerror=\n")` fills every field, `error` `None`; unknown keys and lines without `=` are ignored; a 10 KiB file reads as its first 4096 bytes; `newer("4.1.0","4.0.0")`, `newer("4.0.10","4.0.9")`, `newer("4.0.0","4.0.0-dev+abc")` are true; `newer("4.0.0-dev+abc","4.0.0")`, `newer("4.0.0","4.0.0")`, `newer("4.1","4.0.0")`, `newer("4.1.0","")`, `newer("garbage","4.0.0")` are false; `update_available` is `None` when `installed` is `None` (the spec's "version unknown": no marker).
  - `Watcher`: true on first tick, false when unchanged, true after a rewrite.
- [ ] **Step 2:** `cargo test --locked -j4 --lib upgrade_proto release` (with the MemoryMax wrapper) → FAIL (modules missing).
- [ ] **Step 3:** Implement as specified.
- [ ] **Step 4:** Same command → PASS.
- [ ] **Step 5:** Commit `menu: read the writer's progress lines and the release status`.

### Task 4: The drive list and what each drive offers

**Files:**
- Create: `src/thinfloe-menu/src/drives.rs`
- Modify: `src/thinfloe-menu/Cargo.toml`, `Cargo.lock` (`cargo add serde_json@1 --no-default-features --features std`), `src/thinfloe-menu/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub struct Drive { pub device: String, pub model: String, pub size_bytes: u64, pub serial: Option<String>, pub transport: Option<String>, pub removable: bool, pub mounted: bool, pub is_boot: bool, pub is_thinfloe: bool, pub has_tl_old: bool, pub has_tl_new: bool, pub examined: bool, pub id: String, pub fits: Option<bool>, pub version: Option<String> }` (derive `Debug, Clone, PartialEq, Eq`; `version` is read when present, see open question 5)
  - `pub const DRIVES_MAX: usize = 64;`
  - `pub fn parse(json: &str) -> Result<Vec<Drive>, String>`: a JSON array of objects; a row whose `device` does not match `^/dev/[a-z0-9]{1,32}$` or whose `id` is empty, starts with `-` or holds whitespace or a control character is dropped; text fields cut to 64 characters; more than `DRIVES_MAX` rows is an error.
  - `pub enum Mode { Fresh, Upgrade }` (derive `Debug, Clone, Copy, PartialEq, Eq`)
  - `pub fn internal(d: &Drive) -> bool`: `!d.removable && d.transport.as_deref() != Some("usb") && !d.is_boot`
  - `pub fn tag(d: &Drive) -> String`: `"this machine's stick"`, `"ThinFloe stick"` (`", version 4.0.0"` when known, `", has fallback"` when `has_tl_old`, `", upgrade pending"` when `has_tl_new`), `"other drive"`, `"internal drive"` when `internal`
  - `pub fn modes(d: &Drive, image_bytes: Option<u64>) -> Vec<(Mode, Result<(), String>)>`: boot stick `[Upgrade]`; other ThinFloe stick `[Upgrade, Fresh]`; any other drive `[Fresh]`; `Upgrade` and `Fresh` refused as below
  - `pub fn format_size(bytes: u64) -> String` (decimal: `"512 MB"`, `"7.8 GB"`, `"16 GB"` from 10 GB on; `"needs"` texts round up)
  - `pub fn confirm_text(d: &Drive) -> String` (`"<model> <size>"`), `pub fn typed_matches(typed: &str, d: &Drive) -> bool` (case and runs of whitespace ignored)
- Refusal reasons, exact text:
  - mounted and not boot: `"in use: a partition is mounted"`
  - `examined == false` (no earlier examined row with this id): `"another write is running; wait for it"`
  - Fresh on the boot stick: `"the boot drive of this machine (fresh install not possible)"`
  - Fresh when `fits == Some(false)` or `size_bytes < image_bytes`: `"too small (needs 3.2 GB)"` from `format_size` of the image size, rounded up

- [ ] **Step 1: Write the failing tests.** `parses_the_tools_json` on the exact output of `drives_json` for two rows (copy a line printed by `test_upgrade_batch.TestDriveList`); `a_row_with_a_hostile_device_or_id_is_dropped` (`"/dev/sdb;x"`, `"../sda"`, id `"-x"`, id `"a b"`); `null_fields_are_none`; `sixty_five_rows_are_refused`; `the_boot_stick_offers_only_upgrade`; `another_thinfloe_stick_offers_both`; `a_mounted_stick_is_refused_with_the_reason`; `a_small_stick_says_what_it_needs` (`size_bytes` 2_000_000_000, image 3_148_873_728 → `"too small (needs 3.2 GB)"`); `an_unexamined_row_offers_nothing`; `format_size` table (`15_623_782_400` → `"16 GB"`, `7_800_000_000` → `"7.8 GB"`, `512_000_000` → `"512 MB"`); `typed_matches(" sandisk   cruzer 16 gb ", d)` true, `"SanDisk Cruzer"` false.
- [ ] **Step 2:** `cargo test --locked -j4 --lib drives` → FAIL.
- [ ] **Step 3:** Implement; map `serde_json::Value` by hand (no derive, no `serde` feature flags beyond `std`).
- [ ] **Step 4:** Same command → PASS; `cargo tree -i serde_json` shows it under `thinfloe-menu` only.
- [ ] **Step 5:** Commit `menu: the drive list and which actions each drive allows`.

### Task 5: Rate and time left

**Files:**
- Create: `src/thinfloe-menu/src/speed.rs`
- Modify: `src/thinfloe-menu/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub const WINDOW: Duration = Duration::from_secs(10); pub const MIN_SPAN: Duration = Duration::from_secs(3); pub const SAMPLES_MAX: usize = 64;`
  - `pub struct Speed` with `pub fn new() -> Speed`, `pub fn sample(&mut self, now: Instant, done: u64)` (a `done` below the last sample starts over), `pub fn rate(&self) -> Option<f64>` (bytes per second over the samples within `WINDOW`, `None` until they span `MIN_SPAN`), `pub fn eta(&self, done: u64, total: Option<u64>) -> Option<Duration>` (`None` without a total, without a rate, or when `done > total`)
  - `pub fn fraction(done: u64, total: Option<u64>) -> Option<f32>` (`None` when `total` is `None` or `done > total`)
  - `pub fn rate_text(bytes_per_s: f64) -> String` (`"12.3 MB/s"`), `pub fn eta_text(d: Duration) -> String` (`"less than a minute left"`, `"about 4 min left"`, `"about 1 h 05 min left"`), `pub fn bytes_text(done: u64, total: Option<u64>) -> String` (`"1.2 GB of 3.1 GB"`, `"1.2 GB written"`)

- [ ] **Step 1: Write the failing tests.** `rate_follows_the_last_ten_seconds` (samples 0..20 s at 10 MB/s, then 5 s at 2 MB/s: rate between 2 and 10 MB/s and below the first; after 10 more seconds at 2 MB/s, within 5 % of 2 MB/s); `no_rate_before_three_seconds`; `a_counter_that_restarts_starts_over` (the download counter after the save step); `unknown_or_overflowing_total_shows_no_percentage` (Review Focus 4: `fraction(5, None)`, `fraction(11, Some(10))`, `eta(11, Some(10))` are `None`; `bytes_text(11, Some(10))` is `"… written"` form, never `"of"`); text table for `rate_text`, `eta_text`, `bytes_text`; at most `SAMPLES_MAX` samples kept after 10 000 calls.
- [ ] **Step 2:** `cargo test --locked -j4 --lib speed` → FAIL.
- [ ] **Step 3:** Implement with a `VecDeque<(Instant, u64)>`.
- [ ] **Step 4:** Same command → PASS.
- [ ] **Step 5:** Commit `menu: write speed and time left from the progress lines`.

### Task 6: Running the tool from the menu

**Files:**
- Modify: `src/thinfloe-menu/src/jobs.rs`

**Interfaces:**
- Consumes: `upgrade_proto::{parse, Msg}`, `drives::{parse, Drive}`, `release::{parse_status, Release}`.
- Produces:
  - `pub const UPGRADE_LOG_SOURCE: &str = "upgrade";`, `pub const UPGRADE_DRAIN: Duration = Duration::from_secs(5);`, `pub const DRIVES_BYTES: usize = 256 * 1024;`, `pub const CHECK_BYTES: usize = 4096;`
  - `pub fn default_upgrade_cmd() -> Vec<String>` → `["sudo", "-n", "/usr/bin/thinfloe-upgrade"]`
  - `Paths` gains `pub upgrade_cmd: Vec<String>`
  - `pub fn upgrade_write(cmd: &[String], args: &[String], feed: &Arc<Feed>, on_spawn: impl FnOnce(i32), on_msg: impl Fn(Msg) + Send + 'static) -> Result<Option<i32>, String>`: stdout lines that `parse` accepts go to `on_msg`, every other stdout line and all of stderr to the feed under `UPGRADE_LOG_SOURCE`; after the exit it waits for the stdout pump up to `UPGRADE_DRAIN` (not `DRAIN_GRACE`), so `on_msg` has seen every line before it returns.
  - `pub fn upgrade_drives(cmd: &[String], all: bool, feed: &Arc<Feed>) -> Result<Vec<Drive>, String>`: runs `drives --no-size [--all]`, stdout read up to `DRIVES_BYTES` (more is an error), stderr to the feed.
  - `pub fn upgrade_check(cmd: &[String], source: Option<&str>, feed: &Arc<Feed>) -> Result<Release, String>`: runs `check [--source URL]`; exit 0 or 1 both return the parsed lines (the `error` field carries a failure); any other status or no lines is `Err`.
  - `JobResult` gains `UpgradeStarted { pgid: i32 }`, `UpgradeMsg(Msg)`, `UpgradeEnded { status: Result<Option<i32>, String> }`, `Drives { all: bool, result: Result<Vec<Drive>, String> }`, `Release { custom: bool, result: Result<Release, String> }`.

- [ ] **Step 1: Write the failing tests** (system binaries only, as the module comment says):
  - `upgrade_write_routes_progress_and_logs_the_rest`: `sh -c 'echo "@step write W"; echo plain; echo err >&2; echo "@result ok"'` gives `on_msg` `[Step, Result]` in order, the feed `plain` and `err` under `"upgrade"`, status `Ok(Some(0))`.
  - `a_result_printed_just_before_exit_is_never_lost` (Review Focus 5): 50 runs of `sh -c 'i=0; while [ $i -lt 2000 ]; do echo "@progress $i 2000"; i=$((i+1)); done; echo "@result ok"'`; every run's last `on_msg` is `Result { ok: true, .. }`.
  - `a_write_job_has_a_process_group_of_its_own` (as `an_addon_job_has_a_process_group_of_its_own`).
  - `sudo_refusal_reaches_the_log`: `sh -c 'echo "sudo: a password is required" >&2; exit 1'` gives no `on_msg` call, the line in the feed, `Ok(Some(1))`.
  - `drives_output_over_the_cap_is_an_error`: `sh -c 'head -c 300000 /dev/zero | tr "\0" " "'` → `Err`.
  - `drives_passes_no_size_and_all`: `cmd = sh -c 'echo "[{\"device\":\"/dev/sdb\",\"id\":\"$*\"}]"' sh` → the parsed id text contains `drives --no-size --all` (the arguments reached the tool).
  - `check_exit_one_still_returns_the_lines`.
- [ ] **Step 2:** `cargo test --locked -j4 --lib jobs` → FAIL.
- [ ] **Step 3:** Implement. Reuse `pump` with a line callback rather than a second reader: the `Splitter` gives runs, `Line::plain()` gives the text that `parse` sees.
- [ ] **Step 4:** Same command → PASS (run it three times; the race test must not flake).
- [ ] **Step 5:** Commit `menu: run thinfloe-upgrade drives, check and write`.

### Task 7: The Upgrade screen's state machine

**Files:**
- Create: `src/thinfloe-menu/src/upgrade.rs`
- Modify: `src/thinfloe-menu/src/lib.rs`
- Test: in `upgrade.rs`; reads `test/unit/upgrade-write-args.tsv` (Task 1) and `overlay/usr/bin/thinfloe-upgrade`

**Interfaces:**
- Consumes: Tasks 3 to 6.
- Produces:
  - `pub const POLL: Duration = Duration::from_secs(2);` (the drive poll, also the repaint interval of Task 9)
  - `pub enum Stage { Choose, Running, Finished(Outcome) }`
  - `pub enum Outcome { Done { boot_stick: bool, late_cancel: bool }, Failed { reason: String }, Refused { reason: String }, Cancelled { mode: Mode, written: bool }, NoResult { status: String } }`
  - `pub enum StepMark { Waiting, Running, Done, Failed }`, `pub struct StepView { pub id: String, pub label: String, pub mark: StepMark }`
  - `pub enum UpgradeEvent { Choose(String), Mode(Mode), CopyHome(bool), Verify(bool), ShowAll(bool), Advanced(bool), Source(String), Typed(String), CheckNow, Confirm, Cancel, CancelYes, CancelNo, TryAgain, RestartNow, Back }`
  - `pub enum Request { Check { source: Option<String> }, Drives { all: bool }, Write { args: Vec<String> }, Stop, Restart, Leave }`
  - `pub struct Upgrade` with public read fields for drawing (`stage`, `drives`, `drives_error`, `release`, `custom_release`, `installed`, `chosen`, `mode`, `copy_home`, `verify`, `show_all`, `advanced`, `source_text`, `typed`, `steps`, `progress`, `speed`, `cancel_asked`, `cancelling`, `checking`) and:
    - `pub fn new() -> Upgrade` (`verify` on, the rest off or empty)
    - `pub fn open(&mut self, now: Instant) -> Option<Request>` (Check, unless one runs; the next poll at `now`)
    - `pub fn poll(&mut self, now: Instant) -> Option<Request>` (`Drives` every `POLL = 2 s` in `Stage::Choose` when no drives job runs)
    - `pub fn event(&mut self, e: UpgradeEvent, restore_failed: bool, now: Instant) -> Option<Request>`
    - `pub fn drives_listed(&mut self, all: bool, r: Result<Vec<Drive>, String>, now: Instant) -> Option<String>` (returns a log line only when the error text differs from the last one logged)
    - `pub fn released(&mut self, custom: bool, r: Result<Release, String>)`, `pub fn release_file(&mut self, r: Release)`
    - `pub fn started(&mut self, pgid: i32)`, `pub fn message(&mut self, m: Msg, now: Instant)`, `pub fn ended(&mut self, status: Result<Option<i32>, String>)`
    - `pub fn write_args(&self) -> Result<Vec<String>, String>`, `pub fn summary(&self) -> Option<String>`, `pub fn confirm_label(&self) -> Option<String>`
    - `pub fn job_name(&self) -> Option<String>` (`"Writing SanDisk Cruzer"` for fresh, `"Upgrading SanDisk Cruzer"`), `pub fn fresh_running(&self) -> bool`, `pub fn pgid(&self) -> Option<i32>`, `pub fn writing(&self) -> bool`
    - `pub fn outcome_text(&self) -> (String, Tone)` (the one sentence of the end; `settings::Tone`)
- Rules the tests pin:
  - The choice is `(id, device, mode)`. A poll that no longer lists the id, or lists the device with another id, clears `chosen`, `mode` and `typed`. A row with `examined: false` keeps the examined fields of the last row with the same id.
  - `Confirm` starts `Write` only when: the chosen drive is in the last list with the same id; its mode is offered and allowed; the source field is empty or matches `^https?://\S+$`; an internal drive with `Fresh` has `typed_matches`; no job runs. Otherwise `None`.
  - `write_args`: `write --device D --mode fresh|upgrade --expect-id ID`, then `--source URL` when the field is not empty, `--copy-home` (fresh only, never when `restore_failed`), `--verify` (fresh only), `--allow-internal` when `internal(d)`, in that order.
  - Planned steps (ids and labels, shown as waiting before the tool reaches them): fresh `write` "Downloading and writing", `verify` "Checking the stick" (with verify), `stamp` "Marking the stick", `home` "Copying the home directory" (with copy home), `save` "Saving this machine's home"; upgrade `save` "Saving the home directory" (boot stick only), `download` "Downloading the new release", `check` "Checking the files", `switch` "Switching to the new release". An `@step` with an unplanned id is appended with the tool's text; more than 16 steps are not added. `@progress` belongs to the running step and resets `speed` when the step changes.
  - `Cancel` on fresh only sets `cancel_asked` (the inline question); `CancelYes` returns `Stop` and sets `cancelling`; on upgrade `Cancel` returns `Stop` at once. A second `Cancel` while `cancelling` returns `None` (only the first signal counts).
  - `ended`: with a `@result` that is `ok`: `Done { boot_stick, late_cancel: cancelling }`. Exit 130: `Cancelled { mode, written }`, where `written` is true when `@done write` (and `@done verify` if verify was chosen) came before. Exit 2: `Refused`. `@result failed` otherwise: `Failed { reason }`. No `@result`: `NoResult { status }` whatever the exit code.
  - `TryAgain` goes back to `Choose` keeping drive, mode and options. `RestartNow` returns `Restart` only for `Done { boot_stick: true, .. }`. `Back` returns `Leave` in every stage; a running job keeps running.
  - `outcome_text`, exact copy: boot stick done `"ThinFloe {v} is ready. It starts after a restart."` (`{v}` = latest or `"The new release"` when unknown, so the sentence becomes "The new release is ready. It starts after a restart."); other stick `"The stick is written. You can remove the stick."`; late cancel prefixes `"The cancel came too late: the upgrade was finished. "`; refused `"Nothing was written: {reason}."`; cancelled fresh not written `"Cancelled. The stick will not start until it is written again."`; cancelled fresh written `"Cancelled after the image was written. The stick may start; write it again to be sure."`; cancelled upgrade `"Cancelled. The stick is unchanged."`; no result `"The writer ended without a result ({status}). The log shows why."`; failed `"{reason}"` with the first letter upper case.
  - `summary`: fresh `"Erase SanDisk Cruzer, 16 GB, and install ThinFloe 4.1.0"` (`"the newest ThinFloe"` when the version is unknown); upgrade of the boot stick `"Upgrade this machine's stick to ThinFloe 4.1.0. It becomes active after a restart."`; upgrade of another stick `"Upgrade SanDisk Cruzer, 16 GB, to ThinFloe 4.1.0"`. `confirm_label`: fresh `"Erase SanDisk Cruzer 16 GB"`, upgrade `"Upgrade"`.

- [ ] **Step 1: Write the failing tests.**
  - `write_args_match_the_shared_fixture`: build the five cases of `upgrade-write-args.tsv` through events and compare `write_args()` line by line with the file.
  - `planned_step_ids_are_the_tools`: every planned id appears as `progress.step("<id>", ` in `overlay/usr/bin/thinfloe-upgrade`.
  - `a_pulled_stick_shows_the_tools_reason` (Review Focus 1): `Step write`, two `Progress`, `Result { ok: false, reason: "the stick was removed" }`, `ended(Ok(Some(1)))` → `Finished(Failed)`, `outcome_text` `"The stick was removed"`, the `write` step marked `Failed`.
  - `a_drive_that_disappears_clears_the_choice` (Review Focus 1): choose `sdb`, then `drives_listed` without it → `chosen` `None`, `Confirm` returns `None`.
  - `a_device_with_a_new_id_is_not_the_chosen_drive` (Review Focus 2): choose `/dev/sdb` id `sn-A:…`, poll lists `/dev/sdb` id `sn-B:…` → choice cleared.
  - `no_result_line_is_a_failure` (exit 0 and exit 1 without `@result` → `NoResult`).
  - `result_wins_over_the_exit_code` (`@result ok` with exit 1 → `Done`).
  - `exit_130_after_the_write_may_leave_a_good_stick` and `before_the_write_the_stick_is_wiped`.
  - `a_cancel_during_switch_reports_too_late`.
  - `fresh_cancel_asks_first_and_signals_once`.
  - `refused_goes_back_with_try_again_keeping_the_choices`.
  - `an_internal_drive_needs_its_name_typed`.
  - `copy_home_is_never_passed_after_a_failed_restore`.
  - `an_invalid_source_blocks_confirm` (`"ftp://x"`, `"http:// x"`, `"-o /etc/passwd"`).
  - `drives_errors_are_logged_once_until_they_change`.
  - `polls_only_while_choosing`: no `Drives` request in `Running` or `Finished`.
- [ ] **Step 2:** `cargo test --locked -j4 --lib upgrade::` → FAIL.
- [ ] **Step 3:** Implement as specified.
- [ ] **Step 4:** Same command → PASS.
- [ ] **Step 5:** Commit `menu: the Upgrade screen as a state machine`.

### Task 8: The Upgrade action and the write job in the session's end

**Files:**
- Modify: `src/thinfloe-menu/src/tiles.rs` (`Action::Upgrade`, `parse("upgrade")`, `exit_code` `None`; the test `upgrade_tile_keeps_its_file_name` now asserts `Action::Upgrade` and the reworded name)
- Modify: `overlay/etc/thinfloe/menu/50-upgrade-image.desktop` (`Name=Upgrade`, `Icon=usb`, `X-TF-Action=upgrade`; no `Exec`, no `Terminal`)
- Modify: `src/thinfloe-menu/src/menu.rs`
- Modify: `CHANGES.md`, `CLAUDE.md` (Tiles bullet: `X-TF-Action=upgrade` honoured only in shipped files; the end-of-session paragraph in "Add-ons" names the write job)

**Interfaces:**
- Consumes: `upgrade::{Upgrade, UpgradeEvent, Request}`, `release::update_available`.
- Produces (`menu.rs`):
  - `Screen::Upgrade`; `Job::UpgradeWrite { args: Vec<String> }`, `Job::Drives { all: bool }`, `Job::UpgradeCheck { source: Option<String> }`
  - `Menu.upgrade: Upgrade` (pub), `pub fn set_usb_boot(&mut self, usb: bool)`, `pub fn set_installed(&mut self, v: Option<String>)`
  - `pub fn upgrade_event(&mut self, e: UpgradeEvent, now: Instant) -> Option<Effect>` (maps `Request`: `Check`/`Drives`/`Write` → `Effect::Start`, `Stop` → `Effect::Signal(vec![pgid], Signal::Term)`, `Restart` → `self.begin_ending(Action::Reboot, now)`, `Leave` → `Screen::Grid` and `offer_retry`)
  - `fn begin_ending(&mut self, action: Action, now: Instant) -> Option<Effect>` (the body that `activate` runs today for Log out, Restart and Power off)
  - `pub fn root_jobs(&self) -> Vec<String>` (what `addon_jobs` returns, then `upgrade.job_name()`), `fn root_idle(&self) -> bool`, `fn root_pgids(&self) -> Vec<i32>`
  - `pub fn tile_name(&self, index: usize) -> String`: `"Upgrade to 4.1.0"` for the upgrade tile when `update_available` and the tile is not `Origin::Reworded`, else `tile.name`
  - `visible`: the upgrade tile only when `usb_boot`; `pending_update`: the upgrade tile when `update_available`; `active`: the upgrade tile while `upgrade.writing()`; `can_activate`: always for `Action::Upgrade` (pressing it while a job runs shows the progress)
  - `activate(Upgrade)`: `screen = Screen::Upgrade`, returns `upgrade.open(now)` mapped
  - `tick`: also maps `upgrade.poll(now)` while `screen == Screen::Upgrade`
  - `finished`: routes the five new `JobResult` variants; `UpgradeEnded` counts as "the last thing the logout waited for" exactly like `Addon`; `UpgradeStarted` during `Logout::StoppingAddons` returns `Effect::Signal(vec![pgid], Signal::Term)`
  - `offer_retry` returns early on `Screen::Upgrade` without recording (the question waits for the grid)
  - Every `self.addon_runs.is_empty()` in the logout steps becomes `self.root_idle()`; `choose(LeaveAnyway)` in `WaitAddons` signals `root_pgids()`

- [ ] **Step 1: Write the failing tests** (in `menu.rs` and `tiles.rs`):
  - `the_upgrade_tile_is_a_menu_action` (the shipped file parses to `Action::Upgrade`, empty argv, not terminal) and `user_file_cannot_use_upgrade_action` (banner text as for `settings`).
  - `the_upgrade_tile_is_hidden_on_netboot`.
  - `a_newer_release_names_and_marks_the_tile` (`"Upgrade to 4.1.0"`, `pending_update` true; with `installed` `None` no marker and the name `"Upgrade"`; a reworded tile keeps its name and gets the marker).
  - `pressing_the_tile_checks_and_then_polls` (`Effect::Start(Job::UpgradeCheck { source: None })`, then `tick` gives `Job::Drives { all: false }`).
  - `restart_waits_for_the_write_and_anyway_warns_on_fresh` (Review Focus 3): a fresh write runs (`UpgradeStarted { pgid: 4242 }`); Restart → `Logout::Confirm`; `StopAndLeave` with no programs → `Logout::WaitAddons`; `root_jobs()` is `["Writing SanDisk Cruzer"]`; `LeaveAnyway` → `Effect::Signal(vec![4242], Signal::Term)` and never `Signal::Kill`, also after `tick` past `STOP_GRACE`; `UpgradeEnded` → the logout save starts.
  - `log_out_only_waits_for_the_write`.
  - `back_while_writing_keeps_the_job_and_the_tile_shows_it` (`active` true, pressing the tile shows `Stage::Running`).
  - `restart_now_after_a_boot_stick_upgrade_takes_the_restart_steps` (ends in `Logout::Saving` then exit 10).
  - `no_retry_dialog_over_the_upgrade_screen` (a failed add-on status arrives on `Screen::Upgrade`: `retry` stays `None`, it appears after `Back`).
  - The existing add-on end-of-session tests stay green unchanged.
- [ ] **Step 2:** `cargo test --locked -j4 --lib menu tiles` → FAIL.
- [ ] **Step 3:** Implement. The dialog copy belongs to Task 10; here only `root_jobs()` and the state.
- [ ] **Step 4:** Same command → PASS, and the whole `cargo test --locked -j4` → PASS.
- [ ] **Step 5:** `CHANGES.md`: "Restart, Power off and Log out wait while a stick is written from the Upgrade screen and list it ("Writing SanDisk Cruzer"); Restart anyway and Power off anyway stop the write, and after a cancelled fresh install the stick does not start until it is written again." Update `CLAUDE.md`. Commit `menu: the Upgrade tile opens a screen, and a running write holds the session's end`.

### Task 9: Wiring in the event loop

**Files:**
- Modify: `src/thinfloe-menu/src/app.rs` (`start` for the three jobs; `Wake::Release`; the watcher thread also ticks `release::Watcher`; `refresh_release`)
- Modify: `src/thinfloe-menu/src/main.rs` (`TF_UPGRADE_STATUS` → `release::STATUS_PATH`, `TF_ESP_DIR` → `/run/thinfloe/storage/efi` for `set_usb_boot`, `Paths.upgrade_cmd` = `jobs::default_upgrade_cmd()`, `set_installed` from the header's version)
- Test: `app.rs` tests, `main.rs` tests

**Interfaces:**
- Consumes: Tasks 6 and 8; `Upgrade::release_file` (Task 7) for each change of the status file.
- Produces: `fn usb_boot(esp: &Path) -> bool` in `main.rs` (the directory's `st_dev` differs from its parent's, as Python's `os.path.ismount`; a missing directory is `false`); `Wake::Release`; `UpgradeMsg` events sent from the stdout pump through the proxy before `UpgradeEnded`.

- [ ] **Step 1: Write the failing tests.** `usb_boot_is_a_mount_point_test` (`/proc` → true, a `TestDir` → false, a missing path → false); `a_release_status_change_wakes_the_loop` (unit test of the watcher tick on a `TestDir` file, as for add-ons); in `app.rs` a test that `start(Job::UpgradeWrite)` with `upgrade_cmd = sh -c 'echo "@step write W"; echo "@result ok"' sh` delivers `UpgradeStarted`, `UpgradeMsg(Step)`, `UpgradeMsg(Result)`, `UpgradeEnded` in that order on a channel standing in for the proxy (factor the worker body into `fn run_job(job, paths, feed, send: impl Fn(JobResult))` so it is testable without an event loop).
- [ ] **Step 2:** `cargo test --locked -j4 --lib app main` → FAIL.
- [ ] **Step 3:** Implement. On `Screen::Upgrade` in `Stage::Choose` the UI asks for a repaint after `upgrade::POLL`, so `tick` runs the poll without input; the poll rests while the window is occluded (`shown`).
- [ ] **Step 4:** Same command → PASS; full `cargo test --locked -j4` → PASS.
- [ ] **Step 5:** Commit `menu: start the writer's jobs and follow the release status`.

### Task 10: Drawing the Upgrade screen

**Files:**
- Create: `src/thinfloe-menu/src/upgrade_ui.rs`
- Modify: `src/thinfloe-menu/src/ui.rs` (`UiEvent::Upgrade(UpgradeEvent)`; `show` dispatches `Screen::Upgrade` like Settings, the log stays below; `paint_tile` takes `menu.tile_name(index)`; the end-of-session dialogs name the write job and carry the fresh warning; the shot tests)
- Modify: `src/thinfloe-menu/src/brand.rs` (named sizes for the screen; `upgrade_ui.rs` added to the literal scan's file list)
- Modify: `src/thinfloe-menu/src/app.rs` (`UiEvent::Upgrade(e)` → `menu.upgrade_event(e, now)`)
- Modify: `CHANGES.md`, `CLAUDE.md` (Customizing: a short Upgrade screen bullet next to Tiles; Menu limits: what is not verified on hardware)

**Interfaces:**
- Consumes: `Upgrade` read fields and `outcome_text`, `summary`, `confirm_label`, `drives::{tag, modes, format_size, confirm_text}`, `speed::{fraction, rate_text, eta_text, bytes_text}`, `settings::last_checked`.
- Produces: `pub fn show(ui: &mut Ui, menu: &Menu, area: Rect) -> Option<UpgradeEvent>`.
- Layout (one page in a vertical `ScrollArea`, panel width `GROUP_W` centred as Settings):
  - Header row: title "Upgrade" in `brand::geo(SETTINGS_TITLE_PT)`, Back on the right.
  - Stage Choose, in this order: **Release** (installed version or "version unknown", newest or "version unknown", `last_checked`, an `error` line in `role::WARN`, Check now disabled while `checking`; an Advanced switch shows the source `Input` with the hint "address of the release", invalid input marked `.invalid(true)` with "Only http:// and https:// addresses.", and the "Show all drives" checkbox, as pushback item 1 puts it under Advanced); **Target** (one `Item` per drive, `.selected()` for the chosen one, model and `format_size` as title, `tag` and the refusal reason as description, a refused row drawn under `add_enabled_ui(false)`; "No USB stick found. Plug one in." when the list is empty; the list text says it follows plug and unplug); **Action** (only when the chosen drive offers two modes: two `Item`s "Upgrade (A/B, active after a restart)" and "Erase and install"); **Options** (fresh only: "Copy this machine's home directory" with the line "Left out: the SSH host keys and the WireGuard key. The remote-access password travels with the home." or, disabled, "This boot's home directory was not restored, so it cannot be copied."; "Check the stick after writing"); for an internal drive with Fresh the line "Type <confirm_text> to erase this drive." and an `Input`; **Summary** sentence and one button, `ButtonVariant::Destructive` with `confirm_label` for an erase, the default variant for Upgrade, disabled until `Confirm` would start the write.
  - Stage Running: one row per `StepView` with a painted mark (`StepMark` → a square: border only, `primary` filled, `role::GOOD`, `role::WARN`; `widget_info` names it "waiting", "running", "done", "failed"), the running step with `Progress` (only when `fraction` is `Some`), `bytes_text`, `rate_text` and `eta_text` when known; a Cancel button during `write`, `download` and `verify`; on fresh the inline question "Stop writing? The stick will not start until it is written again." with "Keep writing" and a destructive "Stop"; "Cancelling…" once asked.
  - Stage Finished: `outcome_text` in its tone, then "Restart now" (boot stick done), "Try again" (failed, refused, cancelled, no result) and "Back".
  - Dialogs in `ui.rs`: `Logout::Confirm` title "A stick is being written" when only the write runs; `WaitAddons` title "Waiting for the stick before restarting" (the verb form as today) when the write is the only root job; on a fresh write the line "If you {verb} anyway, the stick in {model} will not start until it is written again." above the buttons.

- [ ] **Step 1: Write the failing tests** (in `ui.rs` tests, with a helper `upgrade_shapes(menu, w, h)` built like `settings_shapes`):
  - `the_choose_stage_draws_release_target_and_summary` (texts contain "Upgrade", "version unknown", "this machine's stick", "too small (needs 3.2 GB)", "Erase SanDisk Cruzer, 16 GB, and install the newest ThinFloe").
  - `the_erase_button_is_destructive_and_names_the_drive` (the button label "Erase SanDisk Cruzer 16 GB" is drawn on the destructive fill colour).
  - `a_refused_row_cannot_be_chosen` (a click at its centre gives no `UpgradeEvent::Choose`).
  - `the_running_stage_shows_bar_rate_and_time_left` (at 42 % of 3.1 GB with 12 MB/s: a `Progress` shape, "12.0 MB/s", "about 3 min left").
  - `no_bar_without_a_total` (Review Focus 4 at the drawing side).
  - `the_dialog_names_the_write_and_warns_on_fresh`.
  - `the_upgrade_screen_draws_nothing_rounded` (as `settings_draws_nothing_rounded`).
  - `ui_code_has_no_style_literals` now scans `upgrade_ui.rs` and stays green.
  - Extend the ignored shot tests: `shot_upgrade_screens` writes `upgrade-choose-1080.ppm` (boot stick, a second ThinFloe stick chosen with Erase and install, a too-small stick, options shown), `upgrade-choose-internal-1080.ppm` (Show all drives, an internal disk chosen, the typed-confirm field), `upgrade-running-1080.ppm` (fresh, write at 42 %), `upgrade-running-unknown-1080.ppm` (no total), `upgrade-cancel-ask-1080.ppm`, `upgrade-done-boot-1080.ppm`, `upgrade-failed-1080.ppm`, `upgrade-choose-21in-4k.ppm` (scale 2.25), `dialog-writing-1080.ppm` (Restart while writing, `WaitAddons`); `shot_desktop_and_settings_at_densities` adds `desktop-1080-upgrade-marker.ppm` (a newer release known).
- [ ] **Step 2:** `cargo test --locked -j4 --lib ui:: brand` → FAIL.
- [ ] **Step 3:** Implement with crate components only (`Button`, `Item`, `Progress`, `Input`, `checkbox`, `Badge` for the tag if it reads better); a component the crate lacks is ported into the crate with `shadcn-egui:port-component`, never painted here. Gaps between blocks are named constants with their Tailwind class in a comment.
- [ ] **Step 4:** Same command → PASS; full `cargo test --locked -j4` → PASS. Render: `TF_SHOT_DIR=/scratch/oetiker/claude-tmp/thinfloe-tests/a2-shots cargo test --locked -j4 --lib shot_ -- --ignored`, convert with `/scratch/oetiker/claude-tmp/thinfloe-tests/tools/topng.py`, look at every picture (no text cut inside its row, the bar inside the panel, the destructive button red, the 4K picture at the same size on paper as 1080p).
- [ ] **Step 5:** `CHANGES.md`, first in the list: "The Upgrade tile opens an Upgrade screen in the menu instead of a terminal: choose the release, the USB stick and whether to upgrade or erase it, confirm once, then follow each step with a bar, the write speed and the time left, and cancel while it downloads or writes. Only USB sticks are listed unless you tick "Show all drives", and erasing another drive needs its name and size typed in. The tile reads "Upgrade to X.Y.Z" when a newer release is known; the check runs five minutes after boot and once a day." `CLAUDE.md` bullets as listed under Files. Commit `menu: the Upgrade screen`.

### Task 11: Acceptance

**Files:** none new; fixes go to the task that owns the code.

- [ ] **Step 1:** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=upgrade-screen image` → artifacts built.
- [ ] **Step 2:** `make VERSION=upgrade-screen test` → every script passes, including the Task 2 lines in `test-session-packages.sh` and the "no GPU library" guard.
- [ ] **Step 3:** `make VERSION=upgrade-screen test-runtime` → PASS (the menu maps under labwc with the new tile; on the runtime rig it is hidden, since no ESP is mounted).
- [ ] **Step 4:** `make VERSION=upgrade-screen qemu-smoke-write` → `PASSED - write smoke` (the tool's write path is unchanged; this confirms Task 1 did not break it). `make VERSION=upgrade-screen qemu-smoke-upgrade` and `make VERSION=upgrade-screen qemu-smoke` → PASSED, no `Found ordering cycle` (the new timer).
- [ ] **Step 5:** Hand the owner the PNGs from Task 10 Step 4 and the list under "Not verified" below. Ask before merging.

### Task 12 (only if open question 3 is answered yes): the tool's boot-stick writes take `esp.lock`

**Files:**
- Modify: `overlay/usr/bin/thinfloe-upgrade` (`ESP_LOCK = "/run/thinfloe/esp.lock"`; a context manager `esp_lock(timeout=30)` with `fcntl.flock` and a deadline; used around the removal of `tl.old/` and the `switch` step on the boot stick only, never around the download)
- Test: `test/unit/test_upgrade_batch.py` (`TestEspLock`)
- Modify: `CLAUDE.md` (the lock order paragraph in "Add-ons" lists `thinfloe-upgrade` as an `esp.lock` user)

- [ ] **Step 1: Write the failing tests.** `test_switch_waits_for_the_esp_lock` (a thread holds the lock file for 1 s; the switch starts after it is released); `test_esp_lock_timeout_fails_before_the_switch` (held 31 s with a patched clock → `WriteFailed("the boot stick is busy …")`, `DEFAULT` unchanged); `test_download_does_not_hold_the_esp_lock`.
- [ ] **Step 2:** `python3 -B -m unittest test.unit.test_upgrade_batch -v` → FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Same command → PASS; `make test-unit` → PASS.
- [ ] **Step 5:** Commit `upgrade: take the ESP lock while the boot stick's release is switched`.

## Not in this plan

- The Part 5 release contract on the tool side: `DEFAULT_URL` moves to `https://github.com/oposs/thinfloe/releases/latest/download/`, and the per-file `<name>.sha256` checks move to the release's one checksum file (`expected_download_sha256` is the one place to change). Until then the menu shows "version unknown" and the tool logs `checksum not checked`.
- `/etc/thinfloe/version` in the image (Part 5); without it no tile marker appears.
- The version of each stick in the Target list (open question 5): the menu reads an optional `version` field; the tool does not print one yet.
- Known tool minors the owner chose to leave (handoff §4): a cancel during the boot-stick save step leaves save-home's `tar` writing `home.tar.gz.new`; a write to a closed stderr after a good stick can turn exit 0 into 1.

## Not verified, to check on hardware

- The bar and the time left on real sticks (`fdatasync` every 32 MiB versus `O_DIRECT` is still unmeasured).
- Clicking through the screen with a real mouse; the headless runtime rig has no input devices.
- The drive list following plug and unplug every 2 s, and the cost of mounting each stick's ESP on every poll.
- Restart now after an upgrade of the boot stick, through to the first boot of `tl.new`.

## Open questions for the owner

1. **Name and format of the version asset.** Part 5 names "the version file the Upgrade tile reads" but not its name. Recommendation: an asset called `version` holding one line `X.Y.Z`, the same format as `/etc/thinfloe/version`; `VERSION_ASSET` in the tool is the one place to change.
2. **Schedule of the check.** The spec says "once a day, randomised within the day". The status file lives in `/run`, and a timer's last run is lost at power-off, so a machine that runs eight hours a day might never check. Recommendation: `OnBootSec=5min`, `OnUnitActiveSec=1d`, `RandomizedDelaySec=1h`, and a retry after an hour on failure.
3. **`esp.lock` for the tool.** The tool's writes to the boot stick's ESP do not take `/run/thinfloe/esp.lock`, which `CLAUDE.md` says every ESP write does; the menu now makes an add-on install, a background save and an upgrade at the same time likely. Recommendation: yes, Task 12, for the removal of `tl.old/` and the switch only, so a save never waits for a 600 MB download.
4. **Netboot.** Part 5 decided "USB machines only; hidden on netboot" for the tile, while the pushback review says the drive rules hold on netboot, as if writing sticks from a netbooted machine were expected. Recommendation: keep the tile hidden on netboot for 4.0.0, as decided; showing it later is one condition in `Menu::visible`.
5. **Version on a stick.** The spec's Target row shows "ThinFloe stick (version, …)", and the `drives` JSON from A1 has no version. Reading it from each stick's squashfs every 2 s is too slow. Recommendation: Part 5 writes `tl/version` on the ESP with each release and `drives` reports it as `version`; this plan already reads and shows the optional field.
