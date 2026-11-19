#!/bin/bash
# Regression test for the labwc environment override bug and the merge order
# system -> generated/environment -> user environment minus menu-managed keys.
#
# labwc, run with -C, re-reads <config_dir>/environment itself
# (src/config/session.c, session_environment_init()) and setenv()s every
# key with overwrite=1. Since 10-thinfloe-session.sh used to point -C straight
# at /etc/thinfloe/labwc, that re-read clobbered any key the user had also set
# in ~/.thinfloe-environment - even though the shell had already exported the
# user's value moments before. thinfloe-compose-labwc-config fixes this by
# building a merged config directory whose environment file has the
# system defaults first and the user's overrides last, so the file labwc
# itself parses already has the user's values winning.
#
# This test exercises the helper offline, without a container: fixture
# system/user directories in, a merged directory out, checked with plain
# grep/diff. It does not touch labwc's own parser - see
# test/runtime/test-labwc-runtime.sh for the assertion that a later
# duplicate assignment actually wins there.
#
# Usage: test/integration/test-labwc-config-merge.sh

set -uo pipefail

HELPER="$(cd "$(dirname "$0")/../.." && pwd)/overlay/usr/bin/thinfloe-compose-labwc-config"

if [[ ! -x "$HELPER" ]]; then
    echo "FAIL: $HELPER not found or not executable" >&2
    exit 1
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok() { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

# --- fixtures -----------------------------------------------------------

SYSDIR="$WORK/system"
mkdir -p "$SYSDIR"
cat >"$SYSDIR/environment" <<'EOF'
XKB_DEFAULT_LAYOUT=us
LABWC_FULLSCREEN_SPAN_OUTPUTS=1
EOF
cat >"$SYSDIR/rc.xml" <<'EOF'
<?xml version="1.0"?>
<labwc_config/>
EOF

echo 'window.active.border.color: #7cc4ff' >"$SYSDIR/themerc-override"

GENENV="$WORK/generated-environment"
cat >"$GENENV" <<'EOF'
XKB_DEFAULT_LAYOUT=de
XKB_DEFAULT_VARIANT=
XKB_DEFAULT_OPTIONS=ctrl:nocaps
EOF

MANAGED="$WORK/managed-keys"
cat >"$MANAGED" <<'EOF'
# keys the menu sets
XKB_DEFAULT_LAYOUT
XKB_DEFAULT_VARIANT
XKB_DEFAULT_OPTIONS
XKB_DEFAULT_MODEL
EOF
export TF_MANAGED_KEYS="$MANAGED"
# No keyboard check unless a case sets one (the host may have a menu).
export TF_KB_CHECK="$WORK/no-checker"

USERENV="$WORK/user-environment"
cat >"$USERENV" <<'EOF'
# a comment line stays
XKB_DEFAULT_LAYOUT=fr
XKB_DEFAULT_LAYOUT = fr
  export XKB_DEFAULT_VARIANT=oss
LABWC_FULLSCREEN_SPAN_OUTPUTS=0
XCURSOR_THEME=redglass
EOF

OUT="$WORK/merged"

# --- run 1 ---------------------------------------------------------------

if ! "$HELPER" "$SYSDIR" "$GENENV" "$USERENV" "$OUT"; then
    echo "FAIL: helper exited non-zero on the happy path" >&2
    exit 1
fi

MERGED_ENV="$OUT/environment"

if [[ ! -f "$MERGED_ENV" ]]; then
    echo "FAIL: $MERGED_ENV was not created" >&2
    exit 1
fi

# key present only in the system file survives
if grep -q '^XKB_DEFAULT_LAYOUT=us$' "$MERGED_ENV"; then
    ok "system-only key (XKB_DEFAULT_LAYOUT) survives"
else
    bad "system-only key (XKB_DEFAULT_LAYOUT) missing from merged file"
fi

# key present in both ends up with the user's value LAST
last_span=$(grep '^LABWC_FULLSCREEN_SPAN_OUTPUTS=' "$MERGED_ENV" | tail -1)
if [[ "$last_span" == "LABWC_FULLSCREEN_SPAN_OUTPUTS=0" ]]; then
    ok "duplicate key ends with the user's value last (=0)"
else
    bad "duplicate key's last occurrence is '$last_span', expected LABWC_FULLSCREEN_SPAN_OUTPUTS=0"
fi

# key present only in the user file appears
if grep -q '^XCURSOR_THEME=redglass$' "$MERGED_ENV"; then
    ok "user-only key (XCURSOR_THEME) appears"
else
    bad "user-only key (XCURSOR_THEME) missing from merged file"
fi

# rc.xml (and any other system file) is carried through, not copied-twice
if [[ -e "$OUT/rc.xml" ]]; then
    ok "rc.xml is present in the merged directory"
else
    bad "rc.xml is missing from the merged directory"
fi

# Both key switches off: rc.xml links .source/rc.xml, which ends at the
# system file, and there is no keybinding.
if [[ -L "$OUT/rc.xml" && "$(readlink "$OUT/rc.xml")" == "$OUT/.source/rc.xml" &&
      "$(readlink -f "$OUT/rc.xml")" == "$(readlink -f "$SYSDIR/rc.xml")" ]] &&
   ! grep -q '<keybind' "$OUT/rc.xml"; then
    ok "rc.xml links .source/rc.xml, which ends at the system file"
else
    bad "rc.xml is $(readlink "$OUT/rc.xml") -> $(readlink -f "$OUT/rc.xml")"
fi

if [[ -L "$OUT/themerc-override" ]] && grep -q '#7cc4ff' "$OUT/themerc-override"; then
    ok "themerc-override is linked into the merged directory"
else
    bad "themerc-override is not linked into the merged directory"
fi

# --- order, managed keys ---------------------------------------------------

l_sys=$(grep -n '^XKB_DEFAULT_LAYOUT=us$' "$MERGED_ENV" | head -1 | cut -d: -f1)
l_gen=$(grep -n '^XKB_DEFAULT_LAYOUT=de$' "$MERGED_ENV" | head -1 | cut -d: -f1)
l_usr=$(grep -n '^LABWC_FULLSCREEN_SPAN_OUTPUTS=0$' "$MERGED_ENV" | head -1 | cut -d: -f1)
if [[ -n $l_sys && -n $l_gen && -n $l_usr && $l_sys -lt $l_gen && $l_gen -lt $l_usr ]]; then
    ok "the order is system, generated, user"
else
    bad "order wrong: system=$l_sys generated=$l_gen user=$l_usr"
fi

if ! grep -q 'XKB_DEFAULT_LAYOUT=fr' "$MERGED_ENV" && ! grep -q 'XKB_DEFAULT_VARIANT=oss' "$MERGED_ENV"; then
    ok "managed keys are dropped from the user file"
else
    bad "a managed key from the user file reached the merged file"
fi

# labwc trims the key, so a blank before "=" is the same key.
if ! grep -q 'XKB_DEFAULT_LAYOUT *= *fr' "$MERGED_ENV"; then
    ok "a managed key with a blank before = is dropped too"
else
    bad "a managed key written 'KEY = value' reached the merged file"
fi

if [[ "$(grep '^XKB_DEFAULT_LAYOUT=' "$MERGED_ENV" | tail -1)" == "XKB_DEFAULT_LAYOUT=de" ]]; then
    ok "the last XKB_DEFAULT_LAYOUT is the generated one"
else
    bad "the last XKB_DEFAULT_LAYOUT is not the generated one"
fi

if grep -q '^XCURSOR_THEME=redglass$' "$MERGED_ENV" && grep -q '^# a comment line stays$' "$MERGED_ENV" &&
   [[ "$(grep '^LABWC_FULLSCREEN_SPAN_OUTPUTS=' "$MERGED_ENV" | tail -1)" == "LABWC_FULLSCREEN_SPAN_OUTPUTS=0" ]]; then
    ok "user-only and unmanaged keys survive"
else
    bad "user-only or unmanaged keys lost"
fi

# --- idempotency: run again into the SAME output dir, compare -----------

cp "$MERGED_ENV" "$WORK/merged-env.first"
"$HELPER" "$SYSDIR" "$GENENV" "$USERENV" "$OUT" >/dev/null

if diff -q "$WORK/merged-env.first" "$MERGED_ENV" >/dev/null; then
    ok "running the helper twice into the same output dir is idempotent"
else
    bad "a second run changed the merged environment file (should be identical)"
fi

REPEAT_COUNT=$(grep -c '^LABWC_FULLSCREEN_SPAN_OUTPUTS=' "$MERGED_ENV")
if [[ "$REPEAT_COUNT" -eq 2 ]]; then
    ok "duplicate key count unchanged by a second run (no append)"
else
    bad "running the helper twice produced $REPEAT_COUNT occurrences of a duplicate key, expected 2 (not growing)"
fi

# --- absent user file is not an error, system values still present -------

OUT3="$WORK/merged-nouserfile"
if "$HELPER" "$SYSDIR" "$WORK/no-gen" "$WORK/does-not-exist" "$OUT3"; then
    ok "absent user file is not an error"
else
    bad "helper failed when the user file does not exist"
fi

if [[ -f "$OUT3/environment" ]] && grep -q '^XKB_DEFAULT_LAYOUT=us$' "$OUT3/environment" &&
   grep -q '^LABWC_FULLSCREEN_SPAN_OUTPUTS=1$' "$OUT3/environment"; then
    ok "merged file still has the system values with no user file"
else
    bad "merged file missing system values when user file is absent"
fi

# --- --env-only ---------------------------------------------------------

OUT4="$WORK/envonly"
"$HELPER" "$SYSDIR" "$GENENV" "$USERENV" "$OUT4" >/dev/null
rm "$OUT4/rc.xml"; echo scaled >"$OUT4/rc.xml"
themelink=$(readlink "$OUT4/themerc-override")
GEN2="$WORK/generated-environment2"
sed 's/=de$/=ch/' "$GENENV" >"$GEN2"
"$HELPER" --env-only "$SYSDIR" "$GEN2" "$USERENV" "$OUT4"; rc=$?
if [[ $rc -eq 0 && -f "$OUT4/rc.xml" && ! -L "$OUT4/rc.xml" && "$(cat "$OUT4/rc.xml")" == scaled &&
      "$(readlink "$OUT4/themerc-override")" == "$themelink" &&
      "$(grep '^XKB_DEFAULT_LAYOUT=' "$OUT4/environment" | tail -1)" == "XKB_DEFAULT_LAYOUT=ch" &&
      -z "$(ls -A "$OUT4" | grep '^[.]environment[.]')" ]]; then
    ok "--env-only rewrites only environment"
else
    bad "--env-only rc=$rc or touched more than environment"
fi

OUT5="$WORK/not-there"
msg=$("$HELPER" --env-only "$SYSDIR" "$GENENV" "$USERENV" "$OUT5" 2>&1); rc=$?
if [[ $rc -eq 1 && "$msg" == *"$OUT5"* && ! -e "$OUT5" ]]; then
    ok "--env-only without a composed dir fails"
else
    bad "--env-only without a composed dir: rc=$rc msg=$msg"
fi

# --- the keyboard check ---------------------------------------------------
# labwc 0.9.3 segfaults at start on a layout/variant pair XKB cannot build,
# so the composer asks the menu (--check-keyboard) about the values labwc
# will read and appends the repaired ones.

CHECKER="$WORK/checker"
cat >"$CHECKER" <<'EOF'
#!/bin/sh
printf '%s|%s|%s\n' "$1" "$2" "$3" >>"$CHECK_LOG"
case "$2" in
    bad) echo "checker: bad is no layout" >&2
         printf 'XKB_DEFAULT_LAYOUT=us\nXKB_DEFAULT_VARIANT=\n' ;;
    broken) exit 3 ;;
