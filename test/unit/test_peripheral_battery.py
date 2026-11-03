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


def dev(name, rname=None, ub=None, bs=None, mv=None, state="on", delay=0.05, kind=0x40,
        ws_index=4, ub_index=9):
    """A paired device. ub: (soc, level, charging) for 0x1004, bs: (level,
    next, status) for 0x1000, mv: millivolts for 0x1001. state is "on",
    "off" (receiver answers 0x8f code 0x04) or "silent" (no answer). kind
    is the first byte of the pairing info: the Bolt receiver answered 0x42
    for the MX Anywhere 3S (a mouse) and 0x01 for the MX Mechanical (a
    keyboard); the default 0x40 has kind nibble 0, unknown. ws_index and
    ub_index are the feature indices of WIRELESS DEVICE STATUS (0x1D4B) and
    UNIFIED BATTERY: 4 and 9 on the MX Anywhere 3S (measured 2026-11-03),
    4 and 7 on the MX Mechanical (Solaar's listing). ws_index None leaves
    0x1D4B out."""
    features = {0x0005: 3}
    if ws_index:
        features[0x1D4B] = ws_index
    if ub:
        features[0x1004] = ub_index
    if bs:
        features[0x1000] = 10
    if mv:
        features[0x1001] = 11
    return {"name": name, "rname": rname or name[:13], "features": features,
            "ub": ub, "bs": bs, "mv": mv, "state": state, "delay": delay, "kind": kind}


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
            return 0.05, (head + bytes([d["kind"], 0x38, 0xB0, 0xC5])).ljust(20, b"\0")
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

    def test_kind_comes_from_the_pairing_info(self):
        # The bytes the Bolt receiver gave on 2026-11-03: 0x42 mouse, 0x01 keyboard.
        t = FakeReceiver({1: dev("MX Anywhere 3S for Business", "MX Anywhere3SB", ub=(90, 8, 0), kind=0x42),
                          2: dev("MX Mechanical", "MX MCHNCL", ub=(100, 8, 3), kind=0x01),
                          3: dev("Pad", ub=(50, 8, 0), kind=0x23),
                          4: dev("Thing", ub=(50, 8, 0), kind=0x0F)}, self.clock)
        got, _ = self.readings(t)
        self.assertEqual([(r.name, r.kind) for r in got], [
            ("MX Anywhere 3S for Business", "mouse"), ("MX Mechanical", "keyboard"),
            ("Pad", "numpad"), ("Thing", "unknown")])

    def test_kind_of_a_switched_off_device_comes_from_the_pairing_info(self):
        t = FakeReceiver({2: dev("MX Mechanical", "MX MCHNCL", state="off", kind=0x01)}, self.clock)
        _, unreachable = mod.read_receiver(t, now=1000)
        self.assertEqual(unreachable, [(2, "MX MCHNCL", "keyboard")])

    def test_kind_numbers(self):
        self.assertEqual({n: mod.kind_of(n) for n in range(16)},
                         {**{n: "unknown" for n in range(16)},
                          1: "keyboard", 2: "mouse", 3: "numpad", 4: "presenter",
                          8: "trackball", 9: "touchpad"})

    def test_receiver_name_spans_two_chunks(self):
        t = FakeReceiver({1: dev("MX Anywhere 3S for Business", "MX Anywhere3SB", state="off")}, self.clock)
        _, unreachable = mod.read_receiver(t, now=1000)
        self.assertEqual(unreachable, [(1, "MX Anywhere3SB", "unknown")])

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
        self.assertEqual(unreachable, [(1, "MX Anywhere3SB", "unknown")])
        merged = mod.merge([], [], [("MX Anywhere3SB", "unknown")], now=1000)
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
        self.assertEqual(sorted(s for s, _, _ in unreachable), [1, 2, 3, 4, 5, 6])

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
        self.assertEqual(unreachable, [(1, "A", "unknown")])


