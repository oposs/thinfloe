#!/bin/bash
# Offline tests for scripts/release-assets.sh and `make release-assets`: the
# six assets of a GitHub release are collected in one directory, with a
# SHA256SUMS that verifies inside it. Anything that does not fit a release
# (a version that differs from the build's, a dev version, a file GitHub
# would refuse, stale files) stops the script before it writes a file.
#
# Usage: test/integration/test-release-assets.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$REPO/scripts/release-assets.sh"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

ASSETS=(vmlinuz initrd.img ramroot.sqsh thinfloe-usb.img.zst version SHA256SUMS)
ART="$WORK/artifacts"
OUT="$WORK/out"

make_artifacts() {  # make_artifacts [version]
    rm -rf "$ART" "$OUT"
    mkdir -p "$ART/rootfs" "$ART/images"
    local f
    for f in vmlinuz initrd.img ramroot.sqsh; do
        printf 'content of %s\n' "$f" >"$ART/rootfs/$f"
    done
    printf 'content of image\n' >"$ART/images/thinfloe-usb.img.zst"
    printf '%s\n' "${1:-4.3.0}" >"$ART/rootfs/version"
}
run() {  # run <version>: the script with its output in $WORK/run.out
    bash "$SCRIPT" "$ART" "$OUT" "$1" >"$WORK/run.out" 2>&1
}
# refuses <label> <message> <version>: non-zero exit, the message, no file
refuses() {
    local label=$1 text=$2 version=$3
    if ! run "$version" && grep -qF -- "$text" "$WORK/run.out" \
            && [[ -z $(find "$OUT" -type f 2>/dev/null) ]]; then
        ok "release-assets.sh refuses $label"
    else
        bad "release-assets.sh did not refuse $label with '$text': $(tail -n1 "$WORK/run.out")"
    fi
}

make_artifacts
if run 4.3.0; then
    names=$(cd "$OUT" && find . -mindepth 1 | sed 's|^\./||' | sort | tr '\n' ' ')
    want=$(printf '%s\n' "${ASSETS[@]}" | sort | tr '\n' ' ')
    if [[ $names == "$want" ]]; then
        ok "the out directory holds exactly the six assets"
    else
        bad "the out directory holds '$names', not '$want'"
    fi
    if [[ $(wc -l <"$OUT/SHA256SUMS") == 5 ]] \
            && ! grep -q SHA256SUMS "$OUT/SHA256SUMS" \
            && (cd "$OUT" && sha256sum -c SHA256SUMS >/dev/null 2>&1); then
        ok "SHA256SUMS lists the other five and sha256sum -c passes in the out directory"
    else
        bad "SHA256SUMS is wrong: $(cat "$OUT/SHA256SUMS")"
    fi
    if [[ $OUT/vmlinuz -ef $ART/rootfs/vmlinuz && $OUT/thinfloe-usb.img.zst -ef $ART/images/thinfloe-usb.img.zst ]]; then
        ok "the assets are hard links, not copies"
    else
        bad "the assets are not hard links of the build outputs"
    fi
    # A second run replaces the six names.
    if run 4.3.0 && (cd "$OUT" && sha256sum -c SHA256SUMS >/dev/null 2>&1); then
        ok "a second run replaces the six assets"
    else
        bad "a second run failed: $(tail -n1 "$WORK/run.out")"
    fi
else
    bad "release-assets.sh failed on valid input: $(cat "$WORK/run.out")"
fi

make_artifacts 4.3.1
refuses "a rootfs/version that differs from the version" \
    "rootfs/version is not 4.3.0" 4.3.0
make_artifacts 4.3.0; printf '4.3.0\n4.3.0\n' >"$ART/rootfs/version"
refuses "a rootfs/version of two lines" "rootfs/version is not 4.3.0" 4.3.0
make_artifacts 4.3.0-dev+abc1234
refuses "a dev version" "is not a release version X.Y.Z" 4.3.0-dev+abc1234
make_artifacts
refuses "a version that is not X.Y.Z" "is not a release version X.Y.Z" 4.3
make_artifacts
refuses "an empty version" "is not a release version X.Y.Z" ""

