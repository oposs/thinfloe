#!/usr/bin/env python3
"""Offline tests for the batch interface of thinfloe-upgrade."""

import importlib.machinery
import importlib.util
import io
import unittest
from pathlib import Path
from unittest import mock

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "overlay" / "usr" / "bin" / "thinfloe-upgrade"


def load_upgrade_image():
    spec = importlib.util.spec_from_loader(
        "upgrade_image_batch",
        importlib.machinery.SourceFileLoader("upgrade_image_batch", str(SCRIPT)),
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ui = load_upgrade_image()
MIB = 1024 * 1024


class TestProgress(unittest.TestCase):
    def make(self, batch, interval=0.5):
        out = io.StringIO()
        return ui.Progress(out, batch, interval), out

    def test_batch_lines(self):
        p, out = self.make(True, 0)
        p.step("write", "Writing")
        p.progress(10, 100)
        p.progress(10, None)
        p.done("write")
        p.result(False, "the stick was removed")
        self.assertEqual(
            out.getvalue(),
            "@step write Writing\n@progress 10 100\n@progress 10 -\n"
            "@done write\n@result failed the stick was removed\n",
        )

    def test_batch_result_ok(self):
        p, out = self.make(True)
        p.result(True, "")
        self.assertTrue(out.getvalue().startswith("@result ok"))
        self.assertEqual(out.getvalue().count("\n"), 1)

    def test_newlines_become_spaces(self):
        p, out = self.make(True)
        p.step("a", "one\ntwo")
        p.result(False, "x\r\ny")
        self.assertEqual(out.getvalue().count("\n"), 2)
        self.assertIn("one two", out.getvalue())

    def test_rate_limit_batch(self):
        p, out = self.make(True, 10)
        with mock.patch.object(ui.time, "monotonic", side_effect=[100.0, 100.1, 100.2]):
            p.progress(1, 100)
            p.progress(2, 100)
            p.progress(100, 100)
        self.assertEqual(out.getvalue(), "@progress 1 100\n@progress 100 100\n")

    def test_interactive_step_and_ok(self):
        p, out = self.make(False)
        p.step("write", "Writing")
        p.result(True, "all good")
        self.assertEqual(out.getvalue(), "==> Writing\n")

    def test_interactive_progress_cr_line(self):
        p, out = self.make(False, 0)
        p.progress(5 * MIB, 10 * MIB)
        self.assertEqual(out.getvalue(), "\r  5 of 10 MiB")
        p.progress(10 * MIB, 10 * MIB)
        self.assertEqual(out.getvalue(), "\r  5 of 10 MiB\r  10 of 10 MiB\n")
        self.assertNotIn("@", out.getvalue())

    def test_interactive_progress_no_total(self):
        p, out = self.make(False, 0)
        p.progress(3 * MIB, None)
        self.assertEqual(out.getvalue(), "\r  3 MiB")

    def test_interactive_rate_limit(self):
        p, out = self.make(False, 10)
        with mock.patch.object(ui.time, "monotonic", side_effect=[100.0, 100.1]):
            p.progress(1 * MIB, 9 * MIB)
            p.progress(2 * MIB, 9 * MIB)
        self.assertEqual(out.getvalue().count("\r"), 1)

    def test_interactive_done_closes_open_line(self):
        p, out = self.make(False, 0)
        p.progress(1 * MIB, None)
        p.done("write")
        self.assertEqual(out.getvalue(), "\r  1 MiB\n")
        p.done("write")
        self.assertEqual(out.getvalue(), "\r  1 MiB\n")

    def test_interactive_failure_closes_line_and_reports(self):
        p, out = self.make(False, 0)
        p.progress(1 * MIB, None)
        p.result(False, "the stick was removed")
        self.assertEqual(out.getvalue(), "\r  1 MiB\nFailed: the stick was removed\n")

    def test_global_default_is_interactive(self):
        self.assertFalse(ui.PROGRESS.batch)


if __name__ == "__main__":
    unittest.main()
