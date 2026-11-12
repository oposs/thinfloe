#!/bin/bash
# Last step of the ubuntu image build: remove what the booted client never
# uses. See docs/superpowers/specs/2026-11-12-image-size-design.md.
#
# This script runs as the last RUN of containers/ubuntu/Dockerfile, after
# dracut. Nothing it calls may run dpkg, apt-get or dracut: a dpkg run can
# fire the update-initramfs trigger, which rebuilds the initrd with default
# flags from the slimmed tree. Never add a find/rm sweep over
# /usr/lib/firmware; the only firmware cuts are the ones named below.

set -euo pipefail

die() {
    echo "slim-rootfs: $*" >&2
    exit 1
}

KVER=""

# Exactly one kernel, and the initrd that dracut built for it.
preconditions() {
    local dirs=()
    local d
    for d in /usr/lib/modules/*/; do
        [[ -d $d ]] && dirs+=("$(basename "$d")")
    done
    if [[ ${#dirs[@]} -ne 1 ]]; then
        die "want one kernel in /usr/lib/modules, found: ${dirs[*]:-none}"
    fi
    KVER=${dirs[0]}
    [[ -f /boot/initrd.img-$KVER ]] || die "run after dracut: no /boot/initrd.img-$KVER"
    [[ -f /boot/System.map-$KVER ]] || die "run after dracut: no /boot/System.map-$KVER"
}

trim_media_modules() {
    : # Task 5
}

decompress_modules() {
    : # Task 4
}

prune_iwlwifi() {
    : # Task 6
}

prune_amdgpu() {
    : # Task 9
}

check_firmware_names() {
    : # Task 6
}

drop_intel_ucode() {
    : # Task 7
}

# The locale archive is compiled; the sources are not read at run time.
drop_i18n() {
    rm -rf /usr/share/i18n
    local line name have
    have=$(locale -a)
    while read -r line; do
        [[ -z $line || $line == \#* ]] && continue
        name=${line%% *}
        # de_CH.UTF-8 is listed as de_CH.utf8 by locale -a
        if [[ $name == *.* ]]; then
            name=${name%%.*}.$(tr '[:upper:]' '[:lower:]' <<<"${name#*.}" | tr -d '-')
        fi
        grep -qxF -- "$name" <<<"$have" \
            || die "locale $name missing after removing /usr/share/i18n"
    done </etc/locale.gen
}

# hwdb.bin is prebuilt and systemd-hwdb-update.service is masked.
drop_hwdb_sources() {
    rm -rf /usr/lib/udev/hwdb.d
    local out
    out=$(systemd-hwdb query usb:v046DpC52B) || die "systemd-hwdb query failed after removing hwdb.d"
    [[ -n $out ]] || die "systemd-hwdb knows no usb:v046DpC52B after removing hwdb.d"
}

main() {
    preconditions
    trim_media_modules
    decompress_modules
    prune_iwlwifi
    prune_amdgpu
    check_firmware_names
    drop_intel_ucode
    drop_i18n
    drop_hwdb_sources
}

main "$@"
