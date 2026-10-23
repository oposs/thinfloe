#!/usr/bin/env python3
"""Offline tests for thinfloe-addon: reading add-on definitions and checking
their sources.

thinfloe-addon has no .py extension, so it is loaded through importlib like
thinfloe-upgrade in test_upgrade_image.py. Its entry point is guarded, so
importing it does nothing.

Every web source is served by an http.server bound to 127.0.0.1 in a thread.
dpkg and apt are the real ones of the dev host; the APT check runs
unprivileged against a flat repository built here and never touches the
system's package lists.
"""

import http.server
import importlib.machinery
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
                 body: bytes = b""):
        self.status = status
        self.headers = headers or {}
        self.body = body


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
                if with_body:
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


if __name__ == "__main__":
    unittest.main()
