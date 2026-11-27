#!/usr/bin/env python3
"""Offline tests of the upgrade of a ThinFloe stick by thinfloe-upgrade:
batch upgrade, esp.lock, a staged upgrade, the rotation of a non-boot stick,
the child processes' own sessions."""

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


class TestBatchUpgrade(BatchUpgradeFixture):

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
        self.assertIn("checksum not checked: neither SHA256SUMS nor ramroot.sqsh.sha256 was found", out)

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
        copy = ui._copy_one
        def fake(src, *a, **kw):
            if src.endswith("ramroot.sqsh"):
                raise OSError(errno.EIO, "I/O error")
            return copy(src, *a, **kw)
        with mock.patch.object(ui, "_copy_one", side_effect=fake):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 1, out)
        self.assertIn("@result failed the new release could not be installed", out)
        self.assertStickUntouched()

    def test_second_cfg_write_failing_restores_current_and_removes_tl_new(self):
        real = ui.set_boot_default_one
        def flaky(mount, rel, label):
            if label == "new" and rel != "syslinux.cfg":
                raise OSError(errno.ENOSPC, "No space left")
            return real(mount, rel, label)
        with mock.patch.object(ui, "set_boot_default_one", side_effect=flaky):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 1, out)
        # A LABEL new entry may have been added; what boots is DEFAULT.
        for c in self.cfgs():
            self.assertIn("DEFAULT current", c.read_text().splitlines())
        self.assertFalse((self.esp / "tl.new").exists())
        for f in ui.FIRMWARE_FILES:
            self.assertEqual((self.esp / "tl" / f).read_text(), "old " + f)

    def test_failed_restore_keeps_tl_new(self):
        # Both copies say new, the final sync fails, and the restore cannot
        # write: tl.new/ must stay for the DEFAULT new that is on the stick.
        real = ui.set_boot_default_one
        run = ui.run_cmd
        state = {"set": False}
        def one(mount, rel, label):
            if label == "current":
                raise OSError(errno.EIO, "I/O error")
            state["set"] = True
            return real(mount, rel, label)
        def fake_run(cmd, *a, **kw):
            if cmd[0] == "sync" and state["set"]:
                raise subprocess.CalledProcessError(1, cmd)
            return run(cmd, *a, **kw)
        with mock.patch.object(ui, "set_boot_default_one", side_effect=one), \
             mock.patch.object(ui, "run_cmd", side_effect=fake_run):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 1, out)
        self.assertTrue((self.esp / "tl.new" / "ramroot.sqsh").is_file())
        for c in self.cfgs():
            self.assertIn("DEFAULT new", c.read_text())

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


