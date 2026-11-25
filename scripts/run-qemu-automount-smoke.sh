#!/bin/bash
# Boot with a USB stick and a USB card reader attached and check that
# thinfloe-automount mounts them under /media/tfuser, follows a card swap and
# ejects both.
#
# Usage: scripts/run-qemu-automount-smoke.sh USB_IMAGE KERNEL INITRD LOG TIMEOUT GRACE APPEND
#
# QEMU boots a copy of USB_IMAGE (virtio, kernel and initrd direct) with, on
# a qemu-xhci controller:
#   - a 64 MiB usb-storage stick, serial SMOKEU, with one vfat partition
#     labelled SMOKEU that holds hello.txt;
#   - a usb-bot card reader with two removable LUNs: slot 0 holds CARD1 (vfat
#     on the whole medium, one.txt), slot 1 a medium without a file system;
#   - a 32 MiB usb-storage stick, serial SMOKES, vfat on the whole medium
#     labelled MEDIASHARE: it must not land on FloeDrop's folder.
# The guest runs scripts/smoke-automount-report.sh as systemd.run= and prints
# ok/FAIL lines. When it prints SMOKE-AUTOMOUNT-SWAP-OUT this script takes
# the card out of slot 0 through the QEMU monitor (QMP eject), and when it
# prints SMOKE-AUTOMOUNT-SWAP-IN it puts CARD2 (two.txt) in
# (blockdev-change-medium), as a hand swaps the card in a reader.
#
# Passing requires check-boot-smoke.sh to pass on LOG, both monitor commands
# to succeed, the report to finish (SMOKE-AUTOMOUNT-DONE) and no FAIL line.
# APPEND is the smoke test's serial-console options. Disks live in a temp
# dir under $TMPDIR, removed on exit.

set -uo pipefail

USAGE="usage: run-qemu-automount-smoke.sh USB_IMAGE KERNEL INITRD LOG TIMEOUT GRACE APPEND"
USB_IMAGE="${1:?$USAGE}"
KERNEL="${2:?missing KERNEL}"
INITRD="${3:?missing INITRD}"
LOG="${4:?missing LOG}"
TIMEOUT="${5:?missing TIMEOUT}"
GRACE="${6:?missing GRACE}"
APPEND="${7:?missing APPEND}"

DIR="$(cd "$(dirname "$0")" && pwd)"
for f in "$USB_IMAGE" "$KERNEL" "$INITRD"; do
    [[ -f "$f" ]] || { echo "FAIL: $f missing" >&2; exit 1; }
done

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

export MTOOLS_SKIP_CHECK=1
tmp=$(mktemp -d --tmpdir thinfloe-automount-smoke.XXXXXX) || exit 2
watcher=""
trap 'kill $watcher 2>/dev/null; wait $watcher 2>/dev/null; rm -rf "$tmp"' EXIT
trap 'exit 1' INT TERM HUP

esp_of() {
    local start
    start=$(partx -g -o START -n 1 "$1") || return 1
    echo "$1@@$((start * 512))"
}

# vfat_image FILE SIZE LABEL NAME TEXT: a vfat file system on the whole of
# FILE holding the file NAME with the line TEXT
vfat_image() {
    truncate -s "$2" "$1" || return 1
    mkfs.vfat -n "$3" "$1" >/dev/null || return 1
    echo "$5" >"$tmp/$4" || return 1
    mcopy -i "$1" "$tmp/$4" "::$4"
}

# --- disks -----------------------------------------------------------------
BOOT="$tmp/boot.img"
STICK="$tmp/stick.img"
CARD1="$tmp/card1.img"
CARD2="$tmp/card2.img"
EMPTY="$tmp/empty.img"
SHARE="$tmp/share.img"
cp --sparse=always "$USB_IMAGE" "$BOOT" || exit 2
truncate -s 64M "$STICK" || exit 2
echo 'start=2048, type=c' | sfdisk -q --label dos "$STICK" || exit 2
mkfs.vfat -n SMOKEU --offset 2048 "$STICK" $((63 * 1024)) >/dev/null || exit 2
echo "hello from SMOKEU" >"$tmp/hello.txt" || exit 2
mcopy -i "$STICK@@1M" "$tmp/hello.txt" ::hello.txt || exit 2
vfat_image "$CARD1" 32M CARD1 one.txt "card one" || exit 2
vfat_image "$CARD2" 32M CARD2 two.txt "card two" || exit 2
truncate -s 16M "$EMPTY" || exit 2
vfat_image "$SHARE" 32M MEDIASHARE share.txt "not the folder" || exit 2

ESP=$(esp_of "$BOOT") || exit 2
mcopy -o -i "$ESP" "$DIR/smoke-automount-report.sh" ::smoke-automount-report.sh || exit 2
REPORT='systemd.wants=kernel-command-line.service systemd.run_success_action=none systemd.run_failure_action=none systemd.run="/bin/sh /run/thinfloe/storage/efi/smoke-automount-report.sh"'

# --- the hand at the card reader -------------------------------------------
QMP="$tmp/qmp.sock"
QMP_LOG="$tmp/qmp.log"
cat >"$tmp/qmp.py" <<'PY'
import json, socket, sys

sock = socket.socket(socket.AF_UNIX)
sock.settimeout(30)
sock.connect(sys.argv[1])
f = sock.makefile("rw")


def answer():
    while True:
        msg = json.loads(f.readline())
        if "event" not in msg:
            return msg


