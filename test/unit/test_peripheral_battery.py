#!/usr/bin/env python3
"""Offline tests for /usr/libexec/thinfloe/peripheral-battery.

The helper has no .py extension, so it is loaded through importlib like
thinfloe-addon in test_addon.py. Its entry point is guarded, so importing
it does nothing.

FakeReceiver answers the way a Logi Bolt receiver (046d:c548) answered on a
test machine on 2026-11-03: see the reply table in
docs/superpowers/specs/2026-11-03-peripheral-battery-tile.md. Time is a fake
clock that the receiver advances, so timeouts cost no real time.
"""

import fnmatch
import importlib.machinery
import importlib.util
import io
import os
import re
import shutil
import stat
import subprocess
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


class Hushed(FakeReceiver):
    """A receiver that gives no answer to the requests hush(msg) names."""

    def __init__(self, devices, clock, hush):
        super().__init__(devices, clock)
        self.hush = hush

    def _answer(self, msg):
        return None if self.hush(msg) else super()._answer(msg)


class RegisterError(FakeReceiver):
    """A receiver that answers the register requests refuse(msg) names with
    the error refuse(msg) returns (a code, or None for an ordinary answer)."""

    def __init__(self, devices, clock, refuse):
        super().__init__(devices, clock)
        self.refuse = refuse

    def _answer(self, msg):
        code = self.refuse(msg)
        if code is None:
            return super()._answer(msg)
        return 0.05, bytes([0x10, 0xFF, 0x8F, 0x83, 0xB5, code, 0x00])


def pairing_request(msg):
    return msg[:4] == bytes([0x10, 0xFF, 0x83, 0xB5]) and msg[4] & 0xF0 == 0x50


def receiver_name_request(msg):
    return msg[:4] == bytes([0x10, 0xFF, 0x83, 0xB5]) and msg[4] & 0xF0 == 0x60


def feature_lookup_of(fid):
    """The root feature's getFeature for fid, on any slot."""
    want = bytes([fid >> 8, fid & 0xFF])
    return lambda msg: msg[0] == 0x11 and msg[2] == 0 and msg[3] >> 4 == 0 and msg[4:6] == want


def device_name_request(msg):
    """Feature 0x0005: its getFeature and its calls, at index 3 in dev()."""
    return msg[0] == 0x11 and (msg[2] == 3 or (msg[2] == 0 and msg[3] >> 4 == 0 and msg[4:6] == b"\x00\x05"))


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

    def test_unified_battery_without_state_of_charge_has_no_percent(self):
        t = FakeReceiver({1: dev("Mouse", ub=(0, 4, 0)), 2: dev("Pad", ub=(0, 1, 0))}, self.clock)
        got, _ = self.readings(t)
        self.assertEqual(got, [R("Mouse", None, "discharging", 1000),
                               R("Pad", None, "critical", 1000)])

    def test_battery_status_level_zero_is_unknown(self):
        t = FakeReceiver({1: dev("Mouse", bs=(0, 0, 0))}, self.clock)
        got, _ = self.readings(t)
        self.assertEqual(got, [R("Mouse", None, "discharging", 1000)])

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
        merged = mod.merge([], [], [("MX Anywhere3SB", "unknown", "")], now=1000)
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

    def test_a_device_that_sleeps_after_the_ping_stopped_answering(self):
        for after in ("silent", "off"):
            class Sleepy(FakeReceiver):
                def _answer(self, msg):
                    ans = super()._answer(msg)
                    if msg[0] == 0x11 and msg[2] == 0 and msg[3] >> 4 == 1:   # the ping
                        self.devices[msg[1]]["state"] = after
                    return ans
            t = Sleepy({1: dev("A", ub=(85, 8, 0), kind=0x01)}, self.clock)
            got, unreachable = mod.read_receiver(t, now=1000)
            self.assertEqual(got, [], after)
            self.assertEqual(unreachable, [(1, "A", "keyboard")], after)

    def test_only_unknown_device_on_the_pairing_register_means_empty(self):
        for code in (0x01, 0x02, 0x07, 0x09, 0x0A):
            t = RegisterError({1: dev("A", ub=(85, 8, 0), kind=0x01)}, self.clock,
                              lambda m: code if pairing_request(m) and m[4] == 0x51 else None)
            got, unreachable = mod.read_receiver(t, now=1000)
            self.assertEqual(got, [], hex(code))
            self.assertIn((1, None, "unknown"), unreachable, hex(code))
        t = RegisterError({1: dev("A", ub=(85, 8, 0), kind=0x01)}, self.clock,
                          lambda m: 0x08 if pairing_request(m) and m[4] == 0x51 else None)
        self.assertEqual(mod.read_receiver(t, now=1000), ([], []))

    def test_a_short_register_error_is_no_empty_slot_and_no_crash(self):
        class Short(FakeReceiver):
            def _answer(self, msg):
                if pairing_request(msg):
                    return 0.05, bytes([0x10, 0xFF, 0x8F, 0x83, 0xB5])
                return super()._answer(msg)
        t = Short({1: dev("A", ub=(85, 8, 0))}, self.clock)
        got, unreachable = mod.read_receiver(t, now=1000)
        self.assertEqual(got, [])
        self.assertIn((1, None, "unknown"), unreachable)

    def test_a_register_error_on_the_pairing_name_gives_no_name(self):
        for code in (0x08, 0x09):
            for chunk in (1, 2):
                t = RegisterError({1: dev("A", "MX Anywhere 3S for Business")}, self.clock,
                                  lambda m: code if receiver_name_request(m) and m[5] == chunk else None)
                self.assertIsNone(mod._receiver_name(t, 1, self.clock() + 100), (code, chunk))

    def test_a_timeout_on_a_later_chunk_of_the_pairing_name_gives_no_name(self):
        t = Hushed({1: dev("A", "MX Anywhere 3S for Business")}, self.clock,
                   lambda m: receiver_name_request(m) and m[5] == 2)
        self.assertIsNone(mod._receiver_name(t, 1, self.clock() + 100))
        got, unreachable = mod.read_receiver(t, now=1000)
        self.assertEqual(got, [])
        self.assertIn((1, None, "unknown"), unreachable)

    def test_a_device_silent_on_a_later_battery_feature_stopped_answering(self):
        for fid in (0x1000, 0x1001):
            t = Hushed({1: dev("A", kind=0x01)}, self.clock, feature_lookup_of(fid))
            got, unreachable = mod.read_receiver(t, now=1000)
            self.assertEqual(got, [], hex(fid))
            self.assertEqual(unreachable, [(1, "A", "keyboard")], hex(fid))

    def test_a_device_without_any_battery_feature_has_no_percent(self):
        t = FakeReceiver({1: dev("Plain")}, self.clock)
        self.assertEqual(self.readings(t)[0], [R("Plain", None, "unknown", 1000)])


