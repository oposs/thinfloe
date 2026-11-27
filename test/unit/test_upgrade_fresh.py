#!/usr/bin/env python3
"""Offline tests of a fresh install by thinfloe-upgrade: the install, a
closed stdout, the interactive drive check, automount."""

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


class TestFreshInstall(FreshInstallFixture):

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
        self.patches["stream_image"].side_effect = self.fails_after_the_first_byte(
            ui.WriteFailed("the stick stopped answering"))
        rc, out = self.run_fresh()
        self.assertEqual(rc, 1)
        self.assertIn("@result failed the stick stopped answering", out)
        self.assertWiped()
        self.patches["stamp_new_stick"].assert_not_called()

    def test_wipe_failure_still_fails_and_is_logged(self):
        self.patches["stream_image"].side_effect = self.fails_after_the_first_byte(
            ui.WriteFailed("x"))
        with mock.patch.object(ui, "wipe_partition_tables", return_value=False) as w:
            rc, out = self.run_fresh()
        w.assert_called_once()
        self.assertEqual(w.call_args.args[0], self.dev)
        self.assertEqual(rc, 1)
        self.assertIn("@result failed x", out)

    def test_wipe_failure_is_logged_for_real(self):
        self.patches["stream_image"].side_effect = self.fails_after_the_first_byte(
            ui.WriteFailed("x"))
        # The stick stopped taking writes: the wipe's own flush fails too.
        with mock.patch.object(ui.os, "fdatasync",
                               side_effect=OSError(errno.EIO, "Input/output error")):
            rc, out = self.run_fresh()
        self.assertEqual(rc, 1)
        self.assertIn("could not clear", out)

    def test_curl_failure_after_the_first_byte_wipes(self):
        self.patches["stream_image"].side_effect = self.fails_after_the_first_byte(
            ui.WriteFailed("the download failed (curl exit 56)"))
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
        self.assertIn("checksum not checked: no checksum was found for thinfloe-usb.img.zst", out)
        self.assertNotWiped()

    def test_sigterm_during_write_wipes_and_gives_130(self):
        import signal

        def term(url, device, total, progress, fd=None, started=None):
            started()
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

    def test_cancel_during_the_checksum_fetch_leaves_the_stick(self):
        # Nothing was written yet: the stick must stay as it was.
        before = Path(self.dev).read_bytes()
        self.patches["expected_download_sha256"].side_effect = ui.Cancelled()
        rc, out = self.run_fresh()
        self.assertEqual(rc, 130)
        self.assertIn("@result failed cancelled", out)
        self.patches["stream_image"].assert_not_called()
        self.assertEqual(Path(self.dev).read_bytes(), before)

    def test_a_download_that_never_starts_leaves_the_stick(self):
        # The real stream: curl fails, not one byte reaches the stick.
        before = Path(self.dev).read_bytes()
        self.patches["stream_image"].side_effect = REAL_STREAM_IMAGE
        rc, out = self.run_fresh(source="file://" + self.tmp.name + "/none")
        self.assertEqual(rc, 1, out)
        self.assertIn("@result failed the download failed", out)
        self.assertEqual(Path(self.dev).read_bytes(), before)

    def test_touched_comes_once_with_the_first_byte(self):
        rc, out = self.run_fresh()
        self.assertEqual(rc, 0, out)
        marks = [l for l in out.splitlines()
                 if l in ("@step write Downloading and writing", "@touched", "@done write")]
        self.assertEqual(marks, ["@step write Downloading and writing", "@touched",
                                 "@done write"])

    def test_touched_is_out_before_the_wipe_flag_is_set(self):
        # The flag is set after the line: a signal between the two can only
        # make the menu warn about a stick that was not wiped, never say
        # "as it was" about a wiped one. A signal right after the line
        # finds the flag unset, so the stick is not wiped.
        real = ui.Progress.touched

        def line_then_signal(progress):
            real(progress)
            raise ui.Cancelled()
        before = Path(self.dev).read_bytes()
        with mock.patch.object(ui.Progress, "touched", line_then_signal):
            rc, out = self.run_fresh()
        self.assertEqual(rc, 130)
        self.assertIn("@touched", out.splitlines())
        self.assertEqual(Path(self.dev).read_bytes(), before)

    def test_no_touched_before_the_first_byte(self):
        self.patches["stream_image"].side_effect = ui.Cancelled()
        rc, out = self.run_fresh()
        self.assertEqual(rc, 130)
        self.assertNotIn("@touched", out.splitlines())

    def test_a_failure_before_the_first_byte_leaves_the_stick(self):
        before = Path(self.dev).read_bytes()
        self.patches["stream_image"].side_effect = ui.WriteFailed(
            "could not start the download: no curl")
        rc, out = self.run_fresh()
        self.assertEqual(rc, 1, out)
        self.assertIn("@result failed could not start the download", out)
        self.assertEqual(Path(self.dev).read_bytes(), before)

    def test_a_cancel_before_the_first_byte_leaves_the_stick(self):
        before = Path(self.dev).read_bytes()
        self.patches["stream_image"].side_effect = ui.Cancelled()
        rc, out = self.run_fresh()
        self.assertEqual(rc, 130)
        self.assertIn("@result failed cancelled", out)
        self.assertEqual(Path(self.dev).read_bytes(), before)

    def test_cancel_while_the_image_size_is_read_leaves_the_stick(self):
        # image_size is a curl request of up to 30 s; nothing is written yet.
        before = Path(self.dev).read_bytes()
        self.patches["image_size"].side_effect = ui.Cancelled()
        rc, out = self.run_fresh()
        self.assertEqual(rc, 130)
        self.assertIn("@result failed cancelled", out)
        self.patches["stream_image"].assert_not_called()
        self.assertEqual(Path(self.dev).read_bytes(), before)

    def test_cancel_after_verify_does_not_wipe(self):
        self.patches["stamp_new_stick"].side_effect = ui.Cancelled()
        rc, out = self.run_fresh()
        self.assertEqual(rc, 130)
        self.assertIn("@result failed cancelled", out)
        self.assertNotWiped()

    def test_required_checksum_missing_fails_before_write(self):
        before = Path(self.dev).read_bytes()
        self.patches["expected_download_sha256"].return_value = None
        rc, out = self.run_fresh(source=ui.RELEASE_DOWNLOAD + "/v4.3.0")
        self.assertEqual(rc, 1, out)
        self.assertEqual(self.lines(out, "@result"), [
            "@result failed the release has no checksum for thinfloe-usb.img.zst"])
        self.patches["stream_image"].assert_not_called()
        self.assertEqual(Path(self.dev).read_bytes(), before)

    def test_write_uses_pinned_tag_for_every_file(self):
        pinned = ui.RELEASE_DOWNLOAD + "/v4.3.0"
        urls = []
        real_run = subprocess.run

        def run(argv, *a, **kw):
            if argv[0] != "curl":
                return real_run(argv, *a, **kw)
            urls.append(argv[-1])
            if argv[-1].endswith("/SHA256SUMS"):
                body = ("cd" * 32 + "  thinfloe-usb.img.zst\n").encode()
                return subprocess.CompletedProcess(argv, 0, body, b"")
            return subprocess.CompletedProcess(argv, 22, b"", b"")
        ident = ui.drive_identity(self.drive)
        with mock.patch.object(ui.subprocess, "run", side_effect=run), \
             mock.patch.object(ui, "expected_download_sha256",
                               REAL_EXPECTED_DOWNLOAD_SHA256), \
             mock.patch.object(ui, "_fetch_version", return_value=("4.3.0", "")), \
             mock.patch.object(ui, "ensure_root"), \
             mock.patch.object(ui, "enumerate_drives", return_value=[self.drive]), \
             mock.patch.object(ui, "fd_identity", return_value=ident), \
             mock.patch.object(ui, "PROGRESS", ui.PROGRESS):
            rc, out = _run_batch(["write", "--device", self.dev, "--mode", "fresh",
                                  "--expect-id", ident])
        self.assertEqual(rc, 0, out)
        urls += [c.args[0] for c in self.patches["stream_image"].call_args_list]
        urls += [c.args[0] for c in self.patches["image_size"].call_args_list]
        self.assertIn(pinned + "/SHA256SUMS", urls)
        self.assertIn(pinned + "/thinfloe-usb.img.zst", urls)
        for url in urls:
            self.assertTrue(url.startswith(pinned + "/"), url)


