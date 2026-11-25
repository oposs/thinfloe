#!/usr/bin/env python3
"""Offline tests for /usr/libexec/thinfloe/thinfloe-automount (decisions).

The helper has no .py extension, so it is loaded through importlib like
peripheral-battery in test_peripheral_battery.py. Its entry point is
guarded, so importing it does nothing. Sysfs, /dev and /run/thinfloe are
fake trees under a temporary directory, named by the TF_* variables.
"""

import importlib.machinery
import importlib.util
import io
import os
import shutil
import signal
import subprocess
import tempfile
import threading
import time
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "overlay" / "usr" / "libexec" / "thinfloe" / "thinfloe-automount"


def load():
    loader = importlib.machinery.SourceFileLoader("thinfloe_automount", str(SCRIPT))
    spec = importlib.util.spec_from_loader("thinfloe_automount", loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


mod = load()

ENV_NAMES = {
    "TF_SYSFS": "sys", "TF_DEV": "dev", "TF_RUN": "run", "TF_MEDIA": "media",
    "TF_CMDLINE": "cmdline", "TF_ESP_DEVICE_FILE": "esp-device",
    "TF_EFI_MOUNT": "efi",
}


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="automount-"))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        saved = {k: os.environ.get(k) for k in ENV_NAMES}
        self.addCleanup(self.restore_env, saved)
        for var, name in ENV_NAMES.items():
            os.environ[var] = str(self.tmp / name)
        for sub in ("sys/class/block", "dev", "run", "media"):
            (self.tmp / sub).mkdir(parents=True, exist_ok=True)
        self.p = mod.paths()

    @staticmethod
    def restore_env(saved):
        for k, v in saved.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v

    def add_disk(self, disk, parts=(), bus="usb", removable="1", mmc_type=None):
        """A fake sysfs disk; class/block entries link to it as in the kernel."""
        base = self.tmp / "sys" / "devices" / bus / disk
        base.mkdir(parents=True, exist_ok=True)
        (base / "removable").write_text(removable + "\n")
        (base / "size").write_text("2048\n")
        if mmc_type:
            (base / "device").mkdir(exist_ok=True)
            (base / "device" / "type").write_text(mmc_type + "\n")
        link = self.tmp / "sys" / "class" / "block" / disk
        link.symlink_to(base)
        for part in parts:
            pd = base / part
            pd.mkdir()
            (pd / "partition").write_text("1\n")
            (pd / "size").write_text("2048\n")
            (self.tmp / "sys" / "class" / "block" / part).symlink_to(pd)
        return base

    def write(self, name, text):
        path = self.tmp / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)


def make_dev(**kw):
    values = dict(
        kname="sdb1", disk="sdb", devnode="/dev/sdb1", fstype="vfat",
        label="STICK", uuid="1234-ABCD", partlabel="", size_bytes=1 << 30,
        model="Flash", id_bus="usb", removable=True, mmc_type=None,
        flash_flags=set(), usb_parent=None, serial="AA",
    )
    values.update(kw)
    return mod.Dev(**values)


class BootDisk(Base):
    def test_boot_disk_from_esp_file(self):
        for dev, disk in (("/dev/sdb1", "sdb"), ("/dev/mmcblk0p1", "mmcblk0"),
                          ("/dev/nvme0n1p1", "nvme0n1")):
            self.write("esp-device", dev + "\n")
            self.assertEqual(mod.boot_disk(self.p, lambda: None), disk)

    def test_boot_disk_uses_sysfs_for_the_parent(self):
        self.add_disk("sdb", parts=["sdb1"])
        self.write("esp-device", "/dev/sdb1\n")
        self.assertEqual(mod.boot_disk(self.p, lambda: None), "sdb")
        self.assertEqual(mod.disk_of("sdb1", self.p), "sdb")
        self.assertEqual(mod.disk_of("sdb", self.p), "sdb")

    def test_boot_disk_fallback_findmnt(self):
        self.assertEqual(mod.boot_disk(self.p, lambda: "/dev/sdc1"), "sdc")

    def test_disk_of_fallback_keeps_whole_disks(self):
        for kname, disk in (("mmcblk0", "mmcblk0"), ("nvme0n1", "nvme0n1"), ("loop0", "loop0"),
                            ("sdb", "sdb"), ("sdb1", "sdb"), ("sdaa12", "sdaa"),
                            ("mmcblk0p1", "mmcblk0"), ("nvme0n1p2", "nvme0n1"),
                            ("loop3p1", "loop3"), ("vdb2", "vdb")):
            self.assertEqual(mod.disk_of(kname, self.p), disk, kname)

    def test_boot_disk_netboot(self):
        self.write("cmdline", "quiet ramroot=http://10.0.2.2/x.sqsh rd.neednet=1\n")
        self.assertIsNone(mod.boot_disk(self.p, lambda: None))

    def test_boot_disk_unknown_on_stick(self):
        self.write("cmdline", "quiet ramroot=tl/ramroot.sqsh\n")
        boot = mod.boot_disk(self.p, lambda: None)
        self.assertIs(boot, mod.BootUnknown)
        reason = mod.skip_reason(make_dev(), boot, {}, set(), lambda pid: False)
        self.assertIn("boot disk unknown", reason)


