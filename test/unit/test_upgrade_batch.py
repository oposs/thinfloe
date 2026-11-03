#!/usr/bin/env python3
"""Offline tests for the batch interface of thinfloe-upgrade."""

import importlib.machinery
import importlib.util
import hashlib
import io
import json
import tempfile
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


LSBLK = json.dumps({"blockdevices": [
    {"name": "sdb", "path": "/dev/sdb", "size": 16000000000, "model": "Stick ",
     "serial": "SER1", "wwn": None, "tran": "usb", "rm": True, "type": "disk",
     "mountpoints": [None],
     "children": [{"name": "sdb1", "path": "/dev/sdb1", "size": 1, "model": None,
                   "serial": None, "wwn": None, "tran": None, "rm": True,
                   "type": "part", "mountpoints": ["/media/x"]}]},
    {"name": "nvme0n1", "path": "/dev/nvme0n1", "size": 500000000000,
     "model": "Internal", "serial": None, "wwn": "eui.1", "tran": "nvme",
     "rm": False, "type": "disk", "mountpoints": [None]},
    {"name": "vda", "path": "/dev/vda", "size": 8000000000, "model": None,
     "serial": None, "wwn": None, "tran": None, "rm": False, "type": "disk",
     "mountpoints": [None]},
    {"name": "loop0", "path": "/dev/loop0", "size": 1000, "model": None,
     "serial": None, "wwn": None, "tran": None, "rm": True, "type": "loop",
     "mountpoints": [None]},
]})


def fake_cmd(cmd):
    if cmd[0] == "lsblk" and "-J" in cmd:
        return LSBLK
    return ""


class TestDriveList(unittest.TestCase):
    def list(self, **kw):
        with mock.patch.object(ui, "get_cmd_output", side_effect=fake_cmd), \
             mock.patch.object(ui, "detect_boot_device", return_value="/dev/vda"), \
             mock.patch.object(ui, "find_efi_partition", return_value=None), \
             mock.patch.object(ui, "ptable_fingerprint", return_value="ab" * 8):
            return ui.enumerate_drives(**kw)

    def test_default_is_usb_and_boot(self):
        self.assertEqual([d.device for d in self.list()], ["/dev/sdb", "/dev/vda"])

    def test_include_internal(self):
        self.assertEqual([d.device for d in self.list(include_internal=True)],
                         ["/dev/sdb", "/dev/nvme0n1", "/dev/vda"])

    def test_mounted_partition(self):
        d = {x.device: x for x in self.list(include_internal=True)}
        self.assertTrue(d["/dev/sdb"].mounted)
        self.assertFalse(d["/dev/vda"].mounted)
        self.assertEqual(d["/dev/sdb"].serial, "SER1")
        self.assertEqual(d["/dev/sdb"].size_bytes, 16000000000)
        self.assertTrue(d["/dev/vda"].is_boot)

    def test_fingerprint(self):
        data = bytes(range(256)) * 80  # 20480 bytes, more than 17408
        with tempfile.NamedTemporaryFile() as f:
            f.write(data)
            f.flush()
            want = hashlib.sha256(data[:17408]).hexdigest()[:16]
            self.assertEqual(ui.ptable_fingerprint(f.name), want)
        self.assertEqual(ui.ptable_fingerprint("/nonexistent/dev"), "-")

    def test_identity_differs_by_fingerprint(self):
        a, b = self.list()[:2]
        a2 = ui.DriveInfo(**{**a.__dict__, "fingerprint": "00" * 8})
        self.assertNotEqual(ui.drive_identity(a), ui.drive_identity(a2))
        self.assertEqual(ui.drive_identity(a), "SER1:16000000000:" + "ab" * 8)

    def test_json(self):
        drives = self.list()
        rows = json.loads(ui.drives_json(drives, 3148873728))
        self.assertEqual(rows[0]["device"], "/dev/sdb")
        self.assertEqual(rows[0]["id"], ui.drive_identity(drives[0]))
        self.assertTrue(rows[0]["fits"])
        self.assertTrue(rows[0]["mounted"])
        small = ui.DriveInfo(**{**drives[0].__dict__, "size_bytes": 2 * 10**9})
        self.assertFalse(json.loads(ui.drives_json([small], 3148873728))[0]["fits"])
        self.assertIsNone(json.loads(ui.drives_json(drives, None))[0]["fits"])
        self.assertEqual(set(rows[0]), {
            "device", "model", "size_bytes", "serial", "transport", "removable",
            "mounted", "is_boot", "is_thinfloe", "has_tl_old", "has_tl_new",
            "efi_partition", "id", "fits"})


if __name__ == "__main__":
    unittest.main()