def call(cmd, args):
    f.write(json.dumps({"execute": cmd, "arguments": args}) + "\n")
    f.flush()
    return answer()


answer()   # the greeting
call("qmp_capabilities", {})
reply = call(sys.argv[2], json.loads(sys.argv[3]))
print(sys.argv[2], json.dumps(reply))
sys.exit(0 if "return" in reply else 1)
PY
qmp() { python3 -I "$tmp/qmp.py" "$QMP" "$@" >>"$QMP_LOG" 2>&1; }

# swap_card: eject slot 0 after SWAP-OUT, insert CARD2 after SWAP-IN.
# Writes "eject <rc>" and "change <rc>" to $tmp/swap.
swap_card() {
    local n out=""
    for ((n = 0; n < TIMEOUT; n++)); do
        if [[ -z "$out" ]] && grep -aq '^SMOKE-AUTOMOUNT-SWAP-OUT' "$LOG" 2>/dev/null; then
            qmp eject '{"id": "slot0", "force": true}'
            echo "eject $?" >>"$tmp/swap"
            out=1
        fi
        if [[ -n "$out" ]] && grep -aq '^SMOKE-AUTOMOUNT-SWAP-IN' "$LOG" 2>/dev/null; then
            qmp blockdev-change-medium "{\"id\": \"slot0\", \"filename\": \"$CARD2\", \"format\": \"raw\"}"
            echo "change $?" >>"$tmp/swap"
            return
        fi
        sleep 1
    done
}

rm -f "$LOG"
swap_card &
watcher=$!

echo "automount-smoke: booting with a stick and a card reader"
SMOKE_WAIT_FOR='^SMOKE-AUTOMOUNT-DONE' "$DIR/run-qemu-smoke.sh" "$LOG" "$TIMEOUT" "$GRACE" -- \
    qemu-system-x86_64 \
    -machine type=q35,accel=kvm \
    -cpu host,migratable=off \
    -smp "${BUILD_JOBS:-4}" \
    -m "${QEMU_MEMORY:-2G}" \
    -display none \
    -no-reboot \
    -qmp "unix:$QMP,server=on,wait=off" \
    -object rng-random,filename=/dev/urandom,id=rng0 \
    -device virtio-rng-pci,rng=rng0 \
    -net nic,model=virtio -net user,restrict=on \
    -drive "file=$BOOT,format=raw,if=virtio" \
    -device qemu-xhci,id=xhci \
    -drive "if=none,id=ustick,file=$STICK,format=raw,cache=unsafe" \
    -device usb-storage,bus=xhci.0,drive=ustick,serial=SMOKEU \
    -device usb-bot,bus=xhci.0,id=reader,serial=SMOKER \
    -drive "if=none,id=card0,file=$CARD1,format=raw,cache=unsafe" \
    -device scsi-hd,bus=reader.0,scsi-id=0,lun=0,drive=card0,removable=on,id=slot0 \
    -drive "if=none,id=card1,file=$EMPTY,format=raw,cache=unsafe" \
    -device scsi-hd,bus=reader.0,scsi-id=0,lun=1,drive=card1,removable=on,id=slot1 \
    -drive "if=none,id=ushare,file=$SHARE,format=raw,cache=unsafe" \
    -device usb-storage,bus=xhci.0,drive=ushare,serial=SMOKES \
    -kernel "$KERNEL" \
    -initrd "$INITRD" \
    -append "ramroot=tl/ramroot.sqsh $APPEND $REPORT" || exit 2
kill $watcher 2>/dev/null
wait $watcher 2>/dev/null
watcher=""

clean="$tmp/log"
LC_ALL=C sed -E -e 's#\x1b\[[0-9;?!>=]*[ -/]*[@-~]##g' -e 's#\r##g' "$LOG" >"$clean"

if "$DIR/check-boot-smoke.sh" "$LOG"; then
    ok "the boot reaches multi-user.target cleanly"
else
    bad "the boot failed check-boot-smoke.sh"
fi

for step in eject change; do
    if grep -qx "$step 0" "$tmp/swap" 2>/dev/null; then
        ok "the monitor's $step of the card in slot 0 succeeded"
    else
        bad "the monitor's $step of the card in slot 0: $(grep "^$step " "$tmp/swap" 2>/dev/null || echo 'never sent')"
    fi
done
[[ -s "$QMP_LOG" ]] && sed 's/^/         qmp: /' "$QMP_LOG"

grep -a '^SMOKE-AUTOMOUNT: ' "$clean" | sed 's/^SMOKE-AUTOMOUNT: //'
grep -aq '^SMOKE-AUTOMOUNT: FAIL' "$clean" && fail=1
if ! grep -aq '^SMOKE-AUTOMOUNT: ok' "$clean"; then
    bad "the report printed no ok line"
fi
if grep -aq '^SMOKE-AUTOMOUNT-DONE' "$clean"; then
    ok "the VM finished its report"
else
    bad "no SMOKE-AUTOMOUNT-DONE in $LOG: the report did not finish"
fi
if [[ $fail -ne 0 ]]; then
    sed -n '/^SMOKE-AUTOMOUNT-STATE-BEGIN/,/^SMOKE-AUTOMOUNT-STATE-END/p' "$clean" | sed 's/^/         /'
    echo ""
    echo "FAILED - automount smoke (log: $LOG)"
    exit 1
fi
echo ""
echo "PASSED - automount smoke"
