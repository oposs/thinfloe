#!/usr/bin/env python3
"""Offline tests for /usr/libexec/thinfloe/thinfloe-automount (decisions).

The helper has no .py extension, so it is loaded through importlib like
peripheral-battery in test_peripheral_battery.py. Its entry point is
guarded, so importing it does nothing. Sysfs, /dev and /run/thinfloe are
fake trees under a temporary directory, named by the TF_* variables.
"""

import importlib.machinery
import importlib.util
import fcntl
import io
import os
import re
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
        self.assertEqual(self.reason(make_dev(), boot="sdb"), "boot disk")
        self.assertEqual(self.reason(make_dev(partlabel="EFI-SYSTEM")), "ThinFloe stick")
        hold = {"sdb": mod.Hold(1234, "sn-AA:16:3f")}
        self.assertEqual(self.reason(make_dev(), holds=hold), "held by a write")
        self.assertIsNone(self.reason(make_dev(), holds=hold, alive=lambda pid: False))
        other = {"sdb": mod.Hold(1234, "sn-BB:16:3f")}
        self.assertIsNone(self.reason(make_dev(), holds=other))
        self.assertEqual(self.reason(make_dev(kname="mmcblk0p1", disk="mmcblk0", mmc_type="MMC")),
                         "eMMC or unknown card type")
        self.assertIsNone(self.reason(make_dev(mmc_type="SD")))
        # Exact reasons: "no filesystem" and "filesystem iso9660 is not
        # mounted" share words with "already mounted".
        self.assertEqual(self.reason(make_dev(fstype="")), "no filesystem")
        self.assertEqual(self.reason(make_dev(size_bytes=0)), "no medium")
        self.assertEqual(self.reason(make_dev(), mounted={"/dev/sdb1"}), "already mounted")
        self.assertEqual(self.reason(make_dev(fstype="iso9660")), "filesystem iso9660 is not mounted")
        self.assertIsNone(self.reason(make_dev()))

    def test_internal_disks_are_skipped(self):
        # The udev rule's candidate test, done again by the helper.
        for d in (make_dev(removable=False, id_bus="ata"),
                  make_dev(kname="nvme0n1p1", disk="nvme0n1", removable=False, id_bus=""),
                  make_dev(kname="sdb1", removable=False, id_bus="scsi", mmc_type="SD")):
            self.assertIn("not removable", self.reason(d), d.kname)
        self.assertIsNone(self.reason(make_dev(removable=True, id_bus="ata")))
        self.assertIsNone(self.reason(make_dev(removable=False, id_bus="usb")))
        self.assertIsNone(self.reason(make_dev(kname="mmcblk0p1", disk="mmcblk0",
                                               removable=False, id_bus="", mmc_type="SD")))

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

    def test_label_from_label_enc(self):
        # udev's ID_FS_LABEL has "_" for every blank; ID_FS_LABEL_ENC keeps it.
        props = {"ID_FS_LABEL": "My_Passport", "ID_FS_LABEL_ENC": "My\\x20Passport"}
        self.assertEqual(mod.fs_label(props), "My Passport")
        self.assertEqual(mod.choose_name(mod.fs_label(props), "", set()), "My Passport")
        self.assertEqual(mod.fs_label({"ID_FS_LABEL": "OLD"}), "OLD")
        for enc, name in (("\\x2e\\x2e", "_."), ("a\\x2fb", "a_b"), ("x\\x0ay", "x_y"),
                          ("\\x20\\x20", "disk"), ("\xdcber", "\xdcber"),
                          ("\\xc3\\xbcber", "\xfcber"), ("a\\xc2\\x85b", "a_b"),
                          ("back\\x5cslash", "back\\slash")):
            self.assertEqual(mod.choose_name(mod.fs_label({"ID_FS_LABEL_ENC": enc}), "", set()),
                             name, enc)
        self.assertEqual(mod.choose_name("a\x9bb", "", set()), "a_b")

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


