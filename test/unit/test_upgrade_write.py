#!/usr/bin/env python3
"""Offline tests of thinfloe-upgrade write: refusals, wipe, lock, source
check, signals, an id without a serial, the exclusive open, a cancel."""

import importlib.machinery
import importlib.util
import hashlib
import contextlib
import argparse
import errno
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
# TestExclusiveOpen reruns the tests of TestFreshInstall. Imported as a
# module, so unittest does not collect that class here a second time.
import test_upgrade_fresh


class TestBatchRefusals(BatchRefusalsFixture):

    def test_expect_id_mismatch_refuses(self):
        rc, out = self.run_write("/dev/sdb", "fresh", "WRONG:1:2")
        self.assertRefused(rc, out, "does not match", "/dev/sdb")

    def test_local_refusals_come_before_the_release_is_read(self):
        # No network: the newest release cannot be read. A write that is
        # refused on this machine anyway says why, exit 2, not exit 1.
        self.m_version.return_value = ("", "no network")
        for device, mode, expect, words in (
                ("/dev/sdb", "fresh", "WRONG:1:2", ("does not match",)),
                ("/dev/nvme0n1", "fresh", None, ("/dev/nvme0n1",)),
                ("/dev/sdc", "fresh", None, ("mounted",))):
            with self.subTest(device=device):
                rc, out = self.run_write(device, mode, expect)
                self.assertRefused(rc, out, *words)
        self.m_version.assert_not_called()

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
        self.assertEqual(args.source, ui.RELEASE_DOWNLOAD + "/v4.3.0")
        self.assertTrue(ui.PROGRESS.batch)
        self.assertIs(progress, ui.PROGRESS)

    def test_default_write_is_pinned_before_the_size_check(self):
        self.run_write("/dev/sdb", "fresh")
        self.m_version.assert_called_once_with(ui.DEFAULT_URL)
        ui.image_size.assert_called_with(
            ui.RELEASE_DOWNLOAD + "/v4.3.0/thinfloe-usb.img.zst")

    def test_unreadable_version_fails_the_write(self):
        self.m_version.return_value = ("", "no version file at the source")
        for mode in ("fresh", "upgrade"):
            rc, out = self.run_write("/dev/sdb", mode)
            self.assertEqual(rc, 1, out)
            self.assertIn("@result failed the newest release could not be read: "
                          "no version file at the source", out)
        self.m_fresh.assert_not_called()
        self.m_upgrade.assert_not_called()

    def test_custom_source_reads_no_version(self):
        self.run_write("/dev/sdb", "upgrade", None, "--source", "https://h/p")
        self.m_version.assert_not_called()
        self.assertEqual(self.m_upgrade.call_args.args[1].source, "https://h/p")

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


class TestCancelNeverLost(FreshInstallFixture):
    """R8/R9: a cancel survives helpers, a second signal, and interactive use."""

    def setUp(self):
        super().setUp()
        self.addCleanup(setattr, ui, "PROGRESS", ui.PROGRESS)
        # Interactive runs print through the module's PROGRESS.
        ui.PROGRESS = ui.Progress(io.StringIO(), False)
        # The interactive path checks the open drive against the chosen one.
        m = mock.patch.object(ui, "fd_identity",
                              side_effect=lambda fd, dev: ui.drive_identity(self.drive))
        m.start()
        self.addCleanup(m.stop)

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

        def twice(url, device, total, progress, fd=None, started=None):
            started()
            Path(device).write_bytes(b"\xee" * (128 * MIB))
            os.kill(os.getpid(), signal.SIGTERM)

        def wipe_after_second(*a, **kw):
            os.kill(os.getpid(), signal.SIGTERM)  # relayed second signal
            return real_wipe(*a, **kw)
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
        self.patches["stream_image"].side_effect = self.fails_after_the_first_byte(
            KeyboardInterrupt())
        with mock.patch.object(ui, "check_drive_fits"), \
             mock.patch.object(ui, "confirm", side_effect=[True, False]), \
             contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaises(KeyboardInterrupt):
                ui.do_fresh_install(self.drive, "http://x")
        self.assertWiped()

    def test_interactive_sighup_wipes(self):
        import signal
        self.patches["stream_image"].side_effect = (
            lambda *a, **k: (k["started"](), os.kill(os.getpid(), signal.SIGHUP)))
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


