#!/bin/bash
# Offline tests for the initramfs home restore helpers in thinfloe-lib.sh.
#
# The home restore moved out of /etc/profile.d/10-thinfloe-session.sh and into
# the initramfs so that /home/tluser is populated before multi-user.target -
# which is what lets the SSH host keys ride along inside the home archive
# instead of needing their own EFI file or boot-server endpoint. See
# docs/superpowers/specs/2026-08-12-four-defects-design.md.
#
# This runs without a container and without root: it stubs dracut's logging
# functions and curl, and calls the library functions directly. It therefore
# proves the logic, not the integration - setup-overlay.sh calling
# restore_home at the right moment is covered by make qemu-smoke.
#
# Usage: test/integration/test-initramfs-home.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
LIB="$REPO/overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh"

if [[ ! -f "$LIB" ]]; then
    echo "FAIL: $LIB not found" >&2
    exit 1
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

# dracut's logging helpers are absent outside an initramfs. Define them
# before sourcing so the library's "type info || . /lib/dracut-lib.sh"
# guard does not try to pull in the real one.
info() { :; }
warn() { echo "warn: $*" >>"$WORK/warnings"; }
dinfo() { :; }
tf_log() { :; }
die()  { echo "die: $*" >>"$WORK/deaths"; return 1; }
: >"$WORK/warnings"
: >"$WORK/deaths"

# thinfloe-lib.sh guards its body with "if [ -z "$_TF_LIB_LOADED" ]", which
# under bash's `set -u` needs the variable to already exist (even unset) or
# referencing it aborts the script before the guard can even run.
_TF_LIB_LOADED=""

# shellcheck source=/dev/null
. "$LIB"
# The library redefines tf_log to write to /dev/console; silence it again.
tf_log() { :; }

# --- tf_home_url ------------------------------------------------------

url=$(tf_home_url "http://boot.example.org/tl/ramroot.sqsh" "aa:bb:cc:dd:ee:ff")
if [[ "$url" == "http://boot.example.org/tl/home.pkg?mac=aa:bb:cc:dd:ee:ff" ]]; then
    ok "tf_home_url builds the archive URL from the ramroot URL"
else
    bad "tf_home_url returned '$url'"
fi

if tf_home_url "" "aa:bb:cc:dd:ee:ff" >/dev/null 2>&1; then
    bad "tf_home_url accepted an empty ramroot URL"
else
    ok "tf_home_url rejects an empty ramroot URL"
fi

if tf_home_url "http://x/tl/ramroot.sqsh" "" >/dev/null 2>&1; then
    bad "tf_home_url accepted an empty MAC"
else
    ok "tf_home_url rejects an empty MAC"
fi

# --- tf_record_boot_mac ----------------------------------------------

mkdir -p "$WORK/sysfs/eth0" "$WORK/state"
echo "de:ad:be:ef:00:01" >"$WORK/sysfs/eth0/address"

if tf_record_boot_mac "$WORK/sysfs/eth0/address" "$WORK/state" &&
   [[ "$(cat "$WORK/state/boot-mac")" == "de:ad:be:ef:00:01" ]]; then
    ok "tf_record_boot_mac writes the MAC to <state-dir>/boot-mac"
else
    bad "tf_record_boot_mac did not record the MAC"
fi

if tf_record_boot_mac "$WORK/sysfs/nosuch/address" "$WORK/state" 2>/dev/null; then
    bad "tf_record_boot_mac succeeded on a missing address file"
else
    ok "tf_record_boot_mac fails on a missing address file"
fi

# --- tf_route_iface ---------------------------------------------------
#
# The boot MAC comes from the interface holding the default route, not from
# the first /sys/class/net entry.

routes='default via 10.0.2.2 dev enp0s3 proto dhcp src 10.0.2.15 metric 1024
default via 192.168.1.1 dev enp1s0 proto dhcp src 192.168.1.9 metric 2048'
got=$(printf '%s\n' "$routes" | tf_route_iface)
[[ "$got" == enp0s3 ]] && ok "tf_route_iface picks the first default route's device" \
    || bad "tf_route_iface returned '$got', want enp0s3"
if got=$(printf '' | tf_route_iface); then
    bad "tf_route_iface succeeded with no default route (printed '$got')"
else
    ok "tf_route_iface fails with no default route"
fi

MS_SRC="$REPO/overlay/usr/lib/dracut/modules.d/90thinfloe/mount-squashfs.sh"
if grep -v '^[[:space:]]*#' "$MS_SRC" | grep -qE '\bdhclient\b|/sys/class/net/\*'; then
    bad "mount-squashfs.sh still runs dhclient or picks the first /sys/class/net entry"
else
    ok "mount-squashfs.sh leaves DHCP to networkd and takes the lease's interface"
fi

# --- tf_restore_home_from_file ---------------------------------------

# Build a home archive the way thinfloe-save-home does: tar czf from inside
# $HOME, so members are stored as ./relative paths.
mkdir -p "$WORK/src-home/.thinlinc"
echo "layout=ch" >"$WORK/src-home/.thinfloe-environment"
echo "tlconf" >"$WORK/src-home/.thinlinc/tlclient.conf"
( cd "$WORK/src-home" && tar czf "$WORK/home.tar.gz" . )

TARGET="$WORK/newroot/home/tluser"
mkdir -p "$TARGET"

if tf_restore_home_from_file "$WORK/home.tar.gz" "$TARGET" &&
   [[ -f "$TARGET/.thinfloe-environment" ]] &&
   [[ -f "$TARGET/.thinlinc/tlclient.conf" ]]; then
    ok "restore_home_from_file extracts the archive into the target home"
else
    bad "restore_home_from_file did not extract the archive"
fi

# An absent archive is the normal first-boot case, not an error condition -
# it must not be fatal and must leave the default home alone.
mkdir -p "$WORK/newroot2/home/tluser"
echo "shipped-default" >"$WORK/newroot2/home/tluser/.bashrc"
if tf_restore_home_from_file "$WORK/nope.tar.gz" "$WORK/newroot2/home/tluser" 2>/dev/null; then
    bad "restore_home_from_file reported success on a missing archive"
else
    ok "restore_home_from_file fails quietly on a missing archive"
fi
if [[ "$(cat "$WORK/newroot2/home/tluser/.bashrc")" == "shipped-default" ]]; then
    ok "missing archive leaves the shipped default home intact"
else
    bad "missing archive damaged the default home"
fi

# A corrupt archive must warn and return non-zero, never abort the boot.
echo "this is not a tarball" >"$WORK/corrupt.tar.gz"
mkdir -p "$WORK/newroot3/home/tluser"
if tf_restore_home_from_file "$WORK/corrupt.tar.gz" "$WORK/newroot3/home/tluser" 2>/dev/null; then
    bad "restore_home_from_file reported success on a corrupt archive"
else
    ok "restore_home_from_file fails on a corrupt archive"
fi
if [[ ! -s "$WORK/deaths" ]]; then
    ok "no die() was called on any failure path"
else
    bad "a failure path called die(): $(cat "$WORK/deaths")"
fi

# Valid gzip that is not a tar: decompresses cleanly, lists no members, and
# GNU tar exits 0 on it. Without an explicit member check this reports a
# successful restore having restored nothing - see the comment in
# tf_restore_home_from_file for why that is worse than it sounds.
printf 'gzipped, but not a tar archive' | gzip > "$WORK/gz-not-tar.tar.gz"
mkdir -p "$WORK/newroot7/home/tluser"
echo "shipped-default" >"$WORK/newroot7/home/tluser/.bashrc"
if tf_restore_home_from_file "$WORK/gz-not-tar.tar.gz" \
        "$WORK/newroot7/home/tluser" 2>/dev/null; then
    bad "restore_home_from_file reported success on a valid-gzip non-tar payload"
