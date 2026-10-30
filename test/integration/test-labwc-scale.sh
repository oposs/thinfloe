#!/bin/bash
# labwc-scale: the title bars follow the scale thinfloe-menu chose.
#
# The helper replaces the rc.xml and themerc-override symlinks of the
# composed config dir by scaled copies and asks labwc to reconfigure. This
# test runs the real thinfloe-compose-labwc-config on the shipped config and
# a stub labwc that records its arguments.
#
# Usage: test/integration/test-labwc-scale.sh

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HELPER="$ROOT/overlay/usr/libexec/thinfloe/labwc-scale"
COMPOSE="$ROOT/overlay/usr/bin/thinfloe-compose-labwc-config"
SYSDIR="$ROOT/overlay/etc/thinfloe/labwc"

if [[ ! -x "$HELPER" ]]; then
    echo "FAIL: $HELPER not found or not executable" >&2
    exit 1
fi

mkdir -p "${TMPDIR:-/scratch/oetiker/claude-tmp/thinfloe-tests}"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok() { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

cat >"$WORK/labwc" <<'STUB'
#!/bin/sh
echo "$*" >>"$STUB_LOG"
[ -z "${STUB_FAIL:-}" ]
STUB
chmod +x "$WORK/labwc"

# Fresh composed dir, empty scale file state, empty stub log.
setup() {
    rm -rf "$WORK/run" "$WORK/scale" "$WORK/calls"
    mkdir -p "$WORK/run"
    : >"$WORK/calls"
    : >"$WORK/home-env"
    "$COMPOSE" "$SYSDIR" "$WORK/home-env" "$WORK/run/thinfloe-labwc"
}

# run [--wait N]: run the helper; stderr in $WORK/err, status in $rc.
run() {
    TF_SCALE_FILE="$WORK/scale" TF_LABWC_DIR="${DIR:-$WORK/run/thinfloe-labwc}" \
        TF_LABWC="$WORK/labwc" STUB_LOG="$WORK/calls" "$HELPER" "$@" 2>"$WORK/err"
    rc=$?
}

D="$WORK/run/thinfloe-labwc"
calls() { wc -l <"$WORK/calls" | tr -d ' '; }
untouched() { [[ -L $D/rc.xml && -L $D/themerc-override ]]; }

sums=$(cksum "$SYSDIR/rc.xml" "$SYSDIR/themerc-override")

# --- 2 -------------------------------------------------------------------
setup; echo 2 >"$WORK/scale"; run --wait 2
[[ $rc -eq 0 ]] && ok "scale 2 exits 0" || bad "scale 2 exit $rc"
[[ $(grep -c '<font place=.*size="20"' "$D/rc.xml") -eq 2 ]] \
    && ok "scale 2: both fonts size 20" || bad "scale 2: fonts"
for pair in 'padding.height: 8' 'window.titlebar.padding.width: 16' \
            'window.button.width: 52' 'window.button.height: 52' 'border.width: 1'; do
    grep -qxF "$pair" "$D/themerc-override" \
        && ok "scale 2: $pair" || bad "scale 2: missing '$pair'"
done
[[ $(grep -c '^window.button.width' "$D/themerc-override") -eq 1 ]] \
    && ok "scale 2: key not duplicated" || bad "scale 2: duplicate key"
[[ $(calls) -eq 1 && $(cat "$WORK/calls") == "--reconfigure" ]] \
    && ok "scale 2: one --reconfigure" || bad "scale 2: labwc calls: $(cat "$WORK/calls")"
[[ -f $D/rc.xml && ! -L $D/rc.xml && ! -L $D/themerc-override ]] \
    && ok "scale 2: copies replace the symlinks" || bad "scale 2: still symlinks"
[[ $(sed -n '/<keyboard>/,/<\/keyboard>/p' "$D/rc.xml") == \
   $(sed -n '/<keyboard>/,/<\/keyboard>/p' "$SYSDIR/rc.xml") ]] \
    && ok "scale 2: keyboard section identical" || bad "scale 2: keyboard section differs"
if grep -q '<keybind\|<default' "$D/rc.xml"; then
    bad "scale 2: rc.xml has a keybinding"
else
    ok "scale 2: no keybinding"
fi
diff <(grep -v '<font' "$D/rc.xml") <(grep -v '<font' "$SYSDIR/rc.xml") >/dev/null \
    && ok "scale 2: only font lines differ" || bad "scale 2: other lines differ"
[[ $(cksum "$SYSDIR/rc.xml" "$SYSDIR/themerc-override") == "$sums" ]] \
    && ok "system files untouched" || bad "system files changed"

# --- 2.25 ----------------------------------------------------------------
setup; echo 2.25 >"$WORK/scale"; run --wait 2
[[ $(grep -c '<font place=.*size="23"' "$D/rc.xml") -eq 2 ]] \
    && ok "scale 2.25: fonts size 23" || bad "scale 2.25: fonts"

# --- 1 -------------------------------------------------------------------
setup; echo 1 >"$WORK/scale"; run --wait 2
{ [[ $rc -eq 0 ]] && untouched && [[ $(calls) -eq 0 ]]; } \
    && ok "scale 1: nothing changed, no reconfigure" || bad "scale 1 changed something"

# --- no file -------------------------------------------------------------
setup; run --wait 1
{ [[ $rc -eq 0 ]] && untouched && [[ $(calls) -eq 0 ]]; } \
    && ok "no scale file: nothing changed" || bad "no scale file changed something"

# --- garbage -------------------------------------------------------------
for g in abc 100 0.1 "2 3"; do
    setup; echo "$g" >"$WORK/scale"; run --wait 1
    { [[ $rc -eq 0 ]] && untouched && [[ $(calls) -eq 0 ]]; } \
        && ok "scale file '$g': nothing changed" || bad "scale file '$g' changed something"
done

# --- composed dir missing ------------------------------------------------
setup; echo 2 >"$WORK/scale"; DIR="$WORK/nonexistent" run --wait 1
{ [[ $rc -eq 0 ]] && [[ $(calls) -eq 0 ]]; } \
    && ok "missing config dir: exit 0, no reconfigure" || bad "missing config dir"

# --- labwc fails ---------------------------------------------------------
setup; echo 2 >"$WORK/scale"; STUB_FAIL=1 run --wait 2
{ [[ $rc -eq 0 ]] && untouched; } \
    && ok "failing labwc: symlinks restored, exit 0" || bad "failing labwc: not restored (rc $rc)"
grep -q 'thinfloe: title bar scale not applied' "$WORK/err" \
    && ok "failing labwc: reason on stderr" || bad "failing labwc: stderr: $(cat "$WORK/err")"
[[ $(readlink "$D/rc.xml") == "$SYSDIR/rc.xml" ]] \
    && ok "failing labwc: symlink target restored" || bad "failing labwc: wrong target"

# --- a file that arrives late -------------------------------------------
setup
( sleep 1; echo 3 >"$WORK/scale.tmp"; mv "$WORK/scale.tmp" "$WORK/scale" ) &
run --wait 5; wait
[[ $(grep -c '<font place=.*size="30"' "$D/rc.xml") -eq 2 ]] \
    && ok "late scale file is picked up" || bad "late scale file ignored"

exit $fail
