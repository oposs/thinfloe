#!/bin/bash
# Offline tests for overlay/usr/libexec/thinfloe/addon-hooks/10-thinlinc, the
# hook thinfloe-addon runs after it installs the ThinLinc client. It puts the
# PipeWire shim over the client's PulseAudio and exports the battery file
# (/run/thinfloe/peripherals) read-only to the session through
# tlclient_mandatory.conf. A temporary directory stands in for /opt/thinlinc
# (TF_THINLINC_DIR); TF_SHIM points at the shim in the overlay.
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

EXPECTED='NFS_SERVER_ENABLED=1
NFS_EXPORTS=/run/thinfloe/peripherals,ro'

# A fresh client tree: lib/tlclient/pulseaudio as the package installs it,
# etc/ with an empty mandatory file unless told otherwise.
tree() {
    local d="$work/$1"
    mkdir -p "$d/lib/tlclient" "$d/etc"
    printf 'package pulseaudio\n' > "$d/lib/tlclient/pulseaudio"
    : > "$d/etc/tlclient_mandatory.conf"
    echo "$d"
}
run() {  # run DIR [SHIM]: output in $out, exit status in $rc
    out=$(TF_THINLINC_DIR="$1" TF_SHIM="${2:-$SHIM}" sh "$HOOK" 2>&1)
    rc=$?
}

d=$(tree empty)
run "$d"
if [[ $rc -eq 0 && "$(cat "$d/etc/tlclient_mandatory.conf")" == "$EXPECTED" ]]; then
    ok "empty mandatory file gets the two export lines"
else
    bad "empty mandatory file: rc=$rc, file: $(cat "$d/etc/tlclient_mandatory.conf")"
fi
if cmp -s "$d/lib/tlclient/pulseaudio" "$SHIM" && [[ -x "$d/lib/tlclient/pulseaudio" ]]; then
    ok "the PipeWire shim replaces the client's pulseaudio"
else
    bad "the PipeWire shim was not installed"
fi

d=$(tree other)
printf 'PRINTER_ENABLED=0\nNFS_EXPORTS=/media,rw\nNFS_SERVER_ENABLED=0\n' > "$d/etc/tlclient_mandatory.conf"
run "$d"
if [[ "$(cat "$d/etc/tlclient_mandatory.conf")" == "PRINTER_ENABLED=0
$EXPECTED" ]]; then
    ok "other mandatory keys are kept, the export keys replaced"
else
    bad "other keys: file is: $(cat "$d/etc/tlclient_mandatory.conf")"
fi

cp "$d/etc/tlclient_mandatory.conf" "$work/first"
run "$d"
if cmp -s "$d/etc/tlclient_mandatory.conf" "$work/first"; then
    ok "a second run changes nothing"
else
    bad "a second run changed the file"
fi
if [[ -z "$(find "$d/etc" -name '*.new*' -o -name '.*' -type f)" ]]; then
    ok "no temporary file is left in etc/"
else
    bad "temporary file left: $(ls -a "$d/etc")"
fi

d=$(tree missing)
rm "$d/etc/tlclient_mandatory.conf"
run "$d"
if [[ "$(cat "$d/etc/tlclient_mandatory.conf" 2>/dev/null)" == "$EXPECTED" &&
      "$(stat -c %a "$d/etc/tlclient_mandatory.conf")" == 644 ]]; then
    ok "a missing mandatory file is created, mode 644"
else
    bad "missing mandatory file: $(ls -l "$d/etc" 2>&1)"
fi

d=$(tree noetc)
rm -r "$d/etc"
run "$d"
if [[ $rc -eq 0 && "$(cat "$d/etc/tlclient_mandatory.conf" 2>/dev/null)" == "$EXPECTED" ]]; then
    ok "a missing etc/ is created"
else
    bad "missing etc/: rc=$rc, $out"
fi

d=$(tree badetc)
rm -r "$d/etc"
: > "$d/etc"
run "$d"
if [[ $rc -eq 0 && "$out" == *"thinfloe: battery export not configured: "* ]] &&
   cmp -s "$d/lib/tlclient/pulseaudio" "$SHIM"; then
    ok "an unusable etc/ is logged, the hook still succeeds and installs the shim"
else
    bad "unusable etc/: rc=$rc, output: $out"
fi

d=$(tree noshim)
rm "$d/lib/tlclient/pulseaudio"
run "$d"
if [[ $rc -eq 0 && "$(cat "$d/etc/tlclient_mandatory.conf")" == "$EXPECTED" ]]; then
    ok "without the client's pulseaudio the export is still written"
else
    bad "no pulseaudio: rc=$rc, $out"
fi

d=$(tree badshim)
run "$d" "$work/no-such-shim"
if [[ $rc -ne 0 ]]; then
    ok "a failing shim copy still fails the hook"
else
    bad "a failing shim copy did not fail the hook"
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - 10-thinlinc hook"
    exit 1
fi
echo ""
echo "PASSED - 10-thinlinc hook"
