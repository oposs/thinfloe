#!/bin/bash
# Checks what the built initrd holds of the kernel drivers. Later size
# reduction steps append their checks here.
#
# Usage: test/integration/test-initramfs-drivers.sh [path/to/initrd.img]

set -uo pipefail

INITRD="${1:-artifacts/rootfs/initrd.img}"

if [[ ! -f "$INITRD" ]]; then
    echo "FAIL: $INITRD not found - run 'make rootfs' first" >&2
    exit 1
fi
if ! command -v lsinitramfs >/dev/null 2>&1; then
    echo "FAIL: lsinitramfs not installed (initramfs-tools-core)" >&2
    exit 1
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
LIST="$WORK/list.txt"

if ! lsinitramfs -l "$INITRD" >"$LIST" 2>/dev/null; then
    echo "FAIL: cannot list $INITRD" >&2
    exit 1
fi

fail=0

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
ucode_size=$(awk -v n="$ucode_name" '$NF == n {print $5}' "$LIST" | head -n 1)
if [[ -n $ucode_size && $ucode_size -gt 1000000 ]]; then
    echo "ok   - the initrd carries the Intel microcode ($ucode_size bytes)"
else
    echo "FAIL - the initrd carries the Intel microcode ($ucode_name: size '${ucode_size:-missing}', want over 1000000)"
    fail=1
fi

if [[ $fail -eq 0 ]]; then
    echo "PASSED"
else
    echo "FAILED"
    exit 1
fi
