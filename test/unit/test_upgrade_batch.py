#!/usr/bin/env python3
"""Offline tests for the batch interface of thinfloe-upgrade."""

import importlib.machinery
import importlib.util
import hashlib
import errno
import io
import json
import os
import subprocess
import threading
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

class TestStreamImage(unittest.TestCase):
    SIZE = 70 * MIB

    @classmethod
    def setUpClass(cls):
        cls._tmp = tempfile.TemporaryDirectory()
        base = Path(cls._tmp.name)
        cls.raw = os.urandom(cls.SIZE)
        (base / "raw.img").write_bytes(cls.raw)
        subprocess.run(["zstd", "-q", "-1", str(base / "raw.img"), "-o", str(base / "i.zst")],
                       check=True)
        cls.zst = base / "i.zst"
        cls.url = "file://" + str(cls.zst)
        cls.zst_bytes = cls.zst.read_bytes()

    @classmethod
    def tearDownClass(cls):
        cls._tmp.cleanup()

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.dev = str(Path(self.dir.name) / "dev")
        Path(self.dev).write_bytes(b"")
        self.out = io.StringIO()
        self.progress = ui.Progress(self.out, True, 0)

    def lines(self):
        return [l for l in self.out.getvalue().splitlines() if l.startswith("@progress")]

    def test_writes_and_hashes(self):
        r = ui.stream_image(self.url, self.dev, self.SIZE, self.progress)
        self.assertEqual(Path(self.dev).read_bytes(), self.raw)
        self.assertEqual(r.written, self.SIZE)
        self.assertEqual(r.image_sha256, hashlib.sha256(self.raw).hexdigest())
        self.assertEqual(r.download_sha256, hashlib.sha256(self.zst_bytes).hexdigest())

    def test_syncs_in_step(self):
        real = os.fdatasync
        calls = []
        with mock.patch.object(ui.os, "fdatasync", lambda fd: (calls.append(fd), real(fd))[1]):
            ui.stream_image(self.url, self.dev, self.SIZE, self.progress)
        self.assertGreaterEqual(len(calls), 3)

    def test_progress_monotonic_and_final(self):
        ui.stream_image(self.url, self.dev, self.SIZE, self.progress)
        done = [int(l.split()[1]) for l in self.lines()]
        self.assertEqual(done, sorted(done))
        self.assertEqual(self.lines()[-1], "@progress 73400320 73400320")

    def test_unknown_total_reports_dash(self):
        ui.stream_image(self.url, self.dev, None, self.progress)
        self.assertTrue(self.lines())
        for l in self.lines():
            self.assertTrue(l.endswith(" -"), l)

    def test_missing_file_is_download_failure(self):
        with self.assertRaises(ui.WriteFailed) as cm:
            ui.stream_image("file://" + self.dir.name + "/nope.zst", self.dev,
                            None, self.progress)
        self.assertIn("download", cm.exception.reason)
        self.assertIn("curl: (37)", cm.exception.reason)

    def test_curl_has_a_stall_limit(self):
        argv = []
        real_popen = subprocess.Popen

        def spy(a, *args, **kw):
            argv.append(a)
            return real_popen(a, *args, **kw)

        with mock.patch.object(ui.subprocess, "Popen", spy):
            ui.stream_image(self.url, self.dev, self.SIZE, self.progress)
        curl = argv[0]
        i = curl.index("--speed-limit")
        self.assertEqual(curl[i:i + 4], ["--speed-limit", "1", "--speed-time", "60"])

    def test_closed_progress_is_not_a_stick_error(self):
        self.progress.progress = mock.Mock(side_effect=BrokenPipeError())
        with self.assertRaises(BrokenPipeError):
            ui.stream_image(self.url, self.dev, self.SIZE, self.progress)

    def test_truncated_image_fails(self):
        cut = Path(self.dir.name) / "cut.zst"
        cut.write_bytes(self.zst_bytes[:len(self.zst_bytes) // 2])
        with self.assertRaises(ui.WriteFailed) as cm:
            ui.stream_image("file://" + str(cut), self.dev, None, self.progress)
        self.assertIn("unpacked", cm.exception.reason)

    def test_interrupt_propagates_and_cleans_up(self):
        procs = []
        real_popen = subprocess.Popen

        def spy(*a, **k):
            p = real_popen(*a, **k)
            procs.append(p)
            return p

        before = threading.active_count()
        self.progress.progress = mock.Mock(side_effect=KeyboardInterrupt())
        with mock.patch.object(ui.subprocess, "Popen", spy):
            with self.assertRaises(KeyboardInterrupt):
                ui.stream_image(self.url, self.dev, self.SIZE, self.progress)
        self.assertEqual(len(procs), 2)
        for p in procs:
            self.assertIsNotNone(p.returncode)
        self.assertEqual(threading.active_count(), before)

    def test_write_error_raises_and_leaves_no_child(self):
        real = os.write
        n = [0]

        def flaky(fd, data):
            n[0] += 1
            if n[0] > 2:
                raise OSError(errno.EIO, "Input/output error")
            return real(fd, data)

        procs = []
        real_popen = subprocess.Popen

        def spy(*a, **k):
            p = real_popen(*a, **k)
            procs.append(p)
            return p

        before = threading.active_count()
        with mock.patch.object(ui.os, "write", flaky), \
                mock.patch.object(ui.subprocess, "Popen", spy):
            with self.assertRaises(ui.WriteFailed) as cm:
                ui.stream_image(self.url, self.dev, self.SIZE, self.progress)
        self.assertIn(self.dev, cm.exception.reason)
        self.assertEqual(len(procs), 2)
        for p in procs:
            self.assertIsNotNone(p.returncode)
        self.assertEqual(threading.active_count(), before)


class TestVerify(unittest.TestCase):
    SIZE = 70 * MIB

    @classmethod
    def setUpClass(cls):
        cls._tmp = tempfile.TemporaryDirectory()
        cls.raw = os.urandom(cls.SIZE)
        cls.path = os.path.join(cls._tmp.name, "dev.img")
        Path(cls.path).write_bytes(cls.raw)
        cls.sha = hashlib.sha256(cls.raw).hexdigest()

    @classmethod
    def tearDownClass(cls):
        cls._tmp.cleanup()

    def progress(self):
        out = io.StringIO()
        return ui.Progress(out, True, 0), out

    def test_matching_content_verifies(self):
        p, out = self.progress()
        self.assertTrue(ui.verify_device(self.path, self.SIZE, self.sha, p))
        self.assertIn("@progress %d %d" % (self.SIZE, self.SIZE), out.getvalue())

    def test_flipped_byte_fails(self):
        bad = os.path.join(self._tmp.name, "bad.img")
        data = bytearray(self.raw)
        data[40 * MIB] ^= 1
        Path(bad).write_bytes(bytes(data))
        p, _ = self.progress()
        self.assertFalse(ui.verify_device(bad, self.SIZE, self.sha, p))

    def test_short_device_fails(self):
        short = os.path.join(self._tmp.name, "short.img")
        Path(short).write_bytes(self.raw[:10 * MIB])
        p, _ = self.progress()
        with mock.patch.object(ui, "log"):
            self.assertFalse(ui.verify_device(short, self.SIZE, self.sha, p))

    def test_unreadable_device_fails_and_is_logged(self):
        p, _ = self.progress()
        with mock.patch.object(ui, "log") as lg:
            self.assertFalse(ui.verify_device(
                os.path.join(self._tmp.name, "missing"), 10, self.sha, p))
        self.assertIn("missing", lg.call_args[0][0])

    def test_drop_cache_on_regular_file_skips_ioctl(self):
        fd = os.open(self.path, os.O_RDONLY)
        try:
            with mock.patch("fcntl.ioctl") as ioc, \
                 mock.patch("os.posix_fadvise") as adv:
                ui.drop_cache(fd)
        finally:
            os.close(fd)
        ioc.assert_not_called()
        adv.assert_called_once_with(fd, 0, 0, os.POSIX_FADV_DONTNEED)

    def test_drop_cache_on_block_device_flushes(self):
        fake = mock.Mock(st_mode=0o060000)
        with mock.patch("os.fstat", return_value=fake), \
             mock.patch("fcntl.ioctl") as ioc, mock.patch("os.posix_fadvise"):
            ui.drop_cache(7)
        ioc.assert_called_once_with(7, 0x1261)

    def test_drop_cache_failures_are_ignored(self):
        fake = mock.Mock(st_mode=0o060000)
        with mock.patch("os.fstat", return_value=fake), \
             mock.patch("fcntl.ioctl", side_effect=OSError(1, "no")), \
             mock.patch("os.posix_fadvise", side_effect=OSError(1, "no")), \
             mock.patch.object(ui, "log") as lg:
            ui.drop_cache(7)
        self.assertEqual(lg.call_count, 2)


class TestExpectedDownloadSha(unittest.TestCase):
    HEX = "ab" * 32

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.dir = Path(self._tmp.name)
        self.base = "file://" + self._tmp.name

    def put(self, text, name="thinfloe-usb.img.zst"):
        (self.dir / (name + ".sha256")).write_text(text)

    def get(self, name="thinfloe-usb.img.zst"):
        return ui.expected_download_sha256(self.base, name)

    def test_two_space_format(self):
        self.put(self.HEX + "  thinfloe-usb.img.zst\n")
        self.assertEqual(self.get(), self.HEX)

    def test_binary_marker_and_uppercase(self):
        self.put(self.HEX.upper() + " *thinfloe-usb.img.zst\n")
        self.assertEqual(self.get(), self.HEX)

    def test_other_file_name(self):
        self.put(self.HEX + "  other.zst\n")
        self.assertIsNone(self.get())

    def test_missing_file(self):
        self.assertIsNone(self.get())

    def test_malformed(self):
        for text in ("", "zz" * 32 + "  thinfloe-usb.img.zst\n",
                     "ab" * 31 + "  thinfloe-usb.img.zst\n",
                     self.HEX + "\n",
                     self.HEX + "  thinfloe-usb.img.zst extra\n"):
            self.put(text)
            self.assertIsNone(self.get(), text)


if __name__ == "__main__":
    unittest.main()
