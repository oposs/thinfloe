#!/usr/bin/env python3
"""Offline tests for the batch interface of thinfloe-upgrade."""

import importlib.machinery
import importlib.util
import hashlib
import contextlib
import errno
import io
import json
import os
import signal
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


class TestRemovableFlag(unittest.TestCase):
    def test_string_zero_is_not_removable(self):
        # Old util-linux prints rm as "0"/"1"; bool("0") would list internal disks.
        old = json.dumps({"blockdevices": [
            {"name": "sdc", "path": "/dev/sdc", "size": 1000, "model": None,
             "serial": None, "wwn": None, "tran": "sata", "rm": "0",
             "type": "disk", "mountpoints": [None]},
            {"name": "sdd", "path": "/dev/sdd", "size": 1000, "model": None,
             "serial": None, "wwn": None, "tran": None, "rm": "1",
             "type": "disk", "mountpoints": [None]},
        ]})

        def cmd(c):
            return old if c[0] == "lsblk" and "-J" in c else ""
        with mock.patch.object(ui, "get_cmd_output", side_effect=cmd), \
             mock.patch.object(ui, "detect_boot_device", return_value=None), \
             mock.patch.object(ui, "find_efi_partition", return_value=None), \
             mock.patch.object(ui, "ptable_fingerprint", return_value="-"):
            self.assertEqual([d.device for d in ui.enumerate_drives()], ["/dev/sdd"])


IMG = 3148873728