class TestClosedStdout(FreshInstallFixture):
    """F3: nobody reading stdout is no reason to leave a half-written stick."""

    def test_emit_after_broken_pipe_is_silent(self):
        class Gone:
            def write(self, text):
                raise BrokenPipeError(errno.EPIPE, "Broken pipe")

            def flush(self):
                pass
        p = ui.Progress(Gone(), True, 0)
        p.step("write", "Writing")
        p.progress(1, 2)
        p.result(False, "x")  # nothing raises

    def test_log_to_hung_up_terminal_is_silent(self):
        # An SSH session that hung up leaves a tty that answers EIO, not EPIPE.
        class HungUp:
            def write(self, text):
                raise OSError(errno.EIO, "Input/output error")

            def flush(self):
                pass
        with mock.patch.object(sys, "stdout", HungUp()):
            ui.log("Copying")  # nothing raises

    def test_unexpected_error_during_write_wipes_and_propagates(self):
        def boom(*a, **kw):
            kw["started"]()
            Path(self.dev).write_bytes(b"\xee" * (128 * MIB))
            raise RuntimeError("unexpected")
        self.patches["stream_image"].side_effect = boom
        with self.assertRaises(RuntimeError):
            self.run_fresh()
        self.assertWiped()

    def test_closed_stdout_mid_write_finishes_or_wipes(self):
        class GoneAfter(io.StringIO):
            n = 0

            def write(self, text):
                self.n += 1
                if self.n > 1:
                    raise BrokenPipeError(errno.EPIPE, "Broken pipe")
                return super().write(text)

        def stream(url, device, total, progress, *a, **kw):
            kw["started"]()
            progress.progress(1, 2)  # stdout is gone by now
            Path(device).write_bytes(b"\xee" * (128 * MIB))
            return ui.WriteResult(written=MIB, image_sha256=self.SHA,
                                  download_sha256="cd" * 32)
        self.patches["stream_image"].side_effect = stream
        import types
        args = types.SimpleNamespace(source="http://x", copy_home=False,
                                     verify=False, device=self.dev, expect_id=None)
        with contextlib.redirect_stdout(io.StringIO()):
            rc = ui.batch_fresh(self.drive, args, ui.Progress(GoneAfter(), True, 0))
        self.assertEqual(rc, 0)
        self.assertNotWiped()


