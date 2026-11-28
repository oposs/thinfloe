#!/bin/bash
# The image's thinfloe-menu and thinfloe-share were built from this
# checkout's sources: each binary holds the TF_SOURCE_KEY the Dockerfile
# computed over the sources it copied, and that key matches the checkout.
#
# The cargo target directories sit in a cache mount that every checkout on a
# host shares, and cargo judges a local crate fresh by mtime. Sources from
# one worktree, older than a binary another worktree had built, were taken as
# built: the image carried the other worktree's menu and the gates passed on it.
#
# Usage: test/integration/test-source-key.sh [path/to/ramroot.sqsh]

set -uo pipefail

SQSH="${1:-artifacts/rootfs/ramroot.sqsh}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=test/integration/lib/source-key.sh
source "$REPO/test/integration/lib/source-key.sh"

if [[ ! -f "$SQSH" ]]; then
    echo "FAIL: $SQSH not found - run 'make rootfs' first" >&2
    exit 1
fi
if ! command -v unsquashfs >/dev/null 2>&1; then
    echo "FAIL: unsquashfs not installed (apt install squashfs-tools)" >&2
    exit 1
fi

# unsquashfs starts a thread per core; the build box is shared.
unsquashfs() { command unsquashfs -processors "${BUILD_JOBS:-4}" "$@"; }

WORK=$(mktemp -d)
trap 'chmod -R u+rwX "$WORK" 2>/dev/null; rm -rf "$WORK"' EXIT
ROOT="$WORK/root"
unsquashfs -d "$ROOT" -f "$SQSH" usr/bin/thinfloe-menu usr/bin/thinfloe-share >/dev/null 2>&1

FAILED=0
check() {
    local bin=$1 key=$2
    shift 2
    if [[ ! -f "$ROOT/usr/bin/$bin" ]]; then
        echo "FAIL: /usr/bin/$bin is not in the image" >&2
        FAILED=1
    elif ! grep -qaF "$key" "$ROOT/usr/bin/$bin"; then
        echo "FAIL: /usr/bin/$bin was not built from this checkout ($*): its source key is not $key" >&2
        FAILED=1
    else
        echo "ok   - /usr/bin/$bin holds the source key of $*"
    fi
}

check thinfloe-menu "$(tf_source_key "$REPO" src/thinfloe-menu overlay/home/tfuser/.config/thinfloe)" \
    src/thinfloe-menu overlay/home/tfuser/.config/thinfloe
check thinfloe-share "$(tf_source_key "$REPO" src/thinfloe-share)" src/thinfloe-share

exit "$FAILED"