esac
EOF
chmod +x "$CHECKER"
export CHECK_LOG="$WORK/check.log"

# Last assignment wins, as in labwc; key and value trimmed, quotes kept.
KBGEN="$WORK/kb-generated"
printf 'XKB_DEFAULT_LAYOUT=ch\nXKB_DEFAULT_VARIANT=fr\n  XKB_DEFAULT_LAYOUT = "de" \n' >"$KBGEN"
OUT6="$WORK/kb-good"
: >"$CHECK_LOG"
err=$(TF_KB_CHECK="$CHECKER" "$HELPER" "$SYSDIR" "$KBGEN" "$USERENV" "$OUT6" 2>&1); rc=$?
if [[ $rc -eq 0 && "$(cat "$CHECK_LOG")" == '--check-keyboard|"de"|fr' && -z "$err" ]] &&
   diff -q <(cat "$SYSDIR/environment" "$KBGEN" <(grep -v -e XKB_DEFAULT_LAYOUT -e XKB_DEFAULT_VARIANT "$USERENV")) \
        "$OUT6/environment" >/dev/null; then
    ok "the checker gets the values labwc reads; a good pair changes nothing"
else
    bad "keyboard check of a good pair: rc=$rc log=$(cat "$CHECK_LOG") err=$err"
fi