class TestEspLock(BatchUpgradeFixture):
    """The boot stick's switch step takes /run/thinfloe/esp.lock."""

    def setUp(self):
        super().setUp()
        self.lock = Path(self.tmp.name) / "esp.lock"
        self.lock.write_text("")
        m = mock.patch.object(ui, "ESP_LOCK", str(self.lock))
        m.start()
        self.addCleanup(m.stop)

    def hold(self, wait=True):
        """Take the lock from another fd. With wait=False the test fails at
        once when the tool holds it, instead of blocking for ever."""
        import fcntl
        fd = os.open(str(self.lock), os.O_RDONLY)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | (0 if wait else fcntl.LOCK_NB))
        except BlockingIOError:
            os.close(fd)
            self.fail("the tool holds esp.lock")
        return fd

    def locked_by_other(self):
        import fcntl
        fd = os.open(str(self.lock), os.O_RDONLY)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return False
        except BlockingIOError:
            return True
        finally:
            os.close(fd)

    def test_switch_waits_for_the_esp_lock(self):
        self.checksums()
        fd = [self.hold()]
        sleeps = []
        order = []
        (self.esp / "tl.old").mkdir()
        def sleep(s):
            if not from_esp_lock():
                return real_sleep(s)
            # nothing may be removed while the lock is not ours
            order.append((self.esp / "tl.old").exists())
            sleeps.append(s)
            if fd:
                os.close(fd.pop())
        with mock.patch.object(ui.time, "sleep", side_effect=sleep):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 0, out)
        self.assertEqual(order, [True])
        self.assertFalse((self.esp / "tl.old").exists())
        self.assertGreaterEqual(len(sleeps), 1)
        self.assertEqual(out.count("waiting for the boot stick"), 1)

    def test_removal_of_tl_old_waits_and_times_out(self):
        self.checksums()
        (self.esp / "tl.old").mkdir()
        (self.esp / "tl.old" / "x").write_text("x")
        fd = self.hold()
        self.addCleanup(os.close, fd)
        clock = [0.0]
        def sleep(s):
            if not from_esp_lock():
                return real_sleep(s)
            clock[0] += max(s, 1.0)
        with mock.patch.object(ui.time, "monotonic", side_effect=lambda: clock[0]), \
             mock.patch.object(ui.time, "sleep", side_effect=sleep):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 1, out)
        self.assertIn("@result failed the boot stick is busy", out)
        self.assertTrue((self.esp / "tl.old" / "x").exists())
        self.assertEqual([c.read_bytes() for c in self.cfgs()], self.before)

    def test_copy_runs_while_someone_else_holds_the_lock(self):
        self.checksums()
        seen = []
        real = ui._copy_one
        state = {"fd": None}
        def fake(*a, **kw):
            if not seen:
                state["fd"] = self.hold(wait=False)  # taken after step (a) let go
            seen.append(self.locked_by_other())
            return real(*a, **kw)
        def sleep(s):                       # the other holder lets go
            if not from_esp_lock():
                return real_sleep(s)
            if state["fd"] is not None:
                os.close(state["fd"])
                state["fd"] = None
        with mock.patch.object(ui, "_copy_one", side_effect=fake), \
             mock.patch.object(ui.time, "sleep", side_effect=sleep):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 0, out)
        self.assertEqual(seen, [True] * len(ui.FIRMWARE_FILES))

    def test_the_copy_is_flushed_before_the_lock_is_taken(self):
        self.checksums()
        seen = []
        real = ui.run_cmd
        def fake(cmd, *a, **kw):
            if cmd[0] == "sync":
                seen.append(self.locked_by_other())
            return real(cmd, *a, **kw)
        with mock.patch.object(ui, "run_cmd", side_effect=fake):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 0, out)
        # The long flush of the copy runs unlocked, the last short one locked.
        self.assertEqual(seen[0], False, seen)
        self.assertEqual(seen[-1], True, seen)

    def test_boot_label_waits_and_timeout_leaves_default(self):
        self.checksums()
        state = {"fd": None}
        real = ui._copy_one
        def fake(src, *a, **kw):
            r = real(src, *a, **kw)
            if src.endswith("ramroot.sqsh"):
                state["fd"] = self.hold()   # someone takes it during the copy
            return r
        clock = [0.0]
        def sleep(s):
            if not from_esp_lock():
                return real_sleep(s)
            clock[0] += max(s, 1.0)
        with mock.patch.object(ui, "_copy_one", side_effect=fake), \
             mock.patch.object(ui.time, "monotonic", side_effect=lambda: clock[0]), \
             mock.patch.object(ui.time, "sleep", side_effect=sleep):
            rc, out = self.run_upgrade()
        if state["fd"] is not None:
            os.close(state["fd"])
        self.assertEqual(rc, 1, out)
        self.assertIn("@result failed the boot stick is busy", out)
        self.assertIn("still starts the old one", out)
        self.assertEqual([c.read_bytes() for c in self.cfgs()], self.before)

    def test_another_stick_never_touches_the_lock(self):
        self.checksums()
        fd = self.hold()
        self.addCleanup(os.close, fd)
        drive = ui.DriveInfo("/dev/sdy", "16G", "Other", "", False,
                             "/dev/sdy1", 1024, False, False, False)
        opened = []
        real_open = os.open
        def spy(path, *a, **kw):
            if str(path) == str(self.lock):
                opened.append(path)
            return real_open(path, *a, **kw)
        with mock.patch.object(ui.os, "open", side_effect=spy), \
             mock.patch.object(ui.time, "sleep", side_effect=AssertionError("waited")):
            with contextlib.redirect_stdout(io.StringIO()):
                ui._switch_release(drive, str(self.esp), str(self.srv))
        self.assertEqual(opened, [])

    def test_download_does_not_hold_the_esp_lock(self):
        import fcntl
        self.checksums()
        held = []
        real = ui.download_firmware
        def spy(*a, **kw):
            fd = os.open(str(self.lock), os.O_RDONLY)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                held.append(False)
            except BlockingIOError:
                held.append(True)
            finally:
                os.close(fd)
            return real(*a, **kw)
        with mock.patch.object(ui, "download_firmware", side_effect=spy):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 0, out)
        self.assertEqual(held, [False])

    def test_missing_lock_file_goes_on_without_it(self):
        self.checksums()
        self.lock.unlink()
        rc, out = self.run_upgrade()
        self.assertEqual(rc, 0, out)
        self.assertIn("without it", out)


