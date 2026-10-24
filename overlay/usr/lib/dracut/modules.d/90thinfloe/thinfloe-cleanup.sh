#!/bin/bash
# thinfloe-cleanup.sh - Pre-pivot cleanup

type info >/dev/null 2>&1 || . /lib/dracut-lib.sh
. /lib/thinfloe-lib.sh

info "thinfloe: Pre-pivot cleanup"

# Do NOT unmount squashfs, EFI partition, ZRAM, or overlay!
# They are needed for the running system

if [ ! -f /tmp/thinfloe-debug ]; then
    rm -f /tmp/thinfloe-ramroot 2>/dev/null || true
fi

# Save boot log
NEWROOT="${NEWROOT:-/sysroot}"

if [ -d "$NEWROOT/var/log" ]; then
    if [ -f /run/initramfs/init.log ]; then
        cp /run/initramfs/init.log "$NEWROOT/var/log/dracut-boot.log" 2>/dev/null || true
    fi

    if [ -d "$TF_STATE" ]; then
        mkdir -p "$NEWROOT/var/log/thinfloe"
        cp -r "$TF_STATE"/* "$NEWROOT/var/log/thinfloe/" 2>/dev/null || true
    fi
fi

info "Cleanup complete"

return 0
