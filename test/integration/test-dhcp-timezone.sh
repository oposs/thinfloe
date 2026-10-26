#!/bin/bash
# Offline tests for /usr/libexec/thinfloe/dhcp-timezone.
#
# The script runs as root on the TIMEZONE= value (DHCP option 101) of a lease
# file, which any DHCP server on the LAN can write. Only a plain zone name
# that is a TZif file inside the zoneinfo directory may reach
# `timedatectl set-timezone`; everything else prints one `ignored` line, with
# the value cut and printable, and changes nothing.
#
# Runs without root: both directories come from environment variables and
# timedatectl is a stub on PATH that records its arguments.
#
# Usage: test/integration/test-dhcp-timezone.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$REPO/overlay/usr/libexec/thinfloe/dhcp-timezone"

if [[ ! -x "$SCRIPT" ]]; then
    echo "FAIL: $SCRIPT not found or not executable" >&2
    exit 1
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/zi/Europe" "$WORK/leases" "$WORK/bin"
printf 'TZif2xxxxxxxx' >"$WORK/zi/Europe/Zurich"
ln -s Zurich "$WORK/zi/Europe/Inside"
ln -s /etc/passwd "$WORK/zi/Europe/Escape"
printf 'not a zone file' >"$WORK/zi/Europe/Junk"
printf 'TZif2xxxxxxxx' >"$WORK/outside"
ln -s ../../outside "$WORK/zi/Europe/Outside"

cat >"$WORK/bin/timedatectl" <<STUB
#!/bin/bash
printf '%s\n' "\$*" >>"$WORK/calls"
STUB
chmod +x "$WORK/bin/timedatectl"

fails=0
fail() { echo "FAIL: $*" >&2; fails=$((fails + 1)); }

# run VALUE: write one lease holding TIMEZONE=VALUE, run the script.
run() {
    rm -f "$WORK"/leases/* "$WORK/calls" "$WORK/out"
    if [[ $# -gt 0 ]]; then
        printf 'ADDRESS=10.0.0.5\nTIMEZONE=%s\nDNS=10.0.0.1\n' "$1" >"$WORK/leases/2"
    else
        printf 'ADDRESS=10.0.0.5\nDNS=10.0.0.1\n' >"$WORK/leases/2"
    fi
    PATH="$WORK/bin:$PATH" THINFLOE_LEASE_DIR="$WORK/leases" \
        THINFLOE_ZONEINFO_DIR="$WORK/zi" "$SCRIPT" >"$WORK/out" 2>&1
    rc=$?
}

expect_set() {
    run "$1"
    [[ $rc -eq 0 ]] || fail "'$1': exit $rc"
    [[ "$(cat "$WORK/calls" 2>/dev/null)" == "set-timezone -- $1" ]] || fail "'$1': timedatectl not called with it"
    [[ "$(cat "$WORK/out")" == "thinfloe: time zone $1 from DHCP" ]] || fail "'$1': wrong output: $(cat "$WORK/out")"
}

expect_ignored() {
    local label=$1
    shift
    run "$@"
    [[ $rc -eq 0 ]] || fail "$label: exit $rc"
    [[ ! -e "$WORK/calls" ]] || fail "$label: timedatectl was called: $(cat "$WORK/calls")"
    [[ $(wc -l <"$WORK/out") -eq 1 ]] || fail "$label: not exactly one line: $(cat "$WORK/out")"
    grep -q "^thinfloe: ignored DHCP time zone '" "$WORK/out" || fail "$label: no ignored line: $(cat "$WORK/out")"
    # Printable ASCII only, and short.
    if LC_ALL=C grep -q '[^ -~]' "$WORK/out"; then fail "$label: control character in output"; fi
    [[ $(wc -c <"$WORK/out") -lt 200 ]] || fail "$label: output too long ($(wc -c <"$WORK/out"))"
}

expect_set Europe/Zurich
expect_set Europe/Inside
expect_ignored "path escape" '../../etc/passwd'
expect_ignored "unknown zone" 'Mars/Olympus'
expect_ignored "300 bytes" "$(printf 'A%.0s' $(seq 300))"
expect_ignored "directory" 'Europe'
expect_ignored "absolute" '/etc/passwd'
expect_ignored "leading dash" '-h'
expect_ignored "symlink out of the tree" 'Europe/Escape'
expect_ignored "relative symlink out of the tree" 'Europe/Outside'
expect_ignored "not TZif" 'Europe/Junk'
expect_ignored "escape sequence" "$(printf 'Europe\033[31m/Zurich')"
expect_ignored "shell syntax" 'Europe/Zurich;id'
run ''
[[ $rc -eq 0 && ! -s "$WORK/out" && ! -e "$WORK/calls" ]] || fail "an empty TIMEZONE= must be silent"

run
[[ $rc -eq 0 && ! -s "$WORK/out" && ! -e "$WORK/calls" ]] || fail "no TIMEZONE= line must be silent"

rm -f "$WORK"/leases/*
PATH="$WORK/bin:$PATH" THINFLOE_LEASE_DIR="$WORK/leases" THINFLOE_ZONEINFO_DIR="$WORK/zi" "$SCRIPT" >"$WORK/out" 2>&1
[[ $? -eq 0 && ! -s "$WORK/out" ]] || fail "no lease at all must be silent"

if [[ $fails -ne 0 ]]; then
    echo "$fails check(s) failed" >&2
    exit 1
fi
echo "dhcp-timezone: all checks passed"