printf 'XKB_DEFAULT_LAYOUT=bad\nXKB_DEFAULT_VARIANT=x\n' >"$KBGEN"
OUT7="$WORK/kb-bad"
err=$(TF_KB_CHECK="$CHECKER" "$HELPER" "$SYSDIR" "$KBGEN" "$USERENV" "$OUT7" 2>&1); rc=$?
last=$(grep -v '^#' "$OUT7/environment" | tail -n 2)
if [[ $rc -eq 0 && "$last" == $'XKB_DEFAULT_LAYOUT=us\nXKB_DEFAULT_VARIANT=' && "$err" == *"bad is no layout"* ]] &&
   grep -q '^XKB_DEFAULT_LAYOUT=bad$' "$OUT7/environment"; then
    ok "a pair labwc cannot build is overridden at the end, with the reason on stderr"
else
    bad "keyboard check of a bad pair: rc=$rc err=$err env=$(cat "$OUT7/environment")"
fi

err=$(TF_KB_CHECK="$CHECKER" "$HELPER" --env-only "$SYSDIR" "$KBGEN" "$USERENV" "$OUT6" 2>&1); rc=$?
last=$(grep -v '^#' "$OUT6/environment" | tail -n 2)
if [[ $rc -eq 0 && "$last" == $'XKB_DEFAULT_LAYOUT=us\nXKB_DEFAULT_VARIANT=' ]] &&
   [[ -z "$(find "$OUT6" -name '.environment.*')" ]]; then
    ok "--env-only checks the keyboard too"
