# shellcheck shell=bash
# tf_source_key ROOT DIR...: the hash of the files under each DIR (relative to
# ROOT), leaving out DIR/target and editor backups (*~). The Dockerfile
# computes the same key over the sources it copies and bakes it into
# thinfloe-menu and thinfloe-share as TF_SOURCE_KEY; keep the two in step.
tf_source_key() {
    local root=$1 dir prune=()
    shift
    for dir in "$@"; do
        prune+=(-path "$dir/target" -prune -o)
    done
    (
        cd "$root" &&
            find "$@" "${prune[@]}" -type f ! -name '*~' -print |
            LC_ALL=C sort | xargs -d '\n' sha256sum | sha256sum | cut -c1-64
    )
}
