#!/bin/bash
# Asserts the USB automount pieces shipped in the image: the helper, the udev
# rule, the template unit, the tmpfiles entries and the kernel modules that
# mount the usual stick and card file systems.
#
# Usage: test/integration/test-automount-image.sh [path/to/ramroot.sqsh]

set -uo pipefail

# unsquashfs starts a thread per core; the build box is shared.
unsquashfs() { command unsquashfs -processors "${BUILD_JOBS:-4}" "$@"; }

SQSH="${1:-artifacts/rootfs/ramroot.sqsh}"

if [[ ! -f "$SQSH" ]]; then
    echo "FAIL: $SQSH not found - run 'make rootfs' first" >&2
    exit 1
fi

WORK=$(mktemp -d)
trap 'chmod -R u+rwX "$WORK" 2>/dev/null; rm -rf "$WORK"' EXIT
LIST="$WORK/list.txt"

if ! unsquashfs -ll "$SQSH" >"$LIST" 2>/dev/null; then
    echo "FAIL: cannot list $SQSH" >&2
    exit 1
fi

KVER=$(sed -nE 's|^.*/usr/lib/modules/([^/]+)/modules\.dep$|\1|p' "$LIST" | head -1)
if [[ -z "$KVER" ]]; then
    echo "FAIL: no /usr/lib/modules/<ver>/modules.dep in $SQSH" >&2
    exit 1
fi

HELPER=usr/libexec/thinfloe/thinfloe-automount
RULE=etc/udev/rules.d/90-thinfloe-automount.rules
UNIT=etc/systemd/system/thinfloe-automount@.service
TMPF=usr/lib/tmpfiles.d/thinfloe-automount.conf
MODDIR=usr/lib/modules/$KVER

{
    printf '%s\n' "$HELPER" "$RULE" "$UNIT" "$TMPF" "$MODDIR/modules.dep" "$MODDIR/modules.builtin"
} >"$WORK/want.txt"
unsquashfs -n -d "$WORK/x" -ef "$WORK/want.txt" "$SQSH" >/dev/null 2>&1
X="$WORK/x"

fail=0
ok()  { echo "ok   - $*"; }
bad() { echo "FAIL - $*"; fail=1; }

# 1. The helper: executable, owned by root.
line=$(grep -E " squashfs-root/$HELPER\$" "$LIST" || true)
if [[ "$line" =~ ^-rwxr-xr-x\ +root/root ]]; then
    ok "helper is 0755 root"
else
    bad "helper missing or not 0755 root: $line"
fi

for f in "$RULE" "$UNIT"; do
    if grep -qE "^-rw-r--r-- +root/root .* squashfs-root/${f//./\\.}\$" "$LIST"; then
        ok "$f is 0644 root"
    else
        bad "$f is not 0644 root"
    fi
done

# 2. The rule and the unit exist; the unit is started by the rule only.
[[ -f "$X/$RULE" ]] && ok "udev rule present" || bad "udev rule $RULE missing"
[[ -f "$X/$UNIT" ]] && ok "unit present" || bad "unit $UNIT missing"
if grep -qE 'etc/systemd/system/[^/]*\.wants/thinfloe-automount' "$LIST"; then
    bad "thinfloe-automount@.service is enabled; the udev rule must start it"
else
    ok "unit is not enabled"
fi
if [[ -f "$X/$UNIT" ]] && grep -qE '^\s*Before=' "$X/$UNIT"; then
    bad "unit declares Before="
else
    ok "unit has no Before="
fi

# 2b. The rule's content: what it starts, and the change lines that make a
# pulled card stop the unit and the next card start it.
if [[ -f "$X/$RULE" ]]; then
    for want in \
        'ENV{ID_FS_USAGE}=="filesystem", TAG+="systemd", ENV{SYSTEMD_WANTS}+="thinfloe-automount@%k.service"' \
        'ACTION=="change", ENV{DEVTYPE}=="disk", ENV{ID_FS_USAGE}!="filesystem", ENV{SYSTEMD_READY}="0", GOTO="thinfloe_automount_end"' \
        'ACTION=="change", ENV{ID_FS_USAGE}=="filesystem", ENV{SYSTEMD_READY}="1"'; do
        if grep -qxF "$want" "$X/$RULE"; then ok "rule has: $want"; else bad "rule lacks: $want"; fi
    done
fi

# 3. The unit's binding and commands.
if [[ -f "$X/$UNIT" ]]; then
    for want in \
        'BindsTo=dev-%i.device' \
        'After=dev-%i.device' \
        'TimeoutStartSec=30' \
        'TimeoutStopSec=30' \
        'ExecStart=/usr/libexec/thinfloe/thinfloe-automount add %i' \
        'ExecStop=/usr/libexec/thinfloe/thinfloe-automount remove %i'; do
        if grep -qxF "$want" "$X/$UNIT"; then ok "unit has $want"; else bad "unit lacks $want"; fi
    done
fi

# 4. The tmpfiles entries.
if [[ -f "$X/$TMPF" ]]; then
    for want in \
        'd /media/tfuser 0755 root root' \
        'L /run/media/tfuser - - - - /media/tfuser' \
        'd /run/thinfloe/automount 0700 root root'; do
        if grep -qxF "$want" "$X/$TMPF"; then ok "tmpfiles has '$want'"; else bad "tmpfiles lacks '$want'"; fi
    done
else
    bad "tmpfiles $TMPF missing"
fi

# 5. Kernel modules: a loadable file in modules.dep or built in.
for m in vfat exfat ntfs3 ext4 btrfs xfs usb-storage uas mmc_block sdhci-pci; do
    pat=${m//[-_]/[-_]}
    if grep -qE "(^|/)${pat}\.ko" "$X/$MODDIR/modules.dep" 2>/dev/null \
       || grep -qE "(^|/)${pat}\.ko" "$X/$MODDIR/modules.builtin" 2>/dev/null; then
        ok "kernel module $m available"
    else
        bad "kernel module $m neither in modules.dep nor built in"
    fi
done

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - USB automount pieces missing from the image"
    exit 1
fi
echo ""
echo "PASSED - USB automount pieces shipped"
exit 0
