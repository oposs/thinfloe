# USB stick automount

Date: 2026-11-24, revised 2026-11-25 after a pushback review. Status: design,
awaiting owner approval.

## Intent

A user plugs a USB stick, an SD card (in a USB card reader or a built-in SD
slot) or a USB disk into a ThinFloe machine and can use its files at once, in
the place a normal Ubuntu desktop puts them: `/media/tfuser/<label>`.
Remote-desktop clients then find them without ThinFloe-specific setup: OxuLnk
can share `/media/tfuser` as one folder (OxuLnk already shares folders
explicitly; a default share of `/media/tfuser` is OxuLnk's work, not part of
this design), and ThinLinc's local drive redirection can export the same path.

Today nothing mounts such a device: the kernel creates `/dev/sdX1` and that is
all. The image has no udisks2, no polkit, no file manager.

## Goals

- Every partition with a filesystem the kernel can read, on every removable or
  USB block device and on cards in a built-in SD slot, is mounted read-write
  at `/media/tfuser/<name>`. Several sticks and multi-slot card readers at
  once. FAT, exFAT and NTFS mounts belong to tfuser; Linux filesystems (ext4,
  btrfs, xfs) keep the owners stored on them, so a stick formatted on another
  Linux machine may not be writable for tfuser.
- `/run/media/tfuser` is a link to `/media/tfuser`, for clients that look in the
  upstream udisks2 place.
- The user sees a short OSD note when a stick is connected and when it is
  removed; a removal without eject gets a warning note ("files may be lost").
- An **Eject** tile in the menu, present only while at least one stick is
  mounted, ejects one stick or all, and says when it is safe to pull them.
- Restart and power off from the menu write all pending data to the sticks
  first and say so; a stick still in use is reported like a failed home save.
- Making a ThinFloe stick on the Upgrade screen keeps working: the writer
  unmounts our automounts of the target itself, and the erase confirmation
  lists the files on the stick, so the user sees what is about to be erased.

## Non-goals

- No restriction system: no read-only mode, no switch to turn USB storage off,
  no admin policy. ThinFloe has no restriction system yet; this is not where it
  starts.
- No file manager, no file browsing in the menu.
- No udisks2, no polkit.
- Not OxuLnk's or ThinLinc's share configuration.
- No automount of internal disks (SATA, NVMe, eMMC), optical drives, or MTP phones.

## Anti-goals (a failure even if it works)

- The boot stick, or any of its partitions, ever mounted by the automounter.
- A stick being written by `thinfloe-upgrade` mounted halfway through the write.
- A slow or broken stick delaying the boot, the login, the menu, another
  stick or the Upgrade screen.
- A mount landing anywhere but a directory the helper itself just made under
  `/media/tfuser` (above all over `/home/tfuser`).
- The OSD taking a click or focus (standing rule: it never does). The note has
  no eject icon or button.
- A false "removed without eject" warning (during a write, at login, at
  shutdown).
- A crash or a hung mount leaving a mount point the next stick collides with.

## Constraints from the project

- Boot stick identity: the ESP the initramfs chose is in
  `/run/thinfloe-esp-device` and mounted at `/run/thinfloe/storage/efi`; the
  whole disk holding it is excluded. Chosen by stick ID, never by partlabel
  (standing constraint).
- No polkit: privileged actions from the session go through `sudo -n` (tfuser
  has NOPASSWD), as reboot and `thinfloe-upgrade` do. `/usr/libexec` is not in
  sudo's search path: callers use the full path.
- udev's own `RUN` cannot mount (systemd-udevd runs in a private mount
  namespace); the rule starts a systemd unit via `SYSTEMD_WANTS`, as
  `90-thinfloe-peripheral-battery.rules` does.
- A unit started after basic.target never declares `Before=` on a
  sysinit.target unit; no `Before=getty@tty1`.
- New overlay files and any new package go through the `Makefile` prerequisite
  lists (`OVERLAY_DIRS` etc.) and the size budget in `test-image-size.sh`.
