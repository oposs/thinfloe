#!/bin/bash
# Regression test for the firmware-stripping bug.
#
# scripts/export-rootfs.sh used to run
#     find /usr/lib/firmware -type f -not -name "iwlwifi*" -delete
# immediately before mksquashfs. That deleted every firmware file except
# iwlwifi*, which left deployed clients without i915 GPU firmware (wedged
# GPU, no runtime power management) and without CPU microcode. Today the
# squashfs leaves the microcode out on purpose, because the initrd carries it
# (O3 in the same spec); the test asserts that it is absent.
#
# It also guards the one Intel WiFi firmware cut (O1 in
# docs/superpowers/specs/2026-11-12-image-size-design.md): per series only
# the ucode the driver loads, checked against the modinfo of the Intel WiFi
# modules in the squashfs with a parser of its own, no dangling firmware
# symlink, and a ucode for each chip in the field.
#
# And the AMD graphics firmware cut (A1 in the same spec): firmware for each
# AMD APU (Carrizo, Stoney, Ryzen), from a list written out here independent
# of amdgpu_keep.py, and
# none for graphics cards (navi10) or for the radeon driver.
#
# Usage: test/integration/test-firmware.sh [path/to/ramroot.sqsh]

set -uo pipefail

SQSH="${1:-artifacts/rootfs/ramroot.sqsh}"

if [[ ! -f "$SQSH" ]]; then
    echo "FAIL: $SQSH not found - run 'make rootfs' first" >&2
    exit 1
fi

if ! command -v unsquashfs >/dev/null 2>&1; then
    echo "FAIL: unsquashfs not installed (apt install squashfs-tools)" >&2
    exit 1
fi

# unsquashfs starts a thread per core; the build box is shared.
unsquashfs() { command unsquashfs -processors "${BUILD_JOBS:-4}" "$@"; }

if ! command -v modinfo >/dev/null 2>&1; then
    echo "FAIL: modinfo not installed (apt install kmod)" >&2
    exit 1
fi

WORK=$(mktemp -d)
trap 'chmod -R u+rwX "$WORK" 2>/dev/null; rm -rf "$WORK"' EXIT
LIST=$WORK/list
LLS=$WORK/lls

if ! unsquashfs -l "$SQSH" >"$LIST" 2>/dev/null \
    || ! unsquashfs -lls "$SQSH" >"$LLS" 2>/dev/null; then
    echo "FAIL: cannot list $SQSH" >&2
    exit 1
fi

fail=0
check() {
    local desc="$1" pattern="$2"
    if grep -qE -- "$pattern" "$LIST"; then
        echo "ok   - $desc"
    else
        echo "FAIL - $desc (no match for: $pattern)"
        fail=1
    fi
}

# Alder Lake GPU firmware - the exact files the field dmesg reported missing.
# Files ship zstd-compressed, so the patterns intentionally omit any suffix.
check "i915 DMC firmware (adlp_dmc.bin)"     'usr/lib/firmware/i915/adlp_dmc\.bin'
check "i915 GuC firmware (adlp_guc_70.bin)"  'usr/lib/firmware/i915/adlp_guc_70\.bin'
# The squashfs once lost the microcode by accident (the sweep above). Now it
# leaves it out on purpose: dracut put the early microcode archive into the
# initrd (test-initramfs-drivers.sh checks it there) and the kernel applies it
# from there, so the copy under intel-ucode is dead weight.
if grep -qE 'usr/lib/firmware/intel-ucode' "$LIST"; then
    echo "FAIL - no intel-ucode in the squashfs (it is applied from the initrd)"
    fail=1
else
    echo "ok   - no intel-ucode in the squashfs (it is applied from the initrd)"
fi
# On 26.04 the real iwlwifi files live under intel/iwlwifi/ with top-level
# compat symlinks; assert the real path.
check "Intel WiFi firmware"                  'usr/lib/firmware/intel/iwlwifi/iwlwifi-'
check "Intel SOF audio firmware"             'usr/lib/firmware/intel/sof/'
check "Realtek NIC firmware"                 'usr/lib/firmware/rtl_nic/'

