# ThinFloe Part 3: Slim Down Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the ThinFloe download about 135 MB smaller (772 → about 635 MB) with four cuts: no WiFi in the initrd, squashfs at zstd 19, `vim-tiny` instead of `vim`, no `ubuntu` account.

**Architecture:** Every cut is a change to the dracut module, the `Makefile`/export script, or `containers/ubuntu/Dockerfile`. Each comes with a shell assertion in `test/integration/` that reads the built `initrd.img` or `ramroot.sqsh`, so it fails on today's artifacts and passes after a rebuild. Each cut is one commit with its measured size delta.

**Tech Stack:** bash test scripts (`lsinitramfs`, `unsquashfs`), dracut 110, podman build, GNU make.

**Spec:** `docs/superpowers/specs/2026-10-21-thinfloe-design.md`, section "Part 3: Slim down" (commit c4e7eed).

## Global Constraints

- Firmware is selected by package, never by a cleanup sweep; no `find`/`rm` over `/usr/lib/firmware`.
- `--no-hostonly` stays on the dracut call.
- The WiFi core stack comes from the kernel; `linux-modules-iwlwifi-generic` stays out.
- Every cut is a package-list change, a build option or an explicit dracut option.
- tfuser stays **uid 1001** (and gid 1001); field archives carry 1001 and the restore keeps numeric owners.
- The initrd keeps `--compress "zstd -3 -T4"`; `hwdb.bin` and `zfs.ko` stay in it; `firmware-sof-signed` stays in the image.
- Each cut is its own commit; the commit message records the delta of `ramroot.sqsh`, `initrd.img` (bytes, before → after).
- CHANGES.md: user-facing wording, under `### Changed` of `## [Unreleased]`, no release header, at most three sentences per entry.
- Machine rules: at most 4 cores; podman; every `make` runs as `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make <target>`; `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests`; never `rm` without asking the owner; never push.
- `artifacts/` is a symlink to the shared `/home/oetiker/scratch/dbrrg-artifacts`: a build anywhere replaces it. Record sizes right after your own build, and copy nothing you need later into it. Work on branch `thinfloe-slim` in a worktree under `/scratch/oetiker/claude-worktrees/dbrrg-slim` with `VERSION=slim` on every make line (two other sessions use the primary checkout).

Baseline (main @ 208e758, measured 2026-10-24): `ramroot.sqsh` 552243200 bytes, `initrd.img` 179380928 bytes, `vmlinuz` 17320328 bytes.

## Review Focus

1. **WiFi on real hardware after the initrd cut:** udev in the real root must load `iwlwifi` and its firmware from the squashfs. Not testable here; the existing `test-firmware.sh` and `test-wifi-stack.sh` must stay green (firmware and drivers still in the squashfs), and Task 1 records "not verified on hardware" in CLAUDE.md.
2. **Wired netboot still works without WiFi modules in the initrd:** `make qemu-smoke-netboot` must stay green (Task 5).
3. **A 3.x home restored onto the new image keeps its owner:** tfuser uid and gid 1001, and tfuser still in `sudo` and `audio` (Task 4 test).
4. **An admin types `vim`:** it must start `vim.tiny` (Task 3 test).
5. **Boot time does not get worse with zstd 19:** compare the `Startup finished` line of the QEMU smoke before and after Task 2 (Task 2, Step 5), and record both in the Task 2 commit message.

---

### Task 1: No WiFi in the initrd

