#!/bin/bash
# Offline tests for the choice of the boot stick's ESP in thinfloe-lib.sh.
#
# With two sticks attached, both carry a partition named EFI-SYSTEM, and the
# udev link /dev/disk/by-partlabel/EFI-SYSTEM points at either one. Each
# stick therefore carries an id in /config/stick-id and on its syslinux.cfg
# APPEND lines (thinfloe.stick=), and the initramfs mounts the ESP whose id
# the kernel command line names. See
# docs/superpowers/plans/2026-10-26-thinfloe-stick-id.md.
#
# This runs without a container and without root: it stubs dracut's helpers
# and calls the library functions directly, with a PATH that holds only the
# tools the initramfs has. make qemu-smoke-two-sticks covers the boot.
#
# Usage: test/integration/test-esp-choice.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
LIB="$REPO/overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh"

if [[ ! -f "$LIB" ]]; then
    echo "FAIL: $LIB not found" >&2
    exit 1
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

# dracut's logging helpers are absent outside an initramfs. Define them
# before sourcing so the library's "type info || . /lib/dracut-lib.sh"
# guard does not try to pull in the real one.
info() { :; }
warn() { echo "warn: $*" >>"$WORK/warnings"; }
dinfo() { :; }
die()  { echo "die: $*" >>"$WORK/deaths"; return 1; }
: >"$WORK/warnings"
: >"$WORK/deaths"

_TF_LIB_LOADED=""
# Read by the library at source time.
TF_EFI_ABSENT_MARKER="$WORK/efi-absent"

# shellcheck source=/dev/null
. "$LIB"
# The library redefines tf_log to write to /dev/console; collect it instead.
tf_log() { echo "$*" >>"$WORK/log"; }

# --- only the initramfs's command set -----------------------------------
#
# Every call below runs with a PATH holding only what module-setup.sh
# installs (parsed, not hard-coded) plus the dracut base tools, as
# test-initramfs-home.sh does. A call to anything else fails.

MS="$REPO/overlay/usr/lib/dracut/modules.d/90thinfloe/module-setup.sh"
BIN="$WORK/initramfs-bin"; mkdir -p "$BIN"
installed=$(grep -E '^[[:space:]]*inst_multiple' "$MS" | sed 's/#.*//' \
    | tr ' ' '\n' | grep -vE '^(inst_multiple|-o|)$|^/' || true)