class SkipRules(Base):
    def reason(self, d, boot=None, holds=None, mounted=(), alive=lambda pid: True):
        return mod.skip_reason(d, boot, holds or {}, set(mounted), alive)

    def test_skip_rules(self):
        self.assertIn("boot disk", self.reason(make_dev(), boot="sdb"))
        self.assertIn("ThinFloe stick", self.reason(make_dev(partlabel="EFI-SYSTEM")))
        hold = {"sdb": mod.Hold(1234, "sn-AA:16:3f")}
        self.assertIn("held", self.reason(make_dev(), holds=hold))
        self.assertIsNone(self.reason(make_dev(), holds=hold, alive=lambda pid: False))
        other = {"sdb": mod.Hold(1234, "sn-BB:16:3f")}
        self.assertIsNone(self.reason(make_dev(), holds=other))
        self.assertIn("eMMC", self.reason(make_dev(kname="mmcblk0p1", disk="mmcblk0", mmc_type="MMC")))
        self.assertIsNone(self.reason(make_dev(mmc_type="SD")))
        self.assertIn("no filesystem", self.reason(make_dev(fstype="")))
        self.assertIn("no medium", self.reason(make_dev(size_bytes=0)))
        self.assertIn("mounted", self.reason(make_dev(), mounted={"/dev/sdb1"}))
        self.assertIn("filesystem", self.reason(make_dev(fstype="iso9660")))
        self.assertIsNone(self.reason(make_dev()))

    def test_hold_with_colon_in_serial(self):
        hold = {"sdb": mod.Hold(1, "sn-AA:BB:16:3f")}
        self.assertIn("held", self.reason(make_dev(serial="AA:BB"), holds=hold))
        # A different serial is not held.
        self.assertIsNone(self.reason(make_dev(serial="CC"), holds=hold))

    def test_hold_without_serial_matches_nosn(self):
        hold = {"sdb": mod.Hold(1, "nosn:16:3f")}
        self.assertIn("held", self.reason(make_dev(serial=""), holds=hold))


class ReadDev(Base):
    def test_read_dev(self):
        self.add_disk("sdb", parts=["sdb1"])
        d = mod.read_dev("sdb1", {
            "ID_FS_TYPE": "vfat", "ID_FS_LABEL": "MYSTICK", "ID_FS_UUID": "1234-ABCD",
            "ID_PART_ENTRY_NAME": "EFI-SYSTEM", "ID_MODEL": "Flash", "ID_BUS": "usb",
            "ID_SERIAL_SHORT": "AA", "ID_DRIVE_FLASH_SD": "1",
        }, self.p)
        self.assertEqual((d.kname, d.disk, d.fstype, d.label, d.uuid, d.partlabel),
                         ("sdb1", "sdb", "vfat", "MYSTICK", "1234-ABCD", "EFI-SYSTEM"))
        self.assertEqual(d.devnode, str(self.tmp / "dev" / "sdb1"))
        self.assertEqual(d.size_bytes, 2048 * 512)
        self.assertEqual((d.model, d.id_bus, d.serial, d.removable), ("Flash", "usb", "AA", True))
        self.assertEqual(d.flash_flags, {"SD"})
        self.assertIsNone(d.mmc_type)

    def test_serial_order_follows_lsblk(self):
        order = ["ID_SCSI_SERIAL", "ID_SERIAL_SHORT", "ID_WWN_WITH_EXTENSION", "ID_WWN"]
        for i, key in enumerate(order):
            props = {k: k + "-v" for k in order[i:]}
            self.assertEqual(mod.drive_serial(props), key + "-v")
        self.assertEqual(mod.drive_serial({}), "")
        self.assertEqual(mod.read_dev("sdz1", {"ID_SCSI_SERIAL": "S", "ID_SERIAL_SHORT": "T"}, self.p).serial, "S")

    def test_read_dev_mmc_type_and_missing_sysfs(self):
        self.add_disk("mmcblk0", parts=["mmcblk0p1"], bus="mmc", removable="0", mmc_type="SD")
        d = mod.read_dev("mmcblk0p1", {}, self.p)
        self.assertEqual((d.disk, d.mmc_type, d.removable), ("mmcblk0", "SD", False))
        gone = mod.read_dev("sdz1", {}, self.p)
        self.assertEqual((gone.size_bytes, gone.fstype), (0, ""))


