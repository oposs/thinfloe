#!/bin/bash
# Boot with two ThinFloe sticks attached and check that every boot mounts the
# ESP of the stick the firmware booted from.
#
# Usage: scripts/run-qemu-two-sticks-smoke.sh USB_IMAGE WORK_PREFIX LOG TIMEOUT GRACE APPEND
#
# Every stick has a partition named EFI-SYSTEM, and the initramfs used to
# mount /dev/disk/by-partlabel/EFI-SYSTEM, which udev points at either one.
# The kernel came from one stick; the squashfs, the home, the add-ons and the
# saves from whichever the link named, differently from boot to boot.
#
# Two copies of USB_IMAGE: stick A (vda) carries stick id
# aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa in config/stick-id and on its APPEND
# lines, as a stick that booted this release before; stick B (vdb) is fresh,
# as written by `make image`. QEMU boots B through OVMF and syslinux
# (bootindex), three times. Passing requires:
#   - each boot passes check-boot-smoke.sh,
#   - each boot mounted /dev/vdb1, and boots 2 and 3 saw the id boot 1 gave B,
#     which is not A's,
#   - afterwards both of B's syslinux.cfg copies carry thinfloe.stick=<that
#     id>, and A's config/stick-id and syslinux.cfg copies are unchanged.
#
# APPEND (the smoke test's serial-console options) and the report call replace
# LABEL current's own APPEND options on both sticks, so the boot is visible in
# LOG.<n>.

set -uo pipefail

USB_IMAGE="${1:?usage: run-qemu-two-sticks-smoke.sh USB_IMAGE WORK_PREFIX LOG TIMEOUT GRACE APPEND}"
PREFIX="${2:?missing WORK_PREFIX}"
LOG="${3:?missing LOG}"
TIMEOUT="${4:?missing TIMEOUT}"
GRACE="${5:?missing GRACE}"
APPEND="${6:?missing APPEND}"

DIR="$(cd "$(dirname "$0")" && pwd)"
fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

A_ID=aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa
A_IMG="$PREFIX-a.img"
B_IMG="$PREFIX-b.img"
export MTOOLS_SKIP_CHECK=1

tmp=$(mktemp -d "$(dirname "$PREFIX")/thinfloe-two-sticks.XXXXXX") || exit 2
trap 'rm -rf "$tmp"' EXIT

esp_of() {
    local start
    start=$(partx -g -o START -n 1 "$1") || return 1
    echo "$1@@$((start * 512))"
}

REPORT='systemd.wants=kernel-command-line.service systemd.run_success_action=none systemd.run_failure_action=none systemd.run="/bin/sh /run/thinfloe/storage/efi/smoke-stick-report.sh"'

# prepare <image> <stick id or empty>
prepare() {
    local img=$1 id=$2 esp cfg
    cp --sparse=always "$USB_IMAGE" "$img" || return 1
    esp=$(esp_of "$img") || return 1
    mcopy -o -i "$esp" "$DIR/smoke-stick-report.sh" ::smoke-stick-report.sh || return 1
    if [[ -n "$id" ]]; then
        echo "$id" >"$tmp/stick-id"
        mmd -i "$esp" ::config 2>/dev/null
        mcopy -o -i "$esp" "$tmp/stick-id" ::config/stick-id || return 1
    fi
    for cfg in ::syslinux.cfg ::efi/boot/syslinux.cfg; do
        mtype -i "$esp" "$cfg" | tr -d '\r' >"$tmp/cfg" || return 1
        awk -v append="$APPEND" -v report="$REPORT" -v id="$id" '
            /^LABEL /   { in_current = ($2 == "current") }
            /^[[:space:]]+APPEND / && !/ramroot=[a-z]+:\/\// {
                if (in_current) $0 = "    APPEND ramroot=tl/ramroot.sqsh " append " " report
                if (id != "") $0 = $0 " thinfloe.stick=" id
            }
            { print }' "$tmp/cfg" >"$tmp/cfg.new"
        grep -q 'smoke-stick-report' "$tmp/cfg.new" || { echo "no LABEL current in $cfg" >&2; return 1; }
        mcopy -o -i "$esp" "$tmp/cfg.new" "$cfg" || return 1
    done
}

# snapshot <image> <dir>: the stick-id and both syslinux.cfg copies, raw
snapshot() {
    local esp
    esp=$(esp_of "$1") || return 1
    mkdir -p "$2"
    mtype -i "$esp" ::config/stick-id >"$2/stick-id" 2>/dev/null
    mtype -i "$esp" ::syslinux.cfg >"$2/syslinux.cfg"
    mtype -i "$esp" ::efi/boot/syslinux.cfg >"$2/efi-syslinux.cfg"
}

prepare "$A_IMG" "$A_ID" || { echo "could not prepare stick A" >&2; exit 2; }
prepare "$B_IMG" "" || { echo "could not prepare stick B" >&2; exit 2; }
snapshot "$A_IMG" "$tmp/a-before" || exit 2

seen=()
for n in 1 2 3; do
    echo "two-sticks: boot $n"
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
        -drive "if=none,id=a,file=$A_IMG,format=raw" \
        -device virtio-blk-pci,drive=a,bootindex=1 \
        -drive "if=none,id=b,file=$B_IMG,format=raw" \
        -device virtio-blk-pci,drive=b,bootindex=0 || exit 2

    if "$DIR/check-boot-smoke.sh" "$LOG.$n"; then
        ok "boot $n reaches multi-user.target cleanly"
    else
        bad "boot $n failed check-boot-smoke.sh"
    fi

    line=$(tr -d '\r' <"$LOG.$n" | grep -a '^SMOKE-STICK: ' | tail -1)
    read -r _ dev id <<<"$line"
    seen+=("${id:-none}")
    if [[ "$dev" == /dev/vdb1 ]]; then
        ok "boot $n mounted /dev/vdb1, the stick it booted from (stick id ${id:-none})"
    else
        bad "boot $n mounted '${dev:-nothing}' (stick id ${id:-none}), not /dev/vdb1"
    fi
done

if [[ "${seen[0]}" != none && "${seen[0]}" != "$A_ID" \
      && "${seen[1]}" == "${seen[0]}" && "${seen[2]}" == "${seen[0]}" ]]; then
    ok "stick B got its own id at boot 1 and kept it: ${seen[0]}"
else
    bad "stick ids seen at boots 1-3: ${seen[*]}"
fi

snapshot "$B_IMG" "$tmp/b-after" || exit 2
for f in syslinux.cfg efi-syslinux.cfg; do
    if [[ "${seen[0]}" != none ]] \
       && tr -d '\r' <"$tmp/b-after/$f" \
          | grep -q "ramroot=tl/ramroot.sqsh .* thinfloe.stick=${seen[0]}\$"; then
        ok "stick B's $f carries thinfloe.stick=${seen[0]}"
    else
        bad "stick B's $f lacks thinfloe.stick=${seen[0]}"
    fi
done

snapshot "$A_IMG" "$tmp/a-after" || exit 2
if diff -r "$tmp/a-before" "$tmp/a-after" >/dev/null; then
    ok "stick A's stick-id and syslinux.cfg copies are unchanged"
else
    bad "stick A changed: $(diff -r "$tmp/a-before" "$tmp/a-after" | head -5)"
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - two-sticks boot smoke"
    exit 1
fi
echo ""
echo "PASSED - two-sticks boot smoke"