class TestInteractiveFreshChecksTheOpenDrive(FreshInstallFixture):
    """L2: over SSH too, the drive behind the O_EXCL open is the chosen one."""

    def setUp(self):
        super().setUp()
        self.addCleanup(setattr, ui, "PROGRESS", ui.PROGRESS)
        ui.PROGRESS = ui.Progress(io.StringIO(), False)

    def run_interactive(self, before_write=None):
        with mock.patch.object(ui, "check_drive_fits"), \
             mock.patch.object(ui, "confirm", side_effect=[True, False]), \
             contextlib.redirect_stdout(io.StringIO()), \
             contextlib.redirect_stderr(io.StringIO()):
            return ui.do_fresh_install(self.drive, "http://x",
                                       before_write=before_write)

    def test_a_swapped_drive_is_refused_and_not_written(self):
        with mock.patch.object(ui, "fd_identity", return_value="sn-OTHER:1:00"):
            with self.assertRaises(SystemExit):
                self.run_interactive()
        self.patches["stream_image"].assert_not_called()
        self.assertEqual(Path(self.dev).read_bytes()[:MIB], b"\xff" * MIB)

    def test_the_identity_of_the_rescanned_drive_is_expected(self):
        now = ui.DriveInfo(**{**self.drive.__dict__, "fingerprint": "12" * 8})
        with mock.patch.object(ui, "fd_identity",
                               return_value=ui.drive_identity(now)):
            self.run_interactive(before_write=lambda: now)
        self.patches["stream_image"].assert_called_once()


