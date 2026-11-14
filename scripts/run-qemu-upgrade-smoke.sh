#!/bin/bash
# Boot a staged upgrade through the firmware and syslinux, and check the
# initramfs rotated it.
#
# Usage: scripts/run-qemu-upgrade-smoke.sh USB_IMAGE WORK_IMAGE LOG TIMEOUT GRACE APPEND
#
# The stick is set up the way thinfloe-upgrade leaves it: tl.new/ holds a
# complete image and both syslinux.cfg copies say DEFAULT new. QEMU then boots
# with OVMF, so syslinux - not -kernel - picks the kernel. "LABEL new" gets
# APPEND (the smoke test's serial-console options) in place of its own, so the
# boot is visible in LOG; everything else about the entry is the shipped one.
#
# The first boot after an upgrade used to load tl/'s kernel while the
# initramfs rotated tl.new into tl/, so the old kernel ran on the new
# squashfs and no module outside the initramfs could load. Passing requires:
#   - a clean boot to multi-user.target (check-boot-smoke.sh),
#   - the rotation's "Complete" line in the log,
#   - a 3.x home.tar.gz on the stick is migrated: one "thinfloe: renamed"
#     line per renamed name (environment, sessionrc, host keys, menu dir),
#   - the 3.x home ticks ThinLinc (it has .thinlinc, not .config/oxulnk); the
#     VM has no network, so the check's status for it says "failed",
#   - afterwards: no tl.new, tl.old holding the former tl, and both
#     syslinux.cfg copies back on DEFAULT current,
#   - the 3.x home is on the legacy layout, its ~/wifi.yaml is installed, and
#     smoke-home-report.sh migrates it and saves it (the smoke boots stop at
#     multi-user.target, so no session does that),
#   - a second boot of the same stick (LABEL current) restores the new layout,
#     installs generated/wifi.yaml, carries the keyboard across and finds the
#     old files in the attic, without renaming a 3.x file again.

set -uo pipefail

USB_IMAGE="${1:?usage: run-qemu-upgrade-smoke.sh USB_IMAGE WORK_IMAGE LOG TIMEOUT GRACE APPEND}"
WORK="${2:?missing WORK_IMAGE}"
LOG="${3:?missing LOG}"
TIMEOUT="${4:?missing TIMEOUT}"
GRACE="${5:?missing GRACE}"
APPEND="${6:?missing APPEND}"

DIR="$(cd "$(dirname "$0")" && pwd)"
fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

cp --sparse=always "$USB_IMAGE" "$WORK" || exit 2
start=$(partx -g -o START -n 1 "$WORK") || exit 2
ESP="$WORK@@$((start * 512))"
export MTOOLS_SKIP_CHECK=1

tmp=$(mktemp -d "$(dirname "$WORK")/thinfloe-upgrade-smoke.XXXXXX") || exit 2
trap 'rm -rf "$tmp"' EXIT

# Stage tl.new from the stick's own tl/, and mark tl/ so the rotation can be
# followed.
mmd -i "$ESP" ::tl.new || exit 2
for f in vmlinuz initrd.img ramroot.sqsh; do
    mcopy -i "$ESP" "::tl/$f" "$tmp/$f" && mcopy -i "$ESP" "$tmp/$f" "::tl.new/$f" || exit 2
    rm -f "$tmp/$f"
done
echo before-upgrade >"$tmp/marker"
mcopy -i "$ESP" "$tmp/marker" ::tl/marker || exit 2

# The stick carries a 3.x home: this boot is the first ThinFloe boot after an
# upgrade, so the initramfs has to rename these on restore.
home="$tmp/home"
mkdir -p "$home/.dbrrg-ssh-host-keys" "$home/.config/dbrrg/menu" "$home/.thinlinc" || exit 2
echo '# 3.x ThinLinc config' >"$home/.thinlinc/tlclient.conf"
echo 'XKB_DEFAULT_LAYOUT=ch' >"$home/.dbrrg-environment"
echo 'TF_IDLE_TIMEOUT=900' >"$home/.dbrrg-sessionrc"
printf 'network:\n  version: 2\n  wifis:\n    wlan0:\n      dhcp4: true\n      access-points:\n        smoke:\n          password: smokepass1\n' >"$home/wifi.yaml"
ssh-keygen -q -t ed25519 -N '' -f "$home/.dbrrg-ssh-host-keys/ssh_host_ed25519_key" || exit 2
printf '[Desktop Entry]\nType=Application\nName=X\nExec=true\n' >"$home/.config/dbrrg/menu/x.desktop"
# The restore keeps the archive's owner, as a saved home is tfuser's (1001).
tar -C "$home" --owner=1001 --group=1001 --numeric-owner -czf "$tmp/home.tar.gz" . || exit 2
mcopy -o -i "$ESP" "$tmp/home.tar.gz" ::home.tar.gz || exit 2
mcopy -o -i "$ESP" "$DIR/smoke-addons-report.sh" ::smoke-report.sh || exit 2
mcopy -o -i "$ESP" "$DIR/smoke-home-report.sh" ::smoke-home-report.sh || exit 2
REPORT_BASE='systemd.wants=kernel-command-line.service systemd.run_success_action=none systemd.run_failure_action=none'
REPORT="$REPORT_BASE systemd.run=\"/bin/sh /run/thinfloe/storage/efi/smoke-home-report.sh migrate\""

