#!/usr/bin/env python3
"""Offline tests for /usr/libexec/thinfloe/thinfloe-automount (decisions).

The helper has no .py extension, so it is loaded through importlib like
peripheral-battery in test_peripheral_battery.py. Its entry point is
guarded, so importing it does nothing. Sysfs, /dev and /run/thinfloe are
fake trees under a temporary directory, named by the TF_* variables.
"""

import importlib.machinery
import importlib.util
import os
import shutil
import tempfile
import unittest
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
        for bad in ("", "garbage", "x\ty", "12\t", "\n"):
            self.assertIsNone(mod.parse_hold(bad), bad)


if __name__ == "__main__":
    unittest.main()
