#!/usr/bin/env python3
"""Offline tests for the batch interface of thinfloe-upgrade: progress, drive
list, notes, image stream, checks and the rest. The shared fixtures are in
upgrade_fixtures.py."""

import importlib.machinery
import importlib.util
import hashlib
import contextlib
import argparse
import errno
import fcntl
import io
import json
import os
import shutil
import signal
import subprocess
import sys
import threading
import time
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from upgrade_fixtures import *  # noqa: F401,F403


class TestProgress(unittest.TestCase):
    def make(self, batch, interval=0.5):
        out = io.StringIO()
        return ui.Progress(out, batch, interval), out

    def test_touched_is_one_line_in_batch_and_silent_over_ssh(self):
        p, out = self.make(True)
        p.touched()
        self.assertEqual(out.getvalue(), "@touched\n")
        p, out = self.make(False)
        p.touched()
        self.assertEqual(out.getvalue(), "")

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
        data = bytes(range(256)) * 80  # no GPT: the MBR entries and signature only
        with tempfile.NamedTemporaryFile() as f:
            f.write(data)
            f.flush()
            want = hashlib.sha256(data[446:512]).hexdigest()[:16]
            self.assertEqual(ui.ptable_fingerprint(f.name), want)
        self.assertEqual(ui.ptable_fingerprint("/nonexistent/dev"), "-")

    @staticmethod
    def _fp(data: bytes) -> str:
        with tempfile.NamedTemporaryFile() as f:
            f.write(data)
            f.flush()
            return ui.ptable_fingerprint(f.name)

    @staticmethod
    def _superfloppy() -> bytearray:
        """FAT32 on the whole disk: boot sector, FSInfo, no partition table."""
        d = bytearray(64 * 1024)
        d[0:3] = b"\xeb\x58\x90"
        d[3:11] = b"mkfs.fat"
        d[0x41] = 0x00                      # BS_Reserved1: the dirty flag
        d[510:512] = b"\x55\xaa"
        d[512:516] = b"RRaA"                # FSInfo in sector 1
        d[512 + 488:512 + 492] = (1000).to_bytes(4, "little")  # free clusters
        d[1022:1024] = b"\x55\xaa"
        return d

    def test_fingerprint_ignores_a_mounted_superfloppy(self):
        # Mounting a whole-disk FAT sets the dirty flag and the umount writes
        # the free count: the stick is the same stick (it was refused as
        # "does not match the drive that was chosen" after its release).
        before = self._superfloppy()
        after = bytearray(before)
        after[0x41] = 0x01
        after[512 + 488:512 + 492] = (990).to_bytes(4, "little")
        after[2048:2052] = b"data"
        self.assertEqual(self._fp(bytes(before)), self._fp(bytes(after)))

    def test_fingerprint_changes_with_an_mbr_entry(self):
        d = self._superfloppy()
        d[446:462] = bytes([0x80, 0, 0, 0, 0x0C, 0, 0, 0]) + (2048).to_bytes(4, "little") \
            + (4096).to_bytes(4, "little")
        e = bytearray(d)
        e[454:458] = (4096).to_bytes(4, "little")   # the partition moved
        self.assertNotEqual(self._fp(bytes(d)), self._fp(bytes(e)))

    @staticmethod
    def _gpt() -> bytearray:
        d = bytearray(64 * 1024)
        d[446:462] = bytes([0, 0, 2, 0, 0xEE, 0xFF, 0xFF, 0xFF]) + (1).to_bytes(4, "little") \
            + (0xFFFFFFFF).to_bytes(4, "little")
        d[510:512] = b"\x55\xaa"
        h = 512
        d[h:h + 8] = b"EFI PART"
        d[h + 12:h + 16] = (92).to_bytes(4, "little")
        d[h + 72:h + 80] = (2).to_bytes(8, "little")      # entries at LBA 2
        d[h + 80:h + 84] = (128).to_bytes(4, "little")
        d[h + 84:h + 88] = (128).to_bytes(4, "little")
        d[1024:1040] = b"\x28\x73\x2a\xc1\x1f\xf8\xd2\x11\xba\x4b\x00\xa0\xc9\x3e\xc9\x3b"
        d[1024 + 32:1024 + 40] = (2048).to_bytes(8, "little")
        return d

    def test_fingerprint_changes_with_a_gpt_entry(self):
        d = self._gpt()
        e = bytearray(d)
        e[1024 + 32:1024 + 40] = (4096).to_bytes(8, "little")
        self.assertNotEqual(self._fp(bytes(d)), self._fp(bytes(e)))
        # The last of the 128 entries counts too.
        f = bytearray(d)
        f[1024 + 127 * 128] = 0x01
        self.assertNotEqual(self._fp(bytes(d)), self._fp(bytes(f)))

    def test_fingerprint_of_gpt_ignores_bytes_past_the_entries(self):
        d = self._gpt()
        e = bytearray(d)
        e[1024 + 128 * 128 + 10] = 0x77   # first partition data area
        self.assertEqual(self._fp(bytes(d)), self._fp(bytes(e)))

    def test_fingerprint_of_a_short_device(self):
        self.assertEqual(self._fp(b""), ui.ptable_fingerprint_of(lambda off, n: b""))
        self.assertNotEqual(self._fp(b""), "-")

    def test_identity_differs_by_fingerprint(self):
        a, b = self.list()[:2]
        a2 = ui.DriveInfo(**{**a.__dict__, "fingerprint": "00" * 8})
        self.assertNotEqual(ui.drive_identity(a), ui.drive_identity(a2))
        self.assertEqual(ui.drive_identity(a), "sn-SER1:16000000000:" + "ab" * 8)

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
            "mounted", "automounted", "is_boot", "is_thinfloe", "has_tl_old",
            "has_tl_new", "efi_partition", "examined", "id", "fits", "version"})


