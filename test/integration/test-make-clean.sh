#!/bin/bash
# Offline tests for `make clean`.
#
# `rm -rf artifacts/*` follows a symlinked artifacts directory and empties its
# target, which may hold other checkouts' builds. clean leaves a link target
# alone unless FORCE=1 and still exits 0, and always removes this checkout's
# container stamps. A link whose target is gone stops make with its cause.
#
# Runs the real Makefile in a scratch tree, never in this checkout. The
# container runtime is a stub that knows no images.
#
# Usage: test/integration/test-make-clean.sh

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
printf '#!/bin/sh\nexit 125\n' >"$RUNTIME"
chmod +x "$RUNTIME"

TREE="$WORK/tree"
SHARED="$WORK/shared-artifacts"

# $1 = "link" (artifacts is a symlink to $SHARED) or "dir" (a real directory)
setup() {
    rm -rf "$TREE" "$SHARED"
    mkdir -p "$TREE" "$SHARED/rootfs"
    cp "$REPO/Makefile" "$TREE/Makefile"
    echo 4.2.0 >"$TREE/RELEASE"
    : >"$SHARED/rootfs/ramroot.sqsh"
    if [[ "$1" == link ]]; then
        ln -s "$SHARED" "$TREE/artifacts"  # a link, but into $WORK
    else
        cp -r "$SHARED" "$TREE/artifacts"
    fi
    : >"$TREE/.ubuntu-container"
    : >"$TREE/.image-builder-container"
    tf_guard_tree "$TREE" "$WORK"
}

run_clean() {
    make -s -C "$TREE" CONTAINER_RUNTIME="$RUNTIME" TF_VERSION=4.2.0-dev+abc clean "$@" >"$WORK/out" 2>&1
    echo $?
}

stamps_gone() {
    [[ ! -e "$TREE/.ubuntu-container" ]] && [[ ! -e "$TREE/.image-builder-container" ]]
}

# ---------------------------------------------------------------- test 1
setup link
rc=$(run_clean)
if [[ "$rc" == 0 ]] && [[ -e "$SHARED/rootfs/ramroot.sqsh" ]] &&
   grep -qF "$SHARED" "$WORK/out" && grep -qF 'make clean FORCE=1' "$WORK/out"; then
    ok "a symlinked artifacts directory is left alone, naming its target and FORCE=1, and clean exits 0"
else
    bad "symlinked artifacts: exit $rc, target $(ls -A "$SHARED"), output: $(cat "$WORK/out")"
fi
if stamps_gone; then
    ok "clean removes the container stamps also when it leaves the link target alone"
else
    bad "clean left the container stamps in place"
fi

# ---------------------------------------------------------------- test 2
setup link
rc=$(run_clean FORCE=1)
if [[ "$rc" == 0 ]] && [[ -z "$(ls -A "$SHARED")" ]] && [[ -L "$TREE/artifacts" ]] && stamps_gone; then
    ok "FORCE=1 empties the link target and keeps the link"
else
    bad "FORCE=1: exit $rc, target $(ls -A "$SHARED"), output: $(cat "$WORK/out")"
fi

# ---------------------------------------------------------------- test 3
setup dir
rc=$(run_clean)
if [[ "$rc" == 0 ]] && [[ -z "$(ls -A "$TREE/artifacts")" ]] && stamps_gone; then
    ok "a real artifacts directory is cleaned without FORCE"
else
    bad "real directory: exit $rc, left $(ls -A "$TREE/artifacts"), output: $(cat "$WORK/out")"
fi

# ---------------------------------------------------------------- test 4
# A link whose target is gone (a checkout copied from another host) stops
# the directory rule with the cause, not with mkdir's "File exists".
setup link
mv "$SHARED" "$WORK/gone"
make -s -C "$TREE" CONTAINER_RUNTIME="$RUNTIME" TF_VERSION=4.2.0-dev+abc artifacts/rootfs >"$WORK/out" 2>&1
rc=$?
if [[ "$rc" != 0 ]] && grep -qF "$SHARED, which does not exist" "$WORK/out" &&
   ! grep -qF 'File exists' "$WORK/out"; then
    ok "a dangling artifacts link stops make with its target named"
else
    bad "dangling link: exit $rc, output: $(cat "$WORK/out")"
fi
rc=$(run_clean)
if [[ "$rc" == 0 ]] && stamps_gone; then
    ok "clean still works on a dangling artifacts link"
else
    bad "clean on a dangling link: exit $rc, output: $(cat "$WORK/out")"
fi

exit $fail
