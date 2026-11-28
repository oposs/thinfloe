#!/bin/sh
# Runs inside a smoke-test VM as kernel-command-line.service
# (systemd.run=, see scripts/run-qemu-addons-smoke.sh and
# scripts/run-qemu-upgrade-smoke.sh) and writes what the add-on machinery
# did to the serial console, where the host reads it. Lives on the stick's
# ESP, not in the image: a smoke test must not need a change to the image.
#
# Usage: smoke-addons-report.sh [check] [reboot]
#   check   run `thinfloe-addon check` first, as the check timers do
#   reboot  then reboot the VM once multi-user.target is reached, from a
#           transient unit (this script is part of the boot transaction, so
#           waiting for the target here would wait for itself); at most
#           300 s, else a SMOKE-ADDON-REBOOT line. QEMU's -no-reboot turns
#           the reboot into an exit, with the shutdown on the console.
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

check=no reboot=no
for a in "$@"; do
    case $a in
        check) check=yes ;;
        reboot) reboot=yes ;;
    esac
done

if [ "$check" = yes ]; then
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

if [ "$reboot" = yes ]; then
    systemd-run --no-block --unit=smoke-addon-reboot /bin/sh -c \
        'n=0; until systemctl is-active -q multi-user.target; do
             n=$((n + 1))
             if [ "$n" -gt 300 ]; then
                 echo "SMOKE-ADDON-REBOOT: no multi-user.target after 300 s, not rebooting" >/dev/console
                 exit 1
             fi
             sleep 1
         done
         systemctl reboot' \
        >/dev/console 2>&1
fi
