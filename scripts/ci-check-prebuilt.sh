#!/bin/bash
# Guard for the smoke jobs of ci-local.yml: they run on files the image job
# built, and must not rebuild anything.
#
# Usage: scripts/ci-check-prebuilt.sh record|verify
#
# record  writes artifacts/prebuilt.sha256, the sha256 of the five files the
#         smoke targets read (paths relative to the checkout).
# verify  fails (::error:: lines) when the manifest is missing, a file differs
#         from it, or a thinfloe-ubuntu container image exists on the runner,
#         which means a rule rebuilt the rootfs instead of using the download.
#
# TF_CHECKOUT overrides the checkout (tests); CONTAINER_RUNTIME defaults to
# podman.
set -euo pipefail

MODE=${1:-}
ROOT=${TF_CHECKOUT:-$(cd "$(dirname "$0")/.." && pwd)}
MANIFEST=artifacts/prebuilt.sha256
FILES=(
    artifacts/rootfs/vmlinuz
    artifacts/rootfs/initrd.img
    artifacts/rootfs/ramroot.sqsh
    artifacts/images/thinfloe-usb.img.zst
    artifacts/images/thinfloe-usb.img.zst.sha256
)

cd "$ROOT"
case "$MODE" in
record)
    sha256sum "${FILES[@]}" > "$MANIFEST"
    ;;
verify)
    status=0
    if [[ ! -f $MANIFEST ]]; then
        echo "::error::$MANIFEST is missing; the smoke job did not get the image job's files"
        exit 1
    fi
    if ! out=$(sha256sum -c "$MANIFEST" 2>&1); then
        echo "$out"
        echo "::error::a prebuilt file differs from the manifest of the image job"
        status=1
    fi
    images=$("${CONTAINER_RUNTIME:-podman}" images -q --filter reference='thinfloe-ubuntu*')
    if [[ -n $images ]]; then
        echo "::error::a thinfloe-ubuntu container image exists; the smoke job rebuilt the rootfs"
        status=1
    fi
    exit $status
    ;;
*)
    echo "Usage: $0 record|verify" >&2
    exit 1
    ;;
esac
