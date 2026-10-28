#!/bin/bash
# Offline tests for overlay/usr/libexec/thinfloe/plymouth-step, the helper
# that the running system uses to put a step on the boot screen and to hand
# the screen over to the session. A recording plymouth stub stands in for
# the real one (TF_PLYMOUTH).
#
# Usage: test/integration/test-plymouth-step.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
HELPER="$REPO/overlay/usr/libexec/thinfloe/plymouth-step"

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

: "${TMPDIR:=/scratch/oetiker/claude-tmp/thinfloe-tests}"
mkdir -p "$TMPDIR"
work=$(mktemp -d "$TMPDIR/test-plymouth-step.XXXXXX")
trap 'rm -rf "$work"' EXIT

# One line per call, arguments joined by "|". --ping fails when ping.fail exists.
stub="$work/plymouth"
cat > "$stub" <<'EOF'
#!/bin/bash
d=$(dirname "$0")
(IFS='|'; printf '%s\n' "$*") >> "$d/calls"
[[ "$1" == --ping && -e "$d/ping.fail" ]] && exit 1
echo "noise on stdout"
echo "noise on stderr" >&2
exit 0
EOF
chmod 755 "$stub"

run() {
    rm -f "$work/calls" "$work/id.ran"
    out=$(TF_PLYMOUTH="$stub" "$HELPER" "$@" 2>&1)
    rc=$?
    calls=$(cat "$work/calls" 2>/dev/null)
}

expect_calls() {
    local name=$1 want=$2
    if [[ $rc -ne 0 ]]; then
        bad "$name: exit $rc"
    elif [[ -n "$out" ]]; then
        bad "$name: printed '$out'"
    elif [[ "$calls" != "$want" ]]; then
        bad "$name: expected calls '$want', got '$calls'"
    else
        ok "$name"
    fi
}

[[ -x "$HELPER" ]] || { bad "helper missing or not executable: $HELPER"; exit 1; }

run status "Setting the clock"
expect_calls "status" $'--ping\nupdate|--status=tf:Setting the clock'
run deactivate
expect_calls "deactivate" $'--ping\ndeactivate'
run reactivate
expect_calls "reactivate" $'--ping\nreactivate'
run quit
expect_calls "quit" $'--ping\nquit'
run quit-retain
expect_calls "quit-retain" $'--ping\nquit|--retain-splash'
run bogus
expect_calls "unknown verb does nothing after the ping" $'--ping'

# A hostile text: quotes, a command substitution, spaces, 200 characters.
text="a \"b\" \$(touch $work/id.ran) $(printf 'x%.0s' $(seq 1 170))"
run status "$text"
want="update|--status=tf:${text:0:64}"
if [[ -e "$work/id.ran" ]]; then
    bad "hostile text: the command substitution ran"
else
    expect_calls "hostile text arrives as one argument, cut to 64" $'--ping\n'"$want"
fi

# The cut counts characters, not bytes: 70 "€" (3 bytes each) give 64 "€",
# never a broken last character. Control characters are removed first.
euro=$(printf '€%.0s' $(seq 1 70))
run status "$euro"
expect_calls "multi-byte text is cut at 64 characters" \
    $'--ping\nupdate|--status=tf:'"$(printf '€%.0s' $(seq 1 64))"
run status $'Install\033ing\nThin\tLinc\177'
expect_calls "control characters are removed" $'--ping\nupdate|--status=tf:InstallingThinLinc'

touch "$work/ping.fail"
for verb in status deactivate reactivate quit quit-retain; do
    run "$verb" "Some text"
    expect_calls "$verb with no Plymouth running: only the ping" $'--ping'
done
rm -f "$work/ping.fail"

out=$(TF_PLYMOUTH="$work/missing" "$HELPER" status "x" 2>&1); rc=$?
if [[ $rc -eq 0 && -z "$out" ]]; then
    ok "missing plymouth: exit 0, no output"
else
    bad "missing plymouth: exit $rc, output '$out'"
fi

exit $fail