class MergeAndNameTests(unittest.TestCase):
    def test_unreachable_device_keeps_last_reading(self):
        prev = [R("MX Mechanical", 80, "discharging", 500)]
        out = mod.merge([], prev, [("MX Mechanical", "unknown")], now=1000)
        self.assertEqual(out, [R("MX Mechanical", 80, "not reachable", 500)])

    def test_unreachable_device_keeps_the_previous_kind(self):
        prev = [R("BT Mouse", 80, "discharging", 500, "mouse")]
        out = mod.merge([], prev, [("BT Mouse", "unknown")], now=1000)
        self.assertEqual(out, [R("BT Mouse", 80, "not reachable", 500, "mouse")])

    def test_a_known_kind_of_an_unreachable_device_wins(self):
        prev = [R("K", 80, "discharging", 500, "unknown")]
        out = mod.merge([], prev, [("K", "keyboard")], now=1000)
        self.assertEqual(out, [R("K", 80, "not reachable", 500, "keyboard")])
        self.assertEqual(mod.merge([], [], [("K", "keyboard")], now=1000),
                         [R("K", None, "not reachable", 1000, "keyboard")])

    def test_cached_kind_used_when_the_pairing_info_has_none(self):
        cache = {("usb-1/input2", 1): ("MX MCHNCL", "MX Mechanical", "keyboard")}
        self.assertEqual(mod.resolve_name(cache, "usb-1/input2", 1, "MX MCHNCL", "unknown"),
                         ("MX Mechanical", "keyboard"))
        self.assertEqual(mod.resolve_name(cache, "usb-1/input2", 1, "MX MCHNCL", "mouse"),
                         ("MX Mechanical", "mouse"))

    def test_reachable_reading_replaces_previous(self):
        prev = [R("A", 80, "discharging", 500)]
        out = mod.merge([R("A", 79, "discharging", 1000)], prev, [], now=1000)
        self.assertEqual(out, [R("A", 79, "discharging", 1000)])

    def test_device_gone_from_receiver_is_dropped(self):
        prev = [R("Unpaired", 80, "discharging", 500)]
        self.assertEqual(mod.merge([], prev, [], now=1000), [])

    def test_cached_full_name_used_while_slot_unchanged(self):
        cache = {("usb-1/input2", 1): ("MX Anywhere3SB", "MX Anywhere 3S for Business", "unknown")}
        self.assertEqual(mod.resolve_name(cache, "usb-1/input2", 1, "MX Anywhere3SB"),
                         ("MX Anywhere 3S for Business", "unknown"))

    def test_repaired_slot_drops_cached_name(self):
        cache = {("usb-1/input2", 1): ("MX Anywhere3SB", "MX Anywhere 3S for Business", "unknown")}
        self.assertEqual(mod.resolve_name(cache, "usb-1/input2", 1, "MX Keys"), ("MX Keys", "unknown"))

    def test_device_name_is_sanitised(self):
        self.assertEqual(mod.sanitise("a\tb\nc\x1b" + "x" * 300), ("a?b?c?" + "x" * 300)[:64])
        self.assertEqual(mod.sanitise(""), "unknown device")
        self.assertEqual(mod.sanitise("MX MCHNCL\0\0"), "MX MCHNCL")

    def test_unicode_line_breaks_and_bidi_are_sanitised(self):
        self.assertEqual(mod.sanitise("a\x85b c d‮e"), "a?b?c?d?e")

    def test_parse_splits_on_newline_only(self):
        self.assertEqual(mod.parse_lines("a b\t90\tfull\t7\n"), [R("a b", 90, "full", 7)])

    def test_format_and_parse_round_trip(self):
        rs = [R("b", None, "not reachable", 5, "mouse"), R("a", 90, "full", 7)]
        text = mod.format_lines(rs)
        self.assertEqual(text, "a\t90\tfull\t7\tunknown\nb\t-\tnot reachable\t5\tmouse\n")
        self.assertEqual(mod.parse_lines(text), sorted(rs))

    def test_parse_takes_four_fields_as_unknown_and_odd_kinds_as_unknown(self):
        self.assertEqual(mod.parse_lines("a\t90\tfull\t7\nb\t5\tfull\t7\tlaser\nc\t5\tfull\t7\tkeyboard\n"),
                         [R("a", 90, "full", 7), R("b", 5, "full", 7), R("c", 5, "full", 7, "keyboard")])
        self.assertEqual(mod.parse_lines("a\t90\tfull\t7\tmouse\textra\n"), [])

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

    def test_power_supply_kind_from_the_name(self):
        ps = self.sysfs / "class" / "power_supply"
        for n, model in enumerate(["Apple Magic Keyboard", "BT MOUSE 3", "Keyboard and Mouse", "Pen"]):
            write(ps / f"hid-{n}-battery" / "uevent",
                  f"POWER_SUPPLY_SCOPE=Device\nPOWER_SUPPLY_MODEL_NAME={model}\nPOWER_SUPPLY_CAPACITY=50\n")
        got = {r.name: r.kind for r in mod.read_power_supply(self.sysfs, now=1000)}
        self.assertEqual(got, {"Apple Magic Keyboard": "keyboard", "BT MOUSE 3": "mouse",
                               "Keyboard and Mouse": "keyboard", "Pen": "unknown"})


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

    def run_main(self, receiver):
        with mock.patch.object(mod, "open_transport", lambda node: receiver):
            return mod.main()

    def battery(self):
        return (self.out / "battery").read_text()

    def test_a_held_lock_ends_in_lock_busy_not_in_a_wait_without_end(self):
        import fcntl
        fd = os.open(str(self.state / "lock"), os.O_RDWR | os.O_CREAT, 0o600)
        self.addCleanup(os.close, fd)
        fcntl.flock(fd, fcntl.LOCK_EX)
        with mock.patch.object(mod, "LOCK_WAIT", 0.3), \
                mock.patch("sys.stderr", new_callable=io.StringIO) as err:
            code = self.run_main(FakeReceiver({}, self.clock))
        self.assertEqual(code, 1)
        self.assertIn("thinfloe: battery: lock busy", err.getvalue())
        self.assertFalse((self.out / "battery").exists())

    def test_a_lock_freed_during_the_wait_is_taken(self):
        import fcntl
        fd = os.open(str(self.state / "lock"), os.O_RDWR | os.O_CREAT, 0o600)
        self.addCleanup(os.close, fd)
        fcntl.flock(fd, fcntl.LOCK_EX)
        real_sleep = mod.time.sleep

        def sleep_and_release(t):
            fcntl.flock(fd, fcntl.LOCK_UN)
            real_sleep(t)

        with mock.patch.object(mod, "LOCK_WAIT", 5.0), \
                mock.patch.object(mod.time, "sleep", sleep_and_release):
            code = self.run_main(FakeReceiver({}, self.clock))
        self.assertEqual(code, 0)

    def test_main_writes_file_and_name_cache(self):
        r = FakeReceiver({1: dev("MX Anywhere 3S for Business", "MX Anywhere3SB", ub=(90, 8, 0))}, self.clock)
        self.assertEqual(self.run_main(r), 0)
        self.assertEqual(self.battery(),
                         "BT Mouse\t100\tfull\t1000\tmouse\n"
                         "MX Anywhere 3S for Business\t90\tdischarging\t1000\tunknown\n")
        self.assertEqual(sorted(os.listdir(self.out)), ["battery"])
        self.assertEqual(stat.S_IMODE(os.stat(self.out / "battery").st_mode), 0o644)
        names = self.state / "names"
        self.assertEqual(names.read_text(),
                         "usb-1/input2\t1\tMX Anywhere3SB\tMX Anywhere 3S for Business\tunknown\n")
        self.assertEqual(stat.S_IMODE(os.stat(names).st_mode), 0o600)

    def test_switched_off_device_keeps_full_name_and_last_reading(self):
        r = FakeReceiver({1: dev("MX Anywhere 3S for Business", "MX Anywhere3SB", ub=(90, 8, 0))}, self.clock)
        self.run_main(r)
        r.devices[1]["state"] = "off"
        with mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(r), 0)
        self.assertIn("MX Anywhere 3S for Business\t90\tnot reachable\t1000\tunknown\n", self.battery())

    def test_switched_off_device_keeps_its_kind_from_the_cache(self):
        r = FakeReceiver({1: dev("MX Mechanical", "MX MCHNCL", ub=(90, 8, 0), kind=0x01)}, self.clock)
        self.run_main(r)
        self.assertIn("MX Mechanical\t90\tdischarging\t1000\tkeyboard\n", self.battery())
        r.devices[1]["state"] = "off"
        r.devices[1]["kind"] = 0x00   # the pairing info no longer says
        with mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(r), 0)
        self.assertIn("MX Mechanical\t90\tnot reachable\t1000\tkeyboard\n", self.battery())

    def test_power_supply_device_gone_is_not_reachable(self):
        r = FakeReceiver({}, self.clock)
        self.run_main(r)
        (self.sysfs / "class" / "power_supply" / "hidpp_battery_0" / "uevent").unlink()
        with mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(r), 0)
        self.assertEqual(self.battery(), "BT Mouse\t100\tnot reachable\t1000\tmouse\n")

    def test_unexpected_error_on_a_node_still_writes_the_file(self):
        class Broken(FakeReceiver):
            def write(self, data):
                raise ValueError("garbage")
        with mock.patch("sys.stderr"):
            self.assertEqual(self.run_main(Broken({}, self.clock)), 0)
        self.assertEqual(self.battery(), "BT Mouse\t100\tfull\t1000\tmouse\n")

    def test_node_vanishing_mid_run_is_skipped(self):
        r = FakeReceiver({1: dev("A", ub=(40, 4, 0))}, self.clock, vanish_after=1)
        with mock.patch("sys.stderr"):
            self.assertEqual(self.run_main(r), 0)
        self.assertEqual(self.battery(), "BT Mouse\t100\tfull\t1000\tmouse\n")

    def test_write_is_atomic(self):
        r = FakeReceiver({1: dev("A", ub=(40, 4, 0))}, self.clock)
        self.run_main(r)
        before = self.battery()
        with mock.patch.object(mod, "format_lines", side_effect=RuntimeError("boom")), \
                mock.patch("sys.stderr"):
            self.assertEqual(self.run_main(r), 1)
        self.assertEqual(self.battery(), before)
        self.assertEqual(sorted(os.listdir(self.out)), ["battery"])
        self.assertEqual(sorted(os.listdir(self.state)), ["lock", "names", "power-supply"])

    def test_any_argument_is_a_usage_error(self):
        with mock.patch("sys.stderr", new_callable=io.StringIO) as err:
            self.assertEqual(mod.main(["enable"]), 2)
        self.assertIn("usage: peripheral-battery", err.getvalue())
        self.assertFalse((self.out / "battery").exists())

    def test_run_holds_the_lock_around_the_receivers(self):
        calls = []
        real = mod.fcntl.flock

        def flock(fd, op):
            calls.append((os.readlink(f"/proc/self/fd/{fd}"), op))
            return real(fd, op)
        receiver = FakeReceiver({1: dev("MX Anywhere 3S for Business", "MX Anywhere3SB",
                                        ub=(90, 8, 0))}, self.clock)

        def transport(node):
            calls.append(("open", node))
            return receiver
        with mock.patch.object(mod.fcntl, "flock", flock), \
                mock.patch.object(mod, "open_transport", transport):
            self.assertEqual(mod.main(), 0)
        self.assertEqual(calls[0], (str(self.state / "lock"), mod.fcntl.LOCK_EX | mod.fcntl.LOCK_NB))
        self.assertEqual(calls[1][0], "open")
        self.assertEqual(stat.S_IMODE((self.state / "lock").stat().st_mode), 0o600)

