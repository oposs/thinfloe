#!/bin/bash
# Offline tests for the release number: RELEASE in the repository, written
# to /etc/thinfloe/version at build, exported to artifacts/rootfs/version
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

DF="$REPO/containers/ubuntu/Dockerfile"
if grep -qx 'COPY RELEASE /RELEASE.in' "$DF" \
        && grep -q 'install -m 644 /RELEASE.in /etc/thinfloe/version' "$DF" \
        && grep -q "RELEASE must be one line X.Y.Z" "$DF"; then
    ok "the Dockerfile checks RELEASE and writes /etc/thinfloe/version"
else
    bad "the Dockerfile does not check RELEASE and write /etc/thinfloe/version"
fi

# The build's check, run on its own: the RUN line between COPY and install.
check=$(sed -n '/^RUN { \[ "\$(wc -l <\/RELEASE.in)"/,/RELEASE must be one line/p' "$DF" \
    | sed -e 's/^RUN //' -e 's/\\$//' | tr '\n' ' ' | sed 's/ || { echo.*//')
build_check() {
    cp "$1" "$WORK/RELEASE.in"
    (cd "$WORK" && sh -c "${check//\/RELEASE.in/$WORK/RELEASE.in}") >/dev/null 2>&1
}
printf '4.0.0\n' >"$WORK/good"; printf '4.0\n' >"$WORK/short"
printf '4.0.0\n4.0.1\n' >"$WORK/two"; printf 'v4.0.0\n' >"$WORK/prefix"
if [[ -n "$check" ]] && build_check "$WORK/good"; then
    ok "the build accepts 4.0.0"
else
    bad "the build's check is missing or refuses 4.0.0"
fi
for f in short two prefix; do
    if build_check "$WORK/$f"; then
        bad "the build accepts a RELEASE that is $f"
    else
        ok "the build refuses a RELEASE that is $f"
    fi
done

# .dockerignore is an allow-list: the last pattern that matches decides,
# a leading ! lets the file into the build context. Without !RELEASE the
# COPY found nothing and the build failed.
if python3 -I - "$REPO/.dockerignore" RELEASE <<'PY'
import fnmatch, sys
keep = True
for line in open(sys.argv[1]).read().splitlines():
    line = line.strip()
    if not line or line.startswith("#"):
        continue
    neg = line.startswith("!")
    pat = line[1:] if neg else line
    if fnmatch.fnmatchcase(sys.argv[2], pat.rstrip("/")):
        keep = neg
sys.exit(0 if keep else 1)
PY
then
    ok ".dockerignore lets RELEASE into the build context"
else
    bad ".dockerignore filters RELEASE out of the build context"
fi

if grep -qE '^\.ubuntu-container:.* RELEASE( |$)' "$REPO/Makefile"; then
    ok "RELEASE is a prerequisite of .ubuntu-container"
else
    bad "RELEASE is not a prerequisite of .ubuntu-container"
fi

if grep -q 'cp /etc/thinfloe/version "${ARTIFACT_DIR}/version"' "$REPO/scripts/export-rootfs.sh"; then
    ok "the export copies the image's version to artifacts/rootfs/version"
else
    bad "the export does not copy /etc/thinfloe/version"
fi

# publish.sh in a scratch tree, with rsync replaced by a recorder.
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
for f in short two prefix; do
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
touch -d '2028-01-01' "$TREE/artifacts/rootfs/version"
if publish && grep -qx "$TREE/artifacts/rootfs/version" "$WORK/rsync.args"; then
    ok "publish.sh uploads the version file"
else
    bad "publish.sh does not upload artifacts/rootfs/version"
fi

exit $fail
