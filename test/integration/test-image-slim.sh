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

WORK=$(mktemp -d)
trap 'chmod -R u+rwX "$WORK" 2>/dev/null; rm -rf "$WORK"' EXIT
LIST="$WORK/list.txt"
LLS="$WORK/lls.txt"

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

# Kernel modules are stored uncompressed (S1): the squashfs compresses them
# better than zstd inside it, and depmod must index the final names.
absent "modules stored uncompressed"  '^squashfs-root/usr/lib/modules/.*\.ko\.zst$'

# modules.dep names only files that exist: every path, left and right of the
# colon, must be in the squashfs.
KVER=$(sed -nE 's|^.*/usr/lib/modules/([^/]+)/modules\.dep$|\1|p' "$LIST" | head -1)
if [[ -z $KVER ]]; then
    echo "FAIL - modules.dep names only files that exist (no modules.dep in $SQSH)"
    fail=1
else
    printf 'usr/lib/modules/%s/modules.dep\n' "$KVER" >"$WORK/want.txt"
    if ! unsquashfs -n -d "$WORK/x" -ef "$WORK/want.txt" "$SQSH" >/dev/null 2>&1 \
        || [[ ! -s $WORK/x/usr/lib/modules/$KVER/modules.dep ]]; then
        echo "FAIL - modules.dep names only files that exist (cannot extract modules.dep)"
        fail=1
    else
        # Names in a modules.dep that the squashfs list does not hold.
        missing_deps() {
            awk -v base="squashfs-root/usr/lib/modules/$KVER/" '
                NR == FNR { have[$0] = 1; next }
                {
                    n = split($0, parts, /: ?| /)
                    for (i = 1; i <= n; i++)
                        if (parts[i] != "" && !((base parts[i]) in have))
                            print parts[i]
                }' "$LIST" "$1" | sort -u | head -3
        }
        # Negative control: one bogus path on each side of the colon must show.
        sed '1s|^|kernel/bogus/left.ko: kernel/bogus/right.ko\n|' \
            "$WORK/x/usr/lib/modules/$KVER/modules.dep" >"$WORK/dep-bogus"
        control=$(missing_deps "$WORK/dep-bogus")
        if [[ $control == *kernel/bogus/left.ko* && $control == *kernel/bogus/right.ko* ]]; then
            echo "ok   - modules.dep check reports a bogus path (control)"
        else
            echo "FAIL - modules.dep check misses a bogus path (control saw: ${control:-nothing})"
            fail=1
        fi
        missing=$(missing_deps "$WORK/x/usr/lib/modules/$KVER/modules.dep")
        if [[ -z $missing ]]; then
            echo "ok   - modules.dep names only files that exist"
        else
            echo "FAIL - modules.dep names files that are not in the squashfs: $(tr '\n' ' ' <<<"$missing")"
            fail=1
        fi
    fi
fi

if [[ $fail -eq 0 ]]; then
    echo "PASSED"
else
    echo "FAILED"
    exit 1
fi