for f in rootfs/vmlinuz rootfs/initrd.img rootfs/ramroot.sqsh rootfs/version images/thinfloe-usb.img.zst; do
    make_artifacts; rm "$ART/$f"
    refuses "a missing $f" "Missing file: $ART/$f" 4.3.0
done

make_artifacts; truncate -s 2G "$ART/rootfs/ramroot.sqsh"
refuses "a file of 2 GiB" "ramroot.sqsh is 2147483648 bytes or more" 4.3.0
make_artifacts; truncate -s 2147483647 "$ART/rootfs/ramroot.sqsh"
if run 4.3.0; then
    ok "a file one byte below 2 GiB passes"
else
    bad "a file one byte below 2 GiB was refused: $(tail -n1 "$WORK/run.out")"
fi

# A symlinked input would bypass the size check and land as a symlink.
make_artifacts; truncate -s 2G "$WORK/big"
rm "$ART/rootfs/ramroot.sqsh"; ln -s "$WORK/big" "$ART/rootfs/ramroot.sqsh"
refuses "a symlinked input" "$ART/rootfs/ramroot.sqsh is a symlink" 4.3.0

make_artifacts; mkdir -p "$OUT"; : >"$OUT/stale.txt"
if ! run 4.3.0 && grep -qF 'stale.txt' "$WORK/run.out" \
        && [[ $(ls "$OUT") == stale.txt ]]; then
    ok "release-assets.sh refuses an out directory with another file and writes nothing"
else
    bad "release-assets.sh did not refuse a stale file in the out directory: $(tail -n1 "$WORK/run.out")"
fi
make_artifacts; mkdir -p "$OUT"; : >"$OUT/vmlinuz"
if run 4.3.0 && [[ $OUT/vmlinuz -ef $ART/rootfs/vmlinuz ]]; then
    ok "an old copy of one of the six names is replaced"
else
    bad "an old copy of vmlinuz was not replaced: $(tail -n1 "$WORK/run.out")"
fi

# A directory with an asset's name: ln -f put the asset inside it.
make_artifacts; mkdir -p "$OUT/vmlinuz"
if ! run 4.3.0 && grep -qF "$OUT/vmlinuz" "$WORK/run.out" \
        && [[ -z $(find "$OUT" -type f) ]]; then
    ok "release-assets.sh refuses a directory named like an asset in the out directory"
else
    bad "release-assets.sh took a directory named vmlinuz in the out directory: $(tail -n1 "$WORK/run.out")"
fi
# ln across file systems: say what to do, not only ln's own message.
make_artifacts
mkdir -p "$WORK/xdev"
cat >"$WORK/xdev/ln" <<'STUB'
#!/bin/bash
echo "ln: failed to create hard link: Invalid cross-device link" >&2
exit 1
STUB
chmod +x "$WORK/xdev/ln"
if ! PATH="$WORK/xdev:$PATH" bash "$SCRIPT" "$ART" "$OUT" 4.3.0 >"$WORK/run.out" 2>&1 \
        && grep -qF "on the same file system as $ART" "$WORK/run.out"; then
    ok "a failed hard link says that the out directory must be on the artifacts' file system"
else
    bad "a failed hard link gave only: $(tail -n1 "$WORK/run.out")"
fi
# A symlinked artifacts directory (the worktrees have one) is accepted.
make_artifacts
mv "$ART" "$WORK/artifacts-real"; ln -s "$WORK/artifacts-real" "$ART"
if run 4.3.0 && (cd "$OUT" && sha256sum -c SHA256SUMS >/dev/null 2>&1) \
        && [[ $OUT/vmlinuz -ef $WORK/artifacts-real/rootfs/vmlinuz ]]; then
    ok "a symlinked artifacts directory is accepted"
else
    bad "a symlinked artifacts directory was refused: $(tail -n1 "$WORK/run.out")"
fi
rm "$ART"; mv "$WORK/artifacts-real" "$ART"

make_artifacts
if ! bash "$SCRIPT" "$ART" >"$WORK/run.out" 2>&1 && grep -qF 'Usage:' "$WORK/run.out"; then
    ok "release-assets.sh prints its usage without arguments"