class TestLock(BatchRefusalsFixture):
    """F5: one lock between `write` and the drive scan of `drives`."""

    def setUp(self):
        super().setUp()
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.lock = os.path.join(self.tmp.name, "upgrade.lock")
        for m in (mock.patch.object(ui, "LOCK_PATH", self.lock),
                  mock.patch.object(ui, "WRITE_LOCK_WAIT", 0.3),
                  mock.patch.object(ui, "DRIVES_LOCK_WAIT", 0.3)):
            m.start()
            self.addCleanup(m.stop)
        self.addCleanup(ui._release_lock)

    def hold(self):
        import fcntl
        fd = os.open(self.lock, os.O_RDWR | os.O_CREAT, 0o644)
        fcntl.flock(fd, fcntl.LOCK_EX)
        self.addCleanup(os.close, fd)

    def held_by_someone(self):
        import fcntl
        fd = os.open(self.lock, os.O_RDWR | os.O_CREAT, 0o644)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(fd, fcntl.LOCK_UN)
            return False
        except BlockingIOError:
            return True
        finally:
            os.close(fd)

    def test_write_holds_the_lock_for_the_whole_job(self):
        seen = []
        self.m_fresh.side_effect = lambda *a: seen.append(self.held_by_someone()) or 0
        rc, out = self.run_write("/dev/sdb", "fresh")
        self.assertEqual(rc, 0, out)
        self.assertEqual(seen, [True])
        self.assertFalse(self.held_by_someone())

    def test_write_while_another_runs_is_refused(self):
        self.hold()
        rc, out = self.run_write("/dev/sdb", "fresh")
        self.assertRefused(rc, out, "another")
        self.drives.stop()
        self.drives.start()

    def test_drives_while_a_write_runs_does_not_mount(self):
        self.drives.stop()
        self.hold()
        out = io.StringIO()
        lsblk = json.dumps({"blockdevices": [lsblk_disk("sdb", None, None)]})
        mounts = []
        with mock.patch.object(ui, "get_cmd_output",
                               side_effect=lambda c: lsblk if "-J" in c else ""), \
             mock.patch.object(ui, "detect_boot_device", return_value=None), \
             mock.patch.object(ui, "find_efi_partition", return_value="/dev/sdb1"), \
             mock.patch.object(ui, "ptable_fingerprint", return_value="ab" * 8), \
             mock.patch.object(ui, "run_cmd", side_effect=lambda c, *a, **k: mounts.append(c)), \
             contextlib.redirect_stdout(out):
            rc = ui.batch_main(["drives"])
        self.drives.start()
        self.assertEqual(rc, 0)
        rows = json.loads(out.getvalue())
        self.assertEqual(rows[0]["device"], "/dev/sdb")
        self.assertFalse(rows[0]["examined"])
        self.assertEqual(mounts, [])
        self.assertIsNone(rows[0]["version"])

    def test_own_check_mounts_are_not_mounted(self):
        node = {"mountpoints": [None], "children": [
            {"mountpoints": ["/tmp/thinfloe-check-abc123"]}]}
        self.assertFalse(ui._any_mounted(node))
        node["children"].append({"mountpoints": ["/media/x"]})
        self.assertTrue(ui._any_mounted(node))