for cfg in ::syslinux.cfg ::efi/boot/syslinux.cfg; do
    mtype -i "$ESP" "$cfg" | tr -d '\r' >"$tmp/cfg" || exit 2
    awk -v append="$APPEND" -v report="$REPORT" '
        /^DEFAULT / { print "DEFAULT new"; next }
        /^LABEL /   { in_new = ($2 == "new") }
        in_new && /^[[:space:]]+APPEND / { print "    APPEND ramroot=tl.new/ramroot.sqsh " append " " report; next }
        { print }' "$tmp/cfg" >"$tmp/cfg.new"
    grep -q '^LABEL new' "$tmp/cfg.new" || { echo "no LABEL new in $cfg" >&2; exit 2; }
    mcopy -o -i "$ESP" "$tmp/cfg.new" "$cfg" || exit 2
done

boot() {
    SMOKE_WAIT_FOR="$1" "$DIR/run-qemu-smoke.sh" "$2" "$TIMEOUT" "$GRACE" -- \
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
        -drive "file=$WORK,format=raw,if=virtio"
}

boot '^SMOKE-HOME-DONE' "$LOG" || exit 2

if "$DIR/check-boot-smoke.sh" "$LOG"; then
    ok "the upgrade boot reaches multi-user.target cleanly"
else
    bad "the upgrade boot failed check-boot-smoke.sh"
fi

if grep -q 'finalize-upgrade: Complete' "$LOG"; then
    ok "the initramfs rotated tl.new into tl/"
else
    bad "no 'finalize-upgrade: Complete' in $LOG"
fi

for pair in ".dbrrg-environment .thinfloe-environment" \
            ".dbrrg-sessionrc .thinfloe-sessionrc" \
            ".dbrrg-ssh-host-keys .thinfloe-ssh-host-keys" \
            ".config/dbrrg .config/thinfloe"; do
    read -r from to <<<"$pair"
    line="thinfloe: renamed ~/$from to ~/$to"
    if grep -qF "$line" "$LOG"; then
        ok "the 3.x home: $line"
    else
        bad "no '$line' in $LOG"
    fi
done

clean="$tmp/log"
LC_ALL=C sed -E -e 's#\x1b\[[0-9;?!>=]*[ -/]*[@-~]##g' -e 's#\r##g' "$LOG" >"$clean"
if grep -qaE '^10-thinlinc\|1\|failed\|' "$clean"; then
    ok "the 3.x home ticked ThinLinc, and with no network its status says failed"
else
    bad "no '10-thinlinc|1|failed' status line in $LOG"
    sed -n '/SMOKE-ADDON-STATUS-BEGIN/,/SMOKE-ADDON-STATUS-END/p' "$clean" | sed 's/^/         /'
fi
if grep -qaE '^20-oxulnk\|0\|' "$clean"; then
    ok "OxuLnk stayed unticked: the 3.x home had no OxuLnk config"
else
    bad "no '20-oxulnk|0' status line in $LOG"
fi

if mdir -i "$ESP" ::tl.new >/dev/null 2>&1; then
    bad "tl.new is still on the stick"
else
    ok "tl.new is gone"
fi

if [[ "$(mtype -i "$ESP" ::tl.old/marker 2>/dev/null | tr -d '\r')" == before-upgrade ]] \
   && ! mtype -i "$ESP" ::tl/marker >/dev/null 2>&1 \
   && mdir -i "$ESP" ::tl/ramroot.sqsh >/dev/null 2>&1; then
    ok "the former tl/ is tl.old/, and tl/ holds the upgrade"
else
    bad "tl/ and tl.old/ are not what the rotation should leave"
fi

for cfg in ::syslinux.cfg ::efi/boot/syslinux.cfg; do
    d=$(mtype -i "$ESP" "$cfg" | tr -d '\r' | grep '^DEFAULT ')
    if [[ "$d" == "DEFAULT current" ]]; then
        ok "$cfg is back on DEFAULT current"
    else
        bad "$cfg says '$d'"
    fi
