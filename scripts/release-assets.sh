#!/bin/bash
# Collect the six assets of a GitHub release in one directory.
#
# Usage: scripts/release-assets.sh <artifact-dir> <out-dir> <version>
#
# <artifact-dir> is the build output (artifacts/). <version> is the release
# number X.Y.Z, which the rootfs must carry. The assets are hard links of the
# build outputs, plus a SHA256SUMS over the other five that verifies inside
# <out-dir>. Every check runs before the first file is written.
set -euo pipefail

# GitHub refuses a release asset of 2 GiB or more.
MAX_SIZE=2147483648

if [[ $# != 3 ]]; then
    echo "Usage: $0 <artifact-dir> <out-dir> <version>" >&2
    exit 1
fi
ART=$1
OUT=$2
VERSION=$3

# asset name:path inside <artifact-dir>
SOURCES=(
    vmlinuz:rootfs/vmlinuz
    initrd.img:rootfs/initrd.img
    ramroot.sqsh:rootfs/ramroot.sqsh
    thinfloe-usb.img.zst:images/thinfloe-usb.img.zst
    version:rootfs/version
)

if ! grep -qxE '[0-9]+\.[0-9]+\.[0-9]+' <<<"$VERSION"; then
    echo "ERROR: version '$VERSION' is not a release version X.Y.Z" >&2
    exit 1
fi

for s in "${SOURCES[@]}"; do
    if [[ ! -f "$ART/${s#*:}" ]]; then
        echo "ERROR: Missing file: $ART/${s#*:}" >&2
        exit 1
    fi
done

# One line, equal to the version: the same text the image carries.
if [[ $(wc -l <"$ART/rootfs/version") != 1 || $(cat "$ART/rootfs/version") != "$VERSION" ]]; then
    echo "ERROR: $ART/rootfs/version is not $VERSION (one line); rebuild the release" >&2
    exit 1
fi

for s in "${SOURCES[@]}"; do
    size=$(stat -c %s "$ART/${s#*:}")
    if (( size >= MAX_SIZE )); then
        echo "ERROR: ${s%%:*} is $MAX_SIZE bytes or more ($size); GitHub refuses it" >&2
        exit 1
    fi
done

# The out directory may hold the six names, nothing else.
if [[ -d $OUT ]]; then
    while IFS= read -r -d '' f; do
        case ${f##*/} in
            vmlinuz|initrd.img|ramroot.sqsh|thinfloe-usb.img.zst|version|SHA256SUMS) ;;
            *)
                echo "ERROR: $OUT holds $f, which is not a release asset; remove it" >&2
                exit 1
                ;;
        esac
    done < <(find "$OUT" -mindepth 1 -maxdepth 1 -print0)
fi

mkdir -p "$OUT"
for s in "${SOURCES[@]}"; do
    ln -f "$ART/${s#*:}" "$OUT/${s%%:*}"
done
(cd "$OUT" && sha256sum vmlinuz initrd.img ramroot.sqsh thinfloe-usb.img.zst version >SHA256SUMS)
echo "✓ Release assets for $VERSION in $OUT"
