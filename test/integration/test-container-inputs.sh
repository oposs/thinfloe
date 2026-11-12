#!/bin/bash
# Offline tests for the inputs of the ubuntu container stamp.
#
# .ubuntu-container depended on the overlay and brand files that exist. A
# file deleted from overlay/ took its own name off that list, so make saw no
# changed prerequisite: the image kept the deleted file, and `make test`
# validated it green (found 2026-10-27, when a removed systemd drop-in stayed
# in ramroot.sqsh). The directories are prerequisites too now; deleting a
# file changes its directory's mtime.
#
# Runs the real Makefile in a scratch tree with a stub container runtime and
# asks `make -q` whether the stamp is up to date.
#
# It also builds a FROM-scratch probe with podman (CONTAINER_RUNTIME) against
# the repository root, to check the build context; without podman the test
# fails.
#
# Usage: test/integration/test-container-inputs.sh

set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fail=0
ok()  { echo "ok   - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

# Every image the Makefile inspects reports the same ID, so the stamp stays
# valid.
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
mkdir -p "$TREE/containers/ubuntu/patches" "$TREE/src/thinfloe-menu/src" \
    "$TREE/overlay/etc/x.d" "$TREE/brand" "$TREE/artifacts/rootfs"
cp "$REPO/Makefile" "$TREE/Makefile"
inputs=(RELEASE containers/ubuntu/Dockerfile src/thinfloe-menu/src/main.rs
        overlay/etc/keep overlay/etc/x.d/gone.conf brand/keep.svg brand/gone.svg)

# Prints "up-to-date" when make would not rebuild the stamp, "rebuild"
# otherwise. Every input, file and directory, is older than the stamp first.
reset() {
    rm -rf "$TREE/overlay" "$TREE/brand"
    mkdir -p "$TREE/overlay/etc/x.d" "$TREE/brand"
    for f in "${inputs[@]}"; do : >"$TREE/$f"; done
    echo 4.2.0 >"$TREE/RELEASE"
    (cd "$TREE" && find RELEASE containers src overlay brand -exec touch -d '2020-01-01' {} +)
    echo sha256:aaaa >"$TREE/.ubuntu-container"
    touch -d '2021-01-01' "$TREE/.ubuntu-container"
}
state() {
    if env -u MAKEFLAGS -u MAKELEVEL -u TF_VERSION make -q -C "$TREE" CONTAINER_RUNTIME="$RUNTIME" TF_VERSION=4.2.0-dev+abc .ubuntu-container >/dev/null 2>&1; then
        echo up-to-date
    else
        echo rebuild
    fi
}

reset
if [[ $(state) == up-to-date ]]; then
    ok "a stamp newer than all its inputs is not built again"
else
    bad "the stamp is rebuilt although no input changed"
fi

reset
rm "$TREE/overlay/etc/x.d/gone.conf"
if [[ $(state) == rebuild ]]; then
    ok "deleting an overlay file rebuilds the image"
else
    bad "deleting an overlay file left the image standing (the file stays in the image)"
fi

reset
rm "$TREE/overlay/etc/x.d/gone.conf" && rmdir "$TREE/overlay/etc/x.d"
if [[ $(state) == rebuild ]]; then
    ok "deleting an overlay directory rebuilds the image"
else
    bad "deleting an overlay directory left the image standing"
fi

reset
rm "$TREE/brand/gone.svg"
if [[ $(state) == rebuild ]]; then
    ok "deleting a brand file rebuilds the image"
else
    bad "deleting a brand file left the image standing"
fi

# The version is written at export, not into the image: a new RELEASE (or a
# new commit) does not rebuild the container, a new kind of version
# (.version-kind, release or dev) exports the rootfs again.
reset
touch "$TREE/RELEASE"
if [[ $(state) == up-to-date ]]; then
    ok "a new RELEASE does not rebuild the image"
else
    bad "a changed RELEASE rebuilds the image"
fi

rootfs_state() {
    if env -u MAKEFLAGS -u MAKELEVEL make -q -C "$TREE" CONTAINER_RUNTIME="$RUNTIME" \
            TF_VERSION="$1" artifacts/rootfs/ramroot.sqsh >/dev/null 2>&1; then
        echo up-to-date
    else
        echo re-export
    fi
}
reset
printf '4.2.0\n' >"$TREE/RELEASE"
printf '4.2.0-dev\n' >"$TREE/artifacts/rootfs/.version-kind"
touch -d '2020-01-01' "$TREE/RELEASE"
touch -d '2021-01-01' "$TREE/artifacts/rootfs/.version-kind"
for f in vmlinuz initrd.img ramroot.sqsh; do
    : >"$TREE/artifacts/rootfs/$f"; touch -d '2022-01-01' "$TREE/artifacts/rootfs/$f"
done
if [[ $(rootfs_state 4.2.0-dev+abc) == up-to-date ]]; then
    ok "an unchanged .version-kind does not export the rootfs again"
else
    bad "the rootfs is exported again although .version-kind did not change"
fi
if [[ $(rootfs_state 4.2.0) == re-export ]]; then
    ok "a changed .version-kind exports the rootfs again"
else
    bad "a changed .version-kind left the rootfs with the old version"
fi

# Rewriting bytecode touches the pyc and its __pycache__, not overlay/usr/bin.
reset
mkdir -p "$TREE/overlay/usr/bin/__pycache__"
(cd "$TREE" && find overlay -name __pycache__ -prune -o -exec touch -d '2020-01-01' {} +)
: >"$TREE/overlay/usr/bin/__pycache__/tool.cpython-312.pyc"
if [[ $(state) == up-to-date ]]; then
    ok "bytecode in overlay/__pycache__ does not make the image stale"
else
    bad "a __pycache__ under overlay/ makes the image rebuild"
fi

# The build context: .dockerignore is an allow-list, and a COPY of a file it
# leaves out fails the build ("no items matching glob", RELEASE, 2026-11-07).
# A FROM-scratch probe runs every COPY/ADD from the repository (not --from)
# of the Dockerfiles built from the repository root, against the real
# .dockerignore, with the container runtime itself as the judge.
RT="${CONTAINER_RUNTIME:-podman}"
if ! command -v "$RT" >/dev/null 2>&1; then
    bad "no $RT to check the build context with"
else
    PROBE="$WORK/context-probe"
    mkdir -p "$PROBE"
    TAG="localhost/thinfloe-context-probe:$$"
    {
        echo "FROM scratch"
        # Dockerfiles whose build context is the repository root (Makefile).
        for df in containers/ubuntu/Dockerfile; do
            grep -iE '^(COPY|ADD)[[:space:]]' "$REPO/$df" | grep -vi -- '--from' \
                | sed -E 's/^[Aa][Dd][Dd]/COPY/; s/^[Cc][Oo][Pp][Yy]/COPY/'
        done
    } >"$PROBE/Dockerfile"
    n=$(grep -c '^COPY' "$PROBE/Dockerfile")
    if [[ $n -eq 0 ]]; then
        bad "no COPY/ADD line found in containers/ubuntu/Dockerfile; the probe checks nothing"
    elif "$RT" build --network=none --pull=never -q -t "$TAG" \
            -f "$PROBE/Dockerfile" "$REPO" >"$PROBE/log" 2>&1; then
        ok "the build context has every source of the $n COPY/ADD lines"
    else
        bad "the build context lacks a COPY/ADD source (.dockerignore?): $(grep -m1 -iE 'error|no items' "$PROBE/log")"
    fi
    "$RT" rmi -f "$TAG" >/dev/null 2>&1
fi

exit $fail
