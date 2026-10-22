#!/bin/bash
# Offline tests for scripts/check-boot-smoke.sh's boot-completion detection.
#
# make qemu-smoke once failed on a good boot: agetty on the serial console
# wrote its terminal reset into the middle of systemd's "Reached target
# multi-user.target" line, and the plain grep no longer matched. These
# fixtures pin that escape sequences inside the line are tolerated, and that
# a boot which never reached the target is still rejected.
#
# Usage: test/integration/test-boot-smoke-check.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
CHECK="$REPO/scripts/check-boot-smoke.sh"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

# expect <name> <want-exit> <printf-format>
expect() {
    printf "$3" >"$WORK/$1.log"
    "$CHECK" --reached "$WORK/$1.log"
    local rc=$?
    if [[ $rc -eq $2 ]]; then
        ok "--reached on '$1' exits $2"
    else
        bad "--reached on '$1' exits $rc, want $2"
    fi
}

# What systemd writes, colour codes included.
expect coloured 0 \
    '[\e[0;32m  OK  \e[0m] Reached target \e[0;1;39mmulti-user.target\e[0m - Multi-User System.\r\n'
# agetty's reset sequence (ESC[!p ESC]104 ST ESC[?7h ESC[1G ESC[0J) landing
# inside the word.
expect agetty-inside 0 \
    'Reached target \e[0;1;39mmulti-u\e[!p\e]104\e\\\e[?7h\e[1G\e[0Jser.target\e[0m\r\n'
expect startup-finished 0 'Startup finished in 1.2s (kernel) + 10s (userspace) = 11s.\n'
# A boot that stopped short of the target.
expect stopped-short 1 \
    'Reached target \e[0;1;39mbasic.target\e[0m\ndracut: FATAL: Download failed\n'
: >"$WORK/empty.log"
if "$CHECK" --reached "$WORK/empty.log"; then
    bad "--reached accepts an empty log"
else
    ok "--reached rejects an empty log"
fi

# The full check uses the same detection.
HOOKS_IN_ORDER='[  OK  ] Finished \e[0;1;39mdracut-cmdline.service\e[0m - dracut cmdline hook.\n         Starting \e[0;1;39mdracut-pre-mount.service\e[0m - dracut pre-mount hook...\n         Starting \e[0;1;39mdracut-mount.service\e[0m - dracut mount hook...\n'
printf "$HOOKS_IN_ORDER"'Reached target \e[0;1;39mmulti-u\e[1G\e[0Jser.target\n' >"$WORK/full.log"
if "$CHECK" "$WORK/full.log" >"$WORK/out" 2>&1; then
    ok "full check passes a log whose target line holds escape sequences"
else
    bad "full check rejected it: $(grep FAIL "$WORK/out")"
fi
# The ThinFloe hooks raced the cmdline hook: pre-mount started first.
printf '         Starting dracut-pre-mount.service - dracut pre-mount hook...\n[  OK  ] Finished dracut-cmdline.service - dracut cmdline hook.\n         Starting dracut-mount.service - dracut mount hook...\nReached target multi-user.target\n' >"$WORK/race.log"
if "$CHECK" "$WORK/race.log" >"$WORK/out" 2>&1; then
    bad "full check passed a boot whose pre-mount hook started before dracut-cmdline finished"
elif grep -q 'FAIL - dracut-pre-mount started' "$WORK/out"; then
    ok "full check fails a boot whose pre-mount hook raced dracut-cmdline"
else
    bad "race log failed for another reason: $(grep FAIL "$WORK/out")"
fi
printf "$HOOKS_IN_ORDER"'Reached target basic.target\n' >"$WORK/short.log"
if "$CHECK" "$WORK/short.log" >"$WORK/out" 2>&1; then
    bad "full check passed a boot that never reached multi-user"
else
    ok "full check fails a boot that never reached multi-user"
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - boot smoke check"
    exit 1
fi
echo ""
echo "PASSED - boot smoke check"
exit 0