- Package selection, not cleanup: if a filesystem tool is needed it is added to
  the Dockerfile package list.
- `X-TF-Action` values like `settings` and `upgrade` are honoured only in
  shipped tile files; `eject` follows the same rule.
- Contract files under `/run/thinfloe/` are written atomically, one line per
  item, tab-separated; readers ignore unknown trailing fields (as the battery
  and Deskflow state files).
- The menu signals a running `thinfloe-upgrade write` with SIGTERM only;
  `thinfloe-upgrade` children run in their own session (`SPAWN`).
- `thinfloe-upgrade drives` holds `upgrade.lock` while it scans and the menu
  runs it every 2 s (`upgrade.rs`): nothing slow or device-dependent may be
  added to that scan.
- thinfloe-menu saves before it exits 0, 10 or 11 (standing constraint); the
  stick flush before 10 and 11 sits beside that save, it does not replace or
  reorder it.

## Approaches considered

1. **udisks2 + polkit.** The standard desktop stack; paths come for free; the
   menu would talk D-Bus. Rejected: adds polkit, which the image avoids, a
   second permission system beside sudo, and udisks2's dependency tree
   (libblockdev plugins) against the size budget.
2. **Own root helper started by udev (chosen by the owner).** A udev rule starts
   a template unit per partition; a small helper mounts, writes a contract
   file, and ejects on request through `sudo -n`. Fits the existing
   `peripheral-battery` pattern (udev rule + units + `/run/thinfloe` file),
   no polkit, almost no new packages.
3. **`systemd-mount` from udev.** Least own code, but defaults to
   `/run/media/system/<label>` and still needs our own path, owner and
   boot-stick logic and our own state file: option 2 with less control.

## Delivery order

Part A (automount, eject, the writer's release and hold, the flush before
restart and power off) ships as one piece; it is useful alone. Part B (OSD
notes) and part C (file list on the erase confirmation) build on A and come
after it in the plan, in that order. All three are in this round.

## The design

### Parts

- `overlay/etc/udev/rules.d/90-thinfloe-automount.rules`: for
  `SUBSYSTEM=="block"`, `ENV{DEVTYPE}=="partition"` (and whole disks with a
  filesystem and no partition table, as many SD cards and some sticks are),
  where the disk is removable, or `ID_BUS=="usb"`, or an `mmcblk*` disk whose
  card reports `ATTRS{type}=="SD"` (a built-in SD slot; internal eMMC reports
  `MMC` and is never matched), and `ENV{ID_FS_USAGE}=="filesystem"`:
  `TAG+="systemd"`, `ENV{SYSTEMD_WANTS}+="thinfloe-automount@%k.service"`.
  The unit is bound to the device (`BindsTo=dev-%i.device`,
  `After=dev-%i.device`), so a removal stops it. A medium change in a card
  reader (`change` event, `ID_FS_TYPE` gone or new) must start or stop the
  unit as well; the builder verifies this on QEMU and with a real reader.
- `overlay/etc/systemd/system/thinfloe-automount@.service`: `Type=oneshot`,
  `RemainAfterExit=yes`, `ExecStart=/usr/libexec/thinfloe/thinfloe-automount
  add %i`, `ExecStop=… remove %i`, `TimeoutStartSec=30` so a hung mount gives
  up, and an explicit `TimeoutStopSec` (about 30 s; the real flush happens in
  the menu before restart and power off, see below). Not enabled; started
  only by udev, at boot too (udev trigger replays devices plugged in before
  boot).
