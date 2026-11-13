#!/bin/bash
# Image size report and budget check.
#
# Prints the size of vmlinuz, initrd.img and ramroot.sqsh (MB = 10^6 bytes),
# the uncompressed size of the trees the size reduction work cuts (regular
# files only: links and directories count nothing), and the
# three budgets. With a baseline directory it prints the delta of every
# number against the same numbers computed from the baseline files.
#
# Usage: test/integration/test-image-size.sh [rootfs-dir [baseline-dir]]

set -uo pipefail

# Budgets in bytes: the sizes measured on 2026-11-13 plus 2 %, rounded up to
# a whole MB. initrd.img 84.7 MB; ramroot.sqsh 391.0 MB and the sum 493.0 MB
# after nouveau stayed and Carrizo/Stoney firmware came in (first set at
# 389.1 MB and 491.1 MB).
# Raising one needs a reason in the commit message.
MAX_SQSH=399000000
MAX_INITRD=87000000
MAX_SUM=503000000

DIR="${1:-artifacts/rootfs}"
BASE="${2:-}"

for d in "$DIR" ${BASE:+"$BASE"}; do
    for f in vmlinuz initrd.img ramroot.sqsh; do
        if [[ ! -f "$d/$f" ]]; then
            echo "FAIL: $d/$f not found - run 'make rootfs' first" >&2
            exit 1
        fi
    done
done

for tool in unsquashfs lsinitramfs; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "FAIL: $tool not installed (apt install squashfs-tools initramfs-tools-core)" >&2
        exit 1
    fi
done

WORK=$(mktemp -d)
trap 'chmod -R u+rwX "$WORK" 2>/dev/null; rm -rf "$WORK"' EXIT

SQ_PATHS=(
    usr/lib/modules/
    usr/lib/firmware/
    usr/lib/firmware/intel/iwlwifi/
    usr/lib/firmware/intel-ucode/
    usr/share/i18n/
    usr/lib/udev/hwdb.d/
    scripts/
)

# measure <dir> <outfile>: writes "<label>\t<bytes>" lines.
measure() {
    local dir="$1" out="$2" f p
    : >"$out"
    local sum=0 size
    for f in vmlinuz initrd.img ramroot.sqsh; do
        size=$(stat -c %s "$dir/$f")
        printf '%s\t%s\n' "$f" "$size" >>"$out"
        sum=$((sum + size))
    done
    printf 'sum\t%s\n' "$sum" >>"$out"
    unsquashfs -lls "$dir/ramroot.sqsh" 2>/dev/null >"$WORK/sq.lst" \
        || { echo "FAIL: cannot list $dir/ramroot.sqsh" >&2; exit 1; }
    for p in "${SQ_PATHS[@]}"; do
        # Regular files only: field 1 starts with "-", field 3 is the size,
        # field 6 and later is the path (prefixed "squashfs-root/").
        awk -v p="squashfs-root/$p" '
            substr($1,1,1) == "-" {
                path = $0
                sub(/^([^ ]+ +){5}/, "", path)
                if (index(path, p) == 1) s += $3
            }
            END { printf "squashfs %s\t%d\n", substr(p, 15), s }' \
            "$WORK/sq.lst" >>"$out"
    done
    lsinitramfs -l "$dir/initrd.img" 2>/dev/null >"$WORK/ird.lst" \
        || { echo "FAIL: cannot list $dir/initrd.img" >&2; exit 1; }
    # lsinitramfs -l: mode, links, owner, group, size, then the date in three
    # fields (month, day, time or year), then the path.
    awk '
        substr($1,1,1) == "-" {
            path = $0
            sub(/^([^ ]+ +){8}/, "", path)
            if (path ~ /\.ko(\.|$)/) ko += $5
            if (path ~ /^kernel\/x86\/microcode\//) uc += $5
        }
        END {
            printf "initrd *.ko*\t%d\n", ko
            printf "initrd kernel/x86/microcode/\t%d\n", uc
        }' "$WORK/ird.lst" >>"$out"
}

measure "$DIR" "$WORK/now.tsv"
[[ -n "$BASE" ]] && measure "$BASE" "$WORK/base.tsv"

mb() { awk -v b="$1" 'BEGIN { printf "%.1f", b / 1000000 }'; }
dmb() { awk -v b="$1" 'BEGIN { printf "%+.1f", b / 1000000 }'; }
get() { awk -F'\t' -v k="$2" '$1 == k { print $2 }' "$1"; }

report() {
    local label="$1" now base
    now=$(get "$WORK/now.tsv" "$label")
    if [[ -n "$BASE" ]]; then
        base=$(get "$WORK/base.tsv" "$label")
        printf '%-40s %10s MB  (delta %s MB)\n' "$label" "$(mb "$now")" "$(dmb $((now - base)))"
    else
        printf '%-40s %10s MB\n' "$label" "$(mb "$now")"
    fi
}

VMLINUZ=$(get "$WORK/now.tsv" vmlinuz)
INITRD=$(get "$WORK/now.tsv" initrd.img)
SQSH=$(get "$WORK/now.tsv" ramroot.sqsh)
SUM=$(get "$WORK/now.tsv" sum)

echo "== Image files =="
report vmlinuz
report initrd.img
report ramroot.sqsh
report sum
echo ""
echo "== Uncompressed contents =="
for p in "${SQ_PATHS[@]}"; do report "squashfs $p"; done
report "initrd *.ko*"
report "initrd kernel/x86/microcode/"
echo ""
echo "Sizes: vmlinuz $(mb "$VMLINUZ") MB, initrd.img $(mb "$INITRD") MB, ramroot.sqsh $(mb "$SQSH") MB, sum $(mb "$SUM") MB"
echo ""

fail=0
budget() {
    local desc="$1" val="$2" max="$3"
    if [[ $val -le $max ]]; then
        echo "ok   - $desc $(mb "$val") MB <= $(mb "$max") MB"
    else
        echo "FAIL - $desc $(mb "$val") MB > $(mb "$max") MB"
        fail=1
    fi
}
budget "ramroot.sqsh budget" "$SQSH" "$MAX_SQSH"
budget "initrd.img budget" "$INITRD" "$MAX_INITRD"
budget "sum budget" "$SUM" "$MAX_SUM"

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - image exceeds its size budget"
    exit 1
fi

echo ""
echo "PASSED - image within its size budget"
exit 0
