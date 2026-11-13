#!/bin/sh
# Runs inside a smoke-test VM as kernel-command-line.service
# (systemd.run=, see scripts/run-qemu-upgrade-smoke.sh) and writes what the
# home layout migration did to the serial console. Lives on the stick's ESP,
# not in the image. The smoke boots stop at multi-user.target, so no session
# runs the migration; "migrate" does what the session would.
#
# Usage: smoke-home-report.sh migrate|check
#   migrate  boot 1: add-on report, migrate the home, save it
#   check    boot 2: report the layout, the keyboard and the attic

CONSOLE=/dev/console
STATE=/run/thinfloe/state
HOME_DIR=/home/tfuser

layout() { tr -d '\n' <"$STATE/home-layout" 2>/dev/null; }

case "${1:-}" in
migrate)
    /bin/sh /run/thinfloe/storage/efi/smoke-report.sh check
    {
        echo "SMOKE-HOME layout $(layout)"
        runuser -u tfuser -- env HOME="$HOME_DIR" TF_STATE_DIR="$STATE" thinfloe-menu --migrate-home
        echo "SMOKE-HOME migrate rc=$?"
        echo "SMOKE-HOME layout-after $(layout)"
        thinfloe-save-home
        echo "SMOKE-HOME save rc=$?"
        echo "SMOKE-HOME-DONE"
    } >"$CONSOLE" 2>&1
    ;;
check)
    {
        echo "SMOKE-HOME layout $(layout)"
        echo "SMOKE-HOME env $(grep '^XKB_DEFAULT_LAYOUT=' "$HOME_DIR/.config/thinfloe/generated/environment" 2>/dev/null)"
        if [ -e "$HOME_DIR/cfg-attic/.thinfloe-environment" ]; then
            echo "SMOKE-HOME attic yes"
        else
            echo "SMOKE-HOME attic no"
        fi
        echo "SMOKE-HOME-DONE"
    } >"$CONSOLE" 2>&1
    ;;
*)
    echo "usage: smoke-home-report.sh migrate|check" >&2
    exit 2
    ;;
esac
