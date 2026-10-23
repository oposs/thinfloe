#!/bin/bash
# The image no longer carries ThinLinc or OxuLnk, so the libraries they need
# must come from the image. Installs each client the way thinfloe-addon does
# (dpkg -i) into a throw-away container of the built image, then checks that
# every ELF file resolves all its libraries and that the client starts.
#
# ThinLinc: $THINLINC_DEB, else the .deb that the shipped tile's page and
# pattern name, downloaded once into ${TMPDIR}/thinfloe-addon-cache/.
# Neither available: the test fails. OxuLnk: only when $OXULNK_DEB is set.
#
# Usage: TF_UBUNTU_IMAGE=localhost/thinfloe-ubuntu:X test/integration/test-addon-deps.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="${TF_UBUNTU_IMAGE:-localhost/thinfloe-ubuntu:3.0.0}"
TILE="$REPO/overlay/etc/thinfloe/menu/10-thinlinc.desktop"
CACHE="${TMPDIR:-/scratch/oetiker/claude-tmp}/thinfloe-addon-cache"

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

# Downloads the newest .deb the tile's page and pattern yield. Prints its path.
fetch_thinlinc() {
    local page pattern url
    page=$(sed -n 's/^X-TF-Source-Page=//p' "$TILE")
    pattern=$(sed -n 's/^X-TF-Source-Pattern=//p' "$TILE")
    [[ -n "$page" && -n "$pattern" ]] || return 1
    mkdir -p "$CACHE" || return 1
    url=$(curl -fsSL --max-time 60 "$page" | python3 -I -c '
import re, sys
pat = re.compile(sys.argv[1])
best = None
for m in pat.finditer(sys.stdin.read()):
    v = m.group("version")
    key = [int(x) for x in re.split(r"[.-]", v)]
    if best is None or key > best[0]:
        best = (key, m.group(0))
print(best[1] if best else "")
' "$pattern") || return 1
    [[ -n "$url" ]] || return 1
    local file="$CACHE/${url##*/}"
    if [[ ! -s "$file" ]]; then
        curl -fsSL --max-time 600 -o "$file.part" "$url" && mv "$file.part" "$file" || return 1
    fi
    echo "$file"
}

# $1 label, $2 deb path, $3 package, $4 command that must exit 0, $5 add-on id
check_addon() {
    local label=$1 deb=$2 pkg=$3 cmd=$4 id=$5 out
    if [[ ! -s "$deb" ]]; then
        bad "$label: no package at $deb"
        return
    fi
    out=$(podman run --rm --network=none -v "$deb:/in/addon.deb:ro,Z" "$IMAGE" /bin/bash -c '
        set -u
        dpkg -i /in/addon.deb >/dev/null 2>&1 || { echo "dpkg -i failed"; dpkg -i /in/addon.deb 2>&1 | tail -5; exit 3; }
        # thinfloe-addon runs the add-on'"'"'s hook after dpkg.
        hook=/usr/libexec/thinfloe/addon-hooks/'"$id"'
        if [ -x "$hook" ]; then "$hook" || { echo "hook $hook failed"; exit 6; }; fi
        # A client keeps private libraries in its own directories and finds
        # them through its wrapper; give ldd the same directories.
        LD_LIBRARY_PATH=$(dpkg -L '"$pkg"' | grep -E "\.so(\.|$)" | while read -r f; do [ -f "$f" ] && dirname "$f"; done | sort -u | paste -sd:)
        export LD_LIBRARY_PATH
        { dpkg -L '"$pkg"'; echo /usr/lib/x86_64-linux-gnu/libXrandr.so.2; } | while read -r f; do
            [ -f "$f" ] || continue
            [ "$(head -c4 "$f" 2>/dev/null | od -An -c | tr -d " ")" = "177ELF" ] || continue
            ldd "$f" 2>&1 | grep "not found" | sed "s|^|$f: |"
        done > /tmp/missing
        cat /tmp/missing
        [ -s /tmp/missing ] && exit 4
        '"$cmd"' >/dev/null 2>&1 || { echo "'"${cmd%% *}"' exited non-zero"; exit 5; }
        if [ '"$id"' = 10-thinlinc ]; then
            cmp -s /usr/libexec/thinfloe/tlclient-pulseaudio /opt/thinlinc/lib/tlclient/pulseaudio \
                || { echo "pulseaudio is not the PipeWire shim after the hook"; exit 7; }
        fi
        exit 0' 2>&1)
    if [[ $? -eq 0 ]]; then
        ok "$label: every library resolves and the client starts"
    else
        bad "$label:"
        echo "$out" | head -20
    fi
}

deb="${THINLINC_DEB:-}"
if [[ -z "$deb" || ! -s "$deb" ]]; then
    deb=$(fetch_thinlinc) || deb=""
fi
if [[ -z "$deb" ]]; then
    bad "ThinLinc: THINLINC_DEB not set and the download from the tile's page failed"
else
    check_addon ThinLinc "$deb" thinlinc-client "/opt/thinlinc/bin/tlclient --version" 10-thinlinc
fi

if [[ -n "${OXULNK_DEB:-}" ]]; then
    check_addon OxuLnk "$OXULNK_DEB" oxulnk-desktop "oxulnk-desktop --version" 20-oxulnk
else
    echo "skip - OxuLnk: OXULNK_DEB not set"
fi

[[ $fail -eq 0 ]] || { echo "FAILED - add-on dependencies"; exit 1; }
echo "PASSED - add-on dependencies"