else
    ok "restore_home_from_file rejects a valid-gzip non-tar payload"
fi
if [[ "$(cat "$WORK/newroot7/home/tluser/.bashrc")" == "shipped-default" ]]; then
    ok "valid-gzip non-tar payload leaves the default home intact"
else
    bad "valid-gzip non-tar payload damaged the default home"
fi

# --- tf_restore_home_from_url ----------------------------------------

# Stub curl. $TF_TEST_CURL_MODE selects the behaviour under test.
STUBS="$WORK/stubs"
mkdir -p "$STUBS"
cat >"$STUBS/curl" <<'STUB'
#!/bin/bash
# Minimal curl stand-in: find the -o target, act per TF_TEST_CURL_MODE.
out=""
prev=""
for a in "$@"; do
    [[ "$prev" == "-o" ]] && out="$a"
    prev="$a"
done
case "${TF_TEST_CURL_MODE:-ok}" in
    ok)      cp "$TF_TEST_CURL_PAYLOAD" "$out"; exit 0 ;;
    notfound) exit 22 ;;   # curl -f exit code for an HTTP 4xx
    timeout)  exit 28 ;;   # curl exit code for a timeout
esac
STUB
chmod +x "$STUBS/curl"
PATH="$STUBS:$PATH"
export TF_TEST_CURL_PAYLOAD="$WORK/home.tar.gz"

mkdir -p "$WORK/newroot4/home/tluser"
TF_TEST_CURL_MODE=ok
export TF_TEST_CURL_MODE
if tf_restore_home_from_url "http://boot/tl/home.pkg?mac=x" \
        "$WORK/newroot4/home/tluser" 5 "$WORK/dl.pkg" &&
   [[ -f "$WORK/newroot4/home/tluser/.thinfloe-environment" ]]; then
    ok "restore_home_from_url extracts a fetched archive"
else
    bad "restore_home_from_url did not extract a fetched archive"
fi
if [[ ! -e "$WORK/dl.pkg" ]]; then
    ok "restore_home_from_url removes its temporary download"
else
    bad "restore_home_from_url left $WORK/dl.pkg behind"
fi

# First netboot of a machine the server has never seen: a 404 is expected,
# not a fault.
mkdir -p "$WORK/newroot5/home/tluser"
TF_TEST_CURL_MODE=notfound
if tf_restore_home_from_url "http://boot/tl/home.pkg?mac=x" \
        "$WORK/newroot5/home/tluser" 5 "$WORK/dl5.pkg" 2>/dev/null; then
    bad "restore_home_from_url reported success on a 404"
else
    ok "restore_home_from_url fails quietly on a 404"
fi

# A hanging boot server must not hang the boot.
mkdir -p "$WORK/newroot6/home/tluser"
TF_TEST_CURL_MODE=timeout
if tf_restore_home_from_url "http://boot/tl/home.pkg?mac=x" \
        "$WORK/newroot6/home/tluser" 5 "$WORK/dl6.pkg" 2>/dev/null; then
    bad "restore_home_from_url reported success on a timeout"
else
    ok "restore_home_from_url fails quietly on a timeout"
fi

# The bounded wait is the whole point: the login-time script this replaces
# looped on ping forever, which in an initramfs is a machine that never boots
# and has no console to interrupt it.
if grep -q -- '--max-time' \
        "$REPO/overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh"; then
    ok "the fetch is bounded by --max-time"
else
    bad "no --max-time in the fetch - an unbounded wait would hang the boot"
fi

if [[ ! -s "$WORK/deaths" ]]; then
    ok "still no die() on any failure path"
else
    bad "a failure path called die(): $(cat "$WORK/deaths")"
fi

# --- restore_home (orchestrator) ----------------------------------------

# USB boot: ramroot is a relative path, archive sits at the EFI mount root.
mkdir -p "$WORK/usb/efi" "$WORK/usb/state" "$WORK/usb/newroot/home/tluser"
cp "$WORK/home.tar.gz" "$WORK/usb/efi/home.tar.gz"
if restore_home "$WORK/usb/newroot" "$WORK/usb/efi" "$WORK/usb/state" \
        "tl/ramroot.sqsh" &&
   [[ -f "$WORK/usb/newroot/home/tluser/.thinfloe-environment" ]]; then
    ok "restore_home restores from the EFI partition on USB boot"
else
    bad "restore_home did not restore from the EFI partition"
fi
if [[ "$(cat "$WORK/usb/state/home-restore" 2>/dev/null)" == "ok" ]]; then
    ok "restore_home records 'ok' after a successful USB restore"
else
    bad "restore_home did not record 'ok' after a successful USB restore (got: $(cat "$WORK/usb/state/home-restore" 2>/dev/null))"
fi

# USB boot, corrupt archive: thinfloe-save-home must NOT be allowed to save
# over the good archive with this default home, so the marker is "failed",
# not "absent" - there WAS something at the archive path, it just did not
# restore.
mkdir -p "$WORK/usbcorrupt/efi" "$WORK/usbcorrupt/state" "$WORK/usbcorrupt/newroot/home/tluser"
echo "this is not a tarball" >"$WORK/usbcorrupt/efi/home.tar.gz"
if restore_home "$WORK/usbcorrupt/newroot" "$WORK/usbcorrupt/efi" \
        "$WORK/usbcorrupt/state" "tl/ramroot.sqsh"; then
    ok "restore_home returns 0 on a corrupt USB archive"
else
    bad "restore_home returned non-zero on a corrupt USB archive"
fi
if [[ "$(cat "$WORK/usbcorrupt/state/home-restore" 2>/dev/null)" == "failed" ]]; then
    ok "restore_home records 'failed' for a corrupt USB archive"
else
    bad "restore_home did not record 'failed' for a corrupt USB archive (got: $(cat "$WORK/usbcorrupt/state/home-restore" 2>/dev/null))"
fi

# Netboot: uses the recorded boot MAC to build the URL.
mkdir -p "$WORK/net/efi" "$WORK/net/state" "$WORK/net/newroot/home/tluser"
echo "de:ad:be:ef:00:01" >"$WORK/net/state/boot-mac"
TF_TEST_CURL_MODE=ok
if restore_home "$WORK/net/newroot" "$WORK/net/efi" "$WORK/net/state" \
        "http://boot.example.org/tl/ramroot.sqsh" &&
   [[ -f "$WORK/net/newroot/home/tluser/.thinfloe-environment" ]]; then
    ok "restore_home fetches from the boot server on netboot"
else
    bad "restore_home did not fetch from the boot server"
fi
if [[ "$(cat "$WORK/net/state/home-restore" 2>/dev/null)" == "ok" ]]; then
    ok "restore_home records 'ok' after a successful netboot restore"
else
    bad "restore_home did not record 'ok' after a successful netboot restore (got: $(cat "$WORK/net/state/home-restore" 2>/dev/null))"
fi

# Netboot, server 404s: the ordinary first-netboot case for a machine the
# server has never seen, not a fault - "absent", and saving at logout is
# correct.
mkdir -p "$WORK/net404/efi" "$WORK/net404/state" "$WORK/net404/newroot/home/tluser"
echo "de:ad:be:ef:00:02" >"$WORK/net404/state/boot-mac"
TF_TEST_CURL_MODE=notfound
if restore_home "$WORK/net404/newroot" "$WORK/net404/efi" "$WORK/net404/state" \
        "http://boot.example.org/tl/ramroot.sqsh"; then
    ok "restore_home returns 0 on a netboot 404"
else
    bad "restore_home returned non-zero on a netboot 404"