# The reports the MX Anywhere 3S sent on its own when it was switched on
# (10.46.102.86, 2026-11-03, slot 1): WIRELESS DEVICE STATUS at feature
# index 4, event 0, status 01 = reconnection; UNIFIED BATTERY at index 9,
# event 0, 100 %, level full, discharging.
WS_EVENT = bytes.fromhex("11 01 04 00 01 01 01").ljust(20, b"\0")
UB_EVENT = bytes.fromhex("11 01 09 00 64 08 00").ljust(20, b"\0")


class EventTests(unittest.TestCase):
    def test_event_head_takes_long_unrequested_reports_only(self):
        self.assertEqual(mod.event_head(WS_EVENT), (1, 4))
        self.assertEqual(mod.event_head(UB_EVENT), (1, 9))
        self.assertEqual(mod.event_head(bytes.fromhex("11 06 07 00 32")), (6, 7))
        for rep in [bytes.fromhex("11 01 04 00"),            # short
                    bytes.fromhex("10 01 41 04 a1 2b 40"),   # HID++ 1.0 report
                    bytes.fromhex("11 01 04 0a 01 01 01"),   # a reply (SWID 0x0a)
                    bytes.fromhex("11 01 04 10 01 01 01"),   # function 1
                    bytes.fromhex("11 ff 04 00 01 01 01"),   # the receiver
                    bytes.fromhex("11 00 04 00 01 01 01"),
                    bytes.fromhex("11 07 04 00 01 01 01"),   # no such slot
                    bytes.fromhex("11 01 00 00 01 01 01"),   # root feature
                    bytes.fromhex("11 01 ff 04 0a 02 00"),   # error reply
                    b""]:
            self.assertIsNone(mod.event_head(rep), rep.hex())

    def test_connect_events_of_both_features(self):
        self.assertTrue(mod.is_connect_event(WS_EVENT, ws=4, ub=9))
        self.assertTrue(mod.is_connect_event(UB_EVENT, ws=4, ub=9))
        # The keyboard: 0x1004 at index 7.
        kb = bytes.fromhex("11 02 07 00 50 04 00").ljust(20, b"\0")
        self.assertTrue(mod.is_connect_event(kb, ws=4, ub=7))
        self.assertFalse(mod.is_connect_event(kb, ws=4, ub=9))

    def test_other_events_are_not_connects(self):
        # Another feature's event (index 5), and 0x1D4B with a status other than 1.
        self.assertFalse(mod.is_connect_event(bytes.fromhex("11 01 05 00 01 00 00"), ws=4, ub=9))
        self.assertFalse(mod.is_connect_event(bytes.fromhex("11 01 04 00 00 01 01"), ws=4, ub=9))
        self.assertFalse(mod.is_connect_event(WS_EVENT, ws=None, ub=9))
        self.assertFalse(mod.is_connect_event(UB_EVENT, ws=4, ub=None))