class TestExclusiveOpen(test_upgrade_fresh.TestFreshInstall):
    """F7: the stick is opened once, exclusively, and checked again after."""
    # Reruns TestFreshInstall's tests: its run_fresh drives the batch path
    # with the exclusive open and the identity check after it.

    def setUp(self):
        super().setUp()
        self.drive.fingerprint = ui.ptable_fingerprint(self.dev)
        m = mock.patch.object(ui, "device_serial", return_value="S")
        m.start()
        self.addCleanup(m.stop)

    def run_fresh(self, copy_home=False, verify=False, expect=None,
                  source="http://x"):
        import types
        args = types.SimpleNamespace(
            source=source, copy_home=copy_home, verify=verify, device=self.dev,
            expect_id=expect or ui.drive_identity(self.drive))
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            rc = ui.batch_fresh(self.drive, args, ui.Progress(out, True, 0))
        return rc, out.getvalue()

    def test_opened_with_o_excl_before_the_size_request(self):
        order = []
        real = os.open

        def spy(path, flags, *a):
            if path == self.dev:
                order.append(("open", flags))
            return real(path, flags, *a)
        self.patches["image_size"].side_effect = lambda *a: order.append(("size",)) or MIB
        with mock.patch.object(ui.os, "open", spy):
            rc, out = self.run_fresh()
        self.assertEqual(rc, 0, out)
        self.assertEqual(order[0][0], "open")
        self.assertTrue(order[0][1] & os.O_EXCL)
        self.assertEqual(self.patches["stream_image"].call_args.kwargs.get("fd") is not None, True)

    def test_the_write_starts_at_byte_zero(self):
        # The identity check seeks to the end for the size; the stream must
        # not start there (ENOSPC on the first write in the smoke).
        seen = []

        def stream(url, device, total, progress, fd=None, started=None):
            seen.append(os.lseek(fd, 0, os.SEEK_CUR))
            return self._stream(url, device, total, progress, fd, started)
        self.patches["stream_image"].side_effect = stream
        rc, out = self.run_fresh()
        self.assertEqual(rc, 0, out)
        self.assertEqual(seen, [0])

    def test_busy_device_is_refused_and_untouched(self):
        real = os.open

        def busy(path, flags, *a):
            if path == self.dev and flags & os.O_EXCL:
                raise OSError(errno.EBUSY, "Device or resource busy")
            return real(path, flags, *a)
        with mock.patch.object(ui.os, "open", busy):
            rc, out = self.run_fresh()
        self.assertEqual(rc, 2, out)
        self.assertIn("in use", out)
        self.patches["stream_image"].assert_not_called()
        self.assertEqual(Path(self.dev).read_bytes()[:MIB], b"\xff" * MIB)

    def test_swapped_stick_after_open_is_refused(self):
        Path(self.dev).write_bytes(b"\x01" * (128 * MIB))  # other table now
        rc, out = self.run_fresh()
        self.assertEqual(rc, 2, out)
        self.assertIn("does not match", out)
        self.patches["stream_image"].assert_not_called()
        self.assertEqual(Path(self.dev).read_bytes()[:MIB], b"\x01" * MIB)


class TestSourceCheck(BatchRefusalsFixture):
    """F9: --source goes to curl; only http(s) is taken, never an option."""

    def test_option_like_source_is_refused(self):
        rc, out = self.run_write("/dev/sdb", "fresh", None, "--source=-o/etc/x")
        self.assertRefused(rc, out, "source")

    def test_other_scheme_is_refused(self):
        rc, out = self.run_write("/dev/sdb", "fresh", None, "--source", "ftp://h/p")
        self.assertRefused(rc, out, "source")

    def test_https_is_taken(self):
        rc, out = self.run_write("/dev/sdb", "fresh", None, "--source", "HTTPS://h/p")
        self.assertEqual(rc, 0, out)

    def test_curl_calls_end_options_before_the_url(self):
        self.size.stop()  # the real image_size, with its curl
        self.addCleanup(self.size.start)
        argvs = []
        real = subprocess.Popen

        def spy(a, *x, **kw):
            argvs.append(a)
            return real(a, *x, **kw)
        with tempfile.TemporaryDirectory() as t, \
             mock.patch.object(ui.subprocess, "Popen", spy), \
             contextlib.redirect_stdout(io.StringIO()):
            src = Path(t) / "f"
            src.write_bytes(b"x")
            url = "file://" + str(src)
            ui.image_size(url)
            ui._content_length(url)
            ui.expected_download_sha256("file://" + t, "f")
            ui.download_file(url, str(Path(t) / "g"))
        curls = [a for a in argvs if a[0] == "curl"]
        # expected_download_sha256 asks for SHA256SUMS, then for f.sha256.
        self.assertEqual(len(curls), 5)
        for a in curls:
            self.assertEqual(a[-2], "--", a)


