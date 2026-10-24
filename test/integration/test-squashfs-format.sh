#!/bin/bash
# The squashfs is zstd level 19 with 1 MiB blocks.
#
# zstd decompresses at the same speed at every level, so a higher level costs
# only build time. Measured 2026-10-24 on the same tree: level 3 gave
# 554.7 MB, 19 gave 514.3 MB, 22 nothing more. xz gave 491.2 MB but costs CPU
# on every page read for the whole session, so it was rejected.
#
# Usage: test/integration/test-squashfs-format.sh [path/to/ramroot.sqsh]

set -uo pipefail

SQSH="${1:-artifacts/rootfs/ramroot.sqsh}"

if [[ ! -f "$SQSH" ]]; then
    echo "FAIL: $SQSH not found - run 'make rootfs' first" >&2
    exit 1
fi

INFO=$(unsquashfs -s "$SQSH" 2>&1) || { echo "FAIL: cannot read $SQSH" >&2; exit 1; }

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

expect() {
    if grep -qE -- "$2" <<<"$INFO"; then
        ok "$1"
    else
        bad "$1 (no match for: $2)"
    fi
}

expect "compressed with zstd"  '^Compression zstd$'
expect "zstd level 19"         '^[[:space:]]*compression-level 19$'
expect "1 MiB blocks"          '^Block size 1048576$'

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - squashfs format"
    exit 1
fi

echo ""
echo "PASSED - squashfs format"
exit 0