fi
if [[ "$(cat "$WORK/net404/state/home-restore" 2>/dev/null)" == "absent" ]]; then
    ok "restore_home records 'absent' for a netboot 404"
else
    bad "restore_home did not record 'absent' for a netboot 404 (got: $(cat "$WORK/net404/state/home-restore" 2>/dev/null))"
fi

# Netboot, server times out: something was presumably there and it could not
# be fetched - "failed", so thinfloe-save-home must not overwrite it.
mkdir -p "$WORK/nettimeout/efi" "$WORK/nettimeout/state" "$WORK/nettimeout/newroot/home/tluser"
echo "de:ad:be:ef:00:03" >"$WORK/nettimeout/state/boot-mac"
TF_TEST_CURL_MODE=timeout
if restore_home "$WORK/nettimeout/newroot" "$WORK/nettimeout/efi" \
        "$WORK/nettimeout/state" "http://boot.example.org/tl/ramroot.sqsh"; then
    ok "restore_home returns 0 on a netboot timeout"
else
    bad "restore_home returned non-zero on a netboot timeout"
fi
if [[ "$(cat "$WORK/nettimeout/state/home-restore" 2>/dev/null)" == "failed" ]]; then
    ok "restore_home records 'failed' for a netboot timeout"
else
    bad "restore_home did not record 'failed' for a netboot timeout (got: $(cat "$WORK/nettimeout/state/home-restore" 2>/dev/null))"
fi
TF_TEST_CURL_MODE=ok

# Netboot with no recorded MAC: skip, do not guess. Guessing is exactly the
# failure this design removed - a wrong key means the home is posted where
# it will never be found again. No boot MAC is a fault in the boot itself,
# not a normal "nothing to restore", so the marker is "failed".
mkdir -p "$WORK/nomac/efi" "$WORK/nomac/state" "$WORK/nomac/newroot/home/tluser"
if restore_home "$WORK/nomac/newroot" "$WORK/nomac/efi" "$WORK/nomac/state" \
        "http://boot.example.org/tl/ramroot.sqsh"; then
    ok "restore_home returns 0 when no boot MAC was recorded"
else
    bad "restore_home returned non-zero with no boot MAC"
fi
if [[ "$(cat "$WORK/nomac/state/home-restore" 2>/dev/null)" == "failed" ]]; then
    ok "restore_home records 'failed' when no boot MAC was recorded"
else
    bad "restore_home did not record 'failed' with no boot MAC (got: $(cat "$WORK/nomac/state/home-restore" 2>/dev/null))"
fi

# The base the URL was built from must be recorded, so thinfloe-save-home posts
# to the same place instead of re-deriving it with a greedy sed over the whole
# /proc/cmdline. That sed backtracks to the last slash ANYWHERE in the command
# line, so a later parameter containing "/" (root=/dev/sda1, init=/sbin/init)
# silently produces a different base and orphans the client's home.
if [[ "$(cat "$WORK/net/state/boot-home-base" 2>/dev/null)" == "http://boot.example.org/tl" ]]; then
    ok "restore_home records boot-home-base for the save side to reuse"
else
    bad "restore_home did not record boot-home-base (got: $(cat "$WORK/net/state/boot-home-base" 2>/dev/null))"
fi

# USB boot never builds a URL, so there is nothing to record and its absence
# must not be treated as a fault.
if [[ ! -e "$WORK/usb/state/boot-home-base" ]]; then
    ok "no boot-home-base recorded on USB boot"
else
    bad "boot-home-base was recorded on USB boot, where it has no meaning"
fi

# Every failure mode must still return 0: restore_home is called from
# setup-overlay.sh, where a non-zero return would abort the boot.
mkdir -p "$WORK/fail/efi" "$WORK/fail/state" "$WORK/fail/newroot/home/tluser"
TF_TEST_CURL_MODE=timeout
if restore_home "$WORK/fail/newroot" "$WORK/fail/efi" "$WORK/fail/state" \
        "tl/ramroot.sqsh"; then
    ok "restore_home returns 0 when the USB archive is absent"
else
    bad "restore_home returned non-zero on a missing USB archive"
fi
if [[ "$(cat "$WORK/fail/state/home-restore" 2>/dev/null)" == "absent" ]]; then
    ok "restore_home records 'absent' when the USB archive is absent"
else
    bad "restore_home did not record 'absent' for a missing USB archive (got: $(cat "$WORK/fail/state/home-restore" 2>/dev/null))"
fi
TF_TEST_CURL_MODE=ok

if [[ ! -s "$WORK/deaths" ]]; then
    ok "restore_home never calls die()"
else
    bad "restore_home called die(): $(cat "$WORK/deaths")"
fi

# --- install_local_network ---
WGUNIT=/usr/lib/systemd/system/wg-quick@.service

mkhome() {  # mkhome <name>: fake newroot with a home, prints its path
    mkdir -p "$WORK/$1/home/tluser"
    echo "$WORK/$1"
}

nr=$(mkhome net-both)
echo "network: {version: 2}" >"$nr/home/tluser/wifi.yaml"
echo "[Interface]" >"$nr/home/tluser/wg0.conf"
install_local_network "$nr"; rc=$?
[[ $rc -eq 0 ]] && ok "install_local_network returns 0 with both files"     || bad "install_local_network returned $rc with both files"
if cmp -s "$nr/home/tluser/wifi.yaml" "$nr/etc/netplan/wifi.yaml" \
        && [[ "$(stat -c%a "$nr/etc/netplan/wifi.yaml")" == 600 ]]; then
    ok "wifi.yaml copied to etc/netplan, mode 600"
else
    bad "wifi.yaml not copied intact with mode 600"
fi
[[ "$(stat -c%a "$nr/etc/wireguard" 2>/dev/null)" == 700 ]] \
    && ok "etc/wireguard is mode 700" || bad "etc/wireguard is not mode 700"
if cmp -s "$nr/home/tluser/wg0.conf" "$nr/etc/wireguard/wg0.conf" \
        && [[ "$(stat -c%a "$nr/etc/wireguard/wg0.conf")" == 600 ]]; then
    ok "wg0.conf copied to etc/wireguard, mode 600"
else
    bad "wg0.conf not copied intact with mode 600"
fi
wl="$nr/etc/systemd/system/multi-user.target.wants/wg-quick@wg0.service"
if [[ -L "$wl" && "$(readlink "$wl")" == "$WGUNIT" ]]; then
    ok "wg-quick@wg0 wants symlink points at the template unit"
else
    bad "wg-quick@wg0 wants symlink missing or wrong"
fi

nr=$(mkhome net-none)
install_local_network "$nr"; rc=$?
if [[ $rc -eq 0 && ! -e "$nr/etc/netplan/wifi.yaml" && ! -e "$nr/etc/wireguard" \
        && ! -L "$nr/etc/systemd/system/multi-user.target.wants/wg-quick@wg0.service" ]]; then
    ok "no files: returns 0 and creates nothing"
else
    bad "no files: rc=$rc or something was created"
fi

nr=$(mkhome net-wg)
echo "[Interface]" >"$nr/home/tluser/wg0.conf"
install_local_network "$nr"; rc=$?
if [[ $rc -eq 0 && ! -e "$nr/etc/netplan/wifi.yaml" \
        && -f "$nr/etc/wireguard/wg0.conf" \
        && -L "$nr/etc/systemd/system/multi-user.target.wants/wg-quick@wg0.service" ]]; then
    ok "wg0.conf alone installs the WireGuard half only"
else
    bad "wg0.conf alone: rc=$rc, wrong result"
fi

