# ThinFloe image size reduction

Date: 2026-11-12. Status: approved by the owner 2026-11-12: the design and
the plan, a size budget of about 495 MB (final measured size plus 2 %), and
the removal of `radeon` as part of `O4` (`nouveau` stays: owner,
2026-11-13, see `O4`). The AMD test machine
for the hardware check is not yet named.

Step 9 (R9) of `docs/superpowers/specs/2026-11-12-github-release-flow-design.md`
and Task 14 of `docs/superpowers/plans/2026-11-12-github-release-flow.md`. The
bridge release (Task 15) is not dispatched before this work is on `main`.

Every decision is marked *owner* (decided by the owner) or *architect* (my
call, which the owner may still overturn).

## Intent

An upgrade downloads `vmlinuz`, `initrd.img` and `ramroot.sqsh`; a netboot
transfers the same three files. Today that is 611 MB: `ramroot.sqsh` 494 MB,
`initrd.img` 100 MB, `vmlinuz` 17 MB (4.2.0-era build of 2026-11-11 in
`~/scratch/dbrrg-artifacts/rootfs/`). The first public images on GitHub
should carry nothing ThinFloe does not use. The owner set the scope after a
size audit (raw files: `/scratch/oetiker/claude-tmp/thinfloe-tests/size-audit/`).
The target is about 460 MB (*owner*); see "Expected result" for what the
approved cuts reach by measurement.

## Scope

Approved cuts (*owner*, 2026-11-12). Nothing is added or dropped here.

- `S1`: kernel modules stored uncompressed in the squashfs, after dracut, then `depmod`.
- `S2`: `/usr/share/i18n` removed after `locale-gen`.
- `S3`: `/usr/lib/udev/hwdb.d` removed (`hwdb.bin` is prebuilt, `systemd-hwdb-update.service` is masked).
- `S4` (defect): the build's `./scripts`, bind-mounted at `/scripts`, is left out of the squashfs.
- `O1`: Intel WiFi firmware: per chip only the newest ucode the kernel loads.
- `O2`: initrd `--omit-drivers` for server NICs, FC/SAS HBAs, InfiniBand and network file systems.
- `O3`: `intel-ucode` removed from the rootfs after dracut built the early microcode archive.
- `O4`: DVB, TV tuner and radio modules removed from the squashfs.
- `O5`: initrd compressed with `zstd -19`.

Added by the owner on 2026-11-12, after the review (option B). It adds size:

- `A1`: AMD graphics firmware for APUs (integrated Radeon graphics of Ryzen
  processors), so `amdgpu` binds on AMD mini PCs. Today the image ships no
  AMD GPU firmware, and `amdgpu` fails on them.

Not in scope (*owner*): camera firmware, VA-API drivers, python3 (netplan
needs it), firmware for AMD graphics cards. Unpacked firmware in the squashfs measured larger and is not
proposed.

## Constraints

- `i915` and the firmware its modinfo names stay in the initrd; the boot
  command line keeps `plymouth.use-simpledrm`. Guard today:
  `test-initramfs-commands.sh` ("i915 in the initrd").
- Netboot and USB boot use the same `vmlinuz`, `initrd.img` and
  `ramroot.sqsh`. No cut may differ between the two.
- dracut runs with `--no-hostonly` (CLAUDE.md, standing constraint).
- Every new build input is a prerequisite of `.ubuntu-container`
  (CLAUDE.md, "Build"); guard `test-container-inputs.sh`.
- Every cut has a guard that fails the build or a test when the cut regresses
  or breaks something. Build-time guards run inside the Dockerfile step and
  stop `make rootfs`; artifact guards run in `make test`.

## Where the cuts happen

Three places, in this build order:

1. `containers/ubuntu/Dockerfile`, dracut `RUN`: `O2` and `O5` change the
   dracut flags.
2. `containers/ubuntu/Dockerfile`, a new `RUN` that is the last `RUN` of
   the file, after the cleanup `RUN` (`apt-get clean`, `apt-get autoremove`),
   and runs `containers/ubuntu/slim/slim-rootfs.sh`: `O4`, `S1`, `O1`, `O3`,
   `S2`, `S3`, in that order, with their build-time guards.
3. `scripts/export-rootfs.sh`, the `mksquashfs -e` list: `S4`.

### Why a separate step, and why last (*architect*)

`S1`, `O3` and `O4` must not reach the initrd: dracut copies modules as it
finds them, so uncompressed modules before dracut would grow the initrd
(the reason `S1` is "after dracut" in the scope), and the early microcode
archive is built from `/usr/lib/firmware/intel-ucode`. One step after the
dracut `RUN` makes the order visible in one place, and keeps the initrd
defined by the dracut flags alone.