class TestNotes(unittest.TestCase):
    """thinfloe-upgrade notes: the CHANGES.md sections of a version range."""

    def setUp(self):
        # batch_main sets the module's PROGRESS; later tests expect the default.
        self.addCleanup(setattr, ui, "PROGRESS", ui.PROGRESS)

    def run_notes(self, argv, text=CHANGES, error=""):
        out, err = io.StringIO(), io.StringIO()
        urls = []

        def fetch(url):
            urls.append(url)
            return (text, error)
        with mock.patch.object(ui, "_fetch_notes", side_effect=fetch), \
             contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = ui.batch_main(["notes"] + argv)
        return rc, out.getvalue(), err.getvalue(), urls

    def test_cut_notes_range(self):
        cut = ui.cut_notes(CHANGES, "4.2.0", "4.4.0")
        self.assertIn("## 4.4.0", cut)
        self.assertIn("## 4.3.0", cut)
        self.assertNotIn("## 4.2.0", cut)
        self.assertNotIn("Unreleased", cut)
        self.assertTrue(cut.startswith("## 4.4.0"))

    def test_cut_notes_without_from_takes_all_up_to_target(self):
        cut = ui.cut_notes(CHANGES, None, "4.3.0")
        self.assertTrue(cut.startswith("## 4.3.0"))
        self.assertIn("## 4.2.0", cut)
        self.assertNotIn("## 4.4.0", cut)

    def test_cut_notes_target_only(self):
        cut = ui.cut_notes(CHANGES, None, "4.3.0", only=True)
        self.assertTrue(cut.startswith("## 4.3.0"))
        self.assertNotIn("## 4.4.0", cut)
        self.assertNotIn("## 4.2.0", cut)

    def test_cut_notes_dev_from(self):
        cut = ui.cut_notes(CHANGES, "4.3.0-dev+abc1234.dirty", "4.4.0")
        self.assertTrue(cut.startswith("## 4.4.0"))
        self.assertNotIn("## 4.3.0", cut)

    def test_cut_notes_old_from_takes_all(self):
        cut = ui.cut_notes(CHANGES, "3.9.1", "4.4.0")
        self.assertIn("## 4.2.0", cut)

    def test_cut_notes_without_target_heading_is_none(self):
        self.assertIsNone(ui.cut_notes(CHANGES, None, "9.9.9"))
        self.assertIsNone(ui.cut_notes("<html>Not Found</html>", None, "4.4.0"))

    def test_notes_prints_the_range(self):
        rc, out, err, urls = self.run_notes(["--from", "4.3.0", "--to", "4.4.0"])
        self.assertEqual(rc, 0, err)
        self.assertTrue(out.startswith("## 4.4.0"))
        self.assertNotIn("## 4.3.0", out)

    def test_notes_only_prints_the_target(self):
        rc, out, err, _ = self.run_notes(["--to", "4.3.0", "--only"])
        self.assertEqual(rc, 0, err)
        self.assertTrue(out.startswith("## 4.3.0"))
        self.assertNotIn("## 4.2.0", out)

    def test_notes_only_and_from_conflict(self):
        rc, _, _, urls = self.run_notes(["--to", "4.3.0", "--only", "--from", "4.2.0"])
        self.assertEqual((rc, urls), (2, []))

    def test_notes_fetch_url_pins_tag(self):
        _, _, _, urls = self.run_notes(["--to", "4.4.0"])
        self.assertEqual(urls, ["https://raw.githubusercontent.com/oposs/thinfloe/v4.4.0/CHANGES.md"])

    def test_notes_empty_range_exits_3(self):
        rc, out, _, _ = self.run_notes(["--from", "4.4.0", "--to", "4.4.0"])
        self.assertEqual((rc, out), (3, ""))
        rc, out, _, _ = self.run_notes(["--from", "4.5.0", "--to", "4.4.0"])
        self.assertEqual((rc, out), (3, ""))

    def test_notes_no_sections_fails(self):
        rc, out, err, _ = self.run_notes(["--to", "4.4.0"], text="<html>Not Found</html>")
        self.assertEqual((rc, out), (1, ""))
        self.assertIn("4.4.0", err)

    def test_notes_fetch_failure_fails(self):
        rc, out, err, _ = self.run_notes(["--to", "4.4.0"], text=None, error="could not connect")
        self.assertEqual((rc, out), (1, ""))
        self.assertIn("could not connect", err)

    def test_notes_too_big_fails(self):
        rc, out, err, _ = self.run_notes(["--to", "4.4.0"], text="x" * (ui.NOTES_BYTES + 1))
        self.assertEqual((rc, out), (1, ""))
        self.assertTrue(err.strip())

    def test_notes_rejects_bad_version(self):
        for argv in (["--to", "x y"], ["--to", "4.4.0", "--from", "-rf"], []):
            with self.subTest(argv=argv):
                rc, _, _, urls = self.run_notes(argv)
                self.assertEqual(rc, 2)
                self.assertEqual(urls, [])

    def test_notes_needs_no_root(self):
        with mock.patch.object(ui, "ensure_root", side_effect=AssertionError("root")):
            rc, _, err, _ = self.run_notes(["--to", "4.4.0"])
        self.assertEqual(rc, 0, err)


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

    def test_started_is_called_once_before_the_first_byte(self):
        seen = []
        ui.stream_image(self.url, self.dev, self.SIZE, self.progress,
                        started=lambda: seen.append(Path(self.dev).stat().st_size))
        self.assertEqual(seen, [0])

    def test_started_is_not_called_when_nothing_was_written(self):
        seen = []
        with self.assertRaises(ui.WriteFailed):
            ui.stream_image("file://" + self.dir.name + "/nope.zst", self.dev,
                            None, self.progress, started=lambda: seen.append(1))
        self.assertEqual(seen, [])

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

    def test_missing_checksum_note_names_what_was_missing(self):
        # Neither file: the note said only "<name>.sha256 not found".
        self.assertIsNone(self.get())
        self.assertEqual(ui.missing_checksum_note(self.base, "thinfloe-usb.img.zst"),
                         "neither SHA256SUMS nor thinfloe-usb.img.zst.sha256 was found")
        # SHA256SUMS without the line.
        other = Path(self._tmp.name) / "other"
        other.mkdir()
        (other / "SHA256SUMS").write_text(self.HEX + "  version\n")
        base = "file://" + str(other)
        self.assertIsNone(ui.expected_download_sha256(base, "thinfloe-usb.img.zst"))
        self.assertEqual(ui.missing_checksum_note(base, "thinfloe-usb.img.zst"),
                         "SHA256SUMS has no line for thinfloe-usb.img.zst, "
                         "and thinfloe-usb.img.zst.sha256 was not found")

    def test_sha256sums_is_fetched_once_per_run(self):
        (self.dir / "SHA256SUMS").write_text(
            "".join(f"{self.HEX}  {n}\n" for n in ("vmlinuz", "initrd.img", "ramroot.sqsh")))
        real = ui._fetch_small
        with mock.patch.object(ui, "_fetch_small", side_effect=real) as f:
            for n in ("vmlinuz", "initrd.img", "ramroot.sqsh"):
                self.assertEqual(ui.expected_download_sha256(self.base, n), self.HEX)
        urls = [c.args[0] for c in f.call_args_list]
        self.assertEqual(urls, [f"{self.base}/SHA256SUMS"])

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

    def sums(self, text):
        (self.dir / "SHA256SUMS").write_text(text)

    def test_sha256sums_line(self):
        self.sums("cd" * 32 + "  vmlinuz\n" + self.HEX.upper()
                  + " *thinfloe-usb.img.zst\n")
        self.assertEqual(self.get(), self.HEX)
        self.assertEqual(self.get("vmlinuz"), "cd" * 32)

    def test_sha256sums_missing_name_is_none(self):
        self.sums("cd" * 32 + "  vmlinuz\n")
        self.assertIsNone(self.get())

    def test_sha256sums_wins_over_single_file(self):
        self.sums(self.HEX + "  thinfloe-usb.img.zst\n")
        self.put("cd" * 32 + "  thinfloe-usb.img.zst\n")
        self.assertEqual(self.get(), self.HEX)

    def test_pinned_source_does_not_fall_back(self):
        pinned = self.dir / "download" / "v4.3.0"
        pinned.mkdir(parents=True)
        (pinned / "thinfloe-usb.img.zst.sha256").write_text(
            self.HEX + "  thinfloe-usb.img.zst\n")
        with mock.patch.object(ui, "RELEASE_DOWNLOAD", self.base + "/download"):
            self.assertIsNone(ui.expected_download_sha256(
                self.base + "/download/v4.3.0", "thinfloe-usb.img.zst"))
        # The same directory as a custom source still uses the old file.
        self.assertEqual(ui.expected_download_sha256(
            self.base + "/download/v4.3.0", "thinfloe-usb.img.zst"), self.HEX)


