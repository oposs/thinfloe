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
HELPER="${HELPER:-$ROOT/overlay/usr/libexec/thinfloe/labwc-scale}"
COMPOSE="$ROOT/overlay/usr/bin/thinfloe-compose-labwc-config"
SYSDIR="$ROOT/overlay/etc/thinfloe/labwc"

if [[ ! -x "$HELPER" ]]; then
    echo "FAIL: $HELPER not found or not executable" >&2
    exit 1
fi

mkdir -p "${TMPDIR:-/scratch/oetiker/claude-tmp/thinfloe-tests}"
WORK=$(mktemp -d)
hpid=
cleanup() {
    [[ -n $hpid ]] && kill "$hpid" 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

fail=0
ok() { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

cat >"$WORK/labwc" <<'STUB'
#!/bin/sh
echo "$*" >>"$STUB_LOG"
n=$(wc -l <"$STUB_LOG")
[ -z "${STUB_FAIL:-}" ] && [ "$n" -gt "${STUB_FAIL_FIRST:-0}" ]
STUB
chmod +x "$WORK/labwc"

# Fresh composed dir, empty scale file state, empty stub log.
setup() {
    rm -rf "$WORK/run" "$WORK/scale" "$WORK/calls" "$WORK/menu-output" "$WORK/menu-output.applied"
    mkdir -p "$WORK/run"
    : >"$WORK/calls"
    : >"$WORK/home-env"
    "$COMPOSE" "$SYSDIR" "$WORK/home-env" "$WORK/run/thinfloe-labwc"
}

# run [--wait N]: run the helper once (--once); stderr in $WORK/err, status in $rc.
run() {
    TF_SCALE_FILE="$WORK/scale" TF_MENU_OUTPUT_FILE="$WORK/menu-output" \
        TF_LABWC_DIR="${DIR:-$WORK/run/thinfloe-labwc}" \
        TF_LABWC="$WORK/labwc" STUB_LOG="$WORK/calls" "$HELPER" --once "$@" 2>"$WORK/err"
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
[[ $(calls) -eq 2 ]] \
    && ok "failing labwc: reconfigured again after the restore" || bad "failing labwc: labwc calls: $(calls)"
[[ $(readlink "$D/rc.xml") == "$SYSDIR/rc.xml" ]] \
    && ok "failing labwc: symlink target restored" || bad "failing labwc: wrong target"

# --- a file that arrives late -------------------------------------------
setup
( sleep 1; echo 3 >"$WORK/scale.tmp"; mv "$WORK/scale.tmp" "$WORK/scale" ) &
run --wait 5; wait
[[ $(grep -c '<font place=.*size="30"' "$D/rc.xml") -eq 2 ]] \
    && ok "late scale file is picked up" || bad "late scale file ignored"

# --- later changes are followed -------------------------------------------
# waitfor DESC CMD...: poll up to 10 s for CMD to succeed.
waitfor() {
    local desc=$1 n=0
    shift
    until "$@"; do
        n=$((n + 1))
        if [[ $n -gt 100 ]]; then bad "$desc"; return 1; fi
        sleep 0.1
    done
    ok "$desc"
}
fonts_are() { [[ $(grep -c "<font place=.*size=\"$1\"" "$D/rc.xml" 2>/dev/null) -eq 2 ]]; }
calls_are() { [[ $(calls) -eq $1 ]]; }

setup; echo 2 >"$WORK/scale"
TF_SCALE_POLL=0.1 TF_SCALE_FILE="$WORK/scale" TF_LABWC_DIR="$D" TF_LABWC="$WORK/labwc" \
    STUB_LOG="$WORK/calls" "$HELPER" --wait 2 2>"$WORK/err" &
hpid=$!
waitfor "follow: first scale 2 applied" fonts_are 20
calls_are 1 && ok "follow: one reconfigure" || bad "follow: calls $(calls)"

echo 1.5 >"$WORK/scale.tmp"; mv "$WORK/scale.tmp" "$WORK/scale"
waitfor "follow: 2 -> 1.5 gives size 15" fonts_are 15
waitfor "follow: second reconfigure" calls_are 2
[[ $(grep -c '^window.button.width: 39$' "$D/themerc-override") -eq 1 ]] \
    && ok "follow: theme rebuilt from the system file" || bad "follow: theme: $(grep button.width "$D/themerc-override")"

sleep 0.6
calls_are 2 && ok "follow: unchanged file, no extra reconfigure" || bad "follow: calls $(calls)"

echo 1 >"$WORK/scale.tmp"; mv "$WORK/scale.tmp" "$WORK/scale"
waitfor "follow: change to 1 restores the links" untouched
waitfor "follow: reconfigure after the restore" calls_are 3
[[ $(readlink "$D/rc.xml") == "$SYSDIR/rc.xml" ]] \
    && ok "follow: link target is the system file" || bad "follow: wrong target"

echo 3 >"$WORK/scale.tmp"; mv "$WORK/scale.tmp" "$WORK/scale"
waitfor "follow: back to 3 gives size 30" fonts_are 30
kill -0 "$hpid" 2>/dev/null && ok "follow: helper still running" || bad "follow: helper exited"
kill "$hpid" 2>/dev/null; wait "$hpid" 2>/dev/null
hpid=

# --- a failed apply is tried again, a bounded number of times ------------
# follow ENV...: start the helper with the poll at 0.1 s and the retry at 0.3 s.
follow() {
    env TF_SCALE_POLL=0.1 TF_SCALE_RETRY=0.3 TF_SCALE_FILE="$WORK/scale" \
        TF_MENU_OUTPUT_FILE="$WORK/menu-output" TF_LABWC_DIR="$D" \
        TF_LABWC="$WORK/labwc" STUB_LOG="$WORK/calls" "$@" "$HELPER" --wait 2 2>"$WORK/err" &
    hpid=$!
}
stop() { kill "$hpid" 2>/dev/null; wait "$hpid" 2>/dev/null; hpid=; }

# labwc fails the first attempt (two calls: the reconfigure and the one after
# the restore) and then works: the same scale is applied by the retry.
setup; echo 2 >"$WORK/scale"
follow STUB_FAIL_FIRST=2
waitfor "retry: the scale is applied after a failed first attempt" fonts_are 20
calls_are 3 && ok "retry: three labwc calls in all" || bad "retry: calls $(calls)"
sleep 0.8
calls_are 3 && ok "retry: no more attempts once it worked" || bad "retry: calls $(calls) after success"
[[ $(grep -c 'title bar scale not applied' "$WORK/err") -eq 1 ]] \
    && ok "retry: the failed attempt was logged once" || bad "retry: stderr: $(cat "$WORK/err")"
stop

# labwc never works: the first attempt and three retries, two calls each,
# then the helper waits for a change.
setup; echo 2 >"$WORK/scale"
follow STUB_FAIL=1
waitfor "retry: four attempts, two calls each" calls_are 8
sleep 1.2
calls_are 8 && ok "retry: it gives up after three retries" || bad "retry: calls $(calls) after giving up"
untouched && ok "retry: symlinks restored" || bad "retry: symlinks not restored"
[[ $(grep -c 'title bar scale not applied' "$WORK/err") -eq 4 ]] \
    && ok "retry: each attempt logged" || bad "retry: stderr: $(cat "$WORK/err")"
echo 3 >"$WORK/scale.tmp"; mv "$WORK/scale.tmp" "$WORK/scale"
calls_ge() { [[ $(calls) -ge $1 ]]; }
waitfor "retry: a changed file is tried at once" calls_ge 10
stop

# --- the menu output -----------------------------------------------------
# The menu names its monitor in $WORK/menu-output; the helper puts a
# MoveToOutput action in the thinfloe-menu window rule, before
# ToggleAlwaysOnBottom, and writes the name to menu-output.applied.
OUT="$WORK/menu-output"
APPLIED="$WORK/menu-output.applied"
rule() { sed -n '/<windowRule identifier="thinfloe-menu"/,/<\/windowRule>/p' "$D/rc.xml"; }
# rule_has NAME: the rule holds exactly one MoveToOutput for NAME, on the line
# before ToggleAlwaysOnBottom.
rule_has() {
    local r
    r=$(rule)
    [[ $(grep -c 'MoveToOutput' <<<"$r") -eq 1 ]] &&
        [[ $(grep -B1 'ToggleAlwaysOnBottom' <<<"$r" | head -1 | sed 's/^ *//') == \
           "<action name=\"MoveToOutput\" output=\"$1\"/>" ]]
}
no_keybinding() { ! grep -q '<keybind\|<default' "$D/rc.xml"; }
applied_is() { [[ -f $APPLIED && $(cat "$APPLIED") == "$1" ]]; }
no_temp_files() { [[ -z $(ls -A "$WORK" | grep '^\.applied\.') ]]; }

# Control: no output file, scale 2 -> no rule at all, and no .applied.
setup; echo 2 >"$WORK/scale"; run --wait 2
{ [[ -f $D/rc.xml ]] && [[ -n $(rule) ]] && ! grep -q 'MoveToOutput' "$D/rc.xml"; } \
    && ok "output control: missing file, no MoveToOutput" || bad "output control: rule present or rule block not found"
[[ ! -e $APPLIED ]] && ok "output control: .applied not written" || bad "output control: .applied exists"
no_keybinding && ok "output control: no keybinding" || bad "output control: keybinding"

# Control: empty output file, scale 2 -> no rule.
setup; echo 2 >"$WORK/scale"; : >"$OUT"; run --wait 2
{ [[ -n $(rule) ]] && ! grep -q 'MoveToOutput' "$D/rc.xml"; } \
    && ok "output control: empty file, no MoveToOutput" || bad "output control: empty file gave a rule"

# Output only, scale 1.
setup; echo 1 >"$WORK/scale"; echo HDMI-A-1 >"$OUT"; run --wait 2
{ [[ $rc -eq 0 ]] && [[ -f $D/rc.xml && ! -L $D/rc.xml ]]; } \
    && ok "output only: rc.xml is a regular file" || bad "output only: rc.xml still a link (rc $rc)"
rule_has HDMI-A-1 && ok "output only: rule before ToggleAlwaysOnBottom" || bad "output only: rule wrong: $(rule)"
[[ $(grep -c '<font place=.*size="10"' "$D/rc.xml") -eq 2 ]] \
    && ok "output only: fonts unscaled" || bad "output only: fonts"
applied_is HDMI-A-1 && ok "output only: .applied holds the name" || bad "output only: .applied: $(cat "$APPLIED" 2>&1)"
[[ $(cat "$WORK/calls") == "--reconfigure" ]] \
    && ok "output only: one --reconfigure" || bad "output only: labwc calls: $(cat "$WORK/calls")"
no_keybinding && ok "output only: no keybinding" || bad "output only: keybinding"
no_temp_files && ok "output only: no temp file left behind" || bad "output only: temp file left"
[[ $(cksum "$SYSDIR/rc.xml" "$SYSDIR/themerc-override") == "$sums" ]] \
    && ok "output only: system files untouched" || bad "output only: system files changed"

# Output and scale 2 in one rc.xml.
setup; echo 2 >"$WORK/scale"; echo DP-2 >"$OUT"; run --wait 2
{ fonts_are 20 && rule_has DP-2; } \
    && ok "output and scale 2: both in one rc.xml" || bad "output and scale 2: fonts or rule missing"
applied_is DP-2 && ok "output and scale 2: .applied holds the name" || bad "output and scale 2: .applied"
grep -qx 'window.button.width: 52' "$D/themerc-override" \
    && ok "output and scale 2: theme scaled" || bad "output and scale 2: theme"
no_keybinding && ok "output and scale 2: no keybinding" || bad "output and scale 2: keybinding"
diff <(grep -v '<font\|MoveToOutput' "$D/rc.xml") <(grep -v '<font' "$SYSDIR/rc.xml") >/dev/null \
    && ok "output and scale 2: only font and rule lines differ" || bad "output and scale 2: other lines differ"

# No scale file at all: the output file alone is enough (the helper waits for it).
setup; ( sleep 1; echo eDP-1 >"$OUT.tmp"; mv "$OUT.tmp" "$OUT" ) &
run --wait 5; wait
{ [[ ! -e $WORK/scale ]] && rule_has eDP-1 && applied_is eDP-1; } \
    && ok "no scale file: the output alone works" || bad "no scale file: output ignored (rc $rc, err $(cat "$WORK/err"))"
no_keybinding && ok "no scale file: no keybinding" || bad "no scale file: keybinding"

# A name that is not a safe output name: refused, nothing written.
for badname in 'a"<b' 'a b' 'x&y' "$(printf 'a%.0s' {1..65})"; do
    setup; echo 1 >"$WORK/scale"; printf '%s\n' "$badname" >"$OUT"; run --wait 2
    { [[ $rc -eq 0 ]] && untouched && [[ $(calls) -eq 0 ]] && [[ ! -e $APPLIED ]]; } \
        && ok "refused name '${badname:0:12}': links kept, no reconfigure, no .applied" \
        || bad "refused name '${badname:0:12}': something changed"
    grep -q 'thinfloe: menu output not applied' "$WORK/err" \
        && ok "refused name '${badname:0:12}': reason on stderr" || bad "refused name '${badname:0:12}': stderr: $(cat "$WORK/err")"
done
setup; echo 2 >"$WORK/scale"; echo 'a"<b' >"$OUT"; run --wait 2
{ fonts_are 20 && ! grep -q 'MoveToOutput' "$D/rc.xml" && [[ ! -e $APPLIED ]]; } \
    && ok "refused name with scale 2: scale applied, no rule, no .applied" || bad "refused name with scale 2"
no_keybinding && ok "refused name with scale 2: no keybinding" || bad "refused name with scale 2: keybinding"

# Cleared: the links come back and .applied is empty. Then a name again.
setup; echo 1 >"$WORK/scale"; echo HDMI-A-1 >"$OUT"
follow
waitfor "cleared: the rule is applied first" applied_is HDMI-A-1
rule_has HDMI-A-1 && ok "cleared: rule present before the clear" || bad "cleared: rule missing before the clear"
: >"$OUT.tmp"; mv "$OUT.tmp" "$OUT"
waitfor "cleared: the links come back" untouched
waitfor "cleared: .applied is empty" applied_is ""
[[ -f $APPLIED && ! -s $APPLIED ]] && ok "cleared: .applied is an empty file" || bad "cleared: .applied not empty"
calls_are 2 && ok "cleared: reconfigure after the restore" || bad "cleared: calls $(calls)"
echo DP-1 >"$OUT.tmp"; mv "$OUT.tmp" "$OUT"
waitfor "cleared: a new name applies again" applied_is DP-1
rule_has DP-1 && ok "cleared: new rule in place" || bad "cleared: new rule wrong"
# A refused name later keeps the last good output and logs once.
printf '%s\n' 'a"<b' >"$OUT.tmp"; mv "$OUT.tmp" "$OUT"
sleep 0.6
{ rule_has DP-1 && applied_is DP-1; } \
    && ok "cleared: a refused name keeps the last output" || bad "cleared: refused name changed state"
[[ $(grep -c 'menu output not applied' "$WORK/err") -eq 1 ]] \
    && ok "cleared: the refusal is logged once" || bad "cleared: stderr: $(cat "$WORK/err")"
stop

# labwc fails: .applied is not written.
setup; echo 1 >"$WORK/scale"; echo HDMI-A-1 >"$OUT"; STUB_FAIL=1 run --wait 2
{ untouched && [[ ! -e $APPLIED ]]; } \
    && ok "failing labwc with an output: links back, no .applied" || bad "failing labwc with an output"

exit $fail
