# ThinFloe image size reduction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The three files an upgrade or a netboot downloads (`vmlinuz`, `initrd.img`, `ramroot.sqsh`) shrink from 611 MB to about 480 to 490 MB through the nine cuts the owner approved, each held by a guard, while AMD Ryzen mini PCs gain the APU graphics firmware `amdgpu` needs (`A1`, about 8 MB added).

**Architecture:** dracut flags in `containers/ubuntu/Dockerfile` shape the initrd (`O2`, `O5`). A new Dockerfile `RUN`, the last of the file and after the cleanup `RUN`, executes `containers/ubuntu/slim/slim-rootfs.sh`, which trims the rootfs in a fixed order with build-time guards (`O4`, `S1`, `O1`, `O3`, `S2`, `S3`). `scripts/export-rootfs.sh` keeps `/scripts` out of the squashfs (`S4`). Artifact guards run in `make test`, a budget test holds the result.

**Tech Stack:** podman, GNU make, dracut 110, kmod (`depmod`, `modinfo`), squashfs-tools, zstd, bash integration tests, Python 3 `unittest`, QEMU/KVM smoke tests.

**Spec:** `docs/superpowers/specs/2026-11-12-image-size-design.md`. Cut names (`S1` to `S4`, `O1` to `O5`) refer to it.

## Global Constraints

- Machine rules: at most 4 cores (`BUILD_JOBS=4`); every build runs as `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make … VERSION=3.0.0-size`; `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests/size-<task>/` (create it; never `/tmp`); containers with `podman`, never docker; no `sudo`.
- `artifacts/` is a symlink to the shared `~/scratch/dbrrg-artifacts`; every `make rootfs` overwrites it. The baseline copy of Task 1 lives in `/scratch/oetiker/claude-tmp/thinfloe-tests/size-baseline/` and is never overwritten after Task 1.
- Scope is fixed (*owner*): only `S1` to `S4` and `O1` to `O5`. No other cut. Camera firmware, VA-API drivers and python3 stay.
- `i915` and its firmware stay in the initrd; `plymouth.use-simpledrm` stays in `configs/syslinux.cfg`; dracut keeps `--no-hostonly`.
- The `slim-rootfs.sh` `RUN` is the last `RUN` of the Dockerfile. Nothing after it may run `dpkg`, `apt-get` or dracut: a `dpkg` run can fire the `update-initramfs` trigger, which reruns dracut with default flags on the slimmed tree.
- The rule "no `find`/`rm` sweep over `/usr/lib/firmware`" stays, with exactly three exceptions in `slim-rootfs.sh`: the iwlwifi prune (`O1`), the AMD graphics prune (`A1`) and `intel-ucode` (`O3`).
- `radeon` and `nouveau` are not changed (*owner*, a question about them is open). `amdgpu` stays in `--omit-drivers`, out of the initrd.
- MB = 10^6 bytes; sizes from `stat -c %s`.
- Every task that changes the image ends its commit message with a line `Sizes: vmlinuz <a> MB, initrd.img <b> MB, ramroot.sqsh <c> MB, sum <d> MB (delta <e> MB)`, delta against the previous task's line (Task 1 against the baseline).
- Integration tests follow the style of `test/integration/test-firmware.sh`: `ok   - …` and `FAIL - …` lines, `PASSED`/`FAILED` at the end, exit 1 on failure, a missing artifact fails with "run 'make rootfs' first". Scratch files under `mktemp -d`, with `chmod -R u+rwX` before `rm -rf` in the trap when `unsquashfs` extracted anything.
- Prose (docs, CHANGES, comments, commit messages): no em dashes, "for example" never "e.g."; CHANGES entries follow the repo-infra `writing-style` skill.
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. A card whose firmware series has an unresolved ceiling (`bz`, `gl` `c`-series) must keep every file of that series, and a family `modinfo` does not name at all must keep its highest file (Task 6, `test_unresolved_ceiling_keeps_all`, `test_unnamed_family_keeps_highest`).
2. A `modinfo` format change that yields almost no names must stop the build rather than prune everything or nothing (Task 6, `test_too_few_names_refused`, and the exit status 2 check in `slim-rootfs.sh`).
3. A module kept in the squashfs that needs a module `O4` would remove (`snd-fm801` needs `tea575x`) must keep it (Task 5, `test-image-slim.sh` "snd-fm801 and tea575x present").
4. A kept initrd module that needs a library `O2` omits must fail `make test`, not the boot of one customer's machine (Task 8, the `modinfo -F depends` closure check).
5. Moving the slim step before dracut must fail a test, because the initrd would silently grow (Task 4, "no uncompressed .ko in the initrd"; Task 7, "GenuineIntel.bin in the initrd").

---

### Task 1: Size report, budget test and baseline

**Files:**
- Create: `test/integration/test-image-size.sh`
- Modify: `Makefile` (`test:` target, add the line after `test-squashfs-format.sh`)

**Interfaces:**
- Produces: `test/integration/test-image-size.sh [rootfs-dir [baseline-dir]]`, default `artifacts/rootfs`. Budget variables at the top: `MAX_SQSH`, `MAX_INITRD`, `MAX_SUM` in bytes. Later tasks only run it; Task 10 tightens the budgets.