done

clean1="$tmp/log1"
LC_ALL=C sed -E -e 's#\x1b\[[0-9;?!>=]*[ -/]*[@-~]##g' -e 's#\r##g' "$LOG" >"$clean1"
if grep -qaF 'thinfloe: home layout legacy' "$clean1" && grep -qaF 'SMOKE-HOME layout legacy' "$clean1"; then
    ok "boot 1: home layout legacy"
else
    bad "boot 1: no legacy home layout in $LOG"
fi
if grep -qaF 'thinfloe: installing ~/wifi.yaml into /etc/netplan' "$clean1"; then
    ok "boot 1: the old ~/wifi.yaml is installed"
else
    bad "boot 1: ~/wifi.yaml was not installed ($LOG)"
fi
if grep -qaF 'SMOKE-HOME migrate rc=0' "$clean1" \
   && grep -qaF 'SMOKE-HOME layout-after new' "$clean1" \
   && grep -qaF 'SMOKE-HOME save rc=0' "$clean1"; then
    ok "boot 1: the migration ran and the layout is new"
else
    bad "boot 1: the migration or the save failed ($LOG)"
    grep -a 'SMOKE-HOME' "$clean1" | sed 's/^/         /'
fi

# Boot 2: the same stick on LABEL current, which DEFAULT is back on after the
# rotation. It carries no APPEND of its own that the smoke could read, so give
# it the smoke options and the check.
LOG2="$LOG.2"
REPORT2="$REPORT_BASE systemd.run=\"/bin/sh /run/thinfloe/storage/efi/smoke-home-report.sh check\""
for cfg in ::syslinux.cfg ::efi/boot/syslinux.cfg; do
    mtype -i "$ESP" "$cfg" | tr -d '\r' >"$tmp/cfg" || exit 2
    awk -v append="$APPEND" -v report="$REPORT2" '
        /^LABEL /   { in_cur = ($2 == "current") }
        in_cur && /^[[:space:]]+APPEND / { print "    APPEND ramroot=tl/ramroot.sqsh " append " " report; next }
        { print }' "$tmp/cfg" >"$tmp/cfg.cur"
    grep -q 'systemd.run=.*smoke-home-report.sh check' "$tmp/cfg.cur" \
        || { echo "boot 2: the rewrite of $cfg did not match LABEL current" >&2; exit 2; }
    mcopy -o -i "$ESP" "$tmp/cfg.cur" "$cfg" || exit 2
done
boot '^SMOKE-HOME-DONE' "$LOG2" || exit 2

clean2="$tmp/log2"
LC_ALL=C sed -E -e 's#\x1b\[[0-9;?!>=]*[ -/]*[@-~]##g' -e 's#\r##g' "$LOG2" >"$clean2"
if "$DIR/check-boot-smoke.sh" "$LOG2"; then
    ok "boot 2 reaches multi-user.target cleanly"
else
    bad "boot 2 failed check-boot-smoke.sh"
fi
if grep -qaF 'thinfloe: home layout new' "$clean2"; then
    ok "boot 2: home layout new"
else
    bad "boot 2: no 'thinfloe: home layout new' in $LOG2"
fi
if grep -qaF 'thinfloe: installing ~/.config/thinfloe/generated/wifi.yaml into /etc/netplan' "$clean2"; then
    ok "boot 2: generated/wifi.yaml is installed"
else
    bad "boot 2: generated/wifi.yaml was not installed ($LOG2)"
fi
if grep -qaxF 'SMOKE-HOME env XKB_DEFAULT_LAYOUT=ch' "$clean2"; then
    ok "boot 2: the keyboard came across"
else
    bad "boot 2: no 'SMOKE-HOME env XKB_DEFAULT_LAYOUT=ch' in $LOG2"
fi
if grep -qaF 'SMOKE-HOME attic yes' "$clean2"; then
    ok "boot 2: the old files are in the attic"
else
    bad "boot 2: no 'SMOKE-HOME attic yes' in $LOG2"
fi
if grep -qaF 'thinfloe: renamed' "$clean2" || ! grep -qaxF 'SMOKE-HOME oldmarker no' "$clean2"; then
    bad "boot 2: the 3.x migration ran again on a new home ($LOG2)"
else
    ok "boot 2: no second 3.x rename, no new 3.x home-version marker"
fi
if grep -qaF 'thinfloe: installing ~/wifi.yaml into /etc/netplan' "$clean2"; then
    bad "boot 2: the old ~/wifi.yaml was installed again ($LOG2)"
else
    ok "boot 2: the old ~/wifi.yaml is not installed"
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - upgrade boot smoke"
    exit 1
fi
echo ""
echo "PASSED - upgrade boot smoke"