- `overlay/usr/libexec/thinfloe/thinfloe-automount` (root; language the
  builder's call, Python like `peripheral-battery` or shell). Commands below.
- `/media/tfuser` (0755, **root**) and the link `/run/media/tfuser ->
  /media/tfuser`, made by a `tmpfiles.d` entry. tfuser cannot create, rename
  or remove entries in `/media/tfuser`, only inside a mounted stick; udisks2
  does the same.
- sudoers: nothing new; tfuser's NOPASSWD covers `sudo -n
  /usr/libexec/thinfloe/thinfloe-automount eject …`.

### Helper commands

**Locks.** No lock is held across a mount, a umount or a sync:

- one short lock for the contract file and the choice of a mount name: taken
  for the read-modify-write of the file and the `mkdir` only;
- one lock per disk (`/run/thinfloe/automount/lock/<disk>`) around that
  disk's own mount, umount and power-off.

`eject` and `release` wait at most a few seconds for a disk lock and then
answer "busy"; a hung stick blocks only itself. Flushing uses `syncfs` (or
`sync -f`) on that disk's mounts, never a global `sync`, which would wait for
every other stick.

**`add <kname>`** skips the device when

- it is on the boot disk. The boot disk is the disk of
  `/run/thinfloe-esp-device`; if that file is missing, the disk of the
  source mounted at `/run/thinfloe/storage/efi` (as `thinfloe-upgrade` finds
  it); if neither exists and the boot was netboot (`ramroot=http…` on the
  command line), there is no boot disk; if neither exists on a stick boot,
  nothing is mounted and the reason goes to the journal;
- its partlabel is `EFI-SYSTEM` (every ThinFloe stick: this skips other
  ThinFloe sticks and a stick the writer has just partitioned);
- a live write hold for its disk exists (see the writer below);
- it is an `mmcblk` device whose card type is not `SD` (checked again here,
  so a wrong udev match cannot mount eMMC);
- it has no medium or no filesystem type;
- it is mounted already.

Otherwise it chooses a name (below), creates `/media/tfuser/<name>` with
`mkdir` (failing if it exists), checks with `lstat` that the path is a real
directory owned by root that it just made, mounts with `--no-canonicalize`
and per-filesystem options, and adds a line to the contract file. A mount
that fails removes the directory again (`rmdir`).

**`remove <kname>`** runs when the unit stops. It acts only on a mount
recorded in its own contract file, never on a path from anywhere else.

- The medium is gone (device node gone, or the device reports size 0 / no
  filesystem: a card pulled from a reader whose `/dev/sdX` stays):
  `umount -l`, `rmdir` the mount point (never a recursive delete), remove
  the line, and record a removal event: clean if the line was marked
  ejected, else unclean.
- The medium is still there (systemd stops the unit at shutdown or restart):
  `syncfs` and a normal `umount` within `TimeoutStopSec`, no `-l`, remove
  the line, no removal event. On a shutdown (`systemctl is-system-running`
  says `stopping`) it never records an event.
- A line already removed by `eject` or `release`: nothing to do, no event.

**`eject <disk>|all`**: marks the disk's lines ejected, `syncfs`, unmounts
every automount of the disk, removes the mount points and lines, records a
clean removal event per stick. Then power-off:

- a plain USB stick or disk is powered off the way `udisksctl power-off` does
  (sysfs), so its light goes off;
- a card reader (`ID_DRIVE_FLASH_SD`, `_MS`, `_CF`, `_SM` or several LUNs)
  and a built-in SD slot are never powered off: unmount only. A powered-off
  built-in USB reader (laptop, monitor, dock) could not be plugged in again
  and would see no further card until reboot.

Exit codes: success, busy (a file is open or the disk lock is held; nothing
unmounted, nothing powered off), failure. Each card in a multi-slot reader is
its own disk and is ejected alone.

**`release <disk>`**: for the writer. Unmounts all automounts of the disk
(as eject, without power-off), removes the lines, records no removal event.
Busy as above.

**`remount <disk>`**: `udevadm trigger --action=change` on the disk's
partitions, so the units start again. Used by the writer when it ends
without writing.

### Mount name

From the filesystem label; without one, the filesystem UUID (udisks2 does the
same). `/`, control characters and leading dots are replaced; a name already in
use gets a suffix (`MYSTICK1`). *My call:* match udisks2's naming so a path a
user learned on a laptop is the same here.

### Mount options (my call, after udisks2's defaults)

- vfat: `uid=1001,gid=1001,shortname=mixed,utf8=1,showexec,flush`
- exfat: `uid=1001,gid=1001,iocharset=utf8,errors=remount-ro`
- ntfs (kernel `ntfs3`): `uid=1001,gid=1001,windows_names`
- ext2/3/4, btrfs, xfs: no uid option exists; mounted as they are, the mount
  root left as the filesystem has it.
- all: `nosuid,nodev`, read-write.
- Any other filesystem: not mounted, no note.

The builder checks which of these modules the image's kernel has after
`slim-rootfs.sh` and adds a test that they stay.

### Contract files

`/run/thinfloe/removable`, written atomically by the helper only, mode 0644.
One line per mounted partition:

`device<TAB>disk<TAB>name<TAB>mountpoint<TAB>fstype<TAB>size_bytes<TAB>model<TAB>kind`

`disk` groups partitions into one stick for eject; `kind` is `stick` or
`card` (reader or SD slot), for the tile's wording. The last removal event
(sequence number, name, `clean` or `unclean`) is carried in a second small
file or a field (builder's call), so a reader can tell clean from unclean
without racing. Readers ignore unknown trailing fields.

A write hold is `/run/thinfloe/automount/hold/<disk>` holding the writer's
PID and the drive's id (serial or WWN, as `thinfloe-upgrade drives` reports
it). `add` honours it only while that PID is alive and the id matches the
disk now under that kernel name; a stale hold is removed.

Both are documented in a new `docs/notes/automount.md` and as a rule in
`CLAUDE.md`.

### OSD notes (part B)

The OSD process (`thinfloe-menu --osd`) watches `/run/thinfloe/removable`
and the event file itself (inotify on the directory, as the files are
replaced atomically) and shows a note for a few seconds:

- `MYSTICK connected`
- `MYSTICK can be removed` after an eject
- `MYSTICK removed without eject — files may be lost` (warning colour)

The OSD's first read after it starts only records the state: sticks present
at login get no note, and an old event is not shown again. Information only:
no icon that looks clickable, no input region, no focus, as every OSD
surface. *My call:* the OSD watches the file rather than the menu sending it
a datagram, so the notes do not depend on the menu's event loop. No Settings
switch turns the notes off in this round.

### Eject tile

- Shipped tile `overlay/etc/thinfloe/menu/<nn>-eject.desktop` with
  `X-TF-Action=eject`, honoured only in shipped files. Hidden while the
  contract file lists no stick.
- One stick: a tap ejects it and shows the result in the menu
  ("MYSTICK can be removed" or "MYSTICK is in use: close the files on it").
- Several sticks: a tap opens a small panel, one row per stick or card (name,
  size, model) with an **Eject** button, plus **Eject all**.
- The eject runs as a menu job (own process group), `sudo -n
  /usr/libexec/thinfloe/thinfloe-automount eject <disk>`.

### Restart, power off, log out

- Before the menu exits 10 (restart) or 11 (power off), and after its home
  save, it runs `eject all` as a job while it shows "Writing to MYSTICK …".
  A busy or failed stick is shown at the machine the way a failed save is,
  and the user chooses to wait or go on. This is the real flush: the unit's
  `ExecStop` at shutdown is only the fallback, bounded by `TimeoutStopSec`.
- Log out (exit 0) stops no system unit: the session starts again on tty1
  and the sticks stay mounted. Nothing is done for log out.

### Writing ThinFloe onto an automounted stick

- `thinfloe-upgrade drives` gains one boolean per drive, `automounted`: its
  only mounts are ours (lines in `/run/thinfloe/removable`). Such a drive is
  not reported as "mounted" for the purpose of refusing a write. `drives`
  reads no file on any stick.
- A new command `thinfloe-upgrade contents --device DEV` (part C) lists the
  top-level entries of each automounted partition of DEV: at most about 20
  names plus a count of the rest, names made valid UTF-8 (undecodable bytes
  replaced) and shortened. It takes no `upgrade.lock`, gives up after a few
  seconds and then reports "could not read the stick". The menu runs it once
  when the erase confirmation opens, as a job, and shows "These files will be
  erased: …"; while it runs, or when it failed, the confirmation shows no
  list and still works.
- `fresh_install` (the one path every write takes, menu and SSH alike),
  before the exclusive open: writes the hold, then runs `thinfloe-automount
  release <disk>`. Busy stops the write with "the stick is in use". The hold
  is removed in the existing cleanup when the write ends (success, failure,
  SIGTERM). A write that ends before its first byte was written (refused,
  cancelled, busy) runs `thinfloe-automount remount <disk>`, so the stick is
  mounted again.
- After `release`, the writer's `partprobe` and the image write make the
  kernel drop and re-create partitions; those `remove` runs find no line and
  record nothing, and the new partitions are skipped by the hold and by
  partlabel `EFI-SYSTEM`.
- Mounts that are not ours keep the present refusal.

### Tests (builder plans the detail)

- Unit: the helper's skip rules (boot disk incl. the missing-file cases,
  `EFI-SYSTEM`, live and stale hold, already mounted, no medium, `mmcblk`
  type `MMC` versus `SD`), name sanitising and collisions, the mount-point
  checks (a pre-existing entry or a symlink in `/media/tfuser` refuses the
  mount), the contract file format, the removal-event cases (pull, eject,
  release, shutdown), card-reader detection for power-off, the `automounted`
  field and the `contents` command (non-UTF-8 names, time limit).
- Lock behaviour: a disk whose mount hangs (a fake `mount`) does not delay
  `add` of another disk or `eject` of another stick.
- Menu: tile hidden or shown from the file, the eject panel, the flush before
  exit 10/11 and its busy case, the confirmation with and without a file
  list; the OSD reading the file, the three notes, no note on its first read.
- QEMU smoke: a new target (`make qemu-smoke-automount`) attaching a
  `usb-storage` FAT disk with a file on it: mounted at `/media/tfuser/<label>`,
  owned by tfuser, `/media/tfuser` owned by root, `/run/media/tfuser`
  resolves, eject unmounts it; the boot stick is never mounted;
  `qemu-smoke-two-sticks` still passes (the second ThinFloe stick is not
  mounted). The existing writer smoke tests run with a blank stick that is
  now automounted first, and a cancelled write mounts it again.
- No QEMU target emulates an SD slot cheaply (QEMU's `sdhci-pci` + `sd-card`
  may; the builder tries it, else the SD/MMC rule is covered by unit tests
  on recorded udev properties and an owner check on real hardware, together
  with a USB card reader's medium change).
- `test-image-size.sh` budget unchanged unless a filesystem module or tool
  must be added.

## Left to the builder

- Helper language; exact sysfs power-off method; lock file locations.
- How the last removal event is carried (second file or a field).
- Exact tile position and icon, panel layout within the theme and
  `brand.rs` constants.
- The number of entries and the time limit of `contents`.

## Calls made in this design (for the owner to check)

- Other ThinFloe sticks (partlabel `EFI-SYSTEM`) are not automounted.
- Mount names and options follow udisks2; `/media/tfuser` belongs to root.
- The OSD watches the contract file itself; no note for sticks present at login.
- No Settings switch for the notes in this round.
- ext4 and similar are mounted without an owner change.
- Built-in SD slots: SD cards mounted, eMMC never (owner: yes to built-in slots).
- Card readers and SD slots are never powered off, only unmounted.
- Restart and power off flush the sticks in the menu, after the home save.
- A write that never started mounts the stick again.
- The erase file list is a separate command, never part of `drives`.