class MergeAndNameTests(unittest.TestCase):
    def test_unreachable_device_keeps_last_reading(self):
        prev = [R("MX Mechanical", 80, "discharging", 500)]
        out = mod.merge([], prev, [("MX Mechanical", "unknown", "")], now=1000)
        self.assertEqual(out, [R("MX Mechanical", 80, "not reachable", 500)])

    def test_unreachable_device_keeps_the_previous_kind(self):
        prev = [R("BT Mouse", 80, "discharging", 500, "mouse")]
        out = mod.merge([], prev, [("BT Mouse", "unknown", "")], now=1000)
        self.assertEqual(out, [R("BT Mouse", 80, "not reachable", 500, "mouse")])

    def test_a_known_kind_of_an_unreachable_device_wins(self):
        prev = [R("K", 80, "discharging", 500, "unknown")]
        out = mod.merge([], prev, [("K", "keyboard", "")], now=1000)
        self.assertEqual(out, [R("K", 80, "not reachable", 500, "keyboard")])
        self.assertEqual(mod.merge([], [], [("K", "keyboard", "")], now=1000),
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

    def test_parse_takes_one_name_with_and_without_a_percent(self):
        try:
            got = mod.parse_lines("a\t-\tfull\t7\na\t5\tfull\t7\n")
        except TypeError as e:
            self.fail(f"two lines of one name ended the parse: {e}")
        self.assertEqual(got, [R("a", None, "full", 7), R("a", 5, "full", 7)])

    def test_parse_skips_digits_int_does_not_take(self):
        for bad in ("A\t\u00b2\tfull\t1000\tmouse", "A\t5\tfull\t\u00b2\tmouse",
                    "A\t\u0663\tfull\t1000\tmouse",
                    "A\t" + "9" * 5000 + "\tfull\t1000\tmouse",
                    "A\t5\tfull\t" + "9" * 5000 + "\tmouse"):
            self.assertEqual(mod.parse_lines(bad), [], bad[:20])

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

    def test_power_supply_with_a_name_that_is_not_utf8(self):
        ps = self.sysfs / "class" / "power_supply"
        write(ps / "hid-1-battery" / "uevent",
              b"POWER_SUPPLY_SCOPE=Device\nPOWER_SUPPLY_MODEL_NAME=Mouse \xff\xfe\n"
              b"POWER_SUPPLY_CAPACITY=50\n", "wb")
        try:
            got = mod.read_power_supply(self.sysfs, now=1000)
        except ValueError as e:
            self.fail(f"the name ended the reading: {e!r}")
        self.assertEqual(got, [R("Mouse \ufffd\ufffd", 50, "unknown", 1000, "mouse")])

    def test_hid_node_with_a_name_that_is_not_utf8(self):
        d = "hidraw0"
        fake_hidraw(self.sysfs, d, "0003:0000046D:0000C548", "hid-generic", HIDPP_DESC, "usb-1/input2")
        write(self.sysfs / "class" / "hidraw" / d / "device" / "uevent",
              b"HID_ID=0003:0000046D:0000C548\nHID_NAME=Logi \xff\nHID_PHYS=usb-1/input2\n", "wb")
        try:
            nodes = mod.find_hidpp_nodes(self.sysfs, Path("/dev"))
        except ValueError as e:
            self.fail(f"the name ended the search: {e!r}")
        self.assertEqual(nodes, [(Path("/dev/hidraw0"), "usb-1/input2")])

    def test_a_line_separator_in_a_device_name_is_no_line_of_the_uevent(self):
        d = "hidraw0"
        fake_hidraw(self.sysfs, d, "0003:00000451:000082FF", "hid-generic", HIDPP_DESC, "usb-1/input2")
        write(self.sysfs / "class" / "hidraw" / d / "device" / "uevent",
              "HID_ID=0003:00000451:000082FF\nHID_NAME=x\u2028HID_ID=0003:0000046D:0000C548\nHID_PHYS=p\n")
        self.assertEqual(mod.find_hidpp_nodes(self.sysfs, Path("/dev")), [])

    def test_power_supply_capacity_zero_is_no_percent(self):
        ps = self.sysfs / "class" / "power_supply"
        write(ps / "hid-1-battery" / "uevent",
              "POWER_SUPPLY_SCOPE=Device\nPOWER_SUPPLY_MODEL_NAME=Pen\n"
              "POWER_SUPPLY_STATUS=Unknown\nPOWER_SUPPLY_CAPACITY=0\n")
        self.assertEqual(mod.read_power_supply(self.sysfs, now=1000),
                         [R("Pen", None, "unknown", 1000)])


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

    def test_readings_name_each_line_for_the_reader_alone(self):
        r = FakeReceiver({1: dev("MX Mechanical", "MX MCHNCL", ub=(90, 8, 0), kind=0x01)}, self.clock)
        self.run_main(r)
        readings = self.state / "readings"
        self.assertEqual(readings.read_text(),
                         "BT Mouse\tBT Mouse\t100\tfull\t1000\tmouse\n"
                         "usb-1/input2/1\tMX Mechanical\t90\tdischarging\t1000\tkeyboard\n")
        self.assertEqual(stat.S_IMODE(os.stat(readings).st_mode), 0o600)
        self.assertEqual(sorted(os.listdir(self.out)), ["battery"])

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

    def first_run(self, device=None):
        """A run that reads the full name and a value of slot 1."""
        r = FakeReceiver({1: device or dev("MX Anywhere 3S for Business", "MX Anywhere3SB",
                                           ub=(85, 8, 0), kind=0x42)}, self.clock)
        self.assertEqual(self.run_main(r), 0)
        self.assertIn("MX Anywhere 3S for Business\t85\tdischarging\t1000\tmouse\n", self.battery())
        return r

    KEPT = "MX Anywhere 3S for Business\t85\tnot reachable\t1000\tmouse\n"

    def test_a_receiver_that_times_out_on_pairing_keeps_the_device(self):
        r = self.first_run()
        silent = Hushed(r.devices, self.clock, pairing_request)
        with mock.patch.object(mod.time, "time", lambda: 2000), \
                mock.patch("sys.stderr", new_callable=io.StringIO) as err:
            self.assertEqual(self.run_main(silent), 0)
        self.assertIn(self.KEPT, self.battery())
        self.assertNotIn("device 1", self.battery())
        self.assertIn("slot 2", err.getvalue())

    def test_a_receiver_that_times_out_on_the_name_keeps_the_device(self):
        r = self.first_run()
        silent = Hushed(r.devices, self.clock, receiver_name_request)
        with mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(silent), 0)
        self.assertIn(self.KEPT, self.battery())
        self.assertNotIn("device 1", self.battery())

    def test_a_slot_the_receiver_does_not_answer_for_and_nobody_knows_is_no_line(self):
        silent = Hushed({1: dev("A", ub=(40, 4, 0))}, self.clock, pairing_request)
        with mock.patch("sys.stderr"):
            self.assertEqual(self.run_main(silent), 0)
        self.assertEqual(self.battery(), "BT Mouse\t100\tfull\t1000\tmouse\n")

    def test_a_node_that_fails_keeps_its_devices(self):
        self.first_run()
        with mock.patch("sys.stderr"), mock.patch.object(mod.time, "time", lambda: 2000), \
                mock.patch.object(mod, "open_transport", side_effect=OSError(5, "Input/output error")):
            self.assertEqual(mod.main(), 0)
        self.assertIn(self.KEPT, self.battery())

    def test_a_device_no_longer_paired_does_not_come_back_when_the_node_fails(self):
        r = self.first_run()
        del r.devices[1]
        self.assertEqual(self.run_main(r), 0)
        self.assertEqual(self.battery(), "BT Mouse\t100\tfull\t1000\tmouse\n")
        with mock.patch("sys.stderr"), \
                mock.patch.object(mod, "open_transport", side_effect=OSError(5, "Input/output error")):
            self.assertEqual(mod.main(), 0)
        self.assertEqual(self.battery(), "BT Mouse\t100\tfull\t1000\tmouse\n")

    def test_an_unexpected_error_on_a_node_keeps_its_devices(self):
        r = self.first_run()

        class Broken(FakeReceiver):
            def write(self, data):
                raise ValueError("garbage")
        with mock.patch("sys.stderr"), mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(Broken(r.devices, self.clock)), 0)
        self.assertIn(self.KEPT, self.battery())

    def test_a_budget_used_up_on_one_node_keeps_the_devices_of_the_next(self):
        fake_hidraw(self.sysfs, "hidraw5", "0003:0000046D:0000C548", "hid-generic", HIDPP_DESC, "usb-2/input2")
        first = FakeReceiver({}, self.clock)
        second = FakeReceiver({1: dev("MX Mechanical", "MX MCHNCL", ub=(70, 8, 0), kind=0x01)}, self.clock)
        by_node = {"hidraw4": first, "hidraw5": second}
        with mock.patch.object(mod, "open_transport", lambda n: by_node[n.name]):
            self.assertEqual(mod.main(), 0)
        self.assertIn("MX Mechanical\t70\tdischarging\t1000\tkeyboard\n", self.battery())
        by_node["hidraw4"] = FakeReceiver({n: dev(f"D{n}", state="silent") for n in range(1, 7)}, self.clock)
        with mock.patch.object(mod, "open_transport", lambda n: by_node[n.name]), \
                mock.patch.object(mod, "RUN_BUDGET", 1.0), \
                mock.patch("sys.stderr"), mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(mod.main(), 0)
        self.assertIn("MX Mechanical\t70\tnot reachable\t1000\tkeyboard\n", self.battery())

    def test_a_device_that_sleeps_after_the_ping_keeps_its_value(self):
        r = self.first_run()

        class Sleepy(FakeReceiver):
            def _answer(self, msg):
                ans = super()._answer(msg)
                if msg[0] == 0x11 and msg[2] == 0 and msg[3] >> 4 == 1:   # the ping
                    self.devices[msg[1]]["state"] = "silent"
                return ans
        with mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(Sleepy(r.devices, self.clock)), 0)
        self.assertIn(self.KEPT, self.battery())

    def test_a_device_name_that_fails_once_keeps_the_full_name(self):
        r = self.first_run()
        r.devices[1]["ub"] = (84, 8, 0)
        silent = Hushed(r.devices, self.clock, device_name_request)
        with mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(silent), 0)
        self.assertIn("MX Anywhere 3S for Business\t84\tdischarging\t2000\tmouse\n", self.battery())
        self.assertNotIn("MX Anywhere3SB\t84", self.battery())
        self.assertIn("\tMX Anywhere 3S for Business\t", (self.state / "names").read_text())

    def test_a_device_name_cut_off_halfway_keeps_the_full_name(self):
        r = self.first_run()
        cut = Hushed(r.devices, self.clock, lambda m: m[0] == 0x11 and m[2] == 3 and m[3] >> 4 == 1 and m[4] > 0)
        with mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(cut), 0)
        self.assertIn("MX Anywhere 3S for Business\t85\tdischarging\t2000\tmouse\n", self.battery())

    def test_a_device_name_that_fails_on_the_first_read_is_the_receiver_name(self):
        silent = Hushed({1: dev("MX Anywhere 3S for Business", "MX Anywhere3SB", ub=(85, 8, 0))},
                        self.clock, device_name_request)
        self.assertEqual(self.run_main(silent), 0)
        self.assertIn("MX Anywhere3SB\t85\tdischarging\t1000\tunknown\n", self.battery())

    def same_named(self, first=(40, 4, 0), second=(90, 8, 0)):
        return {1: dev("MX Keys", "MX KEYS", ub=first, kind=0x01),
                2: dev("MX Keys", "MX KEYS", ub=second, kind=0x01)}

    def test_two_devices_of_one_name_are_two_lines(self):
        self.assertEqual(self.run_main(FakeReceiver(self.same_named(), self.clock)), 0)
        self.assertEqual(self.battery(),
                         "BT Mouse\t100\tfull\t1000\tmouse\n"
                         "MX Keys\t40\tdischarging\t1000\tkeyboard\n"
                         "MX Keys\t90\tdischarging\t1000\tkeyboard\n")

    def test_two_devices_of_one_name_stay_in_slot_order_whatever_they_read(self):
        # The menu numbers them in file order: Keyboard 1 must stay slot 1.
        self.assertEqual(self.run_main(FakeReceiver(self.same_named((90, 8, 0), (40, 4, 0)), self.clock)), 0)
        self.assertEqual(self.battery(),
                         "BT Mouse\t100\tfull\t1000\tmouse\n"
                         "MX Keys\t90\tdischarging\t1000\tkeyboard\n"
                         "MX Keys\t40\tdischarging\t1000\tkeyboard\n")

    def test_each_of_two_devices_of_one_name_keeps_its_own_reading(self):
        r = FakeReceiver(self.same_named(), self.clock)
        self.run_main(r)
        r.devices[2]["state"] = "off"
        r.devices[1]["ub"] = (39, 4, 0)
        with mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(r), 0)
        self.assertEqual(self.battery(),
                         "BT Mouse\t100\tfull\t2000\tmouse\n"
                         "MX Keys\t39\tdischarging\t2000\tkeyboard\n"
                         "MX Keys\t90\tnot reachable\t1000\tkeyboard\n")

    def test_two_receivers_with_a_device_of_one_name_are_two_lines(self):
        fake_hidraw(self.sysfs, "hidraw5", "0003:0000046D:0000C548", "hid-generic", HIDPP_DESC, "usb-2/input2")
        one = FakeReceiver({1: dev("MX Keys", "MX KEYS", ub=(40, 4, 0), kind=0x01)}, self.clock)
        two = FakeReceiver({1: dev("MX Keys", "MX KEYS", ub=(90, 8, 0), kind=0x01)}, self.clock)
        by_node = {"hidraw4": one, "hidraw5": two}
        with mock.patch.object(mod, "open_transport", lambda n: by_node[n.name]):
            self.assertEqual(mod.main(), 0)
        self.assertEqual(self.battery().count("MX Keys"), 2)

    def test_a_device_paired_anew_in_a_slot_does_not_get_the_old_value(self):
        r = FakeReceiver({1: dev("MX Mechanical", "MX MCHNCL", ub=(90, 8, 0), kind=0x01)}, self.clock)
        self.run_main(r)
        r.devices[1] = dev("MX Keys", "MX KEYS", state="off", kind=0x01)
        with mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(r), 0)
        self.assertEqual(self.battery(),
                         "BT Mouse\t100\tfull\t2000\tmouse\n"
                         "MX KEYS\t-\tnot reachable\t2000\tkeyboard\n")

    def test_two_power_supply_devices_of_one_name_are_two_lines_and_one_can_go(self):
        ps = self.sysfs / "class" / "power_supply"
        write(ps / "hidpp_battery_1" / "uevent",
              "POWER_SUPPLY_SCOPE=Device\nPOWER_SUPPLY_MODEL_NAME=BT Mouse\nPOWER_SUPPLY_STATUS=Discharging\nPOWER_SUPPLY_CAPACITY=50\n")
        r = FakeReceiver({}, self.clock)
        self.run_main(r)
        self.assertEqual(self.battery(), "BT Mouse\t100\tfull\t1000\tmouse\n"
                                         "BT Mouse\t50\tdischarging\t1000\tmouse\n")
        (ps / "hidpp_battery_1" / "uevent").unlink()
        with mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(r), 0)
        self.assertEqual(self.battery(), "BT Mouse\t100\tfull\t2000\tmouse\n"
                                         "BT Mouse\t50\tnot reachable\t1000\tmouse\n")

    def test_a_receiver_error_on_the_name_keeps_the_device_and_the_pairing_name(self):
        r = self.first_run()
        names = (self.state / "names").read_text()
        refusing = RegisterError(r.devices, self.clock, lambda m: 0x09 if receiver_name_request(m) else None)
        with mock.patch.object(mod.time, "time", lambda: 2000):
            self.assertEqual(self.run_main(refusing), 0)
        self.assertIn(self.KEPT, self.battery())
        self.assertNotIn("device 1", self.battery())
        self.assertEqual((self.state / "names").read_text(), names)

    def test_a_timeout_on_pairing_leaves_the_pairing_name_in_the_cache(self):
        r = self.first_run()
        names = (self.state / "names").read_text()
        silent = Hushed(r.devices, self.clock, pairing_request)
        with mock.patch.object(mod.time, "time", lambda: 2000), mock.patch("sys.stderr"):
            self.assertEqual(self.run_main(silent), 0)
        self.assertEqual((self.state / "names").read_text(), names)
        self.assertNotIn("None", names)

    CORRUPT = [b"\xff\xfe not text\n",
               "usb-1/input2\t\u00b2\tA\tA\tmouse\n".encode(),
               "usb-1/input2/1\tA\t\u00b2\tfull\t1000\tmouse\n".encode(),
               "usb-1/input2/1\tA\t5\tfull\t\u00b2\tmouse\n".encode(),
               ("usb-1/input2\t" + "9" * 5000 + "\tA\tA\tmouse\n").encode(),
               ("usb-1/input2/1\tA\t" + "9" * 5000 + "\tfull\t1000\tmouse\n").encode()]

    def test_a_corrupt_state_file_does_not_stop_a_run(self):
        r = FakeReceiver({1: dev("A", ub=(40, 4, 0))}, self.clock)
        for which in ("readings", "names", "power-supply"):
            for bad in self.CORRUPT:
                (self.state / which).write_bytes(bad)
                (self.out / "battery").unlink(missing_ok=True)
                self.assertEqual(self.run_main(r), 0, (which, bad[:20]))
                self.assertIn("A\t40\t", self.battery(), (which, bad[:20]))
                # Whatever the run wrote is readable, for the next one.
                (self.state / which).unlink()

    def test_write_is_atomic(self):
        r = FakeReceiver({1: dev("A", ub=(40, 4, 0))}, self.clock)
        self.run_main(r)
        before = self.battery()
        with mock.patch.object(mod, "format_lines", side_effect=RuntimeError("boom")), \
                mock.patch("sys.stderr"):
            self.assertEqual(self.run_main(r), 1)
        self.assertEqual(self.battery(), before)
        self.assertEqual(sorted(os.listdir(self.out)), ["battery"])
        self.assertEqual(sorted(os.listdir(self.state)), ["lock", "names", "power-supply", "readings"])

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
# (a test machine, 2026-11-03, slot 1): WIRELESS DEVICE STATUS at feature
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
        """The public file, and the reader's own copy that names each line's
        slot (MX Anywhere 3S in 1, MX Mechanical in 2)."""
        (self.out / "battery").write_text("".join(lines))
        slots = {"MX Anywhere": 1, "MX Mechanical": 2}
        (self.state / "readings").write_text("".join(
            f"usb-1/input2/{slot}\t{line}" for line in lines
            for prefix, slot in slots.items() if line.startswith(prefix)))

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

    def test_a_connect_event_whose_lookup_failed_reads_when_the_device_answers(self):
        node = self.add_node()
        self.receiver.devices[1]["state"] = "silent"
        w = self.watcher()
        w.step()
        self.listeners[node].queue.append(WS_EVENT)
        self.run_for(w, 3)
        self.assertEqual(self.reads, [])
        self.receiver.devices[1]["state"] = "on"
        self.run_for(w, mod.LOOKUP_RETRY + mod.DEBOUNCE + 1)   # no further event
        self.assertEqual(len(self.reads), 1)

    def test_a_connect_event_during_the_wait_for_a_retry_is_kept(self):
        node = self.add_node()
        self.receiver.devices[1]["state"] = "silent"
        w = self.watcher()
        w.step()
        other = bytes.fromhex("11 01 05 00 01 00 00").ljust(20, b"\0")
        self.listeners[node].queue.append(other)
        self.run_for(w, 3)
        self.listeners[node].queue.append(UB_EVENT)
        self.run_for(w, 3)
        self.assertEqual(self.transports, [node])
        self.receiver.devices[1]["state"] = "on"
        self.run_for(w, mod.LOOKUP_RETRY + mod.DEBOUNCE + 1)
        self.assertEqual(self.transports, [node, node])
        self.assertEqual(len(self.reads), 1)

    def test_a_connect_event_held_before_another_event_still_reads(self):
        node = self.add_node()
        self.receiver.devices[1]["state"] = "silent"
        w = self.watcher()
        w.step()
        self.listeners[node].queue.append(WS_EVENT)
        self.run_for(w, 3)
        self.listeners[node].queue.append(bytes.fromhex("11 01 05 00 01 00 00").ljust(20, b"\0"))
        self.run_for(w, 3)
        self.receiver.devices[1]["state"] = "on"
        self.run_for(w, mod.LOOKUP_RETRY + mod.DEBOUNCE + 1)
        self.assertEqual(len(self.reads), 1)

    def test_held_reports_are_bounded(self):
        node = self.add_node()
        self.receiver.devices[1]["state"] = "silent"
        w = self.watcher()
        w.step()
        other = bytes.fromhex("11 01 05 00 01 00 00").ljust(20, b"\0")
        for _ in range(mod.MAX_HELD + 5):
            w.on_report(node, "usb-1/input2", other)
        self.assertEqual(len(w.held[("usb-1/input2", 1)][1]), mod.MAX_HELD)
        # One beyond the bound is not kept: only those held are judged.
        w.on_report(node, "usb-1/input2", WS_EVENT)
        self.assertNotIn(WS_EVENT, w.held[("usb-1/input2", 1)][1])

    def test_a_dropped_node_forgets_its_slots_and_no_other(self):
        node = self.add_node()
        other = self.add_node("hidraw1", "usb-2/input2")
        w = self.watcher()
        w.step()
        self.receiver.devices[1]["state"] = "silent"
        self.listeners[node].queue.append(WS_EVENT)
        self.listeners[other].queue.append(WS_EVENT)
        w.step()
        self.assertIn(("usb-1/input2", 1), w.held)
        self.assertIn(("usb-2/input2", 1), w.held)
        self.assertIn(("usb-1/input2", 1), w.lookup_failed)
        w.indices[("usb-1/input2", 2)] = (4, 7)
        w.index_owner[("usb-1/input2", 2)] = "MX MCHNCL"
        w.indices[("usb-2/input2", 2)] = (4, 7)
        w._drop(node, None)
        for table in (w.indices, w.index_owner, w.lookup_failed, w.held):
            self.assertEqual([k for k in table if k[0] == "usb-1/input2"], [])
        self.assertIn(("usb-2/input2", 1), w.held)
        self.assertIn(("usb-2/input2", 2), w.indices)

    def test_a_lookup_that_raises_counts_as_a_try_and_waits_for_the_retry(self):
        node = self.add_node()
        w = self.watcher()
        w.step()
        self.listeners[node].queue.append(WS_EVENT)
        with mock.patch.object(mod, "open_transport", side_effect=OSError(5, "Input/output error")) as op:
            self.run_for(w, 3)
        self.assertEqual(op.call_count, 1)
        self.assertEqual(w.held[("usb-1/input2", 1)][2], 1)
        self.assertIn("Input/output error", sys_stderr())

    def test_a_failing_step_waits_a_second_before_the_next(self):
        w = self.watcher()
        with mock.patch.object(w, "step", side_effect=[RuntimeError("boom"), KeyboardInterrupt]), \
                mock.patch.object(mod.time, "sleep") as sleep:
            with self.assertRaises(KeyboardInterrupt):
                w.run()
        sleep.assert_called_once_with(1)
        self.assertIn("boom", sys_stderr())

    def test_a_corrupt_state_file_does_not_stop_the_watcher(self):
        node = self.add_node()
        for name in ("readings", "names"):
            for bad in MainTests.CORRUPT:
                (self.state / name).write_bytes(bad)
                self.reads.clear()
                w = self.watcher()
                w.indices[("usb-1/input2", 1)] = (4, 9)
                w.rescan()
                w.next_scan = self.clock.t + 3600
                self.listeners[node].queue.append(WS_EVENT)
                self.run_for(w, mod.DEBOUNCE + 1)
                self.assertEqual(len(self.reads), 1, (name, bad[:20]))
                self.assertNotIn("Error", sys_stderr(), (name, bad[:20]))

    def test_a_held_event_that_is_no_connect_reads_nothing(self):
        node = self.add_node()
        self.receiver.devices[1]["state"] = "silent"
        w = self.watcher()
        w.step()
        self.listeners[node].queue.append(bytes.fromhex("11 01 05 00 01 00 00").ljust(20, b"\0"))
        self.run_for(w, 3)
        self.receiver.devices[1]["state"] = "on"
        self.run_for(w, mod.LOOKUP_RETRY + mod.DEBOUNCE + 1)
        self.assertEqual(len(self.transports), 2)
        self.assertEqual(self.reads, [])

    def test_a_lookup_that_cannot_open_the_node_keeps_the_event(self):
        node = self.add_node()
        w = self.watcher()
        w.step()
        self.listeners[node].queue.append(WS_EVENT)
        with mock.patch.object(mod, "open_transport", side_effect=OSError(5, "Input/output error")):
            self.run_for(w, 3)
        self.assertEqual(self.reads, [])
        self.run_for(w, mod.LOOKUP_RETRY + mod.DEBOUNCE + 1)
        self.assertEqual(len(self.reads), 1)

    def test_a_device_that_never_answers_is_asked_a_bounded_number_of_times(self):
        node = self.add_node()
        self.receiver.devices[1]["state"] = "silent"
        w = self.watcher()
        w.step()
        self.listeners[node].queue.append(WS_EVENT)
        self.run_for(w, 10 * mod.LOOKUP_RETRY)
        self.assertEqual(len(self.transports), mod.LOOKUP_TRIES)
        self.assertEqual(self.reads, [])
        self.assertIn("feature lookup", sys_stderr())

    def test_the_wait_ends_when_a_retry_is_due(self):
        node = self.add_node()
        self.receiver.devices[1]["state"] = "silent"
        waits = []

        def wait(listeners, timeout):
            waits.append(timeout)
            return self.wait(listeners, timeout)
        w = mod.Watcher(read=lambda: 0, wait=wait)
        w.step()
        self.listeners[node].queue.append(WS_EVENT)
        w.step()
        w.next_scan = self.clock.t + 3600
        waits.clear()
        w.step()
        self.assertLessEqual(waits[0], mod.LOOKUP_RETRY)

    def test_a_slot_paired_anew_is_looked_up_again(self):
        node = self.add_node()
        self.names()
        w = self.watcher()
        w.step()
        self.listeners[node].queue.append(WS_EVENT)
        self.run_for(w, mod.RESCAN + 1)
        self.assertEqual(len(self.reads), 1)
        self.assertEqual(self.transports, [node])
        # Another device in slot 1, its UNIFIED BATTERY at index 8 this time.
        self.receiver.devices[1] = dev("MX Keys", "MX KEYS", ub=(50, 4, 0), kind=0x01, ub_index=8)
        (self.state / "names").write_text("usb-1/input2\t1\tMX KEYS\tMX Keys\tkeyboard\n")
        self.run_for(w, mod.RESCAN + 1)
        self.listeners[node].queue.append(bytes.fromhex("11 01 08 00 32 04 00").ljust(20, b"\0"))
        self.run_for(w, 3)
        self.assertEqual(self.transports, [node, node])
        self.assertEqual(len(self.reads), 2)

    def test_a_slot_with_the_same_device_is_not_looked_up_again(self):
        node = self.add_node()
        self.names()
        w = self.watcher()
        w.step()
        self.listeners[node].queue.append(WS_EVENT)
        self.run_for(w, 3 * mod.RESCAN)
        self.listeners[node].queue.append(WS_EVENT)
        self.run_for(w, 3)
        self.assertEqual(self.transports, [node])
        self.assertEqual(len(self.reads), 2)

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

    def test_the_hold_off_goes_by_slot_when_two_devices_share_a_name(self):
        self.add_node()
        self.receiver.devices = {1: dev("MX Keys", "MX KEYS", ub=(40, 4, 0), kind=0x01),
                                 2: dev("MX Keys", "MX KEYS", ub=(90, 8, 0), kind=0x01)}
        self.assertEqual(mod.read_batteries(), 0)
        self.receiver.devices[2]["state"] = "off"
        self.assertEqual(mod.read_batteries(), 0)
        self.receiver.devices[2]["state"] = "on"
        node = self.dev / "hidraw0"
        w = self.watcher()
        w.step()
        # Slot 1 has a fresh value and is held off; slot 2 is not reachable and is read.
        self.listeners[node].queue.append(WS_EVENT)
        self.run_for(w, 5)
        self.assertEqual(self.reads, [])
        self.listeners[node].queue.append(bytes.fromhex("11 02 04 00 01 01 01").ljust(20, b"\0"))
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