class TestParseSha256sums(unittest.TestCase):
    HEX = "ab" * 32

    def test_listed_twice_is_none(self):
        text = f"{self.HEX}  f\n{self.HEX}  f\n"
        self.assertIsNone(ui.parse_sha256sums(text, "f"))

    def test_malformed_line_is_none(self):
        for line in ("zz" * 32 + "  f", "ab" * 31 + "  f", self.HEX + " f",
                     self.HEX + "  f extra", self.HEX + "\tf"):
            self.assertIsNone(ui.parse_sha256sums(line + "\n", "f"), line)

    def test_other_lines_do_not_matter(self):
        text = f"garbage\n{self.HEX}  f\n{'cd' * 32}  g\n"
        self.assertEqual(ui.parse_sha256sums(text, "f"), self.HEX)


class TestPinSource(unittest.TestCase):
    def test_default_is_pinned_to_the_version_read(self):
        with mock.patch.object(ui, "_fetch_version",
                               return_value=("4.3.0", "")) as fv:
            got = ui.pin_source(ui.DEFAULT_URL)
        fv.assert_called_once_with(ui.DEFAULT_URL)
        self.assertEqual(got, ui.RELEASE_DOWNLOAD + "/v4.3.0")
        self.assertTrue(got.endswith("/download/v4.3.0"))
        self.assertTrue(ui.checksum_required(got))

    def test_custom_source_unchanged(self):
        with mock.patch.object(ui, "_fetch_version",
                               side_effect=AssertionError("version read")):
            self.assertEqual(ui.pin_source("https://h/p"), "https://h/p")
        self.assertFalse(ui.checksum_required("https://h/p"))

    def test_unreadable_version_fails(self):
        with mock.patch.object(ui, "_fetch_version",
                               return_value=("", "no version file at the source")):
            with self.assertRaises(ui.WriteFailed) as cm:
                ui.pin_source(ui.DEFAULT_URL)
        self.assertEqual(cm.exception.reason, "the newest release could not be "
                         "read: no version file at the source")

    def test_the_names_are_githubs(self):
        self.assertEqual(ui.DEFAULT_URL,
                         "https://github.com/oposs/thinfloe/releases/latest/download")
        self.assertEqual(ui.RELEASE_DOWNLOAD,
                         "https://github.com/oposs/thinfloe/releases/download")
        self.assertEqual(ui.CHECKSUM_ASSET, "SHA256SUMS")


class TestRedirects(unittest.TestCase):
    """GitHub answers latest/download/<name> with a chain of redirects."""

    @classmethod
    def setUpClass(cls):
        import http.server
        cls._tmp = tempfile.TemporaryDirectory()
        raw = Path(cls._tmp.name) / "raw.img"
        raw.write_bytes(b"\0" * MIB)
        zst = Path(cls._tmp.name) / "i.zst"
        subprocess.run(["zstd", "-q", str(raw), "-o", str(zst)], check=True)
        files = {"version": b"4.3.0\n", "thinfloe-usb.img.zst": zst.read_bytes()}

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_GET(self):
                path = self.path
                for src, dst in (("/latest/download/", "/hop/"),
                                 ("/hop/", "/download/v4.3.0/")):
                    if path.startswith(src):
                        self.send_response(302)
                        self.send_header("Location", dst + path[len(src):])
                        self.send_header("Content-Length", "0")
                        self.end_headers()
                        return
                name = path[len("/download/v4.3.0/"):]
                if not path.startswith("/download/v4.3.0/") or name not in files:
                    self.send_error(404)
                    return
                body = files[name]
                rng = self.headers.get("Range")
                if rng:
                    # bytes=N-M, or open-ended bytes=N- (to the end).
                    first, last = rng.split("=", 1)[1].split("-")
                    end = int(last) + 1 if last else len(body)
                    body = body[int(first):end]
                    self.send_response(206)
                else:
                    self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        cls.server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.base = f"http://127.0.0.1:{cls.server.server_port}/latest/download"

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls._tmp.cleanup()

    def test_open_ended_range(self):
        r = subprocess.run(["curl", "-fsSL", "-r", "2-", self.base + "/version"],
                           capture_output=True, timeout=30, check=False)
        self.assertEqual((r.returncode, r.stdout), (0, b"3.0\n"))

    def test_follows_redirect_chain(self):
        self.assertEqual(ui._fetch_version(self.base), ("4.3.0", ""))
        self.assertEqual(ui.image_size(self.base + "/thinfloe-usb.img.zst"), MIB)


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


