#!/bin/sh
# Runs inside the two-stick smoke VM as kernel-command-line.service
# (systemd.run=, see scripts/run-qemu-two-sticks-smoke.sh) and writes which
# ESP the initramfs mounted, and that ESP's stick id, to the serial console,
# where the host reads it. Lives on the stick's ESP, not in the image.

ESP=/run/thinfloe/storage/efi

{
    echo "SMOKE-STICK: $(findmnt -n -o SOURCE "$ESP") $(cat "$ESP/config/stick-id" 2>/dev/null || echo none)"
    echo "SMOKE-STICK-DONE"
} >/dev/console 2>&1
