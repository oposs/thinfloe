# DHCP time zone race Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** No DHCP lease change is lost: the time zone from option 101 is applied even when a lease is written while an earlier one is being read.

**Architecture:** Replace `thinfloe-dhcp-timezone.path` + oneshot service with one `Type=simple` service that runs `dhcp-timezone --watch`: `inotifywait -m` on the lease directory is armed first, then a scan, then a scan after every (coalesced) event. Without `--watch` the script scans once, as today.

**Tech Stack:** bash, inotify-tools (`inotifywait`), systemd units, the image Dockerfile.

**Spec:** `docs/superpowers/specs/2026-11-17-dhcp-timezone-race-design.md`

## Global Constraints

- The validation in `check()` and the log lines `thinfloe: time zone <zone> from DHCP`, `thinfloe: ignored DHCP time zone '<value>': <why>`, `thinfloe: could not set DHCP time zone '<value>'` stay byte-identical.
- No `Before=` on anything in the units (CLAUDE.md, multi-user/sysinit cycle).
- The script never creates `/run/systemd/netif/leases` (networkd owns it).
- `THINFLOE_LEASE_DIR` and `THINFLOE_ZONEINFO_DIR` keep working; the watch mode honours `THINFLOE_LEASE_DIR` (its parent is watched for a late directory).
- Tests run under `systemd-run --user --scope -q -p MemoryMax=2G --`; the watch tests kill their watcher in a trap and use a timeout of at most 10 s per wait.
- Image builds: `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make <target> VERSION=tzrace`, at most 4 cores, `timeout: 600000` on every long Bash call, never end a turn while a background shell runs.

## Review Focus

- A lease written while a scan runs (stub `timedatectl` sleeps 1 s): its zone is set afterwards. Task 1 test "lease during a scan".
- The lease directory does not exist when the watcher starts and appears later with a lease: the zone is set. Task 1 test "late directory".
- A renewal that rewrites the same zone: no second `from DHCP` line. Task 1 test "same zone logs once".
- `inotifywait` dies (killed): the script exits non-zero so `Restart=on-failure` restarts it. Task 1 test "watcher exit".
- A lease removed (link down): no error line, nothing set. Task 1 test "removed lease".

---

### Task 1: `dhcp-timezone --watch`

**Files:**
- Modify: `overlay/usr/libexec/thinfloe/dhcp-timezone`
- Test: `test/integration/test-dhcp-timezone.sh`

**Interfaces:**
- Produces: `dhcp-timezone` (no args) = one scan, exit 0, as today. `dhcp-timezone --watch` = runs until killed; exits 1 when `inotifywait` ends.

- [ ] **Step 1: Write the failing tests** in `test-dhcp-timezone.sh`, after the existing cases, each starting `"$SCRIPT" --watch >"$WORK/watch.log" 2>&1 &` with `THINFLOE_LEASE_DIR` set to a fresh dir, and a helper `wait_for <file> <text>` that polls every 0.1 s up to 10 s:
  - "lease during a scan": stub `timedatectl` that sleeps 1 s (env `STUB_SLEEP=1`); write lease A (`TIMEZONE=Europe/Zurich`), wait until the stub is called once, write lease B (`TIMEZONE=Europe/Inside`) while it sleeps; expect `calls` to end with `set-timezone -- Europe/Inside`.
  - "late directory": start the watcher with `THINFLOE_LEASE_DIR=$WORK/late/leases` where `$WORK/late` exists but `leases` does not; `mkdir` it, write a lease; expect `Europe/Zurich` in `calls`.
  - "same zone logs once": write the same lease twice (1 s apart); expect exactly one `time zone Europe/Zurich from DHCP` line in `watch.log`.
  - "removed lease": remove the lease file; expect no new line in `watch.log` and no new call after 1 s.
  - "watcher exit": `pkill -P <watcher pid> inotifywait`; expect the watcher to exit with non-zero status within 10 s.
  - "existing lease at start": a lease present before start is applied (the boot-time case).
- [ ] **Step 2: Run** `test/integration/test-dhcp-timezone.sh`. Expected: the old cases pass, the new ones FAIL (`--watch` unknown, so the script scans once and exits).
- [ ] **Step 3: Implement.** Move the lease loop into `scan()`. `scan` keeps `LAST` (the zone it set last in this process) and prints `from DHCP` only when the zone differs from `LAST`; it still calls `timedatectl` each time (the clock may have been changed by hand). `--watch`: if `$LEASE_DIR` is missing, `inotifywait -q -e create,moved_to --include "^${dir##*/}\$"`-style wait on its parent in a loop until it exists (re-check after arming so a create between the test and the arm is not missed). Then start `inotifywait -m -q -e close_write,moved_to,create,delete "$LEASE_DIR"` as a coprocess with stderr captured; read until `Watches established.` (without `-q` for that line, or use `--format` and the established line — choose one and test it), then `scan`, then `while read -r _ <&"${W[0]}"; do while read -r -t 0.5 _ <&"${W[0]}"; do :; done; scan; done; exit 1`.
- [ ] **Step 4: Run** the test file. Expected: all PASS, three times in a row.
- [ ] **Step 5: Commit** `fix: the DHCP time zone watcher keeps watching while it reads a lease`.

### Task 2: units, image and docs

**Files:**
- Delete: `overlay/etc/systemd/system/thinfloe-dhcp-timezone.path`
- Modify: `overlay/etc/systemd/system/thinfloe-dhcp-timezone.service`, `containers/ubuntu/Dockerfile:427` and its package list, `test/integration/test-session-packages.sh:72-74`, `scripts/run-qemu-timezone-smoke.sh:4`, `docs/notes/time-zone.md`, `CHANGES.md`

- [ ] **Step 1: Tests first.** In `test-session-packages.sh`: replace the path-unit line with `absent`-style checks that no `thinfloe-dhcp-timezone.path` exists in the image (use the file's existing helper for absence, or add one), keep the service-enabled line (reword "watcher"), add `present "inotifywait" 'usr/bin/inotifywait$'`.
- [ ] **Step 2: Units.** Service: `Type=simple`, `ExecStart=/usr/libexec/thinfloe/dhcp-timezone --watch`, `Restart=on-failure`, `RestartSec=2`, keep `StartLimitIntervalSec=0`, `After=systemd-networkd.service`, `StandardOutput=journal+console`, `WantedBy=multi-user.target`; comments explain why there is no path unit (it stops watching while the service runs). Remove the path file. Dockerfile: `systemctl enable thinfloe-dhcp-timezone.service` only; add `inotify-tools` to the package list next to related tools.
- [ ] **Step 3: Docs.** `docs/notes/time-zone.md`: the path-unit sentences become the watcher; keep every guard listed. `scripts/run-qemu-timezone-smoke.sh` header comment. `CHANGES.md` under `### Fixed` in `[Unreleased]`: "The time zone from the DHCP server could stay at UTC after the start when the server's answer came in while an earlier one was still being read; it now always takes effect."
- [ ] **Step 4: Gates** (one background chain under the build lock, `VERSION=tzrace`): `make test-unit`, `make image test`, `make qemu-smoke-timezone` three times, `make qemu-smoke-timezone-netboot`, `make qemu-smoke`. Expected: all green; the size test passes without a budget change.
- [ ] **Step 5: Commit** `fix: one watcher service replaces the DHCP time zone path unit`.
