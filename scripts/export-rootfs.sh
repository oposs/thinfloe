#!/bin/bash
# export-rootfs.sh - Export kernel, initrd, and squashfs

set -euo pipefail

source /scripts/lib/common.sh

ARTIFACT_DIR="/artifacts/rootfs"
SQUASHFS_COMP="${SQUASHFS_COMP:-zstd}"
SQUASHFS_COMP_LEVEL="${SQUASHFS_COMP_LEVEL:-19}"
VERSION="${VERSION:-dev}"
BUILD_ID="${BUILD_ID:-unknown}"
BUILD_JOBS="${BUILD_JOBS:-4}"

log_info "Rootfs Export"
log_info "  Version:     $VERSION"
log_info "  Build ID:    $BUILD_ID"
log_info "  Compression: ${SQUASHFS_COMP}:${SQUASHFS_COMP_LEVEL}"

ensure_dir "$ARTIFACT_DIR"

# Export kernel
log_step "Exporting kernel..."
KERNEL_SRC=$(find /boot -name 'vmlinuz-*' -type f | sort | tail -1)
[[ -n "$KERNEL_SRC" ]] || die "No kernel found"
cp "$KERNEL_SRC" "${ARTIFACT_DIR}/vmlinuz"
log_success "Kernel exported"

# Export initramfs
log_step "Exporting initramfs..."
INITRD_SRC=$(find /boot -name 'initrd.img-*' -type f | sort | tail -1)
[[ -n "$INITRD_SRC" ]] || die "No initramfs found"
cp "$INITRD_SRC" "${ARTIFACT_DIR}/initrd.img"
log_success "Initramfs exported"

# Cleanup
log_step "Cleaning up rootfs..."
rm -rf /boot/* /.dockerenv /etc/machine-id /var/lib/dbus/machine-id /var/log/* /tmp/* /var/tmp/*
log_success "Cleanup complete"

# Create squashfs
log_step "Creating squashfs..."
SQSH_OUTPUT="${ARTIFACT_DIR}/ramroot.sqsh"

# etc/hostname and etc/resolv.conf are excluded because podman bind-mounts
# them (and /etc/hosts) into this container, so mksquashfs would read through
# the mounts and pack the container's runtime files instead of the image's.
# /etc/hosts and /etc/resolv.conf are replaced on every boot by
# un-dockerize.service, and the initramfs writes /etc/hostname from the
# machine-id; before this, every machine shipped the build container's ID.
mksquashfs / "$SQSH_OUTPUT" \
    -comp "$SQUASHFS_COMP" \
    -Xcompression-level "$SQUASHFS_COMP_LEVEL" \
    -b 1M \
    -processors "$BUILD_JOBS" \
    -noappend \
    -no-progress \
    -e boot tmp var/tmp artifacts proc sys dev run \
       etc/hostname etc/resolv.conf || die "mksquashfs failed"

log_success "SquashFS created: $(format_bytes $(stat -c%s "$SQSH_OUTPUT"))"

chmod 644 "${ARTIFACT_DIR}"/*