class Naming(Base):
    def test_name_from_label_uuid(self):
        self.assertEqual(mod.choose_name("MYSTICK", "", set()), "MYSTICK")
        self.assertEqual(mod.choose_name("", "1234-ABCD", set()), "1234-ABCD")
        self.assertEqual(mod.choose_name("MYSTICK", "", {"MYSTICK"}), "MYSTICK1")
        self.assertEqual(mod.choose_name("MYSTICK", "", {"MYSTICK", "MYSTICK1"}), "MYSTICK2")

    def test_name_hostile_labels(self):
        self.assertEqual(mod.choose_name("a/b", "", set()), "a_b")
        self.assertEqual(mod.choose_name("..", "", set()), "_.")
        self.assertEqual(mod.choose_name("   ", "U", set()), "U")
        self.assertEqual(mod.choose_name("x\ny", "", set()), "x_y")
        self.assertEqual(mod.choose_name("", "", set()), "disk")
        for label in ("a" * 300, "\xe4" * 200, "/", ".", "...", "\x00", " .x "):
            name = mod.choose_name(label, "", set())
            self.assertLessEqual(len(name.encode()), 255, label)
            self.assertNotIn("/", name)
            self.assertFalse(name.startswith("."), name)
            self.assertNotIn(name, ("", ".", ".."))
        name = mod.choose_name("\xe4" * 200, "", set())
        self.assertEqual(len(name.encode()), 254)

    def test_name_suffix_stays_within_limit(self):
        taken = {"a" * 255}
        name = mod.choose_name("a" * 300, "", taken)
        self.assertLessEqual(len(name.encode()), 255)
        self.assertNotIn(name, taken)


class Options(Base):
    def test_mount_options_table(self):
        tail = ",nosuid,nodev"
        self.assertEqual(mod.mount_options("vfat"),
                         "uid=1001,gid=1001,shortname=mixed,utf8=1,showexec,flush" + tail)
        self.assertEqual(mod.mount_options("exfat"),
                         "uid=1001,gid=1001,iocharset=utf8,errors=remount-ro" + tail)
        self.assertEqual(mod.mount_options("ntfs"), "uid=1001,gid=1001,windows_names" + tail)
        for fs in ("ext2", "ext3", "ext4", "btrfs", "xfs"):
            self.assertEqual(mod.mount_options(fs), "nosuid,nodev", fs)
        self.assertIsNone(mod.mount_options("iso9660"))
        self.assertIsNone(mod.mount_options(""))

    def test_fs_driver(self):
        self.assertEqual(mod.fs_driver("ntfs"), "ntfs3")
        for fs in ("vfat", "exfat", "ext4", "btrfs", "xfs"):
            self.assertEqual(mod.fs_driver(fs), fs)
        self.assertIn("windows_names", mod.mount_options("ntfs"))

    def test_is_card(self):
        self.assertTrue(mod.is_card(make_dev(flash_flags={"SD"}), 1))
        self.assertTrue(mod.is_card(make_dev(mmc_type="SD"), 1))
        self.assertTrue(mod.is_card(make_dev(), 2))
        self.assertFalse(mod.is_card(make_dev(), 1))


class Lines(Base):
    def test_lines_round_trip_and_tolerance(self):
        lines = [
            mod.Line("/dev/sdb1", "sdb", "MYSTICK", "/media/tfuser/MYSTICK", "vfat", 1024, "Flash", "stick"),
            mod.Line("/dev/sdc1", "sdc", "CARD", "/media/tfuser/CARD", "exfat", 2048, "Reader", "card"),
        ]
        text = mod.format_lines(lines)
        self.assertTrue(text.endswith("\n"))
        self.assertEqual(mod.parse_lines(text), lines)
        self.assertEqual(mod.format_lines([]), "")
        good = "\t".join(["/dev/sdb1", "sdb", "N", "/m/N", "vfat", "5", "M", "stick"])
        text = "\n".join([
            good + "\textra",
            "a\tb\tc",
            "",
            "\t".join(["/dev/sdc1", "sdc", "N2", "/m/N2", "vfat", "big", "M", "stick"]),
            "",
        ])
        parsed = mod.parse_lines(text)
        self.assertEqual(len(parsed), 1)
        self.assertEqual(parsed[0].size_bytes, 5)
        self.assertEqual(parsed[0].kind, "stick")

    def test_format_keeps_one_line_per_entry(self):
        line = mod.Line("/dev/sdb1", "sdb", "a\tb", "/m/a", "vfat", 1, "Mo\ndel", "stick")
        text = mod.format_lines([line])
        self.assertEqual(len(text.splitlines()), 1)
        self.assertEqual(len(mod.parse_lines(text)), 1)

    def test_parse_hold(self):
        self.assertEqual(mod.parse_hold("1234\tsn-AA:16:3f\n"), mod.Hold(1234, "sn-AA:16:3f"))
        for bad in ("", "garbage", "x\ty", "12\t", "\n", "0\tid", "-5\tid"):
            self.assertIsNone(mod.parse_hold(bad), bad)


