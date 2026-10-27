#!/bin/bash
# finalize-upgrade.sh - Finalize pending upgrade by rotating tl.new -> tl -> tl.old
# Runs in initramfs after auto-resize, before mount-squashfs (priority 25)
#
# thinfloe-upgrade writes tl.new/ and sets syslinux.cfg to DEFAULT new, so the
# next boot loads its kernel from tl.new/ with ramroot=tl.new/ramroot.sqsh.
# On that boot this script:
# 1. Sets syslinux.cfg back to DEFAULT current
# 2. Removes old fallback (tl.old/)
# 3. Moves current to fallback (tl/ -> tl.old/)
# 4. Activates new version (tl.new/ -> tl/), and points the mount hook at
#    tl/ramroot.sqsh
# A boot whose kernel came from tl/ never rotates; see tf_finalize_upgrade.

type info >/dev/null 2>&1 || . /lib/dracut-lib.sh
. /lib/thinfloe-lib.sh

# Only run for USB boot (not network boot)
# Read ramroot= from the kernel command line, not from the file the cmdline
# hook writes.
# dracut-pre-mount.service is ordered only after dracut-initqueue.service,
# which is not part of the boot transaction here, so this hook runs in
# parallel with the cmdline hook that writes that file - measured with
# rd.debug, half a second before it. The file was therefore missing, a
# netboot was taken for a USB boot, and the boot waited 60s for an ESP
# that does not exist.
ramroot=$(getarg ramroot=)
if is_remote_url "$ramroot"; then
    tf_log "finalize-upgrade: Network boot - skipping"
    exit 0
fi

# Wait for the boot medium. This hook runs BEFORE mount-squashfs, so it is
# usually the first thing in the boot to look for the device - and it used to
# check once after `udevadm settle`, which returns immediately when the USB
# controller has not enumerated the stick yet. See tf_find_esp().
#
# Getting this wrong here is quieter and nastier than in mount-squashfs: a
# pending tl.new upgrade would simply not be finalized, mount-squashfs would
# then find the device a moment later and boot the OLD tl/, and the machine
# would come up looking perfectly healthy with the upgrade silently skipped -
# possibly applying on some later boot instead. Upgrades must not be a race.
if ! EFI_DEV=$(tf_find_esp); then
    # Still non-fatal: mount-squashfs runs next and will report authoritatively
    # if the device never appears. But say plainly that a pending upgrade is
    # being skipped, rather than implying there was simply nothing to do.
    tf_log "finalize-upgrade: EFI-SYSTEM did not appear - skipping"
    tf_log "finalize-upgrade: a pending tl.new upgrade (if any) was NOT applied"
    exit 0
fi

# Mount EFI partition temporarily
efi_mount="/tmp/efi-finalize-$$"
mkdir -p "$efi_mount"

if ! mount -t vfat "$EFI_DEV" "$efi_mount" 2>/dev/null; then
    tf_log "finalize-upgrade: Failed to mount EFI partition - skipping"
    rmdir "$efi_mount" 2>/dev/null
    exit 0
fi

# The rotation itself lives in tf_finalize_upgrade() in thinfloe-lib.sh, so
# test/integration/test-initramfs-home.sh can run it with only the commands
# the initramfs actually has.
# /tmp/thinfloe-ramroot is written here, never read: after a rotation the
# squashfs is in tl/, not where ramroot= says. The cmdline hook has written
# the file by now - thinfloe-after-cmdline.conf orders pre-mount after it.
tf_finalize_upgrade "$efi_mount" "$ramroot" /tmp/thinfloe-ramroot
rc=$?

umount "$efi_mount"
rmdir "$efi_mount" 2>/dev/null

exit $rc
