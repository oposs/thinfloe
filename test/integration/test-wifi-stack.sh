#!/bin/bash
# Regression test for the duplicated WiFi core stack.
#
# containers/ubuntu/Dockerfile used to install linux-modules-iwlwifi-generic.
# That package is not an Intel driver add-on: it ships its own cfg80211.ko
# and mac80211.ko into /usr/lib/modules/<ver>/ubuntu/dkms/iwlwifi/. Ubuntu's
# depmod order is "search updates ubuntu built-in", so the backport copies
# shadow the in-tree ones for *every* wireless device - rtw89_core, mt76 and
# friends all ended up depending on Intel's backported core. Two copies of
# mac80211/cfg80211 in one image is what produces ieee80211_* symbol
# mismatches at module load.
#
# It was added in 2785aea ("upgrade to ubntu 24.04") where it made sense:
# kernel 6.8 needed backported Intel support. Kernel 7.x ships iwlwifi,
# iwldvm, iwlmvm and iwlmld in-tree, so the backport adds nothing and only
# creates the conflict.
#
# Usage: test/integration/test-wifi-stack.sh [path/to/ramroot.sqsh]

set -uo pipefail

SQSH="${1:-artifacts/rootfs/ramroot.sqsh}"

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

if ! unsquashfs -l "$SQSH" >"$LIST" 2>/dev/null; then
    echo "FAIL: cannot list $SQSH" >&2
    exit 1
fi

KVER=$(sed -nE 's|^.*/usr/lib/modules/([^/]+)/modules\.dep$|\1|p' "$LIST" | head -1)
if [[ -z "$KVER" ]]; then
    echo "FAIL: no /usr/lib/modules/<ver>/modules.dep in $SQSH" >&2
    exit 1
fi
echo "note - kernel $KVER"

fail=0

# 1. The backport package must not be installed at all.
if grep -qE "/usr/lib/modules/$KVER/ubuntu/dkms/iwlwifi/" "$LIST"; then
    echo "FAIL - backport WiFi stack present under ubuntu/dkms/iwlwifi/"
    echo "       remove linux-modules-iwlwifi-generic from containers/ubuntu/Dockerfile"
    fail=1
else
    echo "ok   - no backport WiFi stack under ubuntu/dkms/iwlwifi/"
fi

# 2. The in-tree Intel drivers must be the ones providing the coverage.
for m in iwlwifi iwldvm iwlmvm iwlmld; do
    if grep -qE "kernel/drivers/net/wireless/intel/iwlwifi/.*$m\.ko" "$LIST"; then
        echo "ok   - in-tree $m present"
    else
        echo "FAIL - in-tree $m missing"
        fail=1
    fi
done

# 3. The decisive check: what modules.dep actually resolves to. Absence of the
#    directory is not enough on its own - depmod is what picks the winner, and
#    any future package dropping a second core would show up here first.
printf 'usr/lib/modules/%s/modules.dep\n' "$KVER" >"$WORK/want.txt"
if ! unsquashfs -n -d "$WORK/x" -ef "$WORK/want.txt" "$SQSH" >/dev/null 2>&1; then
    echo "FAIL - cannot extract modules.dep from $SQSH"
    exit 1
fi
DEP="$WORK/x/usr/lib/modules/$KVER/modules.dep"
if [[ ! -f "$DEP" ]]; then
    echo "FAIL - modules.dep not extracted"
    exit 1
fi

for m in mac80211 cfg80211; do
    paths=$(grep -oE "^[^:]*/$m\.ko[^:]*" "$DEP")
    count=$(printf '%s\n' "$paths" | grep -c . )
    if [[ "$count" -ne 1 ]]; then
        echo "FAIL - $m resolves to $count copies (must be exactly 1):"
        printf '       %s\n' $paths
        fail=1
    elif [[ "$paths" != kernel/* ]]; then
        echo "FAIL - $m resolves to $paths (must be the in-tree kernel/ copy)"
        fail=1
    else
        echo "ok   - $m resolves to $paths"
    fi
done

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - shipped image has a duplicated or shadowed WiFi core stack"
    exit 1
fi

echo ""
echo "PASSED - single in-tree WiFi core stack"
exit 0