class TestExamineMounted(unittest.TestCase):
    """F1: an ESP that is mounted already is read where it is, never mounted again."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.mount_calls = []

    def fake_run(self, cmd, *a, **kw):
        self.mount_calls.append(cmd)
        # The kernel refuses a second, read-only mount of a rw superblock.
        return subprocess.CompletedProcess(cmd, 32, "", "already mounted")

    def enum(self, lsblk, boot="/dev/vda", efi=None):
        def cmd(c):
            if c[0] == "lsblk" and "-J" in c:
                return json.dumps({"blockdevices": lsblk})
            return ""
        with mock.patch.object(ui, "get_cmd_output", side_effect=cmd), \
             mock.patch.object(ui, "detect_boot_device", return_value=boot), \
             mock.patch.object(ui, "find_efi_partition", side_effect=lambda d: efi.get(d)), \
             mock.patch.object(ui, "ptable_fingerprint", return_value="ab" * 8), \
             mock.patch.object(ui, "run_cmd", side_effect=self.fake_run):
            return {d.device: d for d in ui.enumerate_drives()}

    disk = staticmethod(lsblk_disk)

    def test_boot_esp_is_examined_in_place(self):
        esp = os.path.join(self.tmp.name, "efi")
        make_thinfloe_esp(esp)
        with mock.patch.object(ui, "BOOT_EFI_MOUNT", esp), \
             mock.patch.object(ui.os.path, "ismount", side_effect=lambda p: p == esp):
            d = self.enum([self.disk("vda", None, esp, tran=None)],
                          efi={"/dev/vda": "/dev/vda1"})["/dev/vda"]
        self.assertTrue(d.is_boot)
        self.assertTrue(d.is_thinfloe)
        self.assertTrue(d.has_tl_old)
        self.assertEqual([c for c in self.mount_calls if c[0] == "mount"], [])

    def test_other_mounted_esp_is_examined_at_its_mount_point(self):
        esp = os.path.join(self.tmp.name, "media")
        make_thinfloe_esp(esp)
        d = self.enum([self.disk("sdb", None, esp)], boot=None,
                      efi={"/dev/sdb": "/dev/sdb1"})["/dev/sdb"]
        self.assertTrue(d.is_thinfloe)
        self.assertTrue(d.mounted)
        self.assertEqual([c for c in self.mount_calls if c[0] == "mount"], [])

    def mounted_sdb(self, files):
        esp = os.path.join(self.tmp.name, "media")
        make_thinfloe_esp(esp)
        for rel, data in files.items():
            path = Path(esp) / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        return self.enum([self.disk("sdb", None, esp)], boot=None,
                         efi={"/dev/sdb": "/dev/sdb1"})["/dev/sdb"]

    def test_drives_reports_stick_version(self):
        d = self.mounted_sdb({"tl/version": b"4.5.0\n"})
        self.assertEqual(d.version, "4.5.0")
        row = json.loads(ui.drives_json([d], None))[0]
        self.assertEqual(row["version"], "4.5.0")

    def test_drives_prefers_tl_new_version(self):
        d = self.mounted_sdb({"tl/version": b"4.4.0\n", "tl.new/version": b"4.5.0\n"})
        self.assertEqual(d.version, "4.5.0")

    def test_drives_without_version_file_is_null(self):
        d = self.mounted_sdb({})
        self.assertIsNone(d.version)
        self.assertIsNone(json.loads(ui.drives_json([d], None))[0]["version"])

    def test_drives_version_junk_is_null(self):
        # foo and 4.5 pass VERSION_RE, but notes --from refuses them.
        for junk in (b"\xff\x00garbage\n", b"4" * (10 * MIB), b"\n", b"4.5.0 extra\n",
                     b"foo\n", b"4.5\n"):
            with self.subTest(junk=junk[:12]):
                self.tmp.cleanup()
                self.tmp = tempfile.TemporaryDirectory()
                self.assertIsNone(self.mounted_sdb({"tl/version": junk}).version)

    def test_drives_boot_stick_falls_back_to_installed(self):
        esp = os.path.join(self.tmp.name, "efi")
        make_thinfloe_esp(esp)
        installed = os.path.join(self.tmp.name, "installed")
        Path(installed).write_text("4.4.0\n")
        with mock.patch.object(ui, "BOOT_EFI_MOUNT", esp), \
             mock.patch.object(ui, "INSTALLED_VERSION_FILE", installed), \
             mock.patch.object(ui.os.path, "ismount", side_effect=lambda p: p == esp):
            d = self.enum([self.disk("vda", None, esp, tran=None)],
                          efi={"/dev/vda": "/dev/vda1"})["/dev/vda"]
        self.assertEqual(d.version, "4.4.0")

    def test_boot_stick_installed_not_comparable_is_null(self):
        esp = os.path.join(self.tmp.name, "efi")
        make_thinfloe_esp(esp)
        installed = os.path.join(self.tmp.name, "installed")
        Path(installed).write_text("foo\n")
        with mock.patch.object(ui, "BOOT_EFI_MOUNT", esp), \
             mock.patch.object(ui, "INSTALLED_VERSION_FILE", installed), \
             mock.patch.object(ui.os.path, "ismount", side_effect=lambda p: p == esp):
            d = self.enum([self.disk("vda", None, esp, tran=None)],
                          efi={"/dev/vda": "/dev/vda1"})["/dev/vda"]
        self.assertIsNone(d.version)

    def test_boot_stick_file_wins_over_installed(self):
        esp = os.path.join(self.tmp.name, "efi")
        make_thinfloe_esp(esp)
        (Path(esp) / "tl.new").mkdir()
        (Path(esp) / "tl.new" / "version").write_text("4.5.0\n")
        installed = os.path.join(self.tmp.name, "installed")
        Path(installed).write_text("4.4.0\n")
        with mock.patch.object(ui, "BOOT_EFI_MOUNT", esp), \
             mock.patch.object(ui, "INSTALLED_VERSION_FILE", installed), \
             mock.patch.object(ui.os.path, "ismount", side_effect=lambda p: p == esp):
            d = self.enum([self.disk("vda", None, esp, tran=None)],
                          efi={"/dev/vda": "/dev/vda1"})["/dev/vda"]
        self.assertEqual(d.version, "4.5.0")

    def test_failed_umount_keeps_the_cleanup_entry(self):
        # F8: atexit must try the umount again.
        self.addCleanup(ui._cleanup_items.clear)

        def run(cmd, *a, **kw):
            rc = 0 if cmd[0] == "mount" else 32
            return subprocess.CompletedProcess(cmd, rc, "", "")
        with mock.patch.object(ui, "run_cmd", side_effect=run):
            ui.examine_efi_partition("/dev/sdz1")
        # Nothing was mounted for real: the kept mount point is an empty
        # directory in $TMPDIR. Remove it before the entries are cleared.
        for kind, path in list(ui._cleanup_items):
            if kind == "dir":
                self.addCleanup(os.rmdir, path)
        kinds = [t for t, _ in ui._cleanup_items]
        self.assertIn("mount", kinds)


class TestRestoreNonUtf8(unittest.TestCase):
    """F12: a byte that is not UTF-8 cannot skip the rollback."""

    def test_restore_keeps_foreign_bytes(self):
        with tempfile.TemporaryDirectory() as t:
            cfg = Path(t) / "syslinux.cfg"
            cfg.write_bytes(b"# caf\xe9\nDEFAULT new\nLABEL current\n  APPEND ramroot=tl/r\n")
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertTrue(ui._restore_default_current(t))
            data = cfg.read_bytes()
        self.assertIn(b"caf\xe9", data)
        self.assertIn(b"DEFAULT current\n", data)


class TestMountedDrivesNotOffered(unittest.TestCase):
    """L3: a mounted drive is offered only as the boot stick, for Upgrade."""

    def setUp(self):
        self.drives = [
            mk_drive("/dev/sda", is_boot=True, is_thinfloe=True, mounted=True),
            mk_drive("/dev/sdb", is_thinfloe=True, mounted=True),
            mk_drive("/dev/sdc", mounted=True),
            mk_drive("/dev/sdd"),
        ]

    def choose(self, *answers):
        out = io.StringIO()
        with mock.patch("builtins.input", side_effect=list(answers)), \
             contextlib.redirect_stdout(out):
            result = ui.show_drives_with_operations(self.drives)
        return result, out.getvalue()

    def test_only_the_boot_stick_and_the_unmounted_drive_have_options(self):
        result, out = self.choose("1")
        self.assertEqual((result[0].device, result[1]), ("/dev/sda", "upgrade"))
        result, out = self.choose("2")
        self.assertEqual((result[0].device, result[1]), ("/dev/sdd", "overwrite"))
        result, out = self.choose("3", "0")
        self.assertIsNone(result)
        self.assertIn("Invalid choice", out)

    def test_the_mounted_drives_say_why(self):
        _, out = self.choose("0")
        blocks = out.split("\n\n")
        for dev in ("/dev/sdb", "/dev/sdc"):
            block = next(b for b in blocks if dev in b)
            self.assertIn("(mounted - unmount it to write it)", block)
            self.assertNotRegex(block, r"\n    \d+\) ")
        self.assertEqual(out.count("(mounted - unmount it to write it)"), 2)


class TestInteractiveLock(unittest.TestCase):
    """M2: an SSH prompt never holds the lock; the write does."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.lock = os.path.join(self.tmp.name, "upgrade.lock")
        self.addCleanup(ui._release_lock)
        self.seen = []
        self.drive = BatchRefusalsFixture.mk("/dev/sdb", thinfloe=True)
        self.later = [self.drive]

        def note(name, value):
            def f(*a, **kw):
                self.seen.append((name, lock_held(self.lock)))
                return value
            return f
        for name, kw in (
            ("LOCK_PATH", dict(new=self.lock)),
            ("WRITE_LOCK_WAIT", dict(new=0.3)),
            ("ensure_root", dict()),
            ("select_source", dict(side_effect=note("source", "http://x"))),
            ("enumerate_drives", dict(side_effect=self._enum)),
            ("show_drives_with_operations", dict(side_effect=self._choose)),
            ("confirm", dict(side_effect=note("confirm", True))),
            ("check_drive_fits", dict()),
            ("image_size", dict(return_value=MIB)),
            ("home_restore_failed", dict(return_value=False)),
            ("fresh_install", dict(side_effect=note(
                "write", ui.FreshOutcome(ok=True, written=True)))),
            ("do_ab_upgrade", dict(side_effect=note("write", None))),
            ("save_home_before_finish", dict(return_value=0)),
        ):
            m = mock.patch.object(ui, name, **kw)
            m.start()
            self.addCleanup(m.stop)
        self.operation = "overwrite"

    def _enum(self, include_internal=False, examine=True):
        self.seen.append(("scan", lock_held(self.lock)))
        return list(self.later) if self.scans_done() else [self.drive]

    def scans_done(self):
        return sum(1 for n, _ in self.seen if n == "scan") > 1

    def _choose(self, drives, offer_all=False):
        self.seen.append(("choose", lock_held(self.lock)))
        return (self.drive, self.operation)

    def run_main(self):
        with contextlib.redirect_stdout(io.StringIO()), \
             contextlib.redirect_stderr(io.StringIO()):
            return ui.main()

    def check(self):
        prompts = [h for n, h in self.seen if n in ("source", "choose", "confirm")]
        writes = [h for n, h in self.seen if n == "write"]
        self.assertTrue(prompts)
        self.assertEqual(set(prompts), {False}, self.seen)
        self.assertEqual(writes, [True], self.seen)

    def test_fresh_install_locks_only_for_the_write(self):
        self.assertEqual(self.run_main(), 0)
        self.check()

    def test_upgrade_locks_only_for_the_write(self):
        self.operation = "upgrade"
        self.assertEqual(self.run_main(), 0)
        self.check()

    def test_the_default_source_is_pinned_after_the_choice(self):
        self.operation = "upgrade"
        ui.select_source.side_effect = None
        ui.select_source.return_value = ui.DEFAULT_URL
        with mock.patch.object(ui, "_fetch_version", return_value=("4.3.0", "")):
            self.assertEqual(self.run_main(), 0)
        self.assertEqual(ui.do_ab_upgrade.call_args.args[1],
                         ui.RELEASE_DOWNLOAD + "/v4.3.0")

    def test_an_unreadable_version_stops_before_the_drive_scan(self):
        ui.select_source.side_effect = None
        ui.select_source.return_value = ui.DEFAULT_URL
        with mock.patch.object(ui, "_fetch_version",
                               return_value=("", "no version file at the source")):
            err = io.StringIO()
            with self.assertRaises(SystemExit), contextlib.redirect_stdout(io.StringIO()), \
                    contextlib.redirect_stderr(err):
                ui.main()
        self.assertIn("no version file at the source", err.getvalue())
        self.assertNotIn("scan", [n for n, _ in self.seen])

    def test_a_drive_changed_during_the_prompts_is_not_written(self):
        self.later = [ui.DriveInfo(**{**self.drive.__dict__, "fingerprint": "00" * 8})]
        with self.assertRaises(SystemExit):
            self.run_main()
        self.assertNotIn("write", [n for n, _ in self.seen])

    def test_a_drive_mounted_during_the_prompts_is_not_written(self):
        self.operation = "upgrade"
        self.later = [ui.DriveInfo(**{**self.drive.__dict__, "mounted": True})]
        with self.assertRaises(SystemExit):
            self.run_main()
        self.assertNotIn("write", [n for n, _ in self.seen])

    def test_a_running_write_stops_the_ssh_write_after_the_prompts(self):
        import fcntl
        state = {}

        def confirm(*a, **kw):
            if "fd" not in state:  # another write starts while we ask
                fd = os.open(self.lock, os.O_RDWR | os.O_CREAT, 0o644)
                self.addCleanup(os.close, fd)
                state["fd"] = fd
                # Non-blocking: a prompt that holds the lock must fail the
                # test, not hang it.
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return True
        ui.confirm.side_effect = confirm
        with self.assertRaises(SystemExit):
            self.run_main()
        self.assertNotIn("write", [n for n, _ in self.seen])


