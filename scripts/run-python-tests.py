#!/usr/bin/env python3

"""Run a unittest file's cases across a thread pool.

`python3 -m unittest tests/test_recording_provision_check.py` took 54 seconds of
the gate, which was more than the Rust and browser lanes together. Each of its
80 cases spawns `scripts/recording-provision-check.sh`, which spawns a dozen or
more fake `gcloud` and `curl` binaries, several of which shell back out to
`python3` to parse JSON.

Threads work here, but not for the reason it is tempting to write down. This
parent is not idling: one case measures 0.56s wall against 0.56s of user plus
system time, so the tree below it is pegging a core the whole way, and the
suite costs about 77 CPU-seconds however it is run. What the pool buys is that
those children are separate processes, and the GIL is released around
`subprocess`, so they land on other cores instead of queueing behind one. That
is a real saving on a machine with cores to spare and close to none on a
two-core runner, which is the honest shape of it.

The cost is therefore not removed, only spread, and the deeper fix is a level
down: `recording-provision-check.sh` invokes `python3` at eight sites to read
JSON, so a run boots the interpreter hundreds of times to assert on the
script's refusal messages. Extracting that JSON handling into a module the
script calls once, and the suite imports directly, would take this suite under
a second serially and delete the need for this file.

This assumes what those suites already do, and it is the condition to check
before adding a file to the gate through here: a case must own everything it
touches. All four suites build their sandbox with `tempfile.TemporaryDirectory`
and hand `subprocess` a *copied* environment (`os.environ | {...}`), so no case
can observe another's. A suite that mutated `os.environ`, called `os.chdir`, or
shared a fixture directory between cases would race here and must keep running
under plain `unittest` instead.

Failures are reported after the run rather than as they happen, sorted by test
id, because the pool finishes in whatever order the kernel schedules and a
report that changes order between runs is one nobody can diff.
"""

import argparse
import importlib.util
import os
import sys
import time
import unittest
from concurrent.futures import ThreadPoolExecutor


def load_cases(path):
    """Every TestCase in one test file, addressed the way the gate addresses it.

    By path rather than by module name: the gate names these files directly and
    `tests/` is not a package, so `unittest`'s own discovery refuses it.
    """
    name = os.path.splitext(os.path.basename(path))[0]
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    # Registered before execution so a suite that imports itself, or pickles a
    # case, finds the same module object rather than loading a second copy.
    sys.modules[name] = module
    spec.loader.exec_module(module)

    cases = []

    def flatten(suite):
        for item in suite:
            if isinstance(item, unittest.TestSuite):
                flatten(item)
            else:
                cases.append(item)

    flatten(unittest.TestLoader().loadTestsFromModule(module))
    return cases


def run_case(case):
    result = unittest.TestResult()
    case(result)
    return result


def main():
    # The summary line only. `argparse` reflows what it is given and would
    # otherwise print the whole rationale above as one paragraph, which is
    # written for someone reading the file rather than a terminal.
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("paths", nargs="+", help="unittest files to run")
    parser.add_argument(
        "--jobs",
        type=int,
        # Capped rather than unbounded: past about this many the fake binaries
        # are contending for process slots rather than running, and the number
        # stops buying wall clock. Measured at 8, 16 and 32 on a 64-core box:
        # 7.4s, 5.0s, 3.8s against 56.3s serial.
        default=min(32, os.cpu_count() or 4),
        help="cases to run at once (default: min(32, cpus))",
    )
    arguments = parser.parse_args()
    # Rejected here rather than by the pool, which raises a bare ValueError and
    # gives the caller a traceback for what is a mistyped flag.
    if arguments.jobs < 1:
        parser.error("--jobs must be at least 1")

    cases = []
    for path in arguments.paths:
        cases.extend(load_cases(path))

    # An empty run is not a pass. Nothing here can tell a file with no tests
    # from one whose tests stopped being collected, and the second is a lane
    # that has quietly stopped gating while still printing OK.
    if not cases:
        print(f"no tests found in {' '.join(arguments.paths)}", file=sys.stderr)
        return 1

    started = time.monotonic()
    with ThreadPoolExecutor(max_workers=arguments.jobs) as pool:
        results = list(pool.map(run_case, cases))
    elapsed = time.monotonic() - started

    failures = sorted(
        (entry for result in results for entry in result.failures + result.errors),
        key=lambda entry: str(entry[0]),
    )
    # Counted as a failure because `unittest.TestResult.wasSuccessful` counts it
    # as one, and this runner replaces that verdict. A case marked
    # `expectedFailure` that starts passing is a gate that has stopped gating,
    # which is the one result nobody goes looking for.
    unexpected = sorted(
        (case for result in results for case in result.unexpectedSuccesses), key=str
    )
    skipped = sum(len(result.skipped) for result in results)

    for case, trace in failures:
        print(f"\n{'=' * 70}\nFAIL: {case}\n{'-' * 70}\n{trace}", file=sys.stderr)
    for case in unexpected:
        print(f"\n{'=' * 70}\nUNEXPECTED SUCCESS: {case}", file=sys.stderr)

    detail = f" ({skipped} skipped)" if skipped else ""
    print(
        f"Ran {len(cases)} tests in {elapsed:.3f}s across {arguments.jobs} jobs{detail}"
    )
    if failures or unexpected:
        counted = [f"failures={len(failures)}"] if failures else []
        if unexpected:
            counted.append(f"unexpected successes={len(unexpected)}")
        print(f"FAILED ({', '.join(counted)})", file=sys.stderr)
        return 1
    print("OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
