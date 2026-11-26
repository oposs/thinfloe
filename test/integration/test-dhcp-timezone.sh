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
# timedatectl is a stub on PATH that records its arguments (then waits for
# the file STUB_GATE, when set, and sleeps for STUB_SLEEP seconds). The
# --watch cases need inotifywait on the host.
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
WATCHER=
# Stop a --watch run left by a failed case before the directory goes.
cleanup() {
    [[ -n "$WATCHER" ]] && kill "$WATCHER" 2>/dev/null && wait "$WATCHER" 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT
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
# STUB_GATE: block after recording the call until the file exists (at most
# 10 s), so a test knows the watcher is inside a scan.
if [[ -n "\${STUB_GATE:-}" ]]; then
    for ((i = 0; i < 200; i++)); do
        [[ -e "\$STUB_GATE" ]] && break
        sleep 0.05
    done
fi
sleep "\${STUB_SLEEP:-0}"
STUB
chmod +x "$WORK/bin/timedatectl"
cat >"$WORK/bin/chronyc" <<STUB
#!/bin/bash
printf '%s\n' "\$*" >>"$WORK/chronyc"
exit "\${STUB_CHRONYC_RC:-0}"
STUB
chmod +x "$WORK/bin/chronyc"

# The root side of the menu's setting, the state file and chrony's DHCP
# directory: all in the scratch tree, never the host's /etc or /run.
export THINFLOE_TIME_FILE="$WORK/time" THINFLOE_STATE_DIR="$WORK/state" \
    THINFLOE_CHRONY_DHCP_DIR="$WORK/chrony-dhcp"

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

# Two leases with different zones: the last lease in glob order wins, and
# only that zone is set and logged.
rm -f "$WORK"/leases/* "$WORK/calls"
printf 'TIMEZONE=Europe/Zurich\n' >"$WORK/leases/2"
printf 'TIMEZONE=Europe/Inside\n' >"$WORK/leases/3"
PATH="$WORK/bin:$PATH" THINFLOE_LEASE_DIR="$WORK/leases" THINFLOE_ZONEINFO_DIR="$WORK/zi" "$SCRIPT" >"$WORK/out" 2>&1
[[ "$(cat "$WORK/calls" 2>/dev/null)" == "set-timezone -- Europe/Inside" ]] \
    || fail "two leases: not only the last zone set: $(cat "$WORK/calls" 2>/dev/null)"
[[ "$(cat "$WORK/out")" == "thinfloe: time zone Europe/Inside from DHCP" ]] \
    || fail "two leases: wrong output: $(cat "$WORK/out")"
# A valid zone in an earlier lease wins over an invalid one in a later lease.
printf 'TIMEZONE=Mars/Olympus\n' >"$WORK/leases/4"
rm -f "$WORK/calls"
PATH="$WORK/bin:$PATH" THINFLOE_LEASE_DIR="$WORK/leases" THINFLOE_ZONEINFO_DIR="$WORK/zi" "$SCRIPT" >"$WORK/out" 2>&1
[[ "$(cat "$WORK/calls" 2>/dev/null)" == "set-timezone -- Europe/Inside" ]] \
    || fail "invalid last lease: wrong calls: $(cat "$WORK/calls" 2>/dev/null)"
grep -q "^thinfloe: ignored DHCP time zone 'Mars/Olympus'" "$WORK/out" \
    || fail "invalid last lease: no ignored line: $(cat "$WORK/out")"
rm -f "$WORK"/leases/*

# --watch: the service's mode. It must arm its watch before it reads the
# leases and keep it armed while it reads, so no lease write is lost.
# ---- the menu's zone, the state file and the DHCP NTP servers ----

# once LEASE_TEXT: one lease with that text, one run.
once() {
    rm -f "$WORK"/leases/* "$WORK/calls" "$WORK/out"
    printf '%b' "$1" >"$WORK/leases/2"
    PATH="$WORK/bin:$PATH" THINFLOE_LEASE_DIR="$WORK/leases" \
        THINFLOE_ZONEINFO_DIR="$WORK/zi" "$SCRIPT" >"$WORK/out" 2>&1
    rc=$?
}
SRCS="$WORK/chrony-dhcp/thinfloe.sources"

rm -rf "$WORK/state" "$WORK/chrony-dhcp" "$WORK/time" "$WORK/chronyc"
once 'TIMEZONE=Europe/Zurich\n'
[[ "$(cat "$WORK/state/dhcp-timezone" 2>/dev/null)" == "Europe/Zurich" ]] \
    || fail "state_file_names_the_dhcp_zone: $(cat "$WORK/state/dhcp-timezone" 2>/dev/null)"
[[ ! -e "$SRCS" && ! -e "$WORK/chronyc" ]] || fail "a lease without NTP= wrote chrony sources or reloaded"

once 'NTP=10.0.0.5 fd00::7\n'
[[ "$(cat "$SRCS" 2>/dev/null)" == $'server 10.0.0.5 iburst\nserver fd00::7 iburst' ]] \
    || fail "dhcp_ntp_written: $(cat "$SRCS" 2>/dev/null)"
[[ "$(cat "$WORK/chronyc" 2>/dev/null)" == "reload sources" ]] || fail "dhcp_ntp_written: no single reload"
grep -qxF "thinfloe: NTP servers 10.0.0.5 fd00::7 from DHCP" "$WORK/out" || fail "dhcp_ntp_written: no log line: $(cat "$WORK/out")"

once 'NTP=10.0.0.5 fd00::7\n'
[[ $(grep -c . "$WORK/chronyc") -eq 1 ]] || fail "unchanged_ntp_no_reload: reloaded again"
[[ ! -s "$WORK/out" ]] || fail "unchanged_ntp_no_reload: logged again: $(cat "$WORK/out")"

# A failed reload is retried at the next read, although the file is unchanged.
once 'NTP=10.0.0.6\n'
rm -f "$WORK/chronyc"
STUB_CHRONYC_RC=1 once 'NTP=10.0.0.9\n'
grep -qxF "thinfloe: chronyc reload sources failed" "$WORK/out" || fail "failed_reload_retried: no failure line: $(cat "$WORK/out")"
[[ -e "$WORK/state/dhcp-ntp-reload-pending" ]] || fail "failed_reload_retried: no pending marker"
rm -f "$WORK/chronyc"
once 'NTP=10.0.0.9\n'
[[ "$(cat "$WORK/chronyc" 2>/dev/null)" == "reload sources" ]] || fail "failed_reload_retried: no reload on the next read"
[[ ! -e "$WORK/state/dhcp-ntp-reload-pending" ]] || fail "failed_reload_retried: marker kept after a good reload"
rm -f "$WORK/chronyc"
once 'NTP=10.0.0.9\n'
[[ ! -e "$WORK/chronyc" ]] || fail "failed_reload_retried: reloaded again with nothing pending"

once 'NTP=evil;rm 1.2.3.4 a.b 999.1.1.1 1.2.3 ::g\n'
[[ "$(cat "$SRCS" 2>/dev/null)" == "server 1.2.3.4 iburst" ]] || fail "hostile_ntp_dropped: $(cat "$SRCS" 2>/dev/null)"
grep -q "^thinfloe: ignored DHCP NTP server 'evil;rm': not an IP address" "$WORK/out" \
    || fail "hostile_ntp_dropped: no ignored line: $(cat "$WORK/out")"

once 'NTP=: 1:2 :: :.: fd00::7 ::ffff:1.2.3.4\n'
[[ "$(cat "$SRCS" 2>/dev/null)" == $'server fd00::7 iburst\nserver ::ffff:1.2.3.4 iburst' ]] \
    || fail "implausible_ipv6_dropped: $(cat "$SRCS" 2>/dev/null)"
for a in : 1:2 :: :.:; do
    grep -qxF "thinfloe: ignored DHCP NTP server '$a': not an IP address" "$WORK/out" \
        || fail "implausible_ipv6_dropped: no ignored line for '$a': $(cat "$WORK/out")"
done

once "NTP=$(echo 10.0.0.{1..10})\\n"
[[ $(grep -c '^server ' "$SRCS" 2>/dev/null) -eq 8 ]] || fail "more than 8 DHCP NTP servers kept: $(cat "$SRCS" 2>/dev/null)"

once 'ADDRESS=10.0.0.5\n'
[[ ! -e "$SRCS" ]] || fail "a lease losing its NTP= kept the sources file"

printf 'ntp=ntp.example.ch\n' >"$WORK/time"
once 'NTP=10.0.0.5\n'
[[ ! -e "$SRCS" ]] || fail "menu_servers_remove_dhcp_file: DHCP sources written"

printf 'zone=Asia/Tokyo\n' >"$WORK/time"
once 'TIMEZONE=Europe/Zurich\n'
[[ ! -e "$WORK/calls" ]] || fail "menu zone: DHCP zone set: $(cat "$WORK/calls")"
grep -qxF "thinfloe: DHCP time zone Europe/Zurich not used: the zone is set in the menu" "$WORK/out" \
    || fail "menu zone: no 'not used' line: $(cat "$WORK/out")"
rm -f "$WORK/time"

# It needs inotifywait (inotify-tools) on the host, as in the image.
if ! command -v inotifywait >/dev/null; then
    fail "inotifywait not found: install inotify-tools to test --watch"
    echo "$fails check(s) failed" >&2
    exit 1
fi

# wait_for FILE TEXT: poll every 0.1 s, at most 10 s, for TEXT in FILE.
wait_for() {
    local i
    for ((i = 0; i < 100; i++)); do
        grep -qF -- "$2" "$1" 2>/dev/null && return 0
        sleep 0.1
    done
    return 1
}

# wait_inotify_on DIR: poll every 0.1 s, at most 10 s, for an inotifywait
# whose last argument is DIR (the watcher runs inotifywait ... -- DIR).
wait_inotify_on() {
    local i
    for ((i = 0; i < 100; i++)); do
        pgrep -f -- "-- $1\$" >/dev/null && return 0
        sleep 0.1
    done
    return 1
}

# watch_start LEASE_DIR: start the watcher on LEASE_DIR, output in watch.log.
watch_start() {
    rm -f "$WORK/calls" "$WORK/watch.log"
    PATH="$WORK/bin:$PATH" THINFLOE_LEASE_DIR="$1" THINFLOE_ZONEINFO_DIR="$WORK/zi" \
        "$SCRIPT" --watch >"$WORK/watch.log" 2>&1 &
    WATCHER=$!
}

watch_stop() {
    [[ -n "$WATCHER" ]] || return 0
    kill "$WATCHER" 2>/dev/null
    wait "$WATCHER" 2>/dev/null
    WATCHER=
}

# lines FILE: the number of non-empty lines, 0 for a missing file.
lines() {
    local n=0
    [[ -f "$1" ]] && n=$(grep -c . "$1")
    echo "${n:-0}"
}

lease() {
    printf 'ADDRESS=10.0.0.5\nTIMEZONE=%s\nDNS=10.0.0.1\n' "$2" >"$1"
}

# A lease that is there before the watcher starts (the boot-time case).
mkdir -p "$WORK/w1"
lease "$WORK/w1/2" Europe/Zurich
watch_start "$WORK/w1"
wait_for "$WORK/calls" "set-timezone -- Europe/Zurich" || fail "watch: lease present at start not applied"
watch_stop

# A lease written while a scan runs: timedatectl blocks on the gate, so the
# second lease comes in while the watcher is inside the scan, by construction,
# and must still be applied.
# A lease with an NTP server only is there at the start: its log line says
# the first scan is over, so the next lease is read by a scan of the loop.
mkdir -p "$WORK/w2"
rm -f "$WORK/gate"
printf 'ADDRESS=10.0.0.4\nNTP=10.0.0.7\n' >"$WORK/w2/1"
STUB_GATE="$WORK/gate" watch_start "$WORK/w2"
wait_for "$WORK/watch.log" "NTP servers 10.0.0.7 from DHCP" || fail "watch: first scan did not run"
lease "$WORK/w2/2" Europe/Zurich
wait_for "$WORK/calls" "set-timezone -- Europe/Zurich" || fail "watch: first lease not applied"
lease "$WORK/w2/.tmp" Europe/Inside && mv "$WORK/w2/.tmp" "$WORK/w2/2"
: >"$WORK/gate"
for ((i = 0; i < 100; i++)); do
    [[ "$(tail -n 1 "$WORK/calls" 2>/dev/null)" == "set-timezone -- Europe/Inside" ]] && break
    sleep 0.1
done
[[ "$(tail -n 1 "$WORK/calls" 2>/dev/null)" == "set-timezone -- Europe/Inside" ]] \
    || fail "watch: lease written during a scan lost: $(cat "$WORK/calls" 2>/dev/null)"
watch_stop
# The NTP server of this case is not left for the next ones.
rm -rf "$WORK/chrony-dhcp"

# The lease directory does not exist yet when the watcher starts.
mkdir -p "$WORK/late"
watch_start "$WORK/late/leases"
sleep 0.5
mkdir "$WORK/late/leases"
lease "$WORK/late/leases/3" Europe/Zurich
wait_for "$WORK/calls" "set-timezone -- Europe/Zurich" || fail "watch: lease in a late directory not applied"
[[ -d "$WORK/late/leases" ]] || fail "watch: lease directory gone"
watch_stop

# The watcher never creates the lease directory itself: once it waits in the
# parent, the directory must still be missing.
mkdir -p "$WORK/nocreate"
watch_start "$WORK/nocreate/leases"
wait_inotify_on "$WORK/nocreate" || fail "watch: no watch on the parent of a missing lease directory"
[[ ! -e "$WORK/nocreate/leases" ]] || fail "watch: the watcher created the lease directory"
watch_stop

# A renewal with the same zone: timedatectl again, but one log line only.
mkdir -p "$WORK/w3"
watch_start "$WORK/w3"
sleep 0.5
lease "$WORK/w3/2" Europe/Zurich
wait_for "$WORK/calls" "Europe/Zurich" || fail "watch: renewal case, first lease not applied"
sleep 1
lease "$WORK/w3/2" Europe/Zurich
for ((i = 0; i < 100; i++)); do
    [[ $(lines "$WORK/calls") -ge 2 ]] && break
    sleep 0.1
done
[[ $(lines "$WORK/calls") -ge 2 ]] || fail "watch: renewal did not rescan"
[[ $(grep -c 'time zone Europe/Zurich from DHCP' "$WORK/watch.log") -eq 1 ]] \
    || fail "watch: same zone logged more than once: $(cat "$WORK/watch.log")"

# A removed lease (link down): no zone set, no zone logged. The lease first
# gets an NTP server, so the scan after the removal logs one line (the NTP
# servers are gone). That line comes before the zone part of the same scan,
# so a sentinel lease with another NTP server follows: its line comes from a
# later scan, which starts only once the removal's scan has ended.
before_calls=$(lines "$WORK/calls")
printf 'ADDRESS=10.0.0.5\nTIMEZONE=Europe/Zurich\nNTP=10.0.0.9\n' >"$WORK/w3/.tmp" \
    && mv "$WORK/w3/.tmp" "$WORK/w3/2"
wait_for "$WORK/watch.log" "NTP servers 10.0.0.9 from DHCP" || fail "watch: lease with an NTP server not read"
for ((i = 0; i < 100; i++)); do
    [[ $(lines "$WORK/calls") -gt $before_calls ]] && break
    sleep 0.1
done
before_calls=$(lines "$WORK/calls")
before_log=$(lines "$WORK/watch.log")
rm -f "$WORK/w3/2"
wait_for "$WORK/watch.log" "no NTP servers from DHCP" || fail "watch: removed lease not scanned"
printf 'ADDRESS=10.0.0.7\nNTP=10.0.0.10\n' >"$WORK/w3/.tmp" && mv "$WORK/w3/.tmp" "$WORK/w3/4"
wait_for "$WORK/watch.log" "NTP servers 10.0.0.10 from DHCP" || fail "watch: removed lease, sentinel lease not read"
[[ $(lines "$WORK/calls") -eq $before_calls ]] || fail "watch: removed lease called timedatectl: $(cat "$WORK/calls")"
[[ $(lines "$WORK/watch.log") -eq $((before_log + 2)) ]] || fail "watch: removed lease logged: $(cat "$WORK/watch.log")"
[[ "$(tail -n 2 "$WORK/watch.log" | head -n 1)" == "thinfloe: no NTP servers from DHCP" ]] \
    || fail "watch: removed lease, log: $(tail -n 2 "$WORK/watch.log")"
rm -f "$WORK/w3/4"

# inotifywait ends: the watcher fails, so systemd restarts it.
pkill -P "$WATCHER" inotifywait || fail "watch: no inotifywait child"
for ((i = 0; i < 100; i++)); do
    kill -0 "$WATCHER" 2>/dev/null || break
    sleep 0.1
done
if kill -0 "$WATCHER" 2>/dev/null; then
    fail "watch: still running after inotifywait ended"
    watch_stop
else
    wait "$WATCHER"
    rc=$?
    WATCHER=
    [[ $rc -ne 0 ]] || fail "watch: exit 0 after inotifywait ended"
fi

# watch_exit_rc: wait up to 10 s for the watcher to end; rc holds its status,
# or 999 if it is still running (then it is stopped).
watch_exit_rc() {
    local i
    for ((i = 0; i < 100; i++)); do
        kill -0 "$WATCHER" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$WATCHER" 2>/dev/null; then
        rc=999
        watch_stop
    else
        wait "$WATCHER"
        rc=$?
        WATCHER=
    fi
}

# systemctl stop (SIGTERM) is a clean exit, or the unit ends up failed at
# every shutdown.
mkdir -p "$WORK/w5"
watch_start "$WORK/w5"
sleep 0.5
kill -TERM "$WATCHER"
watch_exit_rc
[[ $rc -eq 0 ]] || fail "watch: SIGTERM gave exit $rc, not 0"

# The lease directory removed: the watch is gone, so the watcher fails and
# systemd starts it again on the new directory.
mkdir -p "$WORK/w6"
watch_start "$WORK/w6"
sleep 0.5
rmdir "$WORK/w6"
watch_exit_rc
[[ $rc -ne 0 && $rc -ne 999 ]] || fail "watch: removed lease directory, watcher rc $rc (999: still running)"

# Two leases with different zones under --watch: a renewal of either logs
# nothing new, as the zone left set does not change.
mkdir -p "$WORK/w7"
printf 'TIMEZONE=Europe/Zurich\n' >"$WORK/w7/2"
printf 'TIMEZONE=Europe/Inside\n' >"$WORK/w7/3"
watch_start "$WORK/w7"
wait_for "$WORK/calls" "Europe/Inside" || fail "watch, two leases: zone not set"
printf 'TIMEZONE=Europe/Zurich\n' >"$WORK/w7/2"
for ((i = 0; i < 100; i++)); do
    [[ $(lines "$WORK/calls") -ge 2 ]] && break
    sleep 0.1
done
[[ $(lines "$WORK/calls") -ge 2 ]] || fail "watch, two leases: renewal did not rescan"
[[ $(lines "$WORK/watch.log") -eq 1 ]] || fail "watch, two leases: logged more than once: $(cat "$WORK/watch.log")"
watch_stop

# No parent directory to wait in: one clear line naming it, a failed exit.
watch_start "$WORK/nowhere/leases"
watch_exit_rc
[[ $rc -ne 0 && $rc -ne 999 ]] || fail "watch, no parent: rc $rc (999: still running)"
grep -qF "$WORK/nowhere" "$WORK/watch.log" || fail "watch, no parent: missing directory not named: $(cat "$WORK/watch.log")"
if grep -q "unbound variable" "$WORK/watch.log"; then fail "watch, no parent: unbound variable"; fi
[[ $(lines "$WORK/watch.log") -eq 1 ]] || fail "watch, no parent: not one line: $(cat "$WORK/watch.log")"

# The parent removed while the watcher waits in it for the lease directory.
mkdir -p "$WORK/gone"
watch_start "$WORK/gone/leases"
sleep 0.5
rmdir "$WORK/gone"
watch_exit_rc
[[ $rc -ne 0 && $rc -ne 999 ]] || fail "watch, parent removed: rc $rc (999: still running)"
if grep -q "unbound variable" "$WORK/watch.log"; then fail "watch, parent removed: unbound variable"; fi

# The menu's zone wins over a renewal: never set, logged once.
printf 'zone=Asia/Tokyo\n' >"$WORK/time"
mkdir -p "$WORK/w5"
lease "$WORK/w5/2" Europe/Zurich
watch_start "$WORK/w5"
wait_for "$WORK/watch.log" "not used" || fail "menu_zone_wins_over_a_renewal: no 'not used' line"
# The renewal carries an NTP server, so its scan logs a line. That line comes
# before the zone part of the same scan, so a sentinel lease with another
# NTP server follows: its line comes from a later scan, which starts only
# once the renewal's scan has ended.
printf 'ADDRESS=10.0.0.5\nTIMEZONE=Europe/Zurich\nNTP=10.0.0.8\n' >"$WORK/w5/.tmp" \
    && mv "$WORK/w5/.tmp" "$WORK/w5/2"
wait_for "$WORK/watch.log" "NTP servers 10.0.0.8 from DHCP" || fail "menu_zone_wins_over_a_renewal: renewal not read"
printf 'ADDRESS=10.0.0.6\nNTP=10.0.0.11\n' >"$WORK/w5/.tmp" && mv "$WORK/w5/.tmp" "$WORK/w5/3"
wait_for "$WORK/watch.log" "10.0.0.11 from DHCP" || fail "menu_zone_wins_over_a_renewal: sentinel lease not read"
watch_stop
[[ ! -e "$WORK/calls" ]] || fail "menu_zone_wins_over_a_renewal: zone set: $(cat "$WORK/calls")"
zurich='DHCP time zone Europe/Zurich not used'
[[ $(grep -cF "$zurich" "$WORK/watch.log") -eq 1 ]] || fail "menu_zone_wins_over_a_renewal: logged $(grep -cF "$zurich" "$WORK/watch.log") times"
rm -f "$WORK/time" "$WORK/w5/3"

# Stopping the watcher leaves no inotifywait behind.
mkdir -p "$WORK/w4"
watch_start "$WORK/w4"
sleep 0.5
watch_stop
sleep 0.2
if pgrep -f "inotifywait.*$WORK/w4" >/dev/null; then fail "watch: inotifywait left running after stop"; fi

if [[ $fails -ne 0 ]]; then
    echo "$fails check(s) failed" >&2
    exit 1
fi
echo "dhcp-timezone: all checks passed"