**Files:**
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/module-setup.sh:54-60` (the `instmods iwlwifi iwlmvm cfg80211` line, the `# Firmware` comment and the `inst_multiple -o /usr/lib/firmware/iwlwifi-*` line)
- Modify: `containers/ubuntu/Dockerfile:349-351` only if you decide to add the other wireless modules to `--omit-drivers` (spec: builder's call; `mt76` SDIO, `libertas` SDIO, `mac80211` arrive through other dracut modules, with no firmware)
- Test: `test/integration/test-initramfs-commands.sh` (append before the final `if [[ $fail -ne 0 ]]`)
- Modify: `CLAUDE.md`, section "The initramfs has no cut, basename, head, install, wc or date" or a new short paragraph next to it

**Interfaces:**
- Consumes: nothing.
- Produces: an initrd with no `iwlwifi.ko*` and no `usr/lib/firmware/intel/iwlwifi/`.

- [ ] **Step 1: Write the failing test.** In `test-initramfs-commands.sh`, list the initrd once (`lsinitramfs "$INITRD" >"$WORK/initrd.list"`) and add:

```bash
if grep -qE '(^|/)iwlwifi\.ko' "$WORK/initrd.list"; then
    bad "the initrd carries iwlwifi.ko; WiFi drivers belong in the squashfs only"
else
    ok "no iwlwifi driver in the initrd"
fi
if grep -q 'usr/lib/firmware/intel/iwlwifi/' "$WORK/initrd.list"; then
    bad "the initrd carries Intel WiFi firmware (82 MB that nothing in the initrd uses)"
else
    ok "no Intel WiFi firmware in the initrd"
fi
```

Add a comment above it: why (nothing in the initrd uses WiFi, no `wpa_supplicant`, netboot is wired; 82 MB of the download).

- [ ] **Step 2: Run it against the current artifacts.**
Run: `test/integration/test-initramfs-commands.sh`
Expected: `FAIL - the initrd carries iwlwifi.ko…` and `FAIL - the initrd carries Intel WiFi firmware…`, exit 1.

- [ ] **Step 3: Remove the three lines from `module-setup.sh`** named under Files. Keep `instmods e1000 e1000e igb r8169 atlantic` and `instmods af_packet`. Decide on the extra `--omit-drivers` entries and say which in the commit message.

- [ ] **Step 4: Rebuild and rerun.**
Run: `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=slim rootfs`, then `test/integration/test-initramfs-commands.sh`, `test/integration/test-firmware.sh`, `test/integration/test-wifi-stack.sh`
Expected: all three `PASSED`; `ls -l artifacts/rootfs/initrd.img` about 80 MB smaller than 179380928.

- [ ] **Step 5: CLAUDE.md.** One short paragraph: the initrd carries no WiFi drivers or firmware (Part 3, 2026-10-24); the running system loads them from the squashfs after switch-root; `test-initramfs-commands.sh` guards it; **not verified on hardware** until a USB boot on the Intel 8265 machine associates from `~/wifi.yaml`.

- [ ] **Step 6: Commit** `module-setup.sh`, the test, CLAUDE.md (and the Dockerfile if changed): `slim: no WiFi drivers or firmware in the initrd`, body with the initrd byte delta.

### Task 2: squashfs at zstd level 19

**Files:**
- Modify: `Makefile:37` (`SQUASHFS_COMP_LEVEL ?= 19`) and `Makefile:70` (help text says 19)
- Modify: `scripts/export-rootfs.sh:10` (default `19`)
- Create: `test/integration/test-squashfs-format.sh` (executable, same style as the other `test/integration/*.sh`: `ok`/`bad`, `PASSED`/`FAILED` line, exit code)
- Modify: `Makefile` `test:` target, add `@test/integration/test-squashfs-format.sh` after `test-session-packages.sh`

**Interfaces:**
- Consumes: nothing.
- Produces: `ramroot.sqsh` with `unsquashfs -s` reporting `Compression zstd`, `compression-level 19`, `Block size 1048576`.

- [ ] **Step 1: Write the failing test** `test-squashfs-format.sh [ramroot.sqsh]` (default `artifacts/rootfs/ramroot.sqsh`). It runs `unsquashfs -s` once and asserts three lines: `^Compression zstd$`, `compression-level 19` and `^Block size 1048576$`. Header comment: zstd decompresses at the same speed at every level; 19 saved 40 MB in the survey; 22 saves nothing more; xz was rejected because it costs CPU on every page read.

- [ ] **Step 2: Run it.**
Run: `test/integration/test-squashfs-format.sh`
Expected: `FAIL` on the compression level (current build says `compression-level 3`), exit 1.

- [ ] **Step 3: Change the two defaults** named under Files. Note in the commit that a level change alone does not trigger a re-export (the export rule depends only on `.ubuntu-container`); force it with `touch .ubuntu-container`.

- [ ] **Step 4: Rebuild and rerun.**
Run: `touch .ubuntu-container && flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=slim rootfs`, then `test/integration/test-squashfs-format.sh`
Expected: `PASSED`; `ramroot.sqsh` about 40 MB smaller than after Task 1.

- [ ] **Step 5: Boot time.** Run `flock … make VERSION=slim qemu-smoke` and read the `Startup finished in …` line of `artifacts/images/qemu-smoke.log`. Baseline (zstd 3, 208e758 gate build): `3.150s (kernel) + 4.750s (initrd) + 3.451s (userspace) = 11.353s`. QEMU timing is noisy: if the userspace part is more than 30 % above 3.451 s, run the smoke a second time; if it stays there, stop and report to the controller.

- [ ] **Step 6: Commit** the Makefile, the script and the test: `slim: squashfs at zstd level 19`, body with the `ramroot.sqsh` delta and both boot times.

### Task 3: vim-tiny instead of vim

**Files:**
- Modify: `containers/ubuntu/Dockerfile:82` (`vim` → `vim-tiny`), plus one `RUN update-alternatives --install /usr/bin/vim vim /usr/bin/vim.tiny 10` after the package install, with a one-line comment (vim-tiny registers `vi`, `view`, `ex`, `editor`, not `vim`)
- Test: `test/integration/test-session-packages.sh` (next to the labwc dpkg-status check, which already extracts `var/lib/dpkg/status` into `$DPKG_TMP/x`)
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: nothing.
- Produces: `/usr/bin/vim` → `/etc/alternatives/vim` → `/usr/bin/vim.tiny`; package `vim` not installed.

- [ ] **Step 1: Write the failing tests** in `test-session-packages.sh`:
  - dpkg status has `Package: vim-tiny` with `Status: install ok installed`;
  - dpkg status has no installed `Package: vim` (exact name; `vim-common` and `vim-tiny` must not match);
  - the listing has `usr/bin/vim.tiny` and `usr/bin/vim`; `etc/alternatives/vim` is a link to `/usr/bin/vim.tiny` (extract `etc/alternatives/vim` with `unsquashfs` and `readlink`).

- [ ] **Step 2: Run them.**
Run: `TF_UBUNTU_IMAGE=thinfloe-ubuntu:slim test/integration/test-session-packages.sh`
Expected: FAIL on `vim-tiny` and on the alternative (the old build still has `vim`).

- [ ] **Step 3: Change the Dockerfile** as under Files.

- [ ] **Step 4: Rebuild and rerun.**
Run: `flock … make VERSION=slim rootfs`, then the test script.
Expected: `PASSED`; `ramroot.sqsh` smaller than after Task 2 (spec estimate about 15 MB; record the measured figure).

- [ ] **Step 5: CHANGES.md** under `### Changed`: `vim` on the machine is now the small `vim-tiny`: the command `vim` still opens it, without syntax colours and plugins. `joe` is unchanged.

- [ ] **Step 6: Commit** Dockerfile, test, CHANGES.md: `slim: vim-tiny instead of vim`, body with the `ramroot.sqsh` delta.

### Task 4: No ubuntu account

**Files:**
- Modify: `containers/ubuntu/Dockerfile:213-216` (the `adduser … tfuser` block) and its comment
- Test: `test/integration/test-session-packages.sh:308-323` (the account block; it already extracts `etc/passwd etc/shadow etc/sudoers home/tluser` into `$DPKG_TMP/acct`)
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: nothing.
- Produces: no `ubuntu` user, group, home, subuid or subgid line; `tfuser:x:1001:1001:`, member of `sudo` and `audio`.

- [ ] **Step 1: Change the test.** Extend the extract list with `etc/group etc/subuid etc/subgid`. Replace the "locked or absent" check with:
  - no line `^ubuntu:` in `etc/passwd`, `etc/shadow`, `etc/group`, `etc/subuid`, `etc/subgid` (one check per file, each its own `ok`/`FAIL` line);
  - no `home/ubuntu` entry in `$LIST`;
  - `^tfuser:x:1001:1001:[^:]*:/home/tfuser:` in `etc/passwd` (tightens today's `[0-9]*` gid);
  - `^tfuser:x:1001:` in `etc/group`;
  - `tfuser` in the member list of `^sudo:` and of `^audio:` in `etc/group`.
  Update the comment above the block: uid 1001 is fixed because field archives carry it, not because `ubuntu` holds 1000.

- [ ] **Step 2: Run it.**
Run: `TF_UBUNTU_IMAGE=thinfloe-ubuntu:slim test/integration/test-session-packages.sh`
Expected: FAIL on the `ubuntu` lines and on `home/ubuntu`.

- [ ] **Step 3: Change the Dockerfile.** Remove `ubuntu` with `userdel -r ubuntu` (removes the home and the group). Keep tfuser at 1001/1001, either by deleting `ubuntu` after tfuser exists or with explicit `--uid 1001` and a matching group (builder's call). If `userdel` leaves the `/etc/subuid` or `/etc/subgid` line, remove it there too.

- [ ] **Step 4: Rebuild and rerun.**
Run: `flock … make VERSION=slim rootfs`, then the test script and `test/integration/test-password.sh`, `test/integration/test-save-home.sh`, `test/integration/test-initramfs-home.sh`
Expected: all `PASSED`.

- [ ] **Step 5: CHANGES.md** under `### Changed`: The unused `ubuntu` login from the Ubuntu base image is gone. `tfuser` keeps its user ID.

- [ ] **Step 6: Commit** Dockerfile, test, CHANGES.md: `slim: remove the base image's ubuntu account`.

### Task 5: Measure, record, full gates

**Files:**
- Modify: `CHANGES.md`
- Modify: `CLAUDE.md` (the paragraph on the ESP size in "The first boot after an upgrade takes its kernel from tl.new" quotes a release size of 775 MB in 2026-10; add the new measured release size, keep the old figure as history)

**Interfaces:**
- Consumes: Tasks 1-4.
- Produces: the measured download size.

- [ ] **Step 1: Full gates.** Copy `/scratch/oetiker/claude-tmp/dbrrg-build/p2-gates.sh` to `slim-gates.sh` in the same directory, pointing at the worktree, with `VERSION=slim` on each make line and log names `slim-*`. Run it.
Expected: `slim-summary.log` shows rc=0 for `image test test-unit qemu-smoke qemu-smoke-netboot qemu-smoke-upgrade qemu-smoke-addons test-runtime`. The controller re-derives this from the logs; a subagent's word is not enough.

- [ ] **Step 2: Measure.** Sizes of `artifacts/rootfs/{vmlinuz,initrd.img,ramroot.sqsh}` and `artifacts/images/thinfloe-usb.img.zst`. The 772 MB baseline is the 2026-10-08 `dbrrg-usb.img.zst` (771768605 bytes); state that in the commit message.

- [ ] **Step 3: CHANGES.md** under `### Changed`, first entry of the section: The image download (`thinfloe-usb.img.zst`) is now <N> MB instead of 772 MB, and an upgrade downloads <M> MB less. <one sentence: the boot files no longer carry WiFi firmware, which the running system still loads>.

- [ ] **Step 4: Commit** CHANGES.md and CLAUDE.md: `slim: record the measured download size`.

- [ ] **Step 5: Report** to the controller: the per-task deltas, the final sizes, the gate summary, and that WiFi after the initrd cut is still to be confirmed on hardware.
