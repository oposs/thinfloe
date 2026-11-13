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
# Both the check and the write of /etc/thinfloe/version come before
# mksquashfs; the awk used to find only the check.
if awk '/valid_version "\$TF_VERSION"/{v=NR} /^printf .*>\/etc\/thinfloe\/version$/{w=NR}
        /^mksquashfs /{m=NR} END{exit !(v && w && m && v<m && w<m)}' "$EXPORT"; then
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
if mk -n artifacts/rootfs/.version-kind TF_VERSION=4.2.0 >/dev/null 2>&1 \
        && mk -n artifacts/rootfs/.version-kind TF_VERSION=4.2.0-dev+abc >/dev/null 2>&1; then
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
    if ! mk -n artifacts/rootfs/.version-kind TF_VERSION="$v" >/dev/null 2>&1; then
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
    if ! out=$(env -u MAKEFLAGS -u MAKELEVEL make -C "$RTREE" -n image TF_VERSION=4.2.0 2>&1) \
            && grep -qF 'RELEASE must be one line X.Y.Z' <<<"$out"; then
        ok "the Makefile refuses RELEASE '${r//$'\n'/\\n}'"
    else
        bad "the Makefile does not refuse RELEASE '${r//$'\n'/\\n}' clearly: $(tail -n1 <<<"$out")"
    fi
done
# help and clean build nothing: they need neither a valid RELEASE nor, outside
# git, a TF_VERSION.
printf '4.2\n' >"$RTREE/RELEASE"
for goal in help clean; do
    if env -u MAKEFLAGS -u MAKELEVEL -u TF_VERSION make -C "$RTREE" "$goal" >/dev/null 2>&1; then
        ok "make $goal runs with a broken RELEASE"
    else
        bad "make $goal refuses a broken RELEASE"
    fi
    if GIT_CEILING_DIRECTORIES="$WORK" env -u MAKEFLAGS -u MAKELEVEL -u TF_VERSION \
            make -C "$NOGIT" "$goal" >/dev/null 2>&1; then
        ok "make $goal runs outside git without TF_VERSION"
    else
        bad "make $goal needs TF_VERSION outside git"
    fi
done
# The release-assets check refuses only release-assets: another goal parses
# with a dev TF_VERSION.
for args in "help" "-n artifacts/rootfs/.version-kind"; do
    # shellcheck disable=SC2086  # two words on purpose
    if mk $args TF_VERSION=4.2.0-dev+abc >/dev/null 2>&1; then
        ok "make $args parses with a dev TF_VERSION"
    else
        bad "make $args refuses a dev TF_VERSION"
    fi
done

# The session-packages check of the version in the rootfs: in a release run
# (TF_VERSION is RELEASE) only RELEASE itself passes, else also a dev build.
SP="$REPO/test/integration/test-session-packages.sh"
spcheck=$(sed -n '/^version_ok() {/,/^}/p' "$SP")
sp_ok() { bash -c "$spcheck"$'\n''version_ok "$@"' _ "$@" >/dev/null 2>&1; }
# The same check on a rootfs: a fake one whose /etc/thinfloe/version and
# version file beside it say $1, in a release run of RELEASE 4.2.0.
rootfs_check=$(sed -n '/^rootfs_version_ok() {/,/^}/p' "$SP")
rootfs_ok() {
    local d="$WORK/fake-rootfs-$2"
    mkdir -p "$d/tree/etc/thinfloe" "$d/rootfs" "$d/scratch"
    printf '%s\n' "$1" >"$d/tree/etc/thinfloe/version"
    printf '%s\n' "$1" >"$d/rootfs/version"
    mksquashfs "$d/tree" "$d/rootfs/ramroot.sqsh" -noappend -quiet >/dev/null 2>&1 || return 2
    bash -c "$spcheck"$'\n'"$rootfs_check"$'\n''rootfs_version_ok "$@"' _ \
        "$d/rootfs/ramroot.sqsh" 4.2.0 4.2.0 "$d/scratch" >/dev/null 2>&1
}
if [[ -z "$rootfs_check" ]]; then
    bad "test-session-packages.sh has no rootfs_version_ok check"
else
    if rootfs_ok 4.2.0 release; then
        ok "a release run accepts a rootfs of RELEASE"
    else
        bad "a release run refuses a rootfs of RELEASE"
    fi
    if rootfs_ok 4.2.0-dev+abc dev; then
        bad "a release run accepts a dev rootfs on disk"
    else
        ok "a release run refuses a dev rootfs on disk"
    fi
