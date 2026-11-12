#!/bin/bash
# Offline tests for the release number: RELEASE in the repository, written
# as TF_VERSION (X.Y.Z, or X.Y.Z-dev+<sha>[.dirty] for a local build) to
# /etc/thinfloe/version at export, copied to artifacts/rootfs/version
# and published as `version`, which `thinfloe-upgrade check` reads. Without
# that file every machine's check failed and retried every hour.
#
# Usage: test/integration/test-release-version.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

x_y_z() { [[ $(wc -l <"$1") == 1 ]] && grep -qxE '[0-9]+\.[0-9]+\.[0-9]+' "$1"; }

if x_y_z "$REPO/RELEASE"; then
    ok "RELEASE is one line X.Y.Z ($(cat "$REPO/RELEASE"))"
else
    bad "RELEASE is not one line X.Y.Z"
fi

# The version is written by export-rootfs.sh, from TF_VERSION, which the
# Makefile sets: the release X.Y.Z, or X.Y.Z-dev+<sha>[.dirty] for a local
# build. The script's check, run on its own: the valid_version function.
EXPORT="$REPO/scripts/export-rootfs.sh"
check=$(sed -n '/^valid_version() {/,/^}/p' "$EXPORT")
export_check() {
    bash -c "$check"$'\n''valid_version "$1"' _ "$1" >/dev/null 2>&1
}
if [[ -z "$check" ]]; then
    bad "export-rootfs.sh has no valid_version check"
else
    for v in 4.3.0 4.2.0-dev+85034f2 4.2.0-dev+85034f2.dirty; do
        if export_check "$v"; then
            ok "the export accepts $v"
        else
            bad "the export refuses $v"
        fi
    done
    for v in 4.3 "" $'4.3.0\n4.3.1' v4.3.0 4.2.0-dev 4.2.0-dev+XYZ 4.2.0-rc.1; do
        if export_check "$v"; then
            bad "the export accepts '${v//$'\n'/\\n}'"
        else
            ok "the export refuses '${v//$'\n'/\\n}'"
        fi
    done
fi

# shellcheck disable=SC2016  # literal shell and make text, not expanded
if grep -q 'printf .%s\\n. "$TF_VERSION" >/etc/thinfloe/version' "$EXPORT" \
        && grep -q 'cp /etc/thinfloe/version "${ARTIFACT_DIR}/version"' "$EXPORT"; then
    ok "the export writes TF_VERSION to /etc/thinfloe/version and artifacts/rootfs/version"
else
    bad "the export does not write TF_VERSION to /etc/thinfloe/version and artifacts/rootfs/version"
fi
if awk '/valid_version "\$TF_VERSION"/{v=NR} /^mksquashfs /{m=NR} END{exit !(v && m && v<m)}' "$EXPORT"; then
    ok "the export checks and writes the version before mksquashfs"
else
    bad "the export does not check the version before mksquashfs"
fi
# shellcheck disable=SC2016
if grep -qF -- "-e 'TF_VERSION=\$(TF_VERSION)'" "$REPO/Makefile"; then
    ok "the Makefile passes TF_VERSION to the export"
else
    bad "the Makefile does not pass TF_VERSION to the export"
fi

# The real Makefile in a scratch tree with a stub container runtime.
MTREE="$WORK/mtree"
mkdir -p "$MTREE/containers/ubuntu/patches" "$MTREE/artifacts/rootfs"
cp "$REPO/Makefile" "$MTREE/Makefile"
printf '4.2.0\n' >"$MTREE/RELEASE"
: >"$MTREE/containers/ubuntu/Dockerfile"
mkdir -p "$MTREE/overlay/etc"
: >"$MTREE/overlay/etc/keep"
cp "$REPO/.gitignore" "$MTREE/.gitignore"
# A git checkout, so the default TF_VERSION names a real commit.
g() { git -C "$MTREE" -c user.name=test -c user.email=test@example.invalid \
          -c commit.gpgsign=false -c core.hooksPath=/dev/null "$@"; }
g init -q && g add -A && g commit -q -m fixture
cat >"$WORK/podman" <<'STUB'
#!/bin/bash
case "$1 ${2:-}" in
    "image inspect") echo sha256:aaaa ;;
    *) exit 125 ;;