class TestNonBootRotation(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = Path(self.tmp.name)
        self.esp = root / "esp"
        (self.esp / "tl").mkdir(parents=True)
        self.fw = root / "fw"
        self.fw.mkdir()
        for f in ui.FIRMWARE_FILES:
            (self.esp / "tl" / f).write_text("old")
            (self.fw / f).write_text("new")
        (self.esp / "syslinux.cfg").write_text("DEFAULT current\nLABEL current\n  APPEND ramroot=tl/r\n")
        self.drive = ui.DriveInfo("/dev/sdz", "16G", "Test", "", True,
                                  "/dev/sdz1", 1024, False, False, False)

    def switch(self):
        with contextlib.redirect_stdout(io.StringIO()):
            ui._switch_release(self.drive, str(self.esp), str(self.fw))

    def test_switch_release_writes_version(self):
        with contextlib.redirect_stdout(io.StringIO()):
            ui._switch_release(self.drive, str(self.esp), str(self.fw), version="4.5.0")
        self.assertEqual((self.esp / "tl" / "version").read_text(), "4.5.0\n")

    def test_switch_release_without_version_writes_none(self):
        self.switch()
        self.assertFalse((self.esp / "tl" / "version").exists())

    def test_pinned_version(self):
        self.assertEqual(ui.pinned_version(f"{ui.RELEASE_DOWNLOAD}/v4.5.0"), "4.5.0")
        self.assertIsNone(ui.pinned_version("https://example.org/thinfloe"))
        self.assertIsNone(ui.pinned_version(f"{ui.RELEASE_DOWNLOAD}/v4.5.0/x y"))

    def test_copy_failure_leaves_tl(self):
        copy = ui._copy_one
        def fake(src, *a, **kw):
            if src.endswith("ramroot.sqsh"):
                raise OSError(errno.EIO, "I/O error")
            return copy(src, *a, **kw)
        with mock.patch.object(ui, "_copy_one", side_effect=fake), \
             self.assertRaises(ui.WriteFailed):
            self.switch()
        for f in ui.FIRMWARE_FILES:
            self.assertEqual((self.esp / "tl" / f).read_text(), "old")
        self.assertFalse((self.esp / "tl.new").exists())
        self.assertFalse((self.esp / "tl.old").exists())

    def test_second_rename_failure_restores_tl(self):
        real = os.rename
        def fake(a, b):
            if a.endswith("tl.new"):
                raise OSError(errno.EIO, "I/O error")
            return real(a, b)
        with mock.patch.object(ui.os, "rename", side_effect=fake), \
             self.assertRaises(ui.WriteFailed):
            self.switch()
        for f in ui.FIRMWARE_FILES:
            self.assertEqual((self.esp / "tl" / f).read_text(), "old")
        self.assertFalse((self.esp / "tl.old").exists())

    def test_success_rotates(self):
        self.switch()
        for f in ui.FIRMWARE_FILES:
            self.assertEqual((self.esp / "tl" / f).read_text(), "new")
            self.assertEqual((self.esp / "tl.old" / f).read_text(), "old")
        self.assertFalse((self.esp / "tl.new").exists())

    def test_failed_umount_deletes_nothing(self):
        (self.esp / "home.tar.gz").write_text("home")
        def fake(cmd, *a, **kw):
            if cmd[0] == "umount":
                return subprocess.CompletedProcess(cmd, 32, "", "busy")
            if cmd[0] == "mount":
                return subprocess.CompletedProcess(cmd, 0, "", "")
            return subprocess.run(cmd, check=kw.get("check", True))
        work = Path(self.tmp.name) / "work"
        work.mkdir()
        with mock.patch.object(ui, "run_cmd", side_effect=fake), \
             mock.patch.object(ui.tempfile, "mkdtemp", side_effect=[str(self.esp), str(work)]), \
             mock.patch.object(ui, "_is_or_holds_mount", return_value=True), \
             mock.patch.object(ui, "download_firmware", return_value=str(self.fw)), \
             mock.patch.object(ui, "expected_download_sha256", return_value=None), \
             contextlib.redirect_stdout(io.StringIO()):
            ui._ab_upgrade(self.drive, "x", ui.Progress(io.StringIO(), True), False)
        self.assertTrue((self.esp / "home.tar.gz").is_file())
        self.assertTrue((self.esp / "tl").is_dir())
        ui._cleanup_items.clear()


class TestChildIsolation(unittest.TestCase):
    """F2: the menu signals the tool's process group; children must not get it."""

    def test_every_child_starts_in_its_own_session(self):
        seen = []
        real = subprocess.Popen

        def spy(*a, **kw):
            seen.append((a[0] if a else kw.get("args"), kw.get("start_new_session")))
            return real(*a, **kw)
        with tempfile.TemporaryDirectory() as t, \
             mock.patch.object(ui.subprocess, "Popen", spy), \
             contextlib.redirect_stdout(io.StringIO()), \
             contextlib.redirect_stderr(io.StringIO()):
            src = Path(t) / "f"
            src.write_bytes(b"x")
            ui.run_cmd(["true"], check=False)
            ui.get_cmd_output(["true"])
            ui.image_size("file://" + str(src))
            ui._content_length("file://" + str(src))
            ui.expected_download_sha256("file://" + t, "f")
            ui.download_file("file://" + str(src), str(Path(t) / "g"))
            with mock.patch.object(ui.subprocess, "run", wraps=subprocess.run) as r:
                ui.save_home_before_finish()
        self.assertTrue(seen)
        bad = [cmd for cmd, iso in seen if not iso]
        self.assertEqual(bad, [], "children sharing the tool's process group")

    def test_stream_children_start_in_their_own_session(self):
        seen = []
        real = subprocess.Popen

        def spy(*a, **kw):
            seen.append(kw.get("start_new_session"))
            return real(*a, **kw)
        with tempfile.TemporaryDirectory() as t:
            raw = Path(t) / "raw"
            raw.write_bytes(b"y" * 4096)
            subprocess.run(["zstd", "-q", str(raw), "-o", str(raw) + ".zst"], check=True)
            dev = Path(t) / "dev"
            dev.write_bytes(b"")
            with mock.patch.object(ui.subprocess, "Popen", spy):
                ui.stream_image("file://%s.zst" % raw, str(dev), None,
                                ui.Progress(io.StringIO(), True, 0))
        self.assertEqual(seen, [True, True])

    def test_group_sigterm_during_the_switch_still_completes(self):
        # The real thing: the tool runs in a group of its own, the copy of
        # each file is slow, and the whole group gets SIGTERM during it.
        runner = r"""
import contextlib, io, os, signal, sys, time, types
from unittest import mock
import importlib.machinery, importlib.util
spec = importlib.util.spec_from_loader("u", importlib.machinery.SourceFileLoader("u", sys.argv[1]))
ui = importlib.util.module_from_spec(spec); spec.loader.exec_module(ui)
esp, srv = sys.argv[2], sys.argv[3]
ui.BOOT_EFI_MOUNT = esp
ui.save_home_before_finish = lambda: 0
copy = ui._copy_one
def slow(*a, **kw):
    time.sleep(2)
    return copy(*a, **kw)
ui._copy_one = slow
d = ui.DriveInfo("/dev/sdz", "16G", "T", "", True, "/dev/sdz1", 1024, True, False, False)
p = ui.Progress(sys.stdout, True, 0)
signal.signal(signal.SIGTERM, ui._cancel_on_signal)
with mock.patch.object(ui.os.path, "ismount", return_value=True), \
     contextlib.redirect_stderr(io.StringIO()):
    try:
        rc = ui.batch_upgrade(d, types.SimpleNamespace(source="file://" + srv), p)
    except ui.Cancelled:
        p.result(False, "cancelled"); rc = 130
sys.exit(rc)
"""
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            esp, srv = root / "esp", root / "srv"
            (esp / "tl").mkdir(parents=True)
            srv.mkdir()
            for f in ui.FIRMWARE_FILES:
                (esp / "tl" / f).write_text("old")
                (srv / f).write_text("new " + f)
            (esp / "syslinux.cfg").write_text(
                "DEFAULT current\nLABEL current\n  APPEND ramroot=tl/ramroot.sqsh\n")
            proc = subprocess.Popen(
                ["python3", "-B", "-c", runner, str(SCRIPT), str(esp), str(srv)],
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                start_new_session=True, text=True)
            self.addCleanup(lambda: proc.poll() is None and proc.kill())
            lines = []
            for line in proc.stdout:
                lines.append(line)
                if line.startswith("@step switch"):
                    break
            import time
            time.sleep(0.7)  # the first copy is sleeping now
            os.killpg(proc.pid, signal.SIGTERM)
            out, err = proc.communicate(timeout=60)
            lines.append(out)
            out = "".join(lines)
            self.assertEqual(proc.returncode, 0, out + err)
            self.assertIn("@result ok", out)
            self.assertIn("DEFAULT new", (esp / "syslinux.cfg").read_text())
            for f in ui.FIRMWARE_FILES:
                self.assertEqual((esp / "tl.new" / f).read_text(), "new " + f)


class TestStagedUpgrade(TestBatchUpgrade):
    """F4: a stick with an upgrade still staged (DEFAULT new, tl.new/)."""
    # Reruns TestBatchUpgrade's tests: STAGED/CFG and assertStickUntouched
    # make each of them run on a stick with an upgrade still staged.

    STAGED = CFG = ("DEFAULT new\nLABEL current\n  KERNEL /tl/vmlinuz\n"
                    "  APPEND ramroot=tl/ramroot.sqsh\nLABEL new\n"
                    "  KERNEL /tl.new/vmlinuz\n  APPEND ramroot=tl.new/ramroot.sqsh\n")

    def setUp(self):
        super().setUp()
        (self.esp / "tl.new").mkdir()
        for f in ui.FIRMWARE_FILES:
            (self.esp / "tl.new" / f).write_text("staged " + f)

    def assertStickUntouched(self):
        # Inherited tests: a failure before the switch keeps the staged release.
        self.assertEqual([c.read_bytes() for c in self.cfgs()], self.before)
        for f in ui.FIRMWARE_FILES:
            self.assertEqual((self.esp / "tl" / f).read_text(), "old " + f)
            self.assertEqual((self.esp / "tl.new" / f).read_text(), "staged " + f)

    def assertBootable(self):
        for c in self.cfgs():
            lines = c.read_text().splitlines()
            if "DEFAULT new" in lines:
                for f in ui.FIRMWARE_FILES:
                    self.assertTrue((self.esp / "tl.new" / f).is_file(), f)
            else:
                self.assertIn("DEFAULT current", lines)
        for f in ui.FIRMWARE_FILES:
            self.assertEqual((self.esp / "tl" / f).read_text(), "old " + f)

    def test_failed_copy_leaves_a_bootable_stick(self):
        copy = ui._copy_one

        def fake(src, *a, **kw):
            if src.endswith("ramroot.sqsh"):
                raise OSError(errno.EIO, "I/O error")
            return copy(src, *a, **kw)
        with mock.patch.object(ui, "_copy_one", side_effect=fake):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 1, out)
        self.assertBootable()

    def test_a_failed_copy_removes_the_partial_tl_new(self):
        # Staged: the restore puts DEFAULT current back, then the copy fails.
        copy = ui._copy_one

        def fake(src, *a, **kw):
            if src.endswith("ramroot.sqsh"):
                raise OSError(errno.EIO, "I/O error")
            return copy(src, *a, **kw)
        with mock.patch.object(ui, "_copy_one", side_effect=fake):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 1, out)
        self.assertFalse((self.esp / "tl.new").exists())
        for c in self.cfgs():
            self.assertIn("DEFAULT current", c.read_text().splitlines())
        self.assertBootable()

    def test_too_little_space_leaves_stick(self):
        with mock.patch.object(ui, "free_bytes", return_value=0), \
             mock.patch.object(ui, "tree_bytes", return_value=0):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 1, out)
        self.assertIn("Not enough space", out)
        self.assertStickUntouched()

    def test_unconfirmed_restore_removes_nothing(self):
        real = ui.set_boot_default_one

        def one(mount, rel, label):
            if label == "current":
                raise OSError(errno.EIO, "I/O error")
            return real(mount, rel, label)
        with mock.patch.object(ui, "set_boot_default_one", side_effect=one):
            rc, out = self.run_upgrade()
        self.assertEqual(rc, 1, out)
        for f in ui.FIRMWARE_FILES:
            self.assertEqual((self.esp / "tl.new" / f).read_text(), "staged " + f)
        self.assertBootable()

    def test_upgrade_replaces_the_staged_release(self):
        self.checksums()
        rc, out = self.run_upgrade()
        self.assertEqual(rc, 0, out)
        for f in ui.FIRMWARE_FILES:
            self.assertEqual((self.esp / "tl.new" / f).read_bytes(),
                             (self.srv / f).read_bytes())
        self.assertBootable()

    def test_non_boot_staged_failed_copy_leaves_a_bootable_stick(self):
        self.drive = ui.DriveInfo("/dev/sdz", "16G", "Test", "", True,
                                  "/dev/sdz1", 1024, False, False, False)
        copy = ui._copy_one

        def fake(src, *a, **kw):
            if src.endswith("ramroot.sqsh"):
                raise OSError(errno.EIO, "I/O error")
            return copy(src, *a, **kw)
        with contextlib.redirect_stdout(io.StringIO()), \
             mock.patch.object(ui, "_copy_one", side_effect=fake), \
             self.assertRaises(ui.WriteFailed):
            ui._switch_release(self.drive, str(self.esp), str(self.srv))
        self.assertBootable()


if __name__ == "__main__":
    unittest.main()