else
    bad "--env-only keyboard check: rc=$rc err=$err env=$(cat "$OUT6/environment")"
fi

# A checker that fails says nothing about the keyboard: the file stays.
printf 'XKB_DEFAULT_LAYOUT=broken\n' >"$KBGEN"
OUT8="$WORK/kb-broken"
TF_KB_CHECK="$CHECKER" "$HELPER" "$SYSDIR" "$KBGEN" "$USERENV" "$OUT8" 2>/dev/null; rc=$?
if [[ $rc -eq 0 && "$(grep XKB_DEFAULT_LAYOUT "$OUT8/environment" | tail -n 1)" == XKB_DEFAULT_LAYOUT=broken ]]; then
    ok "a failing checker leaves the environment as composed"
else
    bad "failing checker: rc=$rc env=$(cat "$OUT8/environment")"
fi

# A checker that hangs is stopped and changes nothing: the profile runs the
# composer before labwc, and a hang would leave tty1 without a session.
HANGER="$WORK/hanger"
printf '#!/bin/sh\nexec sleep 8\n' >"$HANGER"
chmod +x "$HANGER"
printf 'XKB_DEFAULT_LAYOUT=hangs\n' >"$KBGEN"
OUT9="$WORK/kb-hangs"
start=$SECONDS
TF_KB_CHECK="$HANGER" TF_KB_CHECK_TIMEOUT=1 "$HELPER" "$SYSDIR" "$KBGEN" "$USERENV" "$OUT9" 2>/dev/null; rc=$?
took=$((SECONDS - start))
if [[ $rc -eq 0 && $took -lt 5 &&
      "$(grep XKB_DEFAULT_LAYOUT "$OUT9/environment" | tail -n 1)" == XKB_DEFAULT_LAYOUT=hangs ]]; then
    ok "a hanging checker is stopped and leaves the environment as composed"
else
    bad "hanging checker: rc=$rc took=${took}s env=$(cat "$OUT9/environment" 2>/dev/null)"
fi

# --- Num Lock ----------------------------------------------------------------
# THINFLOE_NUMLOCK=off in generated/environment: .source/base.xml is a copy
# that says off; on, missing, or set only in the user's file: base.xml links
# the system file. rc.xml links .source/rc.xml in every case.
NLSYS="$WORK/nl-system"
mkdir -p "$NLSYS"
cp "$SYSDIR/environment" "$SYSDIR/themerc-override" "$NLSYS/"
printf '%s\n' '<labwc_config>' '  <keyboard>' '    <numlock>on</numlock>' \
    '    <repeatDelay>500</repeatDelay>' '  </keyboard>' '</labwc_config>' >"$NLSYS/rc.xml"
NLGEN="$WORK/nl-gen"
NLUSER="$WORK/nl-user"
: >"$NLUSER"
NLMANAGED="$WORK/nl-managed"
printf 'XKB_DEFAULT_LAYOUT\nTHINFLOE_NUMLOCK\n' >"$NLMANAGED"
nl_compose() { TF_MANAGED_KEYS="$NLMANAGED" "$HELPER" "$@"; }

printf 'XKB_DEFAULT_LAYOUT=de\nTHINFLOE_NUMLOCK=off\n' >"$NLGEN"
OUTN="$WORK/nl-off"
if nl_compose "$NLSYS" "$NLGEN" "$NLUSER" "$OUTN" &&
    [[ -L "$OUTN/rc.xml" && "$(readlink "$OUTN/rc.xml")" == "$OUTN/.source/rc.xml" ]] &&
    [[ -f "$OUTN/.source/base.xml" && ! -L "$OUTN/.source/base.xml" ]] &&
    [[ "$(readlink -f "$OUTN/rc.xml")" == "$OUTN/.source/base.xml" ]] &&
    grep -q '<numlock>off</numlock>' "$OUTN/rc.xml" &&
    ! grep -q '<numlock>on' "$OUTN/rc.xml" &&
    diff <(grep -v numlock "$OUTN/rc.xml") <(grep -v numlock "$NLSYS/rc.xml") >/dev/null; then
    ok "Num Lock off: base.xml is a copy that says off, otherwise unchanged"
else
    bad "Num Lock off: rc.xml is $(readlink -f "$OUTN/rc.xml"): $(cat "$OUTN/rc.xml" 2>/dev/null)"
