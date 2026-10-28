#!/bin/bash
# grow-esp.sh - Grow the boot stick's ESP to at most 6 GiB, once
# Runs in the initramfs before finalize-upgrade.sh (pre-mount 20 < 25), while
# no hook has the ESP mounted. See tf_grow_esp in thinfloe-lib.sh.
#
# No exit here: dracut sources every hook of a stage into one shell
# (source_hook), so an exit ends the stage. The first build of this hook
# ended with `exit 0`, and finalize-upgrade.sh never ran, so a staged
# upgrade never rotated (caught by make qemu-smoke-upgrade).

type info >/dev/null 2>&1 || . /lib/dracut-lib.sh
. /lib/thinfloe-lib.sh

# ramroot= from the command line, for the reason finalize-upgrade.sh gives.
# tf_find_esp caches its answer, so finalize-upgrade.sh and mount-squashfs.sh
# get the same partition without waiting again.
if ! is_remote_url "$(getarg ramroot=)" && _grow_esp_dev=$(tf_find_esp); then
    tf_grow_esp "$_grow_esp_dev"
fi
unset _grow_esp_dev
