#!/bin/bash
# Offline test for scripts/ci-check-prebuilt.sh: the smoke jobs of ci-local.yml
# verify the files they downloaded against the manifest the image job
# recorded, and fail when a container image shows up on the runner.
#
# Usage: test/integration/test-ci-check-prebuilt.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$REPO/scripts/ci-check-prebuilt.sh"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

# A checkout in the scratch dir; the script never sees the real artifacts.
TREE="$WORK/tree"
mkdir -p "$WORK/bin"
cat > "$WORK/bin/podman-none" <<'EOF'
#!/bin/bash
exit 0
EOF
cat > "$WORK/bin/podman-image" <<'EOF'
#!/bin/bash
echo 0123456789ab
EOF
chmod +x "$WORK/bin/podman-none" "$WORK/bin/podman-image"

make_tree() {
    rm -rf "$TREE"
    mkdir -p "$TREE/artifacts/rootfs" "$TREE/artifacts/images"
    local f
    for f in rootfs/vmlinuz rootfs/initrd.img rootfs/ramroot.sqsh \
             images/thinfloe-usb.img.zst images/thinfloe-usb.img.zst.sha256; do
        echo "content of $f" > "$TREE/artifacts/$f"
    done
}

run() {  # run <container-runtime> <mode>
    TF_CHECKOUT="$TREE" CONTAINER_RUNTIME="$1" "$SCRIPT" "$2" >"$WORK/out" 2>&1
}

make_tree
if run "$WORK/bin/podman-none" record && run "$WORK/bin/podman-none" verify; then
    ok "record, then verify, passes"
else
    bad "record, then verify, passes"; cat "$WORK/out"
fi

echo changed > "$TREE/artifacts/rootfs/ramroot.sqsh"
if run "$WORK/bin/podman-none" verify; then
    bad "a changed ramroot.sqsh fails verify"
elif grep -q '^::error::' "$WORK/out"; then
    ok "a changed ramroot.sqsh fails verify"
else
    bad "a changed ramroot.sqsh fails verify without ::error::"; cat "$WORK/out"
fi

make_tree
if run "$WORK/bin/podman-none" verify; then
    bad "a missing manifest fails verify"
elif grep -q '^::error::' "$WORK/out"; then
    ok "a missing manifest fails verify"
else
    bad "a missing manifest fails verify without ::error::"; cat "$WORK/out"
fi

make_tree
run "$WORK/bin/podman-none" record
if run "$WORK/bin/podman-image" verify; then
    bad "a container image on the runner fails verify"
elif grep -q '^::error::' "$WORK/out"; then
    ok "a container image on the runner fails verify"
else
    bad "a container image on the runner fails verify without ::error::"; cat "$WORK/out"
fi

exit $fail
