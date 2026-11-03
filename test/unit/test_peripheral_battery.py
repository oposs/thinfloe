#!/usr/bin/env python3
"""Offline tests for /usr/libexec/thinfloe/peripheral-battery.

The helper has no .py extension, so it is loaded through importlib like
thinfloe-addon in test_addon.py. Its entry point is guarded, so importing
it does nothing.

FakeReceiver answers the way the Logi Bolt receiver (046d:c548) answered on
10.46.102.86 on 2026-11-03: see the reply table in
docs/superpowers/plans/2026-11-03-peripheral-battery.md. Time is a fake
clock that the receiver advances, so timeouts cost no real time.
"""

import importlib.machinery
import importlib.util
import io
import os
import stat
import tempfile
import unittest
from pathlib import Path
from unittest import mock

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "overlay" / "usr" / "libexec" / "thinfloe" / "peripheral-battery"


def load():
    loader = importlib.machinery.SourceFileLoader("peripheral_battery", str(SCRIPT))
    spec = importlib.util.spec_from_loader("peripheral_battery", loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


mod = load()


class Clock:
    def __init__(self):
        self.t = 100.0

    def __call__(self):
        return self.t


def dev(name, rname=None, ub=None, bs=None, mv=None, state="on", delay=0.05):
    """A paired device. ub: (soc, level, charging) for 0x1004, bs: (level,
    next, status) for 0x1000, mv: millivolts for 0x1001. state is "on",
    "off" (receiver answers 0x8f code 0x04) or "silent" (no answer)."""
    features = {0x0005: 3}
    if ub:
        features[0x1004] = 9
    if bs:
        features[0x1000] = 10
    if mv:
        features[0x1001] = 11
    return {"name": name, "rname": rname or name[:13], "features": features,
            "ub": ub, "bs": bs, "mv": mv, "state": state, "delay": delay}


class FakeReceiver:
    def __init__(self, devices, clock=None, vanish_after=None):
        self.devices = devices
        self.clock = clock or Clock()
        self.pending = None
        self.reads = 0
        self.vanish_after = vanish_after

    def write(self, data):
        self.pending = self._answer(bytes(data))

    def read(self, timeout):
        if getattr(self, "queue", None):
            return self.queue.pop(0)
        self.reads += 1
        if self.vanish_after is not None and self.reads > self.vanish_after:
            raise OSError(19, "No such device")
        if self.pending is None:
            self.clock.t += timeout
            return None
        delay, rep = self.pending
        if delay > timeout:
            self.clock.t += timeout
            return None
        self.clock.t += delay
        self.pending = None
        return rep

    def _answer(self, msg):
        if msg[0] == 0x10 and msg[1] == 0xFF and msg[2] == 0x83 and msg[3] == 0xB5:
            return self._register(msg[4], msg[5])
        slot, feat, fsw = msg[1], msg[2], msg[3]
        d = self.devices.get(slot)
        if d is None:
            return 0.05, bytes([0x10, slot, 0x8F, feat, fsw, 0x09, 0x00])
        if d["state"] == "off":
            return 0.05, bytes([0x10, slot, 0x8F, feat, fsw, 0x04, 0x00])
        if d["state"] == "silent":
            return None
        func = fsw >> 4
        data = self._feature(d, feat, func, msg[4:])
        if data is None:
            return d["delay"], bytes([0x11, slot, 0xFF, feat, fsw, 0x02]).ljust(20, b"\0")
        return d["delay"], (bytes([0x11, slot, feat, fsw]) + bytes(data)).ljust(20, b"\0")

    def _register(self, sub, chunk):
        slot = sub & 0x0F
        d = self.devices.get(slot)
        if d is None or slot < 1 or slot > 6:
            return 0.05, bytes([0x10, 0xFF, 0x8F, 0x83, 0xB5, 0x08, 0x00])
        head = bytes([0x11, 0xFF, 0x83, 0xB5, sub])
        if sub & 0xF0 == 0x50:
            return 0.05, (head + bytes([0x42, 0x38, 0xB0, 0xC5])).ljust(20, b"\0")
        name = d["rname"].encode()
        if chunk == 1:
            body = bytes([1, len(name)]) + name[:13]
        else:
            start = 13 + (chunk - 2) * 14
            body = bytes([chunk]) + name[start:start + 14]
        return 0.05, (head + body).ljust(20, b"\0")

    def _feature(self, d, feat, func, params):
        index = {v: k for k, v in d["features"].items()}
        if feat == 0 and func == 0:
            fid = (params[0] << 8) | params[1]
            return [d["features"].get(fid, 0), 0, 0]
        if feat == 0 and func == 1:
            return [4, 5, params[2]]
        fid = index.get(feat)
        name = d["name"].encode()
        if fid == 0x0005 and func == 0:
            return [len(name)]
        if fid == 0x0005 and func == 1:
            return list(name[params[0]:params[0] + 16])
        if fid == 0x1004 and func == 1:
            soc, level, charging = d["ub"]
            return [soc, level, charging, 0]
        if fid == 0x1000 and func == 0:
            return list(d["bs"])
        if fid == 0x1001 and func == 0:
            return [d["mv"] >> 8, d["mv"] & 0xFF, 0]
        return None


R = mod.Reading


class ReceiverTests(unittest.TestCase):
    def setUp(self):
        self.clock = Clock()
        patcher = mock.patch.object(mod, "monotonic", self.clock)
        patcher.start()
        self.addCleanup(patcher.stop)

    def readings(self, t):
        got, unreachable = mod.read_receiver(t, now=1000)
        return sorted(r for _, _, r in got), unreachable

    def test_reads_unified_battery(self):
        t = FakeReceiver({1: dev("MX Anywhere 3S for Business", "MX Anywhere3SB", ub=(90, 8, 0)),
                          2: dev("MX Mechanical", "MX MCHNCL", ub=(100, 8, 3))}, self.clock)
        got, unreachable = self.readings(t)
        self.assertEqual(got, [
            R("MX Anywhere 3S for Business", 90, "discharging", 1000),
            R("MX Mechanical", 100, "full", 1000)])
        self.assertEqual(unreachable, [])

    def test_receiver_name_spans_two_chunks(self):
        t = FakeReceiver({1: dev("MX Anywhere 3S for Business", "MX Anywhere3SB", state="off")}, self.clock)
        _, unreachable = mod.read_receiver(t, now=1000)
        self.assertEqual(unreachable, [(1, "MX Anywhere3SB")])

    def test_low_and_critical_levels(self):
        t = FakeReceiver({1: dev("A", ub=(12, 2, 0)), 2: dev("B", ub=(3, 1, 0))}, self.clock)
        got, _ = self.readings(t)
        self.assertEqual(got, [R("A", 12, "low", 1000), R("B", 3, "critical", 1000)])

    def test_charging(self):
        t = FakeReceiver({1: dev("A", ub=(50, 4, 1))}, self.clock)
        self.assertEqual(self.readings(t)[0], [R("A", 50, "charging", 1000)])

    def test_falls_back_to_battery_status_1000(self):
        t = FakeReceiver({1: dev("Old Mouse", bs=(70, 50, 0))}, self.clock)
        self.assertEqual(self.readings(t)[0], [R("Old Mouse", 70, "discharging", 1000)])

    def test_voltage_only_device_has_no_percent(self):
        t = FakeReceiver({1: dev("Volt Mouse", mv=3900)}, self.clock)
        self.assertEqual(self.readings(t)[0], [R("Volt Mouse", None, "unknown", 1000)])

    def test_device_off_since_boot_shows_receiver_name(self):
        t = FakeReceiver({1: dev("MX Anywhere 3S for Business", "MX Anywhere3SB", state="off")}, self.clock)
        got, unreachable = mod.read_receiver(t, now=1000)
        self.assertEqual(got, [])
        self.assertEqual(unreachable, [(1, "MX Anywhere3SB")])
        merged = mod.merge([], [], ["MX Anywhere3SB"], now=1000)
        self.assertEqual(merged, [R("MX Anywhere3SB", None, "not reachable", 1000)])

    def test_empty_slot_is_not_a_device(self):
        t = FakeReceiver({2: dev("MX Mechanical", "MX MCHNCL", ub=(100, 8, 3))}, self.clock)
        got, unreachable = mod.read_receiver(t, now=1000)
        self.assertEqual([s for s, _, _ in got], [2])
        self.assertEqual(unreachable, [])

    def test_slow_wake_is_reachable(self):
        t = FakeReceiver({2: dev("MX Mechanical", "MX MCHNCL", ub=(100, 8, 3), delay=0.85)}, self.clock)
        got, unreachable = self.readings(t)
        self.assertEqual(got, [R("MX Mechanical", 100, "full", 1000)])
        self.assertEqual(unreachable, [])

    def test_run_respects_budget(self):
        t = FakeReceiver({n: dev(f"Dev {n}", state="silent") for n in range(1, 7)}, self.clock)
        start = self.clock.t
        got, unreachable = mod.read_receiver(t, now=1000)
        self.assertLessEqual(self.clock.t - start, mod.RUN_BUDGET + mod.REQUEST_TIMEOUT)
        self.assertEqual(got, [])
        self.assertEqual(sorted(s for s, _ in unreachable), [1, 2, 3, 4, 5, 6])

    def test_ignores_unrelated_reports(self):
        t = FakeReceiver({1: dev("A", ub=(40, 4, 0))}, self.clock)
        real_read = t.read
        sent = []

        def noisy_read(timeout):
            if not sent:
                sent.append(1)
                return bytes([0x10, 0x01, 0x41, 0x04, 0x01, 0x00, 0x00])
            return real_read(timeout)
        t.read = noisy_read
        self.assertEqual(self.readings(t)[0], [R("A", 40, "discharging", 1000)])

    def test_stale_reply_is_not_taken_for_the_next_request(self):
        # A getFeature reply that arrives after its request timed out sits in
        # the buffer; the next getFeature on the slot must not take it.
        class Late(FakeReceiver):
            armed = False

            def _feature(self, d, feat, func, params):
                if feat == 3 and func == 1:   # the DeviceName chunk request
                    self.armed = True
                return super()._feature(d, feat, func, params)

            def read(self, timeout):
                rep = super().read(timeout)
                if rep is not None and self.armed:   # its reply went out; a late one follows
                    self.armed = False
                    self.queue = [bytes([0x11, 1, 0x00, 0x0A, 3, 0, 0]).ljust(20, b"\0")]
                return rep
        t = Late({1: dev("A", ub=(40, 4, 0))}, self.clock)
        self.assertEqual(self.readings(t)[0], [R("A", 40, "discharging", 1000)])

    def test_short_reply_makes_the_device_unreachable(self):
        class Short(FakeReceiver):
            def _answer(self, msg):
                ans = super()._answer(msg)
                if msg[0] == 0x11 and msg[2] == 9 and msg[3] >> 4 == 1:   # 0x1004 get_status
                    return ans[0], ans[1][:5]
                return ans
        t = Short({1: dev("A", ub=(40, 4, 0))}, self.clock)
        got, unreachable = mod.read_receiver(t, now=1000)
        self.assertEqual(got, [])
        self.assertEqual(unreachable, [(1, "A")])


class MergeAndNameTests(unittest.TestCase):
    def test_unreachable_device_keeps_last_reading(self):
        prev = [R("MX Mechanical", 80, "discharging", 500)]
        out = mod.merge([], prev, ["MX Mechanical"], now=1000)
        self.assertEqual(out, [R("MX Mechanical", 80, "not reachable", 500)])

    def test_reachable_reading_replaces_previous(self):
        prev = [R("A", 80, "discharging", 500)]
        out = mod.merge([R("A", 79, "discharging", 1000)], prev, [], now=1000)
        self.assertEqual(out, [R("A", 79, "discharging", 1000)])

    def test_device_gone_from_receiver_is_dropped(self):
        prev = [R("Unpaired", 80, "discharging", 500)]
        self.assertEqual(mod.merge([], prev, [], now=1000), [])

    def test_cached_full_name_used_while_slot_unchanged(self):
        cache = {("usb-1/input2", 1): ("MX Anywhere3SB", "MX Anywhere 3S for Business")}
        self.assertEqual(mod.resolve_name(cache, "usb-1/input2", 1, "MX Anywhere3SB"),
                         "MX Anywhere 3S for Business")

    def test_repaired_slot_drops_cached_name(self):
        cache = {("usb-1/input2", 1): ("MX Anywhere3SB", "MX Anywhere 3S for Business")}
        self.assertEqual(mod.resolve_name(cache, "usb-1/input2", 1, "MX Keys"), "MX Keys")

    def test_device_name_is_sanitised(self):
        self.assertEqual(mod.sanitise("a\tb\nc\x1b" + "x" * 300), ("a?b?c?" + "x" * 300)[:64])
        self.assertEqual(mod.sanitise(""), "unknown device")
        self.assertEqual(mod.sanitise("MX MCHNCL\0\0"), "MX MCHNCL")

    def test_unicode_line_breaks_and_bidi_are_sanitised(self):
        self.assertEqual(mod.sanitise("a\x85b c d‮e"), "a?b?c?d?e")

    def test_parse_splits_on_newline_only(self):
        self.assertEqual(mod.parse_lines("a b\t90\tfull\t7\n"), [R("a b", 90, "full", 7)])

    def test_format_and_parse_round_trip(self):
        rs = [R("b", None, "not reachable", 5), R("a", 90, "full", 7)]
        text = mod.format_lines(rs)
        self.assertEqual(text, "a\t90\tfull\t7\nb\t-\tnot reachable\t5\n")
        self.assertEqual(mod.parse_lines(text), sorted(rs))

    def test_parse_skips_broken_lines(self):
        self.assertEqual(mod.parse_lines("a\t90\tfull\t7\ngarbage\nb\tx\tfull\t1\n"),
                         [R("a", 90, "full", 7)])


def write(path, text, mode="w"):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, mode) as f:
        f.write(text)


