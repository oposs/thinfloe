#!/bin/bash
# Offline: parse-thinfloe.sh turns debug mode on for rd.thinfloe.debug and for
# rd.dbrrg.debug, the parameter the debug entry of every 3.x syslinux.cfg
# carries. dracut-lib is stubbed; the hook's fixed paths are overridden.
#
# Usage: test/integration/test-cmdline.sh

set -uo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
HOOK="$ROOT/overlay/usr/lib/dracut/modules.d/90thinfloe/parse-thinfloe.sh"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
fail=0
ok()   { echo "ok   - $1"; }
bad()  { echo "FAIL - $1"; fail=1; }

# What dracut-lib.sh and thinfloe-lib.sh provide to the hook, reading the
# command line from $CMDLINE.
cat >"$WORK/stub-lib.sh" <<'STUB'
info() { :; }
warn() { :; }
die() { echo "die: $*" >&2; exit 1; }
tf_log() { :; }
is_remote_url() { case "$1" in http://*|https://*) return 0 ;; *) return 1 ;; esac; }
getarg() {
    for _w in $(cat "$CMDLINE"); do
        case "$_w" in "$1"*) echo "${_w#"$1"}"; return 0 ;; esac
    done
    return 1
}
getargbool() {
    _d=$1; shift
    for _w in $(cat "$CMDLINE"); do
        case "$_w" in
            "$1"|"$1=1") return 0 ;;
            "$1=0") return 1 ;;
        esac
    done
    [ "$_d" = 1 ]
}
STUB

run_hook() {
    rm -rf "$WORK/run" "$WORK/cmdline.d"
    mkdir -p "$WORK/run" "$WORK/cmdline.d"
    echo "$1" >"$WORK/cmdline"
    (
        CMDLINE="$WORK/cmdline" TF_RUN_TMP="$WORK/run" TF_CMDLINE_D="$WORK/cmdline.d" \
        TF_DRACUT_LIB="$WORK/stub-lib.sh" TF_LIB="$WORK/stub-lib.sh"
        export CMDLINE TF_RUN_TMP TF_CMDLINE_D TF_DRACUT_LIB TF_LIB
        . "$WORK/stub-lib.sh"
        . "$HOOK"
    ) >/dev/null 2>&1
}

check() { # cmdline expected(yes|no) label
    run_hook "$1"
    local rc=$? have=no
    [[ -e "$WORK/run/thinfloe-debug" ]] && have=yes
    if [[ $rc -eq 0 && $have == "$2" ]]; then ok "$3"; else bad "$3 (rc=$rc marker=$have)"; fi
}

check "ramroot=tl/ramroot.sqsh" no "no debug parameter: no marker"
check "ramroot=tl/ramroot.sqsh rd.thinfloe.debug" yes "rd.thinfloe.debug enables debug"
check "ramroot=tl/ramroot.sqsh rd.dbrrg.debug" yes "rd.dbrrg.debug enables debug"
check "ramroot=tl/ramroot.sqsh rd.dbrrg.debug=0" no "rd.dbrrg.debug=0 does not"

run_hook "ramroot=tl/ramroot.sqsh"
[[ "$(cat "$WORK/run/thinfloe-ramroot" 2>/dev/null)" == "tl/ramroot.sqsh" ]] \
    && ok "ramroot is recorded" || bad "ramroot not recorded"
run_hook "ramroot=http://h/ramroot.sqsh"
grep -qx 'rd.neednet=1' "$WORK/cmdline.d/99-thinfloe-network.conf" 2>/dev/null \
    && ok "network ramroot sets rd.neednet=1" || bad "rd.neednet=1 missing"

exit $fail