- [ ] **Step 1: Write `test-image-size.sh`.** It prints `vmlinuz`, `initrd.img`, `ramroot.sqsh` and their sum in MB with one decimal; then the uncompressed bytes (from `unsquashfs -lls`) of `usr/lib/modules/`, `usr/lib/firmware/`, `usr/lib/firmware/intel/iwlwifi/`, `usr/lib/firmware/intel-ucode/`, `usr/share/i18n/`, `usr/lib/udev/hwdb.d/`, `scripts/`; and (from `lsinitramfs -l`) the bytes of `*.ko*` and of `kernel/x86/microcode/`. With a second argument it prints the delta of each number against the same numbers computed from that directory's files. It ends with `ok`/`FAIL` lines for the three budgets.
- [ ] **Step 2: Build the baseline:** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make rootfs VERSION=3.0.0-size BUILD_JOBS=4`, then `mkdir -p /scratch/oetiker/claude-tmp/thinfloe-tests/size-baseline && cp artifacts/rootfs/{vmlinuz,initrd.img,ramroot.sqsh,version} /scratch/oetiker/claude-tmp/thinfloe-tests/size-baseline/`.
- [ ] **Step 3: Set the budgets** to the baseline sizes plus 2 %, rounded up to a whole MB, with a comment that Task 10 of this plan lowers them.
- [ ] **Step 4: Run** `test/integration/test-image-size.sh artifacts/rootfs /scratch/oetiker/claude-tmp/thinfloe-tests/size-baseline`. Expected: PASSED, every delta 0.
- [ ] **Step 5: Commit** `test: image size report and budget` with the `Sizes:` line of the baseline.

### Task 2: S4, build scripts out of the squashfs

**Files:**
- Modify: `scripts/export-rootfs.sh` (the `mksquashfs -e` list and the comment above it)
- Modify: `Makefile` (prerequisites of `$(KERNEL) $(INITRD) $(SQUASHFS)`; `test:` target)
- Create: `test/integration/test-image-slim.sh`
- Modify: `test/integration/test-container-inputs.sh`

**Interfaces:**
- Produces: `test/integration/test-image-slim.sh [ramroot.sqsh]`, structured as one `check`/`absent` helper pair over `unsquashfs -l`, so Tasks 3 to 7 append checks.

- [ ] **Step 0: Merge with release-flow Task 4 if it is on `main`.** Check with `git log --oneline main -- Makefile scripts/export-rootfs.sh`. If Task 4 landed, rebase this branch on `main` first, then merge both sides by hand where they meet: the `.ubuntu-container` prerequisites (Task 4 removes `RELEASE`, this plan adds `SLIM_FILES` in Task 3); the prerequisites of `$(KERNEL) $(INITRD) $(SQUASHFS)` (Task 4 adds `.version-kind`, this task adds `scripts/export-rootfs.sh scripts/lib/common.sh`); `scripts/export-rootfs.sh` (Task 4 changes the version export and adds a version write, this task changes the `-e` list); `test/integration/test-container-inputs.sh` (both add cases). Check that `VERSION=3.0.0-size` still sets the container tag next to Task 4's `TF_VERSION` (`make -n rootfs VERSION=3.0.0-size | grep thinfloe-ubuntu:3.0.0-size`). If Task 4 is not on `main`, note that in the commit message; whichever lands second does this merge.
- [ ] **Step 1: Write the failing tests.** `test-image-slim.sh`: `absent "no build scripts in /scripts" '^squashfs-root/scripts(/|$)'` and `absent "no slim scripts" '^squashfs-root/thinfloe-slim(/|$)'`. `test-container-inputs.sh`: a case "a changed export script re-exports the rootfs": with the stub runtime and a fresh `.ubuntu-container`, `touch scripts/export-rootfs.sh` makes `make -q artifacts/rootfs/ramroot.sqsh` exit non-zero; the same for `scripts/lib/common.sh`.
- [ ] **Step 2: Run** both on the Task 1 artifacts. Expected: `test-image-slim.sh` FAIL on "no build scripts in /scripts"; `test-container-inputs.sh` FAIL on the new case.
- [ ] **Step 3: Implement.** Add `scripts` to the `-e` list, and to the comment why: the Makefile bind-mounts `./scripts` at `/scripts`, and 4.2.0 shipped `/scripts/publish.sh` and the smoke scripts. Add `scripts/export-rootfs.sh scripts/lib/common.sh` to the prerequisites of `$(KERNEL) $(INITRD) $(SQUASHFS)`. Add `@test/integration/test-image-slim.sh` to `test:`.
- [ ] **Step 4: Run** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make rootfs VERSION=3.0.0-size BUILD_JOBS=4`, then `test/integration/test-image-slim.sh`, `test/integration/test-container-inputs.sh`, `test/integration/test-image-size.sh`. Expected: PASSED.
- [ ] **Step 5: Commit** `fix: keep the build scripts out of ramroot.sqsh` with the `Sizes:` line.

### Task 3: The slim step, with S2 and S3

**Files:**
- Create: `containers/ubuntu/slim/slim-rootfs.sh`
- Modify: `containers/ubuntu/Dockerfile` (new `COPY` and `RUN` as the last `RUN`, after the cleanup `RUN`; comment on the dracut `RUN`)
- Modify: `Makefile` (`SLIM_FILES`, prerequisites of `.ubuntu-container`)
- Modify: `test/integration/test-container-inputs.sh`, `test/integration/test-image-slim.sh`

**Interfaces:**
- Produces: `slim-rootfs.sh` with `set -euo pipefail`, a `die()` that prints `slim-rootfs: <message>` to stderr and exits 1, the global `KVER`, and one function per cut called from `main` in the spec's order: `trim_media_modules` (Task 5), `decompress_modules` (Task 4), `prune_iwlwifi` (Task 6), `prune_amdgpu` (Task 9), `check_firmware_names` (Task 6), `drop_intel_ucode` (Task 7), `drop_i18n`, `drop_hwdb_sources`. Tasks 4 to 7 and 9 fill in their function; this task leaves them as empty functions with a one-line comment naming the task.
- Produces: `main` step 0 (`preconditions`): exactly one directory in `/usr/lib/modules`, else `die "want one kernel in /usr/lib/modules, found: …"`; `/boot/initrd.img-$KVER` and `/boot/System.map-$KVER` exist, else `die "run after dracut: no /boot/initrd.img-$KVER"`.

