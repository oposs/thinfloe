#!/bin/bash
# Regression test for the ~/.thinfloe-environment override bug.
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

USERENV="$WORK/user-environment"
cat >"$USERENV" <<'EOF'
LABWC_FULLSCREEN_SPAN_OUTPUTS=0
XCURSOR_THEME=whiteglass
EOF

OUT="$WORK/merged"

# --- run 1 ---------------------------------------------------------------

if ! "$HELPER" "$SYSDIR" "$USERENV" "$OUT"; then
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
if grep -q '^XCURSOR_THEME=whiteglass$' "$MERGED_ENV"; then
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

# --- idempotency: run again into the SAME output dir, compare -----------

cp "$MERGED_ENV" "$WORK/merged-env.first"
"$HELPER" "$SYSDIR" "$USERENV" "$OUT" >/dev/null

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
if "$HELPER" "$SYSDIR" "$WORK/does-not-exist" "$OUT3"; then
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

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - labwc config merge helper"
    exit 1
fi

echo ""
echo "PASSED - labwc config merge helper"
exit 0
