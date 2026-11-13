#!/usr/bin/env python3
"""Pick the AMD graphics firmware that AMD APUs load (Carrizo, Stoney,
Ryzen).

amdgpu loads firmware for the older APUs by chip name (carrizo_*, raven_*,
renoir_*, ...) and for the newer ones by IP version found through IP discovery
(gc_X_Y_Z_*, psp_X_Y_Z_*, dcn_X_Y_Z_*, vcn_X_Y_Z*, sdma_X_Y_Z). The IP
versions of every APU come from the kernel's own table,
Documentation/gpu/amdgpu/apu-asic-info-table.csv, copied next to this
script. Firmware for graphics cards is dropped. The rule is section A1 of
docs/superpowers/specs/2026-11-12-image-size-design.md.

Usage: amdgpu_keep.py APU_CSV DGPU_CSV MODINFO_FILE FIRMWARE_DIR
prints the names in FIRMWARE_DIR to drop, one per line. Exit 2 when the
table or the selection is too small, exit 3 when the package ships files
for an IP version neither table knows. When both hold, both reasons are
printed and the exit is 3. On exit 2 or 3 nothing goes to stdout.
"""

import csv
import io
import os
import re
import sys

# Chips amdgpu loads firmware for by name. carrizo and stoney (A-series
# APUs, HP t530 and t630 thin clients; owner, 2026-11-13) are the
# CARRIZO/STONEY row of the table, which names no IP versions.
CHIPS = ("carrizo", "stoney", "raven", "raven2", "picasso", "renoir", "green_sardine", "yellow_carp", "vangogh")
APU_ONLY = ("vpe", "umsch_mm", "isp")
# Versions with files in the package that the dGPU table does not list: the
# MI300 to MI350 accelerators.
DGPU_EXTRAS = ("gc_9_4_4", "gc_9_5_0", "psp_13_0_12", "psp_13_0_14", "sdma_4_4_4", "sdma_4_4_5", "vcn_5_0_1")
GUARDED = ("gc", "psp", "dcn", "vcn", "sdma")

# A moved or reformatted table must not prune blind.
MIN_ROWS = 15
MIN_KEEP = 100

# Table column -> firmware block name, and the tags a cell may carry for it.
COLUMNS = {
    "DCN/DCE version": ("dcn", ("DCN",)),
    "GC version": ("gc", ()),
    "VCE/UVD/VCN version": ("vcn", ("VCN",)),
    "VCN version": ("vcn", ("VCN",)),
    "SDMA version": ("sdma", ()),
    "MP0 version": ("psp", ()),
}
# Its cells (DCE 11, GC 8, VCE 3 / UVD 6) give no IP prefix; CHIPS keeps it.
SKIP_CODES = ("CARRIZO/STONEY",)
CELL_RE = re.compile(r"^(?:(?P<tag>[A-Z]+)\s+)?(?P<ver>\d+(?:\.\d+){1,2})$")
VERSION_RE = re.compile(r"^(?P<block>gc|psp|dcn|vcn|sdma)_(?P<x>\d+)_(?P<y>\d+)(?:_(?P<z>\d+))?(?=_|$)")


class TooSmall(ValueError):
    pass


class UnknownVersions(ValueError):
    pass


def _rows(csv_text):
    lines = [l for l in csv_text.splitlines() if l.strip() and not l.startswith("#")]
    reader = csv.reader(io.StringIO("\n".join(lines)), skipinitialspace=True)
    header = [h.strip() for h in next(reader)]
    for row in reader:
        yield dict(zip(header, (c.strip() for c in row)))


def _versions(row):
    """(block, x, y, z) for every IP version a table row names."""
    out = []
    for column, (block, tags) in COLUMNS.items():
        for part in row.get(column, "").split("/"):
            m = CELL_RE.match(part.strip())
            if not m or (m["tag"] and m["tag"] not in tags):
                continue
            nums = [int(n) for n in m["ver"].split(".")] + [0]
            out.append((block, nums[0], nums[1], nums[2]))
    return out


def _table_versions(csv_text):
    found = set()
    for row in _rows(csv_text):
        if row.get("Code Reference") in SKIP_CODES:
            continue
        found.update(_versions(row))
    return found


def _prefixes(csv_text):
    """(three-part prefixes, two-part prefixes, number of APU rows)."""
    three, two = set(), set()
    rows = 0
    for row in _rows(csv_text):
        if row.get("Code Reference") in SKIP_CODES:
            continue
        rows += 1
        for block, x, y, z in _versions(row):
            three.add(f"{block}_{x}_{y}_{z}")
            if z == 0:
                two.add(f"{block}_{x}_{y}")
    return sorted(three), sorted(two), rows


