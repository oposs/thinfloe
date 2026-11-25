This note covers the automounter: what is mounted where, which devices are left alone, the contract and event files the menu reads, the write hold, the locks, the power-off rule, the tests and the checks that need hardware.

The helper is `overlay/usr/libexec/thinfloe/thinfloe-automount` (root, Python); its docstring has the file formats. Design and decisions: [docs/superpowers/specs/2026-11-24-usb-automount-design.md](../superpowers/specs/2026-11-24-usb-automount-design.md).

## What is mounted where

A USB stick, a USB disk, a card in a USB card reader and an SD card in a
built-in slot are mounted under `/media/tfuser/<name>` when a file system
appears on them, at boot too (udev replays every device). `/media/tfuser`
is root's (0755, `thinfloe-automount.conf` in `tmpfiles.d`), so tfuser
can work inside a mounted stick but cannot create, rename or remove the
mount points; `/run/media/tfuser` is a link to it, the path file managers
and ThinLinc's and OxuLnk's folder sharing look for.

The udev rule `90-thinfloe-automount.rules` only starts
`thinfloe-automount@<kname>.service` (`add` on start, `remove` on stop,
`BindsTo=` the device). Candidates are partitions and whole disks without
a partition table on a removable disk, on the USB bus, or an `mmcblk` card
of type `SD` (eMMC never). A card put into a reader is a change event of
the reader's disk: a change with no file system marks a whole disk not
ready (`SYSTEMD_READY=0`, the unit stops and unmounts), a change with one
makes it ready again and starts the unit.

Name: the file system label, else its UUID, else `disk`; `/`, control
characters (C1 too) and leading dots replaced, cut to 255 bytes, a name in
use gets a number (`MYSTICK1`), as udisks2 does. The label is udev's
`ID_FS_LABEL_ENC` with its `\xNN` escapes decoded, as udisks2 reads it:
`ID_FS_LABEL` has `_` for every blank, so "My Passport" would become
`My_Passport`; it is mounted at `/media/tfuser/My Passport`. Options: vfat
`uid=1001,gid=1001,shortname=mixed,utf8=1,showexec,flush`, exfat
`uid=1001,gid=1001,iocharset=utf8,errors=remount-ro`, ntfs (kernel
`ntfs3`) `uid=1001,gid=1001,windows_names`; ext2/3/4, btrfs and xfs as
they are; always `nosuid,nodev`, read-write. Any other file system is not
mounted.

## Left alone

`add` mounts nothing, and logs why to the journal (`thinfloe-automount`), for

- the boot disk (the disk of `/run/thinfloe-esp-device`, else of the
  source at `/run/thinfloe/storage/efi`; netboot has none; a stick boot
  whose boot disk cannot be found mounts nothing at all),
- a partition labelled `EFI-SYSTEM`: every ThinFloe stick, so a second
  ThinFloe stick and a stick the writer has just written stay unmounted,
- a disk with a live write hold (below),
- an `mmcblk` device whose card type is not `SD`,
- a device that is not on a removable disk, not on USB and not an SD card
  in a built-in slot: the udev rule's test, done again, so `remount` or an
  `add` by hand never mounts an internal disk,
- no medium, no file system, a file system not in the list, or a device
  mounted already.

## Contract and event files

`/run/thinfloe/removable`, written atomically by the helper only, one line
per mounted partition:

    device<TAB>disk<TAB>name<TAB>mountpoint<TAB>fstype<TAB>size_bytes<TAB>model<TAB>kind

`disk` groups the partitions of one stick for eject; `kind` is `stick` or
`card` (a reader, found by `ID_DRIVE_FLASH_*` or by more than one disk on
the same USB device, or a built-in SD slot). Readers ignore unknown trailing
fields. The menu's Eject tile and panel read it (`removable.rs`).

A line counts only while `/proc/self/mountinfo` shows its device at its
mount point. Every command first drops the lines whose mount is gone, with
their empty directories: a helper killed between a umount and the update of
the file (a cancelled write during `release`) would otherwise leave a line
that makes `add` skip the next stick under the same kernel name and
`remove` report it pulled without eject. An empty mountinfo read (the
file always lists `/`) drops nothing. The same step removes the empty
directories under `/media/tfuser` that no line names and that are not
mounted (a helper killed between its `mkdir` and its mount): only empty
directories, never through a link, and never the one a running `add` has
reserved (`/run/thinfloe/automount/reserved/<name>`, its pid).

`/run/thinfloe/removable.event` holds the last removal, one line
`seq<TAB>name<TAB>clean|unclean`: `clean` after an eject, `unclean` for a
stick pulled or a card taken out while mounted. A shutdown records nothing.
`eject all` replaces the line once per disk, so a reader sees only the last.

## Commands

`thinfloe-automount add|remove <kname> | eject <disk>|all | release|remount <disk>`;
exit 0 done, 1 failure (a partly unmounted disk too: the partitions still
mounted are printed), 2 usage (also an argument that is not a kernel name:
no `/`, no `.`, at most 32 characters), 3 busy (nothing changed). The menu calls
`sudo -n /usr/libexec/thinfloe/thinfloe-automount eject …`; tfuser's
`NOPASSWD` covers it.

- `remove`: medium gone, `umount -l` and an `unclean` event; medium still
  there (the unit stopped at shutdown), `sync -f` and a plain `umount`, no
  event. Gone means no device node, size 0, or a udev record without a file
  system (a whole-disk card swapped in a reader keeps its node and a size);
  a udev record that cannot be read counts as there.
- `eject`: `sync -f` and `umount` of every partition of the disk, the
  lines go, a `clean` event; then a stick is powered off (below).
