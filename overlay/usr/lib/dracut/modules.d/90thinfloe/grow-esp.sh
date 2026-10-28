#!/bin/bash
# grow-esp.sh - Grow the boot stick's ESP to at most 6 GiB, once
# Runs in the initramfs before finalize-upgrade.sh (pre-mount 20 < 25), while
# no hook has the ESP mounted. See tf_grow_esp in thinfloe-lib.sh.

type info >/dev/null 2>&1 || . /lib/dracut-lib.sh
. /lib/thinfloe-lib.sh

# ramroot= from the command line, for the reason finalize-upgrade.sh gives.
if is_remote_url "$(getarg ramroot=)"; then
    exit 0
fi

# tf_find_esp caches its answer, so finalize-upgrade.sh and mount-squashfs.sh
# get the same partition without waiting again.
if ESP_DEV=$(tf_find_esp); then
    tf_grow_esp "$ESP_DEV"
fi
exit 0
