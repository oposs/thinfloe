#!/bin/bash
# Boot a stick larger than its image twice and check that the first boot grows
# the ESP to the cap and the second leaves it alone.
#
# Usage: scripts/run-qemu-grow-smoke.sh USB_IMAGE WORK_PREFIX LOG TIMEOUT GRACE APPEND
#
# A copy of USB_IMAGE (a 3000 MB ESP) is extended to 8 GiB, as if written to
# an 8 GB stick, and a file of random data is put on its ESP. QEMU boots it
# through OVMF and syslinux, twice. Passing requires:
#   - each boot passes check-boot-smoke.sh,
#   - boot 1 logs the grown partition and the enlarged FAT32, boot 2 neither,
#   - afterwards partition 1 is 12582912 sectors (6 GiB, TF_ESP_CAP_SECTORS)
#     at its old start, the MBR is still hybrid, the FAT32 fills the
#     partition and fsck.fat -n finds nothing,
#   - the random file, home.tar.gz (when the image has one) and every file
#     under tl/ are unchanged.
#
# APPEND (the smoke test's serial-console options) and the report call replace
# LABEL current's own APPEND options, so the boot is visible in LOG.<n>.

set -uo pipefail

USB_IMAGE="${1:?usage: run-qemu-grow-smoke.sh USB_IMAGE WORK_PREFIX LOG TIMEOUT GRACE APPEND}"
PREFIX="${2:?missing WORK_PREFIX}"
LOG="${3:?missing LOG}"
TIMEOUT="${4:?missing TIMEOUT}"
GRACE="${5:?missing GRACE}"
APPEND="${6:?missing APPEND}"

DIR="$(cd "$(dirname "$0")" && pwd)"
fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

CAP=12582912
IMG="$PREFIX.img"
export MTOOLS_SKIP_CHECK=1

tmp=$(mktemp -d "$(dirname "$PREFIX")/thinfloe-grow.XXXXXX") || exit 2
trap 'rm -rf "$tmp"' EXIT

esp_of() {
    local start
    start=$(partx -g -o START -n 1 "$1") || return 1
    echo "$1@@$((start * 512))"
}

# mbr_types <image>: the type bytes of the four MBR entries
mbr_types() {
    local off
    for off in 450 466 482 498; do
        od -An -tx1 -j"$off" -N1 "$1" | tr -d ' \n'
        echo -n " "
    done
}

# sums <image>: md5 of the random file, home.tar.gz and every file in tl/
sums() {
    local esp f
    esp=$(esp_of "$1") || return 1
    for f in ::smoke-grow.bin ::home.tar.gz $(mdir -b -i "$esp" ::tl 2>/dev/null); do
        mtype -i "$esp" "$f" >/dev/null 2>&1 || continue
        echo "$f $(mcopy -n -i "$esp" "$f" - | md5sum | cut -d' ' -f1)"
    done
}

REPORT='systemd.wants=kernel-command-line.service systemd.run_success_action=none systemd.run_failure_action=none systemd.run="/bin/sh /run/thinfloe/storage/efi/smoke-stick-report.sh"'

prepare() {
    local esp cfg
    cp --sparse=always "$USB_IMAGE" "$IMG" || return 1
    truncate -s 8G "$IMG" || return 1
    esp=$(esp_of "$IMG") || return 1
    mcopy -o -i "$esp" "$DIR/smoke-stick-report.sh" ::smoke-stick-report.sh || return 1
    head -c 4M /dev/urandom >"$tmp/smoke-grow.bin"
    mcopy -o -i "$esp" "$tmp/smoke-grow.bin" ::smoke-grow.bin || return 1
    for cfg in ::syslinux.cfg ::efi/boot/syslinux.cfg; do
        mtype -i "$esp" "$cfg" | tr -d '\r' >"$tmp/cfg" || return 1
        awk -v append="$APPEND" -v report="$REPORT" '
            /^LABEL /   { in_current = ($2 == "current") }
            /^[[:space:]]+APPEND / && in_current && !/ramroot=[a-z]+:\/\// {
                $0 = "    APPEND ramroot=tl/ramroot.sqsh " append " " report
            }
            { print }' "$tmp/cfg" >"$tmp/cfg.new"
        grep -q 'smoke-stick-report' "$tmp/cfg.new" || { echo "no LABEL current in $cfg" >&2; return 1; }
        mcopy -o -i "$esp" "$tmp/cfg.new" "$cfg" || return 1
    done
}

prepare || { echo "could not prepare the stick" >&2; exit 2; }
start_before=$(partx -g -o START -n 1 "$IMG")
size_before=$(partx -g -o SECTORS -n 1 "$IMG")
mbr_before=$(mbr_types "$IMG")
sums "$IMG" >"$tmp/sums-before" || exit 2
echo "grow: partition 1 starts at $start_before, $size_before sectors; MBR types: $mbr_before"

