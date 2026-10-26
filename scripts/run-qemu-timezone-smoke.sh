#!/bin/bash
# Headless USB-style boot whose DHCP server sends a time zone (option 101).
# Requires the guest to log `thinfloe: time zone Europe/Zurich from DHCP`
# (dhcp-timezone, started by thinfloe-dhcp-timezone.path when networkd writes
# the lease) and the boot log to hold no ordering cycle.
#
# The DHCP server is the host's dnsmasq, run unprivileged. The script
# re-executes itself under `podman unshare unshare --net` (a user and network
# namespace), makes a tap device there,
# gives it 10.77.0.1/24 and starts dnsmasq on it with
# --dhcp-option=101,Europe/Zurich and no system configuration. QEMU attaches
# the guest to that tap, so option 101 travels over a real DHCP exchange.
#
# Usage: scripts/run-qemu-timezone-smoke.sh LOG TIMEOUT GRACE QEMU_ARGS...
# QEMU_ARGS are the full qemu-system-x86_64 arguments except the network.

set -uo pipefail

LOG="${1:?usage: run-qemu-timezone-smoke.sh LOG TIMEOUT GRACE QEMU_ARGS...}"
TIMEOUT="${2:?missing TIMEOUT}"
GRACE="${3:?missing GRACE}"
shift 3
# dnsmasq changes directory, so every path it gets must be absolute.
LOG="$(realpath -m -- "$LOG")"

HERE="$(cd "$(dirname "$0")" && pwd)"
ZONE=Europe/Zurich
WANT="thinfloe: time zone $ZONE from DHCP"
DNSMASQ="${DNSMASQ:-$(command -v dnsmasq || echo /sbin/dnsmasq)}"

if [[ -z "${TF_TZ_NS:-}" ]]; then
    # `podman unshare` enters the user namespace podman uses for rootless
    # containers (root plus the subuid/subgid range). A bare `unshare -rn`
    # maps one id and forbids setgroups(2), which dnsmasq calls at start-up
    # and then exits. --kill-child: if unshare is killed, the script inside
    # goes with it.
    TF_TZ_NS=1 exec podman unshare unshare --net \
        --kill-child -- "$0" "$LOG" "$TIMEOUT" "$GRACE" "$@"
    echo "FAIL: cannot create the user and network namespace (podman unshare)" >&2
    exit 1
fi

[[ -x "$DNSMASQ" ]] || { echo "FAIL: no dnsmasq ($DNSMASQ)" >&2; exit 1; }

# Inside the namespace from here on.
ip link set lo up || { echo "FAIL: cannot bring up loopback in the namespace" >&2; exit 1; }
ip tuntap add dev tap0 mode tap ||
    { echo "FAIL: cannot create a tap device in the user namespace" >&2; exit 1; }
ip addr add 10.77.0.1/24 dev tap0 && ip link set tap0 up ||
    { echo "FAIL: cannot configure tap0" >&2; exit 1; }

DNSMASQ_LOG="$LOG.dnsmasq"
dns=""
trap 'kill $dns 2>/dev/null; wait $dns 2>/dev/null' EXIT
trap 'exit 1' INT TERM HUP
rm -f "$DNSMASQ_LOG" "$LOG.leases" "$LOG.pid"
# No system configuration, no DNS (port 0), own lease and pid files; the
# pdeathsig also covers a SIGKILL of this script.
setpriv --pdeathsig TERM -- "$DNSMASQ" --conf-file=/dev/null --keep-in-foreground \
    --user=root --group=root --port=0 --interface=tap0 --except-interface=lo \
    --bind-interfaces --listen-address=10.77.0.1 \
    --dhcp-range=10.77.0.50,10.77.0.60,12h --dhcp-authoritative \
    --dhcp-option=101,"$ZONE" \
    --dhcp-leasefile="$LOG.leases" --pid-file="$LOG.pid" \
    --log-dhcp --log-facility="$DNSMASQ_LOG" &
dns=$!
sleep 1
kill -0 "$dns" 2>/dev/null || { echo "FAIL: dnsmasq did not start (log: $DNSMASQ_LOG)" >&2; cat "$DNSMASQ_LOG" >&2; exit 1; }

SMOKE_WAIT_FOR="$WANT" setpriv --pdeathsig TERM -- "$HERE/run-qemu-smoke.sh" "$LOG" "$TIMEOUT" "$GRACE" -- \
    qemu-system-x86_64 "$@" \
    -netdev tap,id=n0,ifname=tap0,script=no,downscript=no \
    -device virtio-net-pci,netdev=n0,mac=52:54:00:12:34:57 || exit $?

kill "$dns" 2>/dev/null
wait "$dns" 2>/dev/null
dns=""

"$HERE/check-boot-smoke.sh" "$LOG"
fail=$?

echo ""
echo "time zone:"
if grep -qF "DHCPACK(tap0)" "$DNSMASQ_LOG"; then
    echo "ok   - the guest got a DHCP lease from dnsmasq"
else
    echo "FAIL - dnsmasq never acknowledged the guest (log: $DNSMASQ_LOG)"
    fail=1
fi
if grep -qaF "$WANT" "$LOG"; then
    echo "ok   - $WANT"
else
    echo "FAIL - no '$WANT' line in the boot log"
    fail=1
fi
if grep -qaF "Found ordering cycle" "$LOG"; then
    echo "FAIL - ordering cycle in the boot log"
    fail=1
else
    echo "ok   - no ordering cycle"
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - time zone smoke test (log: $LOG)"
    exit 1
fi
echo ""
echo "PASSED - time zone smoke test"
exit 0
