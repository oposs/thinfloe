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
trap 'rm -f "$LIST"' EXIT

if ! unsquashfs -l "$SQSH" >"$LIST" 2>/dev/null; then
    echo "FAIL: cannot list $SQSH" >&2
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

if [[ $fail -eq 0 ]]; then
    echo "PASSED"
else
    echo "FAILED"
    exit 1
fi