nr=$(mkhome net-template)
cp "$REPO/overlay/home/tluser/wifi.yaml" "$nr/home/tluser/wifi.yaml"
install_local_network "$nr"; rc=$?
if [[ $rc -eq 0 && ! -e "$nr/etc/netplan/wifi.yaml" ]]; then
    ok "unedited wifi.yaml template is not installed, returns 0"
else
    bad "template wifi.yaml was installed or rc=$rc"
fi

nr=$(mkhome net-symlink)
echo secret >"$WORK/target.yaml"
ln -s "$WORK/target.yaml" "$nr/home/tluser/wifi.yaml"
install_local_network "$nr"; rc=$?
if [[ $rc -eq 0 && ! -e "$nr/etc/netplan/wifi.yaml" ]]; then
    ok "symlinked wifi.yaml is rejected, returns 0"
else
    bad "symlinked wifi.yaml was installed or rc=$rc"
fi

# --- tf_write_hostname ---

nr="$WORK/host-ok"; mkdir -p "$nr/etc"
tf_write_hostname "$nr" "abcdef0123456789abcdef0123456789"; rc=$?
if [[ $rc -eq 0 && "$(cat "$nr/etc/hostname")" == "thinfloe-abcdef" ]]; then
    ok "tf_write_hostname writes thinfloe-<first 6 of machine-id>"
else
    bad "tf_write_hostname: rc=$rc content=$(cat "$nr/etc/hostname" 2>&1)"
fi

for short in "" "abc12"; do
    nr="$WORK/host-short-${short:-empty}"; mkdir -p "$nr/etc"
    : >"$WORK/warnings"
    tf_write_hostname "$nr" "$short"; rc=$?
    if [[ $rc -eq 0 && ! -e "$nr/etc/hostname" && -s "$WORK/warnings" ]]; then
        ok "machine-id '${short}': no hostname written, warned, returns 0"
    else
        bad "machine-id '${short}': rc=$rc file=$(ls "$nr/etc") warnings=$(cat "$WORK/warnings")"
    fi
done

nr="$WORK/host-noetc"; mkdir -p "$nr"
: >"$WORK/warnings"
tf_write_hostname "$nr" "abcdef0123456789"; rc=$?
if [[ $rc -eq 0 && -s "$WORK/warnings" ]]; then
    ok "unwritable /etc: warns and returns 0"
else
    bad "unwritable /etc: rc=$rc warnings=$(cat "$WORK/warnings")"
fi

# --- tf_write_hostname: boot MAC ---

mkstate() { mkdir -p "$WORK/$1/etc" "$WORK/$1/state"; echo "$WORK/$1"; }
MID=0123456789abcdef0123456789abcdef

nr=$(mkstate mac-ok); echo "52:54:00:AB:CD:EF" >"$nr/state/boot-mac"
tf_write_hostname "$nr" "$MID" "$nr/state"; rc=$?
if [[ $rc -eq 0 && "$(cat "$nr/etc/hostname")" == "thinfloe-abcdef" ]]; then
    ok "boot MAC 52:54:00:AB:CD:EF -> thinfloe-abcdef (last six, lowercase, not the OUI)"
else
    bad "boot MAC hostname: rc=$rc content=$(cat "$nr/etc/hostname" 2>&1)"
fi

nr=$(mkstate mac-none)
tf_write_hostname "$nr" "$MID" "$nr/state"; rc=$?
if [[ $rc -eq 0 && "$(cat "$nr/etc/hostname")" == "thinfloe-012345" ]]; then
    ok "no boot-mac (USB boot) -> machine-id name"
else
    bad "no boot-mac: rc=$rc content=$(cat "$nr/etc/hostname" 2>&1)"
fi

nr=$(mkstate mac-bad); echo "zz" >"$nr/state/boot-mac"; : >"$WORK/warnings"
tf_write_hostname "$nr" "$MID" "$nr/state"; rc=$?
if [[ $rc -eq 0 && "$(cat "$nr/etc/hostname")" == "thinfloe-012345" && -s "$WORK/warnings" ]]; then
    ok "malformed MAC -> warns, falls back to machine-id"
else
    bad "malformed MAC: rc=$rc content=$(cat "$nr/etc/hostname" 2>&1) warnings=$(cat "$WORK/warnings")"
fi

nr=$(mkstate mac-nonhex); echo "52:54:00:ab:cd:gh" >"$nr/state/boot-mac"; : >"$WORK/warnings"
tf_write_hostname "$nr" "$MID" "$nr/state"
if [[ "$(cat "$nr/etc/hostname")" == "thinfloe-012345" && -s "$WORK/warnings" ]]; then
    ok "non-hex MAC digits -> warns, falls back to machine-id"
else
    bad "non-hex MAC: content=$(cat "$nr/etc/hostname" 2>&1)"
fi

nr=$(mkstate mac-both); echo "aa:bb:cc:11:22:33" >"$nr/state/boot-mac"
tf_write_hostname "$nr" "$MID" "$nr/state"
if [[ "$(cat "$nr/etc/hostname")" == "thinfloe-112233" ]]; then
    ok "MAC wins over machine-id when both are present"
else
    bad "MAC vs machine-id: content=$(cat "$nr/etc/hostname" 2>&1)"
fi

nr=$(mkstate mac-neither); echo "zz" >"$nr/state/boot-mac"; : >"$WORK/warnings"
tf_write_hostname "$nr" "" "$nr/state"; rc=$?
if [[ $rc -eq 0 && ! -e "$nr/etc/hostname" && -s "$WORK/warnings" ]]; then
    ok "neither MAC nor machine-id usable -> no hostname, warned, returns 0"
else
    bad "neither usable: rc=$rc file=$(ls "$nr/etc")"
fi

# --- only the initramfs's command set -----------------------------------
#
# The dev host has cut, basename, head and install; the initramfs does not.
# tf_write_hostname used `cut` and passed every test here while writing
# the hostname "thinfloe-" at boot. So run the initramfs helpers with a PATH
# holding only what module-setup.sh installs (parsed, not hard-coded) plus
# the dracut base tools the helpers rely on. A call to anything else fails.

MS="$REPO/overlay/usr/lib/dracut/modules.d/90thinfloe/module-setup.sh"
BIN="$WORK/initramfs-bin"; mkdir -p "$BIN"
installed=$(grep -E '^[[:space:]]*inst_multiple' "$MS" | sed 's/#.*//' \
    | tr ' ' '\n' | grep -vE '^(inst_multiple|-o|)$|^/' || true)
