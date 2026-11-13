#!/bin/bash
# The network files the menu renders (src/thinfloe-menu/src/netfiles.rs) must
# pass the netplan that the image carries, and the root tool's apply must
# install them. Netplan is not faked here: the menu's test writes the files
# (TF_NETPLAN_FIXTURES), then a throw-away container of the built image runs
# `netplan generate --root-dir` on them as tfuser, and apply as root.
#
# Usage: TF_UBUNTU_IMAGE=localhost/thinfloe-ubuntu:X test/integration/test-network-netplan.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="${TF_UBUNTU_IMAGE:-localhost/thinfloe-ubuntu:3.0.0}"
SCRATCH="$(mktemp -d "${TMPDIR:-/scratch/oetiker/claude-tmp}/netplan-fixtures.XXXXXX")" || exit 1
trap 'rm -rf "$SCRATCH"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

FIXTURES="$SCRATCH/fixtures"
mkdir -p "$FIXTURES"
if ! (cd "$REPO/src/thinfloe-menu" \
        && TF_NETPLAN_FIXTURES="$FIXTURES" systemd-run --user --scope -q -p MemoryMax=4G -- \
           cargo test --locked -j 4 --lib -- write_netplan_fixtures >"$SCRATCH/cargo.log" 2>&1); then
    tail -20 "$SCRATCH/cargo.log"
    bad "the menu could not write the network files"
    echo "FAILED - network files and netplan"
    exit 1
fi
for f in wifi-two wifi-odd wifi-bare wifi-dns ethernet-static ethernet-dhcp-dns; do
    [[ -s "$FIXTURES/$f.yaml" ]] || bad "the menu did not write $f.yaml"
done
[[ $fail -eq 0 ]] || { echo "FAILED - network files and netplan"; exit 1; }

# Runs in the container as root. Prints "ok   - " and "FAIL - " lines.
INNER=$(cat <<'INNER_EOF'
set -u
W=$(mktemp -d); chmod 755 "$W"
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; }
check() { # label, command...
    local label=$1; shift
    if "$@"; then ok "$label"; else bad "$label"; fi
}

# gen NAME FILE: netplan generate as tfuser on a root that holds FILE as
# wifi.yaml (or ethernet.yaml for ethernet-*). Sets R to the root; returns
# netplan's exit status.
gen() {
    local name=$1 file=$2 dest=wifi.yaml
    [[ $name == ethernet-* ]] && dest=ethernet.yaml
    R=$W/$name
    mkdir -p "$R/etc/netplan"
    cp "$file" "$R/etc/netplan/$dest"
    chmod 600 "$R/etc/netplan/$dest"
    chown -R tfuser "$R"
    runuser -u tfuser -- netplan generate --root-dir "$R" >"$R/out" 2>&1
}

# Blocks of a wpa_supplicant conf, sorted: netplan writes the networks in
# hash order.
blocks() {
    [[ -r $1 ]] || return 1
    python3 -I -B -c '
import sys
text = open(sys.argv[1], encoding="utf-8").read()
parts = text.split("network={")
print(parts[0].strip())
for b in sorted(parts[1:]):
    print("network={" + b.rstrip())
' "$1"
}

has_line() { grep -qxF -- "$2" "$1"; }

# --- menu wifi gives netplan-wpa-<dev> ---
gen two /fx/wifi-two.yaml; rc=$?
conf=$R/run/netplan/wpa-wlp58s0.conf
svc=$(find "$R/run" -name netplan-wpa-wlp58s0.service 2>/dev/null | head -n1)
if [[ $rc -eq 0 && -n $svc && -s $conf ]] \
    && python3 -I -B -c '
import sys
blocks = open(sys.argv[1], encoding="utf-8").read().split("network={")[1:]
hidden = [b for b in blocks if "hidden net" in b]
opened = [b for b in blocks if "open net" in b]
assert len(blocks) == 2 and len(hidden) == 1 and len(opened) == 1
assert "scan_ssid=1" in hidden[0] and "scan_ssid" not in opened[0]
assert "key_mgmt=NONE" in opened[0] and "psk" not in opened[0]
' "$conf"; then
    ok "menu wifi gives netplan-wpa-wlp58s0 (hidden: scan_ssid=1, open: key_mgmt=NONE)"
