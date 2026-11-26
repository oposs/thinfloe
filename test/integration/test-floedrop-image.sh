#!/bin/bash
# FloeDrop image guards: thinfloe-share and owlyd are installed, Owly's system
# files are the pinned commit's unchanged copies, the FloeDrop tmpfs unit has
# its fixed options, the RTL8822BU driver and firmware are present, and
# neither polkit nor NetworkManager is in the image.
#
# Usage: test/integration/test-floedrop-image.sh [path/to/ramroot.sqsh]

set -uo pipefail

SQSH="${1:-artifacts/rootfs/ramroot.sqsh}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [[ ! -f "$SQSH" ]]; then
    echo "FAIL: $SQSH not found - run 'make rootfs' first" >&2
    exit 1
fi
if ! command -v unsquashfs >/dev/null 2>&1; then
    echo "FAIL: unsquashfs not installed (apt install squashfs-tools)" >&2
    exit 1
fi

# unsquashfs starts a thread per core; the build box is shared.
unsquashfs() { command unsquashfs -processors "${BUILD_JOBS:-4}" "$@"; }

WORK=$(mktemp -d)
trap 'chmod -R u+rwX "$WORK" 2>/dev/null; rm -rf "$WORK"' EXIT
LIST="$WORK/list.txt"
ROOT="$WORK/root"

if ! unsquashfs -l "$SQSH" >"$LIST" 2>/dev/null; then
    echo "FAIL: cannot list $SQSH" >&2
    exit 1
fi

FAILED=0
fail() { echo "FAIL: $*" >&2; FAILED=1; }

# iw: the radio helper reads the channel the link runs on with `iw dev`; journalctl: it
# follows owlyd's STATS lines to see a card that stopped sending.
FILES=(
    usr/bin/thinfloe-share
    usr/sbin/iw
    usr/bin/journalctl
    usr/lib/owly/owlyd
    usr/lib/systemd/system/owlyd.service
    usr/share/dbus-1/system-services/io.github.t4t5.Owly.service
    usr/share/dbus-1/system.d/io.github.t4t5.Owly.conf
    usr/lib/systemd/network/10-owly-awdl0.link
    usr/share/doc/owly/LICENSE
    etc/systemd/system/media-tfuser-FloeDrop.mount
    etc/systemd/system/local-fs.target.wants/media-tfuser-FloeDrop.mount
    var/lib/dpkg/status
)
unsquashfs -d "$ROOT" -f "$SQSH" "${FILES[@]}" >/dev/null 2>&1

for f in usr/bin/thinfloe-share usr/sbin/iw usr/bin/journalctl usr/lib/owly/owlyd; do
    [[ -x "$ROOT/$f" ]] || fail "/$f is missing or not executable"
done

# Owly's four system files are the pinned commit's, unchanged.
for f in usr/lib/systemd/system/owlyd.service \
         usr/share/dbus-1/system-services/io.github.t4t5.Owly.service \
         usr/share/dbus-1/system.d/io.github.t4t5.Owly.conf \
         usr/lib/systemd/network/10-owly-awdl0.link \
         usr/share/doc/owly/LICENSE; do
    if [[ ! -f "$ROOT/$f" ]]; then
        fail "/$f is missing"
    elif ! cmp -s "$ROOT/$f" "$HERE/overlay/$f"; then
        fail "/$f differs from overlay/$f"
    fi
done
grep -qx 'ExecStart=/usr/lib/owly/owlyd' "$ROOT/usr/lib/systemd/system/owlyd.service" \
    || fail "owlyd.service does not start /usr/lib/owly/owlyd"

# The FloeDrop tmpfs.
M="$ROOT/etc/systemd/system/media-tfuser-FloeDrop.mount"
grep -qx 'What=tmpfs' "$M" || fail "mount unit: What=tmpfs missing"
grep -qx 'Where=/media/tfuser/FloeDrop' "$M" || fail "mount unit: Where= wrong"
grep -qx 'Type=tmpfs' "$M" || fail "mount unit: Type=tmpfs missing"
grep -qx 'Options=size=25%,uid=1001,gid=1001,mode=0755,nosuid,nodev,noexec' "$M" \
    || fail "mount unit: Options= is not the fixed set"
grep -qx 'WantedBy=local-fs.target' "$M" || fail "mount unit: WantedBy=local-fs.target missing"
W="$ROOT/etc/systemd/system/local-fs.target.wants/media-tfuser-FloeDrop.mount"
[[ -L "$W" ]] || fail "local-fs.target.wants link is not a symlink"
[[ "$(readlink "$W")" == ../media-tfuser-FloeDrop.mount ]] \
    || fail "local-fs.target.wants link target is $(readlink "$W")"

# RTL8822BU (the usual iPhone-AWDL-capable USB adapter) driver and firmware.
grep -Eq '/rtw88_8822bu\.ko' "$LIST" || fail "rtw88_8822bu module missing"
grep -Eq '/rtw88/rtw8822b_fw\.bin' "$LIST" || fail "rtw88/rtw8822b_fw.bin missing"

# No polkit, no NetworkManager.
for p in network-manager polkitd policykit-1; do
    if grep -qx "Package: $p" "$ROOT/var/lib/dpkg/status"; then
        fail "package $p is installed"
    fi
done

if [[ $FAILED -ne 0 ]]; then
    exit 1
fi
echo "PASS: FloeDrop image files, FloeDrop tmpfs, rtw88 and package guards"