fi

# A system rc.xml without a <numlock> line gets one in <keyboard>.
NLSYS2="$WORK/nl-system2"
mkdir -p "$NLSYS2"
cp "$NLSYS/environment" "$NLSYS/themerc-override" "$NLSYS2/"
grep -v numlock "$NLSYS/rc.xml" >"$NLSYS2/rc.xml"
OUTN2="$WORK/nl-insert"
if nl_compose "$NLSYS2" "$NLGEN" "$NLUSER" "$OUTN2" &&
    [[ "$(sed -n '/<keyboard>/,/<\/keyboard>/p' "$OUTN2/rc.xml" | grep -c '<numlock>off</numlock>')" -eq 1 ]]; then
    ok "Num Lock off: a missing <numlock> line is added to <keyboard>"
else
    bad "Num Lock off without a system line: $(cat "$OUTN2/rc.xml" 2>/dev/null)"
fi

for v in on missing user; do
    case $v in
        on) printf 'THINFLOE_NUMLOCK=on\n' >"$NLGEN"; : >"$NLUSER" ;;
        missing) printf 'XKB_DEFAULT_LAYOUT=de\n' >"$NLGEN"; : >"$NLUSER" ;;
        user) printf 'XKB_DEFAULT_LAYOUT=de\n' >"$NLGEN"; printf 'THINFLOE_NUMLOCK=off\n' >"$NLUSER" ;;
    esac
    # Not nl-user: that is $NLUSER, which the compose would wipe.
    OUTV="$WORK/nl-out-$v"
    if nl_compose "$NLSYS" "$NLGEN" "$NLUSER" "$OUTV" &&
        [[ -L "$OUTV/.source/base.xml" && "$(readlink "$OUTV/.source/base.xml")" == "$NLSYS/rc.xml" &&
           "$(readlink -f "$OUTV/rc.xml")" == "$(readlink -f "$NLSYS/rc.xml")" && ! -e "$OUTV/.numlock" ]]; then
        ok "Num Lock $v: base.xml links the system file"
    else
        bad "Num Lock $v: rc.xml is $(readlink -f "$OUTV/rc.xml")"
    fi
done

# A quoted "off" counts, as the menu's rebuild reads it.
printf 'THINFLOE_NUMLOCK="off"\n' >"$NLGEN"; : >"$NLUSER"
OUTQ="$WORK/nl-quoted"
if nl_compose "$NLSYS" "$NLGEN" "$NLUSER" "$OUTQ" &&
    grep -q '<numlock>off</numlock>' "$OUTQ/rc.xml"; then
    ok "Num Lock: a quoted \"off\" counts"
else
    bad "Num Lock: quoted off: $(cat "$OUTQ/rc.xml" 2>/dev/null)"
fi

# A <numlock> mention in a comment outside <keyboard> is left alone; the
# element is still added to <keyboard>.
NLSYS3="$WORK/nl-system3"
mkdir -p "$NLSYS3"
cp "$NLSYS/environment" "$NLSYS/themerc-override" "$NLSYS3/"
printf '%s\n' '<labwc_config>' '  <!-- was <numlock>on</numlock> -->' '  <keyboard>' \
    '    <repeatDelay>500</repeatDelay>' '  </keyboard>' '</labwc_config>' >"$NLSYS3/rc.xml"
printf 'THINFLOE_NUMLOCK=off\n' >"$NLGEN"
OUTC="$WORK/nl-comment"
if nl_compose "$NLSYS3" "$NLGEN" "$NLUSER" "$OUTC" &&
    grep -q '<!-- was <numlock>on</numlock> -->' "$OUTC/rc.xml" &&
    [[ "$(sed -n '/<keyboard>/,/<\/keyboard>/p' "$OUTC/rc.xml" | grep -c '<numlock>off</numlock>')" -eq 1 ]]; then
    ok "Num Lock off: a comment is left alone and the element is added"
else
    bad "Num Lock off with a comment: $(cat "$OUTC/rc.xml" 2>/dev/null)"
fi

# --env-only never touches base.xml, even when the value changed to off.
printf 'THINFLOE_NUMLOCK=off\n' >"$NLGEN"; : >"$NLUSER"
if nl_compose --env-only "$NLSYS" "$NLGEN" "$NLUSER" "$WORK/nl-out-on" &&
    [[ "$(readlink "$WORK/nl-out-on/.source/base.xml")" == "$NLSYS/rc.xml" &&
       "$(readlink "$WORK/nl-out-on/rc.xml")" == "$WORK/nl-out-on/.source/rc.xml" ]] &&
    grep -q '<numlock>on</numlock>' "$WORK/nl-out-on/rc.xml"; then
    ok "Num Lock: --env-only leaves base.xml alone"