esac
STUB
chmod +x "$WORK/podman"
# A TF_VERSION of the calling make (make test TF_VERSION=X.Y.Z) must not
# leak into it.
mk() { env -u MAKEFLAGS -u MAKELEVEL -u TF_VERSION make -C "$MTREE" CONTAINER_RUNTIME="$WORK/podman" "$@"; }
KIND="$MTREE/artifacts/rootfs/.version-kind"

# test_release_build_refuses_wrong_version
if ! out=$(mk -n image TF_VERSION=9.9.9 2>&1) && grep -qF 'TF_VERSION 9.9.9 is neither RELEASE (4.2.0) nor a dev version' <<<"$out"; then
    ok "a release build refuses a TF_VERSION that is not RELEASE"
else
    bad "make -n image TF_VERSION=9.9.9 did not fail with the RELEASE message: $(tail -n1 <<<"$out")"
fi
if mk -n help TF_VERSION=4.2.0 >/dev/null 2>&1 \
        && mk -n help TF_VERSION=4.2.0-dev+abc >/dev/null 2>&1; then
    ok "the Makefile accepts TF_VERSION RELEASE and RELEASE-dev+<sha>"
else
    bad "the Makefile refuses TF_VERSION 4.2.0 or 4.2.0-dev+abc"
fi
# The Makefile's default TF_VERSION, or its error text.
# shellcheck disable=SC2016
default_tf() { mk -s --no-print-directory -f Makefile -f - print-tf <<<'print-tf: ; @echo $(TF_VERSION)' 2>&1; }
sha=$(g rev-parse --short HEAD)
tf=$(default_tf)
if [[ $tf == "4.2.0-dev+$sha" ]] && export_check "$tf"; then
    ok "without TF_VERSION a clean checkout builds 4.2.0-dev+<sha>, which the export accepts"
else
    bad "without TF_VERSION a clean checkout builds '$tf', not 4.2.0-dev+$sha"
fi
printf 'x\n' >>"$MTREE/overlay/etc/keep"
tf=$(default_tf)
if [[ $tf == "4.2.0-dev+$sha.dirty" ]] && export_check "$tf"; then
    ok "a changed tracked file makes the version .dirty"
else
    bad "a changed tracked file gives '$tf', not 4.2.0-dev+$sha.dirty"
fi
g checkout -q -- overlay/etc/keep
: >"$MTREE/overlay/etc/new"
tf=$(default_tf)
if [[ $tf == "4.2.0-dev+$sha.dirty" ]]; then
    ok "an untracked file makes the version .dirty"
else
    bad "an untracked file gives '$tf', not 4.2.0-dev+$sha.dirty"
fi
rm -f "$MTREE/overlay/etc/new"
# Outside a git checkout there is no sha: the Makefile refuses at once,
# before any container is built, and says what to do.
NOGIT="$WORK/nogit"
mkdir -p "$NOGIT"
cp "$MTREE/Makefile" "$MTREE/RELEASE" "$NOGIT/"
if ! out=$(GIT_CEILING_DIRECTORIES="$WORK" env -u MAKEFLAGS -u MAKELEVEL -u TF_VERSION \
            make -C "$NOGIT" CONTAINER_RUNTIME="$WORK/podman" -n image 2>&1) \
        && grep -qF 'nor a dev version 4.2.0-dev+<sha>[.dirty]' <<<"$out" \
        && grep -qF 'TF_VERSION=4.2.0' <<<"$out"; then
    ok "outside a git checkout the Makefile refuses the default version at once"
else
    bad "outside a git checkout make -n image did not refuse clearly: $(tail -n1 <<<"$out")"
fi
for v in 4.2.0-dev+ 4.2.0-dev+.dirty 4.2.0-dev+XYZ 4.2.0-dev+abc.dirtyx; do
    if ! mk -n help TF_VERSION="$v" >/dev/null 2>&1; then
        ok "the Makefile refuses TF_VERSION $v"
    else
        bad "the Makefile accepts TF_VERSION $v"
    fi