for n in 1 2; do
    echo "grow: boot $n"
    SMOKE_WAIT_FOR='^SMOKE-STICK-DONE' "$DIR/run-qemu-smoke.sh" "$LOG.$n" "$TIMEOUT" "$GRACE" -- \
        qemu-system-x86_64 \
        -machine type=q35,accel=kvm \
        -cpu host,migratable=off \
        -smp "${BUILD_JOBS:-4}" \
        -m "${QEMU_MEMORY:-2G}" \
        -bios /usr/share/ovmf/OVMF.fd \
        -display none \
        -no-reboot \
        -object rng-random,filename=/dev/urandom,id=rng0 \
        -device virtio-rng-pci,rng=rng0 \
        -net nic,model=virtio -net user,restrict=on \
        -drive "if=none,id=a,file=$IMG,format=raw" \
        -device virtio-blk-pci,drive=a,bootindex=0 || exit 2

    if "$DIR/check-boot-smoke.sh" "$LOG.$n"; then
        ok "boot $n reaches multi-user.target cleanly"
    else
        bad "boot $n failed check-boot-smoke.sh"
    fi
    tr -d '\r' <"$LOG.$n" | grep -a 'thinfloe: \(boot stick\|enlarging\|could not grow\|the boot stick\)\|fat32expander:\|sgdisk:' | sed "s/^/    boot $n: /"
done

if tr -d '\r' <"$LOG.1" | grep -aq 'thinfloe: boot stick partition grown'; then
    ok "boot 1 grew the partition"
else
    bad "boot 1 did not log 'boot stick partition grown'"
fi
if tr -d '\r' <"$LOG.1" | grep -aq 'thinfloe: boot stick enlarged'; then
    ok "boot 1 enlarged the FAT32"
else
    bad "boot 1 did not log 'boot stick enlarged'"
fi
if tr -d '\r' <"$LOG.2" | grep -aq 'thinfloe: \(boot stick partition grown\|enlarging\)'; then
    bad "boot 2 grew or enlarged again"
else
    ok "boot 2 left the ESP alone"
fi

start_after=$(partx -g -o START -n 1 "$IMG")
sectors_after=$(partx -g -o SECTORS -n 1 "$IMG")
if [[ "$start_after" == "$start_before" && "$sectors_after" == "$CAP" ]]; then
    ok "partition 1 is $CAP sectors at sector $start_after"
else
    bad "partition 1 is $sectors_after sectors at sector $start_after (before: start $start_before)"
fi

mbr_after=$(mbr_types "$IMG")
if [[ "$mbr_after" == "$mbr_before" && "$mbr_after" != "ee 00 00 00 " ]]; then
    ok "the MBR is still hybrid ($mbr_after)"
else
    bad "MBR types were '$mbr_before', now '$mbr_after'"
fi

dd if="$IMG" of="$tmp/esp.img" bs=1M skip=$((start_after / 2048)) \
   count=$((sectors_after / 2048)) conv=sparse status=none || exit 2
# The smoke run ends the VM while the ESP is mounted, so Linux's dirty bit
# (bit 0 of byte 65) is still set and fsck.fat reports it as a difference
# from the backup boot sector. Clear it in the copy; anything else counts.
printf '\0' | dd of="$tmp/esp.img" bs=1 seek=65 conv=notrunc status=none
fat_sectors=$(od -An -tu4 -j32 -N4 "$tmp/esp.img" | tr -d ' \n')
if (( sectors_after - fat_sectors < 2048 )); then
    ok "the FAT32 has $fat_sectors sectors of $sectors_after"
else
    bad "the FAT32 has $fat_sectors sectors, its partition $sectors_after"
fi
if fsck.fat -n "$tmp/esp.img" >"$tmp/fsck.log" 2>&1; then
    ok "fsck.fat -n finds nothing"
else
    bad "fsck.fat -n: $(head -5 "$tmp/fsck.log" | tr '\n' ' ')"
fi
rm -f "$tmp/esp.img"

sums "$IMG" >"$tmp/sums-after" || exit 2
if diff "$tmp/sums-before" "$tmp/sums-after" >/dev/null; then
    ok "unchanged: $(cut -d' ' -f1 "$tmp/sums-before" | tr '\n' ' ')"
else
    bad "files changed: $(diff "$tmp/sums-before" "$tmp/sums-after" | head -6 | tr '\n' ' ')"
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - grow boot smoke"
    exit 1
fi
echo ""
echo "PASSED - grow boot smoke"