else
    bad "Num Lock: --env-only changed base.xml to $(readlink "$WORK/nl-out-on/.source/base.xml")"
fi

# --- media keys -------------------------------------------------------------
# THINFLOE_VOLUME_KEYS / THINFLOE_BRIGHTNESS_KEYS=on in generated/environment:
# .source/rc.xml is base.xml with the bindings on the lines before
# </keyboard>. Both off: .source/rc.xml links base.xml. The values may be
# quoted, as for THINFLOE_NUMLOCK.
MKUSER="$WORK/mk-user"
: >"$MKUSER"
MKGEN="$WORK/mk-gen"
MKMANAGED="$WORK/mk-managed"
printf 'THINFLOE_NUMLOCK\nTHINFLOE_VOLUME_KEYS\nTHINFLOE_BRIGHTNESS_KEYS\n' >"$MKMANAGED"
mk_compose() { TF_MANAGED_KEYS="$MKMANAGED" "$HELPER" "$@"; }
VOL_KEYS='XF86AudioLowerVolume XF86AudioMicMute XF86AudioMute XF86AudioRaiseVolume'
BRI_KEYS='XF86MonBrightnessDown XF86MonBrightnessUp'
# The key names of the bindings in $1, sorted, on one line.
keys_in() { grep -o '<keybind key="[^"]*"' "$1" | sed 's/.*key="//; s/"$//' | LC_ALL=C sort | tr '\n' ' ' | sed 's/ $//'; }
kb_lines() { grep -c '<keybind' "$1"; }
inserted() { grep -e '<keybind' -e '<!-- thinfloe-compose-labwc-config: THINFLOE_' "$1"; }
# $1 without the inserted lines is $2.
stripped_is() { diff <(grep -v -e '<keybind' -e '<!-- thinfloe-compose-labwc-config: THINFLOE_' "$1") "$2" >/dev/null; }
in_keyboard() { [[ $(sed -n '/<keyboard>/,/<\/keyboard>/p' "$1" | grep -c '<keybind') -eq $(kb_lines "$1") ]]; }
keys_off() { [[ "$(readlink "$1/.source/rc.xml")" == "$1/.source/base.xml" ]] && [[ $(kb_lines "$1/rc.xml") -eq 0 ]]; }

VOL_TEXT='    <!-- thinfloe-compose-labwc-config: THINFLOE_VOLUME_KEYS=on -->
    <keybind key="XF86AudioRaiseVolume" allowWhenInhibited="yes"><action name="Execute" command="/usr/bin/thinfloe-menu --osd-key volume-up" /></keybind>
    <keybind key="XF86AudioLowerVolume" allowWhenInhibited="yes"><action name="Execute" command="/usr/bin/thinfloe-menu --osd-key volume-down" /></keybind>
    <keybind key="XF86AudioMute" allowWhenInhibited="yes" onRelease="yes"><action name="Execute" command="/usr/bin/thinfloe-menu --osd-key volume-mute" /></keybind>
    <keybind key="XF86AudioMicMute" allowWhenInhibited="yes" onRelease="yes"><action name="Execute" command="/usr/bin/thinfloe-menu --osd-key mic-mute" /></keybind>'
BRI_TEXT='    <!-- thinfloe-compose-labwc-config: THINFLOE_BRIGHTNESS_KEYS=on -->
    <keybind key="XF86MonBrightnessUp" allowWhenInhibited="yes"><action name="Execute" command="/usr/bin/thinfloe-menu --osd-key brightness-up" /></keybind>
    <keybind key="XF86MonBrightnessDown" allowWhenInhibited="yes"><action name="Execute" command="/usr/bin/thinfloe-menu --osd-key brightness-down" /></keybind>'

printf 'THINFLOE_VOLUME_KEYS=on\n' >"$MKGEN"
OUTK="$WORK/mk-vol"
if mk_compose "$NLSYS" "$MKGEN" "$MKUSER" "$OUTK" &&
    [[ "$(readlink "$OUTK/rc.xml")" == "$OUTK/.source/rc.xml" &&
       -f "$OUTK/.source/rc.xml" && ! -L "$OUTK/.source/rc.xml" ]] &&
    [[ $(kb_lines "$OUTK/rc.xml") -eq 4 && "$(keys_in "$OUTK/rc.xml")" == "$VOL_KEYS" ]]; then
    ok "volume keys on: exactly the four volume keys"
else
    bad "volume keys on: $(inserted "$OUTK/rc.xml" 2>/dev/null)"
