#!/bin/bash
# Write a USB stick with `thinfloe-upgrade write` from a boot server, then
# boot the stick it wrote.
#
# Usage: scripts/run-qemu-write-smoke.sh USB_IMAGE IMAGE_ZST IMAGE_SHA KERNEL INITRD \
#            PREFIX LOG TIMEOUT GRACE APPEND
#
# Run 1: QEMU boots a copy of USB_IMAGE (virtio, kernel and initrd direct)
# with two blank 4 GiB disks attached as USB mass storage (qemu-xhci +
# usb-storage, so TRAN=usb), serials SMOKEA and SMOKEB. The guest runs
# scripts/smoke-write-report.sh as systemd.run=: `drives` lists both, a fresh
# write to B is cancelled with SIGTERM after its first @progress line (exit
# 130, the first MiB of B all zero), and a fresh write with --verify to A
# from the boot server below ends with exit 0 and `@result ok`.
# Run 2: QEMU boots A alone through OVMF and syslinux; it must pass
# check-boot-smoke.sh, and the stick id the write stamped on A must differ
# from the id the boot disk got and survive the boot.
#
# The boot server is python's http.server with Range support (the tool reads
# the image size with a ranged request), on 127.0.0.1 inside a user and
# network namespace, reached by the guest as slirp's 10.0.2.2 -- the pattern
# of run-qemu-netboot-smoke.sh. It serves IMAGE_ZST and IMAGE_SHA from a
# directory of symlinks; the namespace takes it down on every way out.
# Disks live in a temp dir under $TMPDIR, removed on exit. PREFIX names the
# server log (PREFIX-http.log); LOG.1 and LOG.2 are the serial logs.

set -uo pipefail

USAGE="usage: run-qemu-write-smoke.sh USB_IMAGE IMAGE_ZST IMAGE_SHA KERNEL INITRD PREFIX LOG TIMEOUT GRACE APPEND"
USB_IMAGE="${1:?$USAGE}"
IMAGE_ZST="${2:?missing IMAGE_ZST}"
IMAGE_SHA="${3:?missing IMAGE_SHA}"
KERNEL="${4:?missing KERNEL}"
INITRD="${5:?missing INITRD}"
PREFIX="${6:?missing PREFIX}"
LOG="${7:?missing LOG}"
TIMEOUT="${8:?missing TIMEOUT}"
GRACE="${9:?missing GRACE}"
APPEND="${10:?missing APPEND}"

DIR="$(cd "$(dirname "$0")" && pwd)"
for f in "$USB_IMAGE" "$IMAGE_ZST" "$IMAGE_SHA" "$KERNEL" "$INITRD"; do
    [[ -f "$f" ]] || { echo "FAIL: $f missing" >&2; exit 1; }
done

if [[ -z "${TF_WRITE_NS:-}" ]]; then
    # --kill-child: if unshare is killed, the script inside goes with it.
    TF_WRITE_NS=1 exec unshare --user --map-root-user --net --kill-child -- \
        "$0" "$USB_IMAGE" "$IMAGE_ZST" "$IMAGE_SHA" "$KERNEL" "$INITRD" \
        "$PREFIX" "$LOG" "$TIMEOUT" "$GRACE" "$APPEND"
    echo "FAIL: cannot create the user and network namespace (unshare -rn)" >&2
    exit 1
fi

# Inside the namespace from here on.
fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

export MTOOLS_SKIP_CHECK=1
export TMPDIR="${TMPDIR:-/scratch/oetiker/claude-tmp/thinfloe-tests}"
mkdir -p "$TMPDIR" || exit 2
tmp=$(mktemp -d "$TMPDIR/thinfloe-write-smoke.XXXXXX") || exit 2
srv=""
trap 'kill $srv 2>/dev/null; wait $srv 2>/dev/null; rm -rf "$tmp"' EXIT
trap 'exit 1' INT TERM HUP

