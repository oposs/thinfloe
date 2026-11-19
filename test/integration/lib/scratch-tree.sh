# shellcheck shell=bash
# Sourced by the offline tests that run the real Makefile in a scratch tree.
#
# The checkout's `artifacts` is a symlink to the artifacts directory that
# every checkout on the development host shares. A test that ran make on
# the wrong directory once wrote stub files into it (2026-11-19). So every
# tree checks, before make runs in it, that its artifacts path resolves into
# the test's own scratch directory, and the test stops at once if not.

# $1 = the tree, $2 = the test's scratch directory (WORK).
tf_guard_tree() {
    local tree=$1 work real
    work=$(realpath -m -- "$2")
    real=$(realpath -m -- "$tree/artifacts")
    case "$real" in
        "$work"/*) ;;
        *)
            echo "FAIL - $tree/artifacts resolves to $real, outside the scratch directory $work; stopping before make writes there" >&2
            exit 1 ;;
    esac
    case "$(realpath -m -- "$tree")" in
        "$work"/*) ;;
        *)
            echo "FAIL - the tree $tree is outside the scratch directory $work; stopping" >&2
            exit 1 ;;
    esac
}
