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

exit $fail
