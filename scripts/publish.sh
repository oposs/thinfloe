#!/bin/bash
# Publish ThinFloe artifacts to web server
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
ARTIFACT_DIR="$PROJECT_DIR/artifacts"

TARGET="oetiker@web-volki-01-adm:public_html/dbrrg/"

# Files to publish
FILES=(
    "$ARTIFACT_DIR/rootfs/vmlinuz"
    "$ARTIFACT_DIR/rootfs/initrd.img"
    "$ARTIFACT_DIR/rootfs/ramroot.sqsh"
    "$ARTIFACT_DIR/images/thinfloe-usb.img.zst"
    "$ARTIFACT_DIR/images/thinfloe-usb.img.zst.sha256"
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
rsync -avP "${FILES[@]}" "$TARGET"
echo "Done."
