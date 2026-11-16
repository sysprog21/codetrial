#!/usr/bin/env python3
"""Decide whether a change touches what the Rust gate covers.

    check-gate-evidence.py --scope-only [-z] < paths

Paths are repository-relative, one per line, or NUL-separated with -z because
a path may hold a newline. --scope-only prints `in` when any path is in the
Rust scope and `out` otherwise; an empty list is `out`. A path under .github/
is never in scope. It exits 0 on an answer and 2 on bad usage or a path that is
absolute, empty (a blank line is one), ends in a carriage return in line mode,
or holds a `.` or `..` component: those can name something other than what they
look like, so they are refused rather than classified.
"""

import argparse
import sys

# Whether web/ is in the Rust scope. The binary embeds web/ at build time, so a
# web-only change can break the Rust lane; it stays out by default so that
# JavaScript contributors need no toolchain. Flip it here and nowhere else.
WEB_IN_RUST_SCOPE = False

# vendor/ holds the path dependencies the root manifest excludes from its
# workspace, and problem-bank/ is read by include_str! at compile time; the
# three .toml files configure the lint and license gates of the Rust lane.
RUST_PREFIXES = (
    "src/",
    "desktop/",
    "scripts/hook-check/",
    ".cargo/",
    "vendor/",
    "problem-bank/",
)
RUST_FILES = frozenset(
    (
        "build.rs",
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "clippy.toml",
        "rustfmt.toml",
        "deny.toml",
    )
)
# Files a Rust source names by literal: an include_str!, a path a test reads, a
# script it runs. tests/test_check_gate_evidence.py derives the list from the
# tracked sources, so it grows only when that test says a file is missing. The
# web/ entries are in whatever WEB_IN_RUST_SCOPE says: the constant decides
# only the embedded folder as a whole, and a change to a file a test reads can
# fail that test either way.
RUST_READ_FILES = frozenset(
    (
        "docs/interview-contract-versions.md",
        "docs/recording-contract.md",
        "docs/task-mode.md",
        "scripts/browser-check.cjs",
        "scripts/browser-check.sh",
        "scripts/install-git-hooks.sh",
        "scripts/interview-run-budget.py",
        "scripts/livekit-room-service.cjs",
        "scripts/recording-cleanup.sh",
        "scripts/recording-integration.sh",
        "scripts/recording-provision-check.sh",
        "scripts/server-check.sh",
        "scripts/soak-judge.cjs",
        "web/document-grounding.js",
        "web/index.html",
        "web/interview.html",
        "web/interview.js",
        "web/lib.js",
        "web/problem-pages.json",
        "web/runners.js",
        "web/styles.css",
        "web/vendor/livekit-client.js",
    )
)
# Under tests/ everything is in scope except the browser lane, the executable
# stubs and the top-level Python files.
TESTS_OUT_PREFIXES = ("tests/browser/", "tests/stubs/")


def is_top_level_python(name):
    # test_*.py and the helper they share, support.py, which no Rust source names.
    return "/" not in name and name.endswith(".py")


def in_rust_scope(path):
    if path in RUST_FILES or path in RUST_READ_FILES or path.startswith(RUST_PREFIXES):
        return True
    if WEB_IN_RUST_SCOPE and path.startswith("web/"):
        return True
    if path.startswith("tests/"):
        return not (
            path.startswith(TESTS_OUT_PREFIXES)
            or is_top_level_python(path[len("tests/") :])
        )
    return False


def parse_paths(data, nul):
    """The paths in `data`, or ValueError naming the first one refused."""
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as err:
        raise ValueError("input is not UTF-8") from err
    parts = text.split("\0" if nul else "\n")
    if parts[-1] == "":
        parts.pop()
    for path in parts:
        if (
            "\0" in path
            or (not nul and path.endswith("\r"))
            or path.startswith("/")
            or {"", ".", ".."} & set(path.split("/"))
        ):
            raise ValueError(f"not a relative repository path: {path!r}")
    return parts


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--scope-only", action="store_true", required=True)
    parser.add_argument("-z", dest="nul", action="store_true")
    args = parser.parse_args(argv)
    try:
        paths = parse_paths(sys.stdin.buffer.read(), args.nul)
    except ValueError as err:
        print(f"check-gate-evidence: {err}", file=sys.stderr)
        return 2
    print("in" if any(in_rust_scope(p) for p in paths) else "out")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
