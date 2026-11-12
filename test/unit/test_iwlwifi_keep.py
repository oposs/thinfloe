#!/usr/bin/env python3
"""Offline tests for containers/ubuntu/slim/iwlwifi_keep.py.

The fixtures come from the 4.2.0-era image: modinfo.txt is
`modinfo -F firmware` over the Intel WiFi modules, files.txt the basenames in
usr/lib/firmware/intel/iwlwifi/.
"""

import importlib.util
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
FIXTURES = HERE / "iwlwifi-fixtures"

_spec = importlib.util.spec_from_file_location(
    "iwlwifi_keep", ROOT / "containers" / "ubuntu" / "slim" / "iwlwifi_keep.py"
)
iwlwifi_keep = importlib.util.module_from_spec(_spec)
# No __pycache__ next to the script: containers/ubuntu/slim/ is copied into
# the image and every file in it is a build prerequisite.
_write_bytecode = sys.dont_write_bytecode
sys.dont_write_bytecode = True
try:
    _spec.loader.exec_module(iwlwifi_keep)
finally:
    sys.dont_write_bytecode = _write_bytecode


def _lines(name):
    return [
        line.strip()
        for line in (FIXTURES / name).read_text().splitlines()
        if line.strip()
    ]


NAMES = _lines("modinfo.txt")
FILES = _lines("files.txt")


class SelectTest(unittest.TestCase):
    def setUp(self):
        self.keep, self.drop = iwlwifi_keep.select(NAMES, FILES)

    def assertKept(self, *names):
        for n in names:
            self.assertIn(n, self.keep)

    def assertDropped(self, *names):
        for n in names:
            self.assertIn(n, self.drop)

    def test_keeps_highest_under_ceiling(self):
        self.assertKept("iwlwifi-so-a0-gf-a0-89.ucode.zst")
        self.assertDropped("iwlwifi-so-a0-gf-a0-86.ucode.zst")
        self.assertKept("iwlwifi-8265-36.ucode.zst")
        self.assertDropped("iwlwifi-8265-34.ucode.zst")
        self.assertKept("iwlwifi-cc-a0-77.ucode.zst")
        self.assertDropped("iwlwifi-cc-a0-74.ucode.zst")

    def test_ceiling_cuts_newer_core_files(self):
        self.assertKept("iwlwifi-sc-a0-wh-b0-c101.ucode.zst")
        self.assertDropped(
            "iwlwifi-sc-a0-wh-b0-c102.ucode.zst",
            "iwlwifi-sc-a0-wh-b0-c103.ucode.zst",
        )

    def test_unresolved_ceiling_keeps_all(self):
        self.assertIsNone(iwlwifi_keep.ceilings(NAMES)[("bz", "c")])
        for n in ("101", "102", "103"):
            self.assertKept(
                "iwlwifi-bz-b0-fm-c0-c%s.ucode.zst" % n,
                "iwlwifi-gl-c0-fm-c0-c%s.ucode.zst" % n,
            )

    def test_unnamed_family_keeps_highest(self):
        self.assertKept("iwlwifi-gl-c0-fm-c0-101.ucode.zst")
        self.assertDropped("iwlwifi-gl-c0-fm-c0-100.ucode.zst")

    def test_legacy_and_pnvm_kept(self):
        self.assertKept(
            "iwlwifi-3945-2.ucode.zst",
            "iwlwifi-4965-2.ucode.zst",
            "iwlwifi-6000-4.ucode.zst",
        )
        pnvm = [f for f in FILES if f.endswith(".pnvm.zst")]
        self.assertTrue(pnvm)
        self.assertKept(*pnvm)

    def test_series_above_ceiling_kept_whole(self):
        series = [f for f in FILES if f.startswith("iwlwifi-sc-a0-wh-b0-1")]
        self.assertEqual(series, ["iwlwifi-sc-a0-wh-b0-101.ucode.zst"])
        self.assertEqual(iwlwifi_keep.ceilings(NAMES)[("sc", "")], 100)
        self.assertKept(
            "iwlwifi-sc-a0-wh-b0-101.ucode.zst",
            "iwlwifi-sc-a0-wh-b0-c101.ucode.zst",
        )

    def test_too_few_names_refused(self):
        with self.assertRaises(iwlwifi_keep.TooFewNames):
            iwlwifi_keep.select(NAMES[:5], FILES)

    def test_radio_ceiling_wins(self):
        keep, drop = iwlwifi_keep.select(
            NAMES, FILES + ["iwlwifi-so-a0-jf-b0-83.ucode.zst"]
        )
        self.assertIn("iwlwifi-so-a0-jf-b0-77.ucode.zst", keep)
        self.assertIn("iwlwifi-so-a0-jf-b0-83.ucode.zst", drop)

    def test_fixture_totals(self):
        self.assertEqual(len(FILES), 174)
        self.assertEqual(len(self.drop), 111)
        self.assertEqual(len(self.keep), 63)
        self.assertEqual(sorted(self.keep + self.drop), sorted(FILES))


class LookupTest(unittest.TestCase):
    def test_radio_then_family(self):
        table = iwlwifi_keep.ceilings(NAMES)
        self.assertEqual(iwlwifi_keep.ceiling_for("so-a0-jf-b0", "", table), 77)
        self.assertEqual(iwlwifi_keep.ceiling_for("so-a0-gf4-a0", "", table), 100)
        self.assertEqual(iwlwifi_keep.ceiling_for("8265", "", table), 36)
        self.assertIs(
            iwlwifi_keep.ceiling_for("gl-c0-fm-c0", "", table), iwlwifi_keep.MISSING
        )
        self.assertIsNone(iwlwifi_keep.ceiling_for("bz-b0-fm-c0", "c", table))


if __name__ == "__main__":
    unittest.main()
