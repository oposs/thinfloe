# Stick ID Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** With two ThinFloe sticks attached, every boot mounts the ESP of the stick the firmware booted from, so the home, the add-ons and the saves stay on one stick.

**Architecture:** Each stick gets a random ID in `/config/stick-id` on its ESP, and the initramfs adds `thinfloe.stick=<id>` to the local `APPEND` lines of that stick's `syslinux.cfg`. The initramfs lists every partition named `EFI-SYSTEM` from sysfs, reads each one's ID, and mounts the one whose ID the kernel command line names. The `/dev/disk/by-partlabel/EFI-SYSTEM` link is no longer used for the choice.

**Tech Stack:** bash in the dracut initramfs (only the tools `module-setup.sh` installs), Python 3 for `thinfloe-upgrade`, QEMU + OVMF for the smoke test.

**Spec:** the Design section below. Approved by the owner on 2026-10-26 ("option 1"). Bug evidence: `/scratch/oetiker/claude-tmp/thinfloe-tests/bug1/` (runs C2 and C3 lost the add-on when the initramfs mounted the other stick).

## Design

- **Root cause.** `TF_EFI_DEV=/dev/disk/by-partlabel/EFI-SYSTEM` (`thinfloe-lib.sh:26`) is one udev link. With two partitions named `EFI-SYSTEM`, udev points it at either one, and the choice changes between boots. The firmware loads the kernel from one stick; the squashfs, the home, the add-ons and every save go to the stick the link names.
- **ID.** `/config/stick-id` on the ESP: one line, a lowercase UUID (`[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`), read from `/proc/sys/kernel/random/uuid`. A file that does not hold a valid ID counts as no ID.
- **Command line.** `thinfloe.stick=<id>` at the end of every `APPEND` line in both `syslinux.cfg` copies whose `ramroot=` is not `http://`, `https://` or `ftp://`. An invalid value counts as absent.
- **Choice** (`tf_choose_esp`), over the `EFI-SYSTEM` partitions in device-name order:
  1. The command line names an ID and exactly one partition has it → that one.
  2. More than one has it (a `dd` copy of a used stick) → the first; log `thinfloe: <n> sticks carry stick id <id>, using /dev/<dev>`.
  3. Otherwise (no ID on the command line, or no partition has it): one partition → that one. Else the partitions without an ID: one → that one; several → the first, with the log line `thinfloe: <n> sticks have no stick id, using /dev/<dev>`. Else the first, with `thinfloe: no stick carries stick id <id>, using /dev/<dev>`.
  Rule 3 prefers a stick without an ID because a stick with an ID always boots with it on the command line; a boot without one came from a stick that never booted.
- **Waiting.** As before, up to `TF_DEVICE_TIMEOUT` (60 s) for the first `EFI-SYSTEM` partition. Then, while the rule-1 match (ID on the command line) or a stick without an ID (no ID on the command line) is missing, poll up to `TF_ESP_MATCH_WAIT` (10 s) more: a second USB stick can enumerate later than the first.
- **Once per boot.** `tf_find_esp` writes the chosen device to `/run/thinfloe-esp-device`; `finalize-upgrade.sh` (pre-mount) and `mount-squashfs.sh` (mount) both call it and get the same device.
- **Stamping** (`tf_stamp_stick`), after `mount-squashfs.sh` mounted the ESP: the ID is the stored one, else the command line's, else a new one (log `thinfloe: this stick is now <id>`). Write `config/stick-id` when it differs. Rewrite a `syslinux.cfg` copy only when one of its local `APPEND` lines lacks ` thinfloe.stick=<id>`: drop any old `thinfloe.stick=` word, append the new one, write `.tmp`, `mv`, `sync`. Failures are logged and never stop the boot.
- **Upgrade.** `thinfloe-upgrade` copies `LABEL current` into `LABEL new` (`_new_label_block`), so the ID rides along unchanged.
- **Fresh install.** `do_fresh_install` stamps the new stick right after `dd` and `partprobe`, before the home question: a new ID (`uuid.uuid4()`, lowercase) in `config/stick-id` and on its local `APPEND` lines, by the same rule as `tf_stamp_stick`. Overwriting a used stick therefore also gives it a new ID. The stamp does not depend on the home question, which stays optional and defaults to no (sticks made for other people); `test_fresh_install_stamps_the_new_stick` answers no and still requires the stamp. A failed stamp prints a warning and the install still counts as done: the stick then gets its ID at its first boot.
- **Limits** (go to `CLAUDE.md`): two attached sticks that both never booted and were not written by `thinfloe-upgrade` (a `dd` of the download, `make image`) are still a guess (the first by device name); `dd` copies of a used stick share one ID, one machine-id and one host name; a 3.x fallback boot (`LABEL previous`) runs the 3.x initramfs and still uses the partlabel link.

