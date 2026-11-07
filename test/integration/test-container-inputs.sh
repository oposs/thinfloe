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
    (cd "$TREE" && find RELEASE containers src overlay brand -exec touch -d '2020-01-01' {} +)
    echo sha256:aaaa >"$TREE/.ubuntu-container"
    touch -d '2021-01-01' "$TREE/.ubuntu-container"
}
state() {
    if make -q -C "$TREE" CONTAINER_RUNTIME="$RUNTIME" .ubuntu-container >/dev/null 2>&1; then
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

reset
touch "$TREE/RELEASE"
if [[ $(state) == rebuild ]]; then
    ok "a new RELEASE rebuilds the image"
else
    bad "a changed RELEASE left the image with the old /etc/thinfloe/version"
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
    PROBE_DIR="${TMPDIR:-/scratch/oetiker/claude-tmp/thinfloe-tests}"
    mkdir -p "$PROBE_DIR"
    PROBE=$(mktemp -d "$PROBE_DIR/context-probe.XXXXXX")
    TAG="localhost/thinfloe-context-probe:$$"
    {
        echo "FROM scratch"
        # Dockerfiles whose build context is the repository root (Makefile).
        for df in containers/ubuntu/Dockerfile; do
            grep -E '^(COPY|ADD)[[:space:]]' "$REPO/$df" | grep -v -- '--from' \
                | sed -E 's/^ADD/COPY/'
        done
    } >"$PROBE/Dockerfile"
    n=$(grep -c '^COPY' "$PROBE/Dockerfile")
    if "$RT" build --network=none --pull=never -q -t "$TAG" \
            -f "$PROBE/Dockerfile" "$REPO" >"$PROBE/log" 2>&1; then
        ok "the build context has every source of the $n COPY/ADD lines"
    else
        bad "the build context lacks a COPY/ADD source (.dockerignore?): $(grep -m1 -iE 'error|no items' "$PROBE/log")"
    fi
    "$RT" rmi -f "$TAG" >/dev/null 2>&1
    rm -rf "$PROBE"
fi

exit $fail