- [ ] **Step 1: Write the failing tests.** `test-image-slim.sh`: `absent "no locale sources" '^squashfs-root/usr/share/i18n(/|$)'`, `check "locale archive" '^squashfs-root/usr/lib/locale/locale-archive$'`, `absent "no hwdb sources" '^squashfs-root/usr/lib/udev/hwdb\.d(/|$)'`, `hwdb.bin` present and over 1000000 bytes (from `unsquashfs -lls`), `etc/systemd/system/systemd-hwdb-update.service -> /dev/null`. `test-container-inputs.sh`: a case "a changed slim script rebuilds the container" (touching `containers/ubuntu/slim/slim-rootfs.sh` makes `make -q .ubuntu-container` non-zero) and "a deleted slim file rebuilds the container" (removing a second file in that directory does too).
- [ ] **Step 2: Run** both. Expected: FAIL on the new checks.
- [ ] **Step 3: Implement.** At the end of the Dockerfile, after the cleanup `RUN` (`apt-get -qqy clean && apt-get -qqy autoremove`): `COPY containers/ubuntu/slim/ /thinfloe-slim/` and `RUN /thinfloe-slim/slim-rootfs.sh && rm -rf /thinfloe-slim`, with a comment that points at the spec and says this must stay the last `RUN`: nothing after it may run `dpkg`, `apt-get` or dracut, because a `dpkg` run can fire the `update-initramfs` trigger and rebuild the initrd with default flags from the slimmed tree. Add a sentence to the dracut comment that the slim step at the end of the file depends on this initrd. `drop_i18n`: `rm -rf /usr/share/i18n`, then every locale named in `/etc/locale.gen` (non-comment lines, `de_CH.UTF-8` read as `de_CH.utf8`) must appear in `locale -a`, else `die "locale <name> missing after removing /usr/share/i18n"`. `drop_hwdb_sources`: `rm -rf /usr/lib/udev/hwdb.d`, then `systemd-hwdb query usb:v046DpC52B` must print at least one line, else `die`. In the Makefile `SLIM_FILES := $(wildcard containers/ubuntu/slim/*)`, and `$(SLIM_FILES) containers/ubuntu/slim` as prerequisites of `.ubuntu-container`, with a comment like the one on `PATCH_FILES`.
- [ ] **Step 4: Run** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make rootfs VERSION=3.0.0-size BUILD_JOBS=4`, then `test/integration/test-image-slim.sh`, `test/integration/test-container-inputs.sh`, `TF_UBUNTU_IMAGE=thinfloe-ubuntu:3.0.0-size test/integration/test-session-packages.sh`, `test/integration/test-image-size.sh`. Expected: PASSED; sum about 5 MB below Task 2.
- [ ] **Step 5: Commit** `build: slim step at the end of the image build; drop locale and hwdb sources` with the `Sizes:` line.

### Task 4: S1, uncompressed modules in the squashfs

**Files:**
- Modify: `containers/ubuntu/slim/slim-rootfs.sh` (`preconditions` snapshot, `decompress_modules`)
- Create: `test/integration/test-initramfs-drivers.sh`
- Modify: `test/integration/test-image-slim.sh`, `scripts/check-boot-smoke.sh`, `test/integration/test-boot-smoke-check.sh`, `Makefile` (`test:` target)

**Interfaces:**
- Consumes: `KVER`, `die`, `main` order from Task 3.
- Produces: `DEPMOD_BEFORE`, a file in `/thinfloe-slim/` written by `preconditions` with the sorted output of `depmod -n -e -F /boot/System.map-$KVER $KVER 2>&1 >/dev/null`; `run_depmod`, which runs `depmod -a -e -F /boot/System.map-$KVER $KVER`, compares its sorted stderr with `DEPMOD_BEFORE`, both with `.ko.zst` rewritten to `.ko` (the snapshot names the compressed files), and on any new line calls `die "depmod reports new problems:"` followed by those lines. Task 5 relies on `run_depmod` being called once, at the end of `decompress_modules`.
- Produces: `test/integration/test-initramfs-drivers.sh [initrd.img]`, listing the initrd once with `lsinitramfs`, so Tasks 7 and 8 append checks.

- [ ] **Step 1: Write the failing tests.** `test-image-slim.sh`: `absent "modules stored uncompressed" '^squashfs-root/usr/lib/modules/.*\.ko\.zst$'`; extract `modules.dep` (as `test-wifi-stack.sh` does) and check that every path in it, left and right of the colon, is in the squashfs list ("modules.dep names only files that exist"). `test-initramfs-drivers.sh`: at least 500 `*.ko.zst` and no `*.ko` in the initrd ("the initrd keeps compressed modules, so S1 ran after dracut"). `test-boot-smoke-check.sh`: a fixture case that a log line `foo: Unknown symbol bar (err -2)` fails `check-boot-smoke.sh`. No `modprobe: FATAL` case: udev loads modules through libkmod, which never prints it; the boot check is weak, and the real guards are `run_depmod` and the `modules.dep` check above.
- [ ] **Step 2: Run** them on the Task 3 artifacts. Expected: `test-image-slim.sh` FAIL on "modules stored uncompressed"; `test-initramfs-drivers.sh` PASS (the initrd already is compressed); `test-boot-smoke-check.sh` FAIL on the new case.
- [ ] **Step 3: Check** the existing smoke log first: `grep -a 'Unknown symbol' artifacts/images/qemu-smoke.log`. Expected: no match. If there is one, stop and report it before adding the line.
- [ ] **Step 4: Implement.** `decompress_modules`: `find /usr/lib/modules/$KVER -name '*.ko.zst' -print0 | xargs -0 -r -P 4 -n 200 zstd -d -q --rm`, then `die` if any `*.ko.zst` remains, then `run_depmod`. `check-boot-smoke.sh`: `unwant "no module with unknown symbols" 'Unknown symbol'`, with a comment that this catches only modules the kernel refuses, not missing ones. Add `@test/integration/test-initramfs-drivers.sh` to `test:`.
- [ ] **Step 5: Run** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make rootfs VERSION=3.0.0-size BUILD_JOBS=4`, then `test-image-slim.sh`, `test-initramfs-drivers.sh`, `test-wifi-stack.sh`, `test-initramfs-commands.sh`, `test-boot-smoke-check.sh`, `test-image-size.sh`, then `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make qemu-smoke VERSION=3.0.0-size BUILD_JOBS=4`. Expected: all PASSED; `ramroot.sqsh` about 25 MB below Task 3, `initrd.img` unchanged.
- [ ] **Step 6: Commit** `build: store kernel modules uncompressed in ramroot.sqsh` with the `Sizes:` line.