class TestBatchSignals(BatchRefusalsFixture):
    """F10: SIGHUP cancels like SIGTERM; no signal cuts the cleanup."""

    def test_sighup_cancels(self):
        self.m_fresh.side_effect = lambda *a: os.kill(os.getpid(), signal.SIGHUP)
        old = signal.getsignal(signal.SIGHUP)
        rc, out = self.run_write("/dev/sdb", "fresh")
        self.assertEqual(rc, 130, out)
        self.assertIn("@result failed cancelled", out)
        self.assertEqual(signal.getsignal(signal.SIGHUP), old)

    def test_one_shot_covers_both_signals(self):
        old = {s: signal.getsignal(s) for s in (signal.SIGTERM, signal.SIGHUP)}
        for s, h in old.items():
            self.addCleanup(signal.signal, s, h)
        signal.signal(signal.SIGTERM, ui._cancel_on_signal)
        signal.signal(signal.SIGHUP, ui._cancel_on_signal)
        with self.assertRaises(ui.Cancelled):
            os.kill(os.getpid(), signal.SIGTERM)
        self.assertEqual(signal.getsignal(signal.SIGHUP), signal.SIG_IGN)

    def test_sigterm_during_cleanup_is_ignored_and_cleanup_finishes(self):
        self.addCleanup(ui._cleanup_items.clear)
        seen = []

        def umount(cmd, *a, **kw):
            seen.append(signal.getsignal(signal.SIGTERM))
            os.kill(os.getpid(), signal.SIGTERM)
            seen.append(cmd)
            return subprocess.CompletedProcess(cmd, 0, "", "")

        def job(*a):
            ui._register_cleanup("mount", "/run/x/thinfloe-check-1")
            return 0
        self.m_fresh.side_effect = job
        with mock.patch.object(ui, "run_cmd", side_effect=umount):
            rc, out = self.run_write("/dev/sdb", "fresh")
        self.assertEqual(rc, 0, out)
        self.assertEqual(seen[0], signal.SIG_IGN)
        self.assertEqual(seen[1][0], "umount")
        self.assertNotIn(("mount", "/run/x/thinfloe-check-1"), ui._cleanup_items)


class TestWipeClosesTheWriteFirst(WipeFixture):
    """M1: the kernel re-reads the table only once the O_EXCL open is gone."""

    def test_failed_write_probes_after_close(self):
        self.patches["stream_image"].side_effect = self.fails_after_the_first_byte(
            ui.WriteFailed("x"))
        rc, out = self.run_fresh()
        self.assertEqual(rc, 1, out)
        self.assertWiped()
        self.assertEqual(self.probes, ["closed"])

    def test_cancelled_write_probes_after_close(self):
        self.patches["stream_image"].side_effect = self.fails_after_the_first_byte(
            ui.Cancelled())
        rc, out = self.run_fresh()
        self.assertEqual(rc, 130, out)
        self.assertWiped()
        self.assertEqual(self.probes, ["closed"])

    def test_wipe_closes_a_given_fd_before_partprobe(self):
        fd = os.open(self.dev, os.O_RDWR)
        self.fds.append(fd)
        self.assertTrue(ui.wipe_partition_tables(self.dev, fd))
        self.assertEqual(self.probes, ["closed"])
        self.assertWiped()


