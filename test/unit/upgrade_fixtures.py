"""Fixtures shared by the test_upgrade_*.py modules: thinfloe-upgrade loaded
once per process as `ui`, fake drives and the fixture bases of the test
classes. No test_ prefix: run-unit-tests.py does not run it."""

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

__all__ = [
    "_LOCK_DIR",
    "_ReleaseCase",
    "_run_batch",
    "BatchRefusalsFixture",
    "BatchUpgradeFixture",
    "CHANGES",
    "fake_cmd",
    "FreshInstallFixture",
    "from_esp_lock",
    "IMG",
    "load_upgrade_image",
    "lock_held",
    "lsblk_disk",
    "LSBLK",
    "make_thinfloe_esp",
    "MIB",
    "mk_drive",
    "REAL_EXPECTED_DOWNLOAD_SHA256",
    "real_sleep",
    "REAL_STREAM_IMAGE",
    "REPO",
    "SCRIPT",
    "ui",
    "WipeFixture",
]

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "overlay" / "usr" / "bin" / "thinfloe-upgrade"


def load_upgrade_image():
    # The tools sit in overlay/, which ships; no bytecode may land there.
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_loader(
        "upgrade_image_batch",
        importlib.machinery.SourceFileLoader("upgrade_image_batch", str(SCRIPT)),
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ui = load_upgrade_image()
# Most fixtures replace it; the pinned-tag case needs the real lookup.
REAL_EXPECTED_DOWNLOAD_SHA256 = ui.expected_download_sha256
REAL_STREAM_IMAGE = ui.stream_image
MIB = 1024 * 1024
# Never the real /run/thinfloe/upgrade.lock.
_LOCK_DIR = tempfile.TemporaryDirectory()
ui.LOCK_PATH = os.path.join(_LOCK_DIR.name, "upgrade.lock")


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

CHANGES = (REPO / "CHANGES.md").read_text()


IMG = 3148873728


class BatchRefusalsFixture(unittest.TestCase):
    """TestBatchRefusals without its tests: setUp and helpers."""
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
        # A write from the default source reads the newest version first.
        self.version = mock.patch.object(ui, "_fetch_version",
                                         return_value=("4.3.0", ""))
        self.m_version = self.version.start()
        self.addCleanup(self.version.stop)

    def _enum(self, include_internal=False, examine=True):
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



class FreshInstallFixture(unittest.TestCase):
    """TestFreshInstall without its tests: setUp and helpers."""
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

    def _stream(self, url, device, total, progress, fd=None, started=None):
        if started is not None:
            started()
        Path(device).write_bytes(b"\xee" * (128 * MIB))
        return ui.WriteResult(written=MIB, image_sha256=self.SHA,
                              download_sha256="cd" * 32)

    def fails_after_the_first_byte(self, exc):
        """A stream that wrote to the stick, then raised exc.

        A stream that raises before its first byte leaves the stick as it
        was (test_a_failure_before_the_first_byte_leaves_the_stick); every
        wipe case needs a stream that got that far.
        """
        def stream(url, device, total, progress, fd=None, started=None):
            started()
            Path(device).write_bytes(b"\xee" * (128 * MIB))
            raise exc
        return stream

    def run_fresh(self, copy_home=False, verify=False, source="http://x"):
        import types
        args = types.SimpleNamespace(source=source, copy_home=copy_home,
                                     verify=verify, device=self.dev, expect_id=None)
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



class BatchUpgradeFixture(unittest.TestCase):
    """TestBatchUpgrade without its tests: setUp and helpers."""
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



real_sleep = time.sleep


def from_esp_lock():
    """True when the sleep being mocked was called by esp_lock()."""
    f = sys._getframe(1)
    while f is not None:
        if f.f_code.co_name == "esp_lock":
            return True
        f = f.f_back
    return False


# ---------------------------------------------------------------- final fixes

def mk_drive(device, **kw):
    base = dict(device=device, size="?", model="M", label="", is_thinfloe=False,
                efi_partition=None, part_size_mb=0, is_boot=False,
                has_tl_old=False, has_tl_new=False, size_bytes=16 * 10**9,
                serial="S", transport="usb", removable=True, mounted=False,
                fingerprint="ab" * 8)
    base.update(kw)
    return ui.DriveInfo(**base)


def lsblk_disk(name, mnt, part_mnt, tran="usb"):
    """One disk with one partition as lsblk --json shows it."""
    return {"name": name, "path": "/dev/" + name, "size": 8000000000,
            "model": None, "serial": None, "wwn": None, "tran": tran,
            "rm": False, "type": "disk", "mountpoints": [mnt],
            "children": [{"name": name + "1", "path": "/dev/%s1" % name,
                          "size": 1, "type": "part", "mountpoints": [part_mnt]}]}


def make_thinfloe_esp(root):
    (Path(root) / "tl").mkdir(parents=True)
    for f in ("vmlinuz", "ramroot.sqsh"):
        (Path(root) / "tl" / f).write_text("x")
    (Path(root) / "tl.old").mkdir()


def lock_held(path):
    """True when another open file holds the flock on path."""
    import fcntl
    fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        fcntl.flock(fd, fcntl.LOCK_UN)
        return False
    except BlockingIOError:
        return True
    finally:
        os.close(fd)


class WipeFixture(FreshInstallFixture):
    """TestWipeClosesTheWriteFirst without its tests: setUp and helpers."""

    def setUp(self):
        super().setUp()
        self.fds = []
        real = ui.open_target

        def spy(device):
            fd = real(device)
            self.fds.append(fd)
            return fd
        m = mock.patch.object(ui, "open_target", side_effect=spy)
        m.start()
        self.addCleanup(m.stop)
        self.probes = []

        def run_cmd(cmd, *a, **kw):
            if cmd[0] == "partprobe":
                try:
                    os.fstat(self.fds[0])
                    self.probes.append("open")
                except OSError:
                    self.probes.append("closed")
            return subprocess.CompletedProcess(cmd, 0, "", "")
        self.patches["run_cmd"].side_effect = run_cmd



def _run_batch(argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        rc = ui.batch_main(argv)
    return rc, out.getvalue()


class _ReleaseCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.status = os.path.join(self.tmp.name, "upgrade", "status")
        self.installed = os.path.join(self.tmp.name, "version")
        with open(self.installed, "w") as f:
            f.write("4.0.0\n")
        for name, value in (("UPGRADE_STATUS", self.status),
                            ("INSTALLED_VERSION_FILE", self.installed)):
            m = mock.patch.object(ui, name, value)
            m.start()
            self.addCleanup(m.stop)
        m = mock.patch.object(ui, "ensure_root")
        m.start()
        self.addCleanup(m.stop)
        self.saved_progress = ui.PROGRESS
        self.addCleanup(setattr, ui, "PROGRESS", self.saved_progress)