ip link set lo up || { echo "FAIL: cannot bring up loopback in the namespace" >&2; exit 1; }

# --- the boot server -------------------------------------------------------
mkdir "$tmp/srv"
ln -s "$(realpath "$IMAGE_ZST")" "$tmp/srv/thinfloe-usb.img.zst"
ln -s "$(realpath "$IMAGE_SHA")" "$tmp/srv/thinfloe-usb.img.zst.sha256"
cat >"$tmp/server.py" <<'PY'
import os, re, shutil, sys
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer


class Handler(SimpleHTTPRequestHandler):
    """http.server plus single-range GET, which the tool uses for the size."""

    def do_GET(self):
        m = re.fullmatch(r"bytes=(\d+)-(\d*)", self.headers.get("Range", ""))
        path = self.translate_path(self.path)
        if not m or not os.path.isfile(path):
            return super().do_GET()
        size = os.path.getsize(path)
        first = int(m[1])
        last = min(int(m[2]) if m[2] else size - 1, size - 1)
        if first > last:
            return self.send_error(416)
        self.send_response(206)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Range", f"bytes {first}-{last}/{size}")
        self.send_header("Content-Length", str(last - first + 1))
        self.end_headers()
        with open(path, "rb") as f:
            f.seek(first)
            left = last - first + 1
            while left > 0:
                chunk = f.read(min(left, 1 << 20))
                if not chunk:
                    break
                self.wfile.write(chunk)
                left -= len(chunk)


port, directory = int(sys.argv[1]), sys.argv[2]
ThreadingHTTPServer(("127.0.0.1", port), partial(Handler, directory=directory)).serve_forever()
PY
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
HTTP_LOG="$PREFIX-http.log"
rm -f "$HTTP_LOG"
setpriv --pdeathsig TERM -- python3 -I "$tmp/server.py" "$port" "$tmp/srv" >"$HTTP_LOG" 2>&1 &
srv=$!
up=""
for _ in $(seq 50); do
    curl -fsI "http://127.0.0.1:$port/thinfloe-usb.img.zst" >/dev/null 2>&1 && { up=1; break; }
    sleep 0.1
done
[[ -n "$up" ]] || { echo "FAIL: the HTTP server does not answer inside the namespace" >&2; exit 1; }

# --- disks -----------------------------------------------------------------
BOOT="$tmp/boot.img"
DISK_A="$tmp/a.img"
DISK_B="$tmp/b.img"
cp --sparse=always "$USB_IMAGE" "$BOOT" || exit 2
truncate -s 4G "$DISK_A" "$DISK_B" || exit 2

esp_of() {
    local start
    start=$(partx -g -o START -n 1 "$1") || return 1
    echo "$1@@$((start * 512))"
}

# stick_id <image>: the id in config/stick-id on its ESP, empty when it has none.
stick_id() {
    local esp
    esp=$(esp_of "$1") || return 0
    mtype -i "$esp" ::config/stick-id 2>/dev/null | tr -d '\r\n '
}

# --- run 1: write -----------------------------------------------------------
ESP=$(esp_of "$BOOT") || exit 2
mcopy -o -i "$ESP" "$DIR/smoke-write-report.sh" ::smoke-write-report.sh || exit 2
REPORT="systemd.wants=kernel-command-line.service systemd.run_success_action=none systemd.run_failure_action=none systemd.run=\"/bin/sh /run/thinfloe/storage/efi/smoke-write-report.sh http://10.0.2.2:$port\""

