#!/bin/bash
# Headless netboot smoke test: serve the rootfs artifacts over HTTP, boot the
# kernel and initrd in QEMU with no disk and
# ramroot=http://boot.thinfloe.test:<port>/ramroot.sqsh, then judge the log.
#
# The name boot.thinfloe.test exists only in scripts/smoke-dns.py, so the
# download proves the initramfs used the DNS server from its DHCP lease:
# curl -> the systemd-resolved stub -> the lease's 10.0.2.3 -> slirp, which
# forwards to the first nameserver in /etc/resolv.conf -> smoke-dns.py. To
# make that offline and unprivileged, the script re-executes itself under
# `unshare -rmn`: a user, mount and network namespace with only a loopback
# device, where /etc/resolv.conf is bind-mounted to say 127.0.0.1 and the
# responder may bind port 53. slirp's host address 10.0.2.2 is that
# namespace's loopback, so the HTTP server and the responder run in there too.
# A resolver that ignored the lease's DNS server would fail with
# "Download failed".
#
# Usage: scripts/run-qemu-netboot-smoke.sh ROOTFS_DIR LOG TIMEOUT GRACE QEMU_ARGS...
#
# Beyond check-boot-smoke.sh, a netboot must show what only netboot does:
#   - the hostname is thinfloe-<last six hex digits of the boot MAC>;
#   - the initramfs asked the boot server for home.pkg under that MAC, which
#     proves /run/thinfloe/state/boot-mac was recorded. The server has no
#     archive, so the request is answered 404, the ordinary first-netboot case;
#   - the responder was asked for boot.thinfloe.test.

set -uo pipefail

ROOTFS_DIR="${1:?usage: run-qemu-netboot-smoke.sh ROOTFS_DIR LOG TIMEOUT GRACE QEMU_ARGS...}"
LOG="${2:?missing LOG}"
TIMEOUT="${3:?missing TIMEOUT}"
GRACE="${4:?missing GRACE}"
shift 4

HERE="$(cd "$(dirname "$0")" && pwd)"
MAC=52:54:00:12:34:56
HOSTNAME_WANT=thinfloe-123456
BOOT_NAME=boot.thinfloe.test
HTTP_LOG="$LOG.http"
DNS_LOG="$LOG.dns"

for f in vmlinuz initrd.img ramroot.sqsh; do
    [[ -f "$ROOTFS_DIR/$f" ]] || { echo "FAIL: $ROOTFS_DIR/$f missing" >&2; exit 1; }
done

if [[ -z "${TF_NETBOOT_NS:-}" ]]; then
    # --kill-child: if unshare is killed, the script inside goes with it.
    TF_NETBOOT_NS=1 exec unshare --user --map-root-user --mount --net \
        --kill-child -- "$0" "$ROOTFS_DIR" "$LOG" "$TIMEOUT" "$GRACE" "$@"
    echo "FAIL: cannot create the user and network namespace (unshare -rmn)" >&2
    exit 1
fi

# Inside the namespace from here on.
ip link set lo up || { echo "FAIL: cannot bring up loopback in the namespace" >&2; exit 1; }
resolv="$LOG.resolv.conf"
srv="" dns=""
trap 'kill $srv $dns 2>/dev/null; wait $srv $dns 2>/dev/null; rm -f "$resolv"' EXIT
trap 'exit 1' INT TERM HUP
echo "nameserver 127.0.0.1" >"$resolv"
mount --bind "$resolv" /etc/resolv.conf ||
    { echo "FAIL: cannot bind-mount $resolv over /etc/resolv.conf" >&2; exit 1; }

rm -f "$HTTP_LOG" "$DNS_LOG"
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
# The EXIT trap stops the servers on every way out that runs it; the
# pdeathsig also covers a SIGKILL of this script, for instance by
# unshare --kill-child, and QEMU with run-qemu-smoke.sh below.
orphan_guard=(setpriv --pdeathsig TERM --)
"${orphan_guard[@]}" python3 -m http.server "$port" --bind 127.0.0.1 \
    --directory "$ROOTFS_DIR" >"$HTTP_LOG" 2>&1 &