for tool in $installed cat rm mv sleep; do
    src=$(command -v "$tool" 2>/dev/null) && [[ "$src" == /* ]] && \
        ln -sf "$src" "$BIN/$tool"
done

if [[ -x "$BIN/tr" && -x "$BIN/mkdir" ]] \
   && ! PATH="$BIN" command -v cut >/dev/null 2>&1 \
   && ! PATH="$BIN" command -v basename >/dev/null 2>&1; then
    ok "initramfs PATH sandbox holds module-setup.sh's tools, and no cut/basename"
else
    bad "initramfs PATH sandbox is wrong (is cut/basename listed in inst_multiple?)"
fi

nr=$(mkstate sb-mac); echo "52:54:00:AB:CD:EF" >"$nr/state/boot-mac"
nr2=$(mkstate sb-id)
mkdir -p "$WORK/sb-net/home/tluser"; echo "x: 1" >"$WORK/sb-net/home/tluser/wifi.yaml"
echo "52:54:00:ab:cd:ef" >"$WORK/sb-addr"
: >"$WORK/warnings"
(
    PATH="$BIN"
    tf_write_hostname "$nr" "$MID" "$nr/state"
    tf_write_hostname "$nr2" "$MID" "$nr2/state"
    tf_record_boot_mac "$WORK/sb-addr" "$WORK/sb-state"
    install_local_network "$WORK/sb-net"
) 2>"$WORK/sb-stderr"
if [[ "$(cat "$nr/etc/hostname")" == "thinfloe-abcdef" \
   && "$(cat "$nr2/etc/hostname")" == "thinfloe-012345" \
   && "$(cat "$WORK/sb-state/boot-mac" 2>&1)" == "52:54:00:ab:cd:ef" \
   && -f "$WORK/sb-net/etc/netplan/wifi.yaml" \
   && ! -s "$WORK/sb-stderr" && ! -s "$WORK/warnings" ]]; then
    ok "hostname, boot-mac and local-network helpers work with only the initramfs's tools"
else
    bad "helpers failed under the initramfs PATH: hostnames=$(cat "$nr/etc/hostname" "$nr2/etc/hostname" 2>&1 | tr '\n' ' ') stderr=$(cat "$WORK/sb-stderr") warnings=$(cat "$WORK/warnings")"
fi

# --- home names: dbrrg -> thinfloe ---------------------------------------
#
# A stick in the field carries a home saved by 3.x with ~/.dbrrg-* names. The
# first 4.0 boot must rename them inside the initramfs restore, and leave the
# old names as symlinks so that 3.x in tl.old/ still finds its settings.
# Precedence is decided by what is in the archive: the default home already
# holds .thinfloe-environment and .thinfloe-sessionrc.

OLD_NAMES=".dbrrg-environment .dbrrg-sessionrc .dbrrg-password .dbrrg-ssh-host-keys .config/dbrrg"
declare -A LINK_TARGET=(
    [.dbrrg-environment]=.thinfloe-environment
    [.dbrrg-sessionrc]=.thinfloe-sessionrc
    [.dbrrg-password]=.thinfloe-password
    [.dbrrg-ssh-host-keys]=.thinfloe-ssh-host-keys
    [.config/dbrrg]=thinfloe
)

# mkdefault <dir>: a fake newroot holding the 4.0 default home; echoes the home.
mkdefault() {
    mkdir -p "$1/newroot/home/tluser" "$1/efi" "$1/state"
    echo default-env >"$1/newroot/home/tluser/.thinfloe-environment"
    echo default-rc  >"$1/newroot/home/tluser/.thinfloe-sessionrc"
}

# mk3x <srcdir>: the content of a 3.x home archive.
mk3x() {
    mkdir -p "$1/.dbrrg-ssh-host-keys" "$1/.config/dbrrg/menu"
    echo "XKB_DEFAULT_LAYOUT=ch" >"$1/.dbrrg-environment"
    echo "user-rc"   >"$1/.dbrrg-sessionrc"
    echo "pw-hash"   >"$1/.dbrrg-password"
    echo "hostkey"   >"$1/.dbrrg-ssh-host-keys/ssh_host_ed25519_key"
    echo "[Desktop Entry]" >"$1/.config/dbrrg/menu/x.desktop"
}

# check_3x_result <label> <home> <state>
check_3x_result() {
    local label="$1" h="$2" st="$3" n problems=""
    [[ "$(cat "$h/.thinfloe-environment" 2>/dev/null)" == "XKB_DEFAULT_LAYOUT=ch" ]] || problems+=" env-content"
    [[ "$(cat "$h/.thinfloe-sessionrc" 2>/dev/null)" == "user-rc" ]] || problems+=" rc-content"
    [[ "$(cat "$h/.thinfloe-password" 2>/dev/null)" == "pw-hash" ]] || problems+=" pw-content"
    [[ -f "$h/.thinfloe-ssh-host-keys/ssh_host_ed25519_key" ]] || problems+=" hostkey"
    [[ -f "$h/.config/thinfloe/menu/x.desktop" ]] || problems+=" menu"
    for n in $OLD_NAMES; do
        [[ -L "$h/$n" && "$(readlink "$h/$n")" == "${LINK_TARGET[$n]}" ]] || problems+=" link:$n"
    done
    for n in $OLD_NAMES; do
        grep -qxF "$n" "$st/home-renamed" 2>/dev/null || problems+=" renamed:$n"
    done
    [[ "$(cat "$st/home-restore" 2>/dev/null)" == "ok" ]] || problems+=" state"
    if [[ -z "$problems" ]]; then ok "$label"; else bad "$label:$problems"; fi
}

d="$WORK/hn-usb"; mkdefault "$d"; mkdir -p "$d/src"; mk3x "$d/src"
tar -C "$d/src" -czf "$d/efi/home.tar.gz" .
tf_log() { echo "$@" >>"$WORK/tflog"; }
: >"$WORK/tflog"
restore_home "$d/newroot" "$d/efi" "$d/state" "tl/ramroot.sqsh"
tf_log() { :; }
check_3x_result "3.x archive with a user XKB layout over the 4.0 default home" \
    "$d/newroot/home/tluser" "$d/state"
if grep -qxF "thinfloe: renamed ~/.dbrrg-environment to ~/.thinfloe-environment" "$WORK/tflog" \
   && grep -qxF "thinfloe: renamed ~/.config/dbrrg to ~/.config/thinfloe" "$WORK/tflog"; then
    ok "the rename is logged as 'thinfloe: renamed ~/old to ~/new'"
else
    bad "rename log lines missing: $(cat "$WORK/tflog")"
fi

# Listing fails although extraction worked: the restore counts as failed, the
# real old files stay, and no marker is written.
d="$WORK/hn-listfail"; mkdefault "$d"; mkdir -p "$d/src" "$d/fakebin"
mk3x "$d/src"; tar -C "$d/src" -czf "$d/efi/home.tar.gz" .
REALTAR=$(command -v tar)
cat >"$d/fakebin/tar" <<STUB
#!/bin/bash
# Fails only for list mode after the first call (the usability check lists too).
n=\$(cat "$d/tarcount" 2>/dev/null || echo 0)
case "\$1" in
    -t*) echo \$((n+1)) >"$d/tarcount"; [ "\$n" -ge 1 ] && exit 2 ;;
esac
exec $REALTAR "\$@"
STUB
chmod +x "$d/fakebin/tar"
: >"$WORK/warnings"
( PATH="$d/fakebin:$PATH"; restore_home "$d/newroot" "$d/efi" "$d/state" "tl/ramroot.sqsh" )
h="$d/newroot/home/tluser"
if [[ "$(cat "$d/state/home-restore")" == "failed" && ! -e "$d/state/home-renamed" \
   && -f "$h/.dbrrg-password" && ! -L "$h/.dbrrg-password" \
   && "$(cat "$h/.dbrrg-environment")" == "XKB_DEFAULT_LAYOUT=ch" ]] \
   && grep -q "could not list" "$WORK/warnings"; then
    ok "listing failure after extraction: restore failed, real old files kept, warned"
else
    bad "listing failure: state=$(cat "$d/state/home-restore") ls=$(ls -la "$h" | tr '\n' ' ')"
fi

# Early failed return (netboot without a boot MAC) still links the defaults.
d="$WORK/hn-nomac"; mkdefault "$d"
restore_home "$d/newroot" "$d/efi" "$d/state" "http://boot.example.org/tl/ramroot.sqsh"
h="$d/newroot/home/tluser"
if [[ "$(cat "$d/state/home-restore")" == "failed" && -L "$h/.dbrrg-environment" \
   && "$(readlink "$h/.dbrrg-environment")" == ".thinfloe-environment" ]]; then
    ok "netboot without boot MAC: failed, old names still linked to the defaults"
else
    bad "no-MAC path: $(ls -la "$h" | tr '\n' ' ')"
fi

# A real old entry replaced by the link is warned about.
d="$WORK/hn-warnreal"; mkdefault "$d"; echo old >"$d/newroot/home/tluser/.dbrrg-environment"
: >"$WORK/warnings"
tf_link_old_home_names "$d/newroot/home/tluser"
if grep -q "replacing real ~/.dbrrg-environment" "$WORK/warnings"; then
    ok "replacing a real old entry by a link is warned about"
else
    bad "no warning for the removed real entry"
fi

# Second boot: the saved 4.0 archive holds the new names and the old links.
d="$WORK/hn-4x"; mkdefault "$d"; mkdir -p "$d/src"
echo mine >"$d/src/.thinfloe-environment"
ln -s .thinfloe-environment "$d/src/.dbrrg-environment"
tar -C "$d/src" -czf "$d/efi/home.tar.gz" .
restore_home "$d/newroot" "$d/efi" "$d/state" "tl/ramroot.sqsh"
h="$d/newroot/home/tluser"
if [[ "$(cat "$h/.thinfloe-environment")" == "mine" && -L "$h/.dbrrg-environment" \
   && "$(readlink "$h/.dbrrg-environment")" == ".thinfloe-environment" \
   && ! -e "$d/state/home-renamed" ]]; then
    ok "4.0 archive: new file kept, old link intact, no home-renamed marker"
else
    bad "4.0 archive second boot: env=$(cat "$h/.thinfloe-environment") renamed=$(cat "$d/state/home-renamed" 2>&1)"
fi

# A 4.0 archive whose old name is a symlink to something else is still not 3.x.
d="$WORK/hn-4x-only"; mkdefault "$d"; mkdir -p "$d/src"
ln -s elsewhere "$d/src/.dbrrg-sessionrc"
tar -C "$d/src" -czf "$d/efi/home.tar.gz" .
restore_home "$d/newroot" "$d/efi" "$d/state" "tl/ramroot.sqsh"
h="$d/newroot/home/tluser"
if [[ "$(cat "$h/.thinfloe-sessionrc")" == "default-rc" && ! -e "$d/state/home-renamed" ]]; then
    ok "a symlink under the old name is never renamed over the default"
else
    bad "old-name symlink was treated as a 3.x file"
fi

# Both names as real files: the new name wins, the old becomes a link.
d="$WORK/hn-both"; mkdefault "$d"; mkdir -p "$d/src"
echo old >"$d/src/.dbrrg-password"; echo new >"$d/src/.thinfloe-password"
tar -C "$d/src" -czf "$d/efi/home.tar.gz" .
restore_home "$d/newroot" "$d/efi" "$d/state" "tl/ramroot.sqsh"
h="$d/newroot/home/tluser"
if [[ "$(cat "$h/.thinfloe-password")" == "new" && -L "$h/.dbrrg-password" \
   && "$(cat "$h/.dbrrg-password")" == "new" ]] \
   && ! grep -qxF .dbrrg-password "$d/state/home-renamed" 2>/dev/null; then
    ok "both names real in the archive: new wins, old becomes a link"
else
    bad "both names: new=$(cat "$h/.thinfloe-password") old=$(cat "$h/.dbrrg-password")"
fi

# A password cleared on 3.x: 3.x's `dbrrg-password --clear` removes only the
# ~/.dbrrg-password link, so its save holds .thinfloe-password without the old
# name. Every 4.0 write and restore creates the link, so this shape means
# "cleared by 3.x"; the restore must not bring the revoked password back.
d="$WORK/hn-pwcleared"; mkdefault "$d"; mkdir -p "$d/src"
echo mine >"$d/src/.thinfloe-environment"
ln -s .thinfloe-environment "$d/src/.dbrrg-environment"
echo revoked >"$d/src/.thinfloe-password"
tar -C "$d/src" -czf "$d/efi/home.tar.gz" .
tf_log() { echo "$@" >>"$WORK/tflog"; }
: >"$WORK/tflog"
restore_home "$d/newroot" "$d/efi" "$d/state" "tl/ramroot.sqsh"
tf_log() { :; }
h="$d/newroot/home/tluser"
if [[ ! -e "$h/.thinfloe-password" && ! -L "$h/.thinfloe-password" \
   && ! -e "$h/.dbrrg-password" && ! -L "$h/.dbrrg-password" \
   && "$(cat "$h/.thinfloe-environment")" == "mine" && -L "$h/.dbrrg-environment" \
   && "$(cat "$d/state/home-restore")" == "ok" ]] \
   && grep -qxF "thinfloe: ~/.dbrrg-password was removed by the previous version; clearing ~/.thinfloe-password" "$WORK/tflog"; then
    ok "password cleared on 3.x: ~/.thinfloe-password removed, no link, logged"
else
    bad "password cleared on 3.x: pw=$(cat "$h/.thinfloe-password" 2>&1) link=$(readlink "$h/.dbrrg-password") log=$(cat "$WORK/tflog")"
fi

# A home saved by 4.0 holds both names, the old one a link: the password stays.
d="$WORK/hn-pwkept"; mkdefault "$d"; mkdir -p "$d/src"
echo kept >"$d/src/.thinfloe-password"
ln -s .thinfloe-password "$d/src/.dbrrg-password"
tar -C "$d/src" -czf "$d/efi/home.tar.gz" .
restore_home "$d/newroot" "$d/efi" "$d/state" "tl/ramroot.sqsh"
h="$d/newroot/home/tluser"
if [[ "$(cat "$h/.thinfloe-password" 2>/dev/null)" == "kept" && -L "$h/.dbrrg-password" \
   && "$(readlink "$h/.dbrrg-password")" == ".thinfloe-password" ]]; then
    ok "4.0 archive with the password and its link: the password is kept"
else
    bad "4.0 password with link: pw=$(cat "$h/.thinfloe-password" 2>&1) link=$(readlink "$h/.dbrrg-password")"
fi

# Members without ./ (tar -C dir -czf archive .dbrrg-environment).
d="$WORK/hn-noslash"; mkdefault "$d"; mkdir -p "$d/src"
echo "XKB_DEFAULT_LAYOUT=de" >"$d/src/.dbrrg-environment"
tar -C "$d/src" -czf "$d/efi/home.tar.gz" .dbrrg-environment
tar -tzf "$d/efi/home.tar.gz" | grep -qxF .dbrrg-environment || bad "fixture: member has a ./ prefix"
restore_home "$d/newroot" "$d/efi" "$d/state" "tl/ramroot.sqsh"
h="$d/newroot/home/tluser"
if [[ "$(cat "$h/.thinfloe-environment")" == "XKB_DEFAULT_LAYOUT=de" && -L "$h/.dbrrg-environment" ]]; then
    ok "members listed without ./ are renamed"
else
    bad "members without ./: env=$(cat "$h/.thinfloe-environment")"
fi

# A directory listed only through its members, no entry of its own.
d="$WORK/hn-deep"; mkdefault "$d"; mkdir -p "$d/src/.config/dbrrg/menu" "$d/newroot/home/tluser/.config/thinfloe/menu"
echo default >"$d/newroot/home/tluser/.config/thinfloe/menu/default.desktop"
echo user >"$d/src/.config/dbrrg/menu/x.desktop"
tar -C "$d/src" -czf "$d/efi/home.tar.gz" .config/dbrrg/menu/x.desktop
if tar -tzf "$d/efi/home.tar.gz" | grep -qxF ".config/dbrrg/"; then
    bad "fixture: archive lists the directory itself"
fi
restore_home "$d/newroot" "$d/efi" "$d/state" "tl/ramroot.sqsh"
h="$d/newroot/home/tluser"
if [[ -f "$h/.config/thinfloe/menu/x.desktop" && ! -e "$h/.config/thinfloe/menu/default.desktop" \
   && -L "$h/.config/dbrrg" && "$(readlink "$h/.config/dbrrg")" == "thinfloe" ]]; then
    ok "a directory listed only through its members is renamed over the default"
else
    bad "deep members only: $(ls -R "$h/.config" 2>&1 | tr '\n' ' ')"
fi

# A ~/.config that is a symlink: renaming or linking below it would act on
# whatever it points to, possibly outside the home. The .config/dbrrg entry is
# skipped with a warning; the other names are handled as usual.
d="$WORK/hn-cfglink"; mkdefault "$d"; mkdir -p "$d/src" "$d/outside/dbrrg/menu" "$d/outside/thinfloe"
echo outside >"$d/outside/dbrrg/menu/x.desktop"
echo "XKB_DEFAULT_LAYOUT=fr" >"$d/src/.dbrrg-environment"
ln -s "$d/outside" "$d/src/.config"
tar -C "$d/src" -czf "$d/efi/home.tar.gz" .
: >"$WORK/warnings"
restore_home "$d/newroot" "$d/efi" "$d/state" "tl/ramroot.sqsh"
h="$d/newroot/home/tluser"
if [[ -L "$h/.config" && -d "$d/outside/dbrrg" && ! -L "$d/outside/dbrrg" \
   && ! -L "$d/outside/thinfloe/dbrrg" && ! -e "$d/outside/thinfloe/menu" \
   && "$(cat "$h/.thinfloe-environment")" == "XKB_DEFAULT_LAYOUT=fr" \
   && "$(grep -c 'is a symlink' "$WORK/warnings")" -ge 2 ]]; then
    ok "a symlinked ~/.config: .config/dbrrg skipped and warned, the rest migrated"
else
    bad "symlinked .config: outside=$(ls -la "$d/outside" | tr '\n' ' ') warnings=$(cat "$WORK/warnings")"
fi

# Absent archive: links for names whose new file exists, nothing else.
d="$WORK/hn-absent"; mkdefault "$d"
restore_home "$d/newroot" "$d/efi" "$d/state" "tl/ramroot.sqsh"
h="$d/newroot/home/tluser"
if [[ -L "$h/.dbrrg-environment" && "$(readlink "$h/.dbrrg-environment")" == ".thinfloe-environment" \
   && -L "$h/.dbrrg-sessionrc" && "$(readlink "$h/.dbrrg-sessionrc")" == ".thinfloe-sessionrc" \
   && ! -e "$h/.dbrrg-password" && ! -L "$h/.dbrrg-password" \
   && ! -L "$h/.dbrrg-ssh-host-keys" && ! -L "$h/.config/dbrrg" \
   && ! -e "$d/state/home-renamed" \
   && "$(cat "$d/state/home-restore")" == "absent" ]]; then
    ok "absent archive: links only for the defaults that exist, no home-renamed"
else
    bad "absent archive: $(ls -la "$h" 2>&1 | tr '\n' ' ')"
fi

d="$WORK/hn-corrupt"; mkdefault "$d"; echo "not a tarball" >"$d/efi/home.tar.gz"
restore_home "$d/newroot" "$d/efi" "$d/state" "tl/ramroot.sqsh"
h="$d/newroot/home/tluser"
if [[ -L "$h/.dbrrg-environment" && ! -L "$h/.dbrrg-password" && ! -e "$d/state/home-renamed" \
   && "$(cat "$d/state/home-restore")" == "failed" ]]; then
    ok "corrupt archive: links to the defaults, no home-renamed, state failed"
else
    bad "corrupt archive: $(ls -la "$h" 2>&1 | tr '\n' ' ')"
fi

# Netboot: the same 3.x archive through the curl stub.
d="$WORK/hn-net"; mkdefault "$d"; mkdir -p "$d/src"; mk3x "$d/src"
tar -C "$d/src" -czf "$d/home.pkg" .
echo "de:ad:be:ef:00:03" >"$d/state/boot-mac"
TF_TEST_CURL_PAYLOAD="$d/home.pkg" TF_TEST_CURL_MODE=ok \
    restore_home "$d/newroot" "$d/efi" "$d/state" "http://boot.example.org/tl/ramroot.sqsh"
check_3x_result "netboot: the 3.x home.pkg gets the same renames" \
    "$d/newroot/home/tluser" "$d/state"

# Idempotent: a second run changes nothing.
d="$WORK/hn-idem"; mkdefault "$d"; mkdir -p "$d/src"; mk3x "$d/src"
tar -C "$d/src" -czf "$d/a.tgz" .
h="$d/newroot/home/tluser"
tar -xzf "$d/a.tgz" -C "$h"
tf_migrate_home_names "$h" "$d/a.tgz" "$d/ren"; tf_link_old_home_names "$h"
snap1=$(cd "$h" && find . -printf '%p %y %l\n' | sort | tr '\n' ';')
tf_migrate_home_names "$h" "$d/a.tgz" "$d/ren"; tf_link_old_home_names "$h"
snap2=$(cd "$h" && find . -printf '%p %y %l\n' | sort | tr '\n' ';')
if [[ "$snap1" == "$snap2" && "$(cat "$h/.thinfloe-environment")" == "XKB_DEFAULT_LAYOUT=ch" ]]; then
    ok "migrate and link run twice give the same tree"
else
    bad "not idempotent: $snap1 <> $snap2"
fi

# Same helpers with only the initramfs's tools, on the 3.x archive.
d="$WORK/hn-sb"; mkdefault "$d"; mkdir -p "$d/src"; mk3x "$d/src"
tar -C "$d/src" -czf "$d/a.tgz" .
h="$d/newroot/home/tluser"
tar -xzf "$d/a.tgz" -C "$h"
: >"$WORK/warnings"
( PATH="$BIN"; tf_migrate_home_names "$h" "$d/a.tgz" "$d/state/home-renamed"; tf_link_old_home_names "$h" ) 2>"$d/stderr"
echo ok >"$d/state/home-restore"
if [[ ! -s "$d/stderr" && ! -s "$WORK/warnings" ]]; then
    check_3x_result "rename and link helpers work with only the initramfs's tools" "$h" "$d/state"
else
    bad "sandboxed rename: stderr=$(cat "$d/stderr") warnings=$(cat "$WORK/warnings")"
fi

# The cleared-on-3.x check with only the initramfs's tools.
d="$WORK/hn-sb-pw"; mkdefault "$d"; mkdir -p "$d/src"
echo revoked >"$d/src/.thinfloe-password"
tar -C "$d/src" -czf "$d/a.tgz" .
h="$d/newroot/home/tluser"
tar -xzf "$d/a.tgz" -C "$h"
: >"$WORK/warnings"
( PATH="$BIN"; tf_migrate_home_names "$h" "$d/a.tgz" "$d/ren"; tf_link_old_home_names "$h" ) 2>"$d/stderr"
if [[ ! -e "$h/.thinfloe-password" && ! -L "$h/.dbrrg-password" \
   && ! -s "$d/stderr" && ! -s "$WORK/warnings" ]]; then
    ok "the cleared-on-3.x check works with only the initramfs's tools"
else
    bad "sandboxed clear: pw=$(cat "$h/.thinfloe-password" 2>&1) stderr=$(cat "$d/stderr") warnings=$(cat "$WORK/warnings")"
fi

# --- tf_finalize_upgrade, inside the same sandbox ---------------------
#
# The tl.new -> tl -> tl.old rotation called sync, which the initramfs did not
# have, so every sync in it failed with "command not found" on the stick.
#
# The rotation used to run on every boot that found a tl.new, also when the
# boot loader had loaded the kernel from tl/: the old kernel then ran on the
# new ramroot.sqsh, whose modules are for the new kernel, and every module not
# already in the initramfs (igc, an i226-V NIC) failed to load. thinfloe-upgrade
# now points syslinux at the new "LABEL new" entry, which boots the kernel
# from tl.new/, and the rotation runs only on such a boot.

SYSLINUX_CFG='DEFAULT new
LABEL current
    KERNEL /tl/vmlinuz
    APPEND ramroot=tl/ramroot.sqsh
LABEL new
    KERNEL /tl.new/vmlinuz
    APPEND ramroot=tl.new/ramroot.sqsh'

mkesp() {  # mkesp <name>: fake ESP with tl, tl.old, a complete tl.new and DEFAULT new
    local d="$WORK/$1"
    mkdir -p "$d/tl" "$d/tl.old" "$d/tl.new" "$d/efi/boot"
    echo current >"$d/tl/marker"; echo previous >"$d/tl.old/marker"
    for f in vmlinuz initrd.img ramroot.sqsh; do echo new >"$d/tl.new/$f"; done
    printf '%s\n' "$SYSLINUX_CFG" >"$d/syslinux.cfg"
    printf '%s\n' "$SYSLINUX_CFG" >"$d/efi/boot/syslinux.cfg"
    echo "$d"
}
defaults() {  # defaults <esp>: the DEFAULT line of both syslinux.cfg copies
    grep -h '^DEFAULT' "$1/syslinux.cfg" "$1/efi/boot/syslinux.cfg" | tr '\n' ' '
}

esp=$(mkesp esp-rotate); echo tl.new/ramroot.sqsh >"$WORK/rr-rotate"
( PATH="$BIN"; tf_finalize_upgrade "$esp" tl.new/ramroot.sqsh "$WORK/rr-rotate" ) \
    2>"$WORK/fu-stderr"; rc=$?
if [[ $rc -eq 0 && -f "$esp/tl/ramroot.sqsh" && ! -e "$esp/tl.new" \
   && "$(cat "$esp/tl.old/marker")" == current && ! -s "$WORK/fu-stderr" ]]; then
    ok "a boot from tl.new rotates tl.new -> tl -> tl.old with only the initramfs's tools"
else
    bad "tf_finalize_upgrade under the initramfs PATH: rc=$rc stderr=$(cat "$WORK/fu-stderr") esp=$(ls "$esp" | tr '\n' ' ')"
fi
if [[ "$(defaults "$esp")" == "DEFAULT current DEFAULT current " ]]; then
    ok "after the rotation both syslinux.cfg copies boot current again"
else
    bad "syslinux.cfg defaults after the rotation: $(defaults "$esp")"
fi
if [[ "$(cat "$WORK/rr-rotate")" == tl/ramroot.sqsh ]]; then
    ok "after the rotation the squashfs is mounted from tl/"
else
    bad "ramroot after the rotation: $(cat "$WORK/rr-rotate")"
fi

esp=$(mkesp esp-from-tl); echo tl/ramroot.sqsh >"$WORK/rr-from-tl"
( PATH="$BIN"; tf_finalize_upgrade "$esp" tl/ramroot.sqsh "$WORK/rr-from-tl" ) 2>/dev/null; rc=$?
if [[ $rc -eq 0 && -d "$esp/tl.new" && "$(cat "$esp/tl/marker")" == current \
   && "$(cat "$WORK/rr-from-tl")" == tl/ramroot.sqsh ]]; then
    ok "a boot whose kernel came from tl/ leaves a pending tl.new alone"
else
    bad "boot from tl/ with tl.new pending: rc=$rc ramroot=$(cat "$WORK/rr-from-tl") esp=$(ls "$esp" | tr '\n' ' ')"
fi

esp=$(mkesp esp-incomplete); rm "$esp/tl.new/ramroot.sqsh"; echo tl.new/ramroot.sqsh >"$WORK/rr-inc"
( PATH="$BIN"; tf_finalize_upgrade "$esp" tl.new/ramroot.sqsh "$WORK/rr-inc" ) 2>/dev/null; rc=$?
if [[ $rc -eq 0 && -d "$esp/tl.new" && "$(cat "$esp/tl/marker")" == current \
   && "$(cat "$WORK/rr-inc")" == tl.new/ramroot.sqsh ]]; then
    ok "an incomplete tl.new is left alone"
else
    bad "incomplete tl.new: rc=$rc esp=$(ls "$esp" | tr '\n' ' ')"
fi

esp="$WORK/esp-none"; mkdir -p "$esp/tl"; echo tl/ramroot.sqsh >"$WORK/rr-none"
( PATH="$BIN"; tf_finalize_upgrade "$esp" tl/ramroot.sqsh "$WORK/rr-none" ) 2>/dev/null; rc=$?
if [[ $rc -eq 0 && -d "$esp/tl" && ! -e "$esp/tl.old" ]]; then
    ok "no pending upgrade: nothing is touched"
else
    bad "no pending upgrade: rc=$rc esp=$(ls "$esp" | tr '\n' ' ')"
fi

# Resetting DEFAULT works on a stick without the EFI copy, and leaves every
# other line alone.
esp=$(mkesp esp-default); rm "$esp/efi/boot/syslinux.cfg"
( PATH="$BIN"; tf_set_boot_default "$esp" current ) 2>"$WORK/sd-stderr"; rc=$?
if [[ $rc -eq 0 && "$(sed 1d "$esp/syslinux.cfg")" == "$(printf '%s\n' "$SYSLINUX_CFG" | sed 1d)" \
   && "$(head -n1 "$esp/syslinux.cfg")" == "DEFAULT current" && ! -s "$WORK/sd-stderr" \
   && ! -e "$esp/syslinux.cfg.tmp" ]]; then
    ok "tf_set_boot_default changes only the DEFAULT line"
else
    bad "tf_set_boot_default: rc=$rc stderr=$(cat "$WORK/sd-stderr") cfg=$(cat "$esp/syslinux.cfg")"
fi

# The pre-mount hook used to run in parallel with the cmdline hook that writes
# /tmp/thinfloe-ramroot, so it must read ramroot= from the command line itself.
# Reading the file made every netboot wait 60s for an ESP. It does write the
# file, after a rotation; the drop-in orders that write after the cmdline
# hook's.
FU="$REPO/overlay/usr/lib/dracut/modules.d/90thinfloe/finalize-upgrade.sh"
if grep -v '^[[:space:]]*#' "$FU" | grep -qE '(cat|<)[[:space:]]*/tmp/thinfloe-ramroot'; then
    bad "finalize-upgrade.sh reads /tmp/thinfloe-ramroot, which may not exist yet in pre-mount"
else
    ok "finalize-upgrade.sh does not read the cmdline hook's /tmp/thinfloe-ramroot"
fi
if grep -qE '^tf_finalize_upgrade "\$efi_mount" "\$ramroot" /tmp/thinfloe-ramroot$' "$FU"; then
    ok "finalize-upgrade.sh passes the boot's ramroot= and the mount hook's file"
else
    bad "finalize-upgrade.sh does not call tf_finalize_upgrade with ramroot= and /tmp/thinfloe-ramroot"
fi

# finalize-upgrade.sh itself mounts the ESP and so cannot run here; this
# pins the two commands it was missing.
for tool in sync rmdir; do
    if [[ -x "$BIN/$tool" ]]; then
        ok "module-setup.sh installs $tool"
    else
        bad "module-setup.sh does not install $tool, which finalize-upgrade needs"
    fi
done

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - initramfs home helpers"
    exit 1
fi

echo ""
echo "PASSED - initramfs home helpers"
exit 0