done
# RELEASE must be one line X.Y.Z, else the build stops with a clear message.
RTREE="$WORK/rtree"
mkdir -p "$RTREE"
cp "$MTREE/Makefile" "$RTREE/"
for r in $'4.2.0\n4.2.1' '4.2.0 4.2.1' 4.2 ''; do
    printf '%s\n' "$r" >"$RTREE/RELEASE"
    if ! out=$(env -u MAKEFLAGS -u MAKELEVEL make -C "$RTREE" -n help TF_VERSION=4.2.0 2>&1) \
            && grep -qF 'RELEASE must be one line X.Y.Z' <<<"$out"; then
        ok "the Makefile refuses RELEASE '${r//$'\n'/\\n}'"
    else
        bad "the Makefile does not refuse RELEASE '${r//$'\n'/\\n}' clearly: $(tail -n1 <<<"$out")"
    fi
done

# The session-packages check of the version in the rootfs: in a release run
# (TF_VERSION is RELEASE) only RELEASE itself passes, else also a dev build.
SP="$REPO/test/integration/test-session-packages.sh"
spcheck=$(sed -n '/^version_ok() {/,/^}/p' "$SP")
sp_ok() { bash -c "$spcheck"$'\n''version_ok "$@"' _ "$@" >/dev/null 2>&1; }
if [[ -z "$spcheck" ]]; then
    bad "test-session-packages.sh has no version_ok check"
else
    if ! sp_ok 4.2.0-dev+abc 4.2.0 4.2.0 && ! sp_ok 4.2.0-dev+abc.dirty 4.2.0 4.2.0; then
        ok "a release run refuses a dev rootfs"
    else
        bad "a release run accepts a dev rootfs"
    fi
    if sp_ok 4.2.0 4.2.0 4.2.0 && sp_ok 4.2.0-dev+abc 4.2.0 4.2.0-dev+def \
            && sp_ok 4.2.0-dev+abc.dirty 4.2.0 ""; then
        ok "a release run accepts RELEASE, a dev run a dev rootfs"
    else
        bad "version_ok refuses a matching rootfs version"
    fi
    if ! sp_ok 4.1.0-dev+abc 4.2.0 "" && ! sp_ok 4.2.0 "" ""; then
        ok "version_ok refuses another release's dev build and an empty RELEASE"
    else
        bad "version_ok accepts another release's dev build or an empty RELEASE"
    fi
fi
# shellcheck disable=SC2016
if grep -qE "TF_VERSION='?\\$\(TF_VERSION\)'? .*test/integration/test-session-packages\.sh" "$REPO/Makefile"; then
    ok "make test passes TF_VERSION to test-session-packages.sh"
else
    bad "make test does not pass TF_VERSION to test-session-packages.sh"
fi

rm -f "$KIND"
mk -n help TF_VERSION=4.2.0-dev+abc >/dev/null 2>&1
if [[ $(cat "$KIND" 2>/dev/null) == 4.2.0-dev ]]; then
    ok "a dev build writes .version-kind 4.2.0-dev"
else
    bad "a dev build wrote .version-kind '$(cat "$KIND" 2>/dev/null)'"
fi
touch -d '2020-01-01' "$KIND"
mk -n help TF_VERSION=4.2.0-dev+def >/dev/null 2>&1
if [[ $(stat -c %Y "$KIND") == $(date -d '2020-01-01' +%s) ]]; then
    ok "another commit leaves .version-kind untouched"
else
    bad "another commit rewrote .version-kind with the same content"
fi
mk -n help TF_VERSION=4.2.0 >/dev/null 2>&1
if [[ $(cat "$KIND" 2>/dev/null) == 4.2.0 ]]; then
    ok "a release build writes .version-kind 4.2.0"
else
    bad "a release build wrote .version-kind '$(cat "$KIND" 2>/dev/null)'"
fi

# switching to a release version re-exports the rootfs
printf '4.2.0-dev\n' >"$KIND"
(cd "$MTREE" && find RELEASE containers overlay -exec touch -d '2020-01-01' {} +)
echo sha256:aaaa >"$MTREE/.ubuntu-container"
touch -d '2021-01-01' "$MTREE/.ubuntu-container" "$KIND"
for f in vmlinuz initrd.img ramroot.sqsh; do
    : >"$MTREE/artifacts/rootfs/$f"; touch -d '2022-01-01' "$MTREE/artifacts/rootfs/$f"
