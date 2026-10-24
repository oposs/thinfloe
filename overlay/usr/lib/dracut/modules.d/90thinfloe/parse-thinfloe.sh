#!/bin/bash
# parse-thinfloe.sh - Parse kernel command line

# The paths can be overridden so test/integration/test-cmdline.sh runs this
# hook offline; on a real boot none of the variables is set.
TF_RUN_TMP="${TF_RUN_TMP:-/tmp}"
TF_CMDLINE_D="${TF_CMDLINE_D:-/etc/cmdline.d}"
type info >/dev/null 2>&1 || . "${TF_DRACUT_LIB:-/lib/dracut-lib.sh}"
. "${TF_LIB:-/lib/thinfloe-lib.sh}"

ramroot=$(getarg ramroot=)

if [ -z "$ramroot" ]; then
    warn "No ramroot= parameter found"
    die "Boot parameter ramroot= is required"
fi

# tf_log, not info: under systemd, dracut-lib.sh's info() is a bare echo to
# stdout, and dracut-cmdline.service sends stdout only to the journal
# (StandardError=journal+console, no StandardOutput=). rd.info changes
# nothing about that, so these lines never reached the console.
tf_log "thinfloe: ramroot=$ramroot"
echo "$ramroot" > "$TF_RUN_TMP/thinfloe-ramroot"

# Tell dracut we handle root mounting ourselves
root="thinfloe"
rootok=1

if is_remote_url "$ramroot"; then
    tf_log "thinfloe: Network boot detected"
    echo "rd.neednet=1" >> "$TF_CMDLINE_D/99-thinfloe-network.conf"
fi

# rd.dbrrg.debug: the debug entry of every 3.x syslinux.cfg still carries it,
# and an upgrade does not rewrite that file.
if getargbool 0 rd.thinfloe.debug || getargbool 0 rd.dbrrg.debug; then
    tf_log "thinfloe: Debug mode enabled"
    echo "tf_debug=1" > "$TF_RUN_TMP/thinfloe-debug"
fi

return 0
