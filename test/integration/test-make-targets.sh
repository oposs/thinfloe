#!/bin/bash
# Offline tests for the shape of the test targets in the Makefile.
#
# CI's image job runs `make test-integration`; the unit tests have a job of
# their own, so the image job must not run them a second time. `make test`
# still runs both.
#
# Runs `make -n` on the real Makefile in a scratch tree with a stub container
# runtime; nothing is built.
#
# Usage: test/integration/test-make-targets.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=test/integration/lib/scratch-tree.sh
source "$REPO/test/integration/lib/scratch-tree.sh"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

RUNTIME="$WORK/podman"
cat >"$RUNTIME" <<'STUB'
#!/bin/bash
case "$1 ${2:-}" in
    "image inspect") echo sha256:aaaa ;;
    *) exit 125 ;;
esac
STUB
chmod +x "$RUNTIME"

TREE="$WORK/tree"
mkdir -p "$TREE/containers/image-builder" "$TREE/containers/ubuntu/patches" "$TREE/containers/ubuntu/slim" \
    "$TREE/src/thinfloe-menu/src" "$TREE/artifacts/rootfs" "$TREE/artifacts/images" "$TREE/configs" "$TREE/scripts/lib"
cp "$REPO/Makefile" "$TREE/Makefile"
tf_guard_tree "$TREE" "$WORK"
inputs=(RELEASE containers/ubuntu/Dockerfile containers/image-builder/Dockerfile src/thinfloe-menu/src/main.rs
        configs/syslinux.cfg scripts/make-bootable-image.sh scripts/export-rootfs.sh scripts/lib/common.sh)
for f in "${inputs[@]}"; do : >"$TREE/$f"; done
echo 4.2.0 >"$TREE/RELEASE"

# Prints what `make -n <goal> [VAR=val...]` would run in the scratch tree.
dry() {
    env -u MAKEFLAGS -u MAKELEVEL make -n -C "$TREE" CONTAINER_RUNTIME="$RUNTIME" \
        TF_VERSION=4.2.0-dev+abc "$@" 2>&1
}

out=$(dry test-integration)
if grep -q 'test-firmware.sh' <<<"$out" && ! grep -qE 'unittest|cargo test' <<<"$out"; then
    ok "test-integration runs no unit tests"
else
    bad "test-integration runs no unit tests"
fi

out=$(dry test)
if grep -q 'unittest' <<<"$out" && grep -q 'cargo test' <<<"$out" && grep -q 'test-firmware.sh' <<<"$out"; then
    ok "make test still runs the unit and the integration tests"
else
    bad "make test still runs the unit and the integration tests"
fi

exit $fail
