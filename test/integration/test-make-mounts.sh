#!/bin/bash
# Offline test: the containers mount the checkout make runs in.
#
# The export, iPXE and image recipes mounted $(PWD)/artifacts, scripts and
# configs. make does not change PWD for -C, so `make -C other rootfs` mounted
# the caller's directories: an offline test that ran make -C on a scratch tree
# wrote its stub files into the real shared artifacts directory
# (2026-11-19). The recipes use $(CURDIR) now.
#
# Uses make -n only, from another directory: nothing is built or written.
#
# Usage: test/integration/test-make-mounts.sh [Makefile]

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=test/integration/lib/scratch-tree.sh
source "$REPO/test/integration/lib/scratch-tree.sh"
MAKEFILE="${1:-$REPO/Makefile}"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

RUNTIME="$WORK/podman"
printf '#!/bin/sh\nexit 125\n' >"$RUNTIME"
chmod +x "$RUNTIME"

TREE="$WORK/tree"
mkdir -p "$TREE/artifacts/rootfs" "$TREE/artifacts/images"
cp "$MAKEFILE" "$TREE/Makefile"
echo 4.2.0 >"$TREE/RELEASE"
for f in containers/ubuntu/Dockerfile containers/ipxe/Dockerfile containers/image-builder/Dockerfile \
         src/thinfloe-menu/src/main.rs scripts/export-rootfs.sh scripts/lib/common.sh \
         scripts/make-bootable-image.sh configs/syslinux.cfg; do
    mkdir -p "$TREE/${f%/*}"
    : >"$TREE/$f"
done
mkdir -p "$TREE/containers/ubuntu/patches" "$TREE/containers/ubuntu/slim"
tf_guard_tree "$TREE" "$WORK"

out=$(cd "$WORK" && env -u MAKEFLAGS -u MAKELEVEL -u TF_VERSION make -n -C "$TREE" \
    CONTAINER_RUNTIME="$RUNTIME" TF_VERSION=4.2.0-dev+abc \
    rootfs ipxe artifacts/images/thinfloe-usb.img 2>/dev/null)
mounts=$(grep -o -- '-v [^: ]*:' <<<"$out" | sed 's/^-v //; s/:$//' | sort -u)
if [[ -z "$mounts" ]]; then
    bad "make -n showed no container mounts: $out"
fi
while IFS= read -r m; do
    [[ -n $m ]] || continue
    if [[ "$m" == "$TREE"/* ]]; then
        ok "mount $m is in the checkout make runs in"
    else
        bad "mount $m is outside the checkout ($TREE), though make -C named it"
    fi
done <<<"$mounts"

exit $fail
