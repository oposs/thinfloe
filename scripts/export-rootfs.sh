#!/bin/bash
# export-rootfs.sh - Export kernel, initrd, and squashfs

set -euo pipefail

source /scripts/lib/common.sh

ARTIFACT_DIR="/artifacts/rootfs"
SQUASHFS_COMP="${SQUASHFS_COMP:-zstd}"
SQUASHFS_COMP_LEVEL="${SQUASHFS_COMP_LEVEL:-19}"
BUILD_ID="${BUILD_ID:-unknown}"
BUILD_JOBS="${BUILD_JOBS:-4}"
TF_VERSION="${TF_VERSION:-}"

# The version this rootfs carries, from the Makefile: the release X.Y.Z, or
# X.Y.Z-dev+<short sha>[.dirty] for a local build. One line, nothing else.
valid_version() {
    [[ $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+(-dev\+[0-9a-f]+(\.dirty)?)?$ ]]
}

log_info "Rootfs Export"
# Only the version the rootfs carries: the container tag (VERSION) was
# printed as "Version: 3.0.0" beside it and named no release.
log_info "  Version:     $TF_VERSION"
log_info "  Build ID:    $BUILD_ID"
log_info "  Compression: ${SQUASHFS_COMP}:${SQUASHFS_COMP_LEVEL}"

# Before anything is exported, so a wrong version leaves no new artifact.
valid_version "$TF_VERSION" \
    || die "TF_VERSION must be one line X.Y.Z or X.Y.Z-dev+<sha>[.dirty], got '$TF_VERSION'"

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
find /boot /var/log /tmp /var/tmp -mindepth 1 -delete
rm -f /.dockerenv /etc/machine-id /var/lib/dbus/machine-id
log_success "Cleanup complete"

# Write the version into the rootfs and next to it. This container is
# thrown away after the export, so the image itself carries no version and a
# new commit does not rebuild it.
log_step "Writing version..."
mkdir -p /etc/thinfloe
printf '%s\n' "$TF_VERSION" >/etc/thinfloe/version
chmod 644 /etc/thinfloe/version
cp /etc/thinfloe/version "${ARTIFACT_DIR}/version"
log_success "Version $TF_VERSION written"

# Create squashfs
log_step "Creating squashfs..."
SQSH_OUTPUT="${ARTIFACT_DIR}/ramroot.sqsh"

# etc/hostname is excluded because podman bind-mounts it (and /etc/hosts)
# into this container, so mksquashfs would read through the mount and pack the
# container's runtime file instead of the image's. The initramfs writes
# /etc/hostname from the machine-id; before this, every machine shipped the
# build container's ID. /etc/hosts is replaced on every boot by
# un-dockerize.service.
#
# /etc/resolv.conf is the image's link to the systemd-resolved stub (the
# Dockerfile installs systemd-resolved without a network for that); podman
# mounts its resolv.conf at the link's target under /run, which is excluded.
# Were it a plain file, podman would have mounted the build host's resolver
# list over it and mksquashfs would pack that.
RESOLV_LINK=../run/systemd/resolve/stub-resolv.conf
[ -L /etc/resolv.conf ] && [ "$(readlink /etc/resolv.conf)" = "$RESOLV_LINK" ] ||
    die "/etc/resolv.conf is not a link to $RESOLV_LINK"
# scripts is excluded because the Makefile bind-mounts ./scripts at /scripts
# for this export; 4.2.0 shipped /scripts/publish.sh and the smoke scripts.
mksquashfs / "$SQSH_OUTPUT" \
    -comp "$SQUASHFS_COMP" \
    -Xcompression-level "$SQUASHFS_COMP_LEVEL" \
    -b 1M \
    -processors "$BUILD_JOBS" \
    -noappend \
    -no-progress \
    -e boot tmp var/tmp artifacts proc sys dev run scripts \
       etc/hostname || die "mksquashfs failed"

log_success "SquashFS created: $(format_bytes "$(stat -c%s "$SQSH_OUTPUT")")"

chmod 644 "${ARTIFACT_DIR}"/*
