#!/bin/bash
# Offline tests for the prerequisites of the USB image.
#
# thinfloe-usb.img used to depend on the phony target `rootfs`, so every target
# that needs it (image, qemu-smoke, qemu-test) built it again. The new build
# gets new random GPT GUIDs, and the .zst made by `make image` then no longer
# matched the raw image next to it. The image now depends on the files it is
# built from, and must still be rebuilt when any of them changes.
#
# Runs the real Makefile in a scratch tree with a stub container runtime and
# asks `make -q` whether the image is up to date.
#
# Usage: test/integration/test-usb-image-deps.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

# Every image the Makefile inspects reports the same ID, so both container
# stamps below stay valid.
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
mkdir -p "$TREE/containers/image-builder" "$TREE/containers/ubuntu/patches" "$TREE/src/thinfloe-menu/src" \
    "$TREE/artifacts/rootfs" "$TREE/artifacts/images" "$TREE/configs" "$TREE/scripts/lib"
cp "$REPO/Makefile" "$TREE/Makefile"
inputs=(containers/ubuntu/Dockerfile containers/image-builder/Dockerfile src/thinfloe-menu/src/main.rs
        configs/syslinux.cfg scripts/make-bootable-image.sh scripts/lib/common.sh)
for f in "${inputs[@]}"; do : >"$TREE/$f"; done
(cd "$TREE" && touch -d '2020-01-01' "${inputs[@]}" containers/ubuntu/patches \
    src/thinfloe-menu/src src/thinfloe-menu)
echo sha256:aaaa >"$TREE/.ubuntu-container"
echo sha256:aaaa >"$TREE/.image-builder-container"
touch -d '2021-01-01' "$TREE/.ubuntu-container" "$TREE/.image-builder-container"
for f in vmlinuz initrd.img ramroot.sqsh; do : >"$TREE/artifacts/rootfs/$f"; done
touch -d '2022-01-01' "$TREE"/artifacts/rootfs/*
USB="artifacts/images/thinfloe-usb.img"

# Prints "up-to-date" when make would not rebuild the image, "rebuild" otherwise.
state() {
    local was=""
    touch -d '2023-01-01' "$TREE/$USB"
    [[ -n "${1:-}" ]] && was=$(stat -c %Y "$TREE/$1") && touch -d '2024-01-01' "$TREE/$1"
    if make -q -C "$TREE" CONTAINER_RUNTIME="$RUNTIME" "$USB" >/dev/null 2>&1; then
        echo up-to-date
    else
        echo rebuild
    fi
    [[ -n "$was" ]] && touch -d "@$was" "$TREE/$1"
}

r=$(state)
if [[ "$r" == up-to-date ]]; then
    ok "an image newer than all its inputs is not built again"
else
    bad "the image is rebuilt although no input changed"
fi

for input in artifacts/rootfs/vmlinuz artifacts/rootfs/initrd.img \
             artifacts/rootfs/ramroot.sqsh configs/syslinux.cfg \
             scripts/make-bootable-image.sh scripts/lib/common.sh; do
    r=$(state "$input")
    if [[ "$r" == rebuild ]]; then
        ok "a newer $input rebuilds the image"
    else
        bad "a newer $input left the image standing"
    fi
done

exit $fail