- `release`: as eject, without event and power-off; for the writer. Before
  the first umount it records the partitions it takes off in
  `/run/thinfloe/automount/released/<disk>`.
- `remount`: `add` for each partition that record names, and nothing else,
  with the same locks, hold check and skip rules; the record goes. A release
  killed half-way is covered: its record is written, and the stale lines are
  dropped. Not a udev change event: after `release` the partitions' units
  are still active, and systemd would not start them again.

## Write hold

`thinfloe-upgrade` (every write, menu and SSH alike) writes
`/run/thinfloe/automount/hold/<kname>` (`pid<TAB>id`, atomically) and runs
`release` before it opens the stick; a busy release refuses the write with
"the stick is in use". `add` skips the disk while the pid lives and the
serial part of the id matches (the write repartitions the stick, so the size
and partition fingerprint of the id would never match); a stale hold is
removed, but only the file that was read (same inode): a hold the writer
renamed into place meanwhile stays. The hold goes when the write ends; a write that never wrote a byte
runs `remount`, so the stick comes back (step `remount`, "Mounting the
stick again": it can take up to the helper's 80 s add budget). `drives` reports such a stick as
`automounted`, not `mounted`; an automounted stick that is not the boot
stick is not upgraded in place.

## Locks

No lock is held across a mount, umount or sync of another disk. One short
lock (`/run/thinfloe/automount/`) covers the contract file and the choice
of a mount name; one lock per disk covers that disk's mount, umount and
power-off. Flushing is `sync -f` per mount, never a global `sync`.

Timeouts: one mount, umount or sync gets at most 30 s and is then killed
(and waited for 1 s at most: a mount stuck in the kernel cannot die). The
helper keeps within budgets below the unit's timeouts, so systemd never
kills it between a mount and its line. Each command's timeout counts that
1 s and keeps time for the steps after it; a step with no time left is not
started:

- `add` ends within 80 s (`TimeoutStartSec=90`). udev starts the units of
  all partitions of a stick at once; the second waits for the disk lock
  while the first mounts, up to 10 s before the end of its budget, so both
  are mounted. `remount` shares one such budget for all its partitions.
- `remove` ends within 25 s (`TimeoutStopSec=30`): it waits for the disk
  lock up to 15 s (an `add` of a stick pulled while it was mounted), then
  the udev query, `sync -f`, `umount` and `umount -l` share what is left,
  each keeping 2 s for every step after it.
- `eject` and `release` wait 5 s for a disk lock and then answer busy.

## Power off, restart

After an eject a plain stick or USB disk is removed from the USB bus through
sysfs, as `udisksctl power-off` does, so its light goes off. A card reader
and a built-in SD slot are only unmounted: a powered-off reader built into a
laptop, monitor or dock would see no further card until reboot.

Restart and Power off in the menu run `eject all` after the home save and
show "Writing to …"; a busy or failed stick is shown like a failed save,
with the choice to wait or go on; a release that takes longer than 60 s
(`EJECT_GRACE`) is shown the same way ("Releasing the sticks takes too
long"). Log out leaves the sticks mounted. The unit's `ExecStop` at
shutdown (`TimeoutStopSec=30`) is only the fallback.

## Tests

- `test/unit/test_automount.py`: skip rules, names, options, contract and
  hold formats, add/remove/eject/release/remount on fake sysfs and `/dev`
  trees (`TF_SYSFS`, `TF_DEV`, `TF_RUN`, `TF_MEDIA`, …).
- `test/unit/test_upgrade_batch.py`: hold, release and remount around a
  write; the automounted refusal.
- `test/integration/test-automount-image.sh`: rule, unit, tmpfiles entry,
  helper and the file system modules are in the image, with their modes.
- The menu's `removable`, eject and flush tests (`cargo test`).
- `make qemu-smoke-automount` (`scripts/run-qemu-automount-smoke.sh`): a
  USB stick with a vfat partition and a two-slot USB card reader (`usb-bot`,
  two removable LUNs). The stick is mounted at `/media/tfuser/SMOKEU`,
  owned by tfuser, kind `stick`; the boot disk is not mounted; the card in
  slot 0 is mounted as kind `card`, the medium without a file system in
  slot 1 is not. The host takes the card out through the QEMU monitor (QMP
  `eject`): its line goes and the event says `unclean`; it puts another
  card in (`blockdev-change-medium`): the new card is mounted on the same
  disk. Eject of the card leaves the reader powered, eject of the stick
  powers it off; both give a `clean` event.
- `make qemu-smoke-write`: both target sticks carry vfat and are
  automounted before the writes; a write from a release without the image
  fails before its first byte and the stick is mounted again; while the
  cancelled write runs the stick is released and held, and the hold is gone
  after it; the ThinFloe stick written is not mounted.
- `make qemu-smoke-two-sticks`: nothing is mounted under `/media/tfuser`
  with a second ThinFloe stick attached.

## Owner checks on hardware

Not covered by QEMU; to be done once on real machines:

- a built-in SD slot (`mmcblk`, card type `SD`): a card is mounted as kind
  `card`, eject unmounts it and does not power off the slot, a second card
  afterwards is mounted again;
- a USB card reader with a card swap: the old card's line goes, the new card
  is mounted, the reader is never powered off after an eject;
- a laptop's internal reader on USB, which reports one LUN and no
  `ID_DRIVE_FLASH_*`: check which `kind` it gets and that eject does not
  power it off for good;
- the eMMC of a thin client (`mmcblk`, type `MMC`) is never mounted;
- a stick with exfat and one with NTFS mount and can be written by tfuser.
