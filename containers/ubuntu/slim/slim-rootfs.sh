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
DEPMOD_BEFORE=/thinfloe-slim/depmod-before

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
    # What depmod already complains about, so run_depmod can tell new problems
    # from old ones. The names still end in .ko.zst here.
    depmod -n -e -F "/boot/System.map-$KVER" "$KVER" 2>&1 >/dev/null | sort >"$DEPMOD_BEFORE"
}

# Rebuild the module index for the final file names and fail on any depmod
# complaint the snapshot from preconditions does not hold.
run_depmod() {
    local after new
    after=$(mktemp)
    depmod -a -e -F "/boot/System.map-$KVER" "$KVER" >"$after" 2>&1 \
        || die "depmod -a failed after decompressing modules"
    new=$(comm -13 <(sed 's/\.ko\.zst/.ko/g' "$DEPMOD_BEFORE" | sort) \
        <(sed 's/\.ko\.zst/.ko/g' "$after" | sort))
    rm -f "$after"
    [[ -z $new ]] || die "depmod reports new problems:"$'\n'"$new"
}

# O4: remove DVB, TV tuner and radio modules, and radeon and nouveau. The
# set is read from modules.dep before anything changes (names still end in
# .ko.zst): the seed directories, plus every media module that depends on
# the set, minus every module a module outside the set depends on.
trim_media_modules() {
    local dep=/usr/lib/modules/$KVER/modules.dep
    local list out n m
    list=$(mktemp)
    out=$(python3 -I - "$dep" 2>&1 >"$list" <<'PYEOF'
import sys

SEEDS = (
    "kernel/drivers/media/dvb-frontends/",
    "kernel/drivers/media/tuners/",
    "kernel/drivers/media/radio/",
    "kernel/drivers/media/dvb-core/",
    "kernel/drivers/media/usb/dvb-usb/",
    "kernel/drivers/media/usb/dvb-usb-v2/",
)
MEDIA = ("kernel/drivers/media/", "kernel/drivers/staging/media/")
GPU = ("kernel/drivers/gpu/drm/radeon/", "kernel/drivers/gpu/drm/nouveau/")

deps = {}
with open(sys.argv[1]) as f:
    for line in f:
        name, _, rest = line.rstrip("\n").partition(":")
        if name:
            deps[name] = rest.split()

seed = {m for m in deps if m.startswith(SEEDS)}
cut = set(seed)
changed = True
while changed:
    changed = False
    for m, d in deps.items():
        if m not in cut and m.startswith(MEDIA) and any(x in cut for x in d):
            cut.add(m)
            changed = True
closure_size = len(cut)
# Put back what a module outside the set needs.
changed = True
while changed:
    changed = False
    for m, d in deps.items():
        if m not in cut:
            for x in d:
                if x in cut:
                    cut.discard(x)
                    changed = True
gpu = {m for m in deps if m.startswith(GPU)}
for m, d in deps.items():
    if m not in gpu and any(x in gpu for x in d):
        sys.exit("slim-rootfs: %s depends on a radeon or nouveau module" % m)
media = sorted(cut - gpu)
for m in media + sorted(gpu):
    print(m)
sys.stderr.write("%d %d %d %d\n" % (len(seed), closure_size, len(media), len(gpu)))
PYEOF
    ) || { rm -f "$list"; die "cannot compute the media module set: $out"; }
    # out holds "<seed> <closure> <media> <gpu>"
    read -r _ _ n m <<<"$out"
    if [[ -z ${n:-} || $n -lt 100 ]]; then
        rm -f "$list"
        die "only ${n:-0} media modules to remove, expected more than 100"
    fi
    (cd "/usr/lib/modules/$KVER" && xargs -d '\n' -r rm -f -- <"$list") \
        || { rm -f "$list"; die "removing media modules failed"; }
    rm -f "$list"
    echo "slim-rootfs: removed $n media modules, $m radeon and nouveau modules" >&2
}

# The squashfs compresses better as a whole than the zstd files inside it.
# The initrd keeps its compressed copies, which dracut already made.
decompress_modules() {
    find "/usr/lib/modules/$KVER" -name '*.ko.zst' -print0 \
        | xargs -0 -r -P 4 -n 200 zstd -d -q --rm \
        || die "zstd -d failed while decompressing modules"
    if [[ -n $(find "/usr/lib/modules/$KVER" -name '*.ko.zst' -print -quit) ]]; then
        die "compressed modules remain after zstd -d"
    fi
    run_depmod
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