### Task 5: O4, no DVB, TV tuner or radio modules

**Files:**
- Modify: `containers/ubuntu/slim/slim-rootfs.sh` (`trim_media_modules`)
- Modify: `test/integration/test-image-slim.sh`

**Interfaces:**
- Consumes: `KVER`, `die`; `run_depmod` from Task 4 runs after this function.
- Produces: nothing for later tasks.

- [ ] **Step 1: Write the failing tests** in `test-image-slim.sh`: `absent` for any module under `usr/lib/modules/*/kernel/drivers/media/(dvb-frontends|tuners|radio|dvb-core|usb/dvb-usb|usb/dvb-usb-v2)/` other than `tea575x.ko`; `absent` for `cx88-dvb.ko`, `saa7134-dvb.ko`, `em28xx-dvb.ko`; `check` present, as `.ko`: `cx88xx`, `cx8800`, `saa7134`, `em28xx`, `em28xx-v4l` (cores and analog parts stay), `uvcvideo`, `ib_core`, `comedi`, `industrialio`, `can`, `dsa_core`, `mtd`, `mlx5_core`, `amdgpu`, `radeon`, `nouveau`, `iwlwifi`, `snd-fm801`, `tea575x`.
- [ ] **Step 2: Run** it on the Task 4 artifacts. Expected: FAIL on the `absent` lines.
- [ ] **Step 3: Implement `trim_media_modules`** with the spec's rule, reading `/usr/lib/modules/$KVER/modules.dep` before anything changes. Seed: paths under the six seed directories. Closure: add every module under `kernel/drivers/media/` or `kernel/drivers/staging/media/` whose dependency list contains a module in the set, until nothing changes. Put back: remove from the set every module that a module outside the set depends on. Delete the remaining set; print `slim-rootfs: removed <n> media modules` to stderr; `die` when `n` is below 100 (a moved directory must not silently remove nothing). Use `python3 -I` with an inline script or awk; the algorithm is the one above.
- [ ] **Step 4: Run** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make rootfs VERSION=3.0.0-size BUILD_JOBS=4`, then `test-image-slim.sh`, `test-image-size.sh`, then `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make qemu-smoke VERSION=3.0.0-size BUILD_JOBS=4`. Expected: PASSED; the build log shows 259 media modules removed (218 seed, 260 after the closure, `tea575x` put back); `ramroot.sqsh` about 6 MB below Task 4.
- [ ] **Step 5: Commit** `build: drop DVB, TV tuner and radio modules from ramroot.sqsh` with the `Sizes:` line.

### Task 6: O1, Intel WiFi firmware the driver loads

**Files:**
- Create: `containers/ubuntu/slim/iwlwifi_keep.py`
- Create: `test/unit/test_iwlwifi_keep.py`, `test/unit/iwlwifi-fixtures/modinfo.txt` (copy of `/scratch/oetiker/claude-tmp/thinfloe-tests/size-audit/iwlmax.txt`), `test/unit/iwlwifi-fixtures/files.txt` (the basenames in `usr/lib/firmware/intel/iwlwifi/` from `/scratch/oetiker/claude-tmp/thinfloe-tests/size-audit/lls.txt`, one per line)
- Modify: `containers/ubuntu/slim/slim-rootfs.sh` (`preconditions` snapshot, `prune_iwlwifi`, `check_firmware_names`)
- Modify: `test/integration/test-firmware.sh`
- Modify: `CLAUDE.md` ("Firmware is selected by package, never by cleanup"), `docs/notes/history-of-constraints.md` (same section), `containers/ubuntu/Dockerfile` (the cleanup comment "Never re-introduce a find/rm sweep")

**Interfaces:**
- Produces in `iwlwifi_keep.py`: `ceilings(names: list[str]) -> dict[tuple, int | None]` with two kinds of key, (family, radio, `"c"` or `""`) and (family, `"c"` or `""`), `None` for a macro; `ceiling_for(stem: str, c: str, table) -> int | None | Missing` looks up the radio key first and falls back to the family key (radio = second to last dash token of a stem with four or more tokens, else none); `select(names: list[str], files: list[str]) -> tuple[list[str], list[str]]` returning (keep, drop) basenames; `class TooFewNames(ValueError)`, raised by `select` below 20 parsed names. CLI: `python3 iwlwifi_keep.py MODINFO_FILE FIRMWARE_DIR` prints the basenames to drop, one per line; exit 2 with `iwlwifi_keep: only <n> firmware names from modinfo` on `TooFewNames`.
- Produces in `slim-rootfs.sh`: `FW_BEFORE`, written by `preconditions`: sorted unique firmware names from `modinfo -F firmware` over every module file, kept only when `/usr/lib/firmware/<name>` or `<name>.zst` exists. `DROPPED` (`/thinfloe-slim/dropped`): every prune function appends the paths it removed on purpose, relative to `/usr/lib/firmware` and without `.zst`; `check_firmware_names` skips those names. Task 9 appends the AMD drop list there.

- [ ] **Step 1: Write the failing unit tests** in `test_iwlwifi_keep.py` (load the module with `importlib.util.spec_from_file_location` from `containers/ubuntu/slim/iwlwifi_keep.py`), on the fixtures:
  - `test_keeps_highest_under_ceiling`: keep has `iwlwifi-so-a0-gf-a0-89.ucode.zst`, drop has `iwlwifi-so-a0-gf-a0-86.ucode.zst`; keep has `iwlwifi-8265-36.ucode.zst`, drop has `iwlwifi-8265-34.ucode.zst`; keep has `iwlwifi-cc-a0-77.ucode.zst`, drop has `iwlwifi-cc-a0-74.ucode.zst`.
  - `test_ceiling_cuts_newer_core_files`: keep has `iwlwifi-sc-a0-wh-b0-c101.ucode.zst`, drop has `iwlwifi-sc-a0-wh-b0-c102.ucode.zst` and `-c103`.
  - `test_unresolved_ceiling_keeps_all`: `ceilings(...)[("bz", "c")] is None`; every `iwlwifi-bz-b0-fm-c0-c10[123].ucode.zst` and `iwlwifi-gl-c0-fm-c0-c10[123].ucode.zst` is in keep.
  - `test_unnamed_family_keeps_highest`: keep has `iwlwifi-gl-c0-fm-c0-101.ucode.zst`, drop has `iwlwifi-gl-c0-fm-c0-100.ucode.zst`.
  - `test_legacy_and_pnvm_kept`: keep has `iwlwifi-3945-2.ucode.zst`, `iwlwifi-4965-2.ucode.zst`, `iwlwifi-6000-4.ucode.zst` and every `*.pnvm.zst`.
  - `test_series_above_ceiling_kept_whole`: `iwlwifi-sc-a0-wh-b0-101.ucode.zst` is the only file of its numeric series and above the `sc` numeric ceiling 100; it is in keep, next to `iwlwifi-sc-a0-wh-b0-c101.ucode.zst` of the `c`-series.
  - `test_too_few_names_refused`: `select(names[:5], files)` raises `TooFewNames`.
  - `test_radio_ceiling_wins`: with the fixture files plus a synthetic `iwlwifi-so-a0-jf-b0-83.ucode.zst`, keep has `iwlwifi-so-a0-jf-b0-77.ucode.zst` and drop has `-83` (`modinfo` names `so-a0-jf-b0-77`; the `so` family ceiling 100 must not apply).
  - `test_fixture_totals`: 111 files in drop, 63 in keep.
- [ ] **Step 2: Run** `python3 -m unittest test.unit.test_iwlwifi_keep -v` from the repository root (or `make test-unit`). Expected: FAIL, module not found.
- [ ] **Step 3: Implement `iwlwifi_keep.py`** with the spec's rule (section `O1`): file pattern `^iwlwifi-(?P<stem>.+)-(?P<c>c?)(?P<api>\d+)\.ucode(\.zst)?$`, `modinfo` pattern the same with `(?P<api>\d+|[A-Z_]+)` and no `.zst`, family = `stem.split("-")[0]`, radio = `stem.split("-")[-2]` when the stem has four or more tokens.
- [ ] **Step 4: Run** the unit tests. Expected: PASS.
- [ ] **Step 5: Write the failing integration checks** in `test-firmware.sh`: extract `iwlwifi.ko`, `iwlmvm.ko`, `iwldvm.ko`, `iwlmld.ko`, `iwl3945.ko`, `iwl4965.ko` from the squashfs with `unsquashfs -ef` (the host's `modinfo -F firmware <file>` reads them); with its own parsing (do not import `iwlwifi_keep.py`), check "at most one ucode per series with a known ceiling", "no ucode above its ceiling" (the same radio-then-family lookup as the rule, written again in the test), "no dangling symlink under usr/lib/firmware" (resolve every link from `unsquashfs -lls` against the listed paths), and one `check` each for a ucode of `7265D`, `8265`, `9260-th-b0-jf-b0`, `cc-a0`, `Qu-c0-hr-b0`, `QuZ-a0-hr-b0`, `so-a0-gf-a0` (plus `iwlwifi-so-a0-gf-a0.pnvm`), `ty-a0-gf-a0`, `ma-b0-gf-a0`, `bz-b0-fm-c0`, `gl-c0-fm-c0`.
- [ ] **Step 6: Run** `test/integration/test-firmware.sh` on the Task 5 artifacts. Expected: FAIL on "at most one ucode per series".
- [ ] **Step 7: Implement in `slim-rootfs.sh`.** `prune_iwlwifi`: `modinfo -F firmware` of every module under `/usr/lib/modules/$KVER/kernel/drivers/net/wireless/intel/` into `/thinfloe-slim/iwlwifi-names`, run `iwlwifi_keep.py` on it and `/usr/lib/firmware/intel/iwlwifi`, delete what it prints, then delete every symlink under `/usr/lib/firmware` that no longer resolves (`find -xtype l`), and `die` if any symlink there still dangles afterwards. Print `slim-rootfs: removed <n> iwlwifi files`. `check_firmware_names`: names from `modinfo -F firmware` over the modules now in the tree, intersected with `FW_BEFORE`, minus `DROPPED`; each must still exist as `/usr/lib/firmware/<name>` or `<name>.zst`, else `die "firmware <name> was removed but <module> still asks for it"` (find the module with one more `modinfo` loop only on failure).
- [ ] **Step 8: Docs.** `CLAUDE.md` and `docs/notes/history-of-constraints.md`: the rule stays; the one exception is `containers/ubuntu/slim/slim-rootfs.sh`, which keeps per Intel WiFi series the file the driver loads and removes `intel-ucode` after dracut (owner, 2026-11-12); guards `test-firmware.sh`, `test_iwlwifi_keep.py` and the build-time firmware-name check. The Dockerfile comment names the same exception.
- [ ] **Step 9: Run** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make rootfs VERSION=3.0.0-size BUILD_JOBS=4`, then `test-firmware.sh`, `test-wifi-stack.sh`, `test-image-size.sh`, `make test-unit`. Expected: PASSED; the build log shows about 111 iwlwifi files removed; `ramroot.sqsh` 55 to 61 MB below Task 5.
- [ ] **Step 10: Commit** `build: keep only the Intel WiFi firmware the driver loads` with the `Sizes:` line.