## Global Constraints

- Initramfs code may call only the tools in `inst_multiple` of `90thinfloe/module-setup.sh` plus dracut's base set; no `cut`, `basename`, `head`, `install`, `wc`, `date`. `test/integration/test-initramfs-commands.sh` must stay green.
- English for code, comments and names. Changelog per `~/.claude/CLAUDE.md` rules: what the reader saw, at most three sentences.
- At most 4 cores: `BUILD_JOBS=4`; cargo is not touched by this plan. One `make` at a time under `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock`; temporary files under `/scratch/oetiker/claude-tmp/thinfloe-tests`.
- A `make` in a worktree needs its own `VERSION=` (see `CLAUDE.md`, container stamp).
- Ask the owner before push, publish, `rm` and `kill`.

## Review Focus

1. **One stick, as in the field today.** A stick that booted 4.x before has no `stick-id`: its first boot with this image must stamp it and boot normally, and every later boot must cost no extra wait. Task 2 tests "one candidate, no ID on the command line, no wait"; Task 5's smoke boots each stick again.
2. **The second stick shows up late** (USB enumeration). Task 2 tests that a match appearing within `TF_ESP_MATCH_WAIT` is found and that a missing match costs at most that budget.
3. **The ESP cannot be written** (read-only vfat, full). Task 3 tests that a failed write leaves `syslinux.cfg` intact and returns 0.
4. **Hand-edited `syslinux.cfg`** (a user added kernel options, or the `network` label points at a server). Task 3 tests that other words on the `APPEND` line survive and that `ramroot=http…` lines stay unchanged.
5. **The staged upgrade.** `LABEL new` must carry the same ID, or the first boot after an upgrade falls into rule 3. Task 4 tests `_new_label_block`; `make qemu-smoke-upgrade` must stay green.

---

### Task 1: `tf_choose_esp`, the choice rule

