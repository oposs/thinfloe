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
import io
import struct
import tempfile
import time
import unittest
from contextlib import redirect_stderr
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

    def __init__(self, slots, swids=(0x0B,), ws=None):
        self.slots = slots
        self.ws = ws or {}              # {slot: index of 0x1D4B}
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

    def close(self):
        pass

    def _answer(self, msg, swid):
        if msg[0] == 0x10 and msg[1] == 0xFF:       # receiver register b5/50+n
            slot = msg[4] - 0x50
            if slot in self.slots:
                return bytes([0x10, 0xFF, 0x83, 0xB5, msg[4], 0x00, 0x00])
            return bytes([0x10, 0xFF, 0x8F, 0x83, 0xB5, 0x08, 0x00])
        slot, feat, fsw = msg[1], msg[2], msg[3]
        func = fsw >> 4
        entry = self.slots.get(slot)
        if entry is None:
            return None
        idx, cids = entry
        head = bytes([0x11, slot, feat, (func << 4) | swid])
        if feat == 0 and func == 0:
            fid = (msg[4] << 8) | msg[5]
            found = {0x1B04: idx, 0x1D4B: self.ws.get(slot)}.get(fid)
            return (head + bytes([found or 0])).ljust(20, b"\0")
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


class Base(unittest.TestCase):
    def fake(self, slots, swids=(0x0B,), ws=None):
        f = FakeSlots(slots, swids, ws)
        patcher = mock.patch.object(mod.pb, "monotonic", f.clock)
        patcher.start()
        self.addCleanup(patcher.stop)
        return f


class FindMic(Base):
    def test_find_mic_on_keyboard(self):
        t = self.fake({1: (8, [(0x00C7, 0x3A), (0x011C, 0x3A)])})
        self.assertEqual(mod.find_mic(t, 1, 1e9), 8)

    def test_find_mic_skips_mouse(self):
        t = self.fake({2: (10, [(0x0050, 0x3A), (0x00C4, 0x3A)])})
        self.assertIsNone(mod.find_mic(t, 2, 1e9))

    def test_find_mic_not_divertable(self):
        t = self.fake({1: (8, [(0x011C, 0x04)])})
        self.assertIsNone(mod.find_mic(t, 1, 1e9))

    def test_find_mic_no_feature(self):
        t = self.fake({1: (None, [])})
        self.assertIsNone(mod.find_mic(t, 1, 1e9))

    def test_find_mic_silent_slot(self):
        t = self.fake({})
        self.assertIsNone(mod.find_mic(t, 3, 1e9))

    def test_failed_reply_mid_scan_is_none(self):
        t = self.fake({1: (8, [(0x00C7, 0x3A), (0x011C, 0x3A)])})
        real = t._answer
        t._answer = lambda msg, swid: None if msg[2] == 8 and msg[3] >> 4 == 1 and msg[4] == 1 \
            else real(msg, swid)
        self.assertIsNone(mod.find_mic(t, 1, 1e9))


class SetDivert(Base):
    def test_set_divert_bytes(self):
        t = self.fake({1: (8, [(0x011C, 0x3A)])})
        self.assertTrue(mod.set_divert(t, 1, 8, True, 1e9))
        self.assertEqual(t.written[-1], bytes.fromhex("1101083b011c03").ljust(20, b"\0"))
        self.assertTrue(mod.set_divert(t, 1, 8, False, 1e9))
        self.assertEqual(t.written[-1], bytes.fromhex("1101083b011c02").ljust(20, b"\0"))

    def test_reply_with_other_swid_ignored(self):
        t = self.fake({1: (8, [(0x011C, 0x3A)])}, swids=(0x0A, 0x0B))
        self.assertTrue(mod.set_divert(t, 1, 8, True, 1e9))
        self.assertEqual(len(t.written), 1)

    def test_no_reply_is_false(self):
        t = self.fake({})
        self.assertFalse(mod.set_divert(t, 1, 8, True, 1e9))