else
    bad "menu wifi gives netplan-wpa-wlp58s0"; cat "$R/out"; cat "$conf" 2>/dev/null
fi

# --- wifi is not waited for ---
waitconf=$(find "$R/run" -path '*systemd-networkd-wait-online.service.d/10-netplan.conf' 2>/dev/null | head -n1)
if [[ -z $waitconf ]] || ! grep -q wlp58s0 "$waitconf"; then
    ok "wifi is not waited for"
else
    bad "wifi is not waited for"; cat "$waitconf"
fi

# --- odd text reaches the wpa conf ---
gen odd /fx/wifi-odd.yaml; rc=$?
odd_conf=$R/run/netplan/wpa-wlp58s0.conf
cat >"$W/hand.yaml" <<'YAML'
network:
  version: 2
  renderer: networkd
  wifis:
    wlp58s0:
      access-points:
        "yes": {password: "p@ss: \"x\" #1\\"}
        "Café \"Nord\"": {password: "p@ss: \"x\" #1\\", hidden: true}
        "a:b #c": {password: "p@ss: \"x\" #1\\", hidden: true}
        "123": {password: "p@ss: \"x\" #1\\", hidden: true}
        " lead": {password: "p@ss: \"x\" #1\\"}
        "open": {}
      dhcp4: true
      optional: true
YAML
gen hand "$W/hand.yaml"; hrc=$?
if [[ $rc -eq 0 && $hrc -eq 0 ]] \
    && [[ "$(blocks "$odd_conf")" == "$(blocks "$R/run/netplan/wpa-wlp58s0.conf")" ]] \
    && [[ $(grep -c '^network={' "$odd_conf") -eq 6 ]]; then
    ok "odd text reaches wpa conf as a hand-written file gives it"
else
    bad "odd text reaches wpa conf"; cat "$W/odd/out" "$W/hand/out"
    diff <(blocks "$odd_conf") <(blocks "$W/hand/run/netplan/wpa-wlp58s0.conf")
fi
echo "# netplan wrote for the odd names:" >&2
grep -E '^\s*(ssid|psk)=' "$odd_conf" | sed 's/^/#   /' >&2

# --- netplan still drops a backslash in ssid (the menu refuses it) ---
# netplan writes ssid=P"..." without escaping a backslash, and wpa_supplicant
# decodes it printf-style: back\slash is joined as backslash. The menu refuses
# a backslash in a name for that reason. Ask wpa_supplicant what it read.
cat >"$W/bs.yaml" <<'YAML'
network:
  version: 2
  renderer: networkd
  wifis:
    wlp58s0:
      dhcp4: true
      access-points:
        "back\\slash": {password: "secret123"}
        "plain": {password: "secret123"}
YAML
gen bs "$W/bs.yaml"; rc=$?
wpa_ssids=$(timeout 5 wpa_supplicant -ddd -c "$R/run/netplan/wpa-wlp58s0.conf" -i lo -D wext -N 2>&1 | grep -a "^   id=[0-9]* ssid=")
if [[ $rc -eq 0 ]] && grep -qF "ssid='plain'" <<<"$wpa_ssids" && grep -qF "ssid='backslash'" <<<"$wpa_ssids"; then
    ok "netplan still drops a backslash in ssid (the menu refuses it)"
else
    bad "netplan no longer drops a backslash in ssid (wpa_supplicant read: $(grep -o "ssid='[^']*'" <<<"$wpa_ssids" | paste -sd' ')): netplan was fixed, the menu rule in check_ssid can be lifted"
fi

# --- bare wifi file passes ---
gen bare /fx/wifi-bare.yaml
check "bare wifi file passes" test $? -eq 0

# --- static wired keeps id eth ---
gen ethernet-static /fx/ethernet-static.yaml; rc=$?
net=$R/run/systemd/network/10-netplan-eth.network
if [[ $rc -eq 0 ]] && has_line "$net" Address=192.168.1.10/24 && has_line "$net" Gateway=192.168.1.1 \
    && has_line "$net" DNS=1.1.1.1 && has_line "$net" 'Name=en*'; then
    ok "static wired keeps id eth"
else
    bad "static wired keeps id eth"; cat "$R/out" "$net" 2>/dev/null
fi