done
if mk -q artifacts/rootfs/ramroot.sqsh TF_VERSION=4.2.0-dev+abc >/dev/null 2>&1; then
    ok "a dev build of the same release does not re-export the rootfs"
else
    bad "a dev build of an exported dev rootfs re-exports it"
fi
if ! mk -q artifacts/rootfs/ramroot.sqsh TF_VERSION=4.2.0 >/dev/null 2>&1; then
    ok "switching to a release version re-exports the rootfs"
else
    bad "switching to a release version left the dev rootfs standing"
fi

# publish.sh in a scratch tree, with rsync replaced by a recorder. It
# publishes releases only: a dev version is refused like any other non-X.Y.Z.
printf '4.0.0\n' >"$WORK/good"; printf '4.0\n' >"$WORK/short"
printf '4.0.0\n4.0.1\n' >"$WORK/two"; printf 'v4.0.0\n' >"$WORK/prefix"
printf '4.0.0-dev+85034f2\n' >"$WORK/dev"
TREE="$WORK/tree"
mkdir -p "$TREE/scripts" "$TREE/artifacts/rootfs" "$TREE/artifacts/images" "$WORK/bin"
cp "$REPO/scripts/publish.sh" "$TREE/scripts/"
for f in rootfs/vmlinuz rootfs/initrd.img rootfs/ramroot.sqsh \
         images/thinfloe-usb.img.zst images/thinfloe-usb.img.zst.sha256; do
    : >"$TREE/artifacts/$f"
done
cat >"$WORK/bin/rsync" <<STUB
#!/bin/bash
printf '%s\n' "\$@" >"$WORK/rsync.args"
STUB
chmod +x "$WORK/bin/rsync"
publish() {
    rm -f "$WORK/rsync.args"
    PATH="$WORK/bin:$PATH" bash "$TREE/scripts/publish.sh" >/dev/null 2>&1
}

rm -f "$TREE/artifacts/rootfs/version"
if ! publish && [[ ! -f "$WORK/rsync.args" ]]; then
    ok "publish.sh refuses without artifacts/rootfs/version"
else
    bad "publish.sh published without a version file"
fi
for f in short two prefix dev; do
    cp "$WORK/$f" "$TREE/artifacts/rootfs/version"
    if ! publish && [[ ! -f "$WORK/rsync.args" ]]; then
        ok "publish.sh refuses a version file that is $f"
    else
        bad "publish.sh published a version file that is $f"
    fi
done
# A version file newer than the image is from a later build than it.
cp "$WORK/good" "$TREE/artifacts/rootfs/version"
touch -d '2030-01-01' "$TREE/artifacts/rootfs/version"
touch -d '2029-01-01' "$TREE/artifacts/images/thinfloe-usb.img.zst"
if ! publish && [[ ! -f "$WORK/rsync.args" ]]; then
    ok "publish.sh refuses a version file newer than the image"
else
    bad "publish.sh published a version file newer than the image"
fi
refusal=$(PATH="$WORK/bin:$PATH" bash "$TREE/scripts/publish.sh" 2>&1 >/dev/null)
if grep -q "touch .*thinfloe-usb.img.zst" <<<"$refusal"; then
    ok "the refusal says how to fix a copy that reordered the times"
else
    bad "the refusal does not name 'touch artifacts/images/thinfloe-usb.img.zst'"
fi
# A missing image is refused by the file check, not waved through.
mv "$TREE/artifacts/images/thinfloe-usb.img.zst" "$WORK/zst"
touch -d '2028-01-01' "$TREE/artifacts/rootfs/version"
if ! publish && [[ ! -f "$WORK/rsync.args" ]]; then
    ok "publish.sh refuses without thinfloe-usb.img.zst"
else
    bad "publish.sh published without thinfloe-usb.img.zst"
fi
mv "$WORK/zst" "$TREE/artifacts/images/thinfloe-usb.img.zst"
touch -d '2029-01-01' "$TREE/artifacts/images/thinfloe-usb.img.zst"
touch -d '2028-01-01' "$TREE/artifacts/rootfs/version"
if publish && grep -qx "$TREE/artifacts/rootfs/version" "$WORK/rsync.args"; then
    ok "publish.sh uploads the version file"
else
    bad "publish.sh does not upload artifacts/rootfs/version"
fi

exit $fail