class MicDown(unittest.TestCase):
    def rep(self, hexs):
        return bytes.fromhex(hexs).ljust(20, b"\0")

    def test_mic_down(self):
        self.assertIs(mod.mic_down(self.rep("11010800011c"), 1, 8), True)
        self.assertIs(mod.mic_down(self.rep("110108000000"), 1, 8), False)
        self.assertIs(mod.mic_down(self.rep("1101080000c7011c"), 1, 8), True)
        self.assertIsNone(mod.mic_down(self.rep("11010800011c"), 1, 7))
        self.assertIsNone(mod.mic_down(self.rep("1101083b0000"), 1, 8))

    def test_other_slot_does_not_count(self):
        self.assertIsNone(mod.mic_down(self.rep("11020800011c"), 1, 8))
        self.assertIs(mod.mic_down(self.rep("11020800011c"), 2, 8), True)


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


KBD = (8, [(0x00C7, 0x3A), (0x011C, 0x3A)])
MOUSE = (10, [(0x0050, 0x3A), (0x00C4, 0x3A)])
NODE = Path("/dev/hidraw0")


class FakeUinput:
    def __init__(self):
        self.calls = []
        self.fail = False
        self.closed = False

    def key(self, code, down):
        if self.fail:
            raise OSError(11, "Resource temporarily unavailable")
        self.calls.append((code, down))

    def close(self):
        self.closed = True


class FakeListener:
    def __init__(self):
        self.queue = []
        self.closed = False

    def read(self):
        item = self.queue.pop(0)
        if isinstance(item, Exception):
            raise item
        return item

    def close(self):
        self.closed = True


def rep(hexs):
    return bytes.fromhex(hexs).ljust(20, b"\0")


DOWN1 = rep("11010800011c")
UP1 = rep("110108000000")
DOWN2 = rep("11020800011c")
UP2 = rep("110208000000")
RECONNECT1 = rep("1101040001")


class KeysBase(Base):
    def setUp(self):
        self.uin = FakeUinput()
        self.lst = FakeListener()
        self.open_locks = set()
        self.lock_count = 0
        self.nodes = [(NODE, "phys1")]
        self.busy = False
        real_close = os.close

        def close(fd):
            self.open_locks.discard(fd)
            real_close(fd)
        patcher = mock.patch.object(mod.os, "close", close)
        patcher.start()
        self.addCleanup(patcher.stop)

    def lock(self, state):
        if self.busy:
            return None
        fd = os.open(os.devnull, os.O_RDONLY)
        self.open_locks.add(fd)
        self.lock_count += 1
        return fd

    def lock_is_open(self):
        return bool(self.open_locks)

    def keys(self, slots, ws=None):
        self.f = self.fake(slots, ws=ws if ws is not None else {s: 4 for s in slots})
        real_write = self.f.write

        def write(data):
            self.assertTrue(self.lock_is_open(), "request outside the lock")
            real_write(data)
        self.f.write = write
        self.addCleanup(lambda: [self._close(fd) for fd in list(self.open_locks)])
        return mod.Keys(uinput=self.uin, take_lock=self.lock,
                        find_nodes=lambda: list(self.nodes),
                        open_listener=lambda node: self.lst,
                        open_transport=lambda node: self.f,
                        wait=lambda ls, timeout: [l for l in ls if l.queue])

    @staticmethod
    def _close(fd):
        try:
            os.close(fd)
        except OSError:
            pass

    def diverts(self):
        """(slot, params byte) of every setCidReporting request."""
        return [(w[1], w[6]) for w in self.f.written
                if w[2] in (8, 10) and w[3] >> 4 == 3]


