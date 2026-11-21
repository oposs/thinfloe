"""Tests for scripts/run-unit-tests.py, the parallel unit test runner."""
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

RUNNER = Path(__file__).resolve().parents[2] / "scripts" / "run-unit-tests.py"

PASS = """import unittest
class T(unittest.TestCase):
    def test_ok(self):
        pass
"""
FAIL = """import unittest
class T(unittest.TestCase):
    def test_bad(self):
        self.fail("deliberate failure")
"""


class RunnerTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)

    def module(self, name, body):
        (self.dir / name).write_text(body)

    def run_runner(self, *args, directory=None):
        d = self.dir if directory is None else directory
        return subprocess.run(
            [sys.executable, str(RUNNER), *args, str(d)],
            capture_output=True, text=True, timeout=120)

    def test_all_pass_exits_zero(self):
        self.module("test_a.py", PASS)
        self.module("test_b.py", PASS)
        p = self.run_runner()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(p.stdout.splitlines()[-1], "unit: 2 modules, 0 failed")

    def test_one_failure_exits_one(self):
        self.module("test_a.py", PASS)
        self.module("test_b.py", FAIL)
        p = self.run_runner()
        self.assertEqual(p.returncode, 1)
        lines = p.stdout.splitlines()
        self.assertTrue(lines[-1].endswith("1 failed: test_b.py"), lines[-1])
        head = next(i for i, l in enumerate(lines)
                    if l.startswith("=== test_b.py: FAIL"))
        self.assertIn("deliberate failure", "\n".join(lines[head:]))

    def test_import_error_counts_as_failure(self):
        self.module("test_a.py", "def broken(:\n")
        p = self.run_runner()
        self.assertEqual(p.returncode, 1)
        self.assertIn("=== test_a.py: FAIL", p.stdout)

    def test_no_modules_is_an_error(self):
        self.assertEqual(self.run_runner().returncode, 2)
        self.assertEqual(
            self.run_runner(directory=self.dir / "missing").returncode, 2)

    def test_output_is_not_interleaved(self):
        body = """import time, unittest
class T(unittest.TestCase):
    def test_print(self):
        for i in range(200):
            print("TAG-{tag} line", i)
            time.sleep(0.002)
"""
        self.module("test_a.py", body.replace("{tag}", "A"))
        self.module("test_b.py", body.replace("{tag}", "B"))
        p = self.run_runner("-j", "2")
        self.assertEqual(p.returncode, 0, p.stdout)
        lines = p.stdout.splitlines()
        marks = [i for i, l in enumerate(lines) if l.startswith("=== ")]
        self.assertEqual(len(marks), 2)
        for start, end in zip(marks, marks[1:] + [len(lines)]):
            tag = lines[start].split()[1][5].upper()  # test_a.py -> A
            kinds = {l.split()[0] for l in lines[start + 1:end]
                     if l.startswith("TAG-")}
            self.assertEqual(kinds, {"TAG-" + tag})

    def test_limit_is_respected(self):
        log = self.dir / "times.log"
        body = """import time, unittest
class T(unittest.TestCase):
    def test_sleep(self):
        with open(%r, "a") as f:
            f.write("start %%f\\n" %% time.time())
        time.sleep(1.0)
        with open(%r, "a") as f:
            f.write("stop %%f\\n" %% time.time())
""" % (str(log), str(log))
        for n in "abcd":
            self.module("test_%s.py" % n, body)
        p = self.run_runner("-j", "2")
        self.assertEqual(p.returncode, 0, p.stdout)
        events = sorted(
            (float(l.split()[1]), 1 if l.startswith("start") else -1)
            for l in log.read_text().splitlines())
        live = peak = 0
        for _, delta in sorted(events, key=lambda e: (e[0], e[1])):
            live += delta
            peak = max(peak, live)
        self.assertLessEqual(peak, 2)
        self.assertGreaterEqual(peak, 2)

    def test_fast_module_is_printed_before_a_slower_earlier_one(self):
        self.module("test_a.py", "import time, unittest\n"
                    "class T(unittest.TestCase):\n"
                    "    def test_slow(self):\n"
                    "        time.sleep(2)\n")
        self.module("test_b.py", PASS)
        p = self.run_runner("-j", "2")
        self.assertEqual(p.returncode, 0, p.stdout)
        order = [l.split()[1] for l in p.stdout.splitlines()
                 if l.startswith("=== ")]
        self.assertEqual(order, ["test_b.py:", "test_a.py:"])


if __name__ == "__main__":
    unittest.main()