HIDPP_DESC = bytes.fromhex("06 00 ff 09 01 a1 01 85 10 75 08 95 06 81 00 85 11 95 13 81 00 c0")


def fake_hidraw(sysfs, name, hid_id, driver, desc, phys):
    d = sysfs / "class" / "hidraw" / name / "device"
    write(d / "uevent", f"DRIVER={driver}\nHID_ID={hid_id}\nHID_PHYS={phys}\n")
    write(d / "report_descriptor", desc, "wb")
    drv = sysfs / "bus" / "hid" / "drivers" / driver
    drv.mkdir(parents=True, exist_ok=True)
    (d / "driver").symlink_to(drv)


class SysfsTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(dir=os.environ.get("TMPDIR")))
        self.sysfs = self.tmp / "sys"

    def test_find_nodes_skips_kernel_driven_logitech(self):
        fake_hidraw(self.sysfs, "hidraw0", "0003:0000046D:0000C548", "hid-generic", HIDPP_DESC, "usb-1/input2")
        fake_hidraw(self.sysfs, "hidraw1", "0003:0000046D:0000C52B", "logitech-djreceiver", HIDPP_DESC, "usb-2/input2")
        fake_hidraw(self.sysfs, "hidraw2", "0003:00000451:000082FF", "hid-generic", HIDPP_DESC, "usb-3/input0")
        fake_hidraw(self.sysfs, "hidraw3", "0003:0000046D:0000C548", "hid-generic", bytes.fromhex("05 01 09 06"), "usb-1/input0")
        nodes = mod.find_hidpp_nodes(self.sysfs, Path("/dev"))
        self.assertEqual(nodes, [(Path("/dev/hidraw0"), "usb-1/input2")])

    def test_power_supply_device_scope_only(self):
        ps = self.sysfs / "class" / "power_supply"
        write(ps / "BAT0" / "uevent", "POWER_SUPPLY_NAME=BAT0\nPOWER_SUPPLY_SCOPE=System\nPOWER_SUPPLY_CAPACITY=40\n")
        write(ps / "hidpp_battery_0" / "uevent",
              "POWER_SUPPLY_NAME=hidpp_battery_0\nPOWER_SUPPLY_SCOPE=Device\nPOWER_SUPPLY_MODEL_NAME=MX Keys\n"
              "POWER_SUPPLY_STATUS=Discharging\nPOWER_SUPPLY_CAPACITY=55\n")
        write(ps / "hid-xx-battery" / "uevent",
              "POWER_SUPPLY_NAME=hid-xx-battery\nPOWER_SUPPLY_SCOPE=Device\nPOWER_SUPPLY_MODEL_NAME=Pen\n"
              "POWER_SUPPLY_STATUS=Charging\nPOWER_SUPPLY_CAPACITY_LEVEL=Normal\n")
        got = sorted(mod.read_power_supply(self.sysfs, now=1000))
        self.assertEqual(got, [R("MX Keys", 55, "discharging", 1000), R("Pen", None, "charging", 1000)])


class MainTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(dir=os.environ.get("TMPDIR")))
        self.out = self.tmp / "peripherals"
        self.state = self.tmp / "state"
        self.out.mkdir()
        self.state.mkdir(mode=0o700)
        self.sysfs = self.tmp / "sys"
        fake_hidraw(self.sysfs, "hidraw4", "0003:0000046D:0000C548", "hid-generic", HIDPP_DESC, "usb-1/input2")
        ps = self.sysfs / "class" / "power_supply"
        write(ps / "hidpp_battery_0" / "uevent",
              "POWER_SUPPLY_SCOPE=Device\nPOWER_SUPPLY_MODEL_NAME=BT Mouse\nPOWER_SUPPLY_STATUS=Full\nPOWER_SUPPLY_CAPACITY=100\n")
        env = {"TF_SYSFS": str(self.sysfs), "TF_DEV": str(self.tmp / "dev"),
               "TF_BATTERY_DIR": str(self.out), "TF_BATTERY_STATE": str(self.state)}
        p = mock.patch.dict(os.environ, env)
        p.start()
        self.addCleanup(p.stop)
        self.clock = Clock()
        p2 = mock.patch.object(mod, "monotonic", self.clock)
        p2.start()
        self.addCleanup(p2.stop)
        p3 = mock.patch.object(mod.time, "time", lambda: 1000)
        p3.start()
        self.addCleanup(p3.stop)
        p4 = mock.patch.object(mod, "switch_on", lambda: True)
        p4.start()
        self.addCleanup(p4.stop)

    def run_main(self, receiver):
        with mock.patch.object(mod, "open_transport", lambda node: receiver):
            return mod.main()

    def battery(self):
        return (self.out / "battery").read_text()

    def test_main_writes_file_and_name_cache(self):
        r = FakeReceiver({1: dev("MX Anywhere 3S for Business", "MX Anywhere3SB", ub=(90, 8, 0))}, self.clock)
        self.assertEqual(self.run_main(r), 0)
        self.assertEqual(self.battery(),
                         "BT Mouse\t100\tfull\t1000\nMX Anywhere 3S for Business\t90\tdischarging\t1000\n")
        self.assertEqual(sorted(os.listdir(self.out)), ["battery"])
        self.assertEqual(stat.S_IMODE(os.stat(self.out / "battery").st_mode), 0o644)
        names = self.state / "names"
        self.assertEqual(names.read_text(), "usb-1/input2\t1\tMX Anywhere3SB\tMX Anywhere 3S for Business\n")
        self.assertEqual(stat.S_IMODE(os.stat(names).st_mode), 0o600)

    def test_switched_off_device_keeps_full_name_and_last_reading(self):
        r = FakeReceiver({1: dev("MX Anywhere 3S for Business", "MX Anywhere3SB", ub=(90, 8, 0))}, self.clock)
        self.run_main(r)
        r.devices[1]["state"] = "off"
        with mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(r), 0)
        self.assertIn("MX Anywhere 3S for Business\t90\tnot reachable\t1000\n", self.battery())

    def test_power_supply_device_gone_is_not_reachable(self):
        r = FakeReceiver({}, self.clock)
        self.run_main(r)
        (self.sysfs / "class" / "power_supply" / "hidpp_battery_0" / "uevent").unlink()
        with mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(r), 0)
        self.assertEqual(self.battery(), "BT Mouse\t100\tnot reachable\t1000\n")

    def test_unexpected_error_on_a_node_still_writes_the_file(self):
        class Broken(FakeReceiver):
            def write(self, data):
                raise ValueError("garbage")
        with mock.patch("sys.stderr"):
            self.assertEqual(self.run_main(Broken({}, self.clock)), 0)
        self.assertEqual(self.battery(), "BT Mouse\t100\tfull\t1000\n")

    def test_node_vanishing_mid_run_is_skipped(self):
        r = FakeReceiver({1: dev("A", ub=(40, 4, 0))}, self.clock, vanish_after=1)
        with mock.patch("sys.stderr"):
            self.assertEqual(self.run_main(r), 0)
        self.assertEqual(self.battery(), "BT Mouse\t100\tfull\t1000\n")

    def test_write_is_atomic(self):
        r = FakeReceiver({1: dev("A", ub=(40, 4, 0))}, self.clock)
        self.run_main(r)
        before = self.battery()
        with mock.patch.object(mod, "format_lines", side_effect=RuntimeError("boom")), \
                mock.patch("sys.stderr"):
            self.assertEqual(self.run_main(r), 1)
        self.assertEqual(self.battery(), before)
        self.assertEqual(sorted(os.listdir(self.out)), ["battery"])
        self.assertEqual(sorted(os.listdir(self.state)), ["names", "power-supply"])


class SwitchTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR"))
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        r = self.root
        (r / "home").mkdir()
        (r / "tl" / "etc").mkdir(parents=True)
        self.conf = r / "tl" / "etc" / "tlclient_mandatory.conf"
        self.conf.write_text("PRINTER_ENABLED=0\n")
        (r / "peripherals").mkdir()
        (r / "peripherals" / "battery").write_text("x\t1\tfull\t1\n")
        (r / "state").mkdir()
        (r / "state" / "power-supply").write_text("x\n")
        self.calls = r / "calls"
        self.stop_saw_file = r / "stop_saw_file"
        stub = r / "systemctl"
        stub.write_text(
            '#!/bin/sh\n'
            f'echo "$*" >> {self.calls}\n'
            f'if [ "$1" = stop ]; then [ -e {r}/peripherals/battery ] && echo yes > {self.stop_saw_file}; fi\n'
            'exit "${STUB_RC:-0}"\n')
        stub.chmod(0o755)
        self.sw = r / "home" / ".config" / "thinfloe" / "peripheral-battery"
        env = {"TF_BATTERY_HOME": str(r / "home"), "TF_THINLINC_DIR": str(r / "tl"),
               "TF_SYSTEMCTL": str(stub), "TF_BATTERY_DIR": str(r / "peripherals"),
               "TF_BATTERY_STATE": str(r / "state")}
        p = mock.patch.dict(os.environ, env)
        p.start()
        self.addCleanup(p.stop)

    def run_main(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
            rc = mod.main(list(argv))
        return rc, out.getvalue(), err.getvalue()

    def test_default_is_off(self):
        rc, out, _ = self.run_main("status")
        self.assertEqual((rc, out), (0, "off\n"))
        self.assertFalse(self.sw.exists())

    def test_enable_writes_switch_config_and_starts_service(self):
        self.assertEqual(self.run_main("enable")[0], 0)
        self.assertEqual(self.sw.read_text(), "on\n")
        self.assertEqual(stat.S_IMODE(self.sw.stat().st_mode), 0o644)
        self.assertEqual(self.conf.read_text(),
                         "PRINTER_ENABLED=0\nNFS_SERVER_ENABLED=1\n"
                         "NFS_EXPORTS=/run/thinfloe/peripherals,ro\n")
        self.assertEqual(stat.S_IMODE(self.conf.stat().st_mode), 0o644)
        self.assertEqual(self.calls.read_text(),
                         "start --no-block thinfloe-peripheral-battery.service\n")
        self.assertEqual(self.run_main("status")[1], "on\n")

    def test_enable_without_thinlinc_etc_still_switches_on(self):
        self.conf.unlink()
        (self.root / "tl" / "etc").rmdir()
        self.assertEqual(self.run_main("enable")[0], 0)
        self.assertTrue(self.sw.exists())
        self.assertFalse((self.root / "tl" / "etc").exists())

    def test_disable_removes_switch_keys_and_battery_file(self):
        self.run_main("enable")
        self.assertEqual(self.run_main("disable")[0], 0)
        self.assertFalse(self.sw.exists())
        self.assertFalse((self.root / "peripherals" / "battery").exists())
        self.assertFalse((self.root / "state" / "power-supply").exists())
        self.assertEqual(self.conf.read_text(), "PRINTER_ENABLED=0\n")
        self.assertEqual(self.calls.read_text().splitlines()[-1],
                         "stop thinfloe-peripheral-battery.service")

    def test_disable_stops_the_service_before_removing_the_file(self):
        self.run_main("enable")
        self.run_main("disable")
        self.assertTrue(self.stop_saw_file.exists())

    def test_enable_and_disable_twice_change_nothing(self):
        def leftovers():
            return [p for d in (self.root / "tl" / "etc", self.sw.parent) if d.exists()
                    for p in d.iterdir() if p.name.startswith(".")]
        self.assertEqual(self.run_main("enable")[0], 0)
        a, b = self.sw.read_bytes(), self.conf.read_bytes()
        self.assertEqual(self.run_main("enable")[0], 0)
        self.assertEqual((self.sw.read_bytes(), self.conf.read_bytes()), (a, b))
        self.assertEqual(self.run_main("disable")[0], 0)
        c = self.conf.read_bytes()
        self.assertEqual(self.run_main("disable")[0], 0)
        self.assertEqual(self.conf.read_bytes(), c)
        self.assertFalse(self.sw.exists())
        self.assertEqual(leftovers(), [])

    def test_thinlinc_config_follows_the_switch(self):
        self.conf.write_text("NFS_SERVER_ENABLED=0\nNFS_EXPORTS=/media,rw\nX=1\n")
        self.assertEqual(self.run_main("thinlinc-config")[0], 0)
        self.assertEqual(self.conf.read_text(), "X=1\n")
        mod.set_switch(True)
        self.assertEqual(self.run_main("thinlinc-config")[0], 0)
        self.assertEqual(self.conf.read_text(),
                         "X=1\nNFS_SERVER_ENABLED=1\nNFS_EXPORTS=/run/thinfloe/peripherals,ro\n")
        mod.set_switch(False)
        self.conf.unlink()
        self.assertEqual(self.run_main("thinlinc-config")[0], 0)
        self.assertFalse(self.conf.exists())

    def test_unusable_thinlinc_etc_fails_with_a_reason(self):
        etc = self.root / "tl" / "etc"
        self.conf.unlink()
        etc.rmdir()
        etc.write_text("not a directory")
        mod.set_switch(True)
        rc, _, err = self.run_main("thinlinc-config")
        self.assertEqual(rc, 1)
        self.assertTrue(err.startswith("thinfloe: battery: "), err)

    def test_systemctl_failure_keeps_the_switch_and_exits_1(self):
        with mock.patch.dict(os.environ, {"STUB_RC": "3"}):
            rc, _, err = self.run_main("enable")
        self.assertEqual(rc, 1)
        self.assertTrue(self.sw.exists())
        self.assertIn("thinfloe: battery: ", err)

    def test_symlinked_config_dir_is_not_followed(self):
        outside = self.root / "outside"
        outside.mkdir()
        (self.root / "home" / ".config").symlink_to(outside)
        rc, _, err = self.run_main("enable")
        self.assertEqual(rc, 1)
        self.assertEqual(list(outside.iterdir()), [])
        self.assertIn("thinfloe: battery: ", err)

    def reading_run(self):
        with mock.patch.object(mod, "open_transport", lambda node: None):
            return mod.main([])

    def test_reading_run_with_the_switch_off_writes_nothing(self):
        (self.root / "peripherals" / "battery").unlink()
        (self.root / "state" / "power-supply").unlink()
        self.assertEqual(self.reading_run(), 0)
        self.assertEqual(list((self.root / "peripherals").iterdir()), [])
        self.assertEqual(list((self.root / "state").iterdir()), [])

    def test_reading_run_through_a_symlinked_config_dir_counts_as_off(self):
        real = self.root / "real"
        (real / "thinfloe").mkdir(parents=True)
        (real / "thinfloe" / "peripheral-battery").write_text("on\n")
        (self.root / "home" / ".config").symlink_to(real)
        self.assertEqual(self.run_main("status")[1], "off\n")
        (self.root / "peripherals" / "battery").unlink()
        self.assertEqual(self.reading_run(), 0)
        self.assertFalse((self.root / "peripherals" / "battery").exists())

    def test_non_utf8_mandatory_config_does_not_stop_enable(self):
        self.conf.write_bytes(b"A=\xff\xfe\nPRINTER_ENABLED=0\n")
        rc, _, err = self.run_main("enable")
        self.assertEqual(rc, 0, err)
        self.assertTrue(self.sw.exists())
        self.assertEqual(self.calls.read_text(),
                         "start --no-block thinfloe-peripheral-battery.service\n")

    def test_an_unexpected_error_in_a_step_is_logged_and_the_rest_runs(self):
        with mock.patch.object(mod, "thinlinc_config", side_effect=ValueError("odd")):
            rc, _, err = self.run_main("enable")
        self.assertEqual(rc, 1)
        self.assertTrue(self.sw.exists())
        self.assertEqual(self.calls.read_text(),
                         "start --no-block thinfloe-peripheral-battery.service\n")
        self.assertIn("thinfloe: battery: ThinLinc config: ValueError: odd", err)
        self.assertNotIn("Traceback", err)

    def test_non_utf8_mandatory_config_is_kept_byte_for_byte(self):
        self.conf.write_bytes(b"A=\xff\xfe\nPRINTER_ENABLED=0\nNFS_EXPORTS=/x\n")
        self.run_main("enable")
        self.assertEqual(self.conf.read_bytes(),
                         b"A=\xff\xfe\nPRINTER_ENABLED=0\nNFS_SERVER_ENABLED=1\n"
                         b"NFS_EXPORTS=/run/thinfloe/peripherals,ro\n")

    def test_a_missing_tfuser_is_logged_not_a_traceback(self):
        with mock.patch.object(mod, "_tfuser", side_effect=KeyError("tfuser")), \
                mock.patch.object(mod.os, "geteuid", lambda: 0), \
                mock.patch.dict(os.environ, {"TF_BATTERY_HOME": str(self.root / "home")}):
            rc, _, err = self.run_main("enable")
        self.assertEqual(rc, 1)
        self.assertNotIn("Traceback", err)

    def test_unknown_subcommand_exits_2(self):
        self.assertEqual(self.run_main("bogus")[0], 2)



if __name__ == "__main__":
    unittest.main()