### Task 7: O3, microcode only in the initrd

**Files:**
- Modify: `containers/ubuntu/slim/slim-rootfs.sh` (`drop_intel_ucode`)
- Modify: `test/integration/test-firmware.sh`, `test/integration/test-initramfs-drivers.sh`

**Interfaces:**
- Consumes: `KVER`, `die` from Task 3; `test-initramfs-drivers.sh` list from Task 4. `test-firmware.sh` now also takes the initrd: `test-firmware.sh [ramroot.sqsh [initrd.img]]`, default `artifacts/rootfs/initrd.img`.

- [ ] **Step 1: Write the failing tests.** `test-firmware.sh`: replace `check "Intel CPU microcode" 'usr/lib/firmware/intel-ucode/'` with an absence check "no intel-ucode in the squashfs (it is applied from the initrd)". `test-initramfs-drivers.sh`: `kernel/x86/microcode/GenuineIntel.bin` is in the `lsinitramfs -l` list with a size over 1000000 bytes.
- [ ] **Step 2: Run** both on the Task 6 artifacts. Expected: `test-firmware.sh` FAIL on the new absence check; `test-initramfs-drivers.sh` PASS.
- [ ] **Step 3: Implement `drop_intel_ucode`:** `3cpio --list /boot/initrd.img-$KVER >"$tmp" && grep -qx kernel/x86/microcode/GenuineIntel.bin "$tmp" || die "no early microcode in /boot/initrd.img-$KVER; not removing intel-ucode"` with `tmp` a file in `/thinfloe-slim/` (the image has `3cpio`, not `cpio`; no pipe, because `grep -q` can end the lister with SIGPIPE under `pipefail`), then `rm -rf /usr/lib/firmware/intel-ucode`. Update the header comment of `test-firmware.sh`: the squashfs once lost microcode by accident; now it leaves it out on purpose because the initrd carries it.
- [ ] **Step 4: Run** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make rootfs VERSION=3.0.0-size BUILD_JOBS=4`, then `test-firmware.sh`, `test-initramfs-drivers.sh`, `test-image-size.sh`. Expected: PASSED; `ramroot.sqsh` about 14 MB below Task 6, `initrd.img` unchanged.
- [ ] **Step 5: Commit** `build: apply CPU microcode from the initrd only` with the `Sizes:` line.

### Task 8: O2 and O5, a smaller initrd

**Files:**
- Modify: `containers/ubuntu/Dockerfile` (dracut `--omit-drivers`, `--compress`, the comment above the dracut `RUN`)
- Modify: `test/integration/test-initramfs-drivers.sh`

**Interfaces:**
- Consumes: `test-initramfs-drivers.sh` from Tasks 4 and 7.

- [ ] **Step 1: Write the failing tests** in `test-initramfs-drivers.sh`:
  - absent from the initrd, by module file name (`-` and `_` both accepted): every name of the spec's `O2` list (server NICs with `chcr`, FC and SAS HBAs, InfiniBand with `rnbd_client`, network file systems without `ubifs`), held in one array `OMITTED` in the test. `ubifs` stays and is in the present list below.
  - present: `e1000 e1000e igb igc r8169 atlantic tg3 i915 overlay zram ubifs`.
  - closure: `unmkinitramfs` into the scratch directory; for every `*.ko.zst` there, each name in `modinfo -F depends <file>` (comma-separated) has a module file in the initrd; a failure names both modules.
- [ ] **Step 2: Run** on the Task 7 artifacts. Expected: FAIL on the absent checks; the closure check PASS.
- [ ] **Step 3: Implement.** Append the `O2` names to `--omit-drivers`, grouped in the comment above the `RUN` by the four categories with one line each on why the initrd does not need them (ThinFloe boots from USB or over HTTP; the running system keeps every driver). Change `--compress "zstd -3 -T4"` to `--compress "zstd -19 -T4"`, with a comment: same decompression speed at every level; not `--ultra -22`, whose 128 MiB window the kernel's initramfs decompressor would have to allocate.
- [ ] **Step 4: Run** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make rootfs VERSION=3.0.0-size BUILD_JOBS=4`, then `test-initramfs-drivers.sh`, `test-initramfs-commands.sh`, `test-image-size.sh`, then `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make qemu-smoke qemu-smoke-netboot VERSION=3.0.0-size BUILD_JOBS=4`. Expected: PASSED; `initrd.img` about 15 to 19 MB below Task 7 (11.7 MB of omitted modules, 4 to 7 MB from level 19).
- [ ] **Step 5: Commit** `build: leave server drivers out of the initrd and compress it at zstd 19` with the `Sizes:` line.