class TestFreshInstallAutomount(unittest.TestCase):
    """The hold, the release and the remount around a fresh install."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.hold_dir = os.path.join(self.tmp.name, "hold")
        os.mkdir(self.hold_dir)
        self.hold = os.path.join(self.hold_dir, "sdb")
        env = mock.patch.dict(os.environ, {"TF_AUTOMOUNT_HOLD": self.hold_dir})
        env.start()
        self.addCleanup(env.stop)
        self.events = []
        self.release_rc = 0
        self.drive = ui.DriveInfo("/dev/sdb", "16G", "M", "", False, None, 0,
                                  False, False, False, size_bytes=16 * 10**9,
                                  serial="S", fingerprint="ab" * 8)

    def run_cmd(self, cmd, **kw):
        if cmd[0] == ui.AUTOMOUNT:
            self.events.append((cmd[1], cmd[2], os.path.exists(self.hold)))
            rc = self.release_rc if cmd[1] == "release" else 0
            return subprocess.CompletedProcess(cmd, rc, "", "")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    def install(self, open_target=None, exclusive=None):
        def opened(device):
            self.events.append(("open", device, os.path.exists(self.hold)))
            return os.open(os.devnull, os.O_RDWR)
        patches = [mock.patch.object(ui, "run_cmd", side_effect=self.run_cmd),
                   mock.patch.object(ui, "open_target", side_effect=open_target or opened)]
        if exclusive is not None:
            patches.append(mock.patch.object(ui, "_fresh_install_open", side_effect=exclusive))
        self.out = io.StringIO()
        with contextlib.ExitStack() as st, contextlib.redirect_stdout(self.out):
            for p in patches:
                st.enter_context(p)
            return ui.fresh_install(self.drive, "http://x", False, False, ui.PROGRESS,
                                    save=False, stamp_required=False)

    def touched(self, drive, base_url, copy_home, verify, progress, save,
                stamp_required, held, outcome):
        outcome.touched = True
        outcome.written = True
        outcome.ok = True
        return outcome

    def untouched(self, *a):
        return a[-1]

    def test_fresh_install_holds_and_releases(self):
        self.install(exclusive=self.touched)
        self.assertEqual(self.events, [("release", "sdb", True), ("open", "/dev/sdb", True)])
        self.assertFalse(os.path.exists(self.hold))

    def test_hold_file_carries_pid_and_id(self):
        seen = []
        orig = self.run_cmd

        def peek(cmd, **kw):
            if cmd[0] == ui.AUTOMOUNT and cmd[1] == "release":
                seen.append(Path(self.hold).read_text())
            return orig(cmd, **kw)
        with mock.patch.object(self, "run_cmd", side_effect=peek):
            self.install(exclusive=self.touched)
        self.assertEqual(seen, [f"{os.getpid()}\t{ui.drive_identity(self.drive)}\n"])

    def test_fresh_install_release_busy(self):
        self.release_rc = 3
        outcome = self.install(exclusive=self.touched)
        self.assertTrue(outcome.refused)
        self.assertEqual(outcome.reason, "the stick is in use")
        self.assertEqual([e[0] for e in self.events], ["release", "remount"])
        self.assertFalse(os.path.exists(self.hold))

    def test_hold_is_gone_before_the_remount(self):
        self.install(exclusive=self.untouched)
        self.assertEqual(self.events[-1], ("remount", "sdb", False))

    def test_fresh_install_unwritten_remounts_ebusy(self):
        def busy(device):
            raise OSError(errno.EBUSY, "busy")
        outcome = self.install(open_target=busy)
        self.assertTrue(outcome.refused)
        self.assertEqual([e[0] for e in self.events].count("remount"), 1)

    def test_unwritten_remounts_on_cancel(self):
        # The real write path: a cancel while the checksum is fetched comes
        # before the first byte, so nothing is wiped and the stick comes back.
        wiped = []
        with mock.patch.object(ui, "expected_download_sha256", side_effect=ui.Cancelled()), \
             mock.patch.object(ui, "wipe_partition_tables",
                               side_effect=lambda *a, **kw: wiped.append(a)):
            outcome = self.install()
        self.assertTrue(outcome.cancelled)
        self.assertFalse(outcome.touched)
        self.assertEqual([e[0] for e in self.events], ["release", "open", "remount"])
        self.assertEqual(wiped, [])
        self.assertFalse(os.path.exists(self.hold))

    def test_unwritten_checksum_refusal_remounts(self):
        # The real write path: a release without its checksum is refused
        # before the first byte.
        with mock.patch.object(ui, "expected_download_sha256", return_value=None), \
             mock.patch.object(ui, "checksum_required", return_value=True), \
             mock.patch.object(ui, "stream_image", side_effect=AssertionError("written")):
            outcome = self.install()
        self.assertFalse(outcome.ok or outcome.touched)
        self.assertIn("no checksum", outcome.reason)
        self.assertEqual([e[0] for e in self.events], ["release", "open", "remount"])

    def test_the_remount_is_announced(self):
        # The remount can take the helper's whole add budget: the person
        # sees why the tool still waits.
        said = []
        orig = self.run_cmd

        def peek(cmd, **kw):
            if cmd[0] == ui.AUTOMOUNT and cmd[1] == "remount":
                said.append(self.out.getvalue())
            return orig(cmd, **kw)
        with mock.patch.object(self, "run_cmd", side_effect=peek):
            self.install(exclusive=self.untouched)
        self.assertEqual(len(said), 1)
        self.assertIn("Mounting the stick again", said[0])

    def test_a_write_that_wrote_bytes_does_not_remount(self):
        self.install(exclusive=self.touched)
        self.assertNotIn("remount", [e[0] for e in self.events])

    def test_a_failed_release_is_logged_with_its_code(self):
        self.release_rc = 1
        out = io.StringIO()
        with mock.patch.object(ui, "run_cmd", side_effect=self.run_cmd), \
             mock.patch.object(ui, "open_target", side_effect=OSError(errno.EBUSY, "b")), \
             contextlib.redirect_stdout(out):
            ui.fresh_install(self.drive, "http://x", False, False, ui.PROGRESS)
        self.assertIn("release sdb exited 1", out.getvalue())

    def test_a_missing_helper_is_no_error(self):
        with mock.patch.object(ui, "run_cmd", side_effect=FileNotFoundError(2, "none")), \
             mock.patch.object(ui, "open_target", side_effect=OSError(errno.EBUSY, "b")), \
             contextlib.redirect_stdout(io.StringIO()):
            outcome = ui.fresh_install(self.drive, "http://x", False, False, ui.PROGRESS)
        self.assertIn("in use", outcome.reason)


if __name__ == "__main__":
    unittest.main()