absent() {
    local desc="$1" pattern="$2" hit
    hit=$(grep -m 1 -E -- "$pattern" "$LIST")
    if [[ -n $hit ]]; then
        echo "FAIL - $desc (found: $hit)"
        fail=1
    else
        echo "ok   - $desc"
    fi
}

# AMD graphics firmware: per APU the files amdgpu loads for it (by
# chip name for the older APUs, by IP version for the newer ones).
AMD='usr/lib/firmware/amdgpu/'
while read -r apu names; do
    for n in $names; do
        check "AMD APU firmware for $apu ($n)" "${AMD}${n}\.bin(\.zst)?$"
    done
done <<'APUS'
Carrizo carrizo_me
Stoney stoney_me
Raven raven_dmcu
Picasso picasso_vcn
Renoir renoir_dmcub
Cezanne/Barcelo green_sardine_dmcub
Rembrandt yellow_carp_dmcub
Raphael gc_10_3_6_me dcn_3_1_5_dmcub
Mendocino gc_10_3_7_me dcn_3_1_6_dmcub
Phoenix gc_11_0_1_mes_2 dcn_3_1_4_dmcub psp_13_0_4_toc vcn_4_0_2
Hawk-Point gc_11_0_4_me psp_13_0_11_toc
Strix gc_11_5_0_me dcn_3_5_dmcub psp_14_0_0_toc
Strix-Halo gc_11_5_1_me dcn_3_5_1_dmcub vcn_4_0_6_1
Krackan gc_11_5_2_me gc_11_5_3_me dcn_3_6_dmcub psp_14_0_4_toc psp_14_0_5_toc
APUS
absent "no AMD graphics card firmware (navi10)" "${AMD}navi10_"
absent "no radeon firmware" 'usr/lib/firmware/radeon(/|$)'

# Intel WiFi chips in the field or likely there: each keeps a ucode.
IWL='usr/lib/firmware/intel/iwlwifi/iwlwifi-'
for chip in 7265D 8265 9260-th-b0-jf-b0 cc-a0 Qu-c0-hr-b0 QuZ-a0-hr-b0 \
    so-a0-gf-a0 ty-a0-gf-a0 ma-b0-gf-a0 bz-b0-fm-c0 gl-c0-fm-c0; do
    check "Intel WiFi ucode for $chip" "${IWL}${chip}-c?[0-9]+\.ucode"
done
check "Intel WiFi pnvm for so-a0-gf-a0" "${IWL}so-a0-gf-a0\.pnvm"

# The firmware names the Intel WiFi drivers ask for, read with the host's
# modinfo from the modules in the squashfs.
NAMES=$WORK/names
: >"$NAMES"
INTEL=$(grep -oE 'usr/lib/modules/[^/]+/kernel/drivers/net/wireless/intel/' "$LIST" | head -n 1)
mods=()
for m in iwlwifi/iwlwifi iwlwifi/mvm/iwlmvm iwlwifi/dvm/iwldvm iwlwifi/mld/iwlmld \
    iwlegacy/iwl3945 iwlegacy/iwl4965; do
    path=$(grep -xE "squashfs-root/${INTEL}${m}\.ko(\.zst)?" "$LIST" | head -n 1)
    if [[ -n $path ]]; then
        mods+=("${path#squashfs-root/}")
    else
        echo "FAIL - Intel WiFi module ${m##*/} in the squashfs"
        fail=1
    fi
