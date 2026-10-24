#!/bin/bash
# Boot a stick that carries a ticked add-on and check it is installed before
# the session starts.
#
# Usage: scripts/run-qemu-addons-smoke.sh USB_IMAGE WORK_IMAGE KERNEL INITRD LOG TIMEOUT GRACE APPEND
#
# The stick is set up as an earlier boot leaves it: a tiny test .deb stored as
# addons/90-smoke/current.deb with its current.meta on the ESP, and a home
# archive that ticks "90-smoke" (~/.config/thinfloe/addons) and carries the
# tile ~/.config/thinfloe/menu/90-smoke.desktop with a manual http: source.
# QEMU has no network route to that source, so the file can only come from the
# stick. Passing requires:
#   - a clean boot to multi-user.target (check-boot-smoke.sh), which also
#     fails on any "Found ordering cycle",
#   - thinfloe-addons.service finished before getty@tty1.service started,
#   - /usr/share/thinfloe-smoke-addon/ok exists,
#   - the status file says 90-smoke is installed.
# The VM reports the last two on the serial console through
# scripts/smoke-addons-report.sh, which runs as systemd.run=.

set -uo pipefail

USB_IMAGE="${1:?usage: run-qemu-addons-smoke.sh USB_IMAGE WORK_IMAGE KERNEL INITRD LOG TIMEOUT GRACE APPEND}"
WORK="${2:?missing WORK_IMAGE}"
KERNEL="${3:?missing KERNEL}"
INITRD="${4:?missing INITRD}"
LOG="${5:?missing LOG}"
TIMEOUT="${6:?missing TIMEOUT}"
GRACE="${7:?missing GRACE}"
APPEND="${8:?missing APPEND}"

DIR="$(cd "$(dirname "$0")" && pwd)"
fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

cp --sparse=always "$USB_IMAGE" "$WORK" || exit 2
start=$(partx -g -o START -n 1 "$WORK") || exit 2
ESP="$WORK@@$((start * 512))"
export MTOOLS_SKIP_CHECK=1

tmp=$(mktemp -d "$(dirname "$WORK")/thinfloe-addons-smoke.XXXXXX") || exit 2
trap 'rm -rf "$tmp"' EXIT

# The add-on: one file.
pkg="$tmp/pkg"
mkdir -p "$pkg/DEBIAN" "$pkg/usr/share/thinfloe-smoke-addon" || exit 2
cat >"$pkg/DEBIAN/control" <<'CTL'
Package: thinfloe-smoke-addon
Version: 1.0
Architecture: all
Maintainer: ThinFloe smoke test <nobody@invalid>
Description: add-on of the ThinFloe smoke test
CTL
echo ok >"$pkg/usr/share/thinfloe-smoke-addon/ok"
dpkg-deb --build --root-owner-group "$pkg" "$tmp/current.deb" >/dev/null || exit 2

cat >"$tmp/current.meta" <<META
version=1.0
fingerprint=smoke-1.0
url=http://smoke.invalid/thinfloe-smoke-addon.deb
format=deb
package=thinfloe-smoke-addon
fetched=$(date +%s)
META

mmd -i "$ESP" ::addons ::addons/90-smoke || exit 2
mcopy -i "$ESP" "$tmp/current.deb" "$tmp/current.meta" ::addons/90-smoke/ || exit 2

# A home that is native 4.x (the marker), so no migration runs.
home="$tmp/home"
mkdir -p "$home/.config/thinfloe/menu" || exit 2
echo 1 >"$home/.thinfloe-home-version"
echo 90-smoke >"$home/.config/thinfloe/addons"
cat >"$home/.config/thinfloe/menu/90-smoke.desktop" <<'TILE'
[Desktop Entry]
Type=Application
Name=Smoke add-on
Exec=true
X-TF-Source-URL=http://smoke.invalid/thinfloe-smoke-addon.deb
TILE
tar -C "$home" -czf "$tmp/home.tar.gz" . || exit 2
mcopy -o -i "$ESP" "$tmp/home.tar.gz" ::home.tar.gz || exit 2
mcopy -o -i "$ESP" "$DIR/smoke-addons-report.sh" ::smoke-report.sh || exit 2

REPORT='systemd.wants=kernel-command-line.service systemd.run_success_action=none systemd.run_failure_action=none systemd.run="/bin/sh /run/thinfloe/storage/efi/smoke-report.sh"'

SMOKE_WAIT_FOR='^SMOKE-ADDON-DONE' "$DIR/run-qemu-smoke.sh" "$LOG" "$TIMEOUT" "$GRACE" -- \
    qemu-system-x86_64 \
    -machine type=q35,accel=kvm \
    -cpu host,migratable=off \
    -smp "${BUILD_JOBS:-4}" \
    -m "${QEMU_MEMORY:-2G}" \
    -display none \
    -no-reboot \
    -object rng-random,filename=/dev/urandom,id=rng0 \
    -device virtio-rng-pci,rng=rng0 \
    -net nic,model=virtio -net user,restrict=on \
    -drive "file=$WORK,format=raw,if=virtio" \
    -kernel "$KERNEL" \
    -initrd "$INITRD" \
    -append "ramroot=tl/ramroot.sqsh $APPEND $REPORT" || exit 2

if "$DIR/check-boot-smoke.sh" "$LOG"; then
    ok "the boot reaches multi-user.target cleanly, with no ordering cycle"
else
    bad "the boot failed check-boot-smoke.sh"
fi

# Serial output carries colour codes and carriage returns.
clean="$tmp/log"
LC_ALL=C sed -E -e 's#\x1b\[[0-9;?!>=]*[ -/]*[@-~]##g' -e 's#\r##g' "$LOG" >"$clean"

fin=$(grep -naE 'Finished thinfloe-addons\.service' "$clean" | head -1 | cut -d: -f1)
getty=$(grep -naE 'Started getty@tty1\.service' "$clean" | head -1 | cut -d: -f1)
if [[ -z "$fin" ]]; then
    bad "no 'Finished thinfloe-addons.service' in $LOG"
elif [[ -z "$getty" ]]; then
    bad "no 'Started getty@tty1.service' in $LOG"
elif (( fin < getty )); then
    ok "thinfloe-addons.service finished before getty@tty1.service started"
else
    bad "getty@tty1.service started (line $getty) before thinfloe-addons.service finished (line $fin)"
fi

if grep -qaE '^SMOKE-ADDON-DONE' "$clean"; then
    ok "the VM reported on the add-ons"
else
    bad "no SMOKE-ADDON-DONE in $LOG: the report did not run"
fi

if grep -qaE '^SMOKE-ADDON-FILE: present' "$clean"; then
    ok "/usr/share/thinfloe-smoke-addon/ok exists"
else
    bad "/usr/share/thinfloe-smoke-addon/ok is missing"
fi

if grep -qaE '^90-smoke\|1\|installed\|1\.0\|' "$clean"; then
    ok "the status file says 90-smoke is installed"
else
    bad "the status file does not say 90-smoke is installed:"
    sed -n '/SMOKE-ADDON-STATUS-BEGIN/,/SMOKE-ADDON-STATUS-END/p' "$clean" | sed 's/^/         /'
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - add-ons boot smoke (full log: $LOG)"
    exit 1
fi
echo ""
echo "PASSED - add-ons boot smoke"
