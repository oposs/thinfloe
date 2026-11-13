#!/bin/bash
# Checks what the built initrd holds of the kernel drivers: compressed modules
# and the early Intel microcode archive. It reads the long listing
# (lsinitramfs -l) because the microcode check needs the size. Later size
# reduction steps append their checks here.
#
# The names of the modules left out of the initrd are checked against the
# modules of ramroot.sqsh (next to the initrd unless given).
#
# Usage: test/integration/test-initramfs-drivers.sh [path/to/initrd.img [path/to/ramroot.sqsh]]

set -uo pipefail

INITRD="${1:-artifacts/rootfs/initrd.img}"
SQSH="${2:-$(dirname "$INITRD")/ramroot.sqsh}"

if [[ ! -f "$INITRD" ]]; then
    echo "FAIL: $INITRD not found - run 'make rootfs' first" >&2
    exit 1
fi
if ! command -v lsinitramfs >/dev/null 2>&1; then
    echo "FAIL: lsinitramfs not installed (initramfs-tools-core)" >&2
    exit 1
fi

WORK=$(mktemp -d)
trap 'chmod -R u+rwX "$WORK" 2>/dev/null; rm -rf "$WORK"' EXIT
LIST="$WORK/list.txt"

if ! lsinitramfs -l "$INITRD" >"$LIST" 2>/dev/null; then
    echo "FAIL: cannot list $INITRD" >&2
    exit 1
fi

fail=0

# A check that something is not in the initrd: $2 is what the test found
# (empty when nothing), shown on FAIL.
absent() {
    local desc="$1" found="$2"
    if [[ -n $found ]]; then
        echo "FAIL - $desc (found: $found)"
        fail=1
    else
        echo "ok   - $desc"
    fi
}

# S1 decompresses modules in the squashfs only. The initrd keeps them
# compressed, which shows that S1 ran after dracut.
zst=$(grep -cE '\.ko\.zst$' "$LIST")
plain=$(grep -cE '\.ko$' "$LIST")
if [[ $zst -ge 500 && $plain -eq 0 ]]; then
    echo "ok   - the initrd keeps compressed modules ($zst .ko.zst, $plain .ko)"
else
    echo "FAIL - the initrd keeps compressed modules ($zst .ko.zst, want at least 500; $plain .ko, want 0)"
    fail=1
fi

# O3 removes intel-ucode from the squashfs; the CPU gets its microcode from
# the early archive in front of the initrd.
ucode_name=kernel/x86/microcode/GenuineIntel.bin
# A line of lsinitramfs -l has 9 fields: mode, links, owner, group, size,
# month, day, time or year, path. The size is the field before the month;
# any other layout is reported instead of read from a wrong field.
ucode_size=$(awk -v n="$ucode_name" '$NF == n {
        if (NF == 9 && $6 ~ /^[A-Z][a-z][a-z]$/) print $5; else print "layout:" NF " fields"
        exit
    }' "$LIST")
if [[ -z $ucode_size ]]; then
    echo "FAIL - the initrd carries the Intel microcode ($ucode_name is missing from the initrd)"
    fail=1
elif [[ ! $ucode_size =~ ^[0-9]+$ ]]; then
    echo "FAIL - the initrd carries the Intel microcode (listing format not understood for $ucode_name: $ucode_size)"
    fail=1
elif [[ $ucode_size -gt 1000000 ]]; then
    echo "ok   - the initrd carries the Intel microcode ($ucode_size bytes)"
else
    echo "FAIL - the initrd carries the Intel microcode ($ucode_name has $ucode_size bytes, want over 1000000)"
    fail=1
fi