done
if [[ ${#mods[@]} -gt 0 ]]; then
    printf '%s\n' "${mods[@]}" >"$WORK/ef"
    if unsquashfs -q -n -d "$WORK/root" -ef "$WORK/ef" "$SQSH" >/dev/null 2>&1; then
        for m in "${mods[@]}"; do
            modinfo -F firmware "$WORK/root/$m" >>"$NAMES" \
                || { echo "FAIL - modinfo reads ${m##*/}"; fail=1; }
        done
    else
        echo "FAIL - extract the Intel WiFi modules from $SQSH"
        fail=1
    fi
fi

# Series rule and symlinks, written apart from iwlwifi_keep.py on purpose so
# that a bug there is not repeated here.
python3 -I - "$NAMES" "$LLS" <<'PYEOF' || fail=1
import re
import sys

names = open(sys.argv[1]).read().split()
bad = False


def report(desc, offenders):
    global bad
    if offenders:
        print("FAIL - %s: %s" % (desc, " ".join(sorted(offenders))))
        bad = True
    else:
        print("ok   - %s" % desc)


def keys(stem, c):
    """Lookup order: (stem, c), then (family, radio, c), then (family, c)."""
    t = stem.split("-")
    radio = [(t[0], t[-2], c)] if len(t) >= 4 else []
    return [("stem", stem, c)] + radio + [(t[0], c)]


# Ceilings from modinfo; None stands for an unexpanded macro.
ceil = {}
parsed = 0
odd = []
for n in names:
    m = re.match(r"iwlwifi-(.+)-(c?)(\d+|[A-Z][A-Z0-9_]*)\.ucode$", n)
    if not m:
        if n.startswith("iwlwifi-") and not n.endswith(".pnvm"):
            odd.append(n)
        continue
    parsed += 1
    stem, c, api = m.groups()
    for k in keys(stem, c):
        v = int(api) if api.isdigit() else None
        old = ceil.get(k, -1)
        ceil[k] = None if v is None or old is None else max(old, v)
report("modinfo names at least 20 Intel WiFi ucode files (%d)" % parsed,
       [] if parsed >= 20 else ["too-few"])
report("every Intel WiFi firmware name in modinfo has a known form", odd)

# The listing as path -> (type, link target).
entries = {}
for line in open(sys.argv[2]):
    m = re.match(r"(\S)\S*\s+\S+\s+\S+\s+\S+\s+\S+\s+squashfs-root/(.*)$",
                 line.rstrip("\n"))
    if not m:
        continue
    kind, rest = m.groups()
    if kind == "l":
        path, _, target = rest.partition(" -> ")
    else:
        path, target = rest, None
    entries[path] = (kind, target)

IWLDIR = "usr/lib/firmware/intel/iwlwifi/"
series = {}
for p, (kind, _) in entries.items():
    base = p[len(IWLDIR):]
    if kind != "-" or not p.startswith(IWLDIR) or "/" in base:
        continue
    m = re.match(r"iwlwifi-(.+)-(c?)(\d+)\.ucode(\.zst)?$", base)
    if m:
        stem, c, n, _ = m.groups()
        series.setdefault((stem, c), []).append((int(n), base))

several, above, unnamed = [], [], []
for (stem, c), files in series.items():
    found = [k for k in keys(stem, c) if k in ceil]
    if not found:
        # No modinfo entry for the family: the rule keeps the highest only.
        if len(files) > 1:
            unnamed += [f for _, f in files]
        continue
    if ceil[found[0]] is None:
        continue
    top = ceil[found[0]]
    if not any(n <= top for n, _ in files):
        continue  # no loadable file: the rule keeps the series whole
    if len(files) > 1:
        several += [f for _, f in files]
    above += [f for n, f in files if n > top]
report("at most one Intel WiFi ucode per series with a known ceiling", several)
report("no Intel WiFi ucode above its modinfo ceiling", above)
report("at most one Intel WiFi ucode per series of a family modinfo does not name", unnamed)


def resolves(path):
    parts = path.split("/")
    done = []
    hops = 0
    while parts:
        part = parts.pop(0)
        if part in ("", "."):
            continue
        if part == "..":
            if done:
                done.pop()
            continue
        e = entries.get("/".join(done + [part]))
        if e is None:
            return False
        if e[0] == "l":
            hops += 1
            if hops > 40:
                return False
            if e[1].startswith("/"):
                done = []
            parts = e[1].split("/") + parts
            continue
        done.append(part)
    return True


links = [p for p, (kind, _) in entries.items()
         if kind == "l" and p.startswith("usr/lib/firmware/")]
report("no dangling symlink under usr/lib/firmware (%d links)" % len(links),
       [p for p in links if not resolves(p)])
sys.exit(1 if bad else 0)
PYEOF

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - shipped image firmware is missing or not pruned"
    exit 1
fi

echo ""
echo "PASSED - all expected firmware present"
exit 0