else
    bad "release-assets.sh did not print its usage"
fi

# The Makefile target in a scratch tree. The five inputs are marked as up
# to date with -o, so no container is needed.
MTREE="$WORK/mtree"
mkdir -p "$MTREE"
cp "$REPO/Makefile" "$MTREE/Makefile"
mkdir -p "$MTREE/scripts"
cp "$SCRIPT" "$MTREE/scripts/release-assets.sh"
printf '4.2.0\n' >"$MTREE/RELEASE"
mk() {
    env -u MAKEFLAGS -u MAKELEVEL -u TF_VERSION make -C "$MTREE" \
        -o artifacts/rootfs/vmlinuz -o artifacts/rootfs/initrd.img \
        -o artifacts/rootfs/ramroot.sqsh -o artifacts/images/thinfloe-usb.img.zst "$@"
}
fill_tree() {  # fill_tree <rootfs version>
    rm -rf "$MTREE/artifacts"
    mkdir -p "$MTREE/artifacts/rootfs" "$MTREE/artifacts/images"
    local f
    for f in vmlinuz initrd.img ramroot.sqsh; do
        printf 'content of %s\n' "$f" >"$MTREE/artifacts/rootfs/$f"
    done
    printf '%s\n' "$1" >"$MTREE/artifacts/rootfs/version"
    printf 'content of image\n' >"$MTREE/artifacts/images/thinfloe-usb.img.zst"
    # the version stamp the Makefile writes at start
    printf '4.2.0\n' >"$MTREE/artifacts/rootfs/.version-kind"
}
fill_tree 4.2.0
mkdir -p "$MTREE/artifacts/release"; : >"$MTREE/artifacts/release/stale.txt"
if mk release-assets TF_VERSION=4.2.0 >"$WORK/make.out" 2>&1 \
        && (cd "$MTREE/artifacts/release" && sha256sum -c SHA256SUMS >/dev/null 2>&1) \
        && [[ $(find "$MTREE/artifacts/release" -mindepth 1 | wc -l) == 6 ]]; then
    ok "make release-assets clears artifacts/release and collects the six assets"
else
    bad "make release-assets failed: $(tail -n3 "$WORK/make.out")"
fi
fill_tree 4.2.0
mkdir -p "$MTREE/artifacts/release"; : >"$MTREE/artifacts/release/stale.txt"
if ! mk release-assets TF_VERSION=4.2.0-dev+abc >"$WORK/make.out" 2>&1 \
        && grep -qF 'release-assets needs TF_VERSION 4.2.0 (RELEASE)' "$WORK/make.out" \
        && [[ $(find "$MTREE/artifacts/release" -mindepth 1 -printf "%f\n") == stale.txt ]]; then
    ok "make release-assets refuses a TF_VERSION that is not RELEASE"
else
    bad "make release-assets did not refuse a dev TF_VERSION: $(tail -n1 "$WORK/make.out")"
fi
fill_tree 4.2.0
rm -rf "$WORK/mtree-artifacts"; mv "$MTREE/artifacts" "$WORK/mtree-artifacts"
ln -s "$WORK/mtree-artifacts" "$MTREE/artifacts"
if mk release-assets TF_VERSION=4.2.0 >"$WORK/make.out" 2>&1 \
        && (cd "$WORK/mtree-artifacts/release" && sha256sum -c SHA256SUMS >/dev/null 2>&1); then
    ok "make release-assets works with a symlinked artifacts directory"
else
    bad "make release-assets failed with a symlinked artifacts directory: $(tail -n3 "$WORK/make.out")"
fi
rm "$MTREE/artifacts"
fill_tree 4.1.0
if ! mk release-assets TF_VERSION=4.2.0 >"$WORK/make.out" 2>&1 \
        && grep -qF 'rootfs/version is not 4.2.0' "$WORK/make.out"; then
    ok "make release-assets refuses a rootfs built as another version"
else
    bad "make release-assets did not refuse a stale rootfs: $(tail -n1 "$WORK/make.out")"
fi

exit $fail
