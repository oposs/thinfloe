#!/bin/bash
# Checks that files the image does not need stay out of ramroot.sqsh.
#
# The Makefile bind-mounts ./scripts at /scripts for the export, so anything
# mksquashfs does not exclude ships to every client (4.2.0 shipped
# /scripts/publish.sh and the smoke scripts). Later size reduction steps
# append their checks here.
#
# Usage: test/integration/test-image-slim.sh [path/to/ramroot.sqsh]

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

LIST=$(mktemp)
LLS=$(mktemp)
trap 'rm -f "$LIST" "$LLS"' EXIT

if ! unsquashfs -l "$SQSH" >"$LIST" 2>/dev/null; then
    echo "FAIL: cannot list $SQSH" >&2
    exit 1
fi
if ! unsquashfs -lls "$SQSH" >"$LLS" 2>/dev/null; then
    echo "FAIL: cannot list $SQSH (long format)" >&2
    exit 1
fi

fail=0
check() {
    local desc="$1" pattern="$2"
    if grep -qE -- "$pattern" "$LIST"; then
        echo "ok   - $desc"
    else
        echo "FAIL - $desc (no match for: $pattern)"
        fail=1
    fi
}
absent() {
    local desc="$1" pattern="$2" hit
    hit=$(grep -E -m1 -- "$pattern" "$LIST" || true)
    if [[ -z $hit ]]; then
        echo "ok   - $desc"
    else
        echo "FAIL - $desc (found: $hit)"
        fail=1
    fi
}

absent "no build scripts in /scripts" '^squashfs-root/scripts(/|$)'
absent "no slim scripts"              '^squashfs-root/thinfloe-slim(/|$)'

# Locale and hwdb sources (S2, S3): the compiled forms stay.
absent "no locale sources"            '^squashfs-root/usr/share/i18n(/|$)'
check  "locale archive"               '^squashfs-root/usr/lib/locale/locale-archive$'
absent "no hwdb sources"              '^squashfs-root/usr/lib/udev/hwdb\.d(/|$)'

# hwdb.bin must be present and real (more than 1000000 bytes). Field 3 of
# "unsquashfs -lls" is the size for a regular file.
hwdb_size=$(awk '$NF == "squashfs-root/usr/lib/udev/hwdb.bin" && $1 ~ /^-/ {print $3}' "$LLS" | head -1)
if [[ -n $hwdb_size && $hwdb_size -gt 1000000 ]]; then
    echo "ok   - hwdb.bin present ($hwdb_size bytes)"
else
    echo "FAIL - hwdb.bin missing or too small (size: ${hwdb_size:-none})"
    fail=1
fi

# The unit stays masked, so nothing rebuilds hwdb.bin into the overlay at boot.
if grep -qE 'squashfs-root/etc/systemd/system/systemd-hwdb-update\.service -> /dev/null$' "$LLS"; then
    echo "ok   - systemd-hwdb-update.service is masked"
else
    echo "FAIL - systemd-hwdb-update.service is not masked"
    fail=1
fi

if [[ $fail -eq 0 ]]; then
    echo "PASSED"
else
    echo "FAILED"
    exit 1
fi