**Files:**
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh` (new function after `tf_dump_block_devices`)
- Create: `test/integration/test-esp-choice.sh` (offline, no root; header, stubs and the initramfs PATH sandbox copied from `test/integration/test-initramfs-home.sh:1-50` and `:612-640`)
- Modify: `Makefile` (`test:` target, after `test-initramfs-home.sh`)

**Interfaces:**
- Produces: `tf_valid_stick_id <string>` → exit 0 when the string is a lowercase UUID as in the Design.
- Produces: `tf_choose_esp <want-id> <dev>=<id> ...` → prints the chosen `<dev>` (e.g. `vda1`) on stdout, exit 0; exit 1 with no arguments after `<want-id>`. `<want-id>` may be empty; `<id>` is empty for a stick without one. Arguments arrive in device-name order. Logs the three Design lines with `tf_log`.

- [ ] **Step 1: Write the failing tests** in `test-esp-choice.sh`, each run under `PATH="$BIN"`, with `A=11111111-1111-4111-8111-111111111111`, `B=22222222-2222-4222-8222-222222222222`:
  - `tf_choose_esp "$A" vda1="$B" vdb1="$A"` prints `vdb1` (rule 1).
  - `tf_choose_esp "$A" vda1="$A" vdb1="$A"` prints `vda1` and logs `2 sticks carry stick id $A` (rule 2; capture `tf_log` into a file).
  - `tf_choose_esp "" vda1="$B" vdb1=` prints `vdb1` (rule 3, the stick without an ID).
  - `tf_choose_esp "" vda1= vdb1=` prints `vda1` and logs `2 sticks have no stick id`.
  - `tf_choose_esp "$A" vda1="$B"` prints `vda1` (one partition).
  - `tf_choose_esp "$A" vda1="$B" vdb1="$B"` prints `vda1` and logs `no stick carries stick id $A`.
  - `tf_choose_esp ""` exits 1.
  - `tf_valid_stick_id` accepts `$A`; refuses `""`, `${A^^}`, `$A-x`, `../../x`, `$A x`.

- [ ] **Step 2: Run** `test/integration/test-esp-choice.sh`. Expected: FAIL, `tf_choose_esp: command not found`.

- [ ] **Step 3: Implement** both functions with `case` patterns and shell builtins only (no `grep`, no `sed`).

- [ ] **Step 4: Run** the test again. Expected: every line `ok`, exit 0. Then `test/integration/test-initramfs-home.sh`: still exit 0.

- [ ] **Step 5: Commit** `initramfs: choose the ESP by stick id (rule only)`.

### Task 2: `tf_find_esp`, used by both hooks

**Files:**
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh` (replace `tf_wait_for_efi` and `TF_EFI_DEV`; keep `TF_EFI_ABSENT_MARKER` and the comment on why it polls)
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/finalize-upgrade.sh:43-58`, `mount-squashfs.sh:73-86`
- Test: `test/integration/test-esp-choice.sh`

**Interfaces:**
- Consumes: `tf_choose_esp`, `tf_valid_stick_id` (Task 1).
- Produces: `tf_esp_candidates` → prints the kernel names (`vda1`) of all block devices whose `$TF_SYS_BLOCK/<name>/uevent` (default `/sys/class/block`) has `PARTNAME=EFI-SYSTEM`, sorted, one per line. Read with `read`, no `grep`.
- Produces: `tf_read_stick_id <dev>` → mounts `/dev/<dev>` read-only (`mount -t vfat -o ro`) at `/run/thinfloe-probe`, prints `config/stick-id`'s first line if `tf_valid_stick_id` accepts it, else nothing; always unmounts. Tests replace it with a shell function reading `$WORK/ids/<dev>`.
- Produces: `tf_find_esp` → prints `/dev/<dev>`, exit 0; exit 1 when no candidate appears within `TF_DEVICE_TIMEOUT` (then it writes `TF_EFI_ABSENT_MARKER` as `tf_wait_for_efi` did). Reads the wanted ID with `getarg thinfloe.stick=`. Caches its answer in `TF_ESP_CACHE` (default `/run/thinfloe-esp-device`) and returns the cached one on the next call. Logs `thinfloe: boot stick /dev/<dev> (stick id <id or none>)`.

- [ ] **Step 1: Write the failing tests** with a fake `$TF_SYS_BLOCK` (dirs `vda`, `vda1`, `vdb1`, `loop0` with `uevent` files; only `vda1` and `vdb1` have `PARTNAME=EFI-SYSTEM`), `getarg` stubbed to echo `$WANT`, `TF_ESP_CACHE` and `TF_EFI_ABSENT_MARKER` under `$WORK`, `TF_DEVICE_TIMEOUT=2`, `TF_ESP_MATCH_WAIT=2`, `udevadm` and `sleep` as stubs on a PATH built from `$BIN`:
  - `tf_esp_candidates` prints `vda1` and `vdb1`, not `vda` or `loop0`.
  - `WANT=$B`, ids `vda1=$A vdb1=$B` → prints `/dev/vdb1`; the cache file holds `/dev/vdb1`.
  - A second call after changing the ids to `vda1=$B` still prints `/dev/vdb1` (cache).
  - **Late stick:** `WANT=$B`, only `vda1` present at first; the `sleep` stub creates `vdb1` (id `$B`) on its first call → prints `/dev/vdb1`.
  - **No wait for one stick:** `WANT=` and only `vda1` without ID → prints `/dev/vda1` with zero `sleep` calls (count them in the stub).
  - **Missing match costs at most the budget:** `WANT=$A`, only `vda1=$B` → prints `/dev/vda1` after at most `TF_ESP_MATCH_WAIT` `sleep` calls.
  - **No candidate:** empty `TF_SYS_BLOCK` → exit 1, marker written; a second call returns 1 at once.

- [ ] **Step 2: Run** the test. Expected: FAIL on the new cases.

- [ ] **Step 3: Implement** the three functions. Keep the polling loop of `tf_wait_for_efi` (`udevadm settle --timeout=5`, `sleep 1`, a log line every 5 s, `Waiting for EFI-SYSTEM to appear`). Then switch both hooks: `finalize-upgrade.sh` sets `EFI_DEV=$(tf_find_esp)` in place of `tf_wait_for_efi`; `mount-squashfs.sh` sets `efi_dev=$(tf_find_esp)` and keeps `tf_dump_block_devices` and the `die` text. Remove `TF_EFI_DEV`; update the `by-partlabel` comment in `thinfloe-save-home:14` only if it names the constant.

- [ ] **Step 4: Run** `test/integration/test-esp-choice.sh`, `test/integration/test-initramfs-home.sh`, `test/integration/test-cmdline.sh`. Expected: all exit 0.

- [ ] **Step 5: Commit** `initramfs: mount the ESP of the stick that booted`.

### Task 3: `tf_stamp_stick`, writing the ID

**Files:**
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh` (after `tf_set_boot_default`)
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/mount-squashfs.sh` (call after `mount -t vfat … "$efi_mount"`)
- Test: `test/integration/test-esp-choice.sh`

**Interfaces:**
- Consumes: `tf_valid_stick_id` (Task 1).
- Produces: `tf_stamp_stick <efi-mount> <want-id>` → exit 0 always. Reads a new ID from `TF_UUID_SOURCE` (default `/proc/sys/kernel/random/uuid`). Edits with `sed` into `<cfg>.tmp`, then `mv`, then `sync`, like `tf_set_boot_default`. `mount-squashfs.sh` calls it as `tf_stamp_stick "$efi_mount" "$(getarg thinfloe.stick=)"`.

- [ ] **Step 1: Write the failing tests**, each on a fresh fake ESP holding `configs/syslinux.cfg` as `syslinux.cfg` and `efi/boot/syslinux.cfg`, under `PATH="$BIN"`, `TF_UUID_SOURCE` a file holding `$A`:
  - **Fresh stick, no ID on the command line:** `config/stick-id` is `$A`; in both copies every `APPEND` line with `ramroot=tl` or `ramroot=tl.old` or `ramroot=tl.new` ends in ` thinfloe.stick=$A` exactly once; the `LABEL network` line is byte-identical to the input; all other lines are byte-identical (compare `diff` of the files with the `APPEND` lines removed).
  - **Idempotent:** a second call leaves both copies with the same inode and content (`stat -c %i`), and `config/stick-id` unchanged.
  - **Stored ID wins:** `config/stick-id` = `$B`, want `$A` → every local `APPEND` ends in `thinfloe.stick=$B`, none carries `$A`.
  - **Command line ID reused:** no `config/stick-id`, want `$B` → the file holds `$B`.
  - **Invalid stored ID** (`config/stick-id` = `x`) → replaced by `$A`.
  - **User options survive:** an `APPEND` line with an added `quiet foo=bar` keeps both words.
  - **Read-only ESP:** the ESP dir `chmod 555` (skip when running as root) → exit 0, both copies byte-identical to the input, no `.tmp` left behind.

- [ ] **Step 2: Run** the test. Expected: FAIL on the new cases.

- [ ] **Step 3: Implement** `tf_stamp_stick`, and add its call to `mount-squashfs.sh` in the USB branch only.

- [ ] **Step 4: Run** `test/integration/test-esp-choice.sh` and `test/integration/test-initramfs-home.sh`. Expected: exit 0.

- [ ] **Step 5: Commit** `initramfs: give each stick an id in syslinux.cfg`.

### Task 4: `thinfloe-upgrade` stamps a new stick and keeps the ID on upgrade

**Files:**
- Modify: `overlay/usr/bin/thinfloe-upgrade` (new functions next to `set_boot_default`; call in `do_fresh_install` after `partprobe`)
- Test: `test/unit/test_upgrade_image.py`; `test/integration/test-esp-choice.sh` (the cross-check)

**Interfaces:**
- Consumes: `tf_stamp_stick` (Task 3) for the cross-check; `find_efi_partition(device)` and the mount/cleanup helpers `copy_home_to_drive` already uses (`_register_cleanup`).
- Produces: `stamp_syslinux(text: str, stick_id: str) -> str` (pure: the same edit as `tf_stamp_stick` on one file's text); `stamp_new_stick(device: str) -> bool` (find the ESP, mount it in a temp dir, write `config/stick-id` and both `SYSLINUX_CFGS` with `.tmp` + `os.replace` + fsync, unmount; False with a `log("WARNING: …")` on any failure).

- [ ] **Step 1: Write the tests** in `test_upgrade_image.py`:
  - `test_label_new_keeps_the_stick_id`: a `syslinux.cfg` without `LABEL new` whose `LABEL current` `APPEND` ends in `thinfloe.stick=11111111-1111-4111-8111-111111111111`; after `set_boot_default(esp, "new")` the `LABEL new` `APPEND` is `ramroot=tl.new/ramroot.sqsh … thinfloe.stick=11111111-1111-4111-8111-111111111111`.
  - `test_stamp_syslinux_marks_local_entries`: on `configs/syslinux.cfg`, every local `APPEND` ends in the ID once, `LABEL network` and every non-`APPEND` line unchanged; a second `stamp_syslinux` with another ID replaces it, never adds a second.
  - `test_fresh_install_stamps_the_new_stick`: with `download_and_write_image`, `run_cmd`, `confirm` (returns False) and the mount patched the way the existing `copy_home_to_drive` tests patch them, `do_fresh_install` calls `stamp_new_stick(drive.device)` after `partprobe`; when it returns False the install still returns `None`.
  - In `test-esp-choice.sh`, `the shell and Python stamps agree`: `tf_stamp_stick` on one copy of `configs/syslinux.cfg` and `stamp_syslinux` (loaded with `python3 -I` from the script path, as `test_upgrade_image.py` loads it) on another, same ID → `cmp` equal.

- [ ] **Step 2: Run** `python3 -m unittest test.unit.test_upgrade_image -v` (repo root, as `make test-unit`) and `test/integration/test-esp-choice.sh`. Expected: `test_label_new_keeps_the_stick_id` passes at once (`_new_label_block` copies the line; if not, fix it); the others FAIL, `stamp_syslinux` not defined.

- [ ] **Step 3: Implement** `stamp_syslinux`, `stamp_new_stick`, and the call in `do_fresh_install`.

- [ ] **Step 4: Run** both again. Expected: all pass.

- [ ] **Step 5: Commit** `upgrade: give a freshly written stick its own id`.

### Task 5: two-stick smoke test, docs, changelog

**Files:**
- Create: `scripts/run-qemu-two-sticks-smoke.sh`, `scripts/smoke-stick-report.sh`
- Modify: `Makefile` (`.PHONY`, `help`, new target `qemu-smoke-two-sticks` next to `qemu-smoke-addons`)
- Modify: `CLAUDE.md` (new Standing Constraint, count "Seventeen" → "Eighteen", and the preventive/bug sentence: this one caused a real bug, seen in QEMU), `CHANGES.md` (`### Fixed` under `[Unreleased]`; create the heading if absent)

