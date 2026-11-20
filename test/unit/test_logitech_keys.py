#!/usr/bin/env python3
"""Offline tests for /usr/libexec/thinfloe/logitech-keys (divert and key core).

The program has no .py extension, so it is loaded through importlib like the
battery reader in test_peripheral_battery.py. FakeSlots answers the HID++
requests of a Bolt receiver's slots: the root feature and REPROG_CONTROLS_V4
(0x1B04) functions 0 to 3.
"""

import importlib.machinery
import importlib.util
import os
import struct
import unittest
from pathlib import Path
from unittest import mock

REPO = Path(__file__).resolve().parents[2]
LIBEXEC = REPO / "overlay" / "usr" / "libexec" / "thinfloe"
os.environ["TF_PERIPHERAL_BATTERY"] = str(LIBEXEC / "peripheral-battery")


def load():
    loader = importlib.machinery.SourceFileLoader("logitech_keys", str(LIBEXEC / "logitech-keys"))
    spec = importlib.util.spec_from_loader("logitech_keys", loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


mod = load()


class Clock:
    t = 100.0

    def __call__(self):
        return self.t


class FakeSlots:
    """slots: {slot: (index of 0x1B04 or None, [(cid, flags), ...])}.
    swids: the SWIDs of the replies to every request, sent in this order."""

    def __init__(self, slots, swids=(0x0B,)):
        self.slots = slots
        self.swids = list(swids)
        self.clock = Clock()
        self.pending = []
        self.written = []

    def write(self, data):
        data = bytes(data)
        self.written.append(data)
        self.pending = [r for r in (self._answer(data, s) for s in self.swids) if r]

    def read(self, timeout):
        if not self.pending:
            self.clock.t += timeout
            return None
        return self.pending.pop(0)

    def _answer(self, msg, swid):
        slot, feat, fsw = msg[1], msg[2], msg[3]
        func = fsw >> 4
        entry = self.slots.get(slot)
        if entry is None:
            return None
        idx, cids = entry
        head = bytes([0x11, slot, feat, (func << 4) | swid])
        if feat == 0 and func == 0:
            fid = (msg[4] << 8) | msg[5]
            return (head + bytes([idx if fid == 0x1B04 and idx else 0])).ljust(20, b"\0")
        if feat != idx:
            return None
        if func == 0:
            return (head + bytes([len(cids)])).ljust(20, b"\0")
        if func == 1:
            cid, flags = cids[msg[4]]
            return (head + struct.pack(">HHB", cid, 0x00C7, flags)).ljust(20, b"\0")
        if func == 3:
            return (head + msg[4:7]).ljust(20, b"\0")
        return None


def fake(slots, swids=(0x0B,)):
    f = FakeSlots(slots, swids)
    mod.pb.monotonic = f.clock
    return f


class FindMic(unittest.TestCase):
    def test_find_mic_on_keyboard(self):
        t = fake({1: (8, [(0x00C7, 0x3A), (0x011C, 0x3A)])})
        self.assertEqual(mod.find_mic(t, 1, 1e9), 8)

    def test_find_mic_skips_mouse(self):
        t = fake({2: (10, [(0x0050, 0x3A), (0x00C4, 0x3A)])})
        self.assertIsNone(mod.find_mic(t, 2, 1e9))

    def test_find_mic_not_divertable(self):
        t = fake({1: (8, [(0x011C, 0x04)])})
        self.assertIsNone(mod.find_mic(t, 1, 1e9))

    def test_find_mic_no_feature(self):
        t = fake({1: (None, [])})
        self.assertIsNone(mod.find_mic(t, 1, 1e9))

    def test_find_mic_silent_slot(self):
        t = fake({})
        self.assertIsNone(mod.find_mic(t, 3, 1e9))


class SetDivert(unittest.TestCase):
    def test_set_divert_bytes(self):
        t = fake({1: (8, [(0x011C, 0x3A)])})
        self.assertTrue(mod.set_divert(t, 1, 8, True, 1e9))
        self.assertEqual(t.written[-1], bytes.fromhex("1101083b011c03").ljust(20, b"\0"))
        self.assertTrue(mod.set_divert(t, 1, 8, False, 1e9))
        self.assertEqual(t.written[-1], bytes.fromhex("1101083b011c02").ljust(20, b"\0"))

    def test_reply_with_other_swid_ignored(self):
        t = fake({1: (8, [(0x011C, 0x3A)])}, swids=(0x0A, 0x0B))
        self.assertTrue(mod.set_divert(t, 1, 8, True, 1e9))
        self.assertEqual(len(t.written), 1)

    def test_no_reply_is_false(self):
        t = fake({})
        self.assertFalse(mod.set_divert(t, 1, 8, True, 1e9))


class MicDown(unittest.TestCase):
    def rep(self, hexs):
        return bytes.fromhex(hexs).ljust(20, b"\0")

    def test_mic_down(self):
        self.assertIs(mod.mic_down(self.rep("1101080001 1c".replace(" ", "")), 8), True)
        self.assertIs(mod.mic_down(self.rep("110108000000"), 8), False)
        self.assertIs(mod.mic_down(self.rep("11010800 00c7011c".replace(" ", "")), 8), True)
        self.assertIsNone(mod.mic_down(self.rep("1101080001 1c".replace(" ", "")), 7))
        self.assertIsNone(mod.mic_down(self.rep("1101083b0000"), 8))


class UinputEvents(unittest.TestCase):
    def test_uinput_events(self):
        calls, writes = [], []
        with mock.patch.object(mod.os, "open", return_value=42) as op, \
                mock.patch.object(mod.fcntl, "ioctl", side_effect=lambda fd, req, arg=0: calls.append((fd, req, arg))), \
                mock.patch.object(mod.os, "write", side_effect=lambda fd, d: writes.append((fd, d))), \
                mock.patch.object(mod.os, "close") as cl:
            u = mod.Uinput("/dev/fake")
            self.assertEqual(op.call_args[0][0], "/dev/fake")
            self.assertEqual([c[1] for c in calls],
                             [0x40045564, 0x40045565, 0x405C5503, 0x5501])
            self.assertEqual(calls[0][2], 1)
            self.assertEqual(calls[1][2], 190)
            self.assertIn(b"ThinFloe Logitech keys", calls[2][2])
            u.key(190, True)
            self.assertEqual(len(writes), 2)
            size = struct.calcsize("llHHi")
            first = struct.unpack("llHHi", writes[0][1])
            second = struct.unpack("llHHi", writes[1][1])
            self.assertEqual(len(writes[0][1]), size)
            self.assertEqual(first[2:], (1, 190, 1))
            self.assertEqual(second[2:], (0, 0, 0))
            u.close()
            self.assertEqual(calls[-1][1], 0x5502)
            cl.assert_called_once_with(42)


if __name__ == "__main__":
    unittest.main()
