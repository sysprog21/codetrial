#!/usr/bin/env python3
"""Decide whether a change needs the Rust gate, and whether it carries proof.

    check-gate-evidence.py --scope-only [-z] < paths
    check-gate-evidence.py --head-tree ID --changed-files-file FILE
        --author-type TYPE --base-toolchain-file FILE
        [--receipt-file FILE] [--head-toolchain-file FILE]

Paths are repository-relative, one per line, or NUL-separated with -z because
a path may hold a newline. --scope-only prints `in` when any path is in the
Rust scope and `out` otherwise; an empty list is `out`. A path under .github/
is never in scope. It exits 0 on an answer and 2 on bad usage or a path that is
absolute, empty (a blank line is one), ends in a carriage return in line mode,
or holds a `.` or `..` component: those can name something other than what they
look like, so they are refused rather than classified.

Without a mode the script checks evidence. Every input is a file of data the
caller fetched (the head commit's tree id, the gate note on it, the changed
paths NUL-separated, the two rust-toolchain.toml files); it reads those files
and nothing else, runs nothing, and never places a receipt field in a path or a
command. A change with no path in the Rust scope needs no receipt. Otherwise
some receipt must be for lane rust or all, passed, and name the head commit's
tree, and its rustc must be the channel pinned by the base's
rust-toolchain.toml, or by the head's when the change edits that file; the
toolchain files are read only when a receipt is owed. The
receipt is a hint a dishonest author can forge; it makes the approval decision
mechanical and does not prove the gate ran. A receipt that counts optional-tool
skips still qualifies: a local run tolerates them.

Exit 0 means the evidence holds or no receipt is owed (a Bot author is exempt).
Exit 1 means it is missing or wrong, with the reason and the commands to run.
Exit 2 means the answer cannot be given: a cut path list, a toolchain file that
cannot be read, bad usage.
"""

import argparse
import json
import re
import sys

try:
    import tomllib
except ImportError:  # Python before 3.11; only the evidence check needs it
    tomllib = None

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


RECEIPT_LIMIT = 16384
TOOLCHAIN_LIMIT = 65536
PATHS_LIMIT = 1 << 24
MAX_RECEIPTS = 8
LANES = frozenset(("rust", "all", "portable", "browser"))
# The lanes that run the Rust gates; the others say nothing about a Rust change.
COVERING_LANES = frozenset(("rust", "all"))
TREE_ID = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})")
EXACT_CHANNEL = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
RUSTC_VERSION = re.compile(r"rustc ([0-9]+\.[0-9]+\.[0-9]+) \(.+\)")
TOOLCHAIN_FILE = "rust-toolchain.toml"
PREFIX = "check-gate-evidence: "
NO_NOTE = "no receipt: the pushed head carries no gate note"

GUIDANCE = (
    "Run ./scripts/env-check.sh, then ./scripts/test.sh, then push again.",
    "A receipt names the tree the gate saw, so a partial commit needs the gate "
    "re-run on what is committed.",
)


class Indeterminate(Exception):
    """The answer cannot be given from these inputs (exit 2)."""


class Refused(Exception):
    """The evidence is missing or wrong (exit 1)."""


def read_limited(path, limit):
    """The file's bytes, or None when it is larger than `limit`."""
    with open(path, "rb") as handle:
        data = handle.read(limit + 1)
    return data if len(data) <= limit else None


def reject_duplicates(pairs):
    keys = [key for key, _ in pairs]
    if len(set(keys)) != len(keys):
        raise ValueError("duplicate key")
    return dict(pairs)


def reject_constant(name):
    raise ValueError(name)