**Interfaces:**
- Consumes: `scripts/run-qemu-smoke.sh` (`SMOKE_WAIT_FOR`), `scripts/check-boot-smoke.sh`, the OVMF boot and `syslinux.cfg` rewrite of `scripts/run-qemu-upgrade-smoke.sh`.

- [ ] **Step 1: Write the smoke test.** Two copies of `$(USB_IMAGE)`: stick **A** (`vda`) and stick **B** (`vdb`). Stick A gets `config/stick-id` = `aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa` and that ID on its `APPEND` lines (via `mtype`/`awk`/`mcopy` as the upgrade smoke does): an old stick. Stick B stays fresh: the new stick. On both, add the report args (`console=ttyS0,115200 systemd.unit=multi-user.target … systemd.run="/bin/sh /run/thinfloe/storage/efi/smoke-stick-report.sh"`) to the `LABEL current` `APPEND`, and copy the report script to both ESPs. QEMU boots through OVMF from **B**: `-drive if=none,id=a,…` / `-device virtio-blk-pci,drive=a,bootindex=1` and the same for `b` with `bootindex=0`. The report script prints `SMOKE-STICK: <findmnt source of /run/thinfloe/storage/efi> <its config/stick-id>` and `SMOKE-STICK-DONE`. Boot three times (one QEMU run each, `SMOKE_WAIT_FOR='^SMOKE-STICK-DONE'`). Pass requires:
  - each boot passes `check-boot-smoke.sh`;
  - each boot reports `/dev/vdb1`, and boots 2 and 3 report the same ID as boot 1, which is not stick A's;
  - after the runs, B's two `syslinux.cfg` copies carry `thinfloe.stick=<that ID>`, and stick A's ESP (`config/stick-id`, both `syslinux.cfg`) is byte-identical to its state before boot 1.
  Without the fix, boot 1 picks either stick, so the test fails about half the time on boot 1 alone and nearly always over three boots.