def apu_prefixes(csv_text):
    """(three-part IP prefixes, two-part prefixes for patch level 0)."""
    three, two, rows = _prefixes(csv_text)
    if rows < MIN_ROWS:
        raise TooSmall(f"only {rows} APU rows in the table, expected at least {MIN_ROWS}")
    return three, two


def _stem(name):
    return name.split(".", 1)[0]


def _matches(stem, three, two):
    for p in CHIPS + APU_ONLY:
        if stem.startswith(p + "_"):
            return True
    for p in three:
        if stem == p or stem.startswith(p + "_"):
            return True
    for p in two:
        if stem.startswith(p + "_") and stem[len(p) + 1 : len(p) + 2].isalpha():
            return True
    return False


def _select(three, two, entries):
    """The kept names of entries, without the floors."""
    keep = {n for n in entries if _matches(_stem(n), three, two)}
    changed = True
    while changed:
        changed = False
        for n in list(keep):
            target = entries.get(n)
            if target is not None:
                # Targets are names in the same directory; anything else is
                # left to the dangling link check of the build.
                if target in entries and target not in keep:
                    keep.add(target)
                    changed = True
    return keep


def select(csv_text, entries):
    """entries maps each name in amdgpu/ to its link target or None.
    Returns (keep, drop), both sorted."""
    keep = _select(*apu_prefixes(csv_text), entries)
    if len(keep) < MIN_KEEP:
        raise TooSmall(f"only {len(keep)} files kept, expected at least {MIN_KEEP}")
    return sorted(keep), sorted(set(entries) - keep)


def wanted(csv_text, modinfo):
    """The names of modinfo (amdgpu/<name>.bin) under a kept prefix: after
    the prune each must still exist."""
    three, two = apu_prefixes(csv_text)
    out = []
    for name in modinfo:
        name = name.strip()
        if name.startswith("amdgpu/") and _matches(_stem(name[len("amdgpu/") :]), three, two):
            out.append(name)
    return sorted(set(out))


def _version_of(stem):
    m = VERSION_RE.match(stem)
    if not m:
        return None
    return (m["block"], int(m["x"]), int(m["y"]), int(m["z"] or 0))


def _fmt(v):
    return "%s_%d_%d_%d" % v


def unknown_versions(apu_csv, dgpu_csv, modinfo, entries, keep):
    known = _table_versions(apu_csv) | _table_versions(dgpu_csv)
    known.update(_version_of(e) for e in DGPU_EXTRAS)
    have = {_version_of(_stem(n)) for n in entries} - {None}
    bad = set()
    for name in modinfo:
        name = name.strip()
        if not name.startswith("amdgpu/"):
            continue
        v = _version_of(_stem(name[len("amdgpu/") :]))
        if v is not None and v in have and v not in known:
            bad.add(_fmt(v))
    kept = set(keep)
    for n in entries:
        v = _version_of(_stem(n))
        if v is None or v[0] != "psp":
            continue
        p = _fmt(v)
        if _stem(n) == p + "_toc" and n not in kept:
            if not any(_stem(e).startswith(p + "_sos") for e in entries):
                bad.add(p)
    return sorted(bad)


def _scan(firmware_dir):
    out = {}
    for name in os.listdir(firmware_dir):
        path = os.path.join(firmware_dir, name)
        out[name] = os.readlink(path) if os.path.islink(path) else None
    return out


def main(argv):
    if len(argv) != 5:
        sys.stderr.write(__doc__)
        return 64
    apu_csv, dgpu_csv, modinfo_file, firmware_dir = argv[1:]
    with open(apu_csv) as f:
        apu = f.read()
    with open(dgpu_csv) as f:
        dgpu = f.read()
    with open(modinfo_file) as f:
        modinfo = f.read().splitlines()
    entries = _scan(firmware_dir)
    small = None
    try:
        keep, drop = select(apu, entries)
    except TooSmall as e:
        small = e
        # The guard below still needs a selection: take the unchecked one.
        three, two, _ = _prefixes(apu)
        keep = sorted(_select(three, two, entries))
    if small is not None:
        sys.stderr.write(f"amdgpu_keep: {small}\n")
    unknown = unknown_versions(apu, dgpu, modinfo, entries, keep)
    if unknown:
        sys.stderr.write(
            "amdgpu_keep: unknown amdgpu IP versions: %s; refresh the tables from the kernel\n"
            % " ".join(unknown)
        )
        return 3
    if small is not None:
        return 2
    for n in drop:
        print(n)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