### Task 9: A1, AMD graphics firmware for APUs

**Files:**
- Modify: `containers/ubuntu/Dockerfile` (package list: `linux-firmware-amd-graphics` after `linux-firmware-intel-graphics`; the cleanup comment on firmware)
- Create: `containers/ubuntu/slim/amdgpu_keep.py`, `containers/ubuntu/slim/amdgpu-apu-ip.csv` and `containers/ubuntu/slim/amdgpu-dgpu-ip.csv` (verbatim copies of `/scratch/oetiker/claude-tmp/thinfloe-tests/size-design/amd/src/apu-asic-info-table.csv` and `dgpu-asic-info-table.csv`, which are `Documentation/gpu/amdgpu/` of torvalds/linux tag `v7.0`, each with a first comment line naming that source and the date 2026-11-12, which the parser skips)
- Create: `test/unit/test_amdgpu_keep.py`; fixtures in `test/unit/amdgpu-fixtures/`: `files.txt` (one line per entry of `amdgpu/` in the package: the name, and for a link ` -> <target>`; make it with `find /scratch/oetiker/claude-tmp/thinfloe-tests/size-design/amd/x/usr/lib/firmware/amdgpu -mindepth 1 -printf '%f\n'` for files and `-type l -printf '%f -> %l\n'` for links), `modinfo.txt` (copy of `/scratch/oetiker/claude-tmp/thinfloe-tests/size-design/amd/modinfo.txt`)
- Modify: `containers/ubuntu/slim/slim-rootfs.sh` (`prune_amdgpu`), `test/integration/test-firmware.sh`, `test/integration/test-initramfs-drivers.sh`
- Modify: `CLAUDE.md` and `docs/notes/history-of-constraints.md` (firmware rule: the AMD exception), `docs/notes/session.md` (`amdgpu` binds after switch-root and replaces `simpledrm`; when its firmware is missing: amdgpu fails; the boot waits about 20 s, and then either the menu runs unaccelerated on simpledrm or, worst case, the failure screen appears after about 40 s.)

