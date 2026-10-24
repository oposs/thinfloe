#!/bin/sh
# Runs inside a smoke-test VM as kernel-command-line.service
# (systemd.run=, see scripts/run-qemu-addons-smoke.sh and
# scripts/run-qemu-upgrade-smoke.sh) and writes what the add-on machinery
# did to the serial console, where the host reads it. Lives on the stick's
# ESP, not in the image: a smoke test must not need a change to the image.
#
# Usage: smoke-addons-report.sh [check]
#   check   run `thinfloe-addon check` first, as the check timers do
#
# The console turns tabs into nothing useful, so columns are joined with "|".

CONSOLE=/dev/console
STATUS=/run/thinfloe/addons/status

# thinfloe-addons.service writes the status file as its last step; wait for
# the file and for the unit to be done.
i=0
while [ "$i" -lt 150 ]; do
    state=$(systemctl show -p ActiveState --value thinfloe-addons.service)
    [ -s "$STATUS" ] && [ "$state" != activating ] && break
    i=$((i + 1))
    sleep 1
done

if [ "${1:-}" = check ]; then
    /usr/bin/thinfloe-addon check >/dev/null 2>&1
fi

{
    echo "SMOKE-ADDON-STATUS-BEGIN"
    tr '\t' '|' <"$STATUS"
    echo "SMOKE-ADDON-STATUS-END"
    if [ -f /usr/share/thinfloe-smoke-addon/ok ]; then
        echo "SMOKE-ADDON-FILE: present"
    else
        echo "SMOKE-ADDON-FILE: missing"
    fi
    echo "SMOKE-ADDON-LOG-BEGIN"
    tr '\t' ' ' </run/thinfloe/addons/log 2>/dev/null
    echo "SMOKE-ADDON-LOG-END"
    echo "SMOKE-ADDON-DONE"
} >"$CONSOLE" 2>&1
