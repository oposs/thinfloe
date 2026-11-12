#!/bin/bash
# Offline test that the public image carries no site-specific content: no
# ThinLinc settings for the owner's server, and no host names, addresses or
# ports of the owner's network in the files that go into the image.
#
# Usage: test/integration/test-public-content.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO" || exit 1

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

if [[ -e overlay/home/tfuser/.thinlinc ]]; then
    bad "overlay/home/tfuser/.thinlinc exists ($(ls overlay/home/tfuser/.thinlinc | tr '\n' ' '))"
else
    ok "the overlay carries no ThinLinc client settings"
fi

hits=$(grep -rIl -e aarweg -e 'thinlinc-[a-z]*-[0-9]*\.oetiker\.ch' \
    -e '192\.168\.0\.11' -e 46022 overlay configs containers brand)
if [[ -z $hits ]]; then
    ok "no site-specific host, address or port in overlay, configs, containers, brand"
else
    bad "site-specific content in: $(echo "$hits" | tr '\n' ' ')"
fi

if grep -q '192\.168\.1\.100' configs/syslinux.cfg; then
    ok "configs/syslinux.cfg keeps the netboot server 192.168.1.100"
else
    bad "configs/syslinux.cfg lost 192.168.1.100"
fi

if grep -q 'gitea\.oetiker\.ch' overlay/etc/thinfloe/menu/20-oxulnk.desktop; then
    ok "20-oxulnk.desktop keeps gitea.oetiker.ch"
else
    bad "20-oxulnk.desktop lost gitea.oetiker.ch"
fi

exit $fail
