#!/bin/bash
# Offline tests for the Makefile's container stamps.
#
# .ubuntu-container is per checkout, but the image tag thinfloe-ubuntu:$(VERSION)
# is shared by every checkout on the machine. The stamp used to be a bare
# `touch`, dropped only when the tag did not exist at all, so after another
# checkout built and re-tagged the image this checkout still called its build
# up to date, and `make test` checked the other checkout's image and passed.
# The stamp now holds the image ID, and it is dropped whenever the tag names
# a different image. Its second line is a hash of the inputs, so a
# `touch .ubuntu-container` after an edit no longer hides the edit.
#
# Runs the real Makefile in a scratch tree with a stub container runtime: the
# stub's "build" changes nothing, and "image inspect" reports the ID in
# $TF_TEST_IMAGE_ID (none when it is empty, i.e. no such image).
#
# Usage: test/integration/test-container-stamp.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

RUNTIME="$WORK/podman"
cat >"$RUNTIME" <<'STUB'
#!/bin/bash
echo "podman $*" >>"$TF_TEST_RUNTIME_LOG"
case "$1 ${2:-}" in
    "image inspect")
        [ -n "${TF_TEST_IMAGE_ID:-}" ] || exit 125
        echo "$TF_TEST_IMAGE_ID" ;;
    "build "*) ;;
    *) exit 125 ;;
esac
STUB
chmod +x "$RUNTIME"

# Just enough of a checkout for the .ubuntu-container rule: its inputs, all
# older than the stamp, so only the stamp's content can make it stale.
TREE="$WORK/tree"
mkdir -p "$TREE/containers/ubuntu/patches" "$TREE/containers/ubuntu/slim" "$TREE/artifacts/rootfs" \
    "$TREE/src/thinfloe-menu/src"
cp "$REPO/Makefile" "$TREE/Makefile"
: >"$TREE/containers/ubuntu/Dockerfile"
: >"$TREE/src/thinfloe-menu/src/main.rs"
echo 4.0.0 >"$TREE/RELEASE"
touch -d '2020-01-01' "$TREE/RELEASE" "$TREE/containers/ubuntu/Dockerfile" \
    "$TREE/containers/ubuntu/patches" "$TREE/containers/ubuntu/slim" \
    "$TREE/src/thinfloe-menu/src/main.rs" "$TREE/src/thinfloe-menu/src" "$TREE/src/thinfloe-menu"

STAMP="$TREE/.ubuntu-container"

# $1 = image ID the tag currently names ("" for no image). Prints "built" if
# make ran the container build, "up-to-date" otherwise. With no image the
# stub's build produces none either, so make itself fails after the build;
# only whether it built matters here.
run_make() {
    : >"$WORK/runtime.log"
    TF_TEST_RUNTIME_LOG="$WORK/runtime.log" TF_TEST_IMAGE_ID="$1" \
        make -s -C "$TREE" CONTAINER_RUNTIME="$RUNTIME" TF_VERSION=4.0.0-dev+abc .ubuntu-container \
        >"$WORK/make.out" 2>&1
    if grep -q '^podman build ' "$WORK/runtime.log"; then
        echo built
    else
        echo up-to-date
    fi
}

# ---------------------------------------------------------------- test 1
rm -f "$STAMP"
r=$(run_make sha256:aaaa)
if [[ "$r" == built ]] && [[ "$(sed -n 1p "$STAMP" 2>/dev/null)" == sha256:aaaa ]]; then
    ok "a build records the tagged image's ID in the stamp"
else
    bad "build gave '$r', stamp holds '$(sed -n 1p "$STAMP" 2>/dev/null)': $(cat "$WORK/make.out")"
fi

# ---------------------------------------------------------------- test 2
r=$(run_make sha256:aaaa)
if [[ "$r" == up-to-date ]]; then
    ok "the stamp stands while the tag names the image it recorded"
else
    bad "rebuilt although the tag still names the recorded image ($r)"
fi

# ---------------------------------------------------------------- test 3
# Another checkout rebuilt thinfloe-ubuntu:$(VERSION) from its own sources.
r=$(run_make sha256:bbbb)
if [[ "$r" == built ]] && [[ "$(sed -n 1p "$STAMP" 2>/dev/null)" == sha256:bbbb ]]; then
    ok "a different image under the same tag invalidates the stamp"
else
    bad "a foreign image under the tag left the stamp standing ($r)"
fi

# ---------------------------------------------------------------- test 4
r=$(run_make "")
if [[ "$r" == built ]]; then
    ok "a missing image invalidates the stamp"
else
    bad "a missing image left the stamp standing ($r)"
fi

# ---------------------------------------------------------------- test 5
# A stamp written by the old `touch` is empty and must not match anything.
: >"$STAMP"
r=$(run_make sha256:aaaa)
if [[ "$r" == built ]]; then
    ok "an empty stamp from before the ID check is rebuilt"
else
    bad "an empty stamp was taken as up to date ($r)"
fi

# ---------------------------------------------------------------- test 5b
# `touch .ubuntu-container` after an edit made the stamp newer than the
# edited input, and make took the old image as built from it. The stamp
# holds a hash of the inputs, so an edit is seen whatever the mtimes.
rm -f "$STAMP"
run_make sha256:aaaa >/dev/null
echo "FROM scratch" >"$TREE/containers/ubuntu/Dockerfile"
touch -d '2020-01-01' "$TREE/containers/ubuntu/Dockerfile"
touch "$STAMP"
r=$(run_make sha256:aaaa)
if [[ "$r" == built ]]; then
    ok "an edited input rebuilds, also under a touched stamp"
else
    bad "a touched stamp hid an edited input ($r)"
fi
chmod +x "$TREE/containers/ubuntu/Dockerfile"
touch -d '2020-01-01' "$TREE/containers/ubuntu/Dockerfile"
r=$(run_make sha256:aaaa)
if [[ "$r" == built ]]; then
    ok "a changed file mode rebuilds"
else
    bad "a changed file mode left the stamp standing ($r)"
fi
r=$(run_make sha256:aaaa)
if [[ "$r" == up-to-date ]]; then
    ok "the stamp of that build stands"
else
    bad "the build after the mode change rebuilt again ($r)"
fi

# ---------------------------------------------------------------- test 6
# thinfloe-menu is compiled inside the container build from src/thinfloe-menu, which
# OVERLAY_FILES does not cover. Editing it must make the stamp stale, or
# 'make test' checks a binary built from the old source.
rm -f "$STAMP"
run_make sha256:aaaa >/dev/null
r=$(run_make sha256:aaaa)
if [[ "$r" != up-to-date ]]; then
    bad "setup: stamp not up to date before touching the crate ($r)"
else
    touch "$TREE/src/thinfloe-menu/src/main.rs"
    r=$(run_make sha256:aaaa)
    if [[ "$r" == built ]]; then
        ok "editing a thinfloe-menu source file invalidates the stamp"
    else
        bad "a thinfloe-menu edit left the stamp standing ($r)"
    fi
fi

exit $fail