# mount and umount keep $TF_MOUNTINFO as the kernel keeps mountinfo: a line
# per mount, the mount point with a blank written as \040. "/" is always
# there, as in the kernel's file: an empty mountinfo is a failed read.
ROOT_MOUNT = "1 0 0:1 / / rw - ext4 /dev/root rw\n"
FAKES = {
    "mount": '''#!/bin/sh
printf 'mount\t%s\n' "$*" | tr ' ' '\t' >> "$FAKE_STATE/calls.log"
case "$*" in *"$FAKE_MOUNT_HANG"*) if [ -n "$FAKE_MOUNT_HANG" ]; then echo $$ > "$FAKE_STATE/hang.pid"; exec sleep 30; fi;; esac
case "$*" in *"$FAKE_MOUNT_SLOW"*) [ -n "$FAKE_MOUNT_SLOW" ] && sleep 1.5;; esac
[ -n "$FAKE_MOUNT_FAIL" ] && { echo "wrong fs type" >&2; exit 32; }
prev=; dev=; for a; do dev=$prev; prev=$a; done
esc=$(printf '%s' "$prev" | sed 's/ /\\\\040/g')
printf '99 25 0:99 / %s rw - vfat %s rw\n' "$esc" "$dev" >> "$TF_MOUNTINFO"
exit 0
''',
    "umount": '''#!/bin/sh
printf 'umount\t%s\n' "$*" | tr ' ' '\t' >> "$FAKE_STATE/calls.log"
for a; do mp=$a; done
case " $* " in *" -l "*) ;; *)
  [ -n "$FAKE_UMOUNT_BUSY" ] && case "$*" in *"$FAKE_UMOUNT_BUSY"*) echo "umount: target is busy" >&2; exit 32;; esac;;
esac
esc=$(printf '%s' "$mp" | sed 's/ /\\\\040/g')
if [ -f "$TF_MOUNTINFO" ]; then
  MP="$esc" awk '$5 != ENVIRON["MP"]' "$TF_MOUNTINFO" > "$TF_MOUNTINFO.new" && mv "$TF_MOUNTINFO.new" "$TF_MOUNTINFO"
fi
[ -n "$FAKE_UMOUNT_KILL" ] && kill -9 $PPID
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
        (self.tmp / "mountinfo").write_text(ROOT_MOUNT)
        for k in ("FAKE_MOUNT_HANG", "FAKE_MOUNT_SLOW", "FAKE_MOUNT_FAIL", "FAKE_UMOUNT_BUSY",
                  "FAKE_UMOUNT_KILL"):
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


class Guards(Commands):
    """The final review's findings: candidacy, arguments, ghost lines, locks."""

    def internal_disk(self):
        base = self.add_disk("sda", parts=["sda2"], bus="pci/ata1", removable="0")
        (base / "sda2" / "dev").write_text("8:2\n")
        (self.tmp / "dev" / "sda2").write_text("")
        os.environ["TF_UDEV_PROPS_sda2"] = "ID_FS_TYPE=ntfs\nID_FS_LABEL=Windows\nID_BUS=ata\n"
        self.write("esp-device", "/dev/sdz1\n")

    def test_remount_of_an_internal_disk_mounts_nothing(self):
        self.internal_disk()
        self.assertEqual(self.cmd("remount", "sda"), 0)
        self.write("run/automount/released/sda", "sda2\n")
        self.assertEqual(self.cmd("remount", "sda"), 0)
        self.assertEqual(self.cmd("add", "sda2"), 0)
        self.assertEqual(self.calls("mount"), [])
        self.assertEqual(self.lines(), [])

    def test_commands_take_only_kernel_names(self):
        self.stick()
        for cmd in ("add", "remove", "eject", "release", "remount"):
            for bad in (".", "..", "/dev/sdb1", "sdb/1", "../sdb1", "-x", "a" * 40, "sd b", "*"):
                self.assertEqual(self.cmd(cmd, bad), 2, (cmd, bad))
        self.assertEqual(self.calls("mount"), [])

    def second_partition(self, label, fstype="vfat"):
        base = self.tmp / "sys" / "devices" / "usb" / "1-1" / "sdb"
        (base / "sdb2").mkdir()
        (base / "sdb2" / "partition").write_text("2\n")
        (base / "sdb2" / "size").write_text("2048\n")
        (self.tmp / "sys" / "class" / "block" / "sdb2").symlink_to(base / "sdb2")
        (self.tmp / "dev" / "sdb2").write_text("")
        os.environ["TF_UDEV_PROPS_sdb2"] = f"ID_FS_TYPE={fstype}\nID_FS_LABEL={label}\nID_BUS=usb\n"

    def test_remount_mounts_only_what_release_took(self):
        self.stick()
        self.second_partition("OTHER")
        self.assertEqual(self.cmd("add", "sdb1"), 0)   # sdb2 is not mounted (ejected, say)
        self.assertEqual(self.cmd("release", "sdb"), 0)
        self.assertEqual(self.cmd("remount", "sdb"), 0)
        self.assertEqual([l.name for l in self.lines()], ["MYSTICK"])
        self.assertEqual(len(self.calls("mount")), 2)
        self.assertFalse((self.tmp / "run" / "automount" / "released" / "sdb").exists())

    def test_ghost_line_does_not_block_the_next_stick(self):
        # A line whose mount is gone (a helper killed between umount and
        # its commit): the next stick under the same name is mounted.
        self.stick()
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        (self.tmp / "mountinfo").write_text(ROOT_MOUNT)
        self.unplug("sdb")
        self.stick(label="NEXT", serial="BB", usb="1-2")
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual([l.name for l in self.lines()], ["NEXT"])
        self.assertEqual(len(self.calls("mount")), 2)
        self.assertFalse((self.tmp / "media" / "MYSTICK").exists())

    def test_ghost_line_is_no_unclean_removal(self):
        self.stick()
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        (self.tmp / "mountinfo").write_text(ROOT_MOUNT)
        self.unplug("sdb")
        self.assertEqual(self.cmd("remove", "sdb1"), 0)
        self.assertEqual(self.lines(), [])
        self.assertIsNone(self.event())
        self.assertEqual(self.calls("umount"), [])

    def test_eject_drops_a_ghost_line(self):
        self.stick()
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        (self.tmp / "mountinfo").write_text(ROOT_MOUNT)
        self.assertEqual(self.cmd("eject", "sdb"), 0)
        self.assertIn("nothing to eject", self.out)
        self.assertEqual(self.lines(), [])

    def test_killed_release_is_mounted_again(self):
        # The writer's cancel kills release after its umount, before the
        # line is dropped; the remount in the writer's finally mounts it.
        self.stick()
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        env = dict(os.environ, FAKE_UMOUNT_KILL="1")
        r = subprocess.run(["python3", str(SCRIPT), "release", "sdb"], env=env,
                           capture_output=True, timeout=60)
        self.assertEqual(r.returncode, -9)
        self.assertEqual(len(self.lines()), 1, "the killed release left its line")
        self.assertEqual(self.cmd("remount", "sdb"), 0)
        self.assertEqual(len(self.calls("mount")), 2)
        mp = str(self.tmp / "media" / "MYSTICK")
        self.assertEqual([l.mountpoint for l in self.lines()], [mp])
        self.assertIn(mp, (self.tmp / "mountinfo").read_text())

    def test_second_partition_waits_for_a_slow_first(self):
        mod.LOCK_WAIT = 0.3
        self.stick()
        self.second_partition("DATA", "exfat")
        os.environ["FAKE_MOUNT_SLOW"] = "sdb1"
        result = []
        t = threading.Thread(target=lambda: result.append(self.cmd("add", "sdb1")))
        t.start()
        for _ in range(100):
            if self.calls("mount"):
                break
            time.sleep(0.02)
        self.assertEqual(self.cmd("add", "sdb2"), 0)
        t.join(10)
        self.assertEqual(result, [0])
        self.assertEqual(sorted(l.name for l in self.lines()), ["DATA", "MYSTICK"])

    def test_label_with_blank_is_kept(self):
        self.stick(label="My_Passport")
        os.environ["TF_UDEV_PROPS_sdb1"] += "ID_FS_LABEL_ENC=My\\x20Passport\n"
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        mp = self.tmp / "media" / "My Passport"
        self.assertTrue(mp.is_dir())
        self.assertEqual([l.mountpoint for l in self.lines()], [str(mp)])
        self.assertIn("My\\040Passport", (self.tmp / "mountinfo").read_text())
        # mountinfo's escaped blank is read back: the line is still trusted.
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        self.assertEqual(len(self.calls("mount")), 1)
        self.assertEqual(self.cmd("eject", "sdb"), 0)
        self.assertEqual(self.lines(), [])
        self.assertFalse(mp.exists())

    def test_budgets_fit_the_unit(self):
        unit = (REPO / "overlay" / "etc" / "systemd" / "system"
                / "thinfloe-automount@.service").read_text()
        start = int(re.search(r"^TimeoutStartSec=(\d+)$", unit, re.M).group(1))
        stop = int(re.search(r"^TimeoutStopSec=(\d+)$", unit, re.M).group(1))
        self.assertLess(mod.ADD_BUDGET + 5, start)
        self.assertLess(mod.REMOVE_BUDGET + 3, stop)
        self.assertLessEqual(mod.MOUNT_TIMEOUT, mod.ADD_BUDGET - mod.STEP_RESERVE)
        self.assertLessEqual(mod.Deadline(2.0).step(), 2.0)
        self.assertEqual(mod.Deadline(1000).step(), mod.MOUNT_TIMEOUT)
        self.assertEqual(mod.Deadline(0).lock_wait(), 0.0)
        self.assertEqual(mod.Deadline(0).step(), 0.0)

    def test_no_time_left_starts_nothing(self):
        self.assertEqual(mod.run(["mount", "/dev/sdb1", "/x"], 0)[0], 124)
        self.assertEqual(self.calls("mount"), [])

    def test_hung_command_is_given_up(self):
        start = time.monotonic()
        rc, _, _ = mod.run(["sh", "-c", "sleep 30 & wait"], 0.5)
        self.assertEqual(rc, 124)
        self.assertLess(time.monotonic() - start, 3)


class FakeClock:
    """time for the helper: sleep moves the clock; at release_at the held
    disk lock is let go, as an add that ends just in time."""

    def __init__(self, release_at, held_fd):
        self.now = 0.0
        self.release_at = release_at
        self.held_fd = held_fd

    def monotonic(self):
        return self.now

    def sleep(self, s):
        self.now += s
        if self.held_fd is not None and self.now >= self.release_at:
            os.close(self.held_fd)
            self.held_fd = None


class FollowUps(Commands):
    """The follow-up list: budgets, mountinfo, leftover directories, holds."""

    def worst_case(self, disk, release_at):
        """Hold the disk's lock until release_at; every command then runs
        into its timeout. Returns the clock and the commands run."""
        lock_dir = self.tmp / "run" / "automount" / "lock"
        lock_dir.mkdir(parents=True, exist_ok=True)
        fd = os.open(lock_dir / disk, os.O_RDWR | os.O_CREAT, 0o600)
        fcntl.flock(fd, fcntl.LOCK_EX)
        clock = FakeClock(release_at, fd)
        ran = []

        def run(argv, timeout):
            if timeout <= 0:
                return 124, "", "no time left"
            ran.append((argv[0], argv[1:2], timeout))
            clock.now += timeout + mod.KILL_WAIT
            return 124, "", "timed out"
        for name, value in (("time", clock), ("run", run)):
            self.addCleanup(setattr, mod, name, getattr(mod, name))
            setattr(mod, name, value)
        return clock, ran

    def test_remove_ends_within_its_budget(self):
        self.stick()
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        del os.environ["TF_UDEV_PROPS_sdb1"]   # udevadm runs, and hangs
        clock, ran = self.worst_case("sdb", mod.REMOVE_BUDGET - mod.STEP_RESERVE - 0.05)
        self.assertEqual(self.cmd("remove", "sdb1"), 0)
        self.assertLessEqual(clock.now, mod.REMOVE_BUDGET + 0.01)
        # Every step had its turn, the umount -l last.
        self.assertEqual([r[0] for r in ran], ["udevadm", "sync", "umount", "umount"])
        self.assertEqual(ran[-1][1], ["-l"])
        self.assertEqual(self.lines(), [])

    def test_add_ends_within_its_budget(self):
        self.stick()
        clock, ran = self.worst_case("sdb", mod.ADD_BUDGET - mod.STEP_RESERVE - 0.05)
        self.assertEqual(self.cmd("add", "sdb1"), 1)
        self.assertLessEqual(clock.now, mod.ADD_BUDGET + 0.01)
        self.assertEqual([r[0] for r in ran], ["mount", "umount"])
        self.assertEqual(ran[-1][1], ["-l"])

    def test_empty_mountinfo_drops_no_line(self):
        self.stick()
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        mp = self.tmp / "media" / "MYSTICK"
        for text in ("", None):
            if text is None:
                (self.tmp / "mountinfo").unlink()
            else:
                (self.tmp / "mountinfo").write_text(text)
            mod.prune(self.p)
            self.assertEqual([ln.mountpoint for ln in self.lines()], [str(mp)])
            self.assertTrue(mp.is_dir())

    def test_leftover_empty_directories_are_removed(self):
        media = self.tmp / "media"
        outside = self.tmp / "outside"
        outside.mkdir()
        for name in ("LEFT", "FULL", "BUSY", "DEAD"):
            (media / name).mkdir()
        (media / "FULL" / "file").write_text("x")
        (media / "LINK").symlink_to(outside)
        (media / "PLAIN").write_text("")
        dead = subprocess.Popen(["true"])
        dead.wait()
        self.write("run/automount/reserved/BUSY", f"{os.getpid()}\n")
        self.write("run/automount/reserved/DEAD", f"{dead.pid}\n")
        self.assertEqual(self.cmd("remove", "sdz1"), 0)
        self.assertEqual(sorted(os.listdir(media)), ["BUSY", "FULL", "LINK", "PLAIN"])
        self.assertTrue(outside.is_dir())
        self.assertFalse((self.tmp / "run" / "automount" / "reserved" / "DEAD").exists())

    def test_killed_add_leaves_no_directory_behind(self):
        self.stick()
        env = dict(os.environ, FAKE_MOUNT_HANG="sdb1")
        proc = subprocess.Popen(["python3", str(SCRIPT), "add", "sdb1"], env=env,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        hang = self.tmp / "hang.pid"
        for _ in range(500):
            if hang.exists() and hang.read_text().strip():
                break
            time.sleep(0.02)
        proc.kill()
        proc.wait()
        os.kill(int(hang.read_text()), 9)
        mp = self.tmp / "media" / "MYSTICK"
        self.assertTrue(mp.is_dir(), "killed between its mkdir and its mount")
        self.assertEqual(self.cmd("eject", "all"), 0)
        self.assertFalse(mp.exists())

    def test_a_live_add_keeps_its_directory(self):
        # Another stick's command runs while this add mounts: the reserved
        # directory stays.
        self.stick()
        d = mod.read_dev("sdb1", mod.udev_props("sdb1", self.p), self.p)
        name, mp = mod.reserve_mountpoint(d, self.p)
        mod.prune(self.p)
        self.assertTrue(os.path.isdir(mp))
        mod.unreserve(self.p, name)
        mod.prune(self.p)
        self.assertFalse(os.path.exists(mp))

    def test_a_directory_not_ours_leaks_nothing(self):
        self.stick()
        os.environ["TF_OWNER_UID"] = str(os.getuid() + 1)
        self.assertEqual(self.cmd("add", "sdb1"), 1)
        self.assertEqual(os.listdir(self.tmp / "media"), [])
        self.assertEqual(self.calls("mount"), [])

    def test_a_hold_written_while_a_stale_one_is_read_is_kept(self):
        self.stick()
        hold = self.tmp / "run" / "automount" / "hold" / "sdb"
        self.write("run/automount/hold/sdb", "999999\tsn-AA:1:1\n")
        fresh = f"{os.getpid()}\tsn-AA:2:2\n"
        real = mod.pid_alive

        def writer_comes(pid):
            # The writer puts its new hold in place by rename.
            tmp = hold.with_name(".new")
            tmp.write_text(fresh)
            os.replace(tmp, hold)
            return False
        self.addCleanup(setattr, mod, "pid_alive", real)
        mod.pid_alive = writer_comes
        d = mod.read_dev("sdb1", mod.udev_props("sdb1", self.p), self.p)
        self.assertFalse(mod.live_hold(self.p, d))
        self.assertEqual(hold.read_text(), fresh)
        mod.pid_alive = lambda pid: False
        self.assertFalse(mod.live_hold(self.p, d))
        self.assertFalse(hold.exists(), "a stale hold is still removed")

    def test_card_swapped_for_one_without_a_filesystem_is_unclean(self):
        # A whole-disk card in a reader: the node and a size stay, udev's
        # record has no file system any more.
        self.stick(kind_card=True)
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        os.environ["TF_UDEV_PROPS_sdb1"] = "ID_BUS=usb\nID_DRIVE_FLASH_SD=1\n"
        self.assertEqual(self.cmd("remove", "sdb1"), 0)
        self.assertEqual(self.event(), ["1", "MYSTICK", "unclean"])
        self.assertIn(["umount", "-l", str(self.tmp / "media" / "MYSTICK")], self.calls("umount"))

    def test_unreadable_udev_record_is_no_pull(self):
        self.stick()
        self.assertEqual(self.cmd("add", "sdb1"), 0)
        del os.environ["TF_UDEV_PROPS_sdb1"]   # the fake udevadm prints nothing
        self.assertEqual(self.cmd("remove", "sdb1"), 0)
        self.assertIsNone(self.event())
        self.assertEqual(self.calls("umount")[0], ["umount", str(self.tmp / "media" / "MYSTICK")])


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
        # As if a release had taken sdc1 off: the skip rules still hold.
        self.write("run/automount/released/sdc", "sdc1\n")
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