fi
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
mk artifacts/rootfs/.version-kind TF_VERSION=4.2.0-dev+abc >/dev/null 2>&1
if [[ $(cat "$KIND" 2>/dev/null) == 4.2.0-dev ]]; then
    ok "a dev build writes .version-kind 4.2.0-dev"
else
    bad "a dev build wrote .version-kind '$(cat "$KIND" 2>/dev/null)'"
fi
touch -d '2020-01-01' "$KIND"
mk artifacts/rootfs/.version-kind TF_VERSION=4.2.0-dev+def >/dev/null 2>&1
if [[ $(stat -c %Y "$KIND") == $(date -d '2020-01-01' +%s) ]]; then
    ok "another commit leaves .version-kind untouched"
else
    bad "another commit rewrote .version-kind with the same content"
fi
mk artifacts/rootfs/.version-kind TF_VERSION=4.2.0 >/dev/null 2>&1
if [[ $(cat "$KIND" 2>/dev/null) == 4.2.0 ]]; then
    ok "a release build writes .version-kind 4.2.0"
else
    bad "a release build wrote .version-kind '$(cat "$KIND" 2>/dev/null)'"
fi

# switching to a release version re-exports the rootfs. The export also
# depends on the scripts it runs and the container on the slim directory;
# without them make -q fails (exit 2).
mkdir -p "$MTREE/scripts/lib" "$MTREE/containers/ubuntu/slim"
: >"$MTREE/scripts/export-rootfs.sh"; : >"$MTREE/scripts/lib/common.sh"
printf '4.2.0-dev\n' >"$KIND"
(cd "$MTREE" && find RELEASE containers overlay scripts -exec touch -d '2020-01-01' {} +)
echo sha256:aaaa >"$MTREE/.ubuntu-container"
touch -d '2021-01-01' "$MTREE/.ubuntu-container" "$KIND"
for f in vmlinuz initrd.img ramroot.sqsh; do
    : >"$MTREE/artifacts/rootfs/$f"; touch -d '2022-01-01' "$MTREE/artifacts/rootfs/$f"
done
# Only a build writes .version-kind; make -n, -q and help leave it, and -n
# and -q still see that a release build re-exports.
for args in "-n rootfs" "-q rootfs" "help"; do
    # shellcheck disable=SC2086  # two words on purpose
    mk $args TF_VERSION=4.2.0 >/dev/null 2>&1
    if [[ $(cat "$KIND") == 4.2.0-dev && $(stat -c %Y "$KIND") == $(date -d '2021-01-01' +%s) ]]; then
        ok "make $args leaves .version-kind as it is"
    else
        bad "make $args rewrote .version-kind ($(cat "$KIND"))"
    fi
done
if mk -n artifacts/rootfs/ramroot.sqsh TF_VERSION=4.2.0 2>/dev/null | grep -q export-rootfs.sh; then
    ok "make -n shows the export a release build runs"
else
    bad "make -n of a release build shows no export"
fi
if mk -q artifacts/rootfs/ramroot.sqsh TF_VERSION=4.2.0-dev+abc >/dev/null 2>&1; then
    ok "a dev build of the same release does not re-export the rootfs"
else
    bad "a dev build of an exported dev rootfs re-exports it"
fi
mk -q artifacts/rootfs/ramroot.sqsh TF_VERSION=4.2.0 >/dev/null 2>&1
rc=$?
if [[ $rc -eq 1 ]]; then
    ok "switching to a release version re-exports the rootfs"
else
    bad "switching to a release version left the dev rootfs standing (make -q exit $rc)"
fi