# --- dhcp wired with DNS servers replaces those DHCP gives ---
gen ethernet-dhcp-dns /fx/ethernet-dhcp-dns.yaml; rc=$?
net=$R/run/systemd/network/10-netplan-eth.network
if [[ $rc -eq 0 ]] && has_line "$net" DHCP=ipv4 && has_line "$net" DNS=1.1.1.1 \
    && has_line "$net" DNS=9.9.9.9 && has_line "$net" UseDNS=false && has_line "$net" 'Name=en*' \
    && ! grep -q '^Address=' "$net"; then
    ok "dhcp wired with DNS servers replaces those DHCP gives"
else
    bad "dhcp wired with DNS servers replaces those DHCP gives"; cat "$R/out" "$net" 2>/dev/null
fi

# --- wifi with DNS servers replaces those DHCP gives ---
gen wifi-dns /fx/wifi-dns.yaml; rc=$?
net=$R/run/systemd/network/10-netplan-wlp58s0.network
if [[ $rc -eq 0 ]] && has_line "$net" DHCP=ipv4 && has_line "$net" DNS=1.1.1.1 \
    && has_line "$net" DNS=9.9.9.9 && has_line "$net" UseDNS=false \
    && [[ -s $R/run/netplan/wpa-wlp58s0.conf ]]; then
    ok "wifi with DNS servers replaces those DHCP gives"
else
    bad "wifi with DNS servers replaces those DHCP gives"; cat "$R/out" "$net" 2>/dev/null
fi

# --- dhcp default is the shipped file ---
if cmp -s /usr/share/thinfloe/netplan/ethernet.yaml /etc/netplan/ethernet.yaml \
    && [[ $(stat -c %a /usr/share/thinfloe/netplan/ethernet.yaml) == 644 ]]; then
    ok "dhcp default is the shipped file"
else
    bad "dhcp default is the shipped file"
fi

# --- comments-only wifi.yaml passes as it is ---
printf '# only comments\n\n# here\n' >"$W/comments.yaml"
gen comments "$W/comments.yaml"; rc=$?
if [[ $rc -eq 0 ]]; then
    ok "comments-only wifi.yaml passes as it is"
else
    bad "comments-only wifi.yaml passes as it is"; cat "$R/out"
fi

# --- apply as root installs ---
mkdir -p "$W/home/.config/thinfloe/generated" "$W/run"
cp /fx/wifi-two.yaml "$W/home/.config/thinfloe/generated/wifi.yaml"
cat >"$W/netplan" <<'SH'
#!/bin/sh
if [ "$1" = generate ]; then exec /usr/sbin/netplan "$@"; fi
echo "$@" >>"$TF_TEST_RECORD"
SH
chmod 755 "$W/netplan"
rm -f /etc/netplan/wifi.yaml
out=$(TF_NETWORK_HOME=$W/home TF_NETWORK_ESP=/ TF_NETWORK_NETPLAN=$W/netplan \
      TF_NETWORK_LOCK=$W/run/lock TF_NETWORK_TMP=$W/run/work TF_TEST_RECORD=$W/record \
      /usr/bin/thinfloe-network apply 2>"$W/apply.err")
rc=$?
if [[ $rc -eq 0 && $out == *'"result": "ok"'* ]] \
    && [[ $(stat -c %a /etc/netplan/wifi.yaml) == 600 ]] \
    && cmp -s /etc/netplan/wifi.yaml "$W/home/.config/thinfloe/generated/wifi.yaml" \
    && [[ "$(cat "$W/record" 2>/dev/null)" == apply ]]; then
    ok "apply as root installs"
else
    bad "apply as root installs: rc=$rc out=$out record=$(cat "$W/record" 2>/dev/null)"; cat "$W/apply.err"
fi
rm -rf "$W"
INNER_EOF
)

out=$(podman run --rm --log-driver=none --network=none \
        -v "$FIXTURES:/fx:ro,Z" "$IMAGE" /bin/bash -c "$INNER" 2>"$SCRATCH/inner.err")
echo "$out"
grep '^#' "$SCRATCH/inner.err"
grep -v '^#' "$SCRATCH/inner.err" | head -20
grep -q '^FAIL - ' <<<"$out" && fail=1
[[ $(grep -cE '^(ok   - |FAIL - )' <<<"$out") -ge 11 ]] || { bad "fewer cases ran than expected"; }

[[ $fail -eq 0 ]] || { echo "FAILED - network files and netplan"; exit 1; }
echo "PASSED - network files and netplan"