echo "write-smoke: boot 1, writing from http://10.0.2.2:$port"
SMOKE_WAIT_FOR='^SMOKE-WRITE-DONE' "$DIR/run-qemu-smoke.sh" "$LOG.1" "$TIMEOUT" "$GRACE" -- \
    qemu-system-x86_64 \
    -machine type=q35,accel=kvm \
    -cpu host,migratable=off \
    -smp "${BUILD_JOBS:-4}" \
    -m "${QEMU_MEMORY:-2G}" \
    -display none \
    -no-reboot \
    -object rng-random,filename=/dev/urandom,id=rng0 \
    -device virtio-rng-pci,rng=rng0 \
    -netdev user,id=n0 -device virtio-net-pci,netdev=n0 \
    -drive "file=$BOOT,format=raw,if=virtio" \
    -device qemu-xhci,id=xhci \
    -drive "if=none,id=ua,file=$DISK_A,format=raw,cache=unsafe" \
    -device usb-storage,bus=xhci.0,drive=ua,serial=SMOKEA \
    -drive "if=none,id=ub,file=$DISK_B,format=raw,cache=unsafe" \
    -device usb-storage,bus=xhci.0,drive=ub,serial=SMOKEB \
    -kernel "$KERNEL" \
    -initrd "$INITRD" \
    -append "ramroot=tl/ramroot.sqsh $APPEND $REPORT" || exit 2

# Serial output carries colour codes and carriage returns.
clean="$tmp/log1"
LC_ALL=C sed -E -e 's#\x1b\[[0-9;?!>=]*[ -/]*[@-~]##g' -e 's#\r##g' "$LOG.1" >"$clean"
field() { grep -a "^SMOKE-WRITE-$1: " "$clean" | tail -1 | sed "s/^SMOKE-WRITE-$1: //"; }

if "$DIR/check-boot-smoke.sh" "$LOG.1"; then
    ok "boot 1 reaches multi-user.target cleanly"
else
    bad "boot 1 failed check-boot-smoke.sh"
fi

if grep -qaE '^SMOKE-WRITE-DONE' "$clean"; then
    ok "the VM reported on the writes"
else
    bad "no SMOKE-WRITE-DONE in $LOG.1: the report did not run"
fi