def is_count(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def check_receipt_shape(one):
    if not isinstance(one, dict):
        raise Refused("receipt: an entry is not an object")
    lane = one.get("lane")
    if not isinstance(lane, str) or lane not in LANES:
        raise Refused("receipt: an entry has an unknown lane")
    if not isinstance(one.get("passed"), bool):
        raise Refused("receipt: an entry has a non-boolean passed")
    if not is_count(one.get("skipped")):
        raise Refused("receipt: an entry has a bad skipped count")
    for key, limit in (("time", 32), ("rustc", 200), ("cargo", 200)):
        value = one.get(key)
        if not isinstance(value, str) or len(value) > limit:
            raise Refused(f"receipt: an entry has a bad {key}")


def parse_receipt(data, head_tree):
    """The receipts of a well-formed note for `head_tree`, else Refused."""
    if data is None:
        raise Refused(f"receipt: over {RECEIPT_LIMIT} bytes")
    try:
        body = json.loads(
            data.decode("utf-8"),
            object_pairs_hook=reject_duplicates,
            parse_constant=reject_constant,
        )
    except (UnicodeDecodeError, ValueError, RecursionError) as err:
        raise Refused("receipt: not a single JSON value") from err
    if not isinstance(body, dict):
        raise Refused("receipt: not a JSON object")
    version = body.get("version")
    # 1.0 and True compare equal to 1, so the type is checked too.
    if type(version) is not int or version != 1:
        raise Refused("receipt: unsupported version")
    tree = body.get("worktree_tree")
    if not isinstance(tree, str) or not TREE_ID.fullmatch(tree):
        raise Refused("receipt: bad worktree_tree")
    receipts = body.get("receipts")
    if not isinstance(receipts, list) or not 1 <= len(receipts) <= MAX_RECEIPTS:
        raise Refused(f"receipt: needs 1 to {MAX_RECEIPTS} entries")
    for one in receipts:
        check_receipt_shape(one)
    if tree != head_tree:
        raise Refused("receipt is for another tree than the pushed head")
    return receipts


def pinned_channel(data):
    """The exact `channel` of a rust-toolchain.toml, else Indeterminate.

    A channel that is not X.Y.Z (stable, 1.100) names whatever the toolchain
    manager resolves it to, so no receipt string can be compared to it.
    """
    if tomllib is None:
        raise Indeterminate("the evidence check needs Python 3.11+ (tomllib)")
    try:
        table = tomllib.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as err:
        raise Indeterminate(f"{TOOLCHAIN_FILE} is not readable TOML") from err
    toolchain = table.get("toolchain")
    channel = toolchain.get("channel") if isinstance(toolchain, dict) else None
    if not isinstance(channel, str) or not EXACT_CHANNEL.fullmatch(channel):
        raise Indeterminate(f"{TOOLCHAIN_FILE} does not pin an exact X.Y.Z channel")
    return channel


def toolchain_channel(path):
    if path is None:
        raise Indeterminate(f"no {TOOLCHAIN_FILE} was given to read the pin from")
    try:
        data = read_limited(path, TOOLCHAIN_LIMIT)
    except OSError as err:
        raise Indeterminate(
            f"cannot read {TOOLCHAIN_FILE}: {err.strerror or err}"
        ) from err
    if data is None:
        raise Indeterminate(f"{TOOLCHAIN_FILE} is over {TOOLCHAIN_LIMIT} bytes")
    return pinned_channel(data)


def read_receipt(path):
    if path is None:
        raise Refused(NO_NOTE)
    try:
        return read_limited(path, RECEIPT_LIMIT)
    except FileNotFoundError:
        raise Refused(NO_NOTE) from None
    except OSError as err:
        raise Indeterminate(f"cannot read the receipt: {err.strerror or err}") from err


def check_receipt(args, paths):
    receipts = parse_receipt(read_receipt(args.receipt_file), args.head_tree)
    lane_ok = [r for r in receipts if r["lane"] in COVERING_LANES]
    if not lane_ok:
        raise Refused("no receipt for lane rust or all")
    passed = [r for r in lane_ok if r["passed"]]
    if not passed:
        raise Refused("the rust or all lane did not pass")
    changes_pin = TOOLCHAIN_FILE in paths
    channel = toolchain_channel(
        args.head_toolchain_file if changes_pin else args.base_toolchain_file
    )
    for one in passed:
        found = RUSTC_VERSION.fullmatch(one["rustc"])
        if found and found.group(1) == channel:
            return
    raise Refused(
        f"no passing receipt was made with the pinned rustc {channel}; "
        "if the base branch moved the pin, rebase onto it and run the gate again"
    )


def read_changed_paths(path):
    try:
        data = read_limited(path, PATHS_LIMIT)
        if data is None:
            raise ValueError("list is too large")
        if data and not data.endswith(b"\0"):
            raise ValueError("the last path is not terminated, so the list may be cut")
        return parse_paths(data, True)
    except (OSError, ValueError) as err:
        raise Indeterminate(
            f"cannot use the changed-file list: {str(err)[:200]}"
        ) from err


def evidence(args):
    """Print what holds; raise Refused or Indeterminate when it does not."""
    if not TREE_ID.fullmatch(args.head_tree):
        raise Indeterminate("--head-tree is not a tree id")
    paths = read_changed_paths(args.changed_files_file)
    if args.author_type == "Bot":
        print("exempt: the author type is Bot, so no receipt is owed")
    elif not any(in_rust_scope(p) for p in paths):
        print("no Rust-scope path changed, so no receipt is owed")
    else:
        check_receipt(args, paths)
        print("gate evidence holds")


def run_evidence(args):
    try:
        evidence(args)
    except Indeterminate as err:
        print(PREFIX + str(err), file=sys.stderr)
        return 2
    except Refused as err:
        print(PREFIX + str(err), file=sys.stderr)
        print(*GUIDANCE, sep="\n", file=sys.stderr)
        return 1
    return 0


# The evidence check's value-taking options, by attribute name.
CHECK_OPTIONS = (
    "head_tree",
    "receipt_file",
    "changed_files_file",
    "base_toolchain_file",
    "head_toolchain_file",
    "author_type",
)


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--scope-only", action="store_true")
    parser.add_argument("-z", dest="nul", action="store_true")
    check = parser.add_argument_group("evidence check")
    check.add_argument("--head-tree")
    check.add_argument("--receipt-file")
    check.add_argument("--changed-files-file")
    check.add_argument("--base-toolchain-file")
    check.add_argument("--head-toolchain-file")
    check.add_argument("--author-type")
    args = parser.parse_args(argv)
    if not args.scope_only:
        required = (args.head_tree, args.changed_files_file, args.author_type)
        if args.nul or None in required:
            parser.error(
                "the evidence check needs --head-tree, --changed-files-file and --author-type"
            )
        return run_evidence(args)
    given = vars(args)
    if any(given[name] is not None for name in CHECK_OPTIONS):
        parser.error("--scope-only takes paths on stdin only")
    try:
        paths = parse_paths(sys.stdin.buffer.read(), args.nul)
    except ValueError as err:
        print(f"check-gate-evidence: {err}", file=sys.stderr)
        return 2
    print("in" if any(in_rust_scope(p) for p in paths) else "out")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
