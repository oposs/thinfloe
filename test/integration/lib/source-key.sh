# shellcheck shell=bash
# tf_source_key ROOT CRATE [DIR...]: the hash of the files under CRATE and
# each DIR (relative to ROOT), leaving out CRATE/target and editor backups
# (*~). The Dockerfile computes the same key over the sources it copies and
# bakes it into thinfloe-menu and thinfloe-share as TF_SOURCE_KEY; keep the
# two in step.
tf_source_key() {
    local root=$1
    shift
    (
        cd "$root" &&
            find "$@" -path "$1/target" -prune -o -type f ! -name '*~' -print |
            LC_ALL=C sort | xargs -r -d '\n' sha256sum | sha256sum | cut -c1-64
    )
}
