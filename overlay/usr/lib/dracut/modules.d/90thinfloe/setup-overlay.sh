#!/bin/bash
# setup-overlay.sh - Setup overlayfs with ZRAM

type info >/dev/null 2>&1 || . /lib/dracut-lib.sh
. /lib/thinfloe-lib.sh

# Only run if we're handling the root
[ "$root" = "thinfloe" ] || return 0

lower_mount="$TF_LAYERS/lower"
upper_base="$TF_LAYERS/upper"

mountpoint -q "$lower_mount" || die "Lower layer not mounted"

info "Setting up overlay filesystem"

# Create ZRAM
zram_dev=$(create_zram_overlay "2G")
[ -n "$zram_dev" ] || die "Failed to create ZRAM"

mkdir -p "$upper_base"
mount "$zram_dev" "$upper_base" || die "Failed to mount ZRAM"

mkdir -p "$upper_base/root" "$upper_base/work"

info "ZRAM overlay mounted"

# Load overlay module
modprobe overlay 2>/dev/null || die "Failed to load overlay module"

# Mount overlay
NEWROOT="${NEWROOT:-/sysroot}"

info "Mounting overlay to $NEWROOT"

# Create required mount point directories in upper layer
# (Docker can't create /proc, /sys, /dev in the image since they're mount points)
mkdir -p "$upper_base/root/proc" "$upper_base/root/sys" "$upper_base/root/dev"
mkdir -p "$upper_base/root/run" "$upper_base/root/tmp"

mount -t overlay overlay \
    -o lowerdir="$lower_mount",upperdir="$upper_base/root",workdir="$upper_base/work" \
    "$NEWROOT" || die "Failed to mount overlay"

info "Overlay filesystem mounted"

# Persist machine-id
machine_id=$(get_machine_id)
[ -n "$machine_id" ] || die "Failed to get machine-id"

echo "$machine_id" > "$NEWROOT/etc/machine-id" || \
    warn "Could not write machine-id"

info "Machine-id set: $machine_id"

# Name the machine. mksquashfs runs inside the build container, where podman
# bind-mounts /etc/hostname, so the image used to ship the container ID and
# every machine had the same name; the file is now excluded from the squashfs.
# Writing it here means systemd PID 1 reads the right name at startup, before
# it loads any unit. On netboot there is no ESP, so the machine-id is new
# every boot; the boot MAC (recorded by mount-squashfs.sh) is stable and is
# the identity the home archive is already keyed on, so its last six hex
# digits name the machine. On USB boot the persisted machine-id is used.
tf_write_hostname "$NEWROOT" "$machine_id" "$TF_STATE"

# Restore the home directory here, in the initramfs, rather than at login.
#
# It has to be in place before multi-user.target so thinfloe-ssh-hostkeys can
# take the persistent SSH host keys out of it before sshd starts. Doing it
# at login - where it used to happen - is far too late for that.
#
# restore_home always returns 0; a client that cannot reach its home archive
# still boots with the shipped default home.
restore_home "$NEWROOT" "$TF_STORAGE/efi" "$TF_STATE" \
    "$(cat /tmp/thinfloe-ramroot 2>/dev/null)"

# Install ~/wifi.yaml and ~/wg0.conf into /etc now, before systemd starts in
# the new root, so the netplan generator sees them. Always returns 0.
install_local_network "$NEWROOT"

echo "$zram_dev" > "$TF_STATE/zram-device"

return 0