fi
[[ $(grep '<keybind' "$OUTK/rc.xml" | grep -c 'allowWhenInhibited="yes"') -eq 4 &&
   $(grep -c '<keybind key="XF86AudioMute" .*onRelease="yes"' "$OUTK/rc.xml") -eq 1 &&
   $(grep -c '<keybind key="XF86AudioMicMute" .*onRelease="yes"' "$OUTK/rc.xml") -eq 1 &&
   $(grep -c 'onRelease' "$OUTK/rc.xml") -eq 2 ]] \
    && ok "volume keys on: allowWhenInhibited on all, onRelease on the mute keys" \
    || bad "volume keys on: attributes: $(inserted "$OUTK/rc.xml")"
in_keyboard "$OUTK/rc.xml" && ok "volume keys on: inside <keyboard>" || bad "volume keys on: outside <keyboard>"
[[ "$(inserted "$OUTK/rc.xml")" == "$VOL_TEXT" ]] \
    && ok "volume keys on: the exact lines" || bad "volume keys on: lines: $(inserted "$OUTK/rc.xml")"
stripped_is "$OUTK/rc.xml" "$NLSYS/rc.xml" \
    && ok "volume keys on: otherwise the system file" || bad "volume keys on: other lines differ"

printf 'THINFLOE_BRIGHTNESS_KEYS=on\n' >"$MKGEN"
OUTK="$WORK/mk-bri"
if mk_compose "$NLSYS" "$MKGEN" "$MKUSER" "$OUTK" &&
    [[ $(kb_lines "$OUTK/rc.xml") -eq 2 && "$(keys_in "$OUTK/rc.xml")" == "$BRI_KEYS" &&
       "$(inserted "$OUTK/rc.xml")" == "$BRI_TEXT" ]] && in_keyboard "$OUTK/rc.xml" &&
    stripped_is "$OUTK/rc.xml" "$NLSYS/rc.xml"; then
    ok "brightness keys alone: exactly the two brightness keys"
else
    bad "brightness keys alone: $(inserted "$OUTK/rc.xml" 2>/dev/null)"
fi

printf 'THINFLOE_VOLUME_KEYS=on\nTHINFLOE_BRIGHTNESS_KEYS=on\n' >"$MKGEN"
OUTK="$WORK/mk-both"
if TF_OSD_KEY_CMD=/x/osd mk_compose "$NLSYS" "$MKGEN" "$MKUSER" "$OUTK" &&
    [[ $(kb_lines "$OUTK/rc.xml") -eq 6 && "$(keys_in "$OUTK/rc.xml")" == "$(printf '%s\n' $VOL_KEYS $BRI_KEYS | LC_ALL=C sort | tr '\n' ' ' | sed 's/ $//')" ]] &&
    [[ $(grep -o 'command="[^"]*"' "$OUTK/rc.xml" | grep -c '^command="/x/osd ') -eq 6 &&
       $(grep -c 'command="' "$OUTK/rc.xml") -eq 6 ]] && in_keyboard "$OUTK/rc.xml"; then
    ok "both on: six keys, TF_OSD_KEY_CMD is the command prefix"
else
    bad "both on: $(inserted "$OUTK/rc.xml" 2>/dev/null)"
fi

for v in '"on"' "'on'"; do
    printf 'THINFLOE_VOLUME_KEYS=%s\n' "$v" >"$MKGEN"
    OUTK="$WORK/mk-q"
    if mk_compose "$NLSYS" "$MKGEN" "$MKUSER" "$OUTK" && [[ $(kb_lines "$OUTK/rc.xml") -eq 4 ]]; then
        ok "volume keys $v counts as on"
    else
        bad "volume keys $v: $(kb_lines "$OUTK/rc.xml") bindings"
    fi
done
for v in yes 1 '' off; do
    printf 'THINFLOE_VOLUME_KEYS=%s\nTHINFLOE_BRIGHTNESS_KEYS=%s\n' "$v" "$v" >"$MKGEN"
    OUTK="$WORK/mk-off"
    if mk_compose "$NLSYS" "$MKGEN" "$MKUSER" "$OUTK" && keys_off "$OUTK"; then
        ok "key switches '$v' count as off"
    else
        bad "key switches '$v': $(readlink "$OUTK/.source/rc.xml"), $(kb_lines "$OUTK/rc.xml") bindings"
    fi
done

# Set only in the user's file: a managed key, dropped.
: >"$MKGEN"; printf 'THINFLOE_VOLUME_KEYS=on\n' >"$WORK/mk-user2"
OUTK="$WORK/mk-user-only"
mk_compose "$NLSYS" "$MKGEN" "$WORK/mk-user2" "$OUTK" && keys_off "$OUTK" \
    && ok "volume keys set only in the user's file: off" || bad "volume keys in the user's file count"