class UdevRuleTests(unittest.TestCase):
    """The rule that starts the watcher and the code that picks its nodes must
    agree on what a Logitech node is. Neither a real udev nor a boot runs
    here; `udevadm verify` checks the syntax where the host has it."""

    RULES = REPO / "overlay" / "etc" / "udev" / "rules.d" / "90-thinfloe-peripheral-battery.rules"
    UNIT = REPO / "overlay" / "etc" / "systemd" / "system" / "thinfloe-peripheral-battery-watch.service"

    def rule(self):
        lines = [l for l in self.RULES.read_text().splitlines() if l and not l.startswith("#")]
        self.assertEqual(len(lines), 1, lines)
        return lines[0]

    def kernels(self):
        return re.search(r'KERNELS=="([^"]+)"', self.rule()).group(1)

    def test_it_starts_the_watcher_on_an_add_and_on_the_boot_replay(self):
        rule = self.rule()
        self.assertIn('SUBSYSTEM=="hidraw"', rule)
        self.assertIn('TAG+="systemd"', rule)
        self.assertIn('ENV{SYSTEMD_WANTS}+="' + self.UNIT.name + '"', rule)
        # Coldplug replays devices as "add"; a "change" must not lose the wants.
        self.assertIn('ACTION!="remove"', rule)
        self.assertTrue(self.UNIT.exists())
        self.assertNotIn("[Install]", self.UNIT.read_text())

    def test_it_takes_the_vendor_find_hidpp_nodes_takes_on_any_bus(self):
        # The kernel names a HID device %04X:%04X:%04X.%04X of bus, vendor,
        # product and a counter; find_hidpp_nodes reads the same three from
        # HID_ID in the device's uevent.
        tmp = Path(tempfile.mkdtemp(dir=os.environ.get("TMPDIR")))
        sysfs, devdir = tmp / "sys", tmp / "dev"
        cases = {"hidraw0": ("0003", "046D", "C548", True),    # Bolt over USB
                 "hidraw1": ("0005", "046D", "B023", True),    # over Bluetooth
                 "hidraw2": ("0003", "17EF", "6047", False)}
        for name, (bus, vendor, product, _) in cases.items():
            fake_hidraw(sysfs, name, f"{bus}:0000{vendor}:0000{product}", "hid-generic", HIDPP_DESC, name)
        found = {node.name for node, _ in mod.find_hidpp_nodes(sysfs, devdir)}
        for name, (bus, vendor, product, want) in cases.items():
            self.assertEqual(fnmatch.fnmatchcase(f"{bus}:{vendor}:{product}.0001", self.kernels()), want, name)
            self.assertEqual(name in found, want, name)

    def test_udevadm_accepts_it(self):
        udevadm = shutil.which("udevadm")
        if udevadm is None or subprocess.run([udevadm, "verify", "--help"], capture_output=True).returncode:
            self.skipTest("no udevadm verify here")
        r = subprocess.run([udevadm, "verify", str(self.RULES)], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)


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