srv=$!
# 10.0.2.2 is slirp's address for this namespace's loopback.
"${orphan_guard[@]}" "$HERE/smoke-dns.py" serve "$BOOT_NAME" 10.0.2.2 "$DNS_LOG" &
dns=$!

# Do not boot before both servers answer: a refused download is the failure
# this test exists to catch, and it must not be produced by the test itself.
http_up="" dns_up=""
for _ in $(seq 50); do
    [[ -z "$http_up" ]] &&
        curl -fsI "http://127.0.0.1:$port/ramroot.sqsh" >/dev/null 2>&1 && http_up=1
    [[ -z "$dns_up" ]] && "$HERE/smoke-dns.py" query "$BOOT_NAME" && dns_up=1
    [[ -n "$http_up" && -n "$dns_up" ]] && break
    sleep 0.1
done
[[ -n "$http_up" ]] || { echo "FAIL: the HTTP server does not answer inside the namespace" >&2; exit 1; }
[[ -n "$dns_up" ]] || { echo "FAIL: smoke-dns.py does not answer $BOOT_NAME" >&2; exit 1; }
: >"$DNS_LOG"

"${orphan_guard[@]}" "$HERE/run-qemu-smoke.sh" "$LOG" "$TIMEOUT" "$GRACE" -- \
    qemu-system-x86_64 "$@" \
    -netdev user,id=n0 -device "virtio-net-pci,netdev=n0,mac=$MAC" \
    -kernel "$ROOTFS_DIR/vmlinuz" -initrd "$ROOTFS_DIR/initrd.img" \
    -append "ramroot=http://$BOOT_NAME:$port/ramroot.sqsh console=ttyS0,115200 systemd.unit=multi-user.target systemd.log_target=console systemd.mask=serial-getty@ttyS0.service" ||
    exit $?

kill "$srv" "$dns" 2>/dev/null
wait "$srv" "$dns" 2>/dev/null
srv="" dns=""

"$HERE/check-boot-smoke.sh" "$LOG"
fail=$?

echo ""
echo "netboot:"
if grep -qaF "Hostname set to <$HOSTNAME_WANT>" "$LOG"; then
    echo "ok   - hostname is $HOSTNAME_WANT (last six hex digits of $MAC)"
else
    echo "FAIL - hostname is not $HOSTNAME_WANT ($(grep -aoE 'Hostname set to <[^>]*>' "$LOG" | tail -1))"
    fail=1
fi
if grep -qaF "thinfloe: network is up on " "$LOG"; then
    echo "ok   - the initramfs found the interface holding the DHCP lease"
else
    echo "FAIL - no 'thinfloe: network is up on' line"
    fail=1
fi
if grep -qaF "thinfloe: ramroot=http://$BOOT_NAME:" "$LOG"; then
    echo "ok   - the cmdline hook's ramroot= line reached the console"
else
    echo "FAIL - the cmdline hook's 'thinfloe: ramroot=' line is not on the console"
    fail=1
fi
if grep -qxF "1 $BOOT_NAME" "$DNS_LOG"; then
    echo "ok   - the guest asked the DHCP-supplied DNS server for $BOOT_NAME"
else
    echo "FAIL - smoke-dns.py was never asked for $BOOT_NAME (log: $DNS_LOG)"
    fail=1
fi
if grep -qF "GET /ramroot.sqsh " "$HTTP_LOG"; then
    echo "ok   - ramroot.sqsh was downloaded through $BOOT_NAME"
else
    echo "FAIL - ramroot.sqsh was never requested (server log: $HTTP_LOG)"
    fail=1
fi
if grep -qF "GET /home.pkg?mac=$MAC " "$HTTP_LOG"; then
    echo "ok   - home.pkg was requested under the boot MAC"
else
    echo "FAIL - no home.pkg request for mac=$MAC (server log: $HTTP_LOG)"
    fail=1
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - netboot smoke test (log: $LOG)"
    exit 1
fi
echo ""
echo "PASSED - netboot smoke test"
exit 0