# publish.sh mirrors one downloaded GitHub release directory, with rsync
# replaced by a recorder. The six assets go up unchanged; three more files
# are generated for the old sticks.
ASSETS=(vmlinuz initrd.img ramroot.sqsh thinfloe-usb.img.zst version SHA256SUMS)
printf '4.0\n' >"$WORK/short"; printf '4.0.0\n4.0.1\n' >"$WORK/two"
printf 'v4.0.0\n' >"$WORK/prefix"; printf '4.0.0-dev+85034f2\n' >"$WORK/dev"
REL="$WORK/release"
mkdir -p "$WORK/bin"
# The stub keeps what it was given as hard links in upload/: publish.sh
# removes its work directory once rsync is done.
cat >"$WORK/bin/rsync" <<STUB
#!/bin/bash
printf '%s\n' "\$@" >"$WORK/rsync.args"
mkdir -p "$WORK/upload"
for a in "\$@"; do [[ -f \$a ]] && ln -f "\$a" "$WORK/upload/\${a##*/}"; done
exit 0
STUB
chmod +x "$WORK/bin/rsync"
make_release() {
    local f
    rm -rf "$REL"; mkdir -p "$REL"
    for f in vmlinuz initrd.img ramroot.sqsh thinfloe-usb.img.zst; do
        printf 'content of %s\n' "$f" >"$REL/$f"
    done
    printf '4.0.0\n' >"$REL/version"
    (cd "$REL" && sha256sum vmlinuz initrd.img ramroot.sqsh thinfloe-usb.img.zst version >SHA256SUMS)
}
publish() {  # publish [dir...]: run publish.sh with the recorder
    rm -f "$WORK/rsync.args"
    rm -rf "$WORK/upload"
    PATH="$WORK/bin:$PATH" bash "$REPO/scripts/publish.sh" "$@" >"$WORK/publish.out" 2>&1
}
refuses() {  # refuses <label> <error text> [dir...]
    local label=$1 text=$2; shift 2
    if ! publish "$@" && [[ ! -f "$WORK/rsync.args" ]] \
        && grep -qF -- "$text" "$WORK/publish.out"; then
        ok "publish.sh refuses $label"
    else
        bad "publish.sh did not refuse $label with '$text'"
    fi
}

make_release
refuses "without a release directory" "Usage:"
refuses "a directory that does not exist" "Usage:" "$WORK/nowhere"
for f in "${ASSETS[@]}"; do
    make_release; rm "$REL/$f"
    refuses "a release without $f" "Missing file: $REL/$f" "$REL"
done
make_release; printf 'tampered\n' >>"$REL/ramroot.sqsh"
refuses "a release that fails sha256sum -c" "sha256sum -c SHA256SUMS failed in $REL" "$REL"
make_release; grep -v ' ramroot.sqsh$' "$REL/SHA256SUMS" >"$WORK/sums"; cp "$WORK/sums" "$REL/SHA256SUMS"
refuses "a SHA256SUMS without a ramroot.sqsh line" "$REL/SHA256SUMS has not exactly one line for ramroot.sqsh" "$REL"
for f in short two prefix dev; do
    make_release; cp "$WORK/$f" "$REL/version"
    (cd "$REL" && sha256sum vmlinuz initrd.img ramroot.sqsh thinfloe-usb.img.zst version >SHA256SUMS)
    refuses "a version file that is $f" "$REL/version is not one line X.Y.Z" "$REL"
done

check() { if [[ $1 == 1 ]]; then ok "$2"; else bad "$2"; fi; }
make_release
if publish "$REL"; then
    args=$(cat "$WORK/rsync.args")
    all=1
    for f in "${ASSETS[@]}"; do
        grep -qx "$REL/$f" <<<"$args" || all=0
    done
    check "$all" "publish.sh uploads the six assets"
    extra=1
    for f in thinfloe-usb.img.zst.sha256 dbrrg-usb.img.zst dbrrg-usb.img.zst.sha256; do
        grep -qE "/$f\$" <<<"$args" || extra=0
    done
    check "$extra" "publish.sh uploads the three generated files"
    hl=0; grep -qE '^-[a-zA-Z]*H' <<<"$args" && hl=1
    check "$hl" "rsync keeps hard links (-H)"
    tg=0; [[ $(tail -n1 "$WORK/rsync.args") == "oetiker@web-volki-01-adm:public_html/dbrrg/" ]] && tg=1
    check "$tg" "target is oetiker@web-volki-01-adm:public_html/dbrrg/"
    good=1
    for s in thinfloe-usb.img.zst.sha256 dbrrg-usb.img.zst.sha256; do
        [[ -f $WORK/upload/$s ]] || { good=0; continue; }
        (cd "$WORK/upload" && sha256sum -c "$s" >/dev/null 2>&1) || good=0
    done
    check "$good" "the generated checksums verify their images"
    lk=0; [[ $WORK/upload/dbrrg-usb.img.zst -ef $REL/thinfloe-usb.img.zst ]] && lk=1
    check "$lk" "dbrrg-usb.img.zst is a hard link of the image"
    n=$(find "$REL" -mindepth 1 -maxdepth 1 | wc -l)
    check "$(( n == 6 ))" "the release directory keeps only its six assets (no publish-work/)"
else
    bad "publish.sh failed on a valid release: $(cat "$WORK/publish.out")"
fi

exit $fail
