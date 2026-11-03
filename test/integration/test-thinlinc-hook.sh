#!/bin/bash
# Offline tests for overlay/usr/libexec/thinfloe/addon-hooks/10-thinlinc, the
# hook thinfloe-addon runs after it installs the ThinLinc client. It puts the
# PipeWire shim over the client's PulseAudio; nothing else. A temporary
# directory stands in for /opt/thinlinc (TF_THINLINC_DIR); TF_SHIM points at
# the overlay file.
#
# Usage: test/integration/test-thinlinc-hook.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
HOOK="$REPO/overlay/usr/libexec/thinfloe/addon-hooks/10-thinlinc"
SHIM="$REPO/overlay/usr/libexec/thinfloe/tlclient-pulseaudio"

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

: "${TMPDIR:=/scratch/oetiker/claude-tmp/thinfloe-tests}"
mkdir -p "$TMPDIR"
work=$(mktemp -d "$TMPDIR/test-thinlinc-hook.XXXXXX")
trap 'rm -rf "$work"' EXIT

# A fresh client tree: lib/tlclient/pulseaudio as the package installs it.
tree() {
    local d="$work/$1"
    mkdir -p "$d/lib/tlclient" "$d/etc"
    printf 'package pulseaudio\n' > "$d/lib/tlclient/pulseaudio"
    echo "$d"
}
run() {  # run DIR [SHIM]: output in $out, exit status in $rc
    out=$(TF_THINLINC_DIR="$1" TF_SHIM="${2:-$SHIM}" sh "$HOOK" 2>&1)
    rc=$?
}

d=$(tree plain)
run "$d"
if [[ $rc -eq 0 ]] && cmp -s "$d/lib/tlclient/pulseaudio" "$SHIM" &&
   [[ -x "$d/lib/tlclient/pulseaudio" ]]; then
    ok "the PipeWire shim replaces the client's pulseaudio"
else
    bad "the PipeWire shim was not installed: rc=$rc, $out"
fi

d=$(tree noshim)
rm "$d/lib/tlclient/pulseaudio"
run "$d"
if [[ $rc -eq 0 && ! -e "$d/lib/tlclient/pulseaudio" ]]; then
    ok "without the client's pulseaudio nothing is created"
else
    bad "no pulseaudio: rc=$rc, $out"
fi

d=$(tree badshim)
run "$d" "$work/no-such-shim"
if [[ $rc -ne 0 ]]; then
    ok "a failing shim copy fails the hook"
else
    bad "a failing shim copy did not fail the hook"
fi

d=$(tree mandatory)
printf 'PRINTER_ENABLED=0\nNFS_SERVER_ENABLED=0\n' > "$d/etc/tlclient_mandatory.conf"
cp "$d/etc/tlclient_mandatory.conf" "$work/mandatory.orig"
run "$d"
if [[ $rc -eq 0 ]] && cmp -s "$d/etc/tlclient_mandatory.conf" "$work/mandatory.orig"; then
    ok "the mandatory config is left byte for byte as it was"
else
    bad "the mandatory config changed: rc=$rc"
fi

d=$(tree nomandatory)
run "$d"
if [[ $rc -eq 0 && ! -e "$d/etc/tlclient_mandatory.conf" ]]; then
    ok "no mandatory config is created"
else
    bad "a mandatory config appeared: rc=$rc"
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - 10-thinlinc hook"
    exit 1
fi
echo ""
echo "PASSED - 10-thinlinc hook"