class KeysTests(KeysBase):
    def test_rescan_diverts_keyboard_only(self):
        k = self.keys({1: KBD, 2: MOUSE})
        k.rescan()
        self.assertEqual(self.diverts(), [(1, 0x03)])

    def test_requests_take_the_lock(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.assertTrue(self.f.written)
        self.assertFalse(self.lock_is_open(), "lock fd must be closed after the requests")
        self.lst.queue.append(RECONNECT1)
        k.step(0)
        self.assertEqual(len(self.diverts()), 2)
        self.assertFalse(self.lock_is_open())

    def test_press_release_gives_f20(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.lst.queue += [DOWN1, DOWN1, UP1, UP1]
        for _ in range(4):
            k.step(0)
        self.assertEqual(self.uin.calls, [(190, True), (190, False)])

    def test_reconnect_diverts_again(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.lst.queue.append(RECONNECT1)
        k.step(0)
        self.assertEqual(self.diverts(), [(1, 0x03), (1, 0x03)])

    def test_other_status_does_not_divert_again(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.lst.queue.append(rep("1101040002"))
        k.step(0)
        self.assertEqual(len(self.diverts()), 1)

    def test_rescan_does_not_divert_a_recorded_slot_again(self):
        k = self.keys({1: KBD})
        k.rescan()
        k.rescan()
        self.assertEqual(len(self.diverts()), 1)

    def test_failed_slot_is_tried_again_by_rescan(self):
        k = self.keys({1: KBD})
        real = self.silence(lambda m: m[0] == 0x11)
        k.rescan()
        self.assertEqual(self.diverts(), [])
        self.f._answer = real
        k.rescan()
        self.assertEqual(self.diverts(), [(1, 0x03)])

    def test_busy_lock_skips_and_retries(self):
        k = self.keys({1: KBD})
        self.busy = True
        buf = io.StringIO()
        with redirect_stderr(buf):
            k.rescan()
            k.rescan()
        self.assertEqual(self.diverts(), [])
        self.assertEqual(buf.getvalue().count("lock busy"), 1)
        self.busy = False
        k.rescan()
        self.assertEqual(self.diverts(), [(1, 0x03)])

    def silence(self, what):
        """Make the fake stop answering the requests for which what(msg) holds."""
        real = self.f._answer
        self.f._answer = lambda msg, swid: None if what(msg) else real(msg, swid)
        return real

    def test_failed_wake_divert_is_retried_by_rescan(self):
        k = self.keys({1: KBD})
        k.rescan()
        real = self.silence(lambda m: m[0] == 0x11 and m[3] >> 4 == 3)
        self.lst.queue.append(RECONNECT1)
        with redirect_stderr(io.StringIO()):
            k.step(0)
        self.assertEqual(len(self.diverts()), 2)
        self.f._answer = real
        k.rescan()
        self.assertEqual(len(self.diverts()), 3)

    def test_wake_divert_without_lock_is_retried_by_rescan(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.busy = True
        self.lst.queue.append(RECONNECT1)
        with redirect_stderr(io.StringIO()):
            k.step(0)
        self.busy = False
        k.rescan()
        self.assertEqual(self.diverts(), [(1, 0x03), (1, 0x03)])

    def test_slot_without_mic_is_not_probed_again(self):
        k = self.keys({1: KBD, 2: MOUSE})
        k.rescan()
        before = len(self.f.written)
        k.rescan()
        asked = [w for w in self.f.written[before:] if w[0] == 0x11]
        self.assertEqual(asked, [])

    def test_silent_slot_is_probed_again(self):
        k = self.keys({1: KBD})
        self.silence(lambda m: m[0] == 0x11)
        k.rescan()
        before = len(self.f.written)
        k.rescan()
        self.assertTrue([w for w in self.f.written[before:] if w[0] == 0x11])

    def test_dropped_node_forgets_no_mic_slots(self):
        k = self.keys({2: MOUSE})
        k.rescan()
        self.nodes = []
        k.rescan()
        self.nodes = [(NODE, "phys1")]
        before = len(self.f.written)
        k.rescan()
        self.assertTrue([w for w in self.f.written[before:] if w[0] == 0x11])

    def test_report_of_an_unrecorded_slot_probes_it_at_once(self):
        k = self.keys({1: KBD})
        k.next_scan = self.f.clock.t + 1000
        real = self.silence(lambda m: m[0] == 0x11)
        k.rescan()
        self.assertEqual(self.diverts(), [])
        self.f._answer = real
        self.lst.queue.append(rep("1101090000"))     # any report of the waking keyboard
        k.step(0)
        self.assertEqual(self.diverts(), [(1, 0x03)])

    def test_probe_by_report_is_limited_to_one_per_5_seconds(self):
        k = self.keys({1: KBD})
        k.next_scan = self.f.clock.t + 1000
        self.silence(lambda m: m[0] == 0x11)

        def probes():
            return len([w for w in self.f.written if w[0] == 0x11 and w[2] == 0])
        k.rescan()
        base = probes()
        for _ in range(3):
            self.lst.queue.append(rep("1101090000"))
            k.step(0)
        self.assertEqual(probes(), base + 1)
        self.f.clock.t += 5
        self.lst.queue.append(rep("1101090000"))
        k.step(0)
        self.assertEqual(probes(), base + 2)

    def test_report_of_a_mouse_slot_is_not_probed(self):
        k = self.keys({2: MOUSE})
        k.next_scan = self.f.clock.t + 1000
        k.rescan()
        before = len(self.f.written)
        self.lst.queue.append(rep("1102090000"))
        k.step(0)
        self.assertEqual(len(self.f.written), before)

    def test_warning_comes_again_after_recovery(self):
        k = self.keys({1: KBD})
        buf = io.StringIO()
        with redirect_stderr(buf):
            self.busy = True
            k.rescan()
            self.busy = False
            k.rescan()
            self.busy = True
            self.lst.queue.append(RECONNECT1)
            k.step(0)
        self.assertEqual(buf.getvalue().count("lock busy"), 2)
        self.assertIn("thinfloe: logitech-keys: ", buf.getvalue())
        self.assertNotIn("battery", buf.getvalue())

    def test_stop_logs_undivert_without_answer(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.silence(lambda m: m[0] == 0x11 and m[3] >> 4 == 3)
        buf = io.StringIO()
        with redirect_stderr(buf):
            k.stop()
        self.assertIn("no answer to the undivert request", buf.getvalue())
        self.assertTrue(self.uin.closed)

    def test_lost_ws_reply_does_not_record_the_slot(self):
        k = self.keys({1: KBD})
        real = self.silence(lambda m: m[0] == 0x11 and m[2] == 0 and m[4:6] == b"\x1d\x4b")
        k.rescan()
        self.assertEqual(self.diverts(), [])
        self.f._answer = real
        k.rescan()
        self.assertEqual(self.diverts(), [(1, 0x03)])
        self.lst.queue.append(RECONNECT1)     # the recorded wake index works
        k.step(0)
        self.assertEqual(len(self.diverts()), 2)

    def test_slot_without_wake_feature_is_still_recorded(self):
        k = self.keys({1: KBD}, ws={})
        k.rescan()
        self.assertEqual(self.diverts(), [(1, 0x03)])

    def test_new_node_is_found_within_5_seconds(self):
        k = self.keys({1: KBD})
        self.nodes = []
        k.step(0)
        self.nodes = [(NODE, "phys1")]
        self.f.clock.t += 5
        k.step(0)
        self.assertEqual(self.diverts(), [(1, 0x03)])

    def test_slots_of_a_known_node_are_probed_every_30_seconds(self):
        k = self.keys({1: KBD})
        self.silence(lambda m: m[0] == 0x11)
        k.step(0)

        def asked():
            return len([w for w in self.f.written if w[0] == 0x11])
        base = asked()
        for _ in range(5):                    # 5 s steps up to 25 s
            self.f.clock.t += 5
            k.step(0)
        self.assertEqual(asked(), base)
        self.f.clock.t += 5
        k.step(0)
        self.assertGreater(asked(), base)

    def test_wake_with_busy_lock_logs_only_the_lock(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.busy = True
        self.lst.queue.append(RECONNECT1)
        buf = io.StringIO()
        with redirect_stderr(buf):
            k.step(0)
        self.assertEqual(buf.getvalue().count("lock busy"), 1, buf.getvalue())
        self.assertNotIn("no answer", buf.getvalue())

    def test_node_gone_releases_key(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.lst.queue += [DOWN1, OSError(19, "No such device")]
        buf = io.StringIO()
        with redirect_stderr(buf):
            k.step(0)
            k.step(0)
        self.assertEqual(self.uin.calls, [(190, True), (190, False)])
        self.assertTrue(self.lst.closed)
        self.assertEqual(buf.getvalue().count("No such device"), 1)
        k.step(0)           # no node left, no exception
        before = len(self.f.written)
        self.f.clock.t += 60
        k.rescan()          # the node is found again and diverted again
        self.assertGreater(len(self.f.written), before)
        self.assertEqual(len(self.diverts()), 2)

    def test_vanished_node_is_dropped_by_rescan(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.lst.queue.append(DOWN1)
        k.step(0)
        self.nodes = []
        k.rescan()
        self.assertTrue(self.lst.closed)
        self.assertEqual(self.uin.calls, [(190, True), (190, False)])

    def test_two_keyboards_share_one_key(self):
        k = self.keys({1: KBD, 2: KBD})
        k.rescan()
        self.assertEqual(self.diverts(), [(1, 0x03), (2, 0x03)])
        for r in (DOWN2, DOWN1, UP2):
            self.lst.queue.append(r)
            k.step(0)
        self.assertEqual(self.uin.calls, [(190, True)])
        self.lst.queue.append(UP1)
        k.step(0)
        self.assertEqual(self.uin.calls, [(190, True), (190, False)])

    def test_unrecorded_slot_report_is_ignored(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.lst.queue.append(DOWN2)
        k.step(0)
        self.assertEqual(self.uin.calls, [])

    def test_reconnect_clears_a_held_slot(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.lst.queue += [DOWN1, RECONNECT1]
        k.step(0)
        k.step(0)
        self.assertEqual(self.uin.calls, [(190, True), (190, False)])

    def test_uinput_error_is_logged_once_and_loop_goes_on(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.uin.fail = True
        self.lst.queue += [DOWN1, UP1, DOWN1]
        buf = io.StringIO()
        with redirect_stderr(buf):
            for _ in range(3):
                k.step(0)
        self.assertEqual(buf.getvalue().count("Resource temporarily unavailable"), 1)
        self.uin.fail = False
        self.lst.queue.append(UP1)
        k.step(0)
        self.assertEqual(self.uin.calls, [(190, False)])

    def test_stop_undiverts_and_releases(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.lst.queue.append(DOWN1)
        k.step(0)
        k.stop()
        self.assertEqual(self.diverts(), [(1, 0x03), (1, 0x02)])
        self.assertEqual(self.uin.calls, [(190, True), (190, False)])
        self.assertTrue(self.uin.closed)
        self.assertTrue(self.lst.closed)
        self.assertFalse(self.lock_is_open())

    def test_stop_with_vanished_node(self):
        k = self.keys({1: KBD})
        k.rescan()
        self.lst.queue.append(DOWN1)
        k.step(0)

        def gone(node):
            raise OSError(19, "No such device")
        k.open_transport = gone
        buf = io.StringIO()
        with redirect_stderr(buf):
            k.stop()
        self.assertEqual(self.uin.calls, [(190, True), (190, False)])
        self.assertTrue(self.uin.closed)
        self.assertIn("No such device", buf.getvalue())


class TakeLock(unittest.TestCase):
    def setUp(self):
        td = tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR"))
        self.addCleanup(td.cleanup)
        self.state = Path(td.name)

    def test_free_lock_is_taken(self):
        fd = mod.take_lock(self.state)
        self.assertIsNotNone(fd)
        os.close(fd)

    def test_held_lock_gives_up_after_the_wait(self):
        other = os.open(str(self.state / "lock"), os.O_RDWR | os.O_CREAT, 0o600)
        self.addCleanup(os.close, other)
        mod.fcntl.flock(other, mod.fcntl.LOCK_EX)
        buf = io.StringIO()
        start = time.monotonic()
        with mock.patch.object(mod, "LOCK_WAIT", 0.3), redirect_stderr(buf):
            self.assertIsNone(mod.take_lock(self.state))
        self.assertLess(time.monotonic() - start, 2.0)
        self.assertEqual(buf.getvalue(), "")    # the caller logs a busy lock

    def test_busy_lock_is_one_journal_line_over_two_rescans(self):
        other = os.open(str(self.state / "lock"), os.O_RDWR | os.O_CREAT, 0o600)
        self.addCleanup(os.close, other)
        mod.fcntl.flock(other, mod.fcntl.LOCK_EX)
        k = mod.Keys(uinput=mock.Mock(), take_lock=mod.take_lock,
                     find_nodes=lambda: [(Path("/dev/hidraw9"), "phys1")],
                     open_listener=lambda node: mock.Mock(),
                     open_transport=lambda node: mock.Mock(),
                     wait=lambda ls, timeout: [])
        buf = io.StringIO()
        with mock.patch.object(mod, "LOCK_WAIT", 0.1), \
                mock.patch.object(mod.pb, "_paths", lambda: (None, None, None, self.state)), \
                redirect_stderr(buf):
            k.rescan()
            k.rescan()
        self.assertEqual(buf.getvalue().count("lock busy"), 1, buf.getvalue())


class Main(unittest.TestCase):
    def test_main_rejects_arguments(self):
        with redirect_stderr(io.StringIO()):
            self.assertEqual(mod.main(["x"]), 2)

    def test_main_exits_1_without_uinput(self):
        with mock.patch.object(mod, "Uinput", side_effect=OSError(2, "No such file")), \
                redirect_stderr(io.StringIO()) as err:
            self.assertEqual(mod.main([]), 1)
        self.assertIn("No such file", err.getvalue())


if __name__ == "__main__":
    unittest.main()