**Interfaces:**
- Consumes: `DROPPED`, `die`, `KVER` (Tasks 3 and 6).
- Produces in `amdgpu_keep.py`: `apu_prefixes(csv_text: str) -> tuple[list[str], list[str]]` returning (IP prefixes such as `gc_11_5_0`, `dcn_3_5_0`, `psp_14_0_0`; two-part forms for patch level 0 as `dcn_3_5`), skipping the `CARRIZO/STONEY` row and `n/a`, MP0 mapped to `psp`; `select(csv_text: str, entries: dict[str, str | None]) -> tuple[list[str], list[str]]` with `entries` mapping each name in `amdgpu/` to its link target or `None`, returning (keep, drop); `class TooSmall(ValueError)` below 15 APU rows or 100 kept names. `DGPU_EXTRAS = ("gc_9_4_4", "gc_9_5_0", "psp_13_0_12", "psp_13_0_14", "sdma_4_4_4", "sdma_4_4_5", "vcn_5_0_1")`; `unknown_versions(apu_csv: str, dgpu_csv: str, modinfo: list[str], entries: dict[str, str | None], keep: list[str]) -> list[str]` returns, sorted, every `gc`/`psp`/`dcn`/`vcn`/`sdma` version named by `modinfo` that has files in `entries` and is in neither table nor `DGPU_EXTRAS`, plus every `psp_X_Y_Z` with a `toc` and no `sos` file that is not in `keep`; `class UnknownVersions(ValueError)`. CLI: `python3 amdgpu_keep.py APU_CSV DGPU_CSV MODINFO_FILE FIRMWARE_DIR`, exit 3 with `amdgpu_keep: unknown amdgpu IP versions: <list>; refresh the tables from the kernel` when `unknown_versions` is not empty. Constants: `CHIPS = ("raven", "raven2", "picasso", "renoir", "green_sardine", "yellow_carp", "vangogh")`, `APU_ONLY = ("vpe", "umsch_mm", "isp")`. The CLI prints the names to drop; exit 2 with `amdgpu_keep: <reason>` on `TooSmall`.

- [ ] **Step 1: Write the failing unit tests** in `test_amdgpu_keep.py` (load as in Task 6), on the fixtures and the CSV:
  - `test_newer_apus_by_ip`: keep has `dcn_3_5_dmcub.bin.zst`, `dcn_3_6_dmcub.bin.zst`, `dcn_3_5_1_dmcub.bin.zst`, `gc_11_5_0_mes_2.bin.zst`, `psp_14_0_0_toc.bin.zst`, `gc_11_0_4_me.bin.zst`, `isp_4_1_1.bin.zst`, `vpe_6_1_0.bin.zst`, `umsch_mm_4_0_0.bin.zst`.
  - `test_older_apus_by_chip`: keep has `raven_dmcu.bin.zst`, `picasso_vcn.bin.zst`, `renoir_dmcub.bin.zst`, `green_sardine_vcn.bin.zst`, `yellow_carp_dmcub.bin.zst`, `vangogh_toc.bin.zst`.
  - `test_shared_file_kept_through_link`: keep has `vcn_4_0_2.bin.zst` and its target `vcn_4_0_0.bin.zst`.
  - `test_dgpu_dropped`: drop has every `navi10_*`, `gc_11_0_0_*`, `psp_13_0_0_*`, `smu_13_0_0*`, `polaris10_*`, `carrizo_*` entry.
  - `test_fixture_totals`: 178 kept, 489 dropped.
  - `test_short_table_refused`: the CSV cut to its header and 5 rows raises `TooSmall`.
  - `test_known_versions_pass`: `unknown_versions` on the real fixtures and both tables returns `[]` (it ignores `gc_11_5_4`, `sdma_6_1_4`, `psp_15_0_0`, `psp_15_0_8`, `vcn_5_3_0`, which `modinfo` names but the package has no files for).
  - `test_new_gc_version_refused`: the fixtures plus a synthetic entry `gc_11_5_4_me.bin.zst` return `["gc_11_5_4"]`.
  - `test_unkept_apu_psp_refused`: the fixtures plus synthetic entries `psp_15_0_0_toc.bin.zst` and `psp_15_0_0_ta.bin.zst` (no `sos`), not in keep, return a list containing `psp_15_0_0`.
