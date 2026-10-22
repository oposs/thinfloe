#!/bin/bash
# parse-thinfloe.sh - Parse kernel command line

type info >/dev/null 2>&1 || . /lib/dracut-lib.sh
. /lib/thinfloe-lib.sh

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
echo "$ramroot" > /tmp/thinfloe-ramroot

# Tell dracut we handle root mounting ourselves
root="thinfloe"
rootok=1

if is_remote_url "$ramroot"; then
    tf_log "thinfloe: Network boot detected"
    echo "rd.neednet=1" >> /etc/cmdline.d/99-thinfloe-network.conf
fi

if getargbool 0 rd.thinfloe.debug; then
    tf_log "thinfloe: Debug mode enabled"
    echo "tf_debug=1" > /tmp/thinfloe-debug
fi

return 0
