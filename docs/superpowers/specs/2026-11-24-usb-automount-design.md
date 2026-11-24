# USB stick automount

Date: 2026-11-24. Status: design, awaiting owner approval.

## Intent

A user plugs a USB stick, an SD card in a USB card reader or a USB disk into a
ThinFloe machine and can use its files at once, in the place a normal Ubuntu
desktop puts them: `/media/tfuser/<label>`. Remote-desktop clients then find
them without ThinFloe-specific setup: OxuLnk can share `/media/tfuser` as one
folder (OxuLnk already shares folders explicitly; a default share of
`/media/tfuser` is OxuLnk's work, not part of this design), and ThinLinc's
local drive redirection can export the same path.

Today nothing mounts such a device: the kernel creates `/dev/sdX1` and that is
all. The image has no udisks2, no polkit, no file manager.

## Goals

- Every partition with a filesystem the kernel can read, on every removable or
  USB block device and on cards in a built-in SD slot, is mounted read-write at `/media/tfuser/<label>`, owned by
  tfuser. Several sticks and multi-slot card readers at once.
- `/run/media/tfuser` is a link to `/media/tfuser`, for clients that look in the
  upstream udisks2 place.
- The user sees a short OSD note when a stick is connected and when it is
  removed; a removal without eject gets a warning note ("files may be lost").
- An **Eject** tile in the menu, present only while at least one stick is
  mounted, ejects one stick or all, and says when it is safe to pull them.
- Making a ThinFloe stick on the Upgrade screen keeps working: the writer
  unmounts our automounts of the target itself, and the overwrite warning lists
  the files on the stick, so the user sees what is about to be erased.

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
- A slow or broken stick delaying the boot, the login or the menu.
- The OSD taking a click or focus (standing rule: it never does). The note has
  no eject icon or button.
- A crash or a hung mount leaving a mount point the next stick collides with.

## Constraints from the project

- Boot stick identity: the ESP the initramfs chose is in
  `/run/thinfloe-esp-device` and mounted at `/run/thinfloe/storage/efi`; the
  whole disk holding it is excluded. Chosen by stick ID, never by partlabel
  (standing constraint).
- No polkit: privileged actions from the session go through `sudo -n` (tfuser
  has NOPASSWD), as reboot and `thinfloe-upgrade` do.
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
  `After=dev-%i.device`), so a removal stops it.
- `overlay/etc/systemd/system/thinfloe-automount@.service`: `Type=oneshot`,
  `RemainAfterExit=yes`, `ExecStart=.../thinfloe-automount add %i`,
  `ExecStop=.../thinfloe-automount remove %i`, a `TimeoutStartSec` so a hung
  mount gives up. Not enabled; started only by udev, at boot too (udev trigger
  replays devices plugged in before boot).
