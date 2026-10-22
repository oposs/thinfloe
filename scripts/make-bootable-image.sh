#!/bin/bash
# make-bootable-image.sh - Create bootable USB image

set -euo pipefail

source /scripts/lib/common.sh

EFI_PARTITION_SIZE="${EFI_PARTITION_SIZE:-3000}"
VERSION="${VERSION:-dev}"
BUILD_ID="${BUILD_ID:-unknown}"
PROJECT_NAME="${PROJECT_NAME:-thinfloe}"

ROOTFS_DIR="/artifacts/rootfs"
IMAGE_DIR="/artifacts/images"
CONFIG_DIR="/configs"

log_info "Creating bootable image"
log_info "  Version:    $VERSION"
log_info "  Build ID:   $BUILD_ID"
log_info "  EFI Size:   ${EFI_PARTITION_SIZE}MB"

ensure_dir "$IMAGE_DIR"

# Verify input files exist
verify_file "$ROOTFS_DIR/vmlinuz" 5000000
verify_file "$ROOTFS_DIR/initrd.img" 10000000
verify_file "$ROOTFS_DIR/ramroot.sqsh" 100000000
verify_file "$CONFIG_DIR/syslinux.cfg" 100

IMG="$IMAGE_DIR/${PROJECT_NAME}-usb.img"
MB=$((1024*1024))

log_step "Creating empty image file..."
rm -f "$IMG"
dd if=/dev/zero bs=1M seek=$((2+EFI_PARTITION_SIZE)) count=1 of="$IMG" status=none

log_step "Creating GPT partition table..."
sgdisk --new=1::+$((EFI_PARTITION_SIZE))M \
       --change-name=1:EFI-SYSTEM \
       --typecode=1:ef00 \
       --attributes=1:set:2 \
       --hybrid=1 "$IMG" > /dev/null

log_step "Formatting FAT32 filesystem..."
mkfs.fat -v -I -S 512 -s 1 --offset 2048 -n EFI-SYSTEM "$IMG" $((EFI_PARTITION_SIZE*1024)) > /dev/null

log_step "Installing SYSLINUX bootloader..."
MD="mmd -i ${IMG}@@1M"
CP="mcopy -i ${IMG}@@1M"

$MD ::efi
$MD ::efi/boot
$CP /usr/lib/SYSLINUX.EFI/efi64/syslinux.efi ::efi/boot/bootx64.efi
$CP /usr/lib/syslinux/modules/efi64/ldlinux.e64 ::efi/boot/ldlinux.e64

# Copy required SYSLINUX modules for menu support
# Place in root directory next to syslinux.cfg
$CP /usr/lib/syslinux/modules/efi64/libcom32.c32 ::/
$CP /usr/lib/syslinux/modules/efi64/libutil.c32 ::/
$CP /usr/lib/syslinux/modules/efi64/menu.c32 ::/
$CP /usr/lib/syslinux/modules/efi64/vesamenu.c32 ::efi/boot/
$CP /usr/lib/syslinux/modules/efi64/libcom32.c32 ::efi/boot/
$CP /usr/lib/syslinux/modules/efi64/libutil.c32 ::efi/boot/

dd bs=440 count=1 conv=notrunc if=/usr/lib/syslinux/mbr/gptmbr.bin of="$IMG" status=none
syslinux --offset $MB --install "$IMG"

# Sync backup boot sector with primary (syslinux modifies primary but not backup)
# FAT32 backup boot sector is at sector 6 of the partition
log_step "Syncing FAT32 backup boot sector..."
dd if="$IMG" of="$IMG" bs=512 count=1 skip=2048 seek=2054 conv=notrunc status=none

log_step "Copying boot files..."
$CP "$CONFIG_DIR/syslinux.cfg" ::syslinux.cfg
$CP "$CONFIG_DIR/syslinux.cfg" ::efi/boot/syslinux.cfg
$MD ::tl
$CP "$ROOTFS_DIR/vmlinuz" ::tl/vmlinuz
$CP "$ROOTFS_DIR/initrd.img" ::tl/initrd.img
$CP "$ROOTFS_DIR/ramroot.sqsh" ::tl/ramroot.sqsh

log_success "Bootable image created: $(format_bytes $(stat -c%s "$IMG"))"