FAKES = {
    "mount": '''#!/bin/sh
printf 'mount\t%s\n' "$*" | tr ' ' '\t' >> "$FAKE_STATE/calls.log"
case "$*" in *"$FAKE_MOUNT_HANG"*) if [ -n "$FAKE_MOUNT_HANG" ]; then echo $$ > "$FAKE_STATE/hang.pid"; exec sleep 30; fi;; esac
[ -n "$FAKE_MOUNT_FAIL" ] && { echo "wrong fs type" >&2; exit 32; }
exit 0
''',
    "umount": '''#!/bin/sh
printf 'umount\t%s\n' "$*" | tr ' ' '\t' >> "$FAKE_STATE/calls.log"
case " $* " in *" -l "*) exit 0;; esac
[ -n "$FAKE_UMOUNT_BUSY" ] && case "$*" in *"$FAKE_UMOUNT_BUSY"*) echo "umount: target is busy" >&2; exit 32;; esac
exit 0
''',
    "sync": '''#!/bin/sh
printf 'sync\t%s\n' "$*" | tr ' ' '\t' >> "$FAKE_STATE/calls.log"
''',
    "udevadm": '''#!/bin/sh
printf 'udevadm\t%s\n' "$*" | tr ' ' '\t' >> "$FAKE_STATE/calls.log"
''',
    "systemctl": '''#!/bin/sh
cat "$FAKE_STATE/systemd-state" 2>/dev/null || echo running
''',
}


class Commands(Base):
    """The commands against fake mount, umount, sync, udevadm and systemctl."""

    def setUp(self):
        super().setUp()
        saved = dict(os.environ)
        self.addCleanup(self.reset_env, saved)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        for name, body in FAKES.items():
            f = self.bin / name
            f.write_text(body)
            f.chmod(0o755)
        os.environ["PATH"] = f"{self.bin}:{os.environ['PATH']}"
        os.environ["FAKE_STATE"] = str(self.tmp)
        os.environ["TF_OWNER_UID"] = str(os.getuid())
        os.environ["TF_MOUNTINFO"] = str(self.tmp / "mountinfo")
        for k in ("FAKE_MOUNT_HANG", "FAKE_MOUNT_FAIL", "FAKE_UMOUNT_BUSY"):
            os.environ.pop(k, None)
        self.write("esp-device", "/dev/sda1\n")
        self.addCleanup(setattr, mod, "log", mod.log)
        mod.log = lambda msg: None
        self.saved_wait = mod.LOCK_WAIT
        self.addCleanup(setattr, mod, "LOCK_WAIT", self.saved_wait)

    @staticmethod
    def reset_env(saved):
        os.environ.clear()
        os.environ.update(saved)

    def stick(self, disk="sdb", label="MYSTICK", usb="1-1", kind_card=False, fstype="vfat",
              serial="AA", sectors=2048):
        """A disk with one partition, its device nodes and udev properties."""
        bus = f"usb/{usb}"
        (self.tmp / "sys" / "devices" / bus).mkdir(parents=True, exist_ok=True)
        (self.tmp / "sys" / "devices" / bus / "idVendor").write_text("1234\n")
        base = self.add_disk(disk, parts=[disk + "1"], bus=bus)
        for node in (base / "size", base / (disk + "1") / "size"):
            node.write_text(f"{sectors}\n")
        (base / (disk + "1") / "dev").write_text("8:17\n")
        (self.tmp / "dev" / disk).write_text("")
        (self.tmp / "dev" / (disk + "1")).write_text("")
        (self.tmp / "sys" / "bus" / "usb" / "devices" / usb).mkdir(parents=True, exist_ok=True)
        props = [f"ID_FS_TYPE={fstype}", f"ID_FS_LABEL={label}", "ID_FS_UUID=1234-ABCD",
                 "ID_MODEL=Flash", "ID_BUS=usb", f"ID_SERIAL_SHORT={serial}"]
        if kind_card:
            props.append("ID_DRIVE_FLASH_SD=1")
        os.environ[f"TF_UDEV_PROPS_{disk}1"] = "\n".join(props) + "\n"

    def unplug(self, disk):
        for name in (disk, disk + "1"):
            (self.tmp / "dev" / name).unlink()
            (self.tmp / "sys" / "class" / "block" / name).unlink()

    def cmd(self, *args):
        out = io.StringIO()
        with redirect_stdout(out), redirect_stderr(io.StringIO()):
            rc = mod.main(list(args))
        self.out = out.getvalue()
        return rc

    def calls(self, name=None):
        try:
            rows = (self.tmp / "calls.log").read_text().splitlines()
        except FileNotFoundError:
            return []
        rows = [r.split("\t") for r in rows]
        return [r for r in rows if name is None or r[0] == name]

    def lines(self):
        return mod.parse_lines((self.tmp / "run" / "removable").read_text()
                               if (self.tmp / "run" / "removable").exists() else "")

    def event(self):
        f = self.tmp / "run" / "removable.event"
        return f.read_text().rstrip("\n").split("\t") if f.exists() else None

    def poweroff_file(self, usb="1-1"):
        return self.tmp / "sys" / "bus" / "usb" / "devices" / usb / "remove"