# O2: server and data-centre drivers stay out of the initrd (ThinFloe boots
# from USB or over HTTP; the running system keeps every driver). Names are
# module file names; "-" and "_" are the same.
OMITTED=(
    # server NICs
    mlx4_core mlx4_en mlx5_core mlx5_dpll ice i40e qed qede bnx2x bnxt_en
    cxgb cxgb3 cxgb4 cxgb4vf ch_ipsec ch_ktls chcr libcxgb sfc sfc_falcon
    sfc_siena nfp
    # FC and SAS HBAs
    lpfc qla2xxx bfa efct fnic csiostor qedf bnx2fc fcoe libfcoe libfc mptfc
    mptsas mpt3sas mpi3mr pm80xx isci mvsas aic94xx libsas megaraid_sas
    smartpqi hpsa esas2r
    # InfiniBand
    ib_core ib_cm iw_cm rdma_cm rtrs_client rtrs_core ib_srp rnbd_client
    # network file systems
    cifs cifs_md4 ksmbd ceph libceph rbd kafs rxrpc sunrpc grace nfs_acl
)
# These stay: desktop NICs, i915, overlay and zram for the boot, and ubifs
# (a flash file system, not a network one).
PRESENT=(e1000 e1000e igb igc r8169 atlantic tg3 i915 overlay zram ubifs)

# Module names in the initrd, one per line, "-" read as "_".
NAMES="$WORK/module-names.txt"
awk '$NF ~ /\.ko(\.zst)?$/ {n=$NF; sub(/.*\//, "", n); sub(/\.ko(\.zst)?$/, "", n); gsub(/-/, "_", n); print n}' \
    "$LIST" | sort -u >"$NAMES"

in_initrd() { grep -qxF "${1//-/_}" "$NAMES"; }

absent_bad=()
for m in "${OMITTED[@]}"; do
    in_initrd "$m" && absent_bad+=("$m")
done
absent "none of the ${#OMITTED[@]} server and data-centre modules is in the initrd" "${absent_bad[*]}"

# Each omitted name must be a module of the running system. A module renamed
# in a later kernel would pass the check above without testing anything,
# and the driver would come back into the initrd unnoticed.
if [[ ! -f $SQSH ]] || ! command -v unsquashfs >/dev/null 2>&1; then
    echo "FAIL - every omitted name is a module in ramroot.sqsh ($SQSH or unsquashfs missing; run 'make rootfs' first)"
    fail=1
