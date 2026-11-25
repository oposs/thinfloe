#!/bin/sh
# Runs inside the two-stick smoke VM as kernel-command-line.service
# (systemd.run=, see scripts/run-qemu-two-sticks-smoke.sh) and writes which
# ESP the initramfs mounted, and that ESP's stick id, to the serial console,
# where the host reads it, and what is mounted under /media/tfuser: the
# other ThinFloe stick must not be. Lives on the stick's ESP, not in the image.

ESP=/run/thinfloe/storage/efi

# Let the automounter see every disk first.
udevadm settle --timeout=30 >/dev/null 2>&1

{
    echo "SMOKE-STICK: $(findmnt -n -o SOURCE "$ESP") $(cat "$ESP/config/stick-id" 2>/dev/null || echo none)"
    echo "SMOKE-STICK-MEDIA: $(findmnt -rn -o SOURCE,TARGET | awk 'index($2, "/media/tfuser/") == 1' | tr '\n' ' ')$(cut -f 1 /run/thinfloe/removable 2>/dev/null | tr '\n' ' ')"
    echo "SMOKE-STICK-DONE"
} >/dev/console 2>&1