class Add(Commands):
    def test_add_mounts_and_writes_line(self):
        self.stick()
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        mp = self.tmp / "media" / "MYSTICK"
        self.assertTrue(mp.is_dir())
        self.assertEqual(self.calls("mount")[0][1:], [
            "--no-canonicalize", "-t", "vfat", "-o",
            "uid=1001,gid=1001,shortname=mixed,utf8=1,showexec,flush,nosuid,nodev",
            str(self.tmp / "dev" / "sdb1"), str(mp)])
        self.assertEqual(self.lines(), [mod.Line(
            str(self.tmp / "dev" / "sdb1"), "sdb", "MYSTICK", str(mp), "vfat",
            2048 * 512, "Flash", "stick")])
        self.assertEqual(os.stat(self.tmp / "run" / "removable").st_mode & 0o777, 0o644)

    def test_add_ntfs_uses_ntfs3(self):
        self.stick(fstype="ntfs")
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(self.calls("mount")[0][2:4], ["-t", "ntfs3"])

    def test_add_card_reader_is_a_card(self):
        self.stick(kind_card=True)
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(self.lines()[0].kind, "card")

    def test_add_reader_with_two_luns_is_a_card(self):
        self.stick("sdb", label="A", usb="1-3")
        self.stick("sdc", label="B", usb="1-3")
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(self.lines()[0].kind, "card")

    def test_add_refuses_existing_entry_or_symlink(self):
        self.stick()
        target = self.tmp / "elsewhere"
        target.mkdir()
        (self.tmp / "media" / "MYSTICK").symlink_to(target)
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(self.lines()[0].name, "MYSTICK1")
        self.assertEqual(list(target.iterdir()), [])
        self.assertTrue((self.tmp / "media" / "MYSTICK").is_symlink())
        self.stick("sdc")
        (self.tmp / "media" / "MYSTICK2").write_text("a file")
        self.assertEqual(self.cmd("add", "sdc1"), 0)
        self.assertEqual({l.name for l in self.lines()}, {"MYSTICK1", "MYSTICK3"})

    def test_add_failed_mount_removes_dir(self):
        self.stick()
        os.environ["FAKE_MOUNT_FAIL"] = "1"
        self.assertEqual(self.cmd("add", "sdb1"), 1)
        self.assertEqual(list((self.tmp / "media").iterdir()), [])
        self.assertEqual(self.lines(), [])

    def test_add_same_label_three_times(self):
        self.stick("sdb", label="X")
        self.stick("sdc", label="X")
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(self.cmd("add", "sdc1"), 0)
        self.assertEqual([l.name for l in self.lines()], ["X", "X1"])
        self.unplug("sdb")
        self.assertEqual(self.cmd("remove", "sdb1"), 0)
        self.stick("sdd", label="X")
        self.assertEqual(self.cmd("add", "sdd1"), 0)
        self.assertEqual(sorted(l.name for l in self.lines()), ["X", "X1"])
        mounts = [c[-1] for c in self.calls("mount")]
        self.assertEqual(len(mounts), 3)
        self.assertEqual(mounts[0], mounts[2])

    def test_add_skips_boot_disk_and_other_cases(self):
        self.stick("sda")
        self.assertEqual(self.cmd("add", "sda1"), 0)
        self.stick("sdb", fstype="")
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(self.calls("mount"), [])
        self.assertEqual(self.lines(), [])

    def test_add_twice_mounts_once(self):
        self.stick()
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(len(self.calls("mount")), 1)

    def dead_pid(self):
        child = subprocess.Popen(["true"])
        child.wait()
        return child.pid

    def test_add_skips_device_mounted_elsewhere(self):
        self.stick()
        self.write("mountinfo", "36 25 8:17 / /mnt rw - vfat /dev/other rw\n")
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(self.calls("mount"), [])
        self.write("mountinfo", f"36 25 0:99 / /mnt rw - vfat {self.tmp}/dev/sdb1 rw\n")
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(self.calls("mount"), [])
        self.write("mountinfo", "36 25 8:18 / /mnt rw - vfat /dev/other rw\n")
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(len(self.calls("mount")), 1)

    def test_add_locked_rechecks_the_hold(self):
        self.stick()
        d = mod.read_dev("sdb1", mod.udev_props("sdb1", self.p), self.p)
        self.write("run/automount/hold/sdb", f"{os.getpid()}\tsn-AA:1:2\n")
        self.assertEqual(mod.add_locked(d, self.p), 0)
        self.assertEqual(self.calls("mount"), [])
        self.assertEqual(self.lines(), [])

    def test_stale_hold_is_removed_by_add(self):
        self.stick()
        hold = self.tmp / "run" / "automount" / "hold" / "sdb"
        self.write("run/automount/hold/sdb", f"{self.dead_pid()}\tsn-AA:1:2\n")
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(len(self.calls("mount")), 1)
        self.assertFalse(hold.exists())

    def test_hold_of_another_drive_is_removed_by_add(self):
        self.stick()
        hold = self.tmp / "run" / "automount" / "hold" / "sdb"
        self.write("run/automount/hold/sdb", f"{os.getpid()}\tsn-BB:1:2\n")
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(len(self.calls("mount")), 1)
        self.assertFalse(hold.exists())

    def test_live_hold_file_is_kept(self):
        self.stick()
        hold = self.tmp / "run" / "automount" / "hold" / "sdb"
        self.write("run/automount/hold/sdb", f"{os.getpid()}\tsn-AA:1:2\n")
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertTrue(hold.exists())

    def test_add_under_hold_skips(self):
        self.stick()
        self.write("run/automount/hold/sdb", f"{os.getpid()}\tsn-AA:2048:ff\n")
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(self.calls("mount"), [])
        self.assertEqual(self.lines(), [])


