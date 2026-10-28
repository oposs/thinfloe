#!/bin/bash
# Offline tests for growing the ESP in thinfloe-lib.sh (grow-esp.sh): the
# size rule, the check whether the FAT32 needs fat32expander, and the
# progress shown on the boot screen while it runs. Stubs stand in for
# plymouth and fat32expander; the FAT32 checks use real mkfs.fat images.
# make qemu-smoke-grow covers the boot.
#
# Usage: test/integration/test-esp-grow.sh

set -o pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
LIB="$REPO/overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh"

: "${TMPDIR:=/scratch/oetiker/claude-tmp/thinfloe-tests}"
mkdir -p "$TMPDIR"
WORK=$(mktemp -d "$TMPDIR/test-esp-grow.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

info() { :; }
warn() { :; }
dinfo() { :; }
getarg() { :; }
# shellcheck disable=SC1090
. "$LIB"
tf_log() { echo "$*" >>"$WORK/log"; }

# --- the size rule ----------------------------------------------------------
# tf_esp_grow_size <start> <size> <disk>, all in 512-byte sectors.
size_case() {  # description, start, size, disk, want ("" = stay)
    local got
    got=$(tf_esp_grow_size "$2" "$3" "$4")
    if [[ "$got" == "$5" ]]; then
        ok "$1: ${5:-stays}"
    else
        bad "$1: want '${5:-stays}', got '$got'"
    fi
}
size_case "3000 MB on a 4 GB stick grows to its end"   2048 6144000 7812500  7809024
size_case "3000 MB on a 16 GB stick stops at 6 GiB"    2048 6144000 31250000 12582912
size_case "2000 MB on a 16 GB stick stops at 6 GiB"    2048 4096000 31250000 12582912
size_case "less than 256 MiB to gain"                  2048 6144000 6500000  ""
size_case "already 6 GiB"                              2048 12582912 31250000 ""
size_case "15 GB ESP is never shrunk"                  2048 29564181 30031872 ""
size_case "the image itself (3002 MiB)"                2048 6144000 6150144  ""

# --- does the FAT32 need fat32expander? -------------------------------------
img="$WORK/fs.img"
truncate -s 3000M "$img"
mkfs.fat -I -S 512 -s 1 -n EFI-SYSTEM "$img" >/dev/null
if ! tf_fat_needs_resize "$img" 6144000; then
    ok "a fresh mkfs.fat filling its partition needs no resize"
else
    bad "a fresh mkfs.fat filling its partition is taken for one that needs a resize"
fi
if tf_fat_needs_resize "$img" 12582912; then
    ok "a FAT32 smaller than its grown partition needs a resize"
else
    bad "a FAT32 smaller than its grown partition is not resized"
fi
printf '\0\0' | dd of="$img" bs=1 seek=510 conv=notrunc status=none
if tf_fat_needs_resize "$img" 6144000; then
    ok "a FAT32 without its 55 aa signature (an interrupted resize) is resized"
else
    bad "an interrupted resize is not finished"
fi

# --- progress on the boot screen --------------------------------------------
# fat32expander: the dry run plans 10 MiB; the resize writes 2 MiB a second
# to the fake diskstats for 6 s.
mkdir "$WORK/bin"
cat >"$WORK/bin/fat32expander" <<EOF
#!/bin/bash
if [[ \$2 == --dry-run ]]; then
    echo "  - Planned data shift for 20480 clusters (10485760 bytes)"
    exit 0
fi
for i in 1 2 3 4 5 6; do
    sleep 1
    echo "   8       0 sda 1 0 0 0 1 0 \$(( i * 4096 )) 0 0 0 0" >"$WORK/diskstats"
done
if [[ -n "\${FAKE_RC:-}" ]]; then
    echo "Error: device vanished"
    exit "\$FAKE_RC"
fi
echo "The filesystem has been resized successfully."
exit 0
EOF
cat >"$WORK/bin/plymouth" <<EOF
#!/bin/sh
case "\$1" in
    --ping) exit 0 ;;
    update) echo "\$2" >>"$WORK/steps" ;;
esac
EOF
chmod 755 "$WORK/bin/"*
echo "   8       0 sda 1 0 0 0 1 0 0 0 0 0 0" >"$WORK/diskstats"

PATH="$WORK/bin:$PATH" TF_DISKSTATS="$WORK/diskstats" tf_resize_fat /dev/sda1 sda
rc=$?
[[ $rc -eq 0 ]] && ok "tf_resize_fat returns fat32expander's status" || bad "tf_resize_fat returned $rc"
if grep -qx -- '--status=tf:Enlarging the boot stick, once - do not switch off' "$WORK/steps"; then
    ok "the boot screen announces the resize"
else
    bad "no announcement on the boot screen: $(head -3 "$WORK/steps" 2>/dev/null)"
fi
pcts=$(sed -n 's/.*once: \([0-9]*\) %.*/\1/p' "$WORK/steps" | tr '\n' ' ')
if [[ "$pcts" == *"40 "* && "$pcts" == *"99 "* ]]; then
    ok "the boot screen counts up to 99 %: $pcts"
else
    bad "progress on the boot screen: '$pcts'"
fi
if awk 'length($0) > 64 + 11 { exit 1 }' "$WORK/steps"; then
    ok "every step text fits the 64-character step line"
else
    bad "a step text is longer than 64 characters"
fi
grep -q 'fat32expander:' "$WORK/log" \
    && bad "a resize that worked puts fat32expander's report in the log" \
    || ok "a resize that worked keeps fat32expander's report out of the log"

echo "   8       0 sda 1 0 0 0 1 0 0 0 0 0 0" >"$WORK/diskstats"
FAKE_RC=3 PATH="$WORK/bin:$PATH" TF_DISKSTATS="$WORK/diskstats" tf_resize_fat /dev/sda1 sda
rc=$?
[[ $rc -eq 3 ]] && ok "a failed resize returns fat32expander's status" || bad "a failed resize returned $rc"
grep -q 'fat32expander: Error: device vanished' "$WORK/log" \
    && ok "a failed resize puts fat32expander's report in the log" \
    || bad "a failed resize leaves fat32expander's report out of the log"

# --- the hook ----------------------------------------------------------------
# dracut sources every pre-mount hook into one shell, so an exit in grow-esp.sh
# (pre-mount 20) skipped finalize-upgrade.sh (25) and no upgrade rotated.
if grep -v '^[[:space:]]*#' "${LIB%/*}/grow-esp.sh" \
        | grep -qE '(^|[;&|[:space:]])exit([[:space:]]|$)'; then
    bad "grow-esp.sh calls exit, which ends every later pre-mount hook"
else
    ok "grow-esp.sh has no exit, so the later pre-mount hooks run"
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - ESP grow"
    exit 1
fi
echo ""
echo "PASSED - ESP grow"
exit 0