- `overlay/usr/libexec/thinfloe/thinfloe-automount` (root; language the
  builder's call, Python like `peripheral-battery` or shell):
  - `add <kname>`: skips the device when
    - it is on the boot disk (the disk of `/run/thinfloe-esp-device`),
    - its partlabel is `EFI-SYSTEM` (every ThinFloe stick: this skips other
      ThinFloe sticks and a stick the writer has just partitioned),
    - a write hold for its disk exists (see the writer below),
    - it is mounted already;
    (the helper checks the SD-not-MMC rule again, so a wrong udev match
    cannot mount eMMC) then makes `/media/tfuser/<name>`, mounts with per-filesystem options, and
    adds a line to the contract file.
  - `remove <kname>`: runs when the unit stops. Two cases:
    - the device node is gone (a pull): `umount -l` if still mounted,
      removes the mount point (`rmdir`, never a recursive delete), removes
      the line, and records whether the removal was clean (preceded by an
      eject) or not;
    - the device is still there (systemd stops the unit at shutdown, restart
      or log out): `sync` and a normal `umount`, no `-l`, and no removal
      event, so the OSD shows no warning. *My call:* power off, restart and
      log out need no extra step or warning; shutdown unmounts the sticks
      cleanly. The menu's exit path is not touched.
  - `eject <disk>|all`: `sync`, unmounts every automount of the disk, removes
    the mount points, then powers the USB device off (a built-in SD slot: unmount only) the way `udisksctl
    power-off` does (sysfs), so the stick's light goes off, but only when no
    other disk on the same USB device is still mounted. *My call:* in a
    multi-slot card reader each card is its own row and can be ejected
    alone (sync and unmount make it safe to pull); the reader is powered
    off with its last mounted card. Exit code tells
    success, "busy" (a file is open; nothing powered off) or failure. Takes
    only automounts under `/media/tfuser`; never anything else.
  - `release <disk>`: unmount without power-off, for the writer.
  - Every run takes one lock file, so add, remove and eject never interleave.
- `/media/tfuser` (0755, tfuser) and the link `/run/media/tfuser -> /media/tfuser`,
  made by a `tmpfiles.d` entry.
- sudoers: nothing new; tfuser's NOPASSWD covers `sudo -n thinfloe-automount
  eject ...`.

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

### Contract file `/run/thinfloe/removable`

Written atomically by the helper only; mode 0644. One line per mounted
partition:

`device<TAB>disk<TAB>name<TAB>mountpoint<TAB>fstype<TAB>size_bytes<TAB>model`

`disk` groups partitions into one stick for eject. A second, small file (or a
field; builder's call) carries the last removal event (`name`, clean or not, a
sequence number), so a reader can tell a clean from an unclean removal without
racing. Readers ignore unknown trailing fields. Documented in a new
`docs/notes/automount.md` and as a rule in `CLAUDE.md`.

### OSD notes

The OSD process (`thinfloe-menu --osd`) watches `/run/thinfloe/removable`
itself (inotify on the directory, as the file is replaced atomically) and
shows a note for a few seconds:

- `MYSTICK connected`
- `MYSTICK can be removed` after an eject
- `MYSTICK removed without eject — files may be lost` (warning colour)

Information only: no icon that looks clickable, no input region, no focus, as
every OSD surface. *My call:* the OSD watches the file rather than the menu
sending it a datagram, so the notes do not depend on the menu's event loop.
Whether a Settings switch turns the notes off is left out for now (the owner
said "optional"; the builder adds a `[osd]`/keyboard-tab style switch only if
the owner asks for it at review).

### Eject tile

- Shipped tile `overlay/etc/thinfloe/menu/<nn>-eject.desktop` with
  `X-TF-Action=eject`, honoured only in shipped files. Hidden while the
  contract file lists no stick.
- One stick: a tap ejects it and shows the result in the menu
  ("MYSTICK can be removed" or "MYSTICK is in use: close the files on it").
- Several sticks: a tap opens a small panel, one row per stick (name, size,
  model) with an **Eject** button, plus **Eject all**.
- The eject runs as a menu job (own process group), `sudo -n
  thinfloe-automount eject <disk>`.

### Writing ThinFloe onto an automounted stick

- `thinfloe-upgrade drives`: a drive whose only mounts are our automounts
  (under `/media/tfuser`) is not reported as "mounted" for the purpose of
  refusing a write. The JSON row gets a new field `contents`: the top-level
  entries of each automounted partition (names, at most about 20, plus a
  count of the rest), so the menu can show them.
- `thinfloe-upgrade write`: before the exclusive open, writes a hold for the
  disk (`/run/thinfloe/automount-hold/<disk>`), then runs
  `thinfloe-automount release <disk>`. "Busy" stops the write with "the stick
  is in use". The hold stays until the write ends (success, failure or
  SIGTERM) and is removed in the existing cleanup. A stale hold after a crash
  only stops automount of that one disk until reboot (it lives in `/run`).
- The Upgrade screen's erase confirmation ("Type … to erase this drive")
  shows the `contents` list: "These files will be erased: …".
- Mounts that are not ours keep the present refusal.

### Tests (builder plans the detail)

- Unit: the helper's skip rules (boot disk, `EFI-SYSTEM`, hold, already
  mounted, `mmcblk` with type `MMC` versus `SD`), name sanitising and collisions, the contract file format, the
  `drives` `contents` field and the "only automounts" judgement.
- Menu: tile hidden or shown from the file, the eject panel, the confirmation
  with a file list; the OSD reading the file and the three notes.
- QEMU smoke: a new target (`make qemu-smoke-automount`) attaching a
  `usb-storage` FAT disk with a file on it: mounted at `/media/tfuser/<label>`,
  owned by tfuser, `/run/media/tfuser` resolves, eject unmounts it; the boot
  stick is never mounted; `qemu-smoke-two-sticks` still passes (the second
  ThinFloe stick is not mounted). The existing writer smoke tests run with a
  blank stick that is now automounted first.
- No QEMU target emulates an SD slot cheaply (QEMU's `sdhci-pci` + `sd-card`
  may; the builder tries it, else the SD/MMC rule is covered by unit tests
  on recorded udev properties and an owner check on real hardware).
- `test-image-size.sh` budget unchanged unless a filesystem module or tool
  must be added.

## Left to the builder

- Helper language; exact sysfs power-off method; lock file location.
- How the last removal event is carried (second file or a field).
- Exact tile position and icon, panel layout within the theme and
  `brand.rs` constants.
- The number of entries in `contents`.

## Calls made in this design (for the owner to check)

- Other ThinFloe sticks (partlabel `EFI-SYSTEM`) are not automounted.
- Mount names and options follow udisks2.
- The OSD watches the contract file itself.
- No Settings switch for the notes in this round.
- ext4 and similar are mounted without an owner change.
- Built-in SD slots: SD cards mounted, eMMC never (owner: yes to built-in slots).
- Multi-slot readers: each card ejected alone; the reader powered off with its last card.
- Shutdown, restart and log out unmount sticks silently; no warning, no menu change.