class Remove(Commands):
    def added(self, disk="sdb", **kw):
        self.stick(disk, **kw)
        self.assertEqual(self.cmd("add", disk + "1"), 0)

    def test_remove_after_pull_is_unclean(self):
        self.added()
        self.unplug("sdb")
        self.assertEqual(self.cmd("remove", "sdb1"), 0)
        self.assertEqual(self.calls("umount")[0][1:2], ["-l"])
        self.assertEqual(self.lines(), [])
        self.assertEqual(self.event(), ["1", "MYSTICK", "unclean"])
        self.assertFalse((self.tmp / "media" / "MYSTICK").exists())

    def test_remove_card_pulled_from_reader(self):
        self.added(kind_card=True)
        (self.tmp / "sys" / "class" / "block" / "sdb1" / "size").write_text("0\n")
        self.assertEqual(self.cmd("remove", "sdb1"), 0)
        self.assertEqual(self.calls("umount")[0][1], "-l")
        self.assertEqual(self.event(), ["1", "MYSTICK", "unclean"])
        self.assertEqual(self.lines(), [])

    def test_remove_at_shutdown_is_quiet(self):
        self.added()
        self.write("systemd-state", "stopping\n")
        self.assertEqual(self.cmd("remove", "sdb1"), 0)
        self.assertEqual([c[0] for c in self.calls() if c[0] != "mount"], ["sync", "umount"])
        self.assertEqual(self.calls("sync")[0][1], "-f")
        self.assertNotIn("-l", self.calls("umount")[0])
        self.assertEqual(self.lines(), [])
        self.assertIsNone(self.event())

    def test_remove_unknown_device_is_fine(self):
        self.assertEqual(self.cmd("remove", "sdq1"), 0)
        self.assertEqual(self.calls(), [])

    def test_remove_after_eject_records_nothing(self):
        self.added()
        self.assertEqual(self.cmd("eject", "sdb"), 0)
        self.assertEqual(self.event(), ["1", "MYSTICK", "clean"])
        self.unplug("sdb")
        self.assertEqual(self.cmd("remove", "sdb1"), 0)
        self.assertEqual(self.event(), ["1", "MYSTICK", "clean"])

    def test_remove_present_medium_is_clean_whatever_the_state(self):
        self.added()
        self.assertEqual(self.cmd("remove", "sdb1"), 0)
        self.assertEqual([c[0] for c in self.calls() if c[0] != "mount"], ["sync", "umount"])
        self.assertNotIn("-l", self.calls("umount")[0])
        self.assertEqual(self.lines(), [])
        self.assertIsNone(self.event())

    def test_remove_busy_while_stopping_records_nothing(self):
        self.added()
        self.write("systemd-state", "stopping\n")
        os.environ["FAKE_UMOUNT_BUSY"] = "media"
        self.assertEqual(self.cmd("remove", "sdb1"), 0)
        self.assertEqual(self.calls("umount")[-1][1], "-l")
        self.assertEqual(self.lines(), [])
        self.assertIsNone(self.event())

    def test_pull_with_busy_mount(self):
        self.added(label="X")
        os.environ["FAKE_UMOUNT_BUSY"] = "media"   # only a plain umount is busy
        self.unplug("sdb")
        self.assertEqual(self.cmd("remove", "sdb1"), 0)
        self.assertEqual(self.lines(), [])
        self.assertEqual(self.event(), ["1", "X", "unclean"])
        self.stick("sdc", label="X")
        self.assertEqual(self.cmd("add", "sdc1"), 0)
        self.assertEqual(self.lines()[0].name, "X")