class TestCancelBeforeTheWipe(WipeFixture):
    """L1: a signal between the failed write and the wipe still wipes."""

    def cancel_first_wipe(self):
        real = ui.wipe_partition_tables
        calls = []

        def wipe(*a, **kw):
            calls.append(a)
            if len(calls) == 1:
                # As if SIGTERM arrived before the wipe ignored it.
                raise ui.Cancelled()
            return real(*a, **kw)
        m = mock.patch.object(ui, "wipe_partition_tables", side_effect=wipe)
        m.start()
        self.addCleanup(m.stop)
        return calls

    def assertClosed(self):
        with self.assertRaises(OSError):
            os.fstat(self.fds[0])

    def test_cancel_before_wipe_after_failed_write(self):
        self.cancel_first_wipe()
        self.patches["stream_image"].side_effect = self.fails_after_the_first_byte(
            ui.WriteFailed("x"))
        rc, out = self.run_fresh()
        self.assertEqual(rc, 130, out)
        self.assertWiped()
        self.assertClosed()

    def test_cancel_before_wipe_after_cancelled_write(self):
        self.cancel_first_wipe()
        self.patches["stream_image"].side_effect = self.fails_after_the_first_byte(
            ui.Cancelled())
        rc, out = self.run_fresh()
        self.assertEqual(rc, 130, out)
        self.assertWiped()
        self.assertClosed()
        self.assertEqual(self.probes, ["closed"])


class TestIdWithoutSerial(BatchRefusalsFixture):
    """M4: an id never starts with "-", so it passes as its own argument."""

    def mk_noserial(self):
        return ui.DriveInfo(**{**self.mk("/dev/sdb").__dict__, "serial": ""})

    def test_format(self):
        d = self.mk_noserial()
        self.assertEqual(ui.drive_identity(d), "nosn:%d:%s" % (16 * 10**9, "ab" * 8))
        for serial in ("nosn", "-x", "sn-", ""):
            ident = ui.drive_identity(ui.DriveInfo(**{**d.__dict__, "serial": serial}))
            self.assertFalse(ident.startswith("-"), ident)
            if serial:
                self.assertNotEqual(ident, ui.drive_identity(d), serial)

    def test_fd_identity_uses_the_same_format(self):
        with tempfile.TemporaryDirectory() as t:
            p = os.path.join(t, "dev")
            Path(p).write_bytes(b"\xff" * (2 * MIB))
            d = ui.DriveInfo(**{**self.mk_noserial().__dict__,
                                "size_bytes": 2 * MIB,
                                "fingerprint": ui.ptable_fingerprint(p)})
            fd = os.open(p, os.O_RDONLY)
            try:
                with mock.patch.object(ui, "device_serial", return_value=""):
                    self.assertEqual(ui.fd_identity(fd, p), ui.drive_identity(d))
            finally:
                os.close(fd)

    def test_id_as_two_arguments(self):
        self.drives.stop()
        nos = self.mk_noserial()
        with mock.patch.object(ui, "enumerate_drives", return_value=[nos]):
            rc, out = self.run_write("/dev/sdb", "fresh", ui.drive_identity(nos))
        self.drives.start()
        self.assertEqual(rc, 0, out)
        self.m_fresh.assert_called_once()

    def test_id_with_equals_still_works(self):
        self.drives.stop()
        nos = self.mk_noserial()
        argv = ["write", "--device", "/dev/sdb", "--mode", "fresh",
                "--expect-id=" + ui.drive_identity(nos)]
        with mock.patch.object(ui, "enumerate_drives", return_value=[nos]), \
             contextlib.redirect_stdout(io.StringIO()), \
             contextlib.redirect_stderr(io.StringIO()):
            rc = ui.batch_main(argv)
        self.drives.start()
        self.assertEqual(rc, 0)
        self.m_fresh.assert_called_once()


if __name__ == "__main__":
    unittest.main()
