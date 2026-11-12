#!/usr/bin/env python3
"""Pick the Intel WiFi firmware files the kernel's iwlwifi driver can load.

The driver builds a file name from the card it finds and tries API versions
from its maximum down; the first file that exists is loaded. `modinfo -F
firmware` lists the names with the maximum API. This module keeps, per
series of files on disk, the one file the driver loads and names the others
to drop. The rule is section O1 of
docs/superpowers/specs/2026-11-12-image-size-design.md.

Usage: iwlwifi_keep.py MODINFO_FILE FIRMWARE_DIR
prints the basenames in FIRMWARE_DIR to drop, one per line.
"""

import os
import re
import sys

FILE_RE = re.compile(r"^iwlwifi-(?P<stem>.+)-(?P<c>c?)(?P<api>\d+)\.ucode(\.zst)?$")
NAME_RE = re.compile(r"^iwlwifi-(?P<stem>.+)-(?P<c>c?)(?P<api>\d+|[A-Z_]+)\.ucode$")

# A modinfo changed beyond recognition must not prune blind.
MIN_NAMES = 20


class TooFewNames(ValueError):
    pass


class Missing:
    """No modinfo entry for the family: the ceiling is unknown."""

    def __repr__(self):
        return "MISSING"


MISSING = Missing()


def family(stem):
    return stem.split("-")[0]


def radio(stem):
    tokens = stem.split("-")
    return tokens[-2] if len(tokens) >= 4 else None


def _parse(names):
    out = []
    for n in names:
        m = NAME_RE.match(n.strip())
        if m:
            out.append(m)
    return out


def ceilings(names):
    """Map (family, radio, c) and (family, c) to the highest numeric API
    that modinfo names, or None when one of the entries is a macro."""
    table = {}
    for m in _parse(names):
        stem, c, api = m["stem"], m["c"], m["api"]
        value = int(api) if api.isdigit() else None
        keys = [(family(stem), c)]
        if radio(stem) is not None:
            keys.append((family(stem), radio(stem), c))
        for key in keys:
            if key in table and table[key] is None:
                continue
            if value is None or key not in table:
                table[key] = value
            else:
                table[key] = max(table[key], value)
    return table


def ceiling_for(stem, c, table):
    """Radio ceiling first, then the family ceiling, else MISSING."""
    r = radio(stem)
    if r is not None and (family(stem), r, c) in table:
        return table[(family(stem), r, c)]
    return table.get((family(stem), c), MISSING)


def select(names, files):
    """Return (keep, drop), two lists of basenames from files."""
    parsed = _parse(names)
    if len(parsed) < MIN_NAMES:
        raise TooFewNames(len(parsed))
    table = ceilings(names)
    series = {}
    keep = []
    for f in files:
        m = FILE_RE.match(f)
        if not m:
            keep.append(f)
            continue
        series.setdefault((m["stem"], m["c"]), []).append((int(m["api"]), f))
    drop = []
    for (stem, c), members in sorted(series.items()):
        members.sort()
        ceiling = ceiling_for(stem, c, table)
        if ceiling is None:
            keep += [f for _, f in members]
            continue
        if ceiling is MISSING:
            best = members[-1][1]
        else:
            loadable = [f for n, f in members if n <= ceiling]
            if not loadable:
                keep += [f for _, f in members]
                continue
            best = loadable[-1]
        keep.append(best)
        drop += [f for _, f in members if f != best]
    return sorted(keep), sorted(drop)


def main(argv):
    if len(argv) != 3:
        sys.stderr.write("usage: iwlwifi_keep.py MODINFO_FILE FIRMWARE_DIR\n")
        return 2
    with open(argv[1]) as fh:
        names = fh.read().splitlines()
    files = sorted(
        e.name for e in os.scandir(argv[2]) if e.is_file(follow_symlinks=False)
    )
    try:
        _, drop = select(names, files)
    except TooFewNames as e:
        sys.stderr.write("iwlwifi_keep: only %d firmware names from modinfo\n" % e.args[0])
        return 2
    for f in drop:
        print(f)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
