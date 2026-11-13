#!/usr/bin/env python3
"""Offline tests for containers/ubuntu/slim/amdgpu_keep.py.

The fixtures come from linux-firmware-amd-graphics
20260319.git217ca6e4-0ubuntu3.1 and kernel 7.0.0-43: files.txt lists the
entries of usr/lib/firmware/amdgpu/ (a link as `name -> target`),
modinfo.txt is `modinfo -F firmware amdgpu`. The tables are the copies in
containers/ubuntu/slim/.
"""

import importlib.util
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
SLIM = ROOT / "containers" / "ubuntu" / "slim"
FIXTURES = HERE / "amdgpu-fixtures"

_spec = importlib.util.spec_from_file_location("amdgpu_keep", SLIM / "amdgpu_keep.py")
amdgpu_keep = importlib.util.module_from_spec(_spec)
# No __pycache__ next to the script: containers/ubuntu/slim/ is copied into
# the image and every file in it is a build prerequisite.
_write_bytecode = sys.dont_write_bytecode
sys.dont_write_bytecode = True
try:
    _spec.loader.exec_module(amdgpu_keep)
finally:
    sys.dont_write_bytecode = _write_bytecode


def _lines(name):
    return [
        line.strip()
        for line in (FIXTURES / name).read_text().splitlines()
        if line.strip()
    ]


def _entries():
    out = {}
    for line in _lines("files.txt"):
        name, sep, target = line.partition(" -> ")
        out[name] = target if sep else None
    return out


APU_CSV = (SLIM / "amdgpu-apu-ip.csv").read_text()
DGPU_CSV = (SLIM / "amdgpu-dgpu-ip.csv").read_text()
MODINFO = _lines("modinfo.txt")
ENTRIES = _entries()


class SelectTest(unittest.TestCase):
    def setUp(self):
        self.keep, self.drop = amdgpu_keep.select(APU_CSV, ENTRIES)

    def assertKept(self, *names):
        for n in names:
            self.assertIn(n, ENTRIES, n)
            self.assertIn(n, self.keep)

    def test_newer_apus_by_ip(self):
        self.assertKept(
            "dcn_3_5_dmcub.bin.zst",
            "dcn_3_6_dmcub.bin.zst",
            "dcn_3_5_1_dmcub.bin.zst",
            "gc_11_5_0_mes_2.bin.zst",
            "psp_14_0_0_toc.bin.zst",
            "gc_11_0_4_me.bin.zst",
            "isp_4_1_1.bin.zst",
            "vpe_6_1_0.bin.zst",
            "umsch_mm_4_0_0.bin.zst",
        )

    def test_older_apus_by_chip(self):
        self.assertKept(
            "raven_dmcu.bin.zst",
            "picasso_vcn.bin.zst",
            "renoir_dmcub.bin.zst",
            "green_sardine_vcn.bin.zst",
            "yellow_carp_dmcub.bin.zst",
            "vangogh_toc.bin.zst",
        )

    def test_shared_file_kept_through_link(self):
        self.assertEqual(ENTRIES["vcn_4_0_2.bin.zst"], "vcn_4_0_0.bin.zst")
        self.assertKept("vcn_4_0_2.bin.zst", "vcn_4_0_0.bin.zst")

    def test_dgpu_dropped(self):
        prefixes = ("navi10_", "gc_11_0_0_", "psp_13_0_0_", "smu_13_0_0", "polaris10_", "carrizo_")
        want = [n for n in ENTRIES if n.startswith(prefixes)]
        self.assertGreater(len(want), 30)
        for n in want:
            self.assertIn(n, self.drop)
            self.assertNotIn(n, self.keep)

    def test_fixture_totals(self):
        self.assertEqual(len(ENTRIES), 667)
        self.assertEqual(len(self.keep), 178)
        self.assertEqual(len(self.drop), 489)
        self.assertEqual(set(self.keep) | set(self.drop), set(ENTRIES))

    def test_wanted_names_all_kept(self):
        want = amdgpu_keep.wanted(APU_CSV, MODINFO)
        self.assertEqual(len(want), 177)
        for name in want:
            self.assertIn(name[len("amdgpu/") :] + ".zst", self.keep)

    def test_short_table_refused(self):
        lines = [l for l in APU_CSV.splitlines() if not l.startswith("#")]
        short = "\n".join(lines[:6]) + "\n"
        with self.assertRaises(amdgpu_keep.TooSmall):
            amdgpu_keep.select(short, ENTRIES)


class UnknownVersionsTest(unittest.TestCase):
    def run_guard(self, extra):
        entries = dict(ENTRIES)
        entries.update(extra)
        keep, _ = amdgpu_keep.select(APU_CSV, entries)
        return amdgpu_keep.unknown_versions(APU_CSV, DGPU_CSV, MODINFO, entries, keep)

    def test_known_versions_pass(self):
        for name in ("gc_11_5_4", "sdma_6_1_4", "psp_15_0_0", "psp_15_0_8", "vcn_5_3_0"):
            self.assertTrue(any(m.startswith("amdgpu/" + name) for m in MODINFO), name)
        self.assertEqual(self.run_guard({}), [])

    def test_new_gc_version_refused(self):
        self.assertEqual(self.run_guard({"gc_11_5_4_me.bin.zst": None}), ["gc_11_5_4"])

    def test_unkept_apu_psp_refused(self):
        got = self.run_guard({"psp_15_0_0_toc.bin.zst": None, "psp_15_0_0_ta.bin.zst": None})
        self.assertIn("psp_15_0_0", got)


if __name__ == "__main__":
    unittest.main()