- [ ] **Step 2: Run** `python3 -m unittest test.unit.test_amdgpu_keep -v`. Expected: FAIL, module not found.
- [ ] **Step 3: Implement `amdgpu_keep.py`** with the spec's rule (section `A1`): keep by chip prefix (`<chip>_`, with `raven_` not matching `raven2_`), by IP prefix (`<prefix>` followed by `_` or the end; a two-part prefix only when followed by `_` and a letter), by APU-only block; then add link targets until nothing changes.
- [ ] **Step 4: Run** the unit tests. Expected: PASS.
- [ ] **Step 5: Write the failing integration checks.** `test-firmware.sh`: the pinned per-APU list of the spec (section `A1`, "Guards", the `test-firmware.sh` item, Strix Halo with `vcn_4_0_6_1`) as `check` lines on `usr/lib/firmware/amdgpu/<name>.bin(.zst)?`, plus `absent` for `usr/lib/firmware/amdgpu/navi10_` and `usr/lib/firmware/radeon/`. `test-initramfs-drivers.sh`: `absent` for `amdgpu.ko` and `usr/lib/firmware/amdgpu/`.
- [ ] **Step 6: Run** `test-firmware.sh` on the Task 8 artifacts. Expected: FAIL on the pinned AMD checks.
- [ ] **Step 7: Implement.** Dockerfile: add `linux-firmware-amd-graphics`. `prune_amdgpu`: write `modinfo -F firmware` of `amdgpu.ko` to `/thinfloe-slim/amdgpu-names`, run `amdgpu_keep.py /thinfloe-slim/amdgpu-apu-ip.csv /thinfloe-slim/amdgpu-dgpu-ip.csv /thinfloe-slim/amdgpu-names /usr/lib/firmware/amdgpu` (a non-zero exit fails the build with its message), delete what it prints, `rm -rf /usr/lib/firmware/radeon`, append both to `DROPPED`, remove links under `/usr/lib/firmware` that no longer resolve and `die` if any remain; then every name `modinfo -F firmware` of `amdgpu.ko` lists that matches a kept prefix must exist (as `<name>` or `<name>.zst`), else `die "amdgpu firmware <name> missing after the APU prune"`. Print `slim-rootfs: kept <k>, removed <d> amdgpu files`. Docs as listed under **Files**; the Dockerfile cleanup comment names the AMD exception.
- [ ] **Step 8: Run** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make rootfs VERSION=3.0.0-size BUILD_JOBS=4`, then `test-firmware.sh`, `test-initramfs-drivers.sh`, `test-image-size.sh`, `make test-unit`. Expected: PASSED; the build log shows about 178 kept and 489 removed; `ramroot.sqsh` about 8 MB above Task 8, `initrd.img` unchanged.
- [ ] **Step 9: Commit** `build: AMD graphics firmware for Ryzen APUs` with the `Sizes:` line.

### Task 10: Budget, changelog, full verification

**Files:**
- Modify: `test/integration/test-image-size.sh` (budgets, set from the measurement with `A1` included), `CHANGES.md`, `CLAUDE.md` ("Build" rules)

- [ ] **Step 1: Run** `test/integration/test-image-size.sh artifacts/rootfs /scratch/oetiker/claude-tmp/thinfloe-tests/size-baseline` and record the sum and every delta.
- [ ] **Step 2: Lower the budgets** to the measured sizes plus 2 %, rounded up to a whole MB, with the date and the sum in a comment.
- [ ] **Step 3: `CLAUDE.md`, "Build":** `SLIM_FILES` joins the list of `.ubuntu-container` prerequisites; the `containers/ubuntu/slim/slim-rootfs.sh` `RUN` stays the last in the Dockerfile and nothing after it may run `dpkg`, `apt-get` or dracut; `scripts/export-rootfs.sh` is a prerequisite of the rootfs artifacts; `test-image-size.sh` holds a size budget, and raising it needs a reason in the commit message.
- [ ] **Step 4: `CHANGES.md`** under `[Unreleased]`, with `N` the measured sum rounded to a whole MB. Under `### New`, first in the section: `ThinFloe now shows its picture on mini PCs with AMD Ryzen processors and their built-in Radeon graphics (Ryzen 2000 series and newer, up to Ryzen AI): the image carries the graphics firmware for these processors. AMD graphics cards still get no firmware.` Under `### Changed`, first in the section:
  - `An upgrade or a netboot now downloads about N MB instead of 611 MB. The running system keeps Intel WiFi firmware only in the version its driver loads, and has no drivers for DVB, TV tuner and radio cards and no CPU microcode files, which are applied from initrd.img at boot as before. During boot, before the system is loaded, drivers for data-centre network cards, Fibre Channel and SAS storage controllers, InfiniBand and network file systems (CIFS, Ceph, AFS, NFS) are no longer available.` (with `initrd.img` in a code span)
  - `` `locale-gen` and `systemd-hwdb update` no longer work on a running machine: the locale and hardware database sources are left out. The locales of `/etc/locale.gen` and the hardware database ship ready to use. ``

  Under `### Fixed`: `` The image no longer contains the build and test scripts in `/scripts`. ``
- [ ] **Step 5: Run** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test VERSION=3.0.0-size BUILD_JOBS=4`. Expected: PASS, including the new `test-image-slim.sh`, `test-initramfs-drivers.sh`, `test-image-size.sh`.
- [ ] **Step 6: Run** `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make image qemu-smoke qemu-smoke-netboot qemu-smoke-timezone qemu-smoke-timezone-netboot qemu-smoke-upgrade qemu-smoke-addons qemu-smoke-two-sticks qemu-smoke-write qemu-smoke-grow VERSION=3.0.0-size BUILD_JOBS=4` (Bash `timeout: 600000`, rerun in parts if it does not fit). Expected: every target PASS.
- [ ] **Step 7: Commit** `docs: image size budget and changelog` with the final `Sizes:` line and the total delta against the baseline.
- [ ] **Step 8 (owner, on hardware):** boot the image on the NUC7 (Intel 8265) and the Alder Lake-P machine, from USB. Expected: the boot picture as before; WiFi associates; `journalctl -k | grep -E 'iwlwifi.*loaded firmware version|microcode'` shows the firmware version and the microcode line; `journalctl -b -p err` shows nothing new against 4.2.0.
- [ ] **Step 8b (owner, on hardware, machine to be named by the owner):** boot the image from USB on an AMD Ryzen mini PC with integrated graphics. Expected: the boot picture on `simpledrm`, then the menu on `amdgpu` (no wait-kms failure screen); `journalctl -k | grep amdgpu` shows the driver initialised and no `failed to load firmware` or `Direct firmware load` line for `amdgpu/`.
- [ ] **Step 9 (owner):** the measured sum (with the 8 MB of `A1`) against the target of about 460 MB (spec, "Expected result"). The controller reports the number; the owner decides whether it is enough before Task 15 of the release-flow plan.
