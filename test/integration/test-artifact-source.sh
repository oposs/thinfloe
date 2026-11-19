#!/bin/bash
# Offline tests: the shared artifacts directory is tied to this checkout.
#
# `artifacts` is a symlink in git, so every checkout on the development host
# writes into one directory. Make only compared mtimes: after another
# checkout exported its rootfs, the files were newer than this checkout's
# .ubuntu-container, make called them up to date, and `make test` checked
# the other checkout's rootfs and passed. Each export (rootfs, iPXE loaders,
# USB image) now records a key of what it was made from, and a key that
# differs from this checkout's makes the files stale.
#
# Runs the real Makefile in two scratch checkouts whose artifacts symlinks
# name one directory, with a stub container runtime: "build" succeeds,
# "image inspect" gives sha256:<image>-$TF_TEST_ID, "run" writes the files
# the real container would write.
#
# Usage: test/integration/test-artifact-source.sh

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
    "image inspect")
        name=${*: -1}
        echo "sha256:${name%%:*}-$TF_TEST_ID" ;;
    "build "*) ;;
    "run "*)
        out=""
        for a in "$@"; do
            case "$a" in *:/artifacts) out=${a%:/artifacts} ;; esac
        done
        image=""
        for a in "$@"; do
            case "$a" in thinfloe-*:*) image=$a ;; esac
        done
        # Never write outside the scratch area: a mount of the caller's
        # directory would be the real checkout's shared artifacts.
        case "$out" in
            "$TF_TEST_WORK"/*) ;;
            *) echo "run $image into $out" >>"$TF_TEST_RUNTIME_LOG"; exit 0 ;;
        esac
        echo "run $image" >>"$TF_TEST_RUNTIME_LOG"
        case "$image" in
            thinfloe-ubuntu:*)
                for f in vmlinuz initrd.img ramroot.sqsh; do echo "$image" >"$out/rootfs/$f"; done ;;
            thinfloe-ipxe:*)
                for f in ipxe.pxe undionly.kpxe ipxe.efi; do echo "$image" >"$out/$f"; done ;;
            thinfloe-image-builder:*)
                echo "$image" >"$out/images/thinfloe-usb.img" ;;
        esac ;;
    *) exit 125 ;;
esac
STUB
chmod +x "$RUNTIME"

SHARED="$WORK/shared"
mkdir -p "$SHARED/rootfs" "$SHARED/images"

# A checkout with every input the build rules name, all older than anything
# a build writes.
checkout() {
    local t="$WORK/$1" f
    mkdir -p "$t/containers/ubuntu/patches" "$t/containers/ubuntu/slim" "$t/containers/ipxe" \
        "$t/containers/image-builder" "$t/src/thinfloe-menu/src" "$t/scripts/lib" "$t/configs"
    cp "$REPO/Makefile" "$t/Makefile"
    # A link, as in a real checkout, but to a directory inside $WORK.
    ln -s "$SHARED" "$t/artifacts"
    tf_guard_tree "$t" "$WORK"
    echo 4.2.0 >"$t/RELEASE"
    for f in containers/ubuntu/Dockerfile containers/ipxe/Dockerfile containers/image-builder/Dockerfile \
             src/thinfloe-menu/src/main.rs scripts/export-rootfs.sh scripts/lib/common.sh \
             scripts/make-bootable-image.sh configs/syslinux.cfg; do
        echo "$f" >"$t/$f"
    done
    (cd "$t" && find RELEASE containers src scripts configs -exec touch -d '2020-01-01' {} +)
}
checkout a
checkout b

# $1 = checkout, $2 = image ID suffix, $3 = target. Prints what the runtime
# ran, space separated ("" when make did nothing).
run_make() {
    : >"$WORK/runtime.log"
    TF_TEST_WORK="$WORK" TF_TEST_RUNTIME_LOG="$WORK/runtime.log" TF_TEST_ID="$2" \
        env -u MAKEFLAGS -u MAKELEVEL -u TF_VERSION make -s -C "$WORK/$1" CONTAINER_RUNTIME="$RUNTIME" \
        VERSION="$1" TF_VERSION=4.2.0-dev+abc "$3" >"$WORK/make.out" 2>&1 \
        || echo "make failed: $(cat "$WORK/make.out")" >>"$WORK/runtime.log"
    sed 's/^run //' "$WORK/runtime.log" | tr '\n' ' ' | sed 's/ $//'
}

# ---------------------------------------------------------------- rootfs
# make -C from another directory: the containers must mount the checkout's
# artifacts, not those of the directory make was called from ($PWD).
r=$(run_make a a rootfs)
if [[ "$r" == "thinfloe-ubuntu:a" ]]; then
    ok "make -C mounts the checkout's own artifacts directory"
else
    bad "make -C ran '$r'"
fi
rm -f "$SHARED"/rootfs/* "$WORK"/a/.*-container
r=$(run_make a a rootfs)
if [[ "$r" == "thinfloe-ubuntu:a" ]]; then
    ok "a first build exports the rootfs"
else
    bad "a first build ran '$r'"
fi
r=$(run_make a a rootfs)
if [[ -z "$r" ]]; then
    ok "a second build of the same checkout exports nothing"
else
    bad "a second build ran '$r'"
fi
r=$(run_make b b rootfs)
if [[ "$r" == "thinfloe-ubuntu:b" ]]; then
    ok "another checkout's build exports its own rootfs"
else
    bad "another checkout's build ran '$r'"
fi
r=$(run_make a a rootfs)
if [[ "$r" == "thinfloe-ubuntu:a" ]] && grep -qx thinfloe-ubuntu:a "$SHARED/rootfs/ramroot.sqsh"; then
    ok "a rootfs another checkout exported is exported again"
else
    bad "the other checkout's rootfs was taken as this one's (ran '$r')"
fi
echo changed >>"$WORK/a/scripts/export-rootfs.sh"
touch -d '2020-01-01' "$WORK/a/scripts/export-rootfs.sh"
r=$(run_make a a rootfs)
if [[ "$r" == "thinfloe-ubuntu:a" ]]; then
    ok "a changed export script exports again, whatever its mtime"
else
    bad "a changed export script with an old mtime exported nothing (ran '$r')"
fi
r=$(run_make a a rootfs)
if [[ -z "$r" ]]; then
    ok "the export after it is current"
else
    bad "the build after the export ran '$r'"
fi

# ---------------------------------------------------------------- iPXE
run_make a a ipxe >/dev/null
r=$(run_make b b ipxe)
if [[ "$r" == "thinfloe-ipxe:b" ]]; then
    ok "another checkout's iPXE build exports its own loaders"
else
    bad "another checkout's iPXE build ran '$r'"
fi
r=$(run_make a a ipxe)
if [[ "$r" == "thinfloe-ipxe:a" ]]; then
    ok "iPXE loaders another checkout exported are exported again"
else
    bad "the other checkout's iPXE loaders were taken as this one's (ran '$r')"
fi

# ---------------------------------------------------------------- USB image
# b now builds from the same images as a, so the rootfs is the same; only
# its boot menu differs.
USB=artifacts/images/thinfloe-usb.img
echo "b's menu" >"$WORK/b/configs/syslinux.cfg"
touch -d '2020-01-01' "$WORK/b/configs/syslinux.cfg"
cp "$WORK/a/scripts/export-rootfs.sh" "$WORK/b/scripts/export-rootfs.sh"
r=$(run_make a a "$USB")
if [[ "$r" == "thinfloe-image-builder:a" ]]; then
    ok "a first image build builds the USB image"
else
    bad "a first image build ran '$r'"
fi
r=$(run_make b a "$USB")
if [[ "$r" == "thinfloe-ubuntu:b thinfloe-image-builder:b" ]]; then
    ok "another checkout exports its rootfs and builds its own image"
else
    bad "another checkout's image build ran '$r'"
fi
r=$(run_make a a "$USB")
if [[ "$r" == *"thinfloe-image-builder:a" ]]; then
    ok "a USB image another checkout built is built again"
else
    bad "the other checkout's USB image was taken as this one's (ran '$r')"
fi
r=$(run_make a a "$USB")
if [[ -z "$r" ]]; then
    ok "the image after it is current"
else
    bad "the build after the image ran '$r'"
fi

exit $fail