printf 'THINFLOE_NUMLOCK=off\nTHINFLOE_VOLUME_KEYS=on\n' >"$MKGEN"
OUTK="$WORK/mk-nl"
if mk_compose "$NLSYS" "$MKGEN" "$MKUSER" "$OUTK" &&
    [[ -f "$OUTK/.source/base.xml" && ! -L "$OUTK/.source/base.xml" ]] &&
    grep -q '<numlock>off</numlock>' "$OUTK/.source/base.xml" &&
    grep -q '<numlock>off</numlock>' "$OUTK/.source/rc.xml" &&
    [[ $(kb_lines "$OUTK/.source/rc.xml") -eq 4 ]] &&
    stripped_is "$OUTK/.source/rc.xml" "$OUTK/.source/base.xml"; then
    ok "Num Lock off and volume on: base.xml says off, rc.xml has it and the keys"
else
    bad "Num Lock off and volume on: $(cat "$OUTK/.source/rc.xml" 2>/dev/null)"
fi

# --env-only follows the switches in .source/rc.xml and leaves base.xml alone.
: >"$MKGEN"
OUTK="$WORK/mk-envonly"
mk_compose "$NLSYS" "$MKGEN" "$MKUSER" "$OUTK"
link_before=$(readlink "$OUTK/rc.xml")
printf 'THINFLOE_NUMLOCK=off\nTHINFLOE_VOLUME_KEYS=on\n' >"$MKGEN"
err=$(mk_compose --env-only "$NLSYS" "$MKGEN" "$MKUSER" "$OUTK" 2>&1); rc=$?
if [[ $rc -eq 0 && -z "$err" && -f "$OUTK/.source/rc.xml" && ! -L "$OUTK/.source/rc.xml" &&
      $(kb_lines "$OUTK/.source/rc.xml") -eq 4 &&
      "$(readlink "$OUTK/rc.xml")" == "$link_before" &&
      "$(readlink "$OUTK/.source/base.xml")" == "$NLSYS/rc.xml" ]] &&
   ! grep -q '<numlock>off' "$OUTK/.source/rc.xml" &&
   [[ -z "$(ls -A "$OUTK/.source" | grep -v -x -e base.xml -e rc.xml)" ]]; then
    ok "--env-only: volume on adds the keys, base.xml and the rc.xml link stay"
else
    bad "--env-only volume on: rc=$rc err=$err $(ls -lA "$OUTK/.source")"
fi
: >"$MKGEN"
mk_compose --env-only "$NLSYS" "$MKGEN" "$MKUSER" "$OUTK" && keys_off "$OUTK" \
    && ok "--env-only: switching off links base.xml again" || bad "--env-only switch off: $(readlink "$OUTK/.source/rc.xml")"

# --env-only on a dir without .source/base.xml: only environment, a note.
OUTK="$WORK/mk-nosource"
mkdir -p "$OUTK"
printf 'THINFLOE_VOLUME_KEYS=on\n' >"$MKGEN"
err=$(mk_compose --env-only "$NLSYS" "$MKGEN" "$MKUSER" "$OUTK" 2>&1); rc=$?
if [[ $rc -eq 0 && "$err" == "thinfloe-compose-labwc-config: no .source/base.xml; the key switches follow at the next start" &&
      ! -e "$OUTK/.source" && -f "$OUTK/environment" ]]; then
    ok "--env-only without .source/base.xml: environment only, a note, exit 0"
else
    bad "--env-only without .source/base.xml: rc=$rc err=$err $(ls -A "$OUTK")"
fi

# --env-only with an unwritable .source/: unchanged, a line, exit 0.
if [[ $(id -u) -eq 0 ]]; then
    ok "unwritable .source/: skipped as root"
else
    printf 'THINFLOE_VOLUME_KEYS=on\n' >"$MKGEN"
    OUTK="$WORK/mk-ro"
    mk_compose "$NLSYS" "$MKGEN" "$MKUSER" "$OUTK"
    sum_before=$(cksum <"$OUTK/.source/rc.xml")
    chmod 500 "$OUTK/.source"
    printf 'THINFLOE_VOLUME_KEYS=on\nTHINFLOE_BRIGHTNESS_KEYS=on\n' >"$MKGEN"
    err=$(mk_compose --env-only "$NLSYS" "$MKGEN" "$MKUSER" "$OUTK" 2>&1); rc=$?
    chmod 700 "$OUTK/.source"
    if [[ $rc -eq 0 && "$err" == "thinfloe-compose-labwc-config: cannot write the key bindings; the volume and brightness keys stay off" &&
          "$(cksum <"$OUTK/.source/rc.xml")" == "$sum_before" ]] &&
       grep -q '^THINFLOE_BRIGHTNESS_KEYS=on$' "$OUTK/environment"; then
        ok "unwritable .source/: rc.xml unchanged, the line on stderr, exit 0"
    else
        bad "unwritable .source/: rc=$rc err=$err"
    fi
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - labwc config merge helper"
    exit 1
fi

echo ""
echo "PASSED - labwc config merge helper"
exit 0