for tool in $installed cat rm mv sleep; do
    src=$(command -v "$tool" 2>/dev/null) && [[ "$src" == /* ]] && \
        ln -sf "$src" "$BIN/$tool"
done

if [[ -x "$BIN/sed" && -x "$BIN/mount" ]] \
   && ! PATH="$BIN" command -v cut >/dev/null 2>&1 \
   && ! PATH="$BIN" command -v head >/dev/null 2>&1; then
    ok "initramfs PATH sandbox holds module-setup.sh's tools, and no cut/head"
else
    bad "initramfs PATH sandbox is wrong (is cut/head listed in inst_multiple?)"
fi

A=11111111-1111-4111-8111-111111111111
B=22222222-2222-4222-8222-222222222222

# choose <expected> <args...>: run tf_choose_esp in the sandbox.
choose() {
    local want=$1; shift
    : >"$WORK/log"
    local got
    got=$(PATH="$BIN"; tf_choose_esp "$@")
    if [[ "$got" == "$want" ]]; then
        return 0
    fi
    echo "  got '$got', want '$want'" >&2
    return 1
}

# --- tf_choose_esp ------------------------------------------------------

if choose vdb1 "$A" vda1="$B" vdb1="$A"; then
    ok "rule 1: the partition with the wanted id"
else
    bad "rule 1"
fi

if choose vda1 "$A" vda1="$A" vdb1="$A" \
   && grep -qF "thinfloe: 2 sticks carry stick id $A, using /dev/vda1" "$WORK/log"; then
    ok "rule 2: two partitions with the wanted id -> the first, logged"
else
    bad "rule 2: log=$(cat "$WORK/log")"
fi

if choose vdb1 "" vda1="$B" vdb1=; then
    ok "rule 3: no id wanted -> the stick without an id"
else
    bad "rule 3, stick without an id"
fi

if choose vda1 "" vda1= vdb1= \
   && grep -qF "thinfloe: 2 sticks have no stick id, using /dev/vda1" "$WORK/log"; then
    ok "rule 3: two sticks without an id -> the first, logged"
else
    bad "rule 3, two without an id: log=$(cat "$WORK/log")"
fi

if choose vda1 "$A" vda1="$B" && [[ ! -s "$WORK/log" ]]; then
    ok "rule 3: one partition -> that one, silently"
else
    bad "rule 3, one partition: log=$(cat "$WORK/log")"
fi

if choose vda1 "$A" vda1="$B" vdb1="$B" \
   && grep -qF "thinfloe: no stick carries stick id $A, using /dev/vda1" "$WORK/log"; then
    ok "rule 3: nobody has the wanted id -> the first, logged"
else
    bad "rule 3, no match: log=$(cat "$WORK/log")"
fi

(PATH="$BIN"; tf_choose_esp "") >/dev/null 2>&1; rc=$?
if [[ $rc -eq 1 ]]; then
    ok "no partitions -> exit 1"
else
    bad "no partitions: rc=$rc"
fi

# --- tf_valid_stick_id --------------------------------------------------

if (PATH="$BIN"; tf_valid_stick_id "$A"); then
    ok "tf_valid_stick_id accepts a lowercase uuid"
else
    bad "tf_valid_stick_id refuses $A"
fi
if (PATH="$BIN"; tf_valid_stick_id aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee); then
    ok "tf_valid_stick_id accepts lowercase hex letters"
else
    bad "tf_valid_stick_id refuses lowercase hex letters"
fi
for v in "" AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE "$A-x" "../../x" "$A x" "${A:1}"; do
    if (PATH="$BIN"; tf_valid_stick_id "$v"); then
        bad "tf_valid_stick_id accepts '$v'"
    else
        ok "tf_valid_stick_id refuses '$v'"
    fi
done

# --- tf_esp_candidates and tf_find_esp ----------------------------------
#
# A fake /sys/class/block, getarg answering $WANT, and the stick ids read
# from $WORK/ids/<dev> instead of mounting. udevadm and sleep are stubs;
# sleep counts its calls and runs $WORK/on-sleep once if it exists.

SYS="$WORK/sys"
STUBS="$WORK/stubs"; mkdir -p "$STUBS" "$WORK/ids"
printf '#!/bin/bash\nexit 0\n' >"$STUBS/udevadm"
cat >"$STUBS/sleep" <<STUB
#!/bin/bash
echo x >>"$WORK/sleeps"
if [ -f "$WORK/on-sleep" ]; then
    mv "$WORK/on-sleep" "$WORK/on-sleep.ran"
    . "$WORK/on-sleep.ran"
fi
exit 0
STUB
chmod +x "$STUBS/udevadm" "$STUBS/sleep"

getarg() { echo "$WANT"; }
tf_read_stick_id() { cat "$WORK/ids/$1" 2>/dev/null || true; }

export TF_SYS_BLOCK="$SYS" TF_ESP_CACHE="$WORK/esp-device"
export TF_DEVICE_TIMEOUT=2 TF_ESP_MATCH_WAIT=2

# mkblock <name> [partname]
mkblock() {
    mkdir -p "$SYS/$1"
    {
        echo "MAJOR=253"
        echo "DEVNAME=$1"
        [ -n "${2:-}" ] && echo "PARTNAME=$2"
    } >"$SYS/$1/uevent"
}

# fresh: empty sysfs, ids, cache, marker and counters
fresh() {
    rm -rf "$SYS" "$WORK/ids" "$TF_ESP_CACHE" "$TF_EFI_ABSENT_MARKER" \
        "$WORK/on-sleep" "$WORK/on-sleep.ran"
    mkdir -p "$SYS" "$WORK/ids"
    : >"$WORK/sleeps"
    : >"$WORK/log"
}
sleeps() { local n=0; while read -r _; do n=$((n + 1)); done <"$WORK/sleeps"; echo $n; }
find_esp() { (PATH="$STUBS:$BIN"; tf_find_esp); }

fresh
mkblock vda; mkblock vda1 EFI-SYSTEM; mkblock vdb1 EFI-SYSTEM; mkblock loop0
got=$(PATH="$BIN"; tf_esp_candidates | tr '\n' ' ')
if [[ "$got" == "vda1 vdb1 " ]]; then
    ok "tf_esp_candidates lists the EFI-SYSTEM partitions only"
else
    bad "tf_esp_candidates: '$got'"
fi

echo "$A" >"$WORK/ids/vda1"; echo "$B" >"$WORK/ids/vdb1"
WANT=$B
got=$(find_esp); rc=$?
if [[ $rc -eq 0 && "$got" == /dev/vdb1 && "$(cat "$TF_ESP_CACHE")" == /dev/vdb1 ]] \
   && grep -qF "thinfloe: boot stick /dev/vdb1 (stick id $B)" "$WORK/log"; then
    ok "tf_find_esp takes the stick the command line names, and caches it"
else
    bad "tf_find_esp rule 1: rc=$rc got='$got' log=$(cat "$WORK/log")"
fi

echo "$B" >"$WORK/ids/vda1"
got=$(find_esp)
if [[ "$got" == /dev/vdb1 ]]; then
    ok "a second call returns the cached answer"
else
    bad "second call: got='$got'"
fi

fresh
mkblock vda1 EFI-SYSTEM; echo "$A" >"$WORK/ids/vda1"
{
    echo "mkdir -p '$SYS/vdb1'; echo PARTNAME=EFI-SYSTEM >'$SYS/vdb1/uevent'"
    echo "echo '$B' >'$WORK/ids/vdb1'"
} >"$WORK/on-sleep"
WANT=$B
got=$(find_esp)
if [[ "$got" == /dev/vdb1 ]]; then
    ok "a stick that enumerates late is waited for"
else
    bad "late stick: got='$got' sleeps=$(sleeps)"
fi

fresh
mkblock vda1 EFI-SYSTEM
WANT=
got=$(find_esp)
if [[ "$got" == /dev/vda1 && $(sleeps) -eq 0 ]] \
   && grep -qF "thinfloe: boot stick /dev/vda1 (stick id none)" "$WORK/log"; then
    ok "one stick without an id, none wanted: no wait"
else
    bad "one stick, no id: got='$got' sleeps=$(sleeps) log=$(cat "$WORK/log")"
fi

fresh
mkblock vda1 EFI-SYSTEM; echo "$B" >"$WORK/ids/vda1"
WANT=$A
got=$(find_esp)
if [[ "$got" == /dev/vda1 && $(sleeps) -ge 1 && $(sleeps) -le $TF_ESP_MATCH_WAIT ]]; then
    ok "a missing match costs at most TF_ESP_MATCH_WAIT"
else
    bad "missing match: got='$got' sleeps=$(sleeps)"
fi

fresh
WANT=
got=$(find_esp); rc=$?
if [[ $rc -eq 1 && -z "$got" && -e "$TF_EFI_ABSENT_MARKER" ]]; then
    ok "no EFI-SYSTEM partition: exit 1, marker written"
else
    bad "no partition: rc=$rc got='$got'"
fi
: >"$WORK/sleeps"
find_esp >/dev/null; rc=$?
if [[ $rc -eq 1 && $(sleeps) -eq 0 ]]; then
    ok "after the marker, a second call fails at once"
else
    bad "second call without partition: rc=$rc sleeps=$(sleeps)"
fi

exit $fail
