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
    DRY_RC=$?
    return "$DRY_RC"
}
# dry runs in a command substitution (a subshell), so the status travels in
# the output: dry_rc prints "rc=N" as the last line.
dry_rc() {
    dry "$@"
    echo "rc=$DRY_RC"
}

out=$(dry test-integration)
if grep -q 'test-firmware.sh' <<<"$out" && ! grep -qE 'unittest|run-unit-tests|cargo test' <<<"$out"; then
    ok "test-integration runs no unit tests"
else
    bad "test-integration runs no unit tests"
fi

out=$(dry test)
if grep -q 'run-unit-tests.py' <<<"$out" && grep -q 'cargo test' <<<"$out" && grep -q 'test-firmware.sh' <<<"$out"; then
    ok "make test still runs the unit and the integration tests"
else
    bad "make test still runs the unit and the integration tests"
fi

# PREBUILT=1: the smoke targets run on files that are already there (CI smoke
# jobs download them; no container image, no container stamp).
rm -f "$TREE"/.*-container
: >"$TREE/artifacts/rootfs/vmlinuz"; : >"$TREE/artifacts/rootfs/initrd.img"; : >"$TREE/artifacts/rootfs/ramroot.sqsh"
echo x | zstd -q -o "$TREE/artifacts/images/thinfloe-usb.img.zst"
: >"$TREE/artifacts/images/thinfloe-usb.img.zst.sha256"
for goal in qemu-smoke qemu-smoke-addons qemu-smoke-write qemu-smoke-upgrade qemu-smoke-netboot \
            qemu-smoke-two-sticks qemu-smoke-grow qemu-smoke-timezone qemu-smoke-timezone-netboot \
            qemu-smoke-automount; do
    out=$(dry_rc "$goal" PREBUILT=1)
    if [ "$(tail -n1 <<<"$out")" = "rc=0" ] \
        && ! grep -qE 'podman build|export-rootfs\.sh|make-bootable-image\.sh' <<<"$out"; then
        ok "PREBUILT $goal builds nothing"
    else
        bad "PREBUILT $goal builds nothing"
    fi
done

out=$(dry qemu-smoke-upgrade PREBUILT=1)
if grep -q 'zstd -d' <<<"$out"; then
    ok "PREBUILT makes the raw image from the .zst"
else
    bad "PREBUILT makes the raw image from the .zst"
fi

mv "$TREE/artifacts/rootfs/vmlinuz" "$WORK/vmlinuz.keep"
out=$(dry qemu-smoke PREBUILT=1); rc=$?
if [ "$rc" -ne 0 ] && grep -q 'No rule to make target' <<<"$out"; then
    ok "PREBUILT with a missing kernel stops"
else
    bad "PREBUILT with a missing kernel stops"
fi
mv "$WORK/vmlinuz.keep" "$TREE/artifacts/rootfs/vmlinuz"

out=$(dry qemu-smoke)
if grep -q 'export-rootfs.sh' <<<"$out"; then
    ok "without PREBUILT the build rules stand"
else
    bad "without PREBUILT the build rules stand"
fi

out=$(dry_rc test-menu)
if [ "$(tail -n1 <<<"$out")" = "rc=0" ] && grep -q 'cargo fmt --check' <<<"$out" && grep -q 'cargo clippy' <<<"$out" && grep -q 'cargo test' <<<"$out" \
    && ! grep -qE 'podman|export-rootfs\.sh|unittest' <<<"$out"; then
    ok "test-menu needs no image"
else
    bad "test-menu needs no image"
fi

out=$(dry_rc release-assets PREBUILT=1)
if [ "$(tail -n1 <<<"$out")" != "rc=0" ] && grep -q 'PREBUILT=1 is for the smoke targets only' <<<"$out"; then
    ok "PREBUILT with another goal stops"
else
    bad "PREBUILT with another goal stops"
fi

# The smoke targets the Makefile knows and the ones CI's smoke matrix runs
# must be the same set.
make_set=$(sed -n 's/^\.PHONY: //p' "$REPO/Makefile" | tr ' ' '\n' | grep '^qemu-smoke' | sort)
ci_set=$(python3 -I -c '
import sys, yaml
d = yaml.safe_load(open(sys.argv[1]))
t = set()
for e in d["jobs"]["smoke"]["strategy"]["matrix"]["include"]:
    t.update(e["targets"].split())
print("\n".join(sorted(t)))' "$REPO/.github/workflows/ci-local.yml")
if [ -n "$make_set" ] && [ "$make_set" = "$ci_set" ]; then
    ok "the smoke targets of the Makefile and of CI's matrix are the same"
else
    bad "the smoke targets of the Makefile and of CI's matrix are the same"
    diff <(echo "$make_set") <(echo "$ci_set") | sed 's/^/     /'
fi

exit $fail
