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
#   - afterwards: no tl.new, tl.old holding the former tl, and both
#     syslinux.cfg copies back on DEFAULT current.

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
mkdir -p "$home/.dbrrg-ssh-host-keys" "$home/.config/dbrrg/menu" || exit 2
echo 'XKB_DEFAULT_LAYOUT=ch' >"$home/.dbrrg-environment"
echo '# 3.x session rc' >"$home/.dbrrg-sessionrc"
ssh-keygen -q -t ed25519 -N '' -f "$home/.dbrrg-ssh-host-keys/ssh_host_ed25519_key" || exit 2
printf '[Desktop Entry]\nType=Application\nName=X\nExec=true\n' >"$home/.config/dbrrg/menu/x.desktop"
tar -C "$home" -czf "$tmp/home.tar.gz" . || exit 2
mcopy -o -i "$ESP" "$tmp/home.tar.gz" ::home.tar.gz || exit 2

for cfg in ::syslinux.cfg ::efi/boot/syslinux.cfg; do
    mtype -i "$ESP" "$cfg" | tr -d '\r' >"$tmp/cfg" || exit 2
    awk -v append="$APPEND" '
        /^DEFAULT / { print "DEFAULT new"; next }
        /^LABEL /   { in_new = ($2 == "new") }
        in_new && /^[[:space:]]+APPEND / { print "    APPEND ramroot=tl.new/ramroot.sqsh " append; next }
        { print }' "$tmp/cfg" >"$tmp/cfg.new"
    grep -q '^LABEL new' "$tmp/cfg.new" || { echo "no LABEL new in $cfg" >&2; exit 2; }
    mcopy -o -i "$ESP" "$tmp/cfg.new" "$cfg" || exit 2
done

"$DIR/run-qemu-smoke.sh" "$LOG" "$TIMEOUT" "$GRACE" -- \
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
    -net nic,model=virtio -net user \
    -drive "file=$WORK,format=raw,if=virtio" || exit 2

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

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - upgrade boot smoke"
    exit 1
fi
echo ""
echo "PASSED - upgrade boot smoke"
