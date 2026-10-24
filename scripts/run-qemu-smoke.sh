#!/bin/bash
# Run a headless QEMU boot with its serial console in a log file, and stop it
# once the boot has reached the multi-user target.
#
# Usage: scripts/run-qemu-smoke.sh LOG TIMEOUT GRACE -- qemu-system-x86_64 ARGS...
#
# The VM never powers off by itself, so `timeout 300 qemu ...` ran the full
# 300 s on every good boot. This polls the log with
# `check-boot-smoke.sh --reached` - the same test the verdict uses - and, once
# it passes, lets the VM run GRACE more seconds so anything logged right after
# the target (a failing unit, a late oops) still lands in the log, then stops
# QEMU. A boot that never gets there still runs until TIMEOUT and is then
# failed by check-boot-smoke.sh, exactly as before.
#
# SMOKE_WAIT_FOR, if set, is a grep pattern the log must also contain before
# the VM is stopped, for a smoke test whose VM reports on the console after
# the target (the add-on smokes); the TIMEOUT still ends a boot that never
# prints it.
#
# The exit status is always 0 once QEMU has been started: the verdict is
# check-boot-smoke.sh's, run on the log afterwards. QEMU failing to start at
# all is reported with its status, since there is no log to judge then.

set -uo pipefail

LOG="${1:?usage: run-qemu-smoke.sh LOG TIMEOUT GRACE -- qemu ARGS...}"
TIMEOUT="${2:?missing TIMEOUT}"
GRACE="${3:?missing GRACE}"
shift 3
[[ "${1:-}" == "--" ]] && shift
[[ $# -gt 0 ]] || { echo "run-qemu-smoke.sh: no QEMU command given" >&2; exit 2; }

CHECK="$(cd "$(dirname "$0")" && pwd)/check-boot-smoke.sh"

rm -f "$LOG"
"$@" -serial "file:$LOG" &
qemu=$!
trap 'kill "$qemu" 2>/dev/null' EXIT INT TERM

start=$SECONDS
reached_at=""
while kill -0 "$qemu" 2>/dev/null; do
    elapsed=$((SECONDS - start))
    if [[ -z "$reached_at" ]] && "$CHECK" --reached "$LOG"; then
        reached_at=$elapsed
        echo "run-qemu-smoke: multi-user target reached after ${elapsed}s," \
             "stopping in ${GRACE}s"
    fi
    if [[ -n "$reached_at" ]] && (( elapsed - reached_at >= GRACE )) \
       && { [[ -z "${SMOKE_WAIT_FOR:-}" ]] || grep -aqE -- "$SMOKE_WAIT_FOR" "$LOG"; }; then
        break
    fi
    if (( elapsed >= TIMEOUT )); then
        echo "run-qemu-smoke: no multi-user target after ${TIMEOUT}s, stopping QEMU"
        break
    fi
    sleep 1
done

if kill -0 "$qemu" 2>/dev/null; then
    kill "$qemu" 2>/dev/null
    wait "$qemu" 2>/dev/null
    exit 0
fi

# QEMU exited on its own: -no-reboot turns a reboot or a panic into an exit,
# which the log shows. Only a QEMU that produced no log at all is an error here.
wait "$qemu"
rc=$?
if [[ ! -s "$LOG" ]]; then
    echo "run-qemu-smoke: QEMU exited with status $rc and wrote no log" >&2
    exit "$rc"
fi
exit 0