- [ ] **Step 2: Build and run.** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make BUILD_JOBS=4 image` then `make qemu-smoke-two-sticks`. Expected: `PASSED`. Run it twice more; all three runs pass.

- [ ] **Step 3: Run the gates.** `make test-unit`; `make test` (with `THINLINC_DEB` and `OXULNK_DEB` set as in `/scratch/oetiker/claude-tmp/dbrrg-build/polish-gates.sh`); `make qemu-smoke`, `qemu-smoke-addons`, `qemu-smoke-upgrade`, `qemu-smoke-netboot`. Expected: all green. `test-initramfs-commands.sh` (inside `make test`) proves the hooks call no tool missing from the initrd.

- [ ] **Step 4: Write the docs.** `CLAUDE.md` Standing Constraint "The ESP is chosen by stick ID, never by the partlabel link": the root cause, the rule, `/config/stick-id`, `thinfloe.stick=`, the three limits from the Design, the guards (`test-esp-choice.sh`, `qemu-smoke-two-sticks`). Update the Boot Flow and `qemu-test` notes that `qemu-test` attaches a second stick. `CHANGES.md`, `### Fixed`:
  `- With two ThinFloe sticks plugged in, a boot could take the home directory and the add-ons from the other stick, so an add-on installed before a restart was gone after it, and the home could be saved to the wrong stick. Each stick now gets an ID (\`thinfloe.stick=\` in its \`syslinux.cfg\`) when \`thinfloe-upgrade\` writes it or at its first boot, and the boot uses the stick it started from.`

- [ ] **Step 5: Commit** `smoke: two sticks attached; docs for the stick id`.