else
    SQNAMES="$WORK/sqsh-module-names.txt"
    unsquashfs -l "$SQSH" 2>/dev/null \
        | awk '$NF ~ /usr\/lib\/modules\/.*\.ko(\.[a-z]+)?$/ {n=$NF; sub(/.*\//, "", n); sub(/\.ko(\.[a-z]+)?$/, "", n); gsub(/-/, "_", n); print n}' \
        | sort -u >"$SQNAMES"
    unknown=()
    for m in "${OMITTED[@]}"; do
        grep -qxF "${m//-/_}" "$SQNAMES" || unknown+=("$m")
    done
    if [[ $(wc -l <"$SQNAMES") -lt 1000 ]]; then
        echo "FAIL - every omitted name is a module in ramroot.sqsh (only $(wc -l <"$SQNAMES") module names listed from $SQSH)"
        fail=1
    elif [[ ${#unknown[@]} -eq 0 ]]; then
        echo "ok   - every one of the ${#OMITTED[@]} omitted names is a module in ramroot.sqsh"
    else
        echo "FAIL - every omitted name is a module in ramroot.sqsh (no module: ${unknown[*]})"
        fail=1
    fi
fi

present_bad=()
for m in "${PRESENT[@]}"; do
    in_initrd "$m" || present_bad+=("$m")
done
if [[ ${#present_bad[@]} -eq 0 ]]; then
    echo "ok   - the boot and desktop NIC modules are in the initrd (${PRESENT[*]})"
else
    echo "FAIL - modules missing from the initrd: ${present_bad[*]}"
    fail=1
fi

# A1: amdgpu stays in --omit-drivers. Plymouth uses simpledrm until amdgpu
# binds after switch-root, so neither the driver nor its firmware is here.
absent "amdgpu.ko is not in the initrd" \
    "$(awk '$NF ~ /(^|\/)amdgpu\.ko(\.[a-z]+)?$/ {print $NF; exit}' "$LIST")"
absent "no usr/lib/firmware/amdgpu/ in the initrd" \
    "$(awk '$NF ~ /usr\/lib\/firmware\/amdgpu(\/|$)/ {print $NF; exit}' "$LIST")"

# Closure: every module that a module of the initrd depends on is in the
# initrd too. This catches an omitted library that a kept module needs.
if ! command -v unmkinitramfs >/dev/null 2>&1 || ! command -v modinfo >/dev/null 2>&1; then
    echo "FAIL - closure check needs unmkinitramfs and modinfo"
    fail=1
else
    TREE="$WORK/tree"
    if ! unmkinitramfs "$INITRD" "$TREE" >/dev/null 2>&1; then
        echo "FAIL - cannot unpack $INITRD"
        fail=1
    else
        mapfile -d '' -t mods < <(find "$TREE" \( -name '*.ko.zst' -o -name '*.ko' \) -print0)
        if [[ ${#mods[@]} -lt 100 ]]; then
            echo "FAIL - closure check found only ${#mods[@]} modules in the unpacked initrd"
            fail=1
        else
            missing=()
            modinfo_failed=()
            with_deps=0
            for f in "${mods[@]}"; do
                if ! deps=$(modinfo -F depends "$f" 2>/dev/null); then
                    modinfo_failed+=("$(basename "$f")")
                    continue
                fi
                [[ -n $deps ]] || continue
                with_deps=$((with_deps + 1))
                IFS=, read -r -a dl <<<"$deps"
                for d in "${dl[@]}"; do
                    [[ -n $d ]] || continue
                    in_initrd "$d" || missing+=("$(basename "$f") needs $d")
                done
            done
            if [[ ${#modinfo_failed[@]} -gt 0 ]]; then
                echo "FAIL - modinfo failed on ${#modinfo_failed[@]} modules: ${modinfo_failed[*]:0:5}"
                fail=1
            elif [[ $with_deps -eq 0 ]]; then
                echo "FAIL - no module of the initrd has a dependency (modinfo output not understood)"
                fail=1
            elif [[ ${#missing[@]} -eq 0 ]]; then
                echo "ok   - every dependency of the ${#mods[@]} modules in the initrd is in the initrd"
            else
                echo "FAIL - modules without their dependency in the initrd:"
                printf '       %s\n' "${missing[@]}"
                fail=1
            fi
        fi
    fi
fi

# O5: the Dockerfile asks for zstd level 19 for the main archive.
# Level 15 gives a 4 MiB window. Levels 17 to 19 give 8 MiB.
# So the check rules out level 16 or lower and --ultra.
# It cannot tell level 17, 18 and 19 apart.
# dracut with 3cpio ignores a level inside a custom --compress string.
# dracut then uses level 15. Only --compress-level counts.
# The main archive is the zstd frame after the early microcode cpio.
trailer=$(LC_ALL=C grep -obUaP 'TRAILER!!!' "$INITRD" | head -1 | cut -d: -f1)
off=""
if [[ -n $trailer ]]; then
    off=$(LC_ALL=C grep -obUaP '\x28\xb5\x2f\xfd' "$INITRD" | cut -d: -f1 | awk -v t="$trailer" '$1 >= t {print; exit}')
fi
if [[ -z $off ]]; then
    echo "FAIL - the initrd main archive is not a zstd frame (no zstd frame after the early cpio)"
    fail=1
else
    tail -c +$((off + 1)) "$INITRD" >"$WORK/main.zst"
    win=$(zstd -lv "$WORK/main.zst" 2>/dev/null | sed -n 's/^Window Size:.*(\([0-9]*\) B).*/\1/p')
    if [[ ! $win =~ ^[0-9]+$ ]]; then
        echo "FAIL - the initrd is compressed at zstd level 17 to 19 (window size not readable from zstd -lv)"
        fail=1
    elif [[ $win -eq 8388608 ]]; then
        echo "ok   - the initrd is compressed at zstd level 17 to 19 (window $win bytes)"
    elif [[ $win -lt 8388608 ]]; then
        echo "FAIL - the initrd is compressed at zstd level 17 to 19 (window $win bytes, want 8388608: level 16 or lower)"
        fail=1
    else
        echo "FAIL - the initrd is compressed at zstd level 17 to 19 (window $win bytes, want 8388608: an --ultra level)"
        fail=1
    fi
fi

if [[ $fail -eq 0 ]]; then
    echo "PASSED"
else
    echo "FAILED"
    exit 1
fi