class Eject(Commands):
    def added(self, disk="sdb", **kw):
        self.stick(disk, **kw)
        self.assertEqual(self.cmd("add", disk + "1"), 0)

    def test_eject_busy(self):
        self.added()
        os.environ["FAKE_UMOUNT_BUSY"] = "media"
        self.assertEqual(self.cmd("eject", "sdb"), 3)
        self.assertEqual(len(self.lines()), 1)
        self.assertFalse(self.poweroff_file().exists())
        self.assertIsNone(self.event())

    def test_eject_powers_off_stick_not_card(self):
        self.added("sdb", usb="1-1")
        self.added("sdc", usb="1-2", kind_card=True, label="CARD")
        self.assertEqual(self.cmd("eject", "sdb"), 0)
        self.assertEqual(self.poweroff_file("1-1").read_text(), "1")
        self.assertEqual(self.calls("sync")[0][1], "-f")
        self.assertEqual(self.cmd("eject", "sdc"), 0)
        self.assertFalse(self.poweroff_file("1-2").exists())
        self.assertEqual(self.lines(), [])

    def test_eject_builtin_sd_not_powered_off(self):
        (self.tmp / "sys" / "devices" / "pci").mkdir(parents=True)
        base = self.add_disk("mmcblk0", parts=["mmcblk0p1"], bus="mmc", removable="0", mmc_type="SD")
        (self.tmp / "dev" / "mmcblk0p1").write_text("")
        os.environ["TF_UDEV_PROPS_mmcblk0p1"] = "ID_FS_TYPE=exfat\nID_FS_LABEL=SD\n"
        self.assertEqual(self.cmd("add", "mmcblk0p1"), 0)
        self.assertEqual(self.lines()[0].kind, "card")
        self.assertEqual(self.cmd("eject", "mmcblk0"), 0)
        self.assertEqual(self.lines(), [])

    def test_eject_all(self):
        self.added("sdb", usb="1-1", label="A")
        self.added("sdc", usb="1-2", label="B")
        self.assertEqual(self.cmd("eject", "all"), 0)
        self.assertEqual(self.lines(), [])
        self.assertEqual(self.event()[0], "2")
        self.assertEqual(self.event()[2], "clean")

    def two_partitions(self):
        self.added()
        base = self.tmp / "sys" / "devices" / "usb" / "1-1" / "sdb"
        pd = base / "sdb2"
        pd.mkdir()
        (pd / "partition").write_text("2\n")
        (pd / "size").write_text("2048\n")
        (self.tmp / "sys" / "class" / "block" / "sdb2").symlink_to(pd)
        (self.tmp / "dev" / "sdb2").write_text("")
        os.environ["TF_UDEV_PROPS_sdb2"] = "ID_FS_TYPE=vfat\nID_FS_LABEL=SECOND\nID_SERIAL_SHORT=AA\n"
        self.assertEqual(self.cmd("add", "sdb2"), 0)
        self.assertEqual([l.name for l in self.lines()], ["MYSTICK", "SECOND"])

    def test_eject_first_partition_busy_changes_nothing(self):
        self.two_partitions()
        os.environ["FAKE_UMOUNT_BUSY"] = "MYSTICK"
        self.assertEqual(self.cmd("eject", "sdb"), 3)
        self.assertEqual([l.name for l in self.lines()], ["MYSTICK", "SECOND"])
        self.assertEqual(len(self.calls("umount")), 1)
        self.assertFalse(self.poweroff_file().exists())
        self.assertIsNone(self.event())

    def test_eject_later_partition_busy_reports_what_stays(self):
        self.two_partitions()
        os.environ["FAKE_UMOUNT_BUSY"] = "SECOND"
        self.assertEqual(self.cmd("eject", "sdb"), 1)
        self.assertEqual([l.name for l in self.lines()], ["SECOND"])
        self.assertIn("SECOND", self.out)
        self.assertNotIn("MYSTICK", self.out)
        self.assertFalse(self.poweroff_file().exists())
        self.assertIsNone(self.event())
        self.assertFalse((self.tmp / "media" / "MYSTICK").exists())

    def test_release_under_hold_unmounts(self):
        self.added()
        self.write("run/automount/hold/sdb", f"{os.getpid()}\tsn-AA:1:2\n")
        self.assertEqual(self.cmd("release", "sdb"), 0)
        self.assertEqual(self.lines(), [])
        self.assertIsNone(self.event())

    def test_eject_removes_a_stale_hold(self):
        self.added()
        child = subprocess.Popen(["true"])
        child.wait()
        hold = self.tmp / "run" / "automount" / "hold" / "sdb"
        self.write("run/automount/hold/sdb", f"{child.pid}\tsn-AA:1:2\n")
        self.assertEqual(self.cmd("eject", "sdb"), 0)
        self.assertEqual(self.lines(), [])
        self.assertFalse(hold.exists())

    def test_release_records_no_event(self):
        self.added()
        self.assertEqual(self.cmd("release", "sdb"), 0)
        self.assertEqual(self.lines(), [])
        self.assertIsNone(self.event())
        self.assertFalse(self.poweroff_file().exists())

    def test_release_then_remount_mounts_again(self):
        # The unit of sdb1 stays active after release; remount must not rely
        # on udev starting it again: it mounts by itself, no udevadm trigger.
        self.added()
        self.assertEqual(self.cmd("release", "sdb"), 0)
        self.assertEqual(self.lines(), [])
        self.assertEqual(self.cmd("remount", "sdb"), 0)
        mp = self.tmp / "media" / "MYSTICK"
        self.assertEqual([(l.device, l.mountpoint) for l in self.lines()],
                         [(str(self.tmp / "dev" / "sdb1"), str(mp))])
        self.assertTrue(mp.is_dir())
        self.assertEqual(len(self.calls("mount")), 2)
        self.assertEqual(self.calls("udevadm"), [])
        self.assertIsNone(self.event())

    def test_remount_twice_mounts_once(self):
        self.added()
        self.assertEqual(self.cmd("remount", "sdb"), 0)
        self.assertEqual(len(self.calls("mount")), 1)
        self.assertEqual(len(self.lines()), 1)

    def test_remount_honours_a_live_hold(self):
        self.added()
        self.assertEqual(self.cmd("release", "sdb"), 0)
        self.write("run/automount/hold/sdb", f"{os.getpid()}\tsn-AA:1:2\n")
        self.assertEqual(self.cmd("remount", "sdb"), 0)
        self.assertEqual(self.lines(), [])
        self.assertEqual(len(self.calls("mount")), 1)

    def test_remount_failed_mount_is_a_failure(self):
        self.added()
        self.assertEqual(self.cmd("release", "sdb"), 0)
        os.environ["FAKE_MOUNT_FAIL"] = "1"
        self.assertEqual(self.cmd("remount", "sdb"), 1)
        self.assertEqual(self.lines(), [])

    def test_remount_skips_the_boot_disk(self):
        self.stick("sdc", usb="1-2", label="BOOT", serial="BB")
        self.write("esp-device", "/dev/sdc1\n")
        self.assertEqual(self.cmd("remount", "sdc"), 0)
        self.assertEqual(self.calls("mount"), [])

    def test_eject_twice_and_under_hold(self):
        self.added()
        self.assertEqual(self.cmd("eject", "sdb"), 0)
        self.assertEqual(self.cmd("eject", "sdb"), 0)
        self.assertIn("nothing to eject", self.out)
        self.assertEqual(len(self.calls("umount")), 1)
        self.assertEqual(self.cmd("eject", "all"), 0)
        self.assertIn("nothing to eject", self.out)
        # A live hold keeps eject out; add skips (see test_add_under_hold_skips).
        self.stick("sdc", usb="1-2", label="H", serial="BB")
        self.assertEqual(self.cmd("add", "sdc1"), 0)
        self.write("run/automount/hold/sdc", f"{os.getpid()}\tsn-BB:1:2\n")
        self.assertEqual(self.cmd("eject", "sdc"), 3)
        self.assertEqual(len(self.lines()), 1)

    def test_hung_disk_does_not_block_others(self):
        mod.LOCK_WAIT = 1.0
        self.stick("sdb", usb="1-1")
        self.stick("sdc", usb="1-2", label="OK")
        os.environ["FAKE_MOUNT_HANG"] = "sdb1"
        result = []
        t = threading.Thread(target=lambda: result.append(self.cmd("add", "sdb1")))
        t.start()
        pidfile = self.tmp / "hang.pid"
        try:
            for _ in range(100):
                if pidfile.exists() and pidfile.read_text().strip():
                    break
                time.sleep(0.05)
            self.assertTrue(pidfile.exists())
            start = time.monotonic()
            self.assertEqual(self.cmd("add", "sdc1"), 0)
            self.assertLess(time.monotonic() - start, 3)
            start = time.monotonic()
            self.assertEqual(self.cmd("eject", "sdb"), 3)
            self.assertLess(time.monotonic() - start, 6)
        finally:
            if pidfile.exists() and pidfile.read_text().strip():
                try:
                    os.kill(int(pidfile.read_text()), signal.SIGKILL)
                except ProcessLookupError:
                    pass
            t.join(10)
        self.assertEqual([l.name for l in self.lines()], ["OK"])


class Usage(Commands):
    def test_usage(self):
        for argv in ([], ["add"], ["bogus", "sdb"], ["add", "sdb", "x"], ["eject", ""]):
            self.assertEqual(self.cmd(*argv), 2, argv)


if __name__ == "__main__":
    unittest.main()