class TestBatchRefusals(unittest.TestCase):
    def setUp(self):
        self.stream = mock.patch.object(
            ui, "stream_image", side_effect=AssertionError("stream_image called"))
        self.stream.start()
        self.addCleanup(self.stream.stop)
        for name in ("ensure_root",):
            m = mock.patch.object(ui, name)
            m.start()
            self.addCleanup(m.stop)
        self.drives = mock.patch.object(
            ui, "enumerate_drives", side_effect=self._enum)
        self.drives.start()
        self.addCleanup(self.drives.stop)
        self.size = mock.patch.object(ui, "image_size", return_value=IMG)
        self.size.start()
        self.addCleanup(self.size.stop)
        self.restore = mock.patch.object(ui, "home_restore_failed", return_value=False)
        self.restore.start()
        self.addCleanup(self.restore.stop)
        self.fresh = mock.patch.object(ui, "batch_fresh", return_value=0)
        self.upgrade = mock.patch.object(ui, "batch_upgrade", return_value=0)
        self.m_fresh = self.fresh.start()
        self.m_upgrade = self.upgrade.start()
        self.addCleanup(self.fresh.stop)
        self.addCleanup(self.upgrade.stop)
        self.saved_progress = ui.PROGRESS
        self.addCleanup(setattr, ui, "PROGRESS", self.saved_progress)

    def _enum(self, include_internal=False):
        out = [self.mk("/dev/sdb", mounted=False, thinfloe=True),
               self.mk("/dev/sdc", mounted=True, thinfloe=True),
               self.mk("/dev/sdd", mounted=False, thinfloe=False),
               self.mk("/dev/sde", size=2 * 10**9),
               self.mk("/dev/vda", boot=True, thinfloe=True)]
        if include_internal:
            out.append(self.mk("/dev/nvme0n1", size=500 * 10**9))
        return out

    @staticmethod
    def mk(device, size=16 * 10**9, mounted=False, thinfloe=False, boot=False):
        return ui.DriveInfo(
            device=device, size="?", model="M", label="", is_thinfloe=thinfloe,
            efi_partition=None, part_size_mb=0, is_boot=boot, has_tl_old=False,
            has_tl_new=False, size_bytes=size, serial="S" + device[-1],
            transport="usb", removable=True, mounted=mounted, fingerprint="ab" * 8)

    def ident(self, device):
        return ui.drive_identity(
            [d for d in self._enum(True) if d.device == device][0])

    def run_write(self, device, mode="fresh", expect=None, *extra):
        if expect is None:
            expect = self.ident(device)
        argv = ["write", "--device", device, "--mode", mode,
                "--expect-id", expect, *extra]
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = ui.batch_main(argv)
        return rc, out.getvalue()

    def assertRefused(self, rc, out, *words):
        self.assertEqual(rc, 2)
        lines = [l for l in out.splitlines() if l.startswith("@result")]
        self.assertEqual(len(lines), 1, out)
        self.assertTrue(lines[0].startswith("@result failed "), out)
        for w in words:
            self.assertIn(w, lines[0])
        self.m_fresh.assert_not_called()
        self.m_upgrade.assert_not_called()

    def test_expect_id_mismatch_refuses(self):
        rc, out = self.run_write("/dev/sdb", "fresh", "WRONG:1:2")
        self.assertRefused(rc, out, "does not match", "/dev/sdb")

    def test_unlisted_device_refuses(self):
        rc, out = self.run_write("/dev/sdz", "fresh", "x")
        self.assertRefused(rc, out, "/dev/sdz")

    def test_internal_disk_needs_allow_internal(self):
        rc, out = self.run_write("/dev/nvme0n1", "fresh", "S1:500000000000:" + "ab" * 8)
        self.assertRefused(rc, out)
        rc, out = self.run_write("/dev/nvme0n1", "fresh", None, "--allow-internal")
        self.assertEqual(rc, 0)
        self.m_fresh.assert_called_once()

    def test_fresh_on_boot_drive_refuses(self):
        rc, out = self.run_write("/dev/vda", "fresh")
        self.assertRefused(rc, out, "/dev/vda", "boot")

    def test_fresh_on_mounted_drive_refuses(self):
        rc, out = self.run_write("/dev/sdc", "fresh")
        self.assertRefused(rc, out, "/dev/sdc", "mounted")

    def test_fresh_when_image_does_not_fit_refuses(self):
        rc, out = self.run_write("/dev/sde", "fresh")
        self.assertRefused(rc, out, "/dev/sde")

    def test_fresh_with_unknown_size_goes_on(self):
        self.size.stop()
        self.addCleanup(self.size.start)
        with mock.patch.object(ui, "image_size", return_value=None):
            rc, out = self.run_write("/dev/sde", "fresh")
        self.assertEqual(rc, 0)
        self.m_fresh.assert_called_once()

    def test_fresh_size_check_uses_source_url(self):
        self.run_write("/dev/sdb", "fresh", None, "--source", "https://h/p/")
        ui.image_size.assert_called_with("https://h/p/thinfloe-usb.img.zst")

    def test_upgrade_on_non_thinfloe_refuses(self):
        rc, out = self.run_write("/dev/sdd", "upgrade")
        self.assertRefused(rc, out, "/dev/sdd", "ThinFloe")

    def test_upgrade_on_mounted_non_boot_drive_refuses(self):
        rc, out = self.run_write("/dev/sdc", "upgrade")
        self.assertRefused(rc, out, "/dev/sdc", "mounted")

    def test_upgrade_on_boot_drive_goes_on(self):
        rc, out = self.run_write("/dev/vda", "upgrade")
        self.assertEqual(rc, 0)
        self.m_upgrade.assert_called_once()

    def test_copy_home_with_upgrade_refuses(self):
        rc, out = self.run_write("/dev/sdb", "upgrade", None, "--copy-home")
        self.assertRefused(rc, out, "home")

    def test_copy_home_after_failed_restore_refuses(self):
        with mock.patch.object(ui, "home_restore_failed", return_value=True):
            rc, out = self.run_write("/dev/sdb", "fresh", None, "--copy-home")
        self.assertRefused(rc, out, "home")

    def test_accepted_fresh_calls_batch_fresh(self):
        rc, out = self.run_write("/dev/sdb", "fresh", None, "--verify")
        self.assertEqual(rc, 0)
        drive, args, progress = self.m_fresh.call_args.args
        self.assertEqual(drive.device, "/dev/sdb")
        self.assertTrue(args.verify)
        self.assertEqual(args.source, ui.DEFAULT_URL)
        self.assertTrue(ui.PROGRESS.batch)
        self.assertIs(progress, ui.PROGRESS)

    def test_unknown_mode_is_argparse_error(self):
        rc, out = self.run_write("/dev/sdb", "bogus", "x")
        self.assertEqual(rc, 2)
        self.assertTrue(out.startswith("@result failed "), out)
        self.assertEqual(out.count("\n"), 1)

    def test_abbreviated_option_is_refused(self):
        rc, out = self.run_write("/dev/sdb", "fresh", None, "--ver")
        self.assertEqual(rc, 2)
        self.assertTrue(out.startswith("@result failed "), out)
        self.m_fresh.assert_not_called()

    def test_unexpected_exception_becomes_a_result(self):
        self.m_fresh.side_effect = RuntimeError("boom\nsecond line")
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = ui.batch_main(["write", "--device", "/dev/sdb", "--mode",
                                "fresh", "--expect-id", self.ident("/dev/sdb")])
        self.assertEqual(rc, 1)
        self.assertIn("@result failed RuntimeError: boom second line\n", out.getvalue())
        self.assertIn("Traceback", err.getvalue())

    def test_cancelled_gives_130(self):
        self.m_fresh.side_effect = ui.Cancelled()
        rc, out = self.run_write("/dev/sdb", "fresh")
        self.assertEqual(rc, 130)
        self.assertIn("@result failed cancelled", out)

    def test_sigterm_handler_is_installed_and_restored(self):
        import signal
        old = signal.getsignal(signal.SIGTERM)
        seen = []
        self.m_fresh.side_effect = lambda *a: seen.append(
            signal.getsignal(signal.SIGTERM)) or 0
        self.run_write("/dev/sdb", "fresh")
        self.assertTrue(callable(seen[0]) and seen[0] is not old)
        self.assertEqual(signal.getsignal(signal.SIGTERM), old)

    def test_drives_default_omits_internal(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = ui.batch_main(["drives"])
        self.assertEqual(rc, 0)
        devs = [r["device"] for r in json.loads(out.getvalue())]
        self.assertNotIn("/dev/nvme0n1", devs)
        self.assertIn("/dev/sdb", devs)

    def test_drives_all_includes_internal(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            ui.batch_main(["drives", "--all"])
        self.assertIn("/dev/nvme0n1", [r["device"] for r in json.loads(out.getvalue())])

    def test_drives_fits_null_when_size_unknown(self):
        out = io.StringIO()
        with mock.patch.object(ui, "image_size", return_value=None), \
             contextlib.redirect_stdout(out):
            ui.batch_main(["drives"])
        self.assertTrue(all(r["fits"] is None for r in json.loads(out.getvalue())))


class TestWipe(unittest.TestCase):
    def test_zeroes_first_and_last_mib_only(self):
        with tempfile.TemporaryDirectory() as t:
            p = os.path.join(t, "dev")
            Path(p).write_bytes(b"\xff" * (8 * MIB))
            self.assertTrue(ui.wipe_partition_tables(p))
            data = Path(p).read_bytes()
        self.assertEqual(len(data), 8 * MIB)
        self.assertEqual(data[:MIB], b"\0" * MIB)
        self.assertEqual(data[-MIB:], b"\0" * MIB)
        self.assertEqual(data[MIB:-MIB], b"\xff" * (6 * MIB))

    def test_small_device_is_zeroed_whole(self):
        with tempfile.TemporaryDirectory() as t:
            p = os.path.join(t, "dev")
            Path(p).write_bytes(b"\xff" * (MIB + 100))
            self.assertTrue(ui.wipe_partition_tables(p))
            self.assertEqual(Path(p).read_bytes(), b"\0" * (MIB + 100))

    def test_missing_device_returns_false_and_logs(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertFalse(ui.wipe_partition_tables("/nonexistent/dev"))
        self.assertIn("could not clear", out.getvalue())

    def test_sigterm_during_wipe_is_ignored(self):
        import signal
        old = signal.signal(signal.SIGTERM, lambda *a: (_ for _ in ()).throw(ui.Cancelled()))
        self.addCleanup(signal.signal, signal.SIGTERM, old)
        real = os.fdatasync

        def sync_and_term(fd):
            os.kill(os.getpid(), signal.SIGTERM)
            return real(fd)
        with tempfile.TemporaryDirectory() as t:
            p = os.path.join(t, "dev")
            Path(p).write_bytes(b"\xff" * (4 * MIB))
            with mock.patch.object(ui.os, "fdatasync", side_effect=sync_and_term):
                self.assertTrue(ui.wipe_partition_tables(p))
            self.assertEqual(Path(p).read_bytes()[:MIB], b"\0" * MIB)
        self.assertTrue(callable(signal.getsignal(signal.SIGTERM)))


class TestFreshInstall(unittest.TestCase):
    SHA = "ab" * 32

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dev = os.path.join(self.tmp.name, "dev")
        Path(self.dev).write_bytes(b"\xff" * (128 * MIB))
        self.calls = []
        self.drive = ui.DriveInfo(
            device=self.dev, size="?", model="M", label="", is_thinfloe=False,
            efi_partition=None, part_size_mb=0, is_boot=False, has_tl_old=False,
            has_tl_new=False, size_bytes=128 * MIB, serial="S", transport="usb",
            removable=True, mounted=False, fingerprint="ab" * 8)
        self.patches = {}
        for name, kw in (
            ("stream_image", dict(side_effect=self._stream)),
            ("verify_device", dict(return_value=True)),
            ("expected_download_sha256", dict(return_value="cd" * 32)),
            ("image_size", dict(return_value=MIB)),
            ("run_cmd", dict()),
            ("stamp_new_stick", dict(return_value=True)),
            ("copy_home_to_drive", dict(return_value=True)),
            ("save_home_before_finish", dict(return_value=0)),
        ):
            m = mock.patch.object(ui, name, **kw)
            self.patches[name] = m.start()
            self.addCleanup(m.stop)

    def _stream(self, url, device, total, progress):
        Path(device).write_bytes(b"\xee" * (128 * MIB))
        return ui.WriteResult(written=MIB, image_sha256=self.SHA,
                              download_sha256="cd" * 32)

    def run_fresh(self, copy_home=False, verify=False):
        import types
        args = types.SimpleNamespace(source="http://x", copy_home=copy_home,
                                     verify=verify, device=self.dev)
        out, err = io.StringIO(), io.StringIO()
        prog = ui.Progress(out, True, 0)
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = ui.batch_fresh(self.drive, args, prog)
        return rc, out.getvalue()

    def lines(self, out, kind):
        return [l for l in out.splitlines() if l.startswith(kind)]

    def assertWiped(self):
        data = Path(self.dev).read_bytes()
        self.assertEqual(data[:MIB], b"\0" * MIB)
        self.assertEqual(data[-MIB:], b"\0" * MIB)

    def assertNotWiped(self):
        data = Path(self.dev).read_bytes()
        self.assertEqual(data[:MIB], b"\xee" * MIB)

    def test_success_steps_in_order(self):
        rc, out = self.run_fresh(copy_home=True, verify=True)
        self.assertEqual(rc, 0, out)
        self.assertEqual(
            [l.split()[1] for l in self.lines(out, "@step")],
            ["write", "verify", "stamp", "home", "save"])
        self.assertEqual(
            [l.split()[1] for l in self.lines(out, "@done")],
            ["write", "verify", "stamp", "home", "save"])
        self.assertIn("@step write Downloading and writing", out)
        self.assertEqual(self.lines(out, "@result"), ["@result ok"])
        self.patches["stamp_new_stick"].assert_called_once_with(self.dev)
        self.patches["copy_home_to_drive"].assert_called_once_with(self.dev)
        self.patches["verify_device"].assert_called_once()
        self.assertEqual(self.patches["verify_device"].call_args.args[1:3],
                         (MIB, self.SHA))
        self.assertNotWiped()

    def test_verify_runs_before_stamp(self):
        order = []
        self.patches["verify_device"].side_effect = lambda *a: order.append("v") or True
        self.patches["stamp_new_stick"].side_effect = lambda *a: order.append("s") or True
        self.run_fresh(verify=True)
        self.assertEqual(order, ["v", "s"])

    def test_without_options_no_verify_no_home_step(self):
        rc, out = self.run_fresh()
        self.assertEqual(rc, 0)
        self.assertEqual([l.split()[1] for l in self.lines(out, "@step")],
                         ["write", "stamp", "save"])
        self.patches["verify_device"].assert_not_called()
        self.patches["copy_home_to_drive"].assert_not_called()

    def test_write_error_fails_and_wipes(self):
        self.patches["stream_image"].side_effect = ui.WriteFailed(
            "the stick stopped answering")
        rc, out = self.run_fresh()
        self.assertEqual(rc, 1)
        self.assertIn("@result failed the stick stopped answering", out)
        self.assertWiped()
        self.patches["stamp_new_stick"].assert_not_called()

    def test_wipe_failure_still_fails_and_is_logged(self):
        self.patches["stream_image"].side_effect = ui.WriteFailed("x")
        with mock.patch.object(ui, "wipe_partition_tables", return_value=False) as w:
            rc, out = self.run_fresh()
        w.assert_called_once_with(self.dev)
        self.assertEqual(rc, 1)
        self.assertIn("@result failed x", out)

    def test_wipe_failure_is_logged_for_real(self):
        self.patches["stream_image"].side_effect = ui.WriteFailed("x")
        Path(self.dev).unlink()
        self.patches["stream_image"].side_effect = ui.WriteFailed("x")
        rc, out = self.run_fresh()
        self.assertEqual(rc, 1)
        self.assertIn("could not clear", out)

    def test_curl_failure_fresh_wipes(self):
        self.patches["stream_image"].side_effect = ui.WriteFailed(
            "the download failed: 404")
        rc, out = self.run_fresh()
        self.assertEqual(rc, 1)
        self.assertWiped()

    def test_verify_false_wipes(self):
        self.patches["verify_device"].return_value = False
        rc, out = self.run_fresh(verify=True)
        self.assertEqual(rc, 1)
        self.assertIn("@result failed the stick did not read back what was written", out)
        self.assertWiped()
        self.patches["stamp_new_stick"].assert_not_called()

    def test_checksum_mismatch_wipes(self):
        self.patches["expected_download_sha256"].return_value = "00" * 32
        rc, out = self.run_fresh()
        self.assertEqual(rc, 1)
        self.assertIn("@result failed the download was damaged; write the stick again", out)
        self.assertWiped()
        self.patches["stamp_new_stick"].assert_not_called()

    def test_missing_checksum_is_logged_and_goes_on(self):
        self.patches["expected_download_sha256"].return_value = None
        rc, out = self.run_fresh()
        self.assertEqual(rc, 0)
        self.assertIn("checksum not checked: thinfloe-usb.img.zst.sha256 not found", out)
        self.assertNotWiped()

    def test_sigterm_during_write_wipes_and_gives_130(self):
        import signal

        def term(url, device, total, progress):
            Path(device).write_bytes(b"\xee" * (128 * MIB))
            os.kill(os.getpid(), signal.SIGTERM)
        self.patches["stream_image"].side_effect = term
        old = signal.getsignal(signal.SIGTERM)

        def handler(*a):
            raise ui.Cancelled()
        signal.signal(signal.SIGTERM, handler)
        self.addCleanup(signal.signal, signal.SIGTERM, old)
        rc, out = self.run_fresh()
        self.assertEqual(rc, 130)
        self.assertIn("@result failed cancelled", out)
        self.assertWiped()

    def test_stamp_failure_fails_job_but_keeps_stick(self):
        self.patches["stamp_new_stick"].return_value = False
        rc, out = self.run_fresh()
        self.assertEqual(rc, 1)
        self.assertIn("@result failed", out)
        self.assertIn("id", self.lines(out, "@result")[0])
        self.assertNotWiped()
        self.patches["save_home_before_finish"].assert_not_called()

    def test_home_copy_failure_reported_stick_kept(self):
        self.patches["copy_home_to_drive"].return_value = False
        rc, out = self.run_fresh(copy_home=True)
        self.assertEqual(rc, 1)
        self.assertIn("@result failed", out)
        self.assertIn("home", self.lines(out, "@result")[0])
        self.assertNotWiped()
        self.assertNotIn("@done home", out)

    def test_save_failure_reported_stick_kept(self):
        self.patches["save_home_before_finish"].return_value = 3
        rc, out = self.run_fresh()
        self.assertEqual(rc, 1)
        self.assertIn("@result failed", out)
        self.assertNotIn("@done save", out)
        self.assertNotWiped()

    def test_cancel_after_verify_does_not_wipe(self):
        self.patches["stamp_new_stick"].side_effect = ui.Cancelled()
        rc, out = self.run_fresh()
        self.assertEqual(rc, 130)
        self.assertIn("@result failed cancelled", out)
        self.assertNotWiped()


class TestCancelNeverLost(TestFreshInstall):  # noqa: inherits fixtures
    """R8/R9: a cancel survives helpers, a second signal, and interactive use."""

    def setUp(self):
        super().setUp()
        self.addCleanup(setattr, ui, "PROGRESS", ui.PROGRESS)
        # Interactive runs print through the module's PROGRESS.
        ui.PROGRESS = ui.Progress(io.StringIO(), False)

    def arm_sigterm(self):
        import signal
        self.old = signal.getsignal(signal.SIGTERM)
        self.addCleanup(signal.signal, signal.SIGTERM, self.old)

    def test_cancelled_is_not_an_exception(self):
        self.assertFalse(issubclass(ui.Cancelled, Exception))

    def test_sigterm_during_enumeration_helper_gives_130_without_writing(self):
        import signal
        self.arm_sigterm()

        def term_in_subprocess(*a, **kw):
            os.kill(os.getpid(), signal.SIGTERM)
            return b""
        out = io.StringIO()
        self.patches["run_cmd"].side_effect = term_in_subprocess
        with mock.patch.object(ui, "ensure_root"), \
             mock.patch.object(ui, "image_size", return_value=MIB), \
             contextlib.redirect_stdout(out):
            rc = ui.batch_main(["write", "--device", "/dev/sdb", "--mode",
                                "fresh", "--expect-id", "x"])
        self.assertEqual(rc, 130, out.getvalue())
        self.patches["stream_image"].assert_not_called()
        self.assertEqual(out.getvalue().count("@result"), 1)

    def test_second_sigterm_still_wipes(self):
        import signal
        self.arm_sigterm()
        signal.signal(signal.SIGTERM, ui._cancel_on_signal)
        real_wipe = ui.wipe_partition_tables

        def twice(url, device, total, progress):
            Path(device).write_bytes(b"\xee" * (128 * MIB))
            os.kill(os.getpid(), signal.SIGTERM)

        def wipe_after_second(device):
            os.kill(os.getpid(), signal.SIGTERM)  # relayed second signal
            return real_wipe(device)
        self.patches["stream_image"].side_effect = twice
        with mock.patch.object(ui, "wipe_partition_tables",
                               side_effect=wipe_after_second):
            rc, out = self.run_fresh()
        self.assertEqual(rc, 130)
        self.assertWiped()

    def test_second_sigterm_before_wipe_is_ignored_by_handler(self):
        import signal
        self.arm_sigterm()
        signal.signal(signal.SIGTERM, ui._cancel_on_signal)
        with self.assertRaises(ui.Cancelled):
            os.kill(os.getpid(), signal.SIGTERM)
            for _ in range(100):
                pass
        self.assertEqual(signal.getsignal(signal.SIGTERM), signal.SIG_IGN)

    def test_wipe_ignores_sigint_and_sighup_and_restores(self):
        import signal
        marks = {}
        real = os.fdatasync

        def sync(fd):
            marks.update({s: signal.getsignal(s) for s in
                          (signal.SIGTERM, signal.SIGHUP, signal.SIGINT)})
            return real(fd)
        before = {s: signal.getsignal(s) for s in marks or
                  (signal.SIGTERM, signal.SIGHUP, signal.SIGINT)}
        with mock.patch.object(ui.os, "fdatasync", side_effect=sync):
            ui.wipe_partition_tables(self.dev)
        self.assertEqual(set(marks.values()), {signal.SIG_IGN})
        for s, h in before.items():
            self.assertEqual(signal.getsignal(s), h)

    def test_interactive_keyboard_interrupt_wipes(self):
        self.patches["stream_image"].side_effect = KeyboardInterrupt()
        with mock.patch.object(ui, "check_drive_fits"), \
             mock.patch.object(ui, "confirm", side_effect=[True, False]), \
             contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaises(KeyboardInterrupt):
                ui.do_fresh_install(self.drive, "http://x")
        self.assertWiped()

    def test_interactive_sighup_wipes(self):
        import signal
        self.patches["stream_image"].side_effect = (
            lambda *a: os.kill(os.getpid(), signal.SIGHUP))
        old = signal.getsignal(signal.SIGHUP)
        self.addCleanup(signal.signal, signal.SIGHUP, old)
        with mock.patch.object(ui, "check_drive_fits"), \
             mock.patch.object(ui, "confirm", side_effect=[True, False]), \
             contextlib.redirect_stdout(io.StringIO()), \
             contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                ui.do_fresh_install(self.drive, "http://x")
        self.assertWiped()
        self.assertEqual(signal.getsignal(signal.SIGHUP), old)

    def test_only_one_result_line(self):
        out = io.StringIO()
        p = ui.Progress(out, True, 0)
        p.result(False, "a")
        p.result(False, "b")
        self.assertEqual(out.getvalue(), "@result failed a\n")


class TestBatchUpgrade(unittest.TestCase):
    CFG = "DEFAULT current\nLABEL current\n  KERNEL /tl/vmlinuz\n  APPEND ramroot=tl/ramroot.sqsh\n"

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = Path(self.tmp.name)
        self.esp = root / "esp"
        self.srv = root / "srv"
        (self.esp / "tl").mkdir(parents=True)
        (self.esp / "efi" / "boot").mkdir(parents=True)
        self.srv.mkdir()
        for f in ui.FIRMWARE_FILES:
            (self.esp / "tl" / f).write_text("old " + f)
            (self.srv / f).write_text("new " + f * 100)
        for c in self.cfgs():
            c.write_text(self.CFG)
        self.before = [c.read_bytes() for c in self.cfgs()]
        self.url = "file://" + str(self.srv)
        self.save = mock.Mock(return_value=0)
        for m in (
            mock.patch.object(ui, "BOOT_EFI_MOUNT", str(self.esp)),
            mock.patch.object(ui.os.path, "ismount", return_value=True),
            mock.patch.object(ui, "save_home_before_finish", self.save),
        ):
            m.start()
            self.addCleanup(m.stop)
        self.drive = ui.DriveInfo("/dev/sdz", "16G", "Test", "", True,
                                  "/dev/sdz1", 1024, True, False, False)

    def cfgs(self):
        return [self.esp / "syslinux.cfg", self.esp / "efi" / "boot" / "syslinux.cfg"]

    def checksums(self, bad=None):
        for f in ui.FIRMWARE_FILES:
            digest = hashlib.sha256((self.srv / f).read_bytes()).hexdigest()
            if f == bad:
                digest = "0" * 64
            (self.srv / (f + ".sha256")).write_text(f"{digest}  {f}\n")

    def run_upgrade(self):
        import types
        args = types.SimpleNamespace(source=self.url)
        out = io.StringIO()
        prog = ui.Progress(out, True, 0)
        old = signal.signal(signal.SIGTERM, ui._cancel_on_signal)
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
                try:
                    rc = ui.batch_upgrade(self.drive, args, prog)
                except ui.Cancelled:
                    prog.result(False, "cancelled")
                    rc = 130
        finally:
            signal.signal(signal.SIGTERM, old)
        return rc, out.getvalue()

    def assertStickUntouched(self):
        self.assertEqual([c.read_bytes() for c in self.cfgs()], self.before)
        self.assertFalse((self.esp / "tl.new").exists())
        for f in ui.FIRMWARE_FILES:
            self.assertEqual((self.esp / "tl" / f).read_text(), "old " + f)

    def test_success_steps_in_order(self):
        self.checksums()
        rc, out = self.run_upgrade()
        self.assertEqual(rc, 0, out)
        self.assertEqual([l.split()[1] for l in out.splitlines() if l.startswith("@step")],
                         ["save", "download", "check", "switch"])
        self.assertEqual([l.split()[1] for l in out.splitlines() if l.startswith("@done")],
                         ["save", "download", "check", "switch"])
        self.assertTrue(out.rstrip().endswith("@result ok"))
        total = sum((self.srv / f).stat().st_size for f in ui.FIRMWARE_FILES)
        self.assertIn(f"@progress {total} {total}", out)
        for c in self.cfgs():
            self.assertIn("DEFAULT new", c.read_text())
        for f in ui.FIRMWARE_FILES:
            self.assertEqual((self.esp / "tl.new" / f).read_bytes(), (self.srv / f).read_bytes())
            self.assertEqual((self.esp / "tl" / f).read_text(), "old " + f)

    def test_missing_checksum_is_logged_not_fatal(self):
        rc, out = self.run_upgrade()
        self.assertEqual(rc, 0, out)
        self.assertIn("checksum not checked: ramroot.sqsh.sha256 not found", out)

    def test_failed_save_stops_before_download(self):
        self.save.return_value = 5
        with mock.patch.object(ui, "download_firmware") as dl:
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 1)
        dl.assert_not_called()
        self.assertIn("@result failed the home directory could not be saved", out)
        self.assertNotIn("@step download", out)
        self.assertStickUntouched()

    def test_download_failure_leaves_stick(self):
        (self.srv / "ramroot.sqsh").unlink()
        rc, out = self.run_upgrade()
        self.assertEqual(rc, 1, out)
        self.assertIn("@result failed downloading ramroot.sqsh failed", out)
        self.assertNotIn("@step switch", out)
        self.assertStickUntouched()

    def test_checksum_mismatch_leaves_stick(self):
        self.checksums(bad="ramroot.sqsh")
        rc, out = self.run_upgrade()
        self.assertEqual(rc, 1, out)
        self.assertIn("@result failed the download of ramroot.sqsh was damaged", out)
        self.assertNotIn("@step switch", out)
        self.assertStickUntouched()

    def test_too_little_space_leaves_stick(self):
        with mock.patch.object(ui, "free_bytes", return_value=0):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 1, out)
        self.assertIn("Not enough space", out)
        self.assertStickUntouched()

    def test_sigterm_during_upgrade_keeps_stick_bootable(self):
        # The last file never arrives: curl blocks opening a fifo with no writer.
        (self.srv / "ramroot.sqsh").unlink()
        os.mkfifo(self.srv / "ramroot.sqsh")
        timer = threading.Timer(1.0, os.kill, (os.getpid(), signal.SIGTERM))
        timer.start()
        self.addCleanup(timer.cancel)
        with mock.patch.object(ui, "_content_length", return_value=None):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 130, out)
        self.assertIn("@result failed cancelled", out)
        self.assertStickUntouched()

    def test_a_failed_copy_removes_the_partial_tl_new(self):
        run = ui.run_cmd
        def fake(cmd, *a, **kw):
            if cmd[0] == "cp" and cmd[1].endswith("ramroot.sqsh"):
                raise subprocess.CalledProcessError(1, cmd)
            return run(cmd, *a, **kw)
        with mock.patch.object(ui, "run_cmd", side_effect=fake):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 1, out)
        self.assertIn("@result failed the new release could not be installed", out)
        self.assertStickUntouched()

    def test_the_switch_ignores_sigterm_and_restores_the_handler(self):
        real = ui.set_boot_default
        seen = []
        def term_then_set(*a):
            os.kill(os.getpid(), signal.SIGTERM)  # ignored during the switch
            seen.append(signal.getsignal(signal.SIGTERM))
            return real(*a)
        with mock.patch.object(ui, "set_boot_default", side_effect=term_then_set):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 0, out)
        self.assertEqual(seen, [signal.SIG_IGN])
        for c in self.cfgs():
            self.assertIn("DEFAULT new", c.read_text())


class TestEnsureRootBatch(unittest.TestCase):
    def test_batch_uses_stderr_and_sudo_n(self):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(ui.os, "geteuid", return_value=1000), \
             mock.patch.object(ui.os, "execvp") as ex, \
             mock.patch.object(ui.sys, "argv", ["thinfloe-upgrade", "drives"]), \
             contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            ui.ensure_root(batch=True)
        self.assertEqual(out.getvalue(), "")
        self.assertIn("Requesting root privileges", err.getvalue())
        ex.assert_called_once_with("sudo", ["sudo", "-n", "thinfloe-upgrade", "drives"])


if __name__ == "__main__":
    unittest.main()