class TestDrivesNoSize(_ReleaseCase):
    def test_no_size_sends_no_request(self):
        drive = BatchRefusalsFixture.mk("/dev/sdb")
        with mock.patch.object(ui, "image_size",
                               side_effect=AssertionError("request sent")), \
             mock.patch.object(ui, "enumerate_drives", return_value=[drive]):
            rc, out = _run_batch(["drives", "--no-size"])
        self.assertEqual(rc, 0)
        rows = json.loads(out)
        self.assertEqual(len(rows), 1)
        self.assertIsNone(rows[0]["fits"])


class TestCheck(_ReleaseCase):
    def run_check(self, body=b"4.1.0\n", rc_curl=0, size=3148873728, argv=None):
        done = subprocess.CompletedProcess([], rc_curl, stdout=body, stderr=b"")
        with mock.patch.object(ui.subprocess, "run", return_value=done), \
             mock.patch.object(ui, "image_size", return_value=size):
            rc, out = _run_batch(argv or ["check"])
        return rc, dict(l.split("=", 1) for l in out.splitlines())

    def test_check_reads_version_and_size(self):
        rc, kv = self.run_check()
        self.assertEqual(rc, 0)
        self.assertEqual(kv["latest"], "4.1.0")
        self.assertEqual(kv["image_bytes"], "3148873728")
        self.assertEqual(kv["error"], "")
        self.assertEqual(kv["source"], ui.DEFAULT_URL)
        with open(self.status) as f:
            lines = f.read().splitlines()
        self.assertIn("latest=4.1.0", lines)
        self.assertIn("image_bytes=3148873728", lines)
        self.assertIn("installed=4.0.0", lines)
        self.assertEqual(os.stat(self.status).st_mode & 0o777, 0o644)
        self.assertEqual(os.stat(os.path.dirname(self.status)).st_mode & 0o777, 0o755)

    def test_a_failed_check_keeps_the_known_release(self):
        self.run_check(body=b"4.1.0\n", size=3148873728)
        rc, kv = self.run_check(body=b"", rc_curl=22, size=None)
        self.assertEqual(rc, 1)
        self.assertEqual(kv["latest"], "4.1.0")
        self.assertEqual(kv["image_bytes"], "3148873728")
        self.assertIn("no version file at the source", kv["error"])
        with open(self.status) as f:
            lines = f.read().splitlines()
        self.assertIn("latest=4.1.0", lines)
        self.assertIn("image_bytes=3148873728", lines)
        # Another source's answer is never carried.
        rc, kv = self.run_check(body=b"", rc_curl=22, size=None,
                                argv=["check", "--source", "http://127.0.0.1:1/x"])
        self.assertEqual(kv["latest"], "")

    def test_a_size_is_carried_only_for_the_same_release(self):
        self.run_check(body=b"4.1.0\n", size=3148873728)
        rc, kv = self.run_check(body=b"4.2.0\n", size=None)
        self.assertEqual(rc, 1)
        self.assertEqual(kv["latest"], "4.2.0")
        self.assertEqual(kv["image_bytes"], "", "4.1.0's size is not 4.2.0's")
        rc, kv = self.run_check(body=b"4.2.0\n", size=None)
        self.assertEqual(kv["image_bytes"], "")

    def test_a_size_is_carried_for_the_same_release(self):
        self.run_check(body=b"4.1.0\n", size=3148873728)
        rc, kv = self.run_check(body=b"4.1.0\n", size=None)
        self.assertEqual(rc, 1)
        self.assertEqual(kv["latest"], "4.1.0")
        self.assertEqual(kv["image_bytes"], "3148873728")
        self.assertIn("image size", kv["error"])

    def test_control_characters_never_reach_the_status(self):
        rc, kv = self.run_check(argv=["check", "--source", "http://x/\x1b[31m\x07y\x7f"])
        self.assertEqual(kv["source"], "http://x/ [31m y ")

    def test_a_version_must_start_with_a_letter_or_digit(self):
        for body in (b"-4.1.0\n", b".4\n", b"+1\n"):
            rc, kv = self.run_check(body=body)
            self.assertEqual(kv["latest"], "", body)
        with open(self.installed, "w") as f:
            f.write("--help\n")
        self.assertEqual(ui.read_installed_version(), "")

    def test_status_temp_file_is_not_shared(self):
        names = []
        real = os.replace
        def replace(a, b):
            names.append(os.path.basename(a))
            real(a, b)
        with mock.patch.object(ui.os, "replace", side_effect=replace):
            self.run_check()
            self.run_check()
        self.assertEqual(len(names), 2)
        self.assertNotEqual(names[0], names[1])
        self.assertNotIn("status.new", names)
        self.assertEqual(os.listdir(os.path.dirname(self.status)), ["status"])

    def test_missing_version_file_is_unknown_not_failed(self):
        rc, kv = self.run_check(body=b"", rc_curl=22)
        self.assertEqual(rc, 1)
        self.assertEqual(kv["latest"], "")
        self.assertEqual(kv["error"], "no version file at the source")
        self.assertTrue(os.path.exists(self.status))

    def test_hostile_version_text_is_refused(self):
        for body in (b"4.1.0; rm -rf /\n", b"\x1b[31m4\n", b"9" * 4096):
            rc, kv = self.run_check(body=body)
            self.assertEqual(rc, 1, body)
            self.assertEqual(kv["latest"], "", body)
            self.assertEqual(kv["error"], "version file not usable", body)

    def test_unreadable_size_fails_but_keeps_the_version(self):
        rc, kv = self.run_check(size=None)
        self.assertEqual(rc, 1)
        self.assertEqual(kv["latest"], "4.1.0")
        self.assertEqual(kv["image_bytes"], "")
        self.assertNotEqual(kv["error"], "")

    def test_custom_source_does_not_write_the_status(self):
        rc, kv = self.run_check(argv=["check", "--source", "http://127.0.0.1:1/x"])
        self.assertEqual(rc, 0)
        self.assertEqual(kv["source"], "http://127.0.0.1:1/x")
        self.assertFalse(os.path.exists(self.status))

    def test_bad_source_is_misuse(self):
        rc, out = _run_batch(["check", "--source", "file:///etc/passwd"])
        self.assertEqual(rc, 2)

    def test_status_write_is_atomic(self):
        self.run_check(body=b"4.0.5\n")
        seen = []
        real = os.replace

        def replace(a, b):
            with open(b) as f:
                seen.append(f.read())
            real(a, b)

        with mock.patch.object(ui.os, "replace", side_effect=replace):
            self.run_check(body=b"4.1.0\n")
        self.assertEqual(len(seen), 1)
        self.assertIn("latest=4.0.5\n", seen[0])
        self.assertTrue(seen[0].endswith("\n") and "error=" in seen[0])
        self.assertFalse(os.path.exists(self.status + ".new"))
        self.assertFalse(os.path.exists(os.path.join(
            os.path.dirname(self.status), "status.new")))


