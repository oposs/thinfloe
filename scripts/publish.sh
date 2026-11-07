#!/bin/bash
# Publish ThinFloe artifacts to web server
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
ARTIFACT_DIR="$PROJECT_DIR/artifacts"

TARGET="oetiker@web-volki-01-adm:public_html/dbrrg/"

# The release number thinfloe-upgrade check reads: one line X.Y.Z, taken
# from the built image by the export. Without it every machine's check fails.
VERSION_FILE="$ARTIFACT_DIR/rootfs/version"
if [[ ! -f "$VERSION_FILE" ]]; then
    echo "ERROR: Missing file: $VERSION_FILE" >&2
    echo "Run 'make' first to build artifacts." >&2
    exit 1
fi
if [[ $(wc -l <"$VERSION_FILE") != 1 ]] || ! grep -qxE '[0-9]+\.[0-9]+\.[0-9]+' "$VERSION_FILE"; then
    echo "ERROR: $VERSION_FILE is not one line X.Y.Z" >&2
    exit 1
fi

# The image is made from the exported rootfs, so it is never older than the
# version file of its own build. A newer version file means the rootfs was
# exported again after the image: they would publish different releases.
IMAGE="$ARTIFACT_DIR/images/thinfloe-usb.img.zst"
if [[ -f "$IMAGE" && "$VERSION_FILE" -nt "$IMAGE" ]]; then
    echo "ERROR: $VERSION_FILE is newer than $IMAGE." >&2
    echo "The rootfs was exported again after the image was made: run 'make image'." >&2
    echo "A plain copy of the artifacts (cp without -p) can also reorder the times;" >&2
    echo "if both come from one build (check: cat $VERSION_FILE), run" >&2
    echo "'touch $IMAGE' and publish again." >&2
    exit 1
fi

# Files to publish
FILES=(
    "$VERSION_FILE"
    "$ARTIFACT_DIR/rootfs/vmlinuz"
    "$ARTIFACT_DIR/rootfs/initrd.img"
    "$ARTIFACT_DIR/rootfs/ramroot.sqsh"
    "$ARTIFACT_DIR/images/thinfloe-usb.img.zst"
    "$ARTIFACT_DIR/images/thinfloe-usb.img.zst.sha256"
)

# 3.x sticks fetch only the old name: publish the same image under it. A
# hard link keeps it from costing space here, and rsync -H below keeps it a
# hard link on the server, so it is neither sent nor stored twice. 3.x's
# upgrade-image reads no checksum; dbrrg-usb.img.zst.sha256 names the old
# file for whoever verifies a download of it by hand.
BRIDGE_DIR="$ARTIFACT_DIR/images/bridge"
if [[ -f "$ARTIFACT_DIR/images/thinfloe-usb.img.zst" ]]; then
    mkdir -p "$BRIDGE_DIR"
    ln -f "$ARTIFACT_DIR/images/thinfloe-usb.img.zst" "$BRIDGE_DIR/dbrrg-usb.img.zst"
    (cd "$BRIDGE_DIR" && sha256sum dbrrg-usb.img.zst > dbrrg-usb.img.zst.sha256)
fi
FILES+=(
    "$BRIDGE_DIR/dbrrg-usb.img.zst"
    "$BRIDGE_DIR/dbrrg-usb.img.zst.sha256"
)

# Check all files exist
for f in "${FILES[@]}"; do
    if [[ ! -f "$f" ]]; then
        echo "ERROR: Missing file: $f" >&2
        echo "Run 'make' first to build artifacts." >&2
        exit 1
    fi
done

echo "Publishing to $TARGET..."
rsync -avPH "${FILES[@]}" "$TARGET"
echo "Done."
