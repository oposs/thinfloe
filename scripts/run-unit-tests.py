#!/usr/bin/env python3
"""Run the Python unit test modules of a directory in parallel.

Usage: run-unit-tests.py [-j N] DIR

Runs `python3 -m unittest -v <module>` for every DIR/test_*.py (top level
only, sorted) with DIR as the working directory, at most N at a time (default
4). Each module's output (stdout and stderr merged) is printed whole, under a
header, when the module ends, so the output of two modules never mixes.

Exit status: 0 all modules passed, 1 a module failed, 2 no module found or
DIR is not a directory.
"""
import argparse
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


def run_module(directory, name):
    start = time.monotonic()
    proc = subprocess.run(
        [sys.executable, "-m", "unittest", "-v", name[:-3]],
        cwd=directory, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, errors="replace")
    return name, proc.returncode == 0, time.monotonic() - start, proc.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("-j", type=int, default=4, metavar="N",
                        help="modules run at once (default 4)")
    parser.add_argument("directory")
    args = parser.parse_args()
    directory = Path(args.directory)
    if not directory.is_dir():
        print("run-unit-tests: not a directory: %s" % directory, file=sys.stderr)
        return 2
    names = sorted(p.name for p in directory.glob("test_*.py"))
    if not names:
        print("run-unit-tests: no test_*.py in %s" % directory, file=sys.stderr)
        return 2

    failed = []
    with ThreadPoolExecutor(max_workers=max(1, args.j)) as pool:
        futures = [pool.submit(run_module, directory, n) for n in names]
        # Print in module order: output stays readable and is deterministic.
        for future in futures:
            name, passed, secs, output = future.result()
            if not passed:
                failed.append(name)
            print("=== %s: %s (%.1f s) ===" % (name, "ok" if passed else "FAIL", secs))
            print(output, end="" if output.endswith("\n") else "\n", flush=True)
    summary = "unit: %d modules, %d failed" % (len(names), len(failed))
    if failed:
        summary += ": " + " ".join(failed)
    print(summary)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