It is the last `RUN`, after the cleanup `RUN`, and not directly after
dracut: any `dpkg` run after the slim step (the cleanup's `apt-get
autoremove` included) can fire the `update-initramfs` trigger, which would
run dracut again with its default flags on the slimmed tree, without
`--no-hostonly`, the omit lists or the thinfloe module, and with uncompressed
modules and no `intel-ucode`. Nothing after the slim step may run `dpkg`,
`apt-get` or dracut; the Dockerfile comment says so and the step refuses to
run when `/boot/initrd.img-<kver>` is missing.

The step lives in the Dockerfile and not in `export-rootfs.sh` for two
reasons: the container image that `make test` inspects
(`TF_UBUNTU_IMAGE`, used by `test-session-packages.sh`, `test-addon-deps.sh`,
`test-network-netplan.sh`) then matches the squashfs, and the Dockerfile and
its inputs are prerequisites of the rootfs artifacts while
`export-rootfs.sh` is not (see `S4`).

The scripts are copied in with `COPY containers/ubuntu/slim/ /thinfloe-slim/`
and removed in the same `RUN` that executes them. `SLIM_FILES` (a wildcard
over `containers/ubuntu/slim/*`) and the directory itself become
prerequisites of `.ubuntu-container`, like `PATCH_FILES`.

### Order inside `slim-rootfs.sh` (*architect*)

0. Preconditions: exactly one directory in `/usr/lib/modules` (`KVER`);
   `/boot/initrd.img-$KVER` and `/boot/System.map-$KVER` exist. Snapshot for
   the guards: the `depmod -n -e -F` warnings of the untouched tree, and the
   set of firmware names that some module's `modinfo -F firmware` lists and
   that exist under `/usr/lib/firmware` (with or without `.zst`).
1. `O4` first, while `modules.dep` still describes the shipped tree.
2. `S1`, then one `depmod`, which also covers `O4`.
3. `O1`, which reads `modinfo` of the iwlwifi modules, then `A1`, which
   prunes the AMD graphics firmware.
4. The firmware-name guard, now that modules and firmware are final. It
   skips the names that `O1` and `A1` dropped on purpose (they write their
   drop lists to `/thinfloe-slim/dropped`): `amdgpu` names the firmware of
   every AMD graphics card, and the dGPU files are meant to go.
5. `O3`, `S2`, `S3`: independent removals with their own checks.

## The cuts

Savings are for the downloaded files, in MB (10^6 bytes). "Measured" means
computed from the audit files or from the 2026-11-11 artifacts; "audit"
means the figure from the size audit.

### S1: uncompressed modules in the squashfs

- Where: `slim-rootfs.sh` step 2. `zstd -d -q --rm` on every
  `/usr/lib/modules/$KVER/**/*.ko.zst`, then
  `depmod -a -e -F /boot/System.map-$KVER $KVER`.
- Why this order: after dracut, or the initrd carries uncompressed modules
  and grows. squashfs compresses the plain `.ko` files in 1 MiB blocks across
  file boundaries, which beats per-file zstd.
- Saving: about 25 MB (audit).
- Guards:
  - Build: `depmod -e` warnings after the cuts must equal the snapshot from
    step 0 (any new "needs unknown symbol" line fails the build). Both lists
    are compared with `.ko.zst` rewritten to `.ko`, since the snapshot names
    the compressed files. No `.ko.zst` may remain.
  - `make test` (`test-image-slim.sh`): no `*.ko.zst` under
    `usr/lib/modules/` in the squashfs; every path in the squashfs's
    `modules.dep` (left and right of the colon) exists in the squashfs.
    `test-wifi-stack.sh` keeps working unchanged (its patterns match `.ko`).
  - `make test` (`test-initramfs-drivers.sh`): the initrd holds `.ko.zst`
    and no plain `.ko`, which fails if `S1` ever moves before dracut.
  - Boot: `scripts/check-boot-smoke.sh` gains an `unwant` line for
    `Unknown symbol`, a kernel message, so a `qemu-smoke*` target fails when
    the kernel refuses a module after switch-root. This check is weak: udev
    loads modules through libkmod, which logs no `modprobe: FATAL` line for
    a missing module, so a module that is gone fails silently at boot. The
    real guards are the `depmod -e` comparison and the `modules.dep` check
    above.

### S2: no locale sources

- Where: `slim-rootfs.sh` step 5, `rm -rf /usr/share/i18n`. `locale-gen` runs
  earlier in the Dockerfile and has written `/usr/lib/locale/locale-archive`.
- Saving: about 4 MB (audit; 15.7 MB uncompressed).
- Effect: `locale-gen` and `localedef` no longer work on a running machine.
  Nothing in `overlay/` or the menu calls them; the locales are fixed in
  `overlay/etc/locale.gen`.
- Guards:
  - Build: after the removal, `locale -a` lists every locale of
    `/etc/locale.gen` (`de_CH.utf8`, `de_DE.utf8`, `en_US.utf8`, `fr_CH.utf8`,
    `fr_FR.utf8`).
  - `make test` (`test-image-slim.sh`): no `usr/share/i18n/` and a
    `usr/lib/locale/locale-archive` in the squashfs.

### S3: no hwdb sources

- Where: `slim-rootfs.sh` step 5, `rm -rf /usr/lib/udev/hwdb.d`.
- Saving: about 1 MB (audit; 9.9 MB uncompressed).
- Effect: `systemd-hwdb update` would build an empty database. It never runs:
  `systemd-hwdb-update.service` is masked in the Dockerfile.
- Guards:
  - Build: `systemd-hwdb query usb:v046DpC52B` (a Logitech Unifying
    receiver) still prints properties after the removal.
  - `make test` (`test-image-slim.sh`): no `usr/lib/udev/hwdb.d/`;
    `usr/lib/udev/hwdb.bin` is present and over 1 MB;
    `etc/systemd/system/systemd-hwdb-update.service` links to `/dev/null`.

### S4: the build scripts stay out of the image (defect)

- Where: `scripts/export-rootfs.sh`, the `mksquashfs -e` list gains `scripts`.
  The Makefile bind-mounts `$(PWD)/scripts` at `/scripts` for the export, and
  the 4.2.0-era squashfs holds `/scripts/check-boot-smoke.sh`,
  `/scripts/publish.sh` and every QEMU smoke script.
- Also: `scripts/export-rootfs.sh` and `scripts/lib/common.sh` become
  prerequisites of `$(KERNEL) $(INITRD) $(SQUASHFS)` (*architect*). Without
  that, a changed export script leaves the artifacts up to date and the fix
  would not reach the next `make rootfs`.
- Saving: under 0.1 MB.
- Guards (`make test`, `test-image-slim.sh`): no `squashfs-root/scripts` and
  no `squashfs-root/thinfloe-slim` in the squashfs list;
  `test-container-inputs.sh` (or `test-usb-image-deps.sh`, whichever already
  drives `make -q` on the rootfs targets) shows a touched
  `scripts/export-rootfs.sh` makes `$(SQUASHFS)` out of date.

### O1: Intel WiFi firmware, newest loadable ucode per chip

The owner lifted the rule "never `rm` over `/usr/lib/firmware`" for this cut
(*owner*, 2026-11-12). The rule stays for everything else, and the docs say so.

- Where: `slim-rootfs.sh` step 3, with the selection in
  `containers/ubuntu/slim/iwlwifi_keep.py`.
- Saving: 61 MB of `.zst` files measured (111 of 174 files); squashfs barely
  recompresses `.zst`, so the download shrinks by about 55 to 61 MB (audit:
  40, up to 65).

How the driver picks a file: iwlwifi builds the name from the MAC and RF
type of the card it finds and tries API versions from its maximum down to
its minimum; the first file that exists is loaded. `modinfo -F firmware`
lists names with the maximum API, but not one name per file on disk
(`modinfo` names `iwlwifi-bz-a0-gf-a0-100.ucode`, the disk has
`iwlwifi-bz-b0-gf-a0-*`), and for some families the maximum is an unexpanded
macro (`iwlwifi-bz-a0-fm-b0-cIWL_BZ_UCODE_CORE_MAX.ucode`).

Rule (*architect*):

- Inputs: `modinfo -F firmware` of every module under
  `kernel/drivers/net/wireless/intel/` (iwlwifi, iwlmvm, iwldvm, iwlmld,
  iwl3945, iwl4965), and the files in `/usr/lib/firmware/intel/iwlwifi/`.
- A file name `iwlwifi-<stem>-<c?><N>.ucode[.zst]` belongs to the *series*
  (`stem`, `c` or nothing). The *family* of a series is the first dash token
  of the stem (`so`, `Qu`, `QuZ`, `bz`, `8265`), with the same `c` marker.
  The *radio* of a stem with four or more tokens is its second to last token
  (`jf` in `so-a0-jf-b0` and in `9000-pu-b0-jf-b0`, `gf4` in `so-a0-gf4-a0`).
- The *ceiling* of a series is the highest numeric API that `modinfo` names
  for the same family, radio and marker; when `modinfo` names no entry for
  that radio, the highest it names for the family and marker. An entry that
  is a macro makes that ceiling unresolved. The radio comes first because
  ceilings differ by radio within one family: `modinfo` names
  `so-a0-jf-b0-77` but `so-a0-gf-a0-100`, so a family ceiling of 100 would
  let a future `so-a0-jf-b0-83` displace the `-77` the driver loads.
- Per series: resolved ceiling: keep the one file with the highest `N` at or
  below it, drop the others. Unresolved ceiling: keep every file. Family not
  named by `modinfo` at all: keep the highest `N`. A series with no file at
  or below its ceiling keeps every file.
  With the 4.2.0-era files that last case is `sc-a0-wh-b0-101`, the only
  numeric file of its series, above the `sc` numeric ceiling 100.
- Files that do not match the pattern (`*.pnvm`, anything else) are kept.
- Symlinks under `/usr/lib/firmware` whose target was dropped
  (`/usr/lib/firmware/iwlwifi-*.ucode.zst -> intel/iwlwifi/...`) are removed.
- Fewer than 20 parsed `modinfo` names fails the build: a changed `modinfo`
  format must not prune blind.

Why `c`-series and numeric series are kept apart: `sc-a0-wh-b0` exists both
as `-101` and as `-c101` to `-c103`; the driver of this kernel names
`c`-names for some families and numeric ones for others, so each series
keeps its own best file. Why unresolved ceilings keep everything: keeping
`c103` where the kernel's maximum is `c101` would leave that card without
WiFi. With the 4.2.0-era files, three series stay whole (`bz-b0-fm-c0` c,
`bz-b0-wh-b0` c, `gl-c0-fm-c0` c, eight files).

Guards:

- `make test-unit` (`test/unit/test_iwlwifi_keep.py`, fixtures copied from
  the audit's `iwlmax.txt` and the 4.2.0-era file list): the cases in the
  plan, among them `so-a0-gf-a0-89` kept and `-86` dropped, `sc-a0-wh-b0-c101`
  kept and `-c102` dropped, every `bz-b0-fm-c0-c10x` kept, `3945-2` and
  `4965-2` kept, every `.pnvm` kept, a synthetic `so-a0-jf-b0-83` dropped in
  favour of `-77`, too few names refused. The 4.2.0-era totals (111 dropped,
  63 kept) are the same with and without the radio ceiling.
- Build: no dangling symlink under `/usr/lib/firmware` after the prune (the
  4.2.0-era squashfs has none among its 355 firmware links). The
  firmware-name guard (step 4): every firmware name from the step 0
  snapshot that a module still in the tree names must still exist.
- `make test` (`test-firmware.sh`), checked against the squashfs with its own
  parser and the host's `modinfo` on `iwlwifi.ko` extracted from the
  squashfs: at most one `.ucode` per series whose ceiling is resolved;
  none above its ceiling, with the same radio-then-family ceiling as the
  rule; no dangling firmware symlink; at least one
  ucode for each of the chips in the field or likely there: `7265D`, `8265`,
  `9260-th-b0-jf-b0`, `cc-a0`, `Qu-c0-hr-b0`, `QuZ-a0-hr-b0`, `so-a0-gf-a0`
  (with its `.pnvm`), `ty-a0-gf-a0`, `ma-b0-gf-a0`, `bz-b0-fm-c0`,
  `gl-c0-fm-c0`.
- Hardware (owner, at the end): WiFi associates on the NUC7 (8265) and the
  Alder Lake-P machine, and `journalctl -k | grep iwlwifi` shows
  `loaded firmware version`.

### O2: server and data-centre drivers out of the initrd

dracut's `systemd-networkd` module pulls in every NIC driver
(`kernel-network-modules`), and `kernel-modules` every SCSI driver. The
running system keeps all of them; only the initrd loses them.

- Where: Dockerfile, dracut `--omit-drivers`. dracut turns each entry into
  `^name$` and matches it against module names, `-` read as `_`.
- Names added (*architect*: the module names behind the owner's list,
  plus the libraries only those modules use, as measured in the 2026-11-11
  initrd):
  - Server NICs: `mlx4_core mlx4_en mlx5_core mlx5_dpll ice i40e qed qede
    bnx2x bnxt_en cxgb cxgb3 cxgb4 cxgb4vf ch_ipsec ch_ktls chcr libcxgb sfc
    sfc_falcon sfc_siena nfp`. `chcr` (Chelsio crypto) needs `cxgb4`.
  - FC and SAS HBAs: `lpfc qla2xxx bfa efct fnic csiostor qedf bnx2fc fcoe
    libfcoe libfc mptfc mptsas mpt3sas mpi3mr pm80xx isci mvsas aic94xx
    libsas megaraid_sas smartpqi hpsa esas2r`.
  - InfiniBand: `ib_core ib_cm iw_cm rdma_cm rtrs_client rtrs_core ib_srp
    rnbd_client`. `rnbd_client` (block device over RDMA) needs every other
    module of this group.
  - Network file systems: `cifs cifs_md4 ksmbd ceph libceph rbd kafs rxrpc
    sunrpc grace nfs_acl`. `rbd` is the Ceph block device and needs
    `libceph`; `nfs_acl` needs `sunrpc`, `grace` serves only NFS.
  - Not omitted: `ubifs`. It is a flash file system, not a network file
    system, so it is outside the owner's categories (*architect*); it stays
    in the initrd.
  - Kept on purpose: `mlxfw` (needed by `mlxsw_core`), the SCSI transport
    classes (`scsi_transport_fc` is needed by `hv_storvsc`), `cnic`, every
    desktop NIC (`e1000 e1000e igb igc r8169 atlantic tg3`). `virtio_net`,
    `squashfs`, `loop`, `ext4` and `vfat` are built into this kernel.
- Saving: 11.7 MB of these modules in the 2026-11-11 initrd (audit: 13 MB);
  they are `.zst` files, so the compressed initrd shrinks by about as much.
- Guards (`make test`, new `test-initramfs-drivers.sh`):
  - None of the omitted modules is in the initrd.
  - `e1000 e1000e igb igc r8169 atlantic tg3 i915 overlay zram` are (the
    module files in the 2026-11-11 initrd).
  - Every module named in `modinfo -F depends` of every module in the
    initrd is itself in the initrd (host `modinfo` on the files
    `unmkinitramfs` extracts). This catches an omitted library that a kept
    module needs.
  - Boot: `make qemu-smoke-netboot` and `make qemu-smoke-timezone-netboot`
    bring the network up in the initrd on `virtio_net`; `make qemu-smoke`
    boots from a disk through the initrd.

### O3: microcode only in the initrd

- Where: `slim-rootfs.sh` step 5, `rm -rf /usr/lib/firmware/intel-ucode`.
- Why after dracut: dracut builds the early microcode archive
  (`kernel/x86/microcode/GenuineIntel.bin`, 21 MB uncompressed in the
  2026-11-11 initrd) from that directory. The kernel applies microcode only
  from the early archive at boot; nothing reads the directory after
  switch-root.
- Saving: about 14 MB (audit), possibly more: the files are 21.4 MB
  uncompressed and microcode compresses poorly.
- Guards:
  - Build: before the removal,
    `3cpio --list /boot/initrd.img-$KVER >"$tmp" && grep -qx kernel/x86/microcode/GenuineIntel.bin "$tmp"`
    succeeds; otherwise the build fails. The image has `3cpio`, not `cpio`.
    The list goes to a file first: `grep -q` in a pipe can end the lister
    with SIGPIPE, which `pipefail` reports as a failure.
  - `make test` (`test-firmware.sh`): the check "Intel CPU microcode" in the
    squashfs becomes "no `intel-ucode` in the squashfs" plus
    "`kernel/x86/microcode/GenuineIntel.bin` in the initrd, over 1 MB".
  - Hardware (owner): `journalctl -k | grep microcode` on an Intel machine
    shows the early update or the current revision. QEMU cannot show it: a
    guest does not load microcode.

### O4: no DVB, TV tuner or radio modules, no `radeon`

- Where: `slim-rootfs.sh` step 1.
- Rule (*architect*): seed = every module under
  `kernel/drivers/media/{dvb-frontends,tuners,radio,dvb-core}/` and
  `kernel/drivers/media/usb/{dvb-usb,dvb-usb-v2}/`. Cut set = the seed plus
  every module under `kernel/drivers/media/` or `kernel/drivers/staging/media/`
  that depends, directly or through others (`modules.dep`), on a seed module:
  the DVB parts of capture drivers (`cx88-dvb`, `saa7134-dvb`, `em28xx-dvb`)
  and DVB-only cards (`ttpci`, `ddbridge`, `mantis`). The cores and analog
  parts (`cx88xx`, `cx8800`, `saa7134`, `em28xx`, `em28xx-v4l`) do not depend
  on a seed module and stay. Any module a kept module depends on is put back,
  so nothing outside the media tree loses a dependency: `snd-fm801` and
  `snd-es1968` (sound cards with an FM tuner) keep `tea575x` from
  `drivers/media/radio/`.
- Effect on analog capture: those drivers attach their tuner chips through
  `dvb_attach`/`symbol_request`, which `modules.dep` does not record. The
  analog capture drivers stay, but lose their tuners without an error at
  build time. That is within scope: TV tuners are what `O4` removes.
- Also removed (*owner*, 2026-11-12): every module under
  `kernel/drivers/gpu/drm/radeon/` (one module, `radeon`). On SI, CIK and
  r600 and later it fails without firmware after it has removed the firmware
  framebuffer, and the image ships none (`A1` removes the package's
  `radeon/` directory), so without it those machines keep `simpledrm`. No
  other module in `modules.dep` of 7.0.0-43 depends on it. Its
  `--omit-drivers` entry in the initrd stays.
- `nouveau` stays (*owner*, 2026-11-13, correcting the 2026-11-12 decision,
  which assumed it needs firmware): it sets the mode without any firmware on
  NVIDIA GPUs up to Volta; firmware is needed only for acceleration and
  reclocking. Without it an NVIDIA machine waits the full 20 s of `wait-kms`
  and runs on `simpledrm`, one output at the firmware's mode. It binds after
  switch-root, like `amdgpu`, and stays in `--omit-drivers`, out of the
  initrd. No NVIDIA firmware package is added. The `nouveau` directory
  (`nouveau`, `ch7006`, `sil164`) costs about 1.3 MB of the squashfs
  (6.6 MB uncompressed).
- Kept (*owner*): `uvcvideo`, InfiniBand, comedi, IIO, staging, CAN and DSA,
  MTD, server NICs and HBAs, WiFi drivers without firmware, `amdgpu` (for
  `A1`). The rule touches none of them.
- Saving: 6.0 MB measured for the media part: 218 modules in the seed
  directories, 260 after the closure, 1 put back (`tea575x`), 259 removed;
  2.0 MB for `radeon` and `nouveau` (4 modules, as `.ko.zst`); 8.0 MB in all,
  before `nouveau` came back.
  The audit's 45 MB covered the broader list before the owner's keep
  exceptions; see "Expected result".
- Guards:
  - Build: the `depmod -e` comparison of `S1`.
  - `make test` (`test-image-slim.sh`): no module under the seed
    directories except `tea575x`; no module under `gpu/drm/radeon/` or
    present: `nouveau`, `uvcvideo`, `ib_core`, `comedi`,
    `industrialio`, `can`, `dsa_core`, `mtd`, `mlx5_core`, `amdgpu`,
    `iwlwifi`, `snd-fm801`.

### O5: initrd at zstd level 19

- Where: Dockerfile, dracut `--compress "zstd -19 -T4"` (was `-3 -T4`).
- Why 19 (*owner*): zstd decompresses at the same speed at every level. Not
  `--ultra -22`: its 128 MiB window costs memory in the kernel's initramfs
  decompressor; level 19 uses an 8 MiB window.
- Saving: likely 4 to 7 MB (review estimate; audit: 8 to 13). Most of the
  initrd is `.ko.zst` and firmware that do not compress again, and the early
  microcode archive is never compressed.
- Guards: the size budget (below) and every `qemu-smoke*` target, which
  unpack the initrd.

### A1: AMD graphics firmware for APUs

- Where: `containers/ubuntu/Dockerfile` installs `linux-firmware-amd-graphics`
  in the package list next to `linux-firmware-intel-graphics` (in the build
  image: `20260319.git217ca6e4-0ubuntu3.1`, 28.8 MB installed, files already
  `.zst`). `slim-rootfs.sh` step 3 prunes it, with the selection in
  `containers/ubuntu/slim/amdgpu_keep.py` and the APU table in
  `containers/ubuntu/slim/amdgpu-apu-ip.csv`.
- `amdgpu` stays in the squashfs and stays out of the initrd: it is in
  `--omit-drivers` today and remains there. Plymouth runs on `simpledrm`
  (`plymouth.use-simpledrm`) until `amdgpu` binds after switch-root and
  replaces it, and `thinfloe-wait-kms.service` waits for that KMS driver
  (`docs/notes/session.md`). When the firmware a card needs is missing:
  amdgpu fails; the boot waits about 20 s, and then either the menu runs unaccelerated on simpledrm or, worst case, the failure screen appears after about 40 s. The outcome depends on where the load fails:
  `amdgpu_device_init` removes `simpledrm` after `ip_early_init`; the gfx,
  SDMA and PSP microcode load in early init, so a missing file there most
  likely leaves `simpledrm` in place, while the display firmware (DMCUB)
  loads in the display block's software init after `simpledrm` is gone, so
  no `card0` remains, labwc fails and retries, and the failure screen
  follows (`docs/notes/session.md`). `wait-kms` itself treats `simpledrm` or
  no driver as "keep waiting" and gives up with exit 0 after 20 s. dracut installs
  firmware only for modules it installs, so no AMD firmware reaches the
  initrd either.
- Which APUs (*owner*): Raven and Picasso, Renoir, Lucienne, Cezanne
  (`green_sardine`), Barcelo, Rembrandt (`yellow_carp`), Mendocino, Phoenix,
  Hawk Point, Strix, Krackan and newer. *Architect*: every APU row of the
  table from Raven on, so also Raphael, Granite Ridge and Dragon Range (the
  graphics in Ryzen 7000 and 9000 desktop processors), Van Gogh (Steam Deck),
  Strix Halo and Gorgon Point. *Owner*, 2026-11-13: also Carrizo and Stoney
  (2015, before Raven; HP t530 and t630 thin clients), which amdgpu drives by
  default and which load firmware by chip name. The older APUs the `radeon`
  driver runs (Kaveri and before) are left out; the `radeon/` directory of
  the package is removed whole.

Source of the rule: the kernel's own APU table,
`Documentation/gpu/amdgpu/apu-asic-info-table.csv` at tag `v7.0` of
torvalds/linux (fetched 2026-11-12; the image runs 7.0.0-43). Each row gives
the DCN, GC, VCN, SDMA and MP0 (PSP) IP versions of one APU. amdgpu loads
firmware in two naming schemes, which `modinfo -F firmware amdgpu` (692 names
in 7.0.0-43) confirms:

- Older APUs by chip name: `carrizo_*`, `stoney_*`, `raven_*`, `raven2_*`,
  `picasso_*`, `renoir_*`, `green_sardine_*`, `yellow_carp_*`, `vangogh_*`.
  The CARRIZO/STONEY row of the table names no IP version (DCE 11, GC 8,
  VCE 3 / UVD 6), so its chips are kept by name only.
- Newer APUs by IP version, found through IP discovery: `gc_X_Y_Z_*`,
  `psp_X_Y_Z_*`, `dcn_X_Y_Z_*`, `vcn_X_Y_Z*`, `sdma_X_Y_Z`. A version with
  patch level 0 may be named with two parts: DCN 3.5.0 loads
  `dcn_3_5_dmcub`, DCN 3.6.0 loads `dcn_3_6_dmcub`.
- IP blocks that only APUs have: `vpe_*`, `umsch_mm_*`, `isp_*`.

Rule (*architect*):

- Keep a file of `/usr/lib/firmware/amdgpu/` when its name starts with one of
  the chip prefixes, or with an IP prefix from the table (`<block>_X_Y_Z`
  followed by `_` or the end, or `<block>_X_Y` followed by `_` and a letter
  when Z is 0; MP0 maps to `psp`), or with an APU-only block prefix.
- Keep the target of every kept symlink, transitively. IP versions are
  shared: `vcn_4_0_2` (Phoenix) is a link to `vcn_4_0_0`, the file of a
  graphics card, and stays with it.
- Drop everything else in `amdgpu/`, and `radeon/` whole. Remove symlinks
  that dangle afterwards; none may remain.
- The table must have at least 15 APU rows and the selection at least 100
  files, else the build fails: a moved or reformatted table must not prune
  blind.
- New-APU guard (*architect*). Both kernel tables are copied into
  `containers/ubuntu/slim/`: `amdgpu-apu-ip.csv` and `amdgpu-dgpu-ip.csv`
  (`apu-asic-info-table.csv` and `dgpu-asic-info-table.csv`, torvalds/linux
  `v7.0`). At build time every `gc`, `psp`, `dcn`, `vcn` and `sdma` version
  that `modinfo -F firmware amdgpu` names is checked: a version that has
  files in the installed package and is in neither table nor in the
  explicit list `DGPU_EXTRAS` fails the build. `DGPU_EXTRAS` today:
  `gc_9_4_4`, `gc_9_5_0`, `psp_13_0_12`, `psp_13_0_14`, `sdma_4_4_4`,
  `sdma_4_4_5`, `vcn_5_0_1` (the MI300 to MI350 accelerators, missing from
  the dGPU table). The build also fails when a `psp_X_Y_Z` has a `toc` file
  but no `sos` file and is not kept: that is the APU pattern (an APU's PSP
  boots from the BIOS, so the package ships no `sos` for it). A kernel
  version check would not do: Ubuntu backports support for new APUs into
  7.0.0-NN updates without changing the version's first two numbers.
  `modinfo` of 7.0.0-43 already names `gc_11_5_4`, `sdma_6_1_4`,
  `psp_15_0_0`, `psp_15_0_8` and `vcn_5_3_0` (probably the next APUs) with
  no files in today's package, so the guard fires as soon as
  linux-firmware ships them, and the tables are then refreshed.

Measured on the package (`/scratch/oetiker/claude-tmp/thinfloe-tests/size-design/amd/`):
the package holds 27.3 MB in `amdgpu/` (667 names, 122 of them links) and
1.6 MB in `radeon/`; the rule keeps 178 names with 8.1 MB of files and drops
489 names (19.1 MB) and `radeon/`. Every one of the 177 names that
`modinfo` lists under a kept prefix is in the package. Since the files are
`.zst`, the squashfs grows by about 8 MB; the whole package would have cost
about 29 MB. With Carrizo and Stoney (2026-11-13) the rule keeps 196 names
and drops 471; the 18 more names (16 files, 0.6 MB, and 2 links) are all that
`modinfo` lists for the two chips, so 195 names under a kept prefix are
checked.

Guards:

- `make test-unit` (`test/unit/test_amdgpu_keep.py`, fixtures: the package's
  name and link list and the `modinfo` list of 7.0.0-43, copied to
  `test/unit/amdgpu-fixtures/`, plus the CSV): `dcn_3_5_dmcub`,
  `dcn_3_6_dmcub`, `gc_11_5_0_mes_2`, `psp_14_0_0_toc`, `raven_dmcu`,
  `green_sardine_vcn`, `vangogh_toc`, `isp_4_1_1` kept; `vcn_4_0_0` kept
  through the link from `vcn_4_0_2`; `navi10_*`, `gc_11_0_0_*`,
  `psp_13_0_0_*`, `smu_13_0_0`, `polaris10_*`, `kaveri_*` dropped; every
  `carrizo_*` and `stoney_*` name `modinfo` lists kept; totals 196 kept, 471
  dropped; a table with fewer than 15 rows refused; the
  new-APU guard refuses a fixture with a synthetic `gc_11_5_4_me.bin.zst`
  and a fixture with a `psp` version that has `toc` but no `sos` and is not
  kept.
- Build: after the prune, every name `modinfo -F firmware amdgpu` lists
  under a kept prefix exists; no dangling symlink under `/usr/lib/firmware`.
- `make test` (`test-firmware.sh`), a pinned list written out in the test,
  independent of the rule, one check per APU: Carrizo (`carrizo_me`),
  Stoney (`stoney_me`), Raven (`raven_dmcu`), Picasso
  (`picasso_vcn`), Renoir (`renoir_dmcub`), Cezanne and Barcelo
  (`green_sardine_dmcub`), Rembrandt (`yellow_carp_dmcub`), Raphael
  (`gc_10_3_6_me`, `dcn_3_1_5_dmcub`), Mendocino (`gc_10_3_7_me`,
  `dcn_3_1_6_dmcub`), Phoenix (`gc_11_0_1_mes_2`, `dcn_3_1_4_dmcub`,
  `psp_13_0_4_toc`, `vcn_4_0_2`), Hawk Point (`gc_11_0_4_me`,
  `psp_13_0_11_toc`), Strix (`gc_11_5_0_me`, `dcn_3_5_dmcub`,
  `psp_14_0_0_toc`), Strix Halo (`gc_11_5_1_me`, `dcn_3_5_1_dmcub`,
  `vcn_4_0_6_1`),
  Krackan (`gc_11_5_2_me`, `gc_11_5_3_me`, `dcn_3_6_dmcub`,
  `psp_14_0_4_toc`, `psp_14_0_5_toc`); and no `navi10_` file, no
  `usr/lib/firmware/radeon/`.
- `make test` (`test-initramfs-drivers.sh`): no `amdgpu.ko` and no
  `usr/lib/firmware/amdgpu/` in the initrd.
- Hardware (owner, at the end): QEMU cannot run `amdgpu`. On a real AMD mini
  PC (the owner names the machine), the menu appears, and
  `journalctl -k | grep amdgpu` shows the driver initialised with no
  `failed to load firmware` line.

## Expected result

| Cut | Saving (MB) | Source |
|---|---:|---|
| S1 | 25 | audit |
| S2 | 4 | audit |
| S3 | 1 | audit |
| S4 | 0 | measured |
| O1 | 55 to 61 | measured |
| O2 | 11.7 | measured |
| O3 | 14 or more | audit |
| O4 | about 6.7 | measured (8.0 before `nouveau` came back) |
| O5 | 4 to 7 | review estimate |
| A1 | about -8.6 (adds) | measured (with Carrizo and Stoney) |
| Total | 115 to 125 | |

611 MB minus that is about 486 to 496 MB; without `A1` it would be 478 to
488 MB. The owner accepted a budget of about 495 MB. The target of about 460 MB
assumed 45 MB for `O4`; with the owner's keep exceptions `O4` measures
8 MB, while `O1` measures about 20 MB more than the audit's 40, and `O5`
gives less than the audit expected, and `A1` adds about 8 MB. The approved
work therefore ends about 30 to 40 MB above the target. This design does not add a
cut to close the gap; the measured result goes to the owner with the final
task.

## Measurement

- Unit: MB = 10^6 bytes; sizes from `stat -c %s` of `vmlinuz`, `initrd.img`,
  `ramroot.sqsh` in `artifacts/rootfs/`. The download is their sum.
- Baseline: the build pulls `ubuntu:26.04` and runs `apt-get upgrade`, so an
  old artifact differs by package drift. Task 1 builds the branch before any
  cut and copies the three files and the `version` file to
  `/scratch/oetiker/claude-tmp/thinfloe-tests/size-baseline/`. Every later
  number compares against that copy.
- Attribution: each task that changes the image rebuilds with
  `make rootfs` and records the three sizes and the delta to the previous
  task in its commit message. The deltas add up to the total.
- Tool: `test/integration/test-image-size.sh [rootfs-dir [baseline-dir]]`
  prints the three sizes and their sum, and from `unsquashfs -lls` and
  `lsinitramfs -l` the uncompressed bytes of: modules, firmware,
  `intel/iwlwifi`, `intel-ucode`, `usr/share/i18n`, `udev/hwdb.d`,
  `/scripts`, initrd modules and the early microcode archive. With a
  baseline directory it prints each delta. It fails when a budget is
  exceeded.
- Budget: `ramroot.sqsh`, `initrd.img` and the sum each get a ceiling, set in
  the last task to the measured size of the finished branch plus 2 %,
  rounded up to a whole MB. Raising a ceiling is a deliberate edit with a
  reason in the commit message. The test is part of `make test`.

## Boot coverage

- `make qemu-smoke`: kernel and initrd (`O2`, `O5`), squashfs mount, modules
  loading from the squashfs after switch-root (`S1`, `O4`), systemd to
  `multi-user.target`. With the new `unwant` lines, a module that fails to
  load fails it.
- `make qemu-smoke-netboot`, `make qemu-smoke-timezone-netboot`: networking
  in the initrd (`O2`).
- `make qemu-smoke-upgrade`, `make qemu-smoke-two-sticks`,
  `make qemu-smoke-grow`, `make qemu-smoke-write`: the USB image through
  OVMF and syslinux with the new files.
- `make qemu-smoke-addons`: `dpkg` installs a package at boot on the slimmed
  tree (`S2`, `S3`).
- Not covered by QEMU: Intel WiFi (`O1`), early microcode (`O3`), `i915` on
  real hardware, `amdgpu` (`A1`). These are the owner's hardware checks in
  the last task.

## Docs to change

- `CLAUDE.md`, "Firmware is selected by package, never by cleanup": the rule
  stays, with one exception: `containers/ubuntu/slim/slim-rootfs.sh` prunes
  Intel WiFi firmware to the file the driver loads, keeps AMD graphics
  firmware only for APUs, and removes `intel-ucode` after dracut. Guards:
  `test-firmware.sh`, `test_iwlwifi_keep.py`, `test_amdgpu_keep.py`. The
  package list names `linux-firmware-amd-graphics`, and the APU table in
  `containers/ubuntu/slim/amdgpu-apu-ip.csv` is refreshed from the kernel's
  `Documentation/gpu/amdgpu/apu-asic-info-table.csv` when the kernel
  changes.
- `docs/notes/history-of-constraints.md`: the same exception, with the date
  and the owner's decision.
- `CLAUDE.md`, "Build": `SLIM_FILES` joins the list of prerequisites; nothing
  after `slim-rootfs.sh` may run `dpkg`, `apt-get` or dracut; `scripts/export-rootfs.sh` is a
  prerequisite of the rootfs artifacts.
- The Dockerfile comment "Never re-introduce a find/rm sweep over
  /usr/lib/firmware" names the exception.

## Changelog

Under `[Unreleased]` in `CHANGES.md`:

`### New`:

- ThinFloe now shows its picture on thin clients and mini PCs with AMD
  processors and their built-in Radeon graphics (the A-series with Carrizo
  and Stoney graphics, for example HP t530 and t630, and Ryzen 2000 series
  and newer, up to Ryzen AI): the image carries the graphics firmware for
  these processors. AMD graphics cards still get no firmware.

`### Changed`:

- An upgrade or a netboot now downloads about N MB instead of 611 MB. The
  running system keeps Intel WiFi firmware only in the version its driver
  loads, and has no drivers for DVB, TV tuner and radio cards, none for
  older AMD (`radeon`) graphics, and no CPU
  microcode files, which are applied from `initrd.img` at boot as before.
  During boot, before the system is loaded, drivers for data-centre network
  cards, Fibre Channel and SAS storage controllers, InfiniBand and network
  file systems (CIFS, Ceph, AFS, NFS) are no longer available.
- `locale-gen` and `systemd-hwdb update` no longer work on a running machine:
  the locale and hardware database sources are left out. The locales of
  `/etc/locale.gen` and the hardware database ship ready to use.

`### Fixed`:

- The image no longer contains the build and test scripts in `/scripts`.

`N` is the measured sum from the last task.

## Risks

1. **WiFi on a chip the guards do not name.** `O1` relies on the driver
   trying API versions downwards within one series, and on the family
   ceiling from `modinfo`. A kernel whose driver requests a series under a
   family ceiling that `modinfo` states differently would find no file.
   Mitigations: unresolved ceilings keep everything, `c`-series and numeric
   series are kept apart, the field-chip list in `test-firmware.sh`, and the
   owner's hardware check. QEMU cannot test it.
2. **A kernel or linux-firmware update changes names.** New families arrive
   with every kernel. The rule keeps the highest file of any family
   `modinfo` does not name, the build fails below 20 parsed names, and
   `test_iwlwifi_keep.py` pins the behaviour on the 4.2.0-era data.
3. **The target is missed by about 30 to 40 MB**, of which 8 MB are `A1` (see "Expected result"). Only the
   owner can change the scope.
4. **An add-on package that triggers `systemd-hwdb update` or `locale-gen`**
   at boot. The `udev` trigger fires only for a package that installs into
   `/usr/lib/udev/hwdb.d`; ThinLinc and OxuLnk do not. An empty `hwdb.bin`
   would lose device names and some input quirks until the next boot, since
   the change lives in the ZRAM overlay. `make qemu-smoke-addons` installs a
   package at boot.
5. **A machine that boots from an HBA or over a network file system.** None
   does: ThinFloe boots from a USB stick or over HTTP. The running system
   keeps every driver.
6. **Conflicts with the release-flow plan.** Task 4 of that plan also edits
   the prerequisites of `.ubuntu-container` (removes `RELEASE`) and of
   `$(KERNEL) $(INITRD) $(SQUASHFS)` (adds `.version-kind`), changes
   `scripts/export-rootfs.sh` (the version export, and a new version write)
   and `test-container-inputs.sh`, and adds `TF_VERSION`. Whichever lands
   second merges both; Task 2 of this plan has the step.
7. **An APU the table does not list yet, or a kernel update that moves an
   APU to new IP versions.** Its firmware would be dropped: amdgpu fails; the boot waits about 20 s, and then either the menu runs unaccelerated on simpledrm or, worst case, the failure screen appears after about 40 s.
   The new-APU guard fails the build when the installed package ships files
   for an IP version neither table knows, or a `psp` with the APU pattern
   that the rule does not keep; then both tables are refreshed from the
   kernel. The pinned list in `test-firmware.sh` and the owner's hardware
   check cover the known APUs.