class TestWriteArgsFixture(unittest.TestCase):
    def test_every_fixture_line_parses(self):
        path = REPO / "test" / "unit" / "upgrade-write-args.tsv"
        n = 0
        for line in path.read_text().splitlines():
            if not line.strip() or line.startswith("#"):
                continue
            argv = line.split("\t")
            args = ui._batch_parser().parse_args(argv)
            self.assertEqual(args.command, "write", line)
            n += 1
        self.assertGreaterEqual(n, 5)


class TestAutomountedDrives(unittest.TestCase):
    """A drive mounted only by thinfloe-automount is released by the write."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.removable = os.path.join(self.tmp.name, "removable")
        Path(self.removable).write_text(
            "/dev/sdb1\tsdb\tX\t/media/tfuser/X\tvfat\t1\tM\tstick\n")
        env = mock.patch.dict(os.environ, {"TF_REMOVABLE": self.removable})
        env.start()
        self.addCleanup(env.stop)

    def enum(self, mountpoints):
        node = {"name": "sdb", "path": "/dev/sdb", "size": 8000000000,
                "model": None, "serial": None, "wwn": None, "tran": "usb",
                "rm": True, "type": "disk", "mountpoints": [None],
                "children": [{"name": "sdb1", "path": "/dev/sdb1", "size": 1,
                              "type": "part", "mountpoints": mountpoints}]}

        def cmd(c):
            if c[0] == "lsblk" and "-J" in c:
                return json.dumps({"blockdevices": [node]})
            return ""
        with mock.patch.object(ui, "get_cmd_output", side_effect=cmd), \
             mock.patch.object(ui, "detect_boot_device", return_value=None), \
             mock.patch.object(ui, "find_efi_partition", return_value=None), \
             mock.patch.object(ui, "ptable_fingerprint", return_value="ab" * 8):
            return ui.enumerate_drives()[0]

    def test_drives_json_has_automounted(self):
        row = json.loads(ui.drives_json([self.enum(["/media/tfuser/X"])], None))[0]
        self.assertIs(row["automounted"], True)
        self.assertIs(row["mounted"], False)

    def test_another_mount_point_is_mounted(self):
        row = json.loads(ui.drives_json(
            [self.enum(["/media/tfuser/X", "/mnt/other"])], None))[0]
        self.assertIs(row["automounted"], False)
        self.assertIs(row["mounted"], True)

    def test_unmounted_is_neither(self):
        row = json.loads(ui.drives_json([self.enum([None])], None))[0]
        self.assertIs(row["automounted"], False)
        self.assertIs(row["mounted"], False)

    def test_upgrade_refuses_an_automounted_stick(self):
        d = ui.DriveInfo("/dev/sdb", "16G", "M", "", True, None, 0, False,
                         False, False, size_bytes=16 * 10**9, serial="S",
                         mounted=False, automounted=True, fingerprint="ab" * 8)
        args = argparse.Namespace(device="/dev/sdb", mode="upgrade", source="http://x",
                                  expect_id=ui.drive_identity(d), copy_home=False,
                                  allow_internal=False)
        self.assertIn("cannot be upgraded", ui._write_refusal(args, d))

    def test_hold_is_written_atomically_and_leaves_no_temp(self):
        folder = tempfile.mkdtemp(dir=self.tmp.name)
        with mock.patch.dict(os.environ, {"TF_AUTOMOUNT_HOLD": folder}), \
             contextlib.redirect_stdout(io.StringIO()):
            d = ui.DriveInfo("/dev/sdb", "16G", "M", "", False, None, 0, False,
                             False, False, serial="S", fingerprint="ab" * 8)
            path = ui._hold_drive(d)
        self.assertEqual(os.listdir(folder), ["sdb"])
        ui._unregister_cleanup("file", path)

    def hold_drive(self):
        return ui.DriveInfo("/dev/sdb", "16G", "M", "", False, None, 0, False,
                            False, False, serial="S", fingerprint="ab" * 8)

    def test_hold_is_renamed_in_under_the_directory_lock(self):
        folder = tempfile.mkdtemp(dir=self.tmp.name)
        fd = os.open(folder, os.O_RDONLY | os.O_DIRECTORY)
        fcntl.flock(fd, fcntl.LOCK_EX)               # the helper, mid-delete
        got = []
        with mock.patch.dict(os.environ, {"TF_AUTOMOUNT_HOLD": folder}), \
             contextlib.redirect_stdout(io.StringIO()):
            t = threading.Thread(target=lambda: got.append(ui._hold_drive(self.hold_drive())))
            t.start()
            time.sleep(0.3)
            self.assertNotIn("sdb", os.listdir(folder), "waits for the helper's lock")
            os.close(fd)
            t.join(5)
        self.assertFalse(t.is_alive())
        self.assertIn("sdb", os.listdir(folder))
        ui._unregister_cleanup("file", got[0])

    def test_a_lock_that_stays_taken_delays_the_hold_by_the_wait_only(self):
        folder = tempfile.mkdtemp(dir=self.tmp.name)
        fd = os.open(folder, os.O_RDONLY | os.O_DIRECTORY)
        self.addCleanup(os.close, fd)
        fcntl.flock(fd, fcntl.LOCK_EX)
        out = io.StringIO()
        with mock.patch.dict(os.environ, {"TF_AUTOMOUNT_HOLD": folder}), \
             mock.patch.object(ui, "HOLD_LOCK_WAIT", 0.2), \
             contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            start = time.monotonic()
            path = ui._hold_drive(self.hold_drive())
            took = time.monotonic() - start
        self.assertEqual(os.listdir(folder), ["sdb"])   # written unlocked, logged
        self.assertIn("hold written unlocked", out.getvalue())   # log() prints to stdout
        self.assertLess(took, 3.0)
        ui._unregister_cleanup("file", path)

    def test_no_hold_directory_is_no_hold(self):
        missing = os.path.join(self.tmp.name, "no-such-dir")
        with mock.patch.dict(os.environ, {"TF_AUTOMOUNT_HOLD": missing}), \
             contextlib.redirect_stdout(io.StringIO()), \
             contextlib.redirect_stderr(io.StringIO()):
            self.assertIsNone(ui._hold_drive(self.hold_drive()))
        self.assertFalse(os.path.exists(missing))

    def test_write_refusal_allows_automounted(self):
        d = ui.DriveInfo("/dev/sdb", "16G", "M", "", False, None, 0, False,
                         False, False, size_bytes=16 * 10**9, serial="S",
                         mounted=False, automounted=True, fingerprint="ab" * 8)
        args = argparse.Namespace(device="/dev/sdb", mode="fresh", source="http://x",
                                  expect_id=ui.drive_identity(d), copy_home=False,
                                  allow_internal=False)
        with mock.patch.object(ui, "image_size", return_value=None):
            self.assertIsNone(ui._write_refusal(args, d))


class TestContents(unittest.TestCase):
    """`contents --device DEV`: the top-level files of an automounted stick."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.media = os.path.realpath(os.path.join(self.tmp.name, "media"))
        os.makedirs(self.media)
        self.removable = os.path.join(self.tmp.name, "removable")
        env = mock.patch.dict(os.environ, {
            "TF_REMOVABLE": self.removable, "TF_MEDIA": self.media})
        env.start()
        self.addCleanup(env.stop)
        root = mock.patch.object(ui, "ensure_root")
        root.start()
        self.addCleanup(root.stop)
        # batch_main replaces the global progress reporter.
        progress = mock.patch.object(ui, "PROGRESS", ui.PROGRESS)
        progress.start()
        self.addCleanup(progress.stop)

    def mount(self, name, names=(), disk="sdb", part="sdb1"):
        """A fake mounted partition; returns its mount point."""
        mp = os.path.join(self.media, name)
        os.makedirs(mp, exist_ok=True)
        for n in names:
            if n.endswith("/"):
                os.makedirs(os.path.join(mp, n[:-1]), exist_ok=True)
            else:
                Path(os.path.join(mp, n)).write_bytes(b"x")
        with open(self.removable, "a") as f:
            f.write(f"/dev/{part}\t{disk}\t{name}\t{mp}\tvfat\t1\tM\tstick\n")
        return mp

    def contents(self, device="/dev/sdb"):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = ui.batch_main(["contents", "--device", device])
        self.assertEqual(rc, 0)
        return json.loads(out.getvalue())

    def test_contents_lists_top_level(self):
        self.mount("S", [f"f{i:02}.txt" for i in range(24)] + ["zdir/"])
        r = self.contents()
        self.assertEqual(r["device"], "/dev/sdb")
        self.assertIsNone(r["error"])
        self.assertEqual(len(r["entries"]), 20)
        self.assertEqual(r["entries"], sorted(r["entries"]))
        self.assertEqual(r["more"], 5)
        self.assertEqual(r["entries"][0], "f00.txt")
        self.assertNotIn("zdir/", r["entries"])

    def test_directories_get_a_trailing_slash(self):
        self.mount("S", ["a.txt", "docs/"])
        self.assertEqual(self.contents()["entries"], ["a.txt", "docs/"])

    def test_contents_non_utf8_names(self):
        mp = self.mount("S", ["ok"])
        with open(os.path.join(os.fsencode(mp), b"bad\xff"), "wb") as f:
            f.write(b"x")
        Path(os.path.join(mp, "n" * 250)).write_bytes(b"x")
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            ui.batch_main(["contents", "--device", "/dev/sdb"])
        r = json.loads(out.getvalue())   # valid UTF-8 JSON
        self.assertIn("bad\N{REPLACEMENT CHARACTER}", r["entries"])
        long = [e for e in r["entries"] if e.startswith("nnn")][0]
        self.assertEqual(long, "n" * 80 + "\N{HORIZONTAL ELLIPSIS}")

    def test_contents_time_limit(self):
        self.mount("S", ["a"])
        with mock.patch.object(ui, "CONTENTS_CHILD", "import time; time.sleep(60)"):
            t0 = time.monotonic()
            r = self.contents()
        self.assertLess(time.monotonic() - t0, 4)
        self.assertEqual(r["error"], "could not read the stick")
        self.assertEqual(r["entries"], [])

    def test_contents_child_runs_in_its_own_session(self):
        self.mount("S", ["a"])
        seen = []
        real = subprocess.Popen

        def spy(*a, **kw):
            seen.append(kw)
            return real(*a, **kw)
        with mock.patch.object(ui.subprocess, "Popen", side_effect=spy):
            self.contents()
        self.assertTrue(seen and all(k.get("start_new_session") for k in seen))

    def test_contents_takes_no_lock(self):
        # The lock is held by another process, as a running write holds it.
        self.mount("S", ["a"])
        lock = os.path.join(self.tmp.name, "held.lock")
        holder = subprocess.Popen(
            [sys.executable, "-I", "-c",
             "import fcntl, os, sys, time\n"
             "fd = os.open(sys.argv[1], os.O_RDWR | os.O_CREAT)\n"
             "fcntl.flock(fd, fcntl.LOCK_EX)\n"
             "print('held', flush=True)\n"
             "time.sleep(60)\n", lock],
            stdout=subprocess.PIPE, text=True)
        self.addCleanup(holder.wait)
        self.addCleanup(holder.kill)
        self.assertEqual(holder.stdout.readline().strip(), "held")
        holder.stdout.close()
        with mock.patch.object(ui, "LOCK_PATH", lock), \
             mock.patch.object(ui, "_take_lock", side_effect=AssertionError("lock")):
            r = self.contents()
        self.assertEqual(r["entries"], ["a"])

    def test_contents_not_automounted(self):
        r = self.contents()
        self.assertEqual((r["entries"], r["more"], r["error"]), ([], 0, None))

    def test_other_disks_are_not_listed(self):
        self.mount("T", ["other"], disk="sdc", part="sdc1")
        self.assertEqual(self.contents()["entries"], [])

    def test_several_partitions_are_prefixed_by_name(self):
        self.mount("ONE", ["a"], part="sdb1")
        self.mount("TWO", ["b"], part="sdb2")
        self.assertEqual(self.contents()["entries"], ["ONE: a", "TWO: b"])

    def test_a_mount_point_outside_the_media_directory_is_ignored(self):
        outside = os.path.join(self.tmp.name, "elsewhere")
        os.makedirs(outside)
        Path(outside, "secret").write_bytes(b"x")
        with open(self.removable, "w") as f:
            f.write(f"/dev/sdb1\tsdb\tX\t{outside}\tvfat\t1\tM\tstick\n")
        self.assertEqual(self.contents()["entries"], [])

    def test_a_symlink_out_of_the_media_directory_is_ignored(self):
        outside = os.path.join(self.tmp.name, "elsewhere")
        os.makedirs(outside)
        Path(outside, "secret").write_bytes(b"x")
        link = os.path.join(self.media, "L")
        os.symlink(outside, link)
        with open(self.removable, "w") as f:
            f.write(f"/dev/sdb1\tsdb\tL\t{link}\tvfat\t1\tM\tstick\n")
        self.assertEqual(self.contents()["entries"], [])

    def test_garbage_contract_lines_and_extra_fields(self):
        mp = self.mount("S", ["a"])
        with open(self.removable, "a") as f:
            f.write("garbage\n\n\t\t\n")
            f.write(f"/dev/sdb2\tsdb\tS\t{mp}\tvfat\t1\tM\tstick\tfuture\n")
        # The same mount point twice is listed once.
        self.assertEqual(self.contents()["entries"], ["a"])

    def test_one_unreadable_partition_does_not_hide_the_others(self):
        self.mount("ONE", ["a"], part="sdb1")
        two = self.mount("TWO", ["b"], part="sdb2")
        shutil.rmtree(two)       # a mount point that vanished: scandir fails
        r = self.contents()
        self.assertEqual((r["entries"], r["error"]), (["ONE: a"], None))

    def test_all_partitions_unreadable_is_an_error(self):
        two = self.mount("TWO", ["b"])
        shutil.rmtree(two)
        r = self.contents()
        self.assertEqual((r["entries"], r["error"]), ([], "could not read the stick"))

    def test_a_bad_device_is_a_usage_error(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            rc = ui.batch_main(["contents", "--device", "sdb; rm"])
        self.assertEqual(rc, 2)


if __name__ == "__main__":
    unittest.main()