class FakeListener:
    """The watcher's read-only view of one hidraw node: reports queued by
    the test, an OSError queued to make the node vanish."""

    def __init__(self, node):
        self.node = node
        self.queue = []
        self.closed = False

    def read(self):
        item = self.queue.pop(0)
        if isinstance(item, Exception):
            raise item
        return item

    def close(self):
        self.closed = True


class WatchTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(dir=os.environ.get("TMPDIR")))
        self.out = self.tmp / "peripherals"
        self.state = self.tmp / "state"
        self.out.mkdir()
        self.state.mkdir(mode=0o700)
        self.sysfs = self.tmp / "sys"
        self.sysfs.mkdir()
        self.dev = self.tmp / "dev"
        env = {"TF_SYSFS": str(self.sysfs), "TF_DEV": str(self.dev),
               "TF_BATTERY_DIR": str(self.out), "TF_BATTERY_STATE": str(self.state)}
        for p in (mock.patch.dict(os.environ, env),):
            p.start()
            self.addCleanup(p.stop)
        self.clock = Clock()
        self.wall = 10000
        for p in (mock.patch.object(mod, "monotonic", self.clock),
                  mock.patch.object(mod.time, "time", lambda: self.wall),
                  mock.patch("sys.stderr", new_callable=io.StringIO)):
            p.start()
            self.addCleanup(p.stop)
        self.listeners = {}
        self.opened = []
        self.reads = []
        self.receiver = FakeReceiver({1: dev("MX Anywhere 3S for Business", "MX Anywhere3SB",
                                             ub=(100, 8, 0), kind=0x42),
                                      2: dev("MX Mechanical", "MX MCHNCL", ub=(80, 4, 0),
                                             kind=0x01, ub_index=7)}, self.clock)
        self.transports = []

        def open_listener(node):
            self.opened.append(node)
            lst = FakeListener(node)
            self.listeners[node] = lst
            return lst

        def open_transport(node):
            self.transports.append(node)
            return self.receiver

        for p in (mock.patch.object(mod, "open_listener", open_listener),
                  mock.patch.object(mod, "open_transport", open_transport)):
            p.start()
            self.addCleanup(p.stop)

    def add_node(self, name="hidraw0", phys="usb-1/input2"):
        fake_hidraw(self.sysfs, name, "0003:0000046D:0000C548", "hid-generic", HIDPP_DESC, phys)
        return self.dev / name

    def wait(self, listeners, timeout):
        # Time passes in steps of at most 0.1 s, so run_for() stops on time.
        ready = [lst for lst in listeners if lst.queue]
        if not ready:
            self.clock.t += min(max(timeout, 0), 0.1)
        return ready

    def watcher(self, read=None):
        def fake_read():
            self.reads.append(self.clock.t)
            return 0
        return mod.Watcher(read=read or fake_read, wait=self.wait)

    def run_for(self, w, seconds):
        stop = self.clock.t + seconds
        while self.clock.t < stop:
            w.step()

    def battery(self, *lines):
        (self.out / "battery").write_text("".join(lines))

    def names(self):
        (self.state / "names").write_text(
            "usb-1/input2\t1\tMX Anywhere3SB\tMX Anywhere 3S for Business\tmouse\n"
            "usb-1/input2\t2\tMX MCHNCL\tMX Mechanical\tkeyboard\n")

    def test_a_connect_event_reads_after_the_debounce(self):
        node = self.add_node()
        w = self.watcher()
        w.step()
        self.assertEqual(self.opened, [node])
        self.listeners[node].queue.append(WS_EVENT)
        start = self.clock.t
        w.step()
        self.assertEqual(self.reads, [])
        self.run_for(w, 3)
        self.assertEqual(len(self.reads), 1)
        self.assertGreaterEqual(self.reads[0] - start, mod.DEBOUNCE)

    def test_a_burst_of_events_is_one_read(self):
        node = self.add_node()
        w = self.watcher()
        w.step()
        self.listeners[node].queue += [WS_EVENT, UB_EVENT, WS_EVENT]
        self.run_for(w, 1)
        self.listeners[node].queue.append(UB_EVENT)
        self.run_for(w, 4)
        self.assertEqual(len(self.reads), 1)

    def test_events_of_another_feature_or_a_reply_do_not_read(self):
        node = self.add_node()
        w = self.watcher()
        w.step()
        self.listeners[node].queue += [bytes.fromhex("11 01 05 00 01 00 00").ljust(20, b"\0"),
                                       bytes.fromhex("11 01 04 0a 01 01 01").ljust(20, b"\0"),
                                       bytes.fromhex("11 01 04 00").ljust(4, b"\0"),
                                       bytes.fromhex("10 01 41 04 a1 2b 40")]
        self.run_for(w, 5)
        self.assertEqual(self.reads, [])

    def test_feature_indices_come_from_get_feature_once_per_slot(self):
        node = self.add_node()
        w = self.watcher()
        w.step()
        # Keyboard on slot 2: UNIFIED BATTERY at index 7, not 9.
        self.listeners[node].queue.append(bytes.fromhex("11 02 07 00 50 04 00").ljust(20, b"\0"))
        self.run_for(w, 3)
        self.assertEqual(len(self.reads), 1)
        self.assertEqual(self.transports, [node])
        # Cached: a second event asks nothing.
        self.listeners[node].queue.append(bytes.fromhex("11 02 04 00 01 01 01").ljust(20, b"\0"))
        self.run_for(w, 3)
        self.assertEqual(len(self.reads), 2)
        self.assertEqual(self.transports, [node])

    def test_lookup_runs_under_the_lock(self):
        node = self.add_node()
        calls = []
        real = mod.fcntl.flock

        def flock(fd, op):
            calls.append(("flock", os.readlink(f"/proc/self/fd/{fd}"), op))
            return real(fd, op)

        def open_transport(n):
            calls.append(("open", n))
            return self.receiver
        w = self.watcher()
        w.step()
        self.listeners[node].queue.append(WS_EVENT)
        with mock.patch.object(mod.fcntl, "flock", flock), \
                mock.patch.object(mod, "open_transport", open_transport):
            w.step()
        self.assertEqual(calls[0], ("flock", str(self.state / "lock"),
                                    mod.fcntl.LOCK_EX | mod.fcntl.LOCK_NB))
        self.assertEqual(calls[1], ("open", node))

    def test_a_silent_device_is_looked_up_again_later_not_on_every_event(self):
        node = self.add_node()
        self.receiver.devices[1]["state"] = "silent"
        w = self.watcher()
        w.step()
        self.listeners[node].queue += [WS_EVENT, UB_EVENT]
        self.run_for(w, 3)
        self.assertEqual(self.reads, [])
        self.assertEqual(self.transports, [node])
        self.clock.t += mod.LOOKUP_RETRY
        self.receiver.devices[1]["state"] = "on"
        self.listeners[node].queue.append(WS_EVENT)
        self.run_for(w, 3)
        self.assertEqual(self.transports, [node, node])
        self.assertEqual(len(self.reads), 1)

    def test_a_fresh_successful_value_holds_the_read_off(self):
        node = self.add_node()
        self.names()
        self.battery(f"MX Anywhere 3S for Business\t100\tfull\t{self.wall - 100}\tmouse\n")
        w = self.watcher()
        w.step()
        self.listeners[node].queue.append(WS_EVENT)
        self.run_for(w, 5)
        self.assertEqual(self.reads, [])

    def test_a_stale_value_does_not_hold_the_read_off(self):
        node = self.add_node()
        self.names()
        self.battery(f"MX Anywhere 3S for Business\t100\tfull\t{self.wall - mod.HOLD_OFF - 1}\tmouse\n")
        w = self.watcher()
        w.step()
        self.listeners[node].queue.append(WS_EVENT)
        self.run_for(w, 5)
        self.assertEqual(len(self.reads), 1)

    def test_not_reachable_or_no_percent_does_not_hold_the_read_off(self):
        node = self.add_node()
        for line in (f"MX Anywhere 3S for Business\t100\tnot reachable\t{self.wall - 10}\tmouse\n",
                     f"MX Anywhere 3S for Business\t-\tunknown\t{self.wall - 10}\tmouse\n"):
            self.reads.clear()
            self.names()
            self.battery(line)
            w = self.watcher()
            w.step()
            self.listeners[node].queue.append(WS_EVENT)
            self.run_for(w, 5)
            self.assertEqual(len(self.reads), 1, line)

    def test_a_slot_with_no_name_yet_is_read(self):
        node = self.add_node()
        self.battery(f"MX Mechanical\t80\tdischarging\t{self.wall - 10}\tkeyboard\n")
        w = self.watcher()
        w.step()
        self.listeners[node].queue.append(WS_EVENT)
        self.run_for(w, 5)
        self.assertEqual(len(self.reads), 1)

    def test_the_hold_off_of_one_device_does_not_hold_another(self):
        node = self.add_node()
        self.names()
        self.battery(f"MX Anywhere 3S for Business\t100\tfull\t{self.wall - 10}\tmouse\n",
                     f"MX Mechanical\t80\tnot reachable\t{self.wall - 10}\tkeyboard\n")
        w = self.watcher()
        w.step()
        self.listeners[node].queue += [WS_EVENT, bytes.fromhex("11 02 04 00 01 01 01").ljust(20, b"\0")]
        self.run_for(w, 5)
        self.assertEqual(len(self.reads), 1)

    def test_a_vanished_node_is_dropped_and_found_again(self):
        node = self.add_node()
        w = self.watcher()
        w.step()
        first = self.listeners[node]
        first.queue.append(OSError(19, "No such device"))
        w.step()
        self.assertTrue(first.closed)
        self.assertIn("thinfloe: battery: ", sys_stderr())
        self.run_for(w, mod.RESCAN + 0.5)
        self.assertEqual(self.opened, [node, node])
        self.listeners[node].queue.append(WS_EVENT)
        self.run_for(w, 3)
        self.assertEqual(len(self.reads), 1)

    def test_a_read_that_would_block_keeps_the_node(self):
        node = self.add_node()
        w = self.watcher()
        w.step()
        lst = self.listeners[node]
        lst.queue += [BlockingIOError(11, "Resource temporarily unavailable"), WS_EVENT]
        self.run_for(w, 3)
        self.assertFalse(lst.closed)
        self.assertEqual(self.opened, [node])
        self.assertEqual(len(self.reads), 1)

    def test_no_node_rescans_until_one_appears(self):
        w = self.watcher()
        self.run_for(w, 12)
        self.assertEqual(self.opened, [])
        node = self.add_node()
        self.run_for(w, mod.RESCAN + 0.5)
        self.assertEqual(self.opened, [node])

    def test_a_node_gone_from_sysfs_is_closed(self):
        node = self.add_node()
        w = self.watcher()
        w.step()
        lst = self.listeners[node]
        import shutil
        shutil.rmtree(self.sysfs / "class" / "hidraw" / "hidraw0")
        self.run_for(w, mod.RESCAN + 0.5)
        self.assertTrue(lst.closed)

    def test_a_failing_open_is_retried_at_the_next_rescan(self):
        node = self.add_node()
        fails = [True]

        def open_listener(n):
            if fails.pop(0) if fails else False:
                raise OSError(13, "Permission denied")
            self.opened.append(n)
            self.listeners[n] = FakeListener(n)
            return self.listeners[n]
        with mock.patch.object(mod, "open_listener", open_listener):
            w = self.watcher()
            self.run_for(w, mod.RESCAN + 0.5)
        self.assertEqual(self.opened, [node])

    def test_a_bad_report_or_a_failing_read_never_ends_the_watcher(self):
        node = self.add_node()

        def broken_read():
            self.reads.append(self.clock.t)
            raise RuntimeError("boom")
        w = self.watcher(read=broken_read)
        w.step()
        with mock.patch.object(mod, "is_connect_event", side_effect=[ValueError("bad"), True]):
            self.listeners[node].queue += [WS_EVENT, WS_EVENT]
            self.run_for(w, 3)
        self.assertEqual(len(self.reads), 1)
        self.listeners[node].queue.append(UB_EVENT)
        self.run_for(w, 3)
        self.assertEqual(len(self.reads), 2)

    def test_the_trigger_runs_the_reading_under_the_lock(self):
        node = self.add_node()
        calls = []
        real = mod.fcntl.flock

        def flock(fd, op):
            calls.append(("flock", op))
            return real(fd, op)
        w = mod.Watcher(wait=self.wait)   # the real reading
        w.step()
        self.listeners[node].queue.append(WS_EVENT)
        w.step()                          # lookup
        calls.clear()
        self.transports.clear()

        def open_transport(n):
            calls.append(("open", n))
            return self.receiver
        with mock.patch.object(mod.fcntl, "flock", flock), \
                mock.patch.object(mod, "open_transport", open_transport):
            self.run_for(w, 3)
        self.assertEqual(calls[0], ("flock", mod.fcntl.LOCK_EX | mod.fcntl.LOCK_NB))
        self.assertEqual(calls[1], ("open", node))
        self.assertIn(f"MX Anywhere 3S for Business\t100\tdischarging\t{self.wall}\tmouse\n",
                      (self.out / "battery").read_text())


def sys_stderr():
    import sys
    return sys.stderr.getvalue()


class WatchMainTests(unittest.TestCase):
    def test_watch_runs_the_watcher(self):
        with mock.patch.object(mod, "watch", return_value=0) as w:
            self.assertEqual(mod.main(["watch"]), 0)
        w.assert_called_once_with()

    def test_other_arguments_are_usage_errors(self):
        for argv in (["enable"], ["watch", "x"], ["--watch"]):
            with mock.patch("sys.stderr", new_callable=io.StringIO) as err, \
                    mock.patch.object(mod, "watch") as w:
                self.assertEqual(mod.main(argv), 2)
            w.assert_not_called()
            self.assertIn("usage: peripheral-battery [watch]", err.getvalue())


if __name__ == "__main__":
    unittest.main()
