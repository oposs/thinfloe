#!/bin/bash
# module-setup.sh - ThinFloe dracut module

check() {
    return 255
}

depends() {
    # Netboot networking comes from dracut's systemd-networkd and
    # systemd-resolved modules, added with --add in containers/ubuntu/Dockerfile.
    return 0
}

install() {
    inst_hook cmdline 30 "$moddir/parse-thinfloe.sh"
    inst_hook pre-mount 20 "$moddir/grow-esp.sh"          # Grow the ESP, before anything mounts it
    inst_hook pre-mount 25 "$moddir/finalize-upgrade.sh"  # Finalize pending tl.new upgrade
    inst_hook mount 30 "$moddir/mount-squashfs.sh"   # Mount hooks run in mount phase
    inst_hook mount 40 "$moddir/setup-overlay.sh"    # After squashfs is mounted
    inst_hook pre-pivot 30 "$moddir/thinfloe-cleanup.sh"

    inst_simple "$moddir/thinfloe-lib.sh" "/lib/thinfloe-lib.sh"

    # Order the pre-mount and mount hooks after the cmdline hook that sets
    # root=thinfloe and writes /tmp/thinfloe-ramroot. See thinfloe-after-cmdline.conf.
    local _unit
    for _unit in dracut-pre-mount.service dracut-mount.service; do
        inst_simple "$moddir/thinfloe-after-cmdline.conf" \
            "$systemdsystemunitdir/$_unit.d/90-thinfloe-after-cmdline.conf"
    done

    # The boot screen draws the host name in Geo. label-pango finds fonts
    # through fontconfig, and plymouth-populate-initrd copies only the fonts a
    # theme names in Font=, so the font is installed here. See
    # /usr/share/plymouth/themes/thinfloe/thinfloe.script.
    inst_simple /usr/share/fonts/truetype/thinfloe/Geo-Oblique.ttf

    # Ask the DHCP server for its time zone (option 101), so the lease the
    # initramfs's networkd writes carries TIMEZONE= and
    # thinfloe-dhcp-timezone.service applies it right after switch-root.
    # dracut's systemd-networkd module copies its default network file to
    # /run/systemd/network/zzzz-dracut-default.network (no ip= on the command
    # line); a drop-in with that name extends it.
    inst_simple "$moddir/50-timezone.conf" \
        /etc/systemd/network/zzzz-dracut-default.network.d/50-timezone.conf

    # Network tools. DHCP is systemd-networkd's job (see mount-squashfs.sh);
    # ip finds the interface that holds the lease.
    inst_multiple curl ip

    # Archive tools for the home restore in restore_home(). GNU tar runs
    # gzip as a separate process for -z, so both are required.
    inst_multiple tar gzip

    # Filesystem tools
    inst_multiple zramctl blockdev losetup mountpoint
    inst_multiple mkfs.ext4 fsck.fat
    # Also install fsck.vfat symlink
    inst /sbin/fsck.vfat
    inst_multiple stat dd od tr mkdir mount umount cp ln chmod
    # finalize-upgrade.sh and tf_finalize_upgrade(): sync orders the FAT
    # directory updates of the tl.new -> tl rotation, rmdir removes its
    # temporary mount point. Neither is in dracut's base set.
    inst_multiple sync rmdir
    inst_multiple udevadm awk grep sed lsblk
    # grow-esp.sh: sgdisk rewrites the GPT entry (and the hybrid MBR),
    # partx tells the kernel, fat32expander grows the FAT32.
    inst_multiple sgdisk partx fat32expander

    # Kernel modules (only what we need)
    instmods squashfs overlay zram loop
    instmods ext4 vfat
    # Network - be selective. Wired only: nothing in the initrd uses WiFi
    # (no wpa_supplicant, netboot needs a cable), and iwlwifi brought 82 MB
    # of firmware with it. The running system loads WiFi from the squashfs.
    instmods e1000 e1000e igb r8169 atlantic
    instmods af_packet

    return 0
}

installkernel() {
    instmods squashfs overlay zram loop ext4 vfat
    # Intel graphics, with the firmware its modinfo names (about 12 MB).
    # Plymouth starts on simpledrm; when i915 bound only after switch-root,
    # Plymouth did not follow it to the new card and the screen stayed dark
    # until the menu drew. Bound here, it takes over before the squashfs
    # loads. dracut's drm module stays omitted, so no other GPU driver
    # comes along.
    instmods i915
}