if [[ "$(field DRIVES-RC)" == 0 && "$(field DRIVE-A)" == /dev/* && "$(field DRIVE-B)" == /dev/* ]]; then
    ok "drives lists both USB disks by serial (A $(field DRIVE-A), B $(field DRIVE-B))"
else
    bad "drives did not list SMOKEA and SMOKEB (rc $(field DRIVES-RC), A '$(field DRIVE-A)', B '$(field DRIVE-B)')"
    sed -n '/SMOKE-WRITE-DRIVES-BEGIN/,/SMOKE-WRITE-DRIVES-END/p' "$clean" | sed 's/^/         /'
fi

if [[ "$(field CANCEL-SAW-PROGRESS)" == yes && "$(field CANCEL-RC)" == 130 ]]; then
    ok "SIGTERM after the first @progress ends the write with exit 130"
else
    bad "cancel: progress seen '$(field CANCEL-SAW-PROGRESS)', exit '$(field CANCEL-RC)', result '$(field CANCEL-RESULT)'"
fi
if [[ "$(field CANCEL-BEFORE-NONZERO)" =~ ^[0-9]+$ && "$(field CANCEL-BEFORE-NONZERO)" -gt 0 \
      && "$(field CANCEL-NONZERO)" == 0 ]]; then
    ok "the first MiB of the stick held data while it was written ($(field CANCEL-BEFORE-NONZERO) non-zero bytes), and is all zero after the cancel"
else
    bad "first MiB of the cancelled stick: non-zero bytes before the cancel '$(field CANCEL-BEFORE-NONZERO)' (need > 0), after '$(field CANCEL-NONZERO)' (need 0)"
fi

if [[ "$(field WRITE-RC)" == 0 && "$(field WRITE-RESULT)" == '@result ok'* ]]; then
    ok "write --mode fresh --verify exits 0 with '$(field WRITE-RESULT)'"
else
    bad "write --verify: exit '$(field WRITE-RC)', result '$(field WRITE-RESULT)'"
    sed -n '/SMOKE-WRITE-WRITE-TAIL-BEGIN/,/SMOKE-WRITE-WRITE-TAIL-END/p' "$clean" | sed 's/^/         /'
fi
if [[ "$(field WRITE-PROGRESS-LINES)" =~ ^[0-9]+$ && "$(field WRITE-PROGRESS-LINES)" -gt 0 ]]; then
    ok "the write reported progress ($(field WRITE-PROGRESS-LINES) lines; steps: $(field WRITE-STEPS))"
else
    bad "the write printed no @progress lines"
fi
if [[ " $(field WRITE-STEPS) " == *" verify "* ]]; then
    ok "the write ran the verify step"
else
    bad "no verify step in '$(field WRITE-STEPS)'"
fi

if grep -qaE 'GET /thinfloe-usb\.img\.zst\.sha256 ' "$HTTP_LOG" \
   && grep -qaE 'GET /thinfloe-usb\.img\.zst ' "$HTTP_LOG"; then
    ok "the image and its checksum came from the boot server"
else
    bad "the boot server never saw both requests (log: $HTTP_LOG)"
fi

# --- run 2: boot the written stick ------------------------------------------
BOOT_ID=$(stick_id "$BOOT")
A_ID=$(stick_id "$DISK_A")
if [[ -n "$A_ID" && -n "$BOOT_ID" && "$A_ID" != "$BOOT_ID" ]]; then
    ok "the written stick has its own id ($A_ID), not the boot disk's ($BOOT_ID)"
else
    bad "stick ids: written '$A_ID', boot disk '$BOOT_ID' (need two different ones)"
fi

# Console options and the stick id, as the two-sticks smoke does for LABEL current.
ESP_A=$(esp_of "$DISK_A") || exit 2
for cfg in ::syslinux.cfg ::efi/boot/syslinux.cfg; do
    mtype -i "$ESP_A" "$cfg" | tr -d '\r' >"$tmp/cfg" || { bad "cannot read $cfg on the written stick"; continue; }
    # The tool's own stamp, before this script edits the file.
    if [[ -n "$A_ID" ]] && grep -qE "^[[:space:]]+APPEND .*thinfloe\.stick=$A_ID( |\$)" "$tmp/cfg"; then
        ok "the write stamped thinfloe.stick=$A_ID into $cfg"
    else
        bad "the write did not stamp thinfloe.stick=$A_ID into $cfg"
    fi
    awk -v append="$APPEND" -v id="$A_ID" '
        /^LABEL /   { in_current = ($2 == "current") }
        /^[[:space:]]+APPEND / && !/ramroot=[a-z]+:\/\// {
            if (in_current) $0 = "    APPEND ramroot=tl/ramroot.sqsh " append
            gsub(/ thinfloe\.stick=[^ ]*/, "")
            if (id != "") $0 = $0 " thinfloe.stick=" id
        }
        { print }' "$tmp/cfg" >"$tmp/cfg.new"
    mcopy -o -i "$ESP_A" "$tmp/cfg.new" "$cfg" || bad "cannot write $cfg on the written stick"
done

echo "write-smoke: boot 2, the written stick alone"
"$DIR/run-qemu-smoke.sh" "$LOG.2" "$TIMEOUT" "$GRACE" -- \
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
    -drive "if=none,id=a,file=$DISK_A,format=raw" \
    -device virtio-blk-pci,drive=a,bootindex=0 || exit 2

if "$DIR/check-boot-smoke.sh" "$LOG.2"; then
    ok "boot 2, from the written stick alone, reaches multi-user.target cleanly"
else
    bad "boot 2 failed check-boot-smoke.sh"
fi
if [[ -n "$A_ID" && "$(stick_id "$DISK_A")" == "$A_ID" ]]; then
    ok "the written stick kept its id through its first boot"
else
    bad "the written stick's id after boot 2 is '$(stick_id "$DISK_A")', not '$A_ID'"
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - write smoke (logs: $LOG.1, $LOG.2, $HTTP_LOG)"
    exit 1
fi
echo ""
echo "PASSED - write smoke"
