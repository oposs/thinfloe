#!/usr/bin/env python3
"""Offline tests for thinfloe-addon: reading add-on definitions, checking
their sources, storing payloads, installing and removing them.

thinfloe-addon has no .py extension, so it is loaded through importlib like
thinfloe-upgrade in test_upgrade_image.py. Its entry point is guarded, so
importing it does nothing.

Every web source is served by an http.server bound to 127.0.0.1 in a thread.
dpkg and apt are the real ones of the dev host; the APT check runs
unprivileged against a flat repository built here and never touches the
system's package lists. Installing runs unprivileged: dpkg is a fake script
that records its arguments, tar installs into a temporary root, chown is
patched where it is asserted.
"""

import contextlib
import fcntl
import http.server
import importlib.machinery
import io
import tarfile
import importlib.util
import os
import shutil
import subprocess
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request
from unittest import mock
from pathlib import Path
from typing import Dict, List, Optional, Tuple

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "overlay" / "usr" / "bin" / "thinfloe-addon"


def load_addon():
    loader = importlib.machinery.SourceFileLoader("thinfloe_addon", str(SCRIPT))
    spec = importlib.util.spec_from_loader("thinfloe_addon", loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


ad = load_addon()

# The ThinLinc tile as Task 5 ships it.
THINLINC_TILE = """[Desktop Entry]
Type=Application
Name=ThinLinc
Icon=/opt/thinlinc/lib/tlclient/thinlinc_128.png
Exec=/opt/thinlinc/bin/tlclient
X-TF-Save-On-Exit=true
X-TF-Multiple=true
X-TF-Source-Page=https://www.cendio.com/thinlinc/download/
X-TF-Source-Pattern=https://[^"]*/thinlinc-client_(?P<version>[0-9.]+-[0-9]+)_amd64\\.deb
X-TF-Source-Config=.thinlinc
"""


class Route:
    def __init__(self, status: int = 200, headers: Optional[Dict[str, str]] = None,
                 body: bytes = b"", hold: Optional[threading.Event] = None):
        self.status = status
        self.headers = headers or {}
        self.body = body
        # Set: half the body is sent, the rest only once the event is set.
        self.hold = hold


class Server:
    """A 127.0.0.1 web server whose answers are a dict of path -> Route.
    Every request is recorded as (method, path)."""

    def __init__(self):
        self.routes: Dict[str, Route] = {}
        self.requests: List[Tuple[str, str]] = []
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def _answer(self, with_body: bool):
                # apt asks for "<uri>/./Packages" in a flat repository.
                path = self.path.replace("/./", "/")
                outer.requests.append((self.command, path))
                route = outer.routes.get(path)
                if route is None:
                    route = Route(404, body=b"not found")
                self.send_response(route.status)
                names = {k.lower() for k in route.headers}
                for k, v in route.headers.items():
                    self.send_header(k, v)
                if "content-length" not in names:
                    self.send_header("Content-Length", str(len(route.body)))
                self.end_headers()
                if with_body and route.hold is not None:
                    half = len(route.body) // 2
                    self.wfile.write(route.body[:half])
                    self.wfile.flush()
                    route.hold.wait(30)
                    self.wfile.write(route.body[half:])
                elif with_body:
                    self.wfile.write(route.body)

            def do_GET(self):
                self._answer(True)

            def do_HEAD(self):
                self._answer(False)

            def log_message(self, *args):
                pass

        self.httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.base = "http://127.0.0.1:%d" % self.httpd.server_address[1]
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.httpd.shutdown()
        self.httpd.server_close()


class ServerTest(unittest.TestCase):
    def setUp(self):
        self.srv = Server()
        self.addCleanup(self.srv.close)


def page_source(page: str, pattern: str) -> "ad.Source":
    return ad.Source(kind="page", url=page, pattern=pattern, apt_suite=None,
                     apt_component=None, package=None, key_url=None, fmt=None,
                     config=None)


def url_source(url: str, fmt: Optional[str] = None) -> "ad.Source":
    return ad.Source(kind="url", url=url, pattern=None, apt_suite=None,
                     apt_component=None, package=None, key_url=None, fmt=fmt,
                     config=None)


def apt_source(uri: str, key_url: Optional[str], suite: str = "stable",
               component: Optional[str] = "main",
               package: str = "tf-demo") -> "ad.Source":
    return ad.Source(kind="apt", url=uri, pattern=None, apt_suite=suite,
                     apt_component=component, package=package, key_url=key_url,
                     fmt="deb", config=None)


class TileDirs(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.mkdtemp(prefix="test-addon-")
        self.addCleanup(shutil.rmtree, tmp)
        self.shipped = Path(tmp) / "shipped"
        self.user = Path(tmp) / "user"
        self.shipped.mkdir()
        self.user.mkdir()

    def write(self, d: Path, name: str, text: str):
        (d / name).write_text(text)


class TestLoad(TileDirs):
    def test_thinlinc_tile_parses(self):
        self.write(self.shipped, "10-thinlinc.desktop", THINLINC_TILE)
        self.write(self.shipped, "30-terminal.desktop",
                   "[Desktop Entry]\nName=Terminal\nExec=foot\n")
        addons, log = ad.load_addons(str(self.shipped), str(self.user))
        self.assertEqual(log, [])
        self.assertEqual([a.id for a in addons], ["10-thinlinc"])
        a = addons[0]
        self.assertEqual(a.name, "ThinLinc")
        self.assertTrue(a.shipped)
        self.assertEqual(a.tile_path, str(self.shipped / "10-thinlinc.desktop"))
        self.assertEqual(a.source.kind, "page")
        self.assertEqual(a.source.url, "https://www.cendio.com/thinlinc/download/")
        self.assertEqual(
            a.source.pattern,
            'https://[^"]*/thinlinc-client_(?P<version>[0-9.]+-[0-9]+)_amd64\\.deb')
        self.assertEqual(a.source.config, ".thinlinc")

    def test_user_reword_cannot_change_source(self):
        self.write(self.shipped, "10-thinlinc.desktop", THINLINC_TILE)
        self.write(self.user, "10-thinlinc.desktop",
                   "[Desktop Entry]\nName=Firmen-Desktop\n"
                   "X-TF-Source-URL=http://evil.example/x.deb\n"
                   "X-TF-Source-Config=.evil\n")
        addons, log = ad.load_addons(str(self.shipped), str(self.user))
        self.assertEqual(len(addons), 1)
        a = addons[0]
        self.assertEqual(a.name, "Firmen-Desktop", "Name may be reworded")
        self.assertTrue(a.shipped)
        self.assertEqual(a.source.kind, "page")
        self.assertEqual(a.source.url, "https://www.cendio.com/thinlinc/download/")
        self.assertEqual(a.source.config, ".thinlinc")
        self.assertTrue(any("10-thinlinc.desktop" in line and "ignored" in line
                            for line in log), log)

    def test_user_reword_of_a_plain_tile_does_not_make_an_addon(self):
        self.write(self.shipped, "30-terminal.desktop",
                   "[Desktop Entry]\nName=Terminal\nExec=foot\n")
        self.write(self.user, "30-terminal.desktop",
                   "[Desktop Entry]\nName=T\nX-TF-Source-URL=https://e.example/x.deb\n")
        addons, log = ad.load_addons(str(self.shipped), str(self.user))
        self.assertEqual(addons, [])
        self.assertTrue(any("ignored" in line for line in log), log)

    def test_user_addon(self):
        self.write(self.user, "60-tool.desktop",
                   "[Desktop Entry]\nName=Tool\nExec=tool\n"
                   "X-TF-Source-Apt=https://repo.example/debian stable main\n"
                   "X-TF-Source-Package=tool\n"
                   "X-TF-Source-Key=https://repo.example/key.asc\n")
        addons, log = ad.load_addons(str(self.shipped), str(self.user))
        self.assertEqual(log, [])
        self.assertEqual(len(addons), 1)
        a = addons[0]
        self.assertEqual(a.id, "60-tool")
        self.assertFalse(a.shipped)
        s = a.source
        self.assertEqual((s.kind, s.url, s.apt_suite, s.apt_component, s.package,
                          s.key_url, s.fmt),
                         ("apt", "https://repo.example/debian", "stable", "main",
                          "tool", "https://repo.example/key.asc", "deb"))

    def test_flat_apt_repository(self):
        self.write(self.user, "60-tool.desktop",
                   "[Desktop Entry]\nName=Tool\nExec=tool\n"
                   "X-TF-Source-Apt=https://repo.example/flat ./\n"
                   "X-TF-Source-Package=tool\n")
        addons, log = ad.load_addons(str(self.shipped), str(self.user))
        self.assertEqual(log, [])
        self.assertEqual((addons[0].source.apt_suite, addons[0].source.apt_component),
                         ("./", None))

    def test_user_addon_may_only_run(self):
        self.write(self.user, "60-x.desktop",
                   "[Desktop Entry]\nName=X\nX-TF-Action=reboot\n"
                   "X-TF-Source-URL=https://e.example/x.deb\n")
        addons, log = ad.load_addons(str(self.shipped), str(self.user))
        self.assertEqual(addons, [])
        self.assertEqual(len(log), 1)
        self.assertIn("60-x.desktop", log[0])

    def test_broken_definitions_are_logged_and_skipped(self):
        cases = {
            "a.desktop": "X-TF-Source-URL=https://e.example/x.deb\n"
                         "X-TF-Source-Page=https://e.example/\n",
            "b.desktop": "X-TF-Source-Page=https://e.example/\n",
            "c.desktop": "X-TF-Source-Page=https://e.example/\nX-TF-Source-Pattern=(\n",
            "d.desktop": "X-TF-Source-Apt=https://e.example/debian stable main\n",
            "e.desktop": "X-TF-Source-URL=https://e.example/x.deb\nX-TF-Source-Format=zip\n",
            "f.desktop": "X-TF-Source-URL=ftp://e.example/x.deb\n",
            "g.desktop": "X-TF-Source-Apt=https://e.example/debian stable main\n"
                         "X-TF-Source-Package=p\nX-TF-Source-Key=http://e.example/k\n",
            "h.desktop": "X-TF-Source-URL=https://e.example/x.deb\n"
                         "X-TF-Source-Config=../etc\n",
            "i.desktop": "X-TF-Source-Apt=https://e.example/debian stable main\n"
                         "X-TF-Source-Package=p\nX-TF-Source-Format=tar\n",
            "j.desktop": "X-TF-Source-Package=p\n",
        }
        for name, keys in cases.items():
            self.write(self.user, name, "[Desktop Entry]\nName=N\nExec=n\n" + keys)
        self.write(self.user, "k.desktop", "this is junk\nX-TF-Source-URL=x\n")
        addons, log = ad.load_addons(str(self.shipped), str(self.user))
        self.assertEqual(addons, [])
        for name in list(cases) + ["k.desktop"]:
            self.assertTrue(any(name in line for line in log), (name, log))

    def test_user_cap_is_32_files(self):
        for i in range(40):
            self.write(self.user, "%02d-t.desktop" % i,
                       "[Desktop Entry]\nName=T\nExec=t\n"
                       "X-TF-Source-URL=https://e.example/%d.deb\n" % i)
        addons, log = ad.load_addons(str(self.shipped), str(self.user))
        self.assertEqual(len(addons), 32)
        self.assertEqual(addons[-1].id, "31-t")
        self.assertTrue(any("32" in line for line in log), log)

    def test_symlinked_tile_is_not_read(self):
        target = self.user.parent / "elsewhere.desktop"
        target.write_text("[Desktop Entry]\nName=T\nExec=t\n"
                          "X-TF-Source-URL=https://e.example/x.deb\n")
        (self.user / "60-link.desktop").symlink_to(target)
        addons, _ = ad.load_addons(str(self.shipped), str(self.user))
        self.assertEqual(addons, [])

    def test_missing_user_dir_is_fine(self):
        self.write(self.shipped, "10-thinlinc.desktop", THINLINC_TILE)
        addons, log = ad.load_addons(str(self.shipped), str(self.user / "nope"))
        self.assertEqual(len(addons), 1)
        self.assertEqual(log, [])


class TestAutoAllowed(unittest.TestCase):
    def test_auto_allowed(self):
        page = page_source("https://e.example/", "x")
        self.assertTrue(ad.auto_allowed(
            page, ["https://e.example/", "https://cdn.example/x.deb"]))
        self.assertFalse(ad.auto_allowed(
            page, ["https://e.example/", "http://cdn.example/x.deb",
                   "https://cdn2.example/x.deb"]))
        self.assertFalse(ad.auto_allowed(
            url_source("http://e.example/x.deb"), ["http://e.example/x.deb"]))
        self.assertTrue(ad.auto_allowed(
            apt_source("http://repo.example/debian", "https://repo.example/k.asc"),
            ["http://repo.example/debian"]))
        self.assertFalse(ad.auto_allowed(
            apt_source("http://repo.example/debian", None),
            ["http://repo.example/debian"]))
        self.assertTrue(ad.auto_allowed(
            apt_source("https://repo.example/debian", None),
            ["https://repo.example/debian"]))

    def test_source_url_counts_even_when_not_listed(self):
        self.assertFalse(ad.auto_allowed(url_source("http://e.example/x.deb"), []))


class TestVersionGt(unittest.TestCase):
    def test_dpkg_order(self):
        self.assertTrue(ad.version_gt("4.21.0-4588", "4.20.0-1"))
        self.assertFalse(ad.version_gt("4.20.0-1", "4.21.0-4588"))
        self.assertTrue(ad.version_gt("1.10", "1.9"))
        self.assertTrue(ad.version_gt("1.0", "1.0~rc1"))
        self.assertFalse(ad.version_gt("1.0", "1.0"))


class TestPage(ServerTest):
    def serve_debs(self, *names):
        for n in names:
            self.srv.routes["/dl/" + n] = Route(200, {"ETag": '"%s"' % n})

    def test_page_highest_version_wins(self):
        b = self.srv.base
        self.srv.routes["/download/"] = Route(200, body=(
            '<a href="%s/dl/thinlinc-client_4.20.0-1_amd64.deb">old</a>\n'
            '<a href="%s/dl/thinlinc-client_4.21.0-4588_amd64.deb">new</a>\n'
            '<a href="%s/dl/thinlinc-client_4.3.0-9_amd64.deb">odd</a>\n'
            % (b, b, b)).encode())
        self.serve_debs("thinlinc-client_4.20.0-1_amd64.deb",
                        "thinlinc-client_4.21.0-4588_amd64.deb",
                        "thinlinc-client_4.3.0-9_amd64.deb")
        pattern = 'http://[^"]*/thinlinc-client_(?P<version>[0-9.]+-[0-9]+)_amd64\\.deb'
        found = ad.check(page_source(b + "/download/", pattern))
        self.assertEqual(found.version, "4.21.0-4588")
        self.assertEqual(found.url, b + "/dl/thinlinc-client_4.21.0-4588_amd64.deb")
        self.assertEqual(found.fmt, "deb")
        self.assertIn(b + "/download/", found.urls_seen)
        self.assertIn(found.url, found.urls_seen)

    def test_page_without_version_group_first_match(self):
        b = self.srv.base
        self.srv.routes["/p"] = Route(200, body=(
            '<a href="/dl/tool-b.tar.gz">b</a> <a href="/dl/tool-a.tar.gz">a</a>'
        ).encode())
        self.serve_debs("tool-a.tar.gz", "tool-b.tar.gz")
        found = ad.check(page_source(b + "/p", "/dl/tool-[a-z]\\.tar\\.gz"))
        self.assertEqual(found.url, b + "/dl/tool-b.tar.gz", "relative link resolved")
        self.assertEqual(found.fingerprint, found.url)
        self.assertIsNone(found.version)
        self.assertEqual(found.fmt, "tar")

    def test_pattern_without_match_fails(self):
        self.srv.routes["/p"] = Route(200, body=b"<html>nothing here</html>")
        page = self.srv.base + "/p"
        with self.assertRaises(ad.CheckError) as cm:
            ad.check(page_source(page, "x_(?P<version>[0-9.]+)\\.deb"))
        self.assertIn("no link on %s matches the pattern" % page, str(cm.exception))

    def test_page_larger_than_cap_fails(self):
        self.srv.routes["/p"] = Route(200, body=b"x" * (2 * 1024 * 1024 + 1))
        with self.assertRaises(ad.CheckError) as cm:
            ad.check(page_source(self.srv.base + "/p", "x"))
        self.assertIn("2 MiB", str(cm.exception))

    def test_page_error_status_fails(self):
        with self.assertRaises(ad.CheckError) as cm:
            ad.check(page_source(self.srv.base + "/gone", "x"))
        self.assertIn("404", str(cm.exception))

    def test_catastrophic_pattern_is_cut_off(self):
        # (a+)+$ backtracks exponentially on a run of a's without a match.
        self.srv.routes["/p"] = Route(200, body=b"a" * 40 + b"!")
        page = self.srv.base + "/p"
        start = time.monotonic()
        with self.assertRaises(ad.CheckError) as cm:
            ad.check(page_source(page, "(a+)+$"))
        elapsed = time.monotonic() - start
        self.assertIn("the pattern took too long on %s" % page, str(cm.exception))
        self.assertLess(elapsed, 8, "the match must be stopped after about 5 s")

    def test_user_agent(self):
        seen = []
        self.srv.routes["/p"] = Route(200, body=b'<a href="/f.deb">')
        self.srv.routes["/f.deb"] = Route(200, {"ETag": '"1"'})
        handler = self.srv.httpd.RequestHandlerClass
        old = handler._answer

        def spy(h, with_body):
            seen.append(h.headers.get("User-Agent"))
            return old(h, with_body)

        handler._answer = spy
        self.addCleanup(setattr, handler, "_answer", old)
        ad.check(page_source(self.srv.base + "/p", "/f\\.deb"))
        self.assertTrue(seen)
        for ua in seen:
            self.assertTrue(ua.startswith("thinfloe-addon/"), ua)


class TestUrl(ServerTest):
    def test_url_fingerprint_order(self):
        b = self.srv.base
        self.srv.routes["/e.deb"] = Route(200, {
            "ETag": '"abc"', "Last-Modified": "Wed, 21 Oct 2026 07:28:00 GMT"},
            body=b"12345")
        self.srv.routes["/m.deb"] = Route(200, {
            "Last-Modified": "Wed, 21 Oct 2026 07:28:00 GMT"}, body=b"12345")
        self.srv.routes["/l.deb"] = Route(200, body=b"12345")
        fe = ad.check(url_source(b + "/e.deb"))
        fm = ad.check(url_source(b + "/m.deb"))
        fl = ad.check(url_source(b + "/l.deb"))
        self.assertEqual(fe.fingerprint, 'etag:"abc"')
        self.assertEqual(fm.fingerprint, "last-modified:Wed, 21 Oct 2026 07:28:00 GMT")
        self.assertEqual(fl.fingerprint, "length:5 " + b + "/l.deb")
        self.assertEqual({m for m, _ in self.srv.requests}, {"HEAD"}, "HEAD only")
        for f in (fe, fm, fl):
            self.assertIsNone(f.version)
            self.assertEqual(f.fmt, "deb")

    def test_url_redirects_followed_and_recorded(self):
        b = self.srv.base
        self.srv.routes["/latest.deb"] = Route(302, {"Location": "/hop"})
        self.srv.routes["/hop"] = Route(301, {"Location": b + "/files/tool_2.0.deb"})
        self.srv.routes["/files/tool_2.0.deb"] = Route(200, {"ETag": '"2"'})
        found = ad.check(url_source(b + "/latest.deb"))
        self.assertEqual(found.urls_seen,
                         [b + "/latest.deb", b + "/hop", b + "/files/tool_2.0.deb"])
        self.assertEqual(found.url, b + "/latest.deb")
        self.assertEqual(found.fingerprint, 'etag:"2"')

    def test_too_many_redirects(self):
        for i in range(12):
            self.srv.routes["/r%d" % i] = Route(302, {"Location": "/r%d" % (i + 1)})
        with self.assertRaises(ad.CheckError) as cm:
            ad.check(url_source(self.srv.base + "/r0", fmt="deb"))
        self.assertIn("redirect", str(cm.exception))

    def test_unknown_format_is_error(self):
        self.srv.routes["/tool.zip"] = Route(200, {"ETag": '"z"'})
        with self.assertRaises(ad.CheckError) as cm:
            ad.check(url_source(self.srv.base + "/tool.zip"))
        self.assertIn("format", str(cm.exception))

    def test_format_key_overrides(self):
        self.srv.routes["/tool.zip"] = Route(200, {"ETag": '"z"'})
        self.assertEqual(ad.check(url_source(self.srv.base + "/tool.zip", "tar")).fmt,
                         "tar")

    def test_tar_suffixes(self):
        for suffix in (".tar", ".tar.gz", ".tgz", ".tar.xz", ".tar.zst"):
            self.srv.routes["/o" + suffix] = Route(200, {"ETag": '"t"'})
            self.assertEqual(ad.check(url_source(self.srv.base + "/o" + suffix)).fmt,
                             "tar", suffix)


class FakeRedirectOpener:
    """Answers every https URL in `redirects` with a 302 to its target and
    records each URL asked for; no network."""

    def __init__(self, redirects):
        self.redirects = redirects
        self.asked = []

    def open(self, req, timeout=None):
        url = req.full_url
        self.asked.append(url)
        if url in self.redirects:
            raise urllib.error.HTTPError(url, 302, "Found",
                                         {"Location": self.redirects[url]}, None)
        raise AssertionError("must not be asked for %s" % url)


class TestKeyFetch(unittest.TestCase):
    def test_key_redirect_to_http_is_refused(self):
        key = "https://repo.example/key.asc"
        hop = "http://mirror.example/key.asc"
        opener = FakeRedirectOpener({key: hop})
        src = apt_source("https://repo.example/debian", key)
        with mock.patch.object(ad, "_opener", opener):
            with self.assertRaises(ad.CheckError) as cm:
                ad.check(src)
        self.assertIn(hop, str(cm.exception))
        self.assertEqual(opener.asked, [key], "the http hop is never requested")


class TestApt(ServerTest):
    def test_apt_check(self):
        if not shutil.which("dpkg-scanpackages"):
            self.skipTest("dpkg-scanpackages missing (install dpkg-dev)")
        tmp = Path(tempfile.mkdtemp(prefix="test-addon-apt-"))
        self.addCleanup(shutil.rmtree, tmp)
        repo = tmp / "repo"
        repo.mkdir()
        for version in ("1.2-3", "1.10-1"):
            pkg = tmp / ("pkg-" + version)
            (pkg / "DEBIAN").mkdir(parents=True)
            (pkg / "DEBIAN" / "control").write_text(
                "Package: tf-demo\nVersion: %s\nArchitecture: all\n"
                "Maintainer: Test <test@example.org>\nDescription: demo\n" % version)
            subprocess.run(["dpkg-deb", "--build", str(pkg),
                            str(repo / ("tf-demo_%s_all.deb" % version))],
                           check=True, capture_output=True)
        packages = subprocess.run(
            ["dpkg-scanpackages", "--multiversion", ".", "/dev/null"],
            cwd=repo, check=True, capture_output=True).stdout
        self.srv.routes["/flat/Packages"] = Route(200, body=packages)
        lists_before = sorted(os.listdir("/var/lib/apt/lists"))
        src = apt_source(self.srv.base + "/flat", None, suite="./", component=None)
        found = ad.check(src)
        self.assertEqual(found.version, "1.10-1")
        self.assertEqual(found.fmt, "deb")
        self.assertEqual(found.url, self.srv.base + "/flat/./tf-demo_1.10-1_all.deb")
        self.assertIn("1.10-1", found.fingerprint)
        self.assertEqual(found.urls_seen, [self.srv.base + "/flat"])
        self.assertEqual(sorted(os.listdir("/var/lib/apt/lists")), lists_before)
        self.assertTrue(any(p == "/flat/Packages" for _, p in self.srv.requests))

    def test_apt_missing_package(self):
        if not shutil.which("dpkg-scanpackages"):
            self.skipTest("dpkg-scanpackages missing (install dpkg-dev)")
        self.srv.routes["/flat/Packages"] = Route(200, body=b"")
        src = apt_source(self.srv.base + "/flat", None, suite="./", component=None,
                         package="nope")
        with self.assertRaises(ad.CheckError) as cm:
            ad.check(src)
        self.assertIn("nope", str(cm.exception))

    def test_apt_unreachable(self):
        src = apt_source("http://127.0.0.1:9/flat", None, suite="./", component=None)
        with self.assertRaises(ad.CheckError):
            ad.check(src)


# ============ Store, install, remove, status ============

FAKE_DPKG = """#!/bin/sh
printf '%s\\n' "$*" >> "$(dirname "$0")/dpkg.log"
if flock -n "$(dirname "$0")/../esp.lock" true; then echo free; else echo held; fi \\
    >> "$(dirname "$0")/dpkg.lockstate"
if ! mkdir "$(dirname "$0")/dpkg.running" 2>/dev/null; then
    echo concurrent >> "$(dirname "$0")/dpkg.clash"
fi
if [ -e "$(dirname "$0")/dpkg.slow" ]; then sleep 0.5; fi
# dpkg.hang-on / dpkg.fail-on: hang or fail when the arguments hold its text.
if [ -e "$(dirname "$0")/dpkg.hang-on" ]; then
    case "$*" in *"$(cat "$(dirname "$0")/dpkg.hang-on")"*) sleep 30 ;; esac
fi
if [ -e "$(dirname "$0")/dpkg.fail-on" ]; then
    case "$*" in *"$(cat "$(dirname "$0")/dpkg.fail-on")"*) touch "$(dirname "$0")/dpkg.fail" ;; esac
fi
rmdir "$(dirname "$0")/dpkg.running" 2>/dev/null
if [ -e "$(dirname "$0")/dpkg.fail" ]; then
    [ -e "$(dirname "$0")/dpkg.fail-on" ] && rm -f "$(dirname "$0")/dpkg.fail"
    echo "dpkg: error: boom" >&2
    exit 1
fi
exit 0
"""

DPKG_I = "--force-confdef --force-confold -i "
DEMO_PATTERN = "/dl/tf-demo_(?P<version>[0-9.]+)_all\\.deb"


def build_deb(workdir: Path, version: str, package: str = "tf-demo") -> bytes:
    pkg = workdir / ("build-%s-%s" % (package, version))
    (pkg / "DEBIAN").mkdir(parents=True)
    (pkg / "DEBIAN" / "control").write_text(
        "Package: %s\nVersion: %s\nArchitecture: all\n"
        "Maintainer: Test <test@example.org>\nDescription: demo\n" % (package, version))
    (pkg / "usr" / "share" / package).mkdir(parents=True)
    (pkg / "usr" / "share" / package / "version").write_text(version + "\n")
    out = workdir / ("%s_%s_all.deb" % (package, version))
    subprocess.run(["dpkg-deb", "--build", "--root-owner-group", str(pkg), str(out)],
                   check=True, capture_output=True)
    return out.read_bytes()


def build_tar(members: Dict[str, bytes]) -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as t:
        for name, data in members.items():
            ti = tarfile.TarInfo(name)
            ti.size = len(data)
            ti.mode = 0o755
            t.addfile(ti, io.BytesIO(data))
    return buf.getvalue()


class AddonEnv(ServerTest):
    """A machine in a temporary directory: ESP, /run/thinfloe/addons, the lock,
    a root for tar installs, tfuser's home, the shipped tiles and a fake
    dpkg. Every http URL counts as https unless a test says otherwise."""

    def setUp(self):
        super().setUp()
        tmp = Path(tempfile.mkdtemp(prefix="test-addon-store-"))
        self.addCleanup(shutil.rmtree, tmp)
        self.tmp = tmp
        self.esp = tmp / "esp"
        self.run_dir = tmp / "run"
        self.root = tmp / "root"
        self.home = tmp / "home"
        self.tiles = tmp / "tiles"
        self.bin = tmp / "bin"
        for d in (self.esp, self.root, self.home, self.tiles, self.bin):
            d.mkdir()
        self.lock = tmp / "esp.lock"
        dpkg = self.bin / "dpkg"
        dpkg.write_text(FAKE_DPKG)
        dpkg.chmod(0o755)
        patches = {
            "ESP_DIR": str(self.esp), "RUN_DIR": str(self.run_dir),
            "LOCK_PATH": str(self.lock), "ROOT_DIR": str(self.root),
            "HOME_DIR": str(self.home), "DPKG": str(dpkg),
            "SHIPPED_TILES": str(self.tiles),
            "PLYMOUTH_STEP": str(tmp / "no-plymouth-step"),
            "esp_mounted": lambda: True,
            "tfuser_ids": lambda: (os.getuid(), os.getgid()),
            "_secure": lambda url: True,
        }
        for name, value in patches.items():
            p = mock.patch.object(ad, name, value)
            p.start()
            self.addCleanup(p.stop)
        self.debs = {}

    # -- helpers --

    def demo_tile(self, extra: str = ""):
        (self.tiles / "10-demo.desktop").write_text(
            "[Desktop Entry]\nName=Demo\nExec=demo\n"
            "X-TF-Source-Page=%s/dl/\nX-TF-Source-Pattern=%s\n%s"
            % (self.srv.base, DEMO_PATTERN, extra))

    def publish(self, *versions: str):
        """The download page lists exactly these versions."""
        links = []
        for v in versions:
            if v not in self.debs:
                self.debs[v] = build_deb(self.tmp, v)
            path = "/dl/tf-demo_%s_all.deb" % v
            self.srv.routes[path] = Route(200, {"ETag": '"%s"' % v}, body=self.debs[v])
            links.append('<a href="%s">%s</a>' % (path, v))
        self.srv.routes["/dl/"] = Route(200, body="\n".join(links).encode())

    def tick(self, *ids: str):
        d = self.home / ".config" / "thinfloe"
        d.mkdir(parents=True, exist_ok=True)
        (d / "addons").write_text("".join(i + "\n" for i in ids))

    def ticks(self) -> List[str]:
        f = self.home / ".config" / "thinfloe" / "addons"
        return f.read_text().split() if f.exists() else []

    def run_cmd(self, *argv: str) -> int:
        with contextlib.redirect_stdout(io.StringIO()):
            return ad.main(list(argv))

    def addon_dir(self, aid: str = "10-demo") -> Path:
        return self.esp / "addons" / aid

    def files(self, aid: str = "10-demo") -> List[str]:
        d = self.addon_dir(aid)
        return sorted(os.listdir(d)) if d.exists() else []

    def meta(self, slot: str, aid: str = "10-demo") -> Dict[str, str]:
        out = {}
        for line in (self.addon_dir(aid) / (slot + ".meta")).read_text().splitlines():
            k, v = line.split("=", 1)
            out[k] = v
        return out

    def dpkg_calls(self) -> List[str]:
        f = self.bin / "dpkg.log"
        return f.read_text().splitlines() if f.exists() else []

    def status(self) -> List[str]:
        return (self.run_dir / "status").read_text().splitlines()

    def log_lines(self) -> List[str]:
        f = self.run_dir / "log"
        return [l.split("\t", 1)[1] for l in f.read_text().splitlines()] if f.exists() else []


class TestStore(AddonEnv):
    def test_first_install_stores_current_and_installs(self):
        self.demo_tile()
        self.publish("1.0")
        self.assertEqual(self.run_cmd("install", "10-demo"), 0)
        self.assertEqual(self.ticks(), ["10-demo"])
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertEqual((self.addon_dir() / "current.deb").read_bytes(), self.debs["1.0"])
        m = self.meta("current")
        self.assertEqual((m["version"], m["format"], m["package"], m["fingerprint"]),
                         ("1.0", "deb", "tf-demo", "1.0"))
        self.assertEqual(m["url"], self.srv.base + "/dl/tf-demo_1.0_all.deb")
        self.assertTrue(m["fetched"].isdigit())
        self.assertEqual(self.dpkg_calls(),
                         [DPKG_I + str(self.addon_dir() / "current.deb")])
        self.assertEqual(self.status(), ["10-demo\t1\tinstalled\t1.0\t-\t1\tDemo 1.0 installed"])
        self.assertIn("Demo 1.0 installed", self.log_lines())

    def test_check_installs_when_there_is_no_copy_yet(self):
        self.demo_tile()
        self.publish("1.0")
        self.tick("10-demo")
        self.assertEqual(self.run_cmd("check"), 0)
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertEqual(len(self.dpkg_calls()), 1)
        self.assertTrue((self.run_dir / "last-check").read_text().strip().isdigit())

    def test_newer_version_becomes_next_and_pending(self):
        self.demo_tile()
        self.publish("1.0")
        self.assertEqual(self.run_cmd("install", "10-demo"), 0)
        self.publish("1.0", "1.1")
        self.assertEqual(self.run_cmd("check"), 0)
        self.assertEqual(self.files(), ["current.deb", "current.meta", "next.deb", "next.meta"])
        self.assertEqual(self.meta("current")["version"], "1.0")
        self.assertEqual(self.meta("next")["version"], "1.1")
        self.assertEqual((self.addon_dir() / "next.deb").read_bytes(), self.debs["1.1"])
        self.assertEqual(len(self.dpkg_calls()), 1, "the check installs nothing")
        self.assertEqual(self.status(), [
            "10-demo\t1\tpending\t1.0\t1.1\t1\t"
            "Demo 1.1 stored on the boot stick, active at next boot"])
        # A second check finds the same version and leaves next alone.
        before = len(self.srv.requests)
        self.assertEqual(self.run_cmd("check"), 0)
        self.assertNotIn(("GET", "/dl/tf-demo_1.1_all.deb"), self.srv.requests[before:])
        self.assertEqual(self.status()[0].split("\t")[2], "pending")

    def test_up_to_date(self):
        self.demo_tile()
        self.publish("1.0")
        self.run_cmd("install", "10-demo")
        self.assertEqual(self.run_cmd("check"), 0)
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertIn("Demo: up to date", self.log_lines())
        self.assertEqual(self.status(), ["10-demo\t1\tinstalled\t1.0\t-\t1\tDemo: up to date"])

    def test_update_promotes_next(self):
        self.demo_tile()
        self.publish("1.0")
        self.run_cmd("install", "10-demo")
        self.publish("1.0", "1.1")
        self.run_cmd("check")
        self.assertEqual(self.run_cmd("update", "10-demo"), 0)
        self.assertEqual(self.dpkg_calls()[-1], DPKG_I + str(self.addon_dir() / "next.deb"))
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertEqual(self.meta("current")["version"], "1.1")
        self.assertEqual((self.addon_dir() / "current.deb").read_bytes(), self.debs["1.1"])
        self.assertEqual(self.status(), ["10-demo\t1\tinstalled\t1.1\t-\t1\tDemo 1.1 installed"])

    def test_update_without_next_fetches_first(self):
        self.demo_tile()
        self.publish("1.0")
        self.run_cmd("install", "10-demo")
        self.publish("1.0", "1.1")
        self.assertEqual(self.run_cmd("update", "10-demo"), 0)
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertEqual(self.meta("current")["version"], "1.1")
        self.assertEqual(len(self.dpkg_calls()), 2)

    def test_failed_install_keeps_next(self):
        self.demo_tile()
        self.publish("1.0")
        self.run_cmd("install", "10-demo")
        self.publish("1.0", "1.1")
        self.run_cmd("check")
        (self.bin / "dpkg.fail").touch()
        self.assertEqual(self.run_cmd("update", "10-demo"), 1)
        self.assertEqual(self.meta("current")["version"], "1.0")
        self.assertEqual(self.meta("next")["version"], "1.1")

    def test_failed_check_keeps_current(self):
        self.demo_tile()
        self.publish("1.0")
        self.run_cmd("install", "10-demo")
        before = (self.addon_dir() / "current.deb").read_bytes()
        self.srv.routes["/dl/"] = Route(500, body=b"down")
        self.assertEqual(self.run_cmd("check"), 1)
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertEqual((self.addon_dir() / "current.deb").read_bytes(), before)
        self.assertEqual(self.status(), [
            "10-demo\t1\tfailed\t1.0\t-\t1\t"
            "Demo: check failed: %s/dl/ answered HTTP 500" % self.srv.base])
        self.assertEqual(len(self.dpkg_calls()), 1)

    def test_html_instead_of_a_deb_is_refused(self):
        self.demo_tile()
        self.publish("1.0")
        url = self.srv.base + "/dl/tf-demo_1.0_all.deb"
        self.srv.routes["/dl/tf-demo_1.0_all.deb"] = Route(
            200, {"ETag": '"x"'}, body=b"<html><body>Please log in</body></html>")
        self.assertEqual(self.run_cmd("install", "10-demo"), 1)
        self.assertEqual(self.files(), [])
        self.assertEqual(self.dpkg_calls(), [])
        self.assertTrue(any(url in l and "not a Debian package" in l
                            for l in self.log_lines()), self.log_lines())
        self.assertEqual(self.status()[0].split("\t")[:5],
                         ["10-demo", "1", "failed", "-", "-"])

    def test_cut_download_is_refused(self):
        self.demo_tile()
        self.publish("1.0")
        r = self.srv.routes["/dl/tf-demo_1.0_all.deb"]
        r.headers["Content-Length"] = str(len(r.body) + 100)
        r.headers["Connection"] = "close"
        self.assertEqual(self.run_cmd("install", "10-demo"), 1)
        self.assertEqual(self.files(), [])

    def test_no_space_keeps_current_and_removes_new(self):
        self.demo_tile()
        self.publish("1.0")
        self.run_cmd("install", "10-demo")
        self.publish("1.0", "1.1")
        esp = str(self.esp)
        with mock.patch.object(ad, "free_bytes", lambda path: 10 * 1024 * 1024
                               if str(path).startswith(esp) else 1 << 40):
            self.assertEqual(self.run_cmd("check"), 1)
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertEqual(self.meta("current")["version"], "1.0")
        self.assertEqual(self.status(), [
            "10-demo\t1\tfailed\t1.0\t-\t1\t"
            "Demo: download failed: not enough space on the boot stick"])

    def test_the_esp_reserve_holds_a_second_home_archive(self):
        # m3: a save writes home.tar.gz.new beside home.tar.gz, so a store on
        # the ESP keeps MIN_FREE_BYTES plus the archive's size free.
        self.demo_tile()
        self.publish("1.0")
        self.tick("10-demo")
        esp = str(self.esp)
        mib = 1024 * 1024
        free = lambda path: 100 * mib if str(path).startswith(esp) else 1 << 40
        with open(self.esp / "home.tar.gz", "wb") as f:
            f.truncate(50 * mib)
        self.assertEqual(ad.store_reserve(), ad.MIN_FREE_BYTES + 50 * mib)
        with mock.patch.object(ad, "free_bytes", free):
            self.assertEqual(self.run_cmd("check"), 1)
        self.assertEqual(self.files(), [])
        self.assertIn("Demo: download failed: not enough space on the boot stick",
                      self.log_lines())
        # A smaller archive leaves room.
        with open(self.esp / "home.tar.gz", "wb") as f:
            f.truncate(10 * mib)
        with mock.patch.object(ad, "free_bytes", free):
            self.assertEqual(self.run_cmd("check"), 0, self.log_lines())
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        # No ESP (netboot): the store is in RAM and no save writes there.
        with mock.patch.object(ad, "esp_mounted", lambda: False):
            self.assertEqual(ad.store_reserve(), ad.MIN_FREE_BYTES)
        (self.esp / "home.tar.gz").unlink()
        self.assertEqual(ad.store_reserve(), ad.MIN_FREE_BYTES)

    def test_store_waits_for_the_esp_lock(self):
        self.demo_tile()
        self.publish("1.0")
        self.tick("10-demo")
        fd = os.open(str(self.lock), os.O_RDWR | os.O_CREAT, 0o644)
        self.addCleanup(os.close, fd)
        fcntl.flock(fd, fcntl.LOCK_EX)
        rc = []
        t = threading.Thread(target=lambda: rc.append(ad.main(["check"])))
        with contextlib.redirect_stdout(io.StringIO()):
            t.start()
            deadline = time.monotonic() + 15
            while ("HEAD", "/dl/tf-demo_1.0_all.deb") not in self.srv.requests:
                self.assertLess(time.monotonic(), deadline, "the check never ran")
                time.sleep(0.05)
            time.sleep(0.5)
            self.assertFalse(self.addon_dir().exists(), "wrote the ESP under a held lock")
            fcntl.flock(fd, fcntl.LOCK_UN)
            t.join(30)
        self.assertEqual(rc, [0])
        self.assertEqual(self.files(), ["current.deb", "current.meta"])

    def test_netboot_stores_in_the_cache(self):
        cache = self.tmp / "cache"
        self.demo_tile()
        self.publish("1.0")
        with mock.patch.object(ad, "esp_mounted", lambda: False), \
                mock.patch.object(ad, "CACHE_DIR", str(cache)):
            self.assertEqual(self.run_cmd("install", "10-demo"), 0)
        self.assertEqual(sorted(os.listdir(cache / "10-demo")), ["current.deb", "current.meta"])
        self.assertFalse((self.esp / "addons").exists())


class TestTar(AddonEnv):
    def tar_tile(self, members: Dict[str, bytes]):
        (self.tiles / "20-tool.desktop").write_text(
            "[Desktop Entry]\nName=Tool\nExec=/opt/tool/bin/tool\n"
            "X-TF-Source-URL=%s/tool.tar.gz\n" % self.srv.base)
        self.srv.routes["/tool.tar.gz"] = Route(200, {"ETag": '"t1"'}, body=build_tar(members))

    def test_a_listing_timeout_at_boot_keeps_next_tar(self):
        # N1, the tar case of test_a_timeout_at_boot_keeps_next.
        self.tar_tile({"opt/tool/v": b"1\n"})
        self.assertEqual(self.run_cmd("install", "20-tool"), 0)
        self.srv.routes["/tool.tar.gz"] = Route(200, {"ETag": '"t2"'},
                                                body=build_tar({"opt/tool/v": b"2\n"}))
        self.assertEqual(self.run_cmd("check"), 0, self.log_lines())
        self.assertEqual(self.files("20-tool"),
                         ["current.meta", "current.tar", "next.meta", "next.tar"])
        shutil.rmtree(self.run_dir)
        real = ad._tool

        def slow_listing(cmd, timeout=None):
            if cmd[0] == "tar" and "-tf" in cmd and cmd[-1].endswith("next.tar"):
                raise subprocess.TimeoutExpired(cmd, 1)
            return real(cmd, timeout)

        with mock.patch.object(ad, "_tool", slow_listing):
            self.assertEqual(self.run_cmd("boot"), 1)
        self.assertEqual(self.files("20-tool"),
                         ["current.meta", "current.tar", "next.meta", "next.tar"])
        line = self.status()[0].split("\t")
        self.assertEqual(line[2], "install-failed")
        self.assertIn("took longer than 1 s; the check after boot installs it", line[6])
        self.assertEqual(self.run_cmd("check"), 0, self.log_lines())
        self.assertEqual(self.files("20-tool"), ["current.meta", "current.tar"])
        self.assertEqual((self.root / "opt/tool/v").read_bytes(), b"2\n")

    def test_tar_installs_into_root_and_remove_deletes_it(self):
        self.tar_tile({"opt/tool/bin/tool": b"#!/bin/sh\n", "opt/tool/README": b"hi\n"})
        self.assertEqual(self.run_cmd("install", "20-tool"), 0)
        self.assertEqual(self.files("20-tool"), ["current.meta", "current.tar"])
        self.assertEqual((self.root / "opt/tool/README").read_bytes(), b"hi\n")
        self.assertEqual(self.dpkg_calls(), [])
        self.assertEqual(self.status()[0], "20-tool\t1\tinstalled\t-\t-\t1\tTool installed")
        self.assertEqual(self.run_cmd("remove", "20-tool"), 0)
        self.assertFalse((self.root / "opt/tool/README").exists())
        self.assertFalse((self.root / "opt/tool/bin/tool").exists())
        self.assertFalse(self.addon_dir("20-tool").exists())
        self.assertEqual(self.ticks(), [])

    def test_tar_must_be_relative(self):
        for bad in ("/etc/evil", "opt/../../evil", "../evil"):
            with self.subTest(member=bad):
                self.tar_tile({"opt/tool/ok": b"x", bad: b"x"})
                self.assertEqual(self.run_cmd("install", "20-tool"), 1)
                self.assertEqual(self.files("20-tool"), [])
                self.assertEqual(os.listdir(self.root), [])
                self.assertTrue(any(bad in l for l in self.log_lines()), self.log_lines())

    def test_tar_member_with_a_newline_is_refused(self):
        self.tar_tile({"opt/tool/x\netc/passwd": b"x"})
        self.assertEqual(self.run_cmd("install", "20-tool"), 1)
        self.assertEqual(os.listdir(self.root), [])

    def test_not_a_tar_is_refused(self):
        self.tar_tile({})
        self.srv.routes["/tool.tar.gz"] = Route(200, {"ETag": '"t"'}, body=b"<html>no</html>")
        self.assertEqual(self.run_cmd("install", "20-tool"), 1)
        self.assertEqual(self.files("20-tool"), [])


class TestRemove(AddonEnv):
    def test_remove_unticks_and_cleans(self):
        self.demo_tile()
        self.publish("1.0")
        self.run_cmd("install", "10-demo")
        self.publish("1.0", "1.1")
        self.run_cmd("check")
        self.assertEqual(self.run_cmd("remove", "10-demo"), 0)
        self.assertEqual(self.ticks(), [])
        self.assertEqual(self.dpkg_calls()[-1], "-r tf-demo")
        self.assertFalse(self.addon_dir().exists())
        self.assertEqual(self.status(), ["10-demo\t0\tabsent\t-\t-\t1\t-"])
        self.assertIn("Demo removed", self.log_lines())

    def test_remove_keeps_other_ticks(self):
        self.demo_tile()
        self.publish("1.0")
        self.tick("05-gone", "10-demo")
        self.run_cmd("install", "10-demo")
        self.run_cmd("remove", "10-demo")
        self.assertEqual(self.ticks(), ["05-gone"], "unknown ids are kept")
        self.assertTrue(any("05-gone" in l for l in self.log_lines()))


class TestManual(AddonEnv):
    def setUp(self):
        super().setUp()
        p = mock.patch.object(ad, "_secure", lambda url: url.lower().startswith("https:"))
        p.start()
        self.addCleanup(p.stop)

    def test_manual_source_skipped_by_check_all(self):
        self.demo_tile()
        self.publish("1.0")
        self.tick("10-demo")
        self.assertEqual(self.run_cmd("check"), 0)
        self.assertIn("Demo: not checked automatically (neither HTTPS nor signed)",
                      self.log_lines())
        self.assertEqual(self.srv.requests, [])
        self.assertEqual(self.files(), [])
        self.assertEqual(self.status(), ["10-demo\t1\tabsent\t-\t-\t0\t-"])

    def test_check_installs_a_stored_manual_copy_without_a_request(self):
        # R20: installing what is on the boot stick is no fetch, so R1 allows
        # it, as boot does.
        self.demo_tile()
        self.publish("1.0")
        self.assertEqual(self.run_cmd("install", "10-demo"), 0)
        shutil.rmtree(self.run_dir)
        (self.bin / "dpkg.log").unlink()
        before = len(self.srv.requests)
        self.assertEqual(self.run_cmd("check"), 0, self.log_lines())
        self.assertEqual(self.srv.requests[before:], [])
        self.assertEqual(self.dpkg_calls(),
                         [DPKG_I + str(self.addon_dir() / "current.deb")])
        self.assertEqual(self.status()[0].split("\t")[2:4], ["installed", "1.0"])

    def test_check_with_the_id_of_a_manual_source_stores_nothing(self):
        # Ruling R1: a stored next.* would be installed at the next boot with
        # nobody pressing anything.
        self.demo_tile()
        self.publish("1.0")
        self.tick("10-demo")
        self.assertEqual(self.run_cmd("check", "10-demo"), 0)
        self.assertEqual(self.files(), [], "not installed: still nothing stored")
        self.assertEqual(self.dpkg_calls(), [])

        self.assertEqual(self.run_cmd("install", "10-demo"), 0, "Install now is manual")
        self.publish("1.0", "1.1")
        self.assertEqual(self.run_cmd("check", "10-demo"), 0)
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertEqual(len(self.dpkg_calls()), 1)
        self.assertNotIn(("GET", "/dl/tf-demo_1.1_all.deb"), self.srv.requests)
        self.assertTrue(any("Demo 1.1" in l and "Update now" in l for l in self.log_lines()),
                        self.log_lines())

        self.assertEqual(self.run_cmd("update", "10-demo"), 0, "Update now is manual")
        self.assertEqual(self.meta("current")["version"], "1.1")

    def test_automatic_download_never_follows_http(self):
        # https page and link, but the download redirects to http.
        self.demo_tile()
        self.publish("1.0")
        with mock.patch.object(ad, "_secure", lambda url: True):
            self.run_cmd("install", "10-demo")
        self.publish("1.0", "1.1")
        self.tick("10-demo")
        calls = []
        real = ad._request

        def spy(method, url, seen, https_only=False):
            calls.append((method, https_only))
            return real(method, url, seen, False)

        with mock.patch.object(ad, "auto_allowed", lambda src, seen: True), \
                mock.patch.object(ad, "_request", spy):
            self.run_cmd("check")
        self.assertIn(("GET", True), calls, "the automatic download must be https only")


class TestStatusAndLog(AddonEnv):
    def test_status_columns(self):
        self.demo_tile()
        self.publish("1.0")
        self.assertEqual(self.run_cmd("status"), 0)
        self.assertEqual(self.status(), ["10-demo\t0\tabsent\t-\t-\t1\t-"])

        (self.bin / "dpkg.fail").touch()
        self.assertEqual(self.run_cmd("install", "10-demo"), 1)
        self.assertEqual(self.status(), [
            "10-demo\t1\tinstall-failed\t-\t-\t1\t"
            "Demo: install failed: dpkg: error: boom"])
        (self.bin / "dpkg.fail").unlink()

        self.assertEqual(self.run_cmd("install", "10-demo"), 0)
        self.assertEqual(self.status(), ["10-demo\t1\tinstalled\t1.0\t-\t1\tDemo 1.0 installed"])

        self.publish("1.0", "1.1")
        self.run_cmd("check")
        self.assertEqual(self.status(), [
            "10-demo\t1\tpending\t1.0\t1.1\t1\t"
            "Demo 1.1 stored on the boot stick, active at next boot"])

        self.srv.routes["/dl/"] = Route(404)
        self.run_cmd("check")
        self.assertEqual(self.status(), [
            "10-demo\t1\tfailed\t1.0\t1.1\t1\t"
            "Demo: check failed: %s/dl/ answered HTTP 404" % self.srv.base])

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(ad.main(["status"]), 0)
        self.assertEqual(out.getvalue().splitlines(), self.status())

    def test_status_via_the_environment(self):
        self.demo_tile()
        env = dict(os.environ, TF_ADDON_ESP=str(self.esp), TF_ADDON_RUN=str(self.run_dir),
                   TF_ADDON_LOCK=str(self.lock), TF_ADDON_ROOT=str(self.root),
                   TF_ADDON_HOME=str(self.home), TF_ADDON_DPKG=str(self.bin / "dpkg"),
                   TF_ADDON_TILES=str(self.tiles))
        r = subprocess.run([str(SCRIPT), "status"], env=env, capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stdout, "10-demo\t0\tabsent\t-\t-\t0\t-\n")
        self.assertEqual(self.status(), ["10-demo\t0\tabsent\t-\t-\t0\t-"])

    def test_log_kept_to_500_lines(self):
        with contextlib.redirect_stdout(io.StringIO()):
            for i in range(600):
                ad.log("line %d" % i)
            ad.log("tab\there\nand newline")
        lines = (self.run_dir / "log").read_text().splitlines()
        self.assertEqual(len(lines), 500)
        self.assertRegex(lines[0], r"^[0-9]+\tline 101$")
        self.assertRegex(lines[-1], r"^[0-9]+\ttab here and newline$")

    def test_unknown_command_is_usage(self):
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(self.run_cmd("frobnicate"), 2)

    def test_tile_name_that_is_no_id_is_logged_and_skipped(self):
        (self.tiles / "my tool.desktop").write_text(
            "[Desktop Entry]\nName=T\nExec=t\nX-TF-Source-URL=https://e.example/t.deb\n")
        self.assertEqual(self.run_cmd("check"), 0)
        self.assertEqual(self.status(), [])
        self.assertTrue(any("my tool.desktop" in l for l in self.log_lines()))

    def test_ids_follow_the_menus_rule(self):
        # src/thinfloe-menu/src/addons.rs checks valid_id against this table.
        rows = [l.split("\t", 1) for l in
                (REPO / "test" / "unit" / "addon-ids.tsv").read_text().splitlines()
                if not l.startswith("#")]
        self.assertGreaterEqual(len(rows), 20)
        for verdict, aid in rows:
            with self.subTest(aid=aid):
                self.assertEqual(bool(ad.ID_RE.fullmatch(aid)), verdict == "accept")
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(self.run_cmd("install", "y" * 65), 2)

    def test_the_addons_taken_match_the_menus_settings_rows(self):
        # The same tiles as menu.rs settings_rows_follow_the_tools_id_rule.
        def tile(d, n):
            (d / ("%s.desktop" % n)).write_text(
                "[Desktop Entry]\nName=%s\nExec=true\n"
                "X-TF-Source-URL=https://e.example/%s.deb\n" % (n, n))
        tile(self.tiles, "10-thinlinc")
        menu = self.home / ".config" / "thinfloe" / "menu"
        menu.mkdir(parents=True)
        for n in ["10-ThinLinc", "my+app", "-lead", "trail.", "A-B", "a-b",
                  "x" * 64, "y" * 65]:
            tile(menu, n)
        self.assertEqual(self.run_cmd("status"), 0)
        self.assertEqual(sorted(l.split("\t")[0] for l in self.status()),
                         sorted(["10-thinlinc", "A-B", "my+app", "x" * 64]))

    def test_install_of_an_unknown_id_fails(self):
        self.assertEqual(self.run_cmd("install", "99-nope"), 1)
        self.assertEqual(self.ticks(), [])


class TestTickFile(AddonEnv):
    def test_tick_file_owned_by_tfuser(self):
        self.demo_tile()
        self.publish("1.0")
        with mock.patch.object(ad, "tfuser_ids", lambda: (1000, 1001)), \
                mock.patch.object(ad.os, "fchown") as fchown:
            self.assertEqual(self.run_cmd("install", "10-demo"), 0)
        self.assertEqual(self.ticks(), ["10-demo"])
        owners = {c.args[1:] for c in fchown.call_args_list}
        self.assertEqual(owners, {(1000, 1001)})
        # ~/.config, ~/.config/thinfloe and the file itself
        self.assertEqual(len(fchown.call_args_list), 3)
        self.assertEqual(stat_mode(self.home / ".config/thinfloe/addons"), 0o644)

    def test_tick_file_never_written_through_a_symlink(self):
        self.demo_tile()
        self.publish("1.0")
        elsewhere = self.tmp / "elsewhere"
        elsewhere.mkdir()
        (self.home / ".config").mkdir()
        (self.home / ".config" / "thinfloe").symlink_to(elsewhere)
        self.assertEqual(self.run_cmd("install", "10-demo"), 1)
        self.assertEqual(os.listdir(elsewhere), [])
        self.assertEqual(self.files(), [])

class TestFixRound1(AddonEnv):
    def test_download_and_install_run_without_the_esp_lock(self):
        # R4: the download goes to RAM; esp.lock only covers the copy.
        self.demo_tile()
        self.publish("1.0")
        hold = threading.Event()
        self.srv.routes["/dl/tf-demo_1.0_all.deb"].hold = hold
        self.addCleanup(hold.set)
        rc = []
        t = threading.Thread(target=lambda: rc.append(ad.main(["install", "10-demo"])))
        with contextlib.redirect_stdout(io.StringIO()):
            t.start()
            deadline = time.monotonic() + 15
            while ("GET", "/dl/tf-demo_1.0_all.deb") not in self.srv.requests:
                self.assertLess(time.monotonic(), deadline, "the download never started")
                time.sleep(0.05)
            time.sleep(0.3)
            r = subprocess.run(["flock", "-n", str(self.lock), "true"])
            self.assertEqual(r.returncode, 0, "esp.lock is held during the download")
            hold.set()
            t.join(30)
        self.assertEqual(rc, [0])
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertEqual((self.bin / "dpkg.lockstate").read_text().split(), ["free"],
                         "dpkg -i must run without esp.lock")

    def test_no_memory_for_the_download(self):
        self.demo_tile()
        self.publish("1.0")
        run = str(self.run_dir)
        with mock.patch.object(ad, "free_bytes", lambda path: 10 * 1024 * 1024
                               if str(path).startswith(run) else 1 << 40):
            self.assertEqual(self.run_cmd("install", "10-demo"), 1)
        self.assertEqual(self.files(), [])
        self.assertEqual(self.status()[0].split("\t")[6],
                         "Demo: download failed: not enough memory for the download")
        self.assertEqual(os.listdir(self.run_dir / "dl"), [])

    def test_user_addon_differing_only_in_case_is_refused(self):
        self.demo_tile()
        self.publish("1.0")
        self.run_cmd("install", "10-demo")
        menu = self.home / ".config" / "thinfloe" / "menu"
        menu.mkdir(parents=True)
        (menu / "10-DEMO.desktop").write_text(
            "[Desktop Entry]\nName=Other\nExec=o\n"
            "X-TF-Source-URL=https://e.example/o.deb\n")
        self.assertEqual(self.run_cmd("remove", "10-DEMO"), 1)
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertEqual(len(self.dpkg_calls()), 1)
        self.assertEqual(self.ticks(), ["10-demo"])
        self.assertTrue(any("10-DEMO.desktop" in l and "10-demo.desktop" in l
                            for l in self.log_lines()), self.log_lines())
        self.assertEqual([l.split("\t")[0] for l in self.status()], ["10-demo"])
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(self.run_cmd("remove", "10-demo."), 2,
                             "a trailing dot is no id (vfat strips it)")

    def test_install_failed_survives_a_check_and_update_retries(self):
        self.demo_tile()
        self.publish("1.0")
        (self.bin / "dpkg.fail").touch()
        self.assertEqual(self.run_cmd("install", "10-demo"), 1)
        # The check installs the stored copy again (R20); it fails again.
        self.assertEqual(self.run_cmd("check"), 1)
        self.assertEqual(len(self.dpkg_calls()), 2)
        self.assertEqual(self.status()[0].split("\t")[2], "install-failed",
                         "an up-to-date check must not hide the failed install")
        (self.bin / "dpkg.fail").unlink()
        self.assertEqual(self.run_cmd("update", "10-demo"), 0)
        self.assertEqual(len(self.dpkg_calls()), 3, "update retries the current copy")
        self.assertEqual(self.status(), ["10-demo\t1\tinstalled\t1.0\t-\t1\tDemo 1.0 installed"])

    def test_install_failed_survives_storing_next(self):
        self.demo_tile()
        self.publish("1.0")
        (self.bin / "dpkg.fail").touch()
        self.run_cmd("install", "10-demo")
        self.publish("1.0", "1.1")
        self.assertEqual(self.run_cmd("check"), 1, "the retried install failed")
        self.assertEqual(self.files(), ["current.deb", "current.meta", "next.deb", "next.meta"])
        self.assertEqual(self.status()[0].split("\t")[2], "install-failed")

    def test_concurrent_ticks_are_not_lost(self):
        real = ad.read_ticks

        def slow():
            ids = real()
            time.sleep(0.02)
            return ids

        errors = []

        def add(i):
            try:
                ad.change_ticks(lambda t: t + ["t%d" % i])
            except Exception as e:  # noqa: BLE001 - reported below
                errors.append(e)

        with mock.patch.object(ad, "read_ticks", slow):
            threads = [threading.Thread(target=add, args=(i,)) for i in range(8)]
            for t in threads:
                t.start()
            for t in threads:
                t.join(30)
        self.assertEqual(errors, [])
        self.assertEqual(sorted(self.ticks()), sorted("t%d" % i for i in range(8)))

    def test_concurrent_run_files_do_not_collide(self):
        errors = []

        def write(i):
            try:
                for n in range(30):
                    ad.set_result("10-demo", None, "m%d-%d" % (i, n))
            except Exception as e:  # noqa: BLE001 - reported below
                errors.append(e)

        threads = [threading.Thread(target=write, args=(i,)) for i in range(8)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(30)
        self.assertEqual(errors, [])
        self.assertTrue(ad.get_result("10-demo")["message"].startswith("m"))
        self.assertEqual(stat_mode(self.run_dir / "results" / "10-demo.json"), 0o644)
        self.assertEqual([n for n in os.listdir(self.run_dir / "results")], ["10-demo.json"])

    def test_concurrent_installs_are_serialized(self):
        self.demo_tile()
        self.publish("1.0")
        (self.bin / "dpkg.slow").touch()
        rc = []
        threads = [threading.Thread(target=lambda: rc.append(ad.main(["install", "10-demo"])))
                   for _ in range(2)]
        with contextlib.redirect_stdout(io.StringIO()):
            for t in threads:
                t.start()
            for t in threads:
                t.join(60)
        self.assertEqual(rc, [0, 0])
        self.assertFalse((self.bin / "dpkg.clash").exists(), "dpkg ran twice at once")
        self.assertEqual(len(self.dpkg_calls()), 1, "the waiting run reinstalls nothing")
        self.assertEqual(self.status(), ["10-demo\t1\tinstalled\t1.0\t-\t1\tDemo 1.0 installed"])

    def test_dpkg_keeps_changed_conffiles(self):
        self.demo_tile()
        self.publish("1.0")
        self.run_cmd("install", "10-demo")
        self.assertTrue(self.dpkg_calls()[0].startswith("--force-confdef --force-confold -i "))

    def test_hook_runs_after_install_and_its_failure_fails_the_install(self):
        self.demo_tile()
        self.publish("1.0")
        hooks = self.root / "hooks"
        hooks.mkdir()
        hook = hooks / "10-demo"
        hook.write_text("#!/bin/sh\necho ran > %s\n" % (self.root / "hook-ran"))
        hook.chmod(0o755)
        with mock.patch.object(ad, "HOOKS_DIR", str(hooks)):
            self.assertEqual(self.run_cmd("install", "10-demo"), 0)
        self.assertEqual((self.root / "hook-ran").read_text(), "ran\n")
        self.assertEqual(self.status()[0].split("\t")[2], "installed")
        hook.write_text("#!/bin/sh\necho first >&2\necho broken hook >&2\nexit 7\n")
        self.publish("1.1")
        with mock.patch.object(ad, "HOOKS_DIR", str(hooks)):
            self.run_cmd("update", "10-demo")
        fields = self.status()[0].split("\t")
        self.assertEqual(fields[2], "install-failed")
        self.assertIn("broken hook", fields[6])


class TestAutomaticHttps(AddonEnv):
    def setUp(self):
        super().setUp()
        p = mock.patch.object(ad, "_secure", lambda url: url.lower().startswith("https:"))
        p.start()
        self.addCleanup(p.stop)

    def test_automatic_check_never_asks_an_http_hop(self):
        url = "https://e.example/x.deb"
        hop = "http://mirror.example/x.deb"
        (self.tiles / "40-x.desktop").write_text(
            "[Desktop Entry]\nName=X\nExec=x\nX-TF-Source-URL=%s\n" % url)
        self.tick("40-x")
        opener = FakeRedirectOpener({url: hop})
        with mock.patch.object(ad, "_opener", opener):
            self.assertEqual(self.run_cmd("check"), 1)
        self.assertEqual(opener.asked, [url], "the http hop is never requested")
        self.assertTrue(any(hop in l for l in self.log_lines()), self.log_lines())
        self.assertEqual(self.files("40-x"), [])


class TestPromote(unittest.TestCase):
    """promote() at every point a power loss can hit: each slot is either
    whole (its payload matches its meta) or missing, never mixed."""

    class Crash(Exception):
        pass

    def setUp(self):
        self.d = Path(tempfile.mkdtemp(prefix="test-addon-promote-"))
        self.addCleanup(shutil.rmtree, self.d)

    def fill(self, cur_fmt: str, nxt_fmt: str):
        for name in os.listdir(self.d):
            os.unlink(self.d / name)
        for slot, fmt, version in (("current", cur_fmt, "1.0"), ("next", nxt_fmt, "1.1")):
            (self.d / ("%s.%s" % (slot, fmt))).write_bytes(b"payload-" + version.encode())
            (self.d / (slot + ".meta")).write_text(
                "version=%s\nformat=%s\nfingerprint=%s\n" % (version, fmt, version))

    def whole_or_missing(self, slot: str) -> Optional[str]:
        st = ad.stored(str(self.d), slot)
        if st is None:
            return None
        self.assertEqual(Path(st.path).read_bytes(), b"payload-" + st.meta["version"].encode(),
                         "%s: payload and meta disagree" % slot)
        return st.meta["version"]

    def test_every_crash_point(self):
        real_replace, real_unlink = os.replace, os.unlink
        for cur_fmt, nxt_fmt in (("deb", "deb"), ("tar", "deb")):
            for crash_at in range(1, 12):
                with self.subTest(formats=(cur_fmt, nxt_fmt), crash_at=crash_at):
                    self.fill(cur_fmt, nxt_fmt)
                    count = [0]

                    def step(real):
                        def op(*args, **kwargs):
                            count[0] += 1
                            if count[0] == crash_at:
                                raise self.Crash()
                            return real(*args, **kwargs)
                        return op

                    crashed = False
                    with mock.patch.object(ad.os, "replace", step(real_replace)), \
                            mock.patch.object(ad.os, "unlink", step(real_unlink)):
                        try:
                            ad.promote(str(self.d))
                        except self.Crash:
                            crashed = True
                    self.whole_or_missing("current")
                    self.whole_or_missing("next")
                    if not crashed:
                        self.assertEqual(self.whole_or_missing("current"), "1.1")
                        self.assertEqual(sorted(os.listdir(self.d)),
                                         ["current.%s" % nxt_fmt, "current.meta"])
                        break



class TestBoot(AddonEnv):
    """thinfloe-addon boot: the first boot from 3.x ticks add-ons, next.*
    becomes current.*, every ticked current.* is installed, all from the
    boot stick and with no network."""

    def setUp(self):
        super().setUp()
        self.state = self.tmp / "state"
        self.state.mkdir()
        p = mock.patch.object(ad, "STATE_DIR", str(self.state), create=True)
        p.start()
        self.addCleanup(p.stop)
        self.marker = self.esp / "config" / "addons-seeded"

    def reboot(self):
        """What a reboot leaves: the ESP and the home, a new /run."""
        shutil.rmtree(self.run_dir)
        for name in ("dpkg.log", "dpkg.lockstate"):
            with contextlib.suppress(FileNotFoundError):
                (self.bin / name).unlink()

    def stored_demo(self):
        self.demo_tile()
        self.publish("1.0")
        self.assertEqual(self.run_cmd("install", "10-demo"), 0)
        self.reboot()

    def client_tiles(self):
        """Two shipped add-ons with a config path, one without."""
        for name, config in (("10-thinlinc", ".thinlinc"), ("20-oxulnk", ".config/oxulnk")):
            (self.tiles / (name + ".desktop")).write_text(
                "[Desktop Entry]\nName=%s\nExec=%s\n"
                "X-TF-Source-URL=%s/%s.deb\nX-TF-Source-Config=%s\n"
                % (name, name, self.srv.base, name, config))
        self.demo_tile()

    def tick_file(self) -> Path:
        return self.home / ".config" / "thinfloe" / "addons"

    def test_boot_installs_from_esp_with_no_network(self):
        self.stored_demo()
        connects = []

        def no_network(sock, address):
            connects.append(address)
            raise OSError("network is down")

        with mock.patch("socket.socket.connect", no_network):
            self.assertEqual(self.run_cmd("boot"), 0, self.log_lines())
        self.assertEqual(connects, [])
        self.assertEqual(self.dpkg_calls(), [DPKG_I + str(self.addon_dir() / "current.deb")])
        self.assertEqual(self.status(), ["10-demo\t1\tinstalled\t1.0\t-\t1\tDemo 1.0 installed"])

    def test_boot_promotes_next(self):
        self.demo_tile()
        self.publish("1.0")
        self.run_cmd("install", "10-demo")
        self.publish("1.0", "1.1")
        self.run_cmd("check")
        self.reboot()
        before = len(self.srv.requests)
        self.assertEqual(self.run_cmd("boot"), 0, self.log_lines())
        self.assertEqual(self.srv.requests[before:], [])
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertEqual(self.meta("current")["version"], "1.1")
        self.assertEqual((self.addon_dir() / "current.deb").read_bytes(), self.debs["1.1"])
        self.assertEqual(self.dpkg_calls(), [DPKG_I + str(self.addon_dir() / "next.deb")])
        self.assertEqual(self.status(), ["10-demo\t1\tinstalled\t1.1\t-\t1\tDemo 1.1 installed"])

    def stored_next(self):
        self.demo_tile()
        self.publish("1.0")
        self.run_cmd("install", "10-demo")
        self.publish("1.0", "1.1")
        self.run_cmd("check")
        self.reboot()

    def test_boot_failed_next_keeps_current(self):
        self.stored_next()
        (self.bin / "dpkg.fail-on").write_text("next.deb")
        self.assertEqual(self.run_cmd("boot"), 1)
        d = self.addon_dir()
        self.assertEqual(self.dpkg_calls(), [DPKG_I + str(d / "next.deb"),
                                             DPKG_I + str(d / "current.deb")])
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertEqual((d / "current.deb").read_bytes(), self.debs["1.0"])
        self.assertEqual(self.meta("current")["version"], "1.0")
        self.assertEqual(self.status(), [
            "10-demo\t1\tinstalled\t1.0\t-\t1\t"
            "Demo 1.1 could not be installed: dpkg: error: boom; Demo 1.0 installed"])
        self.assertIn("Demo 1.1 could not be installed: dpkg: error: boom", self.log_lines())

    def test_boot_failed_next_without_current(self):
        self.stored_next()
        for name in ("current.meta", "current.deb"):
            (self.addon_dir() / name).unlink()
        (self.bin / "dpkg.fail-on").write_text("next.deb")
        self.assertEqual(self.run_cmd("boot"), 1)
        self.assertEqual(len(self.dpkg_calls()), 1)
        self.assertEqual(self.files(), [])
        self.assertEqual(self.status(), [
            "10-demo\t1\tinstall-failed\t-\t-\t1\t"
            "Demo: install of Demo 1.1 failed: dpkg: error: boom"])

    def test_hung_install_is_killed_and_the_next_addon_installs(self):
        self.stored_demo()
        (self.tiles / "20-other.desktop").write_text(
            "[Desktop Entry]\nName=Other\nExec=other\n"
            "X-TF-Source-URL=%s/other.deb\n" % self.srv.base)
        other = self.addon_dir("20-other")
        other.mkdir(parents=True)
        (other / "current.deb").write_bytes(b"deb")
        (other / "current.meta").write_text(
            "version=2.0\nfingerprint=2.0\nurl=x\nformat=deb\npackage=tf-other\n")
        self.tick("10-demo", "20-other")
        (self.bin / "dpkg.hang-on").write_text("10-demo")
        start = time.monotonic()
        with mock.patch.object(ad, "BOOT_TOOL_TIMEOUT", 1, create=True):
            self.assertEqual(self.run_cmd("boot"), 1)
        self.assertLess(time.monotonic() - start, 10)
        self.assertEqual(self.ticks(), ["10-demo", "20-other"])
        st = {l.split("\t")[0]: l.split("\t") for l in self.status()}
        self.assertEqual(st["10-demo"][2], "install-failed")
        self.assertIn("took longer than 1 s", st["10-demo"][6])
        self.assertEqual(st["20-other"][2:4], ["installed", "2.0"])
        self.assertEqual(ad.TOOL_TIMEOUT, 600)
        self.assertIsNone(ad._boot_deadline, "boot's deadline leaked out of boot")

    def test_boot_stops_when_its_time_is_spent(self):
        self.stored_demo()
        for aid, name in (("20-other", "Other"), ("30-third", "Third")):
            (self.tiles / (aid + ".desktop")).write_text(
                "[Desktop Entry]\nName=%s\nExec=x\nX-TF-Source-URL=%s/x.deb\n"
                % (name, self.srv.base))
            d = self.addon_dir(aid)
            d.mkdir(parents=True)
            (d / "current.deb").write_bytes(b"deb")
            (d / "current.meta").write_text(
                "version=2.0\nfingerprint=2.0\nurl=x\nformat=deb\npackage=tf-other\n")
        self.tick("10-demo", "20-other", "30-third")
        (self.bin / "dpkg.hang-on").write_text("10-demo")
        start = time.monotonic()
        with mock.patch.object(ad, "BOOT_BUDGET", 1.5, create=True), \
                mock.patch.object(ad, "BOOT_TOOL_TIMEOUT", 25):
            self.assertEqual(self.run_cmd("boot"), 1)
        self.assertLess(time.monotonic() - start, 10)
        self.assertEqual(len(self.dpkg_calls()), 1, "nothing ran after the time was spent")
        self.assertIn("the time for add-ons at boot is used up; not installed: Other, Third",
                      self.log_lines())
        st = {l.split("\t")[0]: l.split("\t") for l in self.status()}
        self.assertIn("took longer than", st["10-demo"][6])
        for aid, name in (("20-other", "Other"), ("30-third", "Third")):
            self.assertEqual(st[aid][2:4], ["install-failed", "-"])
            self.assertEqual(st[aid][6], "%s: not installed at boot: the time for add-ons "
                                         "was used up; the check after boot installs it" % name)
        self.assertIsNone(ad._boot_deadline)
        # The check after boot installs all three from the boot stick
        # (ruling R20, the R13 gap), with no Retry pressed.
        (self.bin / "dpkg.hang-on").unlink()
        (self.bin / "dpkg.log").unlink()
        self.run_cmd("check")
        self.assertEqual(sorted(self.dpkg_calls()), sorted(
            DPKG_I + str(self.addon_dir(aid) / "current.deb")
            for aid in ("10-demo", "20-other", "30-third")))
        st = {l.split("\t")[0]: l.split("\t") for l in self.status()}
        self.assertEqual(st["10-demo"][3], "1.0")
        for aid in ("20-other", "30-third"):
            self.assertEqual(st[aid][3], "2.0", st[aid])

    def test_boot_finishes_a_promote_cut_by_a_power_loss(self):
        # m4: promote's first rename done, its second not.
        self.stored_next()
        d = self.addon_dir()
        (d / "current.meta").unlink()
        os.replace(d / "next.deb", d / "current.deb")
        self.assertIsNone(ad.stored(str(d), "current"))
        self.assertEqual(self.run_cmd("boot"), 0, self.log_lines())
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertEqual(self.meta("current")["version"], "1.1")
        self.assertEqual((d / "current.deb").read_bytes(), self.debs["1.1"])
        self.assertEqual(self.dpkg_calls(), [DPKG_I + str(d / "current.deb")])
        self.assertIn("10-demo: version 1.1 was being made current when the power went; "
                      "it is current now", self.log_lines())

    def test_other_half_slots_stay_incomplete(self):
        self.stored_next()
        d = self.addon_dir()
        cur_meta = (d / "current.meta").read_text()
        # Cut before promote's payload rename: next.* is whole, current.meta
        # gone; nothing is finished, and boot installs next.* as usual.
        (d / "current.meta").unlink()
        with ad.esp_lock():
            self.assertIsNone(ad.finish_promote(str(d)))
        self.assertIsNotNone(ad.stored(str(d), "next"))
        # Payloads without a meta: incomplete, and nothing is renamed.
        (d / "next.meta").unlink()
        with ad.esp_lock():
            self.assertIsNone(ad.finish_promote(str(d)))
        self.assertEqual(self.files(), ["current.deb", "next.deb"])
        self.assertIsNone(ad.stored(str(d), "current"))
        self.assertIsNone(ad.stored(str(d), "next"))
        # A meta of the other format does not pair with this payload.
        (d / "next.meta").write_text(cur_meta.replace("format=deb", "format=tar"))
        (d / "next.deb").unlink()
        with ad.esp_lock():
            self.assertIsNone(ad.finish_promote(str(d)))
        self.assertIsNone(ad.stored(str(d), "current"))

    def test_remove_deletes_the_metas_first(self):
        self.stored_next()
        with mock.patch.object(ad.shutil, "rmtree", side_effect=OSError(5, "I/O error")):
            self.assertEqual(self.run_cmd("remove", "10-demo"), 1)
        self.assertEqual([f for f in self.files() if f.endswith(".meta")], [])

    def test_a_timeout_at_boot_keeps_next(self):
        # m7: running out of time says nothing about the copy. next.* stays,
        # current.* gets no time it does not have, and the check after boot
        # installs next.* with the full timeout.
        self.stored_next()
        (self.bin / "dpkg.hang-on").write_text("next.deb")
        with mock.patch.object(ad, "BOOT_TOOL_TIMEOUT", 1, create=True):
            self.assertEqual(self.run_cmd("boot"), 1)
        d = self.addon_dir()
        self.assertEqual(self.dpkg_calls(), [DPKG_I + str(d / "next.deb")])
        self.assertEqual(self.files(), ["current.deb", "current.meta", "next.deb", "next.meta"])
        self.assertEqual(self.status(), [
            "10-demo\t1\tinstall-failed\t-\t1.1\t1\tDemo: Demo 1.1 not installed at boot: "
            + str(self.bin / "dpkg") + " took longer than 1 s; the check after boot installs it"])
        (self.bin / "dpkg.hang-on").unlink()
        (self.bin / "dpkg.log").unlink()
        before = len(self.srv.requests)
        self.assertEqual(self.run_cmd("check"), 0, self.log_lines())
        self.assertEqual(self.dpkg_calls(), [DPKG_I + str(d / "next.deb")])
        self.assertEqual(self.files(), ["current.deb", "current.meta"])
        self.assertEqual(self.meta("current")["version"], "1.1")
        self.assertNotIn(("GET", "/dl/tf-demo_1.1_all.deb"), self.srv.requests[before:])
        self.assertEqual(self.status()[0].split("\t")[2:4], ["installed", "1.1"])

    def test_the_check_installs_a_copy_whose_boot_install_failed(self):
        self.stored_demo()
        (self.bin / "dpkg.fail-on").write_text("current.deb")
        self.assertEqual(self.run_cmd("boot"), 1)
        (self.bin / "dpkg.fail-on").unlink()
        self.assertEqual(self.run_cmd("check"), 0, self.log_lines())
        self.assertEqual(len(self.dpkg_calls()), 2)
        self.assertEqual(self.status(), ["10-demo\t1\tinstalled\t1.0\t-\t1\tDemo: up to date"])
        # Installed now: the next check installs nothing.
        self.assertEqual(self.run_cmd("check"), 0)
        self.assertEqual(len(self.dpkg_calls()), 2)

    def test_boot_promotes_under_the_esp_lock(self):
        self.demo_tile()
        self.publish("1.0")
        self.run_cmd("install", "10-demo")
        self.publish("1.0", "1.1")
        self.run_cmd("check")
        self.reboot()
        fd = os.open(str(self.lock), os.O_RDWR | os.O_CREAT, 0o644)
        self.addCleanup(os.close, fd)
        fcntl.flock(fd, fcntl.LOCK_EX)
        rc = []
        t = threading.Thread(target=lambda: rc.append(ad.main(["boot"])))
        with contextlib.redirect_stdout(io.StringIO()):
            t.start()
            time.sleep(0.5)
            self.assertIn("next.meta", self.files(), "promoted under a held lock")
            fcntl.flock(fd, fcntl.LOCK_UN)
            t.join(30)
        self.assertEqual(rc, [0])
        self.assertEqual(self.files(), ["current.deb", "current.meta"])

    def test_boot_without_a_stored_copy_installs_nothing(self):
        self.demo_tile()
        self.tick("10-demo")
        self.assertEqual(self.run_cmd("boot"), 0)
        self.assertEqual(self.dpkg_calls(), [])
        self.assertEqual(self.srv.requests, [])
        self.assertEqual(self.status()[0].split("\t")[:3], ["10-demo", "1", "absent"])

    def test_boot_installs_only_ticked_addons(self):
        self.stored_demo()
        self.tick()
        self.assertEqual(self.run_cmd("boot"), 0)
        self.assertEqual(self.dpkg_calls(), [])

    def step_recorder(self, status: int = 0) -> Path:
        """A plymouth-step that logs its arguments, one call per line."""
        rec = self.bin / "plymouth-step"
        rec.write_text('#!/bin/sh\necho "$*" >> "$0.log"\nexit %d\n' % status)
        rec.chmod(0o755)
        p = mock.patch.object(ad, "PLYMOUTH_STEP", str(rec))
        p.start()
        self.addCleanup(p.stop)
        return Path(str(rec) + ".log")

    def test_boot_sends_a_step_per_addon(self):
        steps = self.step_recorder()
        for name, label in (("10-thinlinc", "ThinLinc"), ("20-oxulnk", "OxuLnk")):
            (self.tiles / (name + ".desktop")).write_text(
                "[Desktop Entry]\nName=%s\nExec=%s\nX-TF-Source-URL=%s/%s.deb\n"
                % (label, name, self.srv.base, name))
        self.tick("10-thinlinc", "20-oxulnk")
        self.assertEqual(self.run_cmd("boot"), 0, self.log_lines())
        self.assertEqual(steps.read_text().splitlines(),
                         ["status Installing ThinLinc", "status Installing OxuLnk"])

    def test_boot_step_failure_is_ignored(self):
        self.stored_demo()
        steps = self.step_recorder(status=1)
        self.assertEqual(self.run_cmd("boot"), 0, self.log_lines())
        self.assertEqual(steps.read_text().splitlines(), ["status Installing Demo"])
        self.assertEqual(self.dpkg_calls(), [DPKG_I + str(self.addon_dir() / "current.deb")])

    def test_seed_from_home_renamed(self):
        self.client_tiles()
        (self.state / "home-renamed").write_text(".dbrrg-environment\n")
        (self.state / "home-restore").write_text("restored\n")
        (self.home / ".thinlinc").mkdir()
        self.assertEqual(self.run_cmd("boot"), 0, self.log_lines())
        self.assertEqual(self.ticks(), ["10-thinlinc"])
        self.assertTrue(self.marker.exists())
        self.assertEqual(self.srv.requests, [])
        self.assertIn("10-thinlinc ticked: this home came from ThinFloe 3.x", self.log_lines())
        self.assertNotIn("ticked: the boot stick", "\n".join(self.log_lines()))

    def test_seed_absent_archive_after_3x_upgrade(self):
        self.client_tiles()
        (self.state / "home-restore").write_text("absent\n")
        (self.esp / "tl.old").mkdir()
        self.assertEqual(self.run_cmd("boot"), 0, self.log_lines())
        self.assertEqual(self.ticks(), ["10-thinlinc", "20-oxulnk"])
        self.assertTrue(self.marker.exists())
        self.assertIn("10-thinlinc ticked: the boot stick was upgraded from ThinFloe 3.x "
                      "without a saved home", self.log_lines())

    def test_seed_fires_once(self):
        self.client_tiles()
        (self.state / "home-restore").write_text("absent\n")
        (self.esp / "tl.old").mkdir()
        self.marker.parent.mkdir()
        self.marker.write_text("1\n")
        self.assertEqual(self.run_cmd("boot"), 0)
        self.assertFalse(self.tick_file().exists())

    def test_seed_keeps_an_existing_tick_file(self):
        self.client_tiles()
        (self.state / "home-renamed").write_text(".dbrrg-environment\n")
        (self.home / ".thinlinc").mkdir()
        self.tick()
        self.assertEqual(self.run_cmd("boot"), 0)
        self.assertEqual(self.tick_file().read_text(), "")

    def test_fresh_install_ticks_nothing(self):
        self.client_tiles()
        (self.state / "home-restore").write_text("absent\n")
        (self.home / ".thinlinc").mkdir()
        self.assertEqual(self.run_cmd("boot"), 0)
        self.assertFalse(self.tick_file().exists())
        self.assertTrue(self.marker.exists(), "every USB boot writes the marker")

    def test_fresh_4x_stick_upgrade_does_not_seed(self):
        self.client_tiles()
        (self.state / "home-restore").write_text("absent\n")
        self.assertEqual(self.run_cmd("boot"), 0)
        self.assertTrue(self.marker.exists())
        self.reboot()
        # A 4.x upgrade rotated tl/ to tl.old/; the home was never saved.
        (self.esp / "tl.old").mkdir()
        self.assertEqual(self.run_cmd("boot"), 0)
        self.assertFalse(self.tick_file().exists())

    def test_marker_written_when_a_tick_file_blocks_seeding(self):
        self.client_tiles()
        (self.state / "home-renamed").write_text(".dbrrg-environment\n")
        self.tick("10-demo")
        self.assertEqual(self.run_cmd("boot"), 0)
        self.assertEqual(self.ticks(), ["10-demo"])
        self.assertTrue(self.marker.exists())

    def test_boot_on_netboot_does_nothing(self):
        cache = self.tmp / "cache"
        self.stored_demo()
        before = len(self.srv.requests)
        shutil.copytree(self.addon_dir(), cache / "10-demo")
        (self.state / "home-restore").write_text("absent\n")
        (self.esp / "tl.old").mkdir()
        with mock.patch.object(ad, "esp_mounted", lambda: False), \
                mock.patch.object(ad, "CACHE_DIR", str(cache)):
            self.assertEqual(self.run_cmd("boot"), 0)
        self.assertEqual(self.dpkg_calls(), [])
        self.assertFalse(self.marker.exists())
        self.assertEqual(self.ticks(), ["10-demo"])
        self.assertEqual(self.srv.requests[before:], [])

    def test_netboot_seeds_from_home_renamed(self):
        self.client_tiles()
        (self.state / "home-renamed").write_text(".dbrrg-environment\n")
        (self.home / ".config" / "oxulnk").mkdir(parents=True)
        with mock.patch.object(ad, "esp_mounted", lambda: False), \
                mock.patch.object(ad, "CACHE_DIR", str(self.tmp / "cache")):
            self.assertEqual(self.run_cmd("boot"), 0)
        self.assertEqual(self.ticks(), ["20-oxulnk"])
        self.assertFalse(self.marker.exists())
        self.assertEqual(self.dpkg_calls(), [])
        self.assertEqual(self.srv.requests, [])

    def test_install_failure_hides_tile(self):
        self.stored_demo()
        (self.bin / "dpkg.fail").touch()
        self.assertEqual(self.run_cmd("boot"), 1)
        self.assertEqual(self.status(), [
            "10-demo\t1\tinstall-failed\t-\t-\t1\tDemo: install failed: dpkg: error: boom"])
        self.assertEqual(self.files(), ["current.deb", "current.meta"])



def stat_mode(p: Path) -> int:
    return p.stat().st_mode & 0o777


class TestAptStore(AddonEnv):
    def test_apt_install_downloads_with_apt(self):
        if not shutil.which("dpkg-scanpackages"):
            self.skipTest("dpkg-scanpackages missing (install dpkg-dev)")
        repo = self.tmp / "repo"
        repo.mkdir()
        deb = build_deb(self.tmp, "1.10-1")
        (repo / "tf-demo_1.10-1_all.deb").write_bytes(deb)
        packages = subprocess.run(["dpkg-scanpackages", ".", "/dev/null"], cwd=repo,
                                  check=True, capture_output=True).stdout
        self.srv.routes["/flat/Packages"] = Route(200, body=packages)
        self.srv.routes["/flat/tf-demo_1.10-1_all.deb"] = Route(200, body=deb)
        (self.tiles / "30-apt.desktop").write_text(
            "[Desktop Entry]\nName=AptDemo\nExec=demo\n"
            "X-TF-Source-Apt=%s/flat ./\nX-TF-Source-Package=tf-demo\n" % self.srv.base)
        self.assertEqual(self.run_cmd("install", "30-apt"), 0, self.log_lines())
        self.assertEqual((self.addon_dir("30-apt") / "current.deb").read_bytes(), deb)
        m = self.meta("current", "30-apt")
        self.assertEqual((m["version"], m["package"], m["fingerprint"]),
                         ("1.10-1", "tf-demo", "tf-demo=1.10-1"))
        self.assertEqual(len(self.dpkg_calls()), 1)

    def test_apt_dirs_are_open_to_the_apt_sandbox(self):
        tmp = self.tmp / "aptdir"
        tmp.mkdir(mode=0o700)
        for d in ("lists/partial", "cache/archives/partial", "download"):
            (tmp / d).mkdir(parents=True)
        key = tmp / "key.asc"
        key.write_text("k")
        key.chmod(0o600)
        apt_user = mock.Mock(pw_uid=100, pw_gid=65534)
        with mock.patch.object(ad.os, "geteuid", lambda: 0), \
                mock.patch.object(ad.pwd, "getpwnam", lambda n: apt_user), \
                mock.patch.object(ad.os, "chown") as chown:
            ad._apt_sandbox(str(tmp))
        self.assertEqual(stat_mode(tmp), 0o755)
        self.assertEqual(stat_mode(key), 0o644)
        chowned = {c.args[0] for c in chown.call_args_list}
        self.assertEqual(chowned, {str(tmp / "lists/partial"),
                                   str(tmp / "cache/archives/partial"),
                                   str(tmp / "download")})
        for c in chown.call_args_list:
            self.assertEqual(c.args[1:], (100, 0))

if __name__ == "__main__":
    unittest.main()
