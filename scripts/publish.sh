#!/bin/bash
# Mirror one downloaded GitHub release to tobi.oetiker.ch, the one-time
# bridge for sticks that still fetch from there.
#
# Usage: scripts/publish.sh <release-dir>
#
# <release-dir> holds the six assets of a GitHub release. They go up
# unchanged; only the three files the old sticks read are generated.
set -e

TARGET="oetiker@web-volki-01-adm:public_html/dbrrg/"
ASSETS=(vmlinuz initrd.img ramroot.sqsh thinfloe-usb.img.zst version SHA256SUMS)

if [[ $# != 1 || ! -d $1 ]]; then
    echo "Usage: $0 <release-dir>" >&2
    echo "The directory holds the six assets of one GitHub release." >&2
    exit 1
fi
REL=$(cd "$1" && pwd)

for f in "${ASSETS[@]}"; do
    if [[ ! -f "$REL/$f" ]]; then
        echo "ERROR: Missing file: $REL/$f" >&2
        exit 1
    fi
done

# The files must be the ones the release published.
if ! (cd "$REL" && sha256sum -c SHA256SUMS >/dev/null); then
    echo "ERROR: sha256sum -c SHA256SUMS failed in $REL" >&2
    exit 1
fi

# The release number thinfloe-upgrade check reads: one line X.Y.Z.
if [[ $(wc -l <"$REL/version") != 1 ]] || ! grep -qxE '[0-9]+\.[0-9]+\.[0-9]+' "$REL/version"; then
    echo "ERROR: $REL/version is not one line X.Y.Z" >&2
    exit 1
fi

# Files made here live in a work directory, never among the assets.
# 4.0 to 4.2 sticks read thinfloe-usb.img.zst.sha256. 3.x sticks fetch only
# the old name dbrrg-usb.img.zst: a hard link keeps it from costing space
# here, and rsync -H below keeps it a hard link on the server, so it is
# neither sent nor stored twice. 3.x reads no checksum; the .sha256 names
# the old file for whoever verifies a download of it by hand.
WORK_DIR="$REL/publish-work"
mkdir -p "$WORK_DIR"
ln -f "$REL/thinfloe-usb.img.zst" "$WORK_DIR/thinfloe-usb.img.zst"
ln -f "$REL/thinfloe-usb.img.zst" "$WORK_DIR/dbrrg-usb.img.zst"
(cd "$WORK_DIR" && sha256sum thinfloe-usb.img.zst >thinfloe-usb.img.zst.sha256 \
    && sha256sum dbrrg-usb.img.zst >dbrrg-usb.img.zst.sha256)

FILES=()
for f in "${ASSETS[@]}"; do
    FILES+=("$REL/$f")
done
FILES+=(
    "$WORK_DIR/thinfloe-usb.img.zst.sha256"
    "$WORK_DIR/dbrrg-usb.img.zst"
    "$WORK_DIR/dbrrg-usb.img.zst.sha256"
)

echo "Publishing to $TARGET..."
rsync -avPH "${FILES[@]}" "$TARGET"
echo "Done."
