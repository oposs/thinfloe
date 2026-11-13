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

USERENV="$WORK/user-environment"
cat >"$USERENV" <<'EOF'
# a comment line stays
XKB_DEFAULT_LAYOUT=fr
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

if [[ -L "$OUT/rc.xml" ]]; then
    ok "rc.xml is a symlink, not a copy"
else
    bad "rc.xml should be a symlink into the system dir"
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

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - labwc config merge helper"
    exit 1
fi

echo ""
echo "PASSED - labwc config merge helper"
exit 0
