#!/usr/bin/env python3
"""Offline tests for thinfloe-network status and scan.

thinfloe-network has no .py extension, so it is loaded through importlib like
thinfloe-addon. Importing it does nothing. ip, iw and wg are fake scripts
that record their calls; /sys/class/net is a directory built here.
"""

import contextlib
import importlib.machinery
import importlib.util
import io
import json
import os
import stat
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "overlay" / "usr" / "bin" / "thinfloe-network"
FIX = Path(__file__).resolve().parent / "network-fixtures"


def load_tool():
    # The tools sit in overlay/, which ships; no bytecode may land there.
    sys.dont_write_bytecode = True
    loader = importlib.machinery.SourceFileLoader("thinfloe_network", str(SCRIPT))
    spec = importlib.util.spec_from_loader("thinfloe_network", loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


nw = load_tool()


def fixture(name: str) -> str:
    return (FIX / name).read_text(encoding="utf-8")


def make_sys(root: Path) -> None:
    """The /sys/class/net of the fixture: eth0, eth1 and eth2 are PCI/USB
    devices, wlan0 is wireless, wg0 and br0 are virtual."""
    for name, parts in (("lo", {"type": "772"}), ("br0", {"type": "1"}),
                        ("wg0", {"type": "65534"}),
                        ("eth0", {"type": "1", "device": None}),
                        ("eth1", {"type": "1", "device": None}),
                        ("eth2", {"type": "1", "device": None}),
                        ("wlan0", {"type": "1", "device": None, "wireless": None})):
        d = root / name
        d.mkdir(parents=True)
        for entry, value in parts.items():
            if value is None:
                (d / entry).mkdir()
            else:
                (d / entry).write_text(value + "\n")


def by_name(interfaces):
    return {i["name"]: i for i in interfaces}


class ParseTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="tf-network-")
        self.addCleanup(self.tmp.cleanup)
        self.sys = Path(self.tmp.name) / "sys"
        make_sys(self.sys)

    def test_parse_ip_kinds_and_states(self):
        result = nw.parse_ip(fixture("ip-link.json"), fixture("ip-addr.json"), str(self.sys))
        self.assertEqual([i["name"] for i in result], ["eth0", "eth1", "wlan0", "wg0", "eth2"])
        got = by_name(result)
        self.assertEqual(got["eth0"], {
            "name": "eth0", "kind": "wired", "state": "connected",
            "addresses": ["192.168.1.20/24", "192.168.1.21/24"],
            "ssid": None, "signal_dbm": None, "handshake_age_s": None})
        self.assertEqual(got["eth1"]["state"], "no-cable")
        self.assertEqual(got["eth1"]["addresses"], [])
        self.assertEqual(got["eth2"]["state"], "off")
        self.assertEqual(got["wlan0"]["kind"], "wifi")
        self.assertEqual(got["wlan0"]["state"], "not-connected")
        self.assertEqual(got["wlan0"]["addresses"], ["10.0.0.7/16"])
        self.assertEqual(got["wg0"]["kind"], "wireguard")
        self.assertEqual(got["wg0"]["addresses"], ["10.8.0.2/32"])
        self.assertNotIn("lo", got)
        self.assertNotIn("br0", got)

    def test_parse_iw_link(self):
        self.assertEqual(nw.parse_iw_link(fixture("iw-link-connected.txt")), ("Office WiFi", -52))
        self.assertEqual(nw.parse_iw_link(fixture("iw-link-not.txt")), (None, None))

    def test_parse_handshake(self):
        text = fixture("wg-handshakes.txt")
        self.assertEqual(nw.parse_handshake(text, 1731000150), 30)
        self.assertEqual(nw.parse_handshake(text, 1731001000), 880)
        self.assertEqual(nw.parse_handshake("KEY=\t0\n", 1731000000), None)
        self.assertEqual(nw.parse_handshake("", 1731000000), None)

    def test_parse_scan_dedupes_and_sorts(self):
        nets = nw.parse_scan(fixture("iw-scan.txt"))
        names = [n["ssid"] for n in nets]
        self.assertEqual(names, ["Office WiFi", "Guest", "Legacy WPA", "Old WEP", "Café \\ Bar"])
        self.assertEqual(nets[0]["signal_dbm"], -52)
        self.assertEqual(nets[1]["signal_dbm"], -60)  # -60.50 rounds to even
        self.assertEqual(len(names), len(set(names)))

    def test_parse_scan_marks_wep_and_open(self):
        got = {n["ssid"]: n for n in nw.parse_scan(fixture("iw-scan.txt"))}
        self.assertEqual(got["Office WiFi"], {"ssid": "Office WiFi", "signal_dbm": -52,
                                             "secured": True, "supported": True})
        self.assertEqual((got["Guest"]["secured"], got["Guest"]["supported"]), (False, True))
        self.assertEqual((got["Legacy WPA"]["secured"], got["Legacy WPA"]["supported"]), (True, True))
        self.assertEqual((got["Old WEP"]["secured"], got["Old WEP"]["supported"]), (True, False))

    def test_parse_scan_authentication_suites(self):
        def bss(ssid, section, suites):
            return ("BSS 00:11:22:33:44:55(on wlan0)\n\tcapability: ESS Privacy (0x0011)\n"
                    "\tsignal: -50.00 dBm\n\tSSID: %s\n\t%s:\t * Version: 1\n"
                    "\t\t * Group cipher: CCMP\n\t\t * Authentication suites: %s\n"
                    "\t\t * Capabilities: 16-PTKSA-RC (0x000c)\n"
                    "\tExtended capabilities:\n" % (ssid, section, suites))
        text = (bss("Corp", "RSN", "IEEE 802.1X") + bss("Enhanced", "RSN", "OWE")
                + bss("Mixed", "RSN", "IEEE 802.1X PSK") + bss("Three", "RSN", "SAE")
                + bss("FtPsk", "RSN", "FT/PSK") + bss("Sha", "RSN", "PSK/SHA-256")
                + bss("FtSae", "RSN", "FT/SAE") + bss("ExtKey", "RSN", "SAE-EXT-KEY")
                + bss("SaeBoth", "RSN", "SAE SAE-EXT-KEY") + bss("PskFt", "RSN", "PSK FT/PSK")
                + bss("Sha384", "RSN", "PSK/SHA-384")
                + bss("FtCorp", "RSN", "FT/IEEE 802.1X IEEE 802.1X/SHA-256")
                + bss("OldCorp", "WPA", "IEEE 802.1X"))
        got = {n["ssid"]: n for n in nw.parse_scan(text)}
        want = {"Corp": False, "Enhanced": False, "Mixed": True, "Three": True,
                "FtPsk": False, "Sha": True, "FtSae": False, "ExtKey": False,
                "SaeBoth": True, "PskFt": True, "Sha384": False, "FtCorp": False, "OldCorp": False}
        self.assertEqual({k: v["supported"] for k, v in got.items()}, want)
        self.assertTrue(all(v["secured"] for v in got.values()))

    def test_parse_scan_decodes_escapes(self):
        text = fixture("iw-scan.txt")
        names = [n["ssid"] for n in nw.parse_scan(text)]
        self.assertIn("Café \\ Bar", names)
        # The all-NUL and the empty name are hidden networks: dropped.
        self.assertEqual(len(names), 5)

    def test_parse_scan_caps_at_64(self):
        blocks = "".join("BSS 00:00:00:00:%02x:%02x(on wlan0)\n\tcapability: ESS (0x0001)\n"
                         "\tsignal: -%d.00 dBm\n\tSSID: n%d\n" % (i // 256, i % 256, 30 + i % 60, i)
                         for i in range(100))
        self.assertEqual(len(nw.parse_scan(blocks)), 64)


class Env(unittest.TestCase):
    """Fake ip, iw and wg in a temporary directory; calls are recorded."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="tf-network-")
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        self.sys = self.dir / "sys"
        make_sys(self.sys)
        self.calls = self.dir / "calls"
        self.calls.write_text("")
        patcher = mock.patch.multiple(
            nw, IP=str(self.dir / "ip"), IW=str(self.dir / "iw"), WG=str(self.dir / "wg"),
            SYS_NET=str(self.sys), BUSY_WAIT=0.05)
        patcher.start()
        self.addCleanup(patcher.stop)
        # No test line may reach the journal of the host.
        patcher = mock.patch.object(nw, "syslog")
        patcher.start()
        self.addCleanup(patcher.stop)
        env = mock.patch.dict(os.environ, {"TF_NETWORK_ASSUME_ROOT": "1"})
        env.start()
        self.addCleanup(env.stop)

    def script(self, name: str, body: str) -> None:
        path = self.dir / name
        path.write_text("#!/bin/sh\necho \"%s $*\" >> %s\n%s\n" % (name, self.calls, body))
        path.chmod(0o755)

    def recorded(self):
        return self.calls.read_text().splitlines()

    def run_main(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = nw.main(list(argv))
        return rc, out.getvalue(), err.getvalue()


class ScanTests(Env):
    def fake_ip(self, flags: str) -> None:
        self.script("ip", 'case "$*" in\n'
                    '  *"link show dev"*) echo \'[{"ifname":"wlan0","flags":[%s]}]\' ;;\n'
                    'esac' % flags)

    def test_scan_brings_link_up_and_retries_busy(self):
        self.fake_ip('"BROADCAST","MULTICAST"')
        scan = self.dir / "scan.txt"
        scan.write_text(fixture("iw-scan.txt"))
        count = self.dir / "count"
        self.script("iw", 'n=$(cat %s 2>/dev/null || echo 0); echo $((n+1)) > %s\n'
                    'if [ "$n" = 0 ]; then echo "command failed: Device or resource busy (-16)" >&2; exit 240; fi\n'
                    'cat %s' % (count, count, scan))
        rc, out, _ = self.run_main("scan")
        self.assertEqual(rc, 0)
        data = json.loads(out)
        self.assertEqual(data["device"], "wlan0")
        self.assertEqual(data["networks"][0]["ssid"], "Office WiFi")
        calls = self.recorded()
        self.assertIn("ip link set wlan0 up", calls)
        self.assertEqual(calls.count("iw dev wlan0 scan"), 2)
        self.assertLess(calls.index("ip link set wlan0 up"), calls.index("iw dev wlan0 scan"))

    def test_scan_leaves_up_link_alone(self):
        self.fake_ip('"BROADCAST","UP","LOWER_UP"')
        self.script("iw", "cat %s" % (FIX / "iw-scan.txt"))
        rc, _, _ = self.run_main("scan")
        self.assertEqual(rc, 0)
        self.assertNotIn("ip link set wlan0 up", self.recorded())

    def test_scan_gives_up_after_one_retry(self):
        self.fake_ip('"UP"')
        self.script("iw", 'echo "command failed: Device or resource busy (-16)" >&2; exit 240')
        rc, out, _ = self.run_main("scan")
        self.assertEqual(rc, 1)
        self.assertEqual(json.loads(out)["result"], "failed")
        self.assertEqual(self.recorded().count("iw dev wlan0 scan"), 2)

    def test_scan_times_out(self):
        self.fake_ip('"UP"')
        pidfile = self.dir / "pid"
        self.script("iw", "echo $$ > %s\nexec sleep 60" % pidfile)
        started = time.monotonic()
        with mock.patch.dict(os.environ, {"TF_NETWORK_SCAN_TIMEOUT": "1"}):
            rc, out, _ = self.run_main("scan")
        self.assertLess(time.monotonic() - started, 3)
        self.assertEqual(rc, 1)
        self.assertEqual(json.loads(out)["result"], "failed")
        pid = int(pidfile.read_text())
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)

    def test_scan_without_adapter(self):
        os.rename(self.sys / "wlan0" / "wireless", self.sys / "wlan0" / "gone")
        rc, out, _ = self.run_main("scan")
        self.assertEqual(rc, 1)
        self.assertEqual(json.loads(out), {"result": "failed", "reason": "No WiFi adapter found"})


class StatusTests(Env):
    def test_status_lists_interfaces(self):
        self.script("ip", 'case "$*" in\n  *"-d link"*) cat %s ;;\n  *addr*) cat %s ;;\nesac'
                    % (FIX / "ip-link.json", FIX / "ip-addr.json"))
        self.script("iw", "cat %s" % (FIX / "iw-link-connected.txt"))
        self.script("wg", "printf 'KEY=\\t%d\\n' $(( $(date +%s) - 20 ))")
        rc, out, _ = self.run_main("status")
        self.assertEqual(rc, 0)
        self.assertEqual(len(out.splitlines()), 1)
        got = by_name(json.loads(out)["interfaces"])
        self.assertEqual(got["wlan0"]["state"], "connected")
        self.assertEqual(got["wlan0"]["ssid"], "Office WiFi")
        self.assertEqual(got["wlan0"]["signal_dbm"], -52)
        self.assertEqual(got["wg0"]["state"], "connected")
        self.assertIn(got["wg0"]["handshake_age_s"], range(19, 25))

    def test_status_wireguard_stale_handshake(self):
        self.script("ip", 'case "$*" in\n  *"-d link"*) cat %s ;;\n  *addr*) cat %s ;;\nesac'
                    % (FIX / "ip-link.json", FIX / "ip-addr.json"))
        self.script("iw", "cat %s" % (FIX / "iw-link-not.txt"))
        self.script("wg", "printf 'KEY=\\t1731000000\\n'")
        rc, out, _ = self.run_main("status")
        got = by_name(json.loads(out)["interfaces"])
        self.assertEqual(got["wg0"]["state"], "not-connected")
        self.assertGreater(got["wg0"]["handshake_age_s"], 180)
        self.assertEqual(got["wlan0"]["state"], "not-connected")
        self.assertIsNone(got["wlan0"]["ssid"])

    def test_status_steps_time_out_within_the_menu_deadline(self):
        # Ruling 24: a stuck `iw dev link` costs 1.5 s, not 10, so the
        # whole status fits the menu's 5 s and still answers.
        self.script("ip", 'case "$*" in\n  *"-d link"*) cat %s ;;\n  *addr*) cat %s ;;\nesac'
                    % (FIX / "ip-link.json", FIX / "ip-addr.json"))
        self.script("iw", "exec sleep 30")
        self.script("wg", "printf 'KEY=\\t1731000000\\n'")
        started = time.monotonic()
        rc, out, _ = self.run_main("status")
        self.assertLess(time.monotonic() - started, 4)
        self.assertEqual(rc, 0)
        got = by_name(json.loads(out)["interfaces"])
        self.assertIsNone(got["wlan0"]["ssid"])
        self.assertIn("wg0", got)

    def test_status_needs_root(self):
        with mock.patch.dict(os.environ, {"TF_NETWORK_ASSUME_ROOT": ""}), \
                mock.patch.object(nw.os, "geteuid", return_value=1000):
            rc, out, _ = self.run_main("status")
        self.assertEqual(rc, 2)
        self.assertEqual(json.loads(out), {"result": "failed", "reason": "needs root"})

    def test_unknown_subcommand(self):
        rc, out, err = self.run_main("frobnicate")
        self.assertEqual(rc, 2)
        self.assertEqual(json.loads(out)["result"], "failed")
        self.assertIn("usage", err)


WIFI = "network:\n  version: 2\n  wifis:\n    wlan0:\n      access-points:\n        Office: {password: pw}\n"
TEMPLATE_WIFI = "# Put the device name of your wifi card here\nnetwork:\n  version: 2\n"
WIRED_HOME = "network:\n  version: 2\n  ethernets:\n    eth0: {addresses: [10.0.0.2/24]}\n"
WIRED_DEFAULT = "network:\n  version: 2\n  ethernets:\n    all: {dhcp4: true}\n"
ETC_WIFI = "network:\n  version: 2\n  wifis:\n    wlan0:\n      access-points:\n        Old: {password: old}\n"


class ApplyTests(Env):
    """Fake netplan: generate fails while the flag file generate-fails exists,
    apply while apply-fails exists; the files of the check root are copied to
    seen/ so that the tests see what generate was given."""

    def setUp(self):
        super().setUp()
        self.home = self.dir / "home"
        self.etc = self.dir / "etc"
        self.home.mkdir()
        self.etc.mkdir()
        self.default = self.dir / "default-ethernet.yaml"
        self.default.write_text(WIRED_DEFAULT)
        self.script("netplan", 'case "$1" in\n'
                    'generate) rm -rf %(d)s/seen; cp -r "$3/etc/netplan" %(d)s/seen\n'
                    '  echo "$3" > %(d)s/check-root\n'
                    '  if [ -e %(d)s/generate-fails ]; then\n'
                    '    echo "Invalid YAML at wifi.yaml line 3" >&2\n'
                    '    echo "Failed to reload systemctl daemon-reload" >&2; exit 1; fi\n'
                    '  echo "Failed to connect to bus: systemctl daemon-reload" >&2 ;;\n'
                    'apply) if [ -e %(d)s/apply-fails ]; then echo "apply broke" >&2; exit 1; fi ;;\n'
                    'esac' % {"d": self.dir})
        patcher = mock.patch.multiple(
            nw, HOME_DIR=str(self.home), ETC_NETPLAN=str(self.etc),
            DEFAULT_WIRED=str(self.default), NETPLAN=str(self.dir / "netplan"),
            LOCK=str(self.dir / "run" / "network.lock"), WORK_DIR=str(self.dir / "work"),
            is_netboot=lambda: False)
        patcher.start()
        self.addCleanup(patcher.stop)

    def snapshot(self):
        return {p.name: p.read_bytes() for p in sorted(self.etc.iterdir())}

    def netplan_calls(self):
        return [c for c in self.recorded() if c.startswith("netplan")]

    def test_apply_installs_home_files_mode_600(self):
        (self.home / "wifi.yaml").write_text(WIFI)
        (self.home / "ethernet.yaml").write_text(WIRED_HOME)
        (self.etc / "wifi.yaml").write_text(ETC_WIFI)
        rc, out, _ = self.run_main("apply")
        self.assertEqual((rc, json.loads(out)), (0, {"result": "ok", "reason": ""}))
        self.assertEqual((self.etc / "wifi.yaml").read_text(), WIFI)
        self.assertEqual((self.etc / "ethernet.yaml").read_text(), WIRED_HOME)
        for name in ("wifi.yaml", "ethernet.yaml"):
            self.assertEqual(stat.S_IMODE((self.etc / name).stat().st_mode), 0o600)
        self.assertEqual(sorted(self.snapshot()), ["ethernet.yaml", "wifi.yaml"])
        calls = self.netplan_calls()
        self.assertEqual(calls[0].split()[:3], ["netplan", "generate", "--root-dir"])
        self.assertEqual(calls[-1], "netplan apply")
        root = Path((self.dir / "check-root").read_text().strip())
        self.assertFalse(root.exists())
        self.assertEqual(sorted(os.listdir(self.home)), ["ethernet.yaml", "wifi.yaml"])

    def test_apply_template_leaves_etc_wifi(self):
        (self.home / "wifi.yaml").write_text(TEMPLATE_WIFI)
        (self.etc / "wifi.yaml").write_text(ETC_WIFI)
        rc, _, _ = self.run_main("apply")
        self.assertEqual(rc, 0)
        self.assertEqual((self.etc / "wifi.yaml").read_text(), ETC_WIFI)
        # The checked set holds the wifi file that stays in force.
        self.assertEqual((self.dir / "seen" / "wifi.yaml").read_text(), ETC_WIFI)

    def test_apply_missing_home_files_use_defaults(self):
        rc, _, _ = self.run_main("apply")
        self.assertEqual(rc, 0)
        self.assertEqual(self.snapshot(), {"ethernet.yaml": WIRED_DEFAULT.encode()})

    def test_apply_dhcp_installs_default_copy(self):
        # Automatic: the menu removed ~/ethernet.yaml; an old static file is in /etc.
        (self.etc / "ethernet.yaml").write_text(WIRED_HOME)
        rc, _, _ = self.run_main("apply")
        self.assertEqual(rc, 0)
        self.assertEqual((self.etc / "ethernet.yaml").read_text(), WIRED_DEFAULT)
        self.assertEqual(stat.S_IMODE((self.etc / "ethernet.yaml").stat().st_mode), 0o600)

    def test_apply_installs_comment_only_wired_file_as_is(self):
        (self.home / "ethernet.yaml").write_text("# only a comment\n")
        rc, _, _ = self.run_main("apply")
        self.assertEqual(rc, 0)
        self.assertEqual((self.etc / "ethernet.yaml").read_text(), "# only a comment\n")
        self.assertEqual((self.dir / "seen" / "ethernet.yaml").read_text(), "# only a comment\n")

    def test_apply_checks_other_etc_files_too(self):
        (self.etc / "50-cloud.yaml").write_text("network: {version: 2}\n")
        (self.etc / "notes.txt").write_text("x")
        rc, _, _ = self.run_main("apply")
        self.assertEqual(rc, 0)
        self.assertEqual(sorted(os.listdir(self.dir / "seen")), ["50-cloud.yaml", "ethernet.yaml"])

    def test_apply_rejects_bad_home_file(self):
        (self.home / "wifi.yaml").write_text(WIFI)
        (self.etc / "wifi.yaml").write_text(ETC_WIFI)
        (self.etc / "ethernet.yaml").write_text(WIRED_DEFAULT)
        before = self.snapshot()
        (self.dir / "generate-fails").write_text("")
        rc, out, _ = self.run_main("apply")
        self.assertEqual(rc, 1)
        got = json.loads(out)
        self.assertEqual(got["result"], "failed")
        self.assertIn("Invalid YAML", got["reason"])
        self.assertNotIn("systemctl", got["reason"])
        self.assertEqual(self.snapshot(), before)
        self.assertNotIn("netplan apply", self.recorded())
        self.assertFalse(Path((self.dir / "check-root").read_text().strip()).exists())

    def test_apply_refuses_linked_home_file(self):
        target = self.dir / "elsewhere.yaml"
        target.write_text(WIFI)
        os.symlink(target, self.home / "wifi.yaml")
        (self.home / "ethernet.yaml").write_text(WIRED_HOME)
        before = self.snapshot()
        rc, out, _ = self.run_main("apply")
        self.assertEqual(rc, 1)
        self.assertEqual(json.loads(out), {"result": "failed", "reason": "wifi.yaml is a link"})
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(self.netplan_calls(), [])

    def test_apply_refuses_other_than_regular_and_big_files(self):
        os.mkfifo(self.home / "wifi.yaml")
        rc, out, _ = self.run_main("apply")
        self.assertEqual(rc, 1)
        self.assertIn("not a regular file", json.loads(out)["reason"])
        os.unlink(self.home / "wifi.yaml")
        (self.home / "wifi.yaml").write_text("#" * (64 * 1024) + "x")
        rc, out, _ = self.run_main("apply")
        self.assertEqual(rc, 1)
        self.assertIn("64 KiB", json.loads(out)["reason"])
        self.assertEqual(self.snapshot(), {})
        self.assertEqual(self.netplan_calls(), [])

    def test_apply_netboot_ignores_home_wired(self):
        (self.home / "ethernet.yaml").write_text(WIRED_HOME)
        with mock.patch.object(nw, "is_netboot", lambda: True):
            rc, _, _ = self.run_main("apply")
        self.assertEqual(rc, 0)
        self.assertEqual((self.etc / "ethernet.yaml").read_text(), WIRED_DEFAULT)

    def test_apply_reports_netplan_apply_failure(self):
        (self.home / "ethernet.yaml").write_text(WIRED_HOME)
        (self.dir / "apply-fails").write_text("")
        rc, out, _ = self.run_main("apply")
        self.assertEqual(rc, 1)
        got = json.loads(out)
        self.assertEqual(got["result"], "failed")
        self.assertIn("apply broke", got["reason"])
        # The files stay: they are what the next boot installs.
        self.assertEqual((self.etc / "ethernet.yaml").read_text(), WIRED_HOME)
        self.assertEqual(sorted(os.listdir(self.etc)), ["ethernet.yaml"])

    def test_apply_apply_timeout_kills_netplan(self):
        self.script("netplan", 'case "$1" in apply) echo $$ > %s; exec sleep 60 ;; esac' % (self.dir / "pid"))
        started = time.monotonic()
        with mock.patch.object(nw, "APPLY_TIMEOUT", 1):
            rc, out, _ = self.run_main("apply")
        self.assertLess(time.monotonic() - started, 4)
        self.assertEqual(rc, 1)
        self.assertIn("too long", json.loads(out)["reason"])
        with self.assertRaises(ProcessLookupError):
            os.kill(int((self.dir / "pid").read_text()), 0)

    def test_apply_waits_for_the_lock(self):
        import fcntl
        (self.dir / "run").mkdir()
        fd = os.open(self.dir / "run" / "network.lock", os.O_RDWR | os.O_CREAT, 0o600)
        self.addCleanup(os.close, fd)
        fcntl.flock(fd, fcntl.LOCK_EX)
        with mock.patch.object(nw, "LOCK_WAIT", 0.3):
            rc, out, _ = self.run_main("apply")
        self.assertEqual(rc, 1)
        self.assertIn("Another", json.loads(out)["reason"])
        self.assertEqual(self.netplan_calls(), [])

    def test_apply_needs_root(self):
        with mock.patch.dict(os.environ, {"TF_NETWORK_ASSUME_ROOT": ""}), \
                mock.patch.object(nw.os, "geteuid", return_value=1000):
            rc, out, _ = self.run_main("apply")
        self.assertEqual(rc, 2)
        self.assertEqual(json.loads(out), {"result": "failed", "reason": "needs root"})
        self.assertEqual(self.netplan_calls(), [])


class NetbootTests(unittest.TestCase):
    def test_is_netboot_follows_the_mount_point(self):
        with mock.patch.object(nw, "ESP_DIR", "/"):
            self.assertFalse(nw.is_netboot())
        with tempfile.TemporaryDirectory(prefix="tf-network-") as d, \
                mock.patch.object(nw, "ESP_DIR", d):
            self.assertTrue(nw.is_netboot())


class ImportTests(unittest.TestCase):
    def test_file_is_executable(self):
        self.assertTrue(SCRIPT.stat().st_mode & stat.S_IXUSR)


if __name__ == "__main__":
    unittest.main()
