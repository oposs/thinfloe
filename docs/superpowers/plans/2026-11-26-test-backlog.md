# Test backlog (Branch E) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the 16 items of the handoff's defect backlog: make the flaky and vacuous tests assert what they claim, fix the two menu defects (lost click events, lost `[network]` keys), split the slowest Python test module, and correct the build-lock and clippy advice.

**Architecture:** Each item is fixed where it lives: shell test scripts get deterministic synchronisation instead of sleeps; the menu's frame loop collects the events of every egui pass; `network_apply` merges its result onto the store per field; `test_upgrade_batch.py` is split into modules that share one fixture module, and inherited test reruns are dropped. Items already fixed by earlier work are verified and recorded, not re-fixed.

**Tech Stack:** bash (integration tests, `scripts/`), Rust 1.96.0 (`src/thinfloe-menu`, egui 0.37), Python 3.12 unittest, GNU make.

**Spec:** `docs/superpowers/plans/2026-11-26-round-fix-everything.md` (Branch E: fix/test-backlog, items E1-E16).

## Deviations from the round spec

- **E2 is already fixed; verify and drop.** The handoff note reads "mktemp falls back to /tmp (Makefile sets no TMPDIR)". Since the dev-pace work (2026-11-21) the Makefile sets `TEST_TMPDIR := $(or $(TMPDIR),$(CURDIR)/.tmp)` and runs every integration test as `@$(TEST_ENV) test/integration/...` (`Makefile:642-647`, `:696`), so `test-dhcp-timezone.sh`'s `mktemp -d` lands under `.tmp/` like every other test's. A direct call without TMPDIR falls back to /tmp for all ~40 test scripts alike; that is the documented `mktemp` behaviour, not a defect of this one test. Task 1 has the verification step; no code change.
- **E3: four tests, not two.** The two #26 tests (PR #26 = merge 3270689, commits 953caa2/910b281) that pass vacuously under load are "a lease written while a scan runs" (`test-dhcp-timezone.sh:263-277`) and "a removed lease" (`:312-318`). Two more cases use the same sleep-then-assert-nothing pattern and are fixed in the same task: "the watcher never creates the lease directory" (#26, `:289-294`) and "menu_zone_wins_over_a_renewal" (4cf567a, `:405-416`).
- **E9 is already fixed; verify and drop.** The 1/40 failure was seen by 2026-11-21; commit 0c79006 (2026-11-23, "test(osd): make the stale socket in a child that has exited") removed its cause (a sibling test thread's fork() copying the socket fd). 200 runs alone and 4 full-crate runs on 2026-11-26 (plan research) were green. Task 4 repeats the check under CPU starvation.
- **E16 is already fixed; verify and drop.** #58 (ab5bac2) anchored the check at `test-save-home.sh:397` to the command word (`'^sudo (/[^ ]*/)?u?mount( |$)'`). Run with `TMPDIR=/scratch/oetiker/claude-tmp/plan-E/automount` (plan research, 2026-11-26): all checks ok. Task 1 repeats it.
- **E1: no script in the repo wraps a build in flock.** `rg -n flock Makefile scripts .github` finds none; the lock commands live in the private handoff and memory store and in old plan files (historical records, left as they are). The fix is a rule line in `CLAUDE.md` (Build) plus this plan's own command forms; the handoff's §5 cargo/make lines and `_memory/podman-build-flock.md` ("Prevention idea, not built") are the controller's to update (listed in Task 9).
- **E15: the advice is written in one repo file**, `docs/superpowers/plans/2026-11-23-screen-tab-lists.md:22`, and in the handoff (outside the repo, controller's). Only the repo line is edited.
- **E10: no change** (accepted earlier, per the spec).
- **E4: no repo change.** On 2026-11-26 `buildah ps` listed 566 working containers, not ~10; many may belong to builds of other sessions that are running. Task 9 writes the list for the owner; nothing is removed.
- **E13: cause not settled by reading.** std's `random/linux.rs:157` (Rust 1.96.0) is the `/dev/urandom` fallback's `read_exact` after `getrandom(2)` returned ENOSYS/EPERM (seccomp) or EAGAIN; `UnexpectedEof` means that read returned 0 bytes. The same message appeared in other projects inside the Claude Code sandbox (handoff store: "sandbox /dev/urandom reads EOF") and once with a truncated `cp` of a fresh cargo binary. Task 8 tells the two apart with one probe and fixes or documents accordingly.

## Global Constraints

- Branch `fix/test-backlog`, cut from `round/2026-11-26b` in a worktree under `/scratch/oetiker/claude-worktrees/`; rebased onto the round branch per CLAUDE.md Workflow.
- At most 4 cores: `CARGO_BUILD_JOBS=4`, `--test-threads=4`, `BUILD_JOBS=4`, `-j 4`. Never /tmp: scratch is `/scratch/oetiker/claude-tmp/test-backlog/` (create it); make targets set their own `TMPDIR` (`.tmp/`).
- Cargo, from `src/thinfloe-menu`: `flock -o /scratch/oetiker/claude-tmp/thinfloe-cargo.lock systemd-run --user --scope -q -p MemoryMax=6G -- env CARGO_BUILD_JOBS=4 TMPDIR=/scratch/oetiker/claude-tmp/test-backlog cargo test --locked <filter> -- --test-threads=4` (written CARGO below).
- Make: `flock -o /scratch/oetiker/claude-tmp/dbrrg-build.lock make <target> VERSION=testbacklog` (written MAKE below); every Bash call of a long target with `timeout: 600000`, polled until it exits.
- CPU starvation for reproducing load flakes: wrap the command in `systemd-run --user --scope -q -p MemoryMax=6G -p CPUQuota=<n>% --`; this slows the test without taking cores from anyone.
- UI code takes colours and sizes from `brand.rs` (`ui_code_has_no_style_literals`); no new labwc keybinding; no `exit` in a 90thinfloe hook (none touched here).
- No rm outside the scratch dir; no podman `rm`/`buildah rm`; no ssh; no push.
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`; the cause analysis goes in the commit message, the effect in CHANGES.md.

## Review Focus

- A watcher test that waits for a sentinel lease must still fail when the watcher is broken: check each rewritten #26 case against the bug it guards by reverting the fix it guards in a scratch copy of `dhcp-timezone` (Task 1, Step 4 does this for the scan case).
- Two egui passes that each report an event for the same click (a widget that reports from state, not input) would now act twice; the queue must not double a tile start. Task 5 Step 5 pins "one click, one event" through `ui::show` with a forced discard.
- A field changed by both the job and a concurrent writer: the merge must take the job's value for that field (the person's last action is the Apply), and keep the concurrent value for every other field. Task 6's tests cover both.
- The split of `test_upgrade_batch.py` must not lose a test: the set of (defining class, method) pairs before and after must be equal. Task 7 Step 1 records it, Step 6 compares.
- Widened timeouts in tests must only cost time when a test fails: every widened bound is a poll limit or a deadline that a passing run never reaches (Tasks 3 and 4 say which).

---

### Task 1: dhcp-timezone watcher tests wait for what they assert (E3; verify E2, E16)

**Files:**
- Modify: `test/integration/test-dhcp-timezone.sh:42-47` (timedatectl stub), `:263-277`, `:289-294`, `:312-318`, `:405-416`
- Test: the same file (it is the test)

**Interfaces:**
- Consumes: `wait_for FILE TEXT` (`:221-228`), `watch_start DIR`, `watch_stop`, `lines FILE`, `lease FILE ZONE` (`:231-254`).
- Produces: stub variable `STUB_GATE` (a path; while set and the file is absent, the stub blocks after recording its call, at most 10 s); helper `wait_inotify_on DIR` (returns 0 once an `inotifywait` whose last argument is DIR runs, polls 0.1 s, at most 10 s).

- [ ] **Step 1: Gate the stub.** In the heredoc at `:42-47`, after the line that appends to `$WORK/calls` and before `sleep`, add: if `STUB_GATE` is set, poll every 0.05 s up to 200 times until `[[ -e $STUB_GATE ]]`. Keep `STUB_SLEEP` (other cases may use it; after this task none of the --watch cases do).

- [ ] **Step 2: Rewrite "A lease written while a scan runs" (`:263-277`).** Start the watcher with `STUB_GATE=$WORK/gate` (removed first); write lease 2 = Europe/Zurich; `wait_for $WORK/calls 'set-timezone -- Europe/Zurich'` (the stub is now blocked inside the scan, by construction); write lease 2 = Europe/Inside via `.tmp` + `mv`; then `: >$WORK/gate`; then the existing 100 x 0.1 s poll for the last call `set-timezone -- Europe/Inside`. Failure text stays `watch: lease written during a scan lost: ...`. Drop the `sleep 0.5` before the first lease (the watcher arms before it scans; a lease before the arm is read by the first scan).

- [ ] **Step 3: Rewrite the three negative cases with a sentinel.**
  - "never creates the lease directory" (`:289-294`): replace `sleep 1` with `wait_inotify_on "$WORK/nocreate" || fail "watch: no watch on the parent of a missing lease directory"`, then the existing `[[ ! -e ... ]]` check. `wait_inotify_on` uses `pgrep -f -- "-- $1\$"` (the watcher runs `inotifywait ... -- "$2"`, `overlay/usr/libexec/thinfloe/dhcp-timezone:214`).
  - "A removed lease" (`:312-318`): after `rm -f "$WORK/w3/2"`, write the sentinel `lease "$WORK/w3/3" Europe/Inside` and `wait_for "$WORK/calls" "set-timezone -- Europe/Inside"`. Assert `lines calls == before_calls + 1` and `lines watch.log == before_log + 1`, and the last log line is `thinfloe: time zone Europe/Inside from DHCP`. Events are read in order, so the removal's scan has run by the time the sentinel's has. Remove lease 3 again before the `pkill` case that follows.
  - "menu_zone_wins_over_a_renewal" (`:405-416`): replace `sleep 1.5` with the sentinel `lease "$WORK/w5/3" Europe/Inside` and `wait_for "$WORK/watch.log" "DHCP time zone Europe/Inside not used"`; keep the two assertions (no calls file; exactly one `Europe/Zurich not used` line: change the grep to that exact text).

- [ ] **Step 4: Prove the scan case is no longer vacuous.** Copy `overlay/usr/libexec/thinfloe/dhcp-timezone` to `/scratch/oetiker/claude-tmp/test-backlog/dtz-broken` and edit the copy so `watch()` (`:245-270`) stops the watch around `scan` (call `watch_stop` before the `scan` at `:269` and `watch_start ... "$LEASE_DIR"` after it: the path-unit bug of #26). Run the test with `SCRIPT` pointed at the copy (temporarily edit line 19 in a scratch copy of the test, not in the repo).
Expected: `FAIL: watch: lease written during a scan lost`. Then the real script: `TMPDIR=/scratch/oetiker/claude-tmp/test-backlog test/integration/test-dhcp-timezone.sh` -> `dhcp-timezone: all checks passed`.

- [ ] **Step 5: Run it starved, five times.** `for i in 1 2 3 4 5; do TMPDIR=/scratch/oetiker/claude-tmp/test-backlog systemd-run --user --scope -q -p CPUQuota=10% -- test/integration/test-dhcp-timezone.sh || break; done`
Expected: five `all checks passed` lines.

- [ ] **Step 6: Verify E2 and E16 (no edit).** `grep -n 'test-dhcp-timezone.sh' Makefile` shows `@$(TEST_ENV) test/integration/test-dhcp-timezone.sh`. `mkdir -p /scratch/oetiker/claude-tmp/test-backlog/automount && TMPDIR=/scratch/oetiker/claude-tmp/test-backlog/automount test/integration/test-save-home.sh` exits 0 with `ok   - writes to the existing ESP mount without mounting anything`. Record both results for the PR body.

- [ ] **Step 7: Commit**

```bash
git add test/integration/test-dhcp-timezone.sh
git commit -m "test(time): the DHCP watcher cases wait for a sentinel, not a sleep"
```
Body: which four cases, why a sleep made them pass with a broken watcher under load, the Step 4 evidence.

### Task 2: run-qemu-smoke names the line it waited for (E5)

**Files:**
- Modify: `scripts/run-qemu-smoke.sh:42-57`
- Test: `test/integration/test-boot-smoke-check.sh` (append cases; already run by `make test-integration` at `Makefile:692`)

**Interfaces:**
- Consumes: `scripts/check-boot-smoke.sh --reached LOG` (matches `Reached target.*[Mm]ulti-[Uu]ser` or `Startup finished in`).
- Produces: the timeout lines of `run-qemu-smoke.sh`:
  - target never reached: `run-qemu-smoke: no multi-user target after ${TIMEOUT}s, stopping QEMU` (unchanged)
  - target reached, `SMOKE_WAIT_FOR` not seen: `run-qemu-smoke: multi-user target reached after ${reached_at}s, but no line matching '${SMOKE_WAIT_FOR}' after ${TIMEOUT}s, stopping QEMU`

- [ ] **Step 1: Write the failing cases** in `test-boot-smoke-check.sh`, before its final verdict: a fake QEMU script `$WORK/fake-qemu` that takes the argument after `-serial` (`file:<path>`), writes `Reached target multi-user.target - Multi-User System.` to that path and sleeps 30. Case `wait_for_missing`: `SMOKE_WAIT_FOR='^NEVER' scripts/run-qemu-smoke.sh $WORK/q.log 3 0 -- $WORK/fake-qemu`; its output contains `but no line matching '^NEVER' after 3s` and does not contain `no multi-user target`. Case `never_reached`: a fake that writes nothing; output contains `no multi-user target after 3s`. Case `wait_for_seen`: the fake also writes `SMOKE-X-DONE`; with `SMOKE_WAIT_FOR='^SMOKE-X-DONE'` the script returns within 3 s and prints no `stopping QEMU` line. Each case exit status 0 (the script's contract).

- [ ] **Step 2: Run** `TMPDIR=/scratch/oetiker/claude-tmp/test-backlog test/integration/test-boot-smoke-check.sh`. Expected: `FAIL - wait_for_missing ...` (the old text), the other two ok.

- [ ] **Step 3: Implement** in the loop's timeout branch (`:53-56`): print the second line above when `reached_at` is set, else the old one. Update the header comment (`:15-18`) to say which line names what.

- [ ] **Step 4: Run** the same command. Expected: all ok, exit 0.

- [ ] **Step 5: Commit**

```bash
git add scripts/run-qemu-smoke.sh test/integration/test-boot-smoke-check.sh
git commit -m "fix(smoke): a smoke that times out waiting for its done line says so"
```

### Task 3: render_icon tests do not race the icon deadline (E8)

**Files:**
- Modify: `src/thinfloe-menu/tests/render_icon.rs:45-48`, `:103-129`, `:131-146`, `:230-258`

**Interfaces:**
- Consumes: `icons::render_all(renderer: &Path, jobs: &[IconJob], deadline: Duration, budget: Duration) -> Vec<Result<egui::ColorImage, String>>`; `ICON_DEADLINE` (2 s), `ICONS_BUDGET` (5 s), `ICON_MEMORY` (256 MiB).
- Produces: test-local `const TEST_DEADLINE: Duration = Duration::from_secs(30);` and `const TEST_BUDGET: Duration = Duration::from_secs(120);`; `fn render(paths: Vec<PathBuf>)` passes them.

Reproduced in plan research: `the_longest_allowed_chain_renders_in_the_renderer` fails with `Some("took longer than 2s")` under `CPUQuota=15%`. These tests are about stack, memory, the filter guard and PDEATHSIG; the wall-clock deadline is `icons.rs`' own unit tests' business.

- [ ] **Step 1: Reproduce.** Build: CARGO with `--no-run`. Then `systemd-run --user --scope -q -p MemoryMax=6G -p CPUQuota=15% -- env TMPDIR=/scratch/oetiker/claude-tmp/test-backlog cargo test --locked --test render_icon -- --test-threads=4` (from `src/thinfloe-menu`). Expected: FAIL in `the_longest_allowed_chain_renders_in_the_renderer` (or another deadline-bound case).

- [ ] **Step 2: Widen.** `render()` uses `TEST_DEADLINE`/`TEST_BUDGET`. In `a_pattern_bomb_draws_the_letter_and_its_memory_is_bounded` drop `assert!(start.elapsed() < ICON_DEADLINE ...)` and the `start` variable: the per-result check `!e.starts_with("took longer")` already says the bomb ended by itself, and now holds with a 30 s deadline. In `a_renderer_dies_with_its_menu`, the two 1 s poll limits (`:235`, `:255`) become 10 s; the 200 ms sleep at `:238` stays (the slow SVG keeps resvg busy for seconds; starved, longer). Comment each change with the reason (CPU starvation in `make test-unit`).

- [ ] **Step 3: Run** Step 1's command three times, then CARGO `--test render_icon` once unstarved. Expected: PASS every time.

- [ ] **Step 4: Commit**

```bash
git add src/thinfloe-menu/tests/render_icon.rs
git commit -m "test(icons): the renderer tests no longer race the 2 s icon deadline under load"
```

### Task 4: Deskflow large-clipboard test runs on a fixed clock (E14; verify E9)

**Files:**
- Modify: `src/thinfloe-menu/src/deskflow/conn.rs:410-479` (test `a_clipboard_larger_than_the_tls_default_reaches_the_client` only)

**Interfaces:**
- Consumes: `Conn::new(id: usize, stream: TcpStream, cfg: Arc<ServerConfig>, now: Instant) -> io::Result<Conn>`, `Conn::on_readable(&mut self, now: Instant) -> Vec<Event>`, `Conn::set_verdict(&mut self, v: Verdict, now: Instant)`, `Conn::clipboard(&mut self, text: &[u8])`, `Conn::flush`, `Conn::closing`; deadlines `HANDSHAKE_DEADLINE` (10 s, `conn.rs:25`), `session::HELLO_DEADLINE` (10 s).
- Produces: nothing outside the test.

Not reproduced in plan research (30 runs alone, 3 runs at `CPUQuota=3%`, 4 full-crate runs). Wall-clock bounds in the test and in `Conn` are the only time-dependent parts: the client's 5 s read timeout (`:417`), the server loop's 10 s end (`:454`), and Conn's own 10 s handshake and hello deadlines, driven by the `Instant::now()` the loop passes (`:456`). Under `make test-unit` the client's `rcgen` key and the server's `ident::load_or_create` run in debug builds next to dozens of other tests.

- [ ] **Step 1: Try to reproduce under real load.** Run `MAKE test-unit` in one shell and, while it runs, the test in a loop: `for i in $(seq 30); do <lib test binary> deskflow::conn::tests::a_clipboard_larger_than_the_tls_default_reaches_the_client --exact -q || echo FAIL $i; done` (the binary path from CARGO `--no-run`; both under the 4-core quota). Keep any failure output in `/scratch/oetiker/claude-tmp/test-backlog/e14-*.log`. If a failure shows `the server's end: Some(...)` with a reason other than a deadline, stop and fix that cause instead of Step 2 (report it).

- [ ] **Step 2: Fix the clock and widen the outer bounds.** Take `let t0 = Instant::now();` once before `Conn::new` and pass `t0` to `Conn::new`, `on_readable`, `set_verdict` (no Conn deadline can fire); client read timeout 30 s; the loop's end 60 s (both only reached when the test fails). Comment: why the clock is frozen.

- [ ] **Step 3: Run** CARGO `deskflow::conn::tests`. Expected: PASS. Then Step 1's loop once more (10 runs). Expected: no FAIL.

- [ ] **Step 4: Verify E9 (no edit unless it fails).** `systemd-run --user --scope -q -p MemoryMax=6G -p CPUQuota=10% -- <lib test binary> osd::socket::tests::send_without_a_server_is_no_server --exact -q` 200 times in a loop, plus once inside Step 1's loop. Expected: 0 failures; record "fixed by 0c79006" for the PR body. A failure: keep its log, find the race (start from `testdir::stale_socket`, `src/thinfloe-menu/src/testdir.rs:109-145`) and fix it in this task with its own commit.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/deskflow/conn.rs
git commit -m "test(deskflow): the large clipboard test runs on a fixed clock"
```

### Task 5: the menu keeps the events of every egui pass (E7)

**Files:**
- Modify: `src/thinfloe-menu/src/app.rs:1279-1281` (`let mut event = None;`), `:1345-1347` (the `run_ui` call), `:1445-1469` (handling)
- Test: `src/thinfloe-menu/src/app.rs` `mod tests` (`:1916`)

**Interfaces:**
- Consumes: `ui::show(ui: &mut Ui, menu: &Menu, icons: &[Option<Icon>], chrome: &Chrome, now: Instant) -> Option<UiEvent>` (`ui.rs:577`); `ui::UiEvent` (Debug, Clone, PartialEq, Eq; `ui.rs:238-251`).
- Produces: `fn run_passes(ctx: &egui::Context, input: egui::RawInput, show: impl FnMut(&mut egui::Ui) -> Option<ui::UiEvent>) -> (egui::FullOutput, Vec<ui::UiEvent>)` in `app.rs`, next to `needs_backdrop`.

Cause: egui 0.37 runs the closure again when a pass asks for a discard (`Context::run_dyn`, max 2 passes) and gives the later pass no input events (`new_input.take()`). `event = ui::show(...)` keeps only the last pass's answer, so an event of the discarded pass is lost (`ui.rs:1867` discards when a page or its footer changes height; `network_ui.rs:119-127` already returns its event in a discarded pass for that reason).

- [ ] **Step 1: Write the failing tests** in `app.rs` `mod tests`:
  - `an_event_of_a_discarded_pass_is_kept`: `egui::Context::default()`; a closure that on its first call does `ui.ctx().request_discard("test")` and returns `Some(UiEvent::Packages)`, and `None` after. `assert_eq!(events, vec![UiEvent::Packages])`; the closure ran twice.
  - `events_of_both_passes_are_kept_in_order`: first call `Some(UiEvent::Tile(1))` plus discard, second `Some(UiEvent::Tile(2))` -> `vec![Tile(1), Tile(2)]`.
  - `a_pass_without_a_discard_runs_once`: closure returns `Some(UiEvent::Retry(true))` -> one call, `vec![Retry(true)]`.
  Each test clears `out.textures_delta` as `the_zoom_factor_is_the_pixels_per_point` does.

- [ ] **Step 2: Run** CARGO `app::tests::an_event_of_a_discarded_pass_is_kept`. Expected: compile error, `run_passes` not found.

- [ ] **Step 3: Implement `run_passes`** (`ctx.run_ui` with a closure that pushes each `Some` into a Vec) and use it at `:1345`: `let (mut out, events) = run_passes(&self.ctx, input, |ui| ui::show(ui, &self.cfg.menu, &live.icons, &live.chrome, now));`. Replace `let mut event = None;` (`:1281`). At `:1445-1469`: move the `match` into a method `fn handle(&mut self, event: Option<ui::UiEvent>, now: Instant) -> Option<Effect>` on the same impl (same arms; `None` -> `self.cfg.menu.tick(now)`); with no events call it once with `None`, else once per event in order, and after each `self.apply(effect, event_loop)` when the effect is `Some` or the event was; one `request_redraw` at the end when anything was applied.

- [ ] **Step 4: Run** CARGO `app::tests`. Expected: PASS.

- [ ] **Step 5: One click, one event.** Add `a_tile_click_in_a_discarded_pass_starts_one_tile` in `ui.rs`' tests next to the existing click tests that drive `ui::show` with a `RawInput` click (follow the pattern at `ui.rs:2505`): wrap `ui::show` in `app::run_passes` (make it `pub(crate)`) with a `request_discard` in the first pass before `show`; a click on tile 0 gives exactly `vec![UiEvent::Tile(0)]`. Run CARGO `ui::tests` and the whole crate. Expected: PASS, no other test changed.

- [ ] **Step 6: Commit**

```bash
git add src/thinfloe-menu/src/app.rs src/thinfloe-menu/src/ui.rs
git commit -m "fix(menu): a click in a pass egui discards is no longer lost"
```

### Task 6: network_apply merges its result onto the store (E6)

**Files:**
- Modify: `src/thinfloe-menu/src/config.rs:580-592` (replace `with_public_ip_of`), test `:1042-1060`; `src/thinfloe-menu/src/jobs.rs:1010-1021` (`net_take_over`), `:1023-1077` (`network_apply`, doc comment included)
- Test: `jobs.rs` `mod tests` (near `a_network_apply_keeps_the_public_ip_settings_set_while_it_ran`, `:2796`)

**Interfaces:**
- Consumes: `NetworkCfg` (fields `wifi_device`, `networks`, `wifi_dns`, `wired`, `kept_wifi`, `kept_wired`, `public_ip`, `public_ip_url`; `config.rs:518-536`); `net_snapshot(paths) -> (ThinfloeConfig, Inputs)`; `netfiles::plan(cfg: &NetworkCfg, c: &Change, netboot: bool, h: &Homes, inputs: &Inputs) -> Result<NetworkCfg, String>`.
- Produces: `impl NetworkCfg { pub fn merged_onto(&self, before: &NetworkCfg, current: &NetworkCfg) -> NetworkCfg }`: for each field, `self`'s value where `self` differs from `before`, else `current`'s. `with_public_ip_of` is removed (its two callers move to `merged_onto`).

Merge rule: the job's keys are the fields its plan changed against the snapshot it planned on; every other field comes from the store as it is when the job writes. A field both changed takes the job's value. Add a compile-time guard so a new field cannot be forgotten: destructure `NetworkCfg { wifi_device, networks, ... }` without `..` in `merged_onto`.

- [ ] **Step 1: Write the failing tests.**
  - `config.rs`: `merged_onto_takes_the_planned_change_and_keeps_the_rest`: before = default; planned = before with `wifi_dns = ["1.1.1.1"]`; current = before with `public_ip = true`, `public_ip_url = "https://ip.example.net"`, `kept_wired = Some("x")`; result has all four. `merged_onto_prefers_the_plan_on_a_field_both_changed`: both change `wifi_dns` -> the planned value. Replace `with_public_ip_of_copies_both_keys` by these.
  - `jobs.rs`: `a_network_apply_keeps_a_network_key_changed_while_it_ran`: `net_paths(&d, "/bin/true", <ok tool>)`; change = wifi `one_net("home")`; in `on_spawn`, `lock(&p.config).update(Table::Network, |c| c.network.wired.dns = vec!["9.9.9.9".into()])`; after `Ok(())`: `cfg.network.nets() == one_net("home")` and `cfg.network.wired.dns == ["9.9.9.9"]`, in memory and in `config.toml`. The existing public-IP test stays and must pass unchanged.

- [ ] **Step 2: Run** CARGO `merged_onto` and CARGO `jobs::tests::a_network_apply_keeps`. Expected: compile error, then (after a stub) the jobs test FAILs with `wired.dns == []`.

- [ ] **Step 3: Implement.** `merged_onto` in `config.rs`. In `network_apply`: keep `cfg.network` from `net_snapshot` as `before`; the staged candidate (`:1043`) and the commit (`:1059`) both use `net.merged_onto(&before, &s.cfg.network)`. In `net_take_over` (`:1018`) the same. Rewrite the comments at `:1014-1015`, `:1025-1028`, `:1056-1058` to the merge rule.

- [ ] **Step 4: Run** CARGO `jobs::tests` and CARGO `config::tests`. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src/config.rs src/thinfloe-menu/src/jobs.rs
git commit -m "fix(menu): a network apply keeps every [network] key it did not change"
```

### Task 7: split test_upgrade_batch.py, drop inherited reruns (E12)

**Files:**
- Create: `test/unit/upgrade_fixtures.py` (no `test_` prefix, so `scripts/run-unit-tests.py` does not run it), `test/unit/test_upgrade_write.py`, `test/unit/test_upgrade_fresh.py`, `test/unit/test_upgrade_stick.py`
- Modify: `test/unit/test_upgrade_batch.py` (keeps the rest), `docs/notes/automount.md:179` (the file it names for hold/release/remount)

**Interfaces:**
- Produces in `upgrade_fixtures.py`: `REPO`, `SCRIPT`, `load_upgrade_image()`, `ui` (loaded once per process), `REAL_STREAM_IMAGE`, `MIB`, `LSBLK`, `fake_cmd`, `IMG`, `mk_drive`, `make_thinfloe_esp`, `from_esp_lock`, `lock_held`, `_run_batch`, `slow`, the `_LOCK_DIR` setup (`test_upgrade_batch.py:46-48`), and fixture bases without test methods: `BatchRefusalsFixture`, `FreshInstallFixture`, `BatchUpgradeFixture`, `WipeFixture` (setUp and helpers of `TestBatchRefusals`, `TestFreshInstall`, `TestBatchUpgrade`, `TestWipeClosesTheWriteFirst`) and `_ReleaseCase`. Test modules do `from upgrade_fixtures import *` (explicit `__all__`); each test module's `ui` is that module object, so `mock.patch.object(ui, ...)` keeps working.

Measured 2026-11-26 on the dev host: 552 tests, 162 s in one process. Twelve subclasses (`TestCancelNeverLost`, `TestEspLock`, `TestClosedStdout`, `TestStagedUpgrade`, `TestLock`, `TestExclusiveOpen`, `TestSourceCheck`, `TestBatchSignals`, `TestWipeClosesTheWriteFirst`, `TestCancelBeforeTheWipe`, `TestInteractiveFreshChecksTheOpenDrive`, `TestIdWithoutSerial`) inherit and rerun every test of their base, most marked `# noqa: inherits fixtures`.

Rerun rule: a subclass keeps its base's tests only if it overrides a class attribute or a helper that those tests call: `TestStagedUpgrade` (`STAGED`/`CFG`, the staged stick is the point) and `TestExclusiveOpen` (overrides `run_fresh`). The implementer checks the other ten against the rule; each that only adds setUp for its own tests derives from the fixture base instead. A class kept as rerun gets a one-line comment saying why.

- [ ] **Step 1: Record the baseline.** `cd test/unit && python3 -I -m unittest --durations 30 test_upgrade_batch 2>&1 | tail -40 > /scratch/oetiker/claude-tmp/test-backlog/e12-before.txt` and the (defining class, method) set: a short `python3 -I` snippet that loads the module with `unittest.defaultTestLoader.loadTestsFromName`, walks the suite, and for each test prints `next(c.__name__ for c in type(t).__mro__ if t._testMethodName in vars(c))` and the method name, sorted unique, to `e12-before.set`; also the plain count.

- [ ] **Step 2: Move the shared code** to `upgrade_fixtures.py`, with the four fixture bases split off their test classes (`class TestFreshInstall(FreshInstallFixture)` keeps the tests). Run `python3 -I -m unittest test_upgrade_batch` from `test/unit`. Expected: OK, same count as Step 1.

- [ ] **Step 3: Apply the rerun rule** (subclasses that do not qualify derive from the fixture base). Run again. Expected: OK, fewer tests.

- [ ] **Step 4: Split by area** (classes keep their names): `test_upgrade_write.py` refusals, wipe, lock, source check, signals, id without serial; `test_upgrade_fresh.py` fresh install and its subclasses, child isolation, fresh install automount; `test_upgrade_stick.py` batch upgrade, esp lock, staged upgrade, non-boot rotation; the rest stays in `test_upgrade_batch.py`. Move classes between modules until each module runs in at most 45 s on the dev host (`--durations` per module).

- [ ] **Step 5: Run** `python3 scripts/run-unit-tests.py -j 4 test/unit`. Expected: `unit: <n+3> modules, 0 failed`; note each module's seconds.

- [ ] **Step 6: Compare.** The Step 1 snippet over the four modules: the (defining class, method) set equals `e12-before.set` (`diff` empty). The count drop equals the reruns removed (state the arithmetic in the commit message). Update `docs/notes/automount.md:179` to the module that now holds the hold/release/remount tests.

- [ ] **Step 7: Commit**

```bash
git add test/unit/upgrade_fixtures.py test/unit/test_upgrade_*.py docs/notes/automount.md
git commit -m "test(upgrade): split the batch tests into four modules and run each test once"
```

### Task 8: TF_RUNTIME_MENU, find why a host-built menu panics (E13)

**Files:**
- Modify: `test/runtime/test-labwc-runtime.sh` (comment at `:1654-1660`; a preflight before the first case that mounts `TF_RUNTIME_MENU`, `:1686`), `docs/notes/menu.md` (Known limits, `:478`) only if the cause is outside the repo

**Interfaces:**
- Produces: `runtime_menu_preflight` in the script: when `TF_RUNTIME_MENU` is set, fail at once with one line naming the cause unless it is a regular executable file and `thinfloe-menu --check` runs in the runtime image with it mounted (exit 0 or 1 and no `panicked` in its output).

- [ ] **Step 1: Build a host menu and probe.** CARGO `build --release --locked` (rust-toolchain.toml pins 1.96.0, as the image). Copy with `cp --sparse=never` to the scratch dir and `cmp` it with the original. With the runtime image from `MAKE test-runtime` (its tag is `thinfloe-runtime-test:testbacklog`), run both: (a) `podman run --rm -v <copy>:/usr/bin/thinfloe-menu:ro <image> sh -c 'mkdir -p /tmp/s /tmp/u /tmp/st; ls -l /dev/urandom; head -c 16 /dev/urandom | wc -c; TF_MENU_SHIPPED_DIR=/tmp/s TF_MENU_USER_DIR=/tmp/u TF_STATE_DIR=/tmp/st thinfloe-menu --check'` and (b) the same without `-v` (the image's menu). Run both once from the agent's shell and ask the owner to run (a) once from a plain terminal (outside Claude Code's sandbox).

- [ ] **Step 2: Decide by the result.**
  - (a) and (b) both panic in the agent shell, neither in the owner's terminal: the sandbox's `/dev/urandom` and getrandom filter, not the binary. Document in the script comment and in menu.md Known limits: "a run of test-runtime inside Claude Code's sandbox panics in std's random source; run it from a terminal".
  - only (a) panics, with the plain copy but not the `--sparse=never` copy (or the `cmp` differs): a truncated copy; the comment says to pass cargo's own output path (`$CARGO_TARGET_DIR/release/thinfloe-menu`), never a copy.
  - only (a) panics, copy intact: compare `ldd` of the host binary inside the image and its `strace -f -e trace=getrandom,openat` (if strace is in the image; else the host binary under `podman run --security-opt seccomp=unconfined` once) and fix what differs; report the finding before a larger change.
  In every case add `runtime_menu_preflight`, so a broken `TF_RUNTIME_MENU` fails with one clear line instead of nine cases failing.

- [ ] **Step 3: Run** `MAKE test-runtime` without `TF_RUNTIME_MENU` (the evidence run), then `TF_RUNTIME_MENU=/nonexistent MAKE test-runtime`. Expected: the first green; the second stops at the preflight with its line.

- [ ] **Step 4: Commit**

```bash
git add test/runtime/test-labwc-runtime.sh docs/notes/menu.md
git commit -m "test(runtime): check TF_RUNTIME_MENU once before the cases use it"
```
Body: the probe results and the cause.

### Task 9: build-lock and clippy advice; the buildah list (E1, E15, E4)

**Files:**
- Modify: `CLAUDE.md:94` (Build, the `artifacts` bullet: append one sentence), `docs/superpowers/plans/2026-11-23-screen-tab-lists.md:22`
- Create (scratch, not in git): `/scratch/oetiker/claude-tmp/test-backlog/buildah-ps.txt`

- [ ] **Step 1: E1.** Add to the `CLAUDE.md` Build bullet at `:94`: "On the development host a build that queues on a lock takes it with `flock -o <lock> make …`: without `-o` podman's fuse-overlayfs helpers inherit the lock and hold it after a stopped build." Check: `flock -o` exists in the host's util-linux (`flock --help | grep -- -o`).

- [ ] **Step 2: E15.** In the 2026-11-23 plan line 22 replace "Host clippy: `-A clippy::chunks-exact-to-as-chunks` (pre-existing lints, container uses Rust 1.96.0)." with "Clippy: `make test-menu` (the old `-A clippy::chunks-exact-to-as-chunks` advice broke host clippy)." Check: `rg -n chunks-exact-to-as-chunks` finds only that line and the round spec.

- [ ] **Step 3: E4.** `buildah ps --format '{{.ContainerID}} {{.ImageID}} {{.ContainerName}}' > /scratch/oetiker/claude-tmp/test-backlog/buildah-ps.txt` and the count. Nothing is removed. The PR body lists the file, the count, and the owner's command for later (`buildah rm <id>` per line, after checking no build of another session runs).

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-11-23-screen-tab-lists.md
git commit -m "docs: take the build lock with flock -o; drop the host clippy advice"
```
Controller follow-ups (outside the repo, listed in the PR body): handoff §5 cargo/make lines to `flock -o`; `_memory/podman-build-flock.md` "Prevention idea, not built" -> built; handoff backlog items closed.

### Task 10: make test-unit exit 2, loop with logs (E11)

**Files:** none unless a cause is found.

`make` exits 2 whenever a recipe fails; so "exit 2" says only that one of `test-unit`'s three recipes (Python runner, menu `cargo test`, share `cargo test`, `Makefile:651-655`) failed, each under `MEMCAP` (`MemoryMax=2G`, `Makefile:640`). Candidates: the flakes of Tasks 3 and 4, or an OOM kill of a test binary or the linker under the 2 GiB cap.

- [ ] **Step 1: Loop.** After Tasks 1-7 are committed: `for i in $(seq 8); do MAKE test-unit > /scratch/oetiker/claude-tmp/test-backlog/e11-$i.log 2>&1; echo "run $i rc=$?"; done` (one Bash call per run, `timeout: 600000`). Before and after the loop, record `grep oom_kill /proc/vmstat`.

- [ ] **Step 2: Read every non-zero run.** `grep -nE 'FAILED|panicked|Killed|signal: 9|unit: .* failed|Error [0-9]' e11-<i>.log`; an `oom_kill` increase in the run's window points at MEMCAP. A test failure: fix it in its own task-sized commit (TDD as above). An OOM: raise `MEMCAP` for the cargo recipes only after measuring the peak (`/usr/bin/time -v` around one `cargo test --no-run`), and say the number in the commit.

- [ ] **Step 3: Record.** No failure in 8 runs: one line in the PR body, "E11 not reproduced in 8 runs of make test-unit after the flake fixes (Tasks 3, 4)", with the run times. No commit in that case; otherwise commit the fix:

```bash
git add <files of the fix>
git commit -m "fix(test): <the cause> no longer fails make test-unit"
```

### Task 11: CHANGES.md and docs/notes

**Files:**
- Modify: `CHANGES.md` (under `## [Unreleased]` -> `### Fixed`), `docs/notes/menu.md`

Only E7 changes what a person at the machine sees. E6 is latent today (the menu runs one network job at a time, `menu.rs:723`, and the public IP keys were already kept), so it gets no entry; the rest are test and developer changes, for commit messages only.

- [ ] **Step 1: CHANGES.md**, one entry under `### Fixed`:

```markdown
- Now and then a click in the menu did nothing and had to be repeated; it happened when the page changed its layout at the same moment. Every click now counts.
```

- [ ] **Step 2: docs/notes/menu.md.** In "The menu's log" or the event-loop text that the Notes index names (search `run_ui`, `request_discard` in `docs/notes/`; if none, add two sentences under "Known limits" reworded as a fact): the frame loop collects the events of every egui pass (egui reruns a discarded pass without input). If Task 8 found an outside cause, its Known-limits line is already there.

- [ ] **Step 3: Commit**

```bash
git add CHANGES.md docs/notes/menu.md
git commit -m "docs: changelog and notes for the test backlog round"
```

### Task 12: Gates

The branch changes menu code (`app.rs`, `ui.rs`, `jobs.rs`, `config.rs`), test scripts, `scripts/run-qemu-smoke.sh` (every QEMU smoke calls it) and the runtime test. CLAUDE.md Workflow: the full set before the branch joins the round.

- [ ] **Step 1:** `MAKE test-menu` (fmt, clippy `-D warnings`, cargo test). Expected: exit 0.
- [ ] **Step 2:** `MAKE test-unit`. Expected: exit 0, `unit: <n> modules, 0 failed` (n = old count + 3), menu and share cargo tests ok.
- [ ] **Step 3:** `MAKE test` (builds the rootfs; `test-integration` includes `test-boot-smoke-check.sh`, `test-dhcp-timezone.sh`, `test-save-home.sh`). Expected: exit 0, no `FAIL`.
- [ ] **Step 4:** `MAKE test-runtime` (menu event loop changed). Expected: exit 0.
- [ ] **Step 5: Smoke tests the change touches:** `MAKE qemu-smoke` (plain path of `run-qemu-smoke.sh`) and `MAKE qemu-smoke-timezone` (the `SMOKE_WAIT_FOR` path; also the DHCP watcher Task 1 tests). Expected: both pass. No other smoke target is affected beyond the one message line.
- [ ] **Step 6:** Re-derive the results from the logs, not from memory: note each gate's commit (`git rev-parse --short HEAD`), exit code and test counts in the PR body, with the E2/E9/E16 verifications (Tasks 1, 4), the E11 loop (Task 10), the E4 list (Task 9), and the E12 before/after timings and counts (Task 7).

---

## Self-review

- **Spec coverage:** E1 Task 9; E2 Task 1 Step 6 (verified, deviation); E3 Task 1; E4 Task 9 Step 3; E5 Task 2; E6 Task 6; E7 Task 5; E8 Task 3; E9 Task 4 Step 4 (verified, deviation); E10 no change (deviation); E11 Task 10; E12 Task 7; E13 Task 8; E14 Task 4; E15 Task 9 Step 2; E16 Task 1 Step 6 (verified, deviation). CHANGES and notes: Task 11; gates: Task 12.
- **Names:** `run_passes` (Task 5) is used in Task 5 Step 5 only; `merged_onto` (Task 6) replaces `with_public_ip_of` in both callers; `STUB_GATE`, `wait_inotify_on` are Task 1 only; fixture class names are fixed in Task 7's Interfaces.
