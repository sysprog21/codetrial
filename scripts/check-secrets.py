#!/usr/bin/env python3
"""Check changed Git blobs offline; diagnostics never contain matching text."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys


class InspectionError(Exception):
    pass


UNREADABLE = "cannot inspect Git objects"


def git(*args):
    result = subprocess.run(
        ["git", "--literal-pathspecs", *args], capture_output=True, check=False
    )
    if result.returncode:
        raise InspectionError(UNREADABLE)
    return result.stdout


# (label, literals every match contains, pattern). A blob is searched with the
# pattern only when it holds one of the literals, which is much cheaper than
# running the pattern over every byte; keep the two in step.
PATTERNS = (
    (
        "private key",
        (b"PRIVATE KEY",),
        re.compile(rb"-----BEGIN (?:[A-Z]+ )*PRIVATE KEY(?: BLOCK)?-----"),
    ),
    # A lookahead rather than \b, since the key alphabet ends in "-".
    (
        "Google API key",
        (b"AIza",),
        re.compile(rb"(?<![0-9A-Za-z])AIza[0-9A-Za-z_-]{35}(?![0-9A-Za-z_-])"),
    ),
    (
        "GitHub token",
        (b"ghp_", b"gho_", b"ghu_", b"ghs_", b"ghr_", b"github_pat_"),
        re.compile(
            rb"(?<![0-9A-Za-z])(?:gh[pousr]_[0-9A-Za-z]{36,}|github_pat_[0-9A-Za-z_]{60,})(?![0-9A-Za-z])"
        ),
    ),
)
LOCAL_ENV = (".env", ".env.local", "config/codetrial.env.local")
# Tests feed throwaway keys to the code under test, so a private-key header
# under here is not a finding; every other rule still applies.
KEY_FIXTURE_DIR = b"tests/"


ASSIGNMENT = re.compile(rb"\s*(?:export\s+)?([A-Za-z_][A-Za-z_0-9]*)\s*=\s*(.*?)\s*$")
CREDENTIAL_NAME = re.compile(rb"KEY|TOKEN|PASSWORD|SECRET", re.I)
QUOTED = re.compile(rb"(['\"])(.*?)\1(?:[ \t]+#[^\r\n]*)?")
PLACEHOLDER = re.compile(rb"YOUR_|EXAMPLE|PLACEHOLDER|CHANGEME", re.I)
# A whole-value reference is not a credential; a password that merely contains
# "$" is, and is kept as a literal.
REFERENCE = re.compile(rb"\$(?:[A-Za-z_][A-Za-z_0-9]*|\{[^}]*\}|\(.*\))|`.*`")


def literal_value(raw):
    """The credential an assignment's right side spells out, or None."""
    quote = raw[:1]
    if quote in (b"'", b'"'):
        quoted = QUOTED.fullmatch(raw)
        if not quoted:
            return None
        value = quoted[2]
    else:
        value = re.split(rb"\s+#", raw, maxsplit=1)[0].strip()
    if len(value) < 8 or PLACEHOLDER.search(value):
        return None
    if quote != b"'" and REFERENCE.fullmatch(value):
        return None
    return value


def local_values(root):
    """Read literal assignments only, without sourcing or interpolating env files."""
    values = set()
    for name in LOCAL_ENV:
        path = root / name
        if path.is_symlink():
            raise InspectionError("local credential file is a symbolic link")
        try:
            data = path.read_bytes()
        except FileNotFoundError:
            continue
        except OSError:
            raise InspectionError("cannot read local credential file") from None
        for line in data.splitlines():
            match = ASSIGNMENT.match(line)
            if match and CREDENTIAL_NAME.search(match[1]):
                value = literal_value(match[2])
                if value is not None:
                    values.add(value)
    return values


def sensitive_path(path):
    name = path.rsplit(b"/", 1)[-1]
    return (
        name == b".env" or name.startswith(b".env.") or name.endswith(b".env.local")
    ) and not name.endswith((b".example", b".sample", b".template"))


def display_path(path, values):
    # Spans from every rule are merged first, so one match cannot hide part of
    # another and leave the rest of a token in the output.
    spans = []
    for value in values:
        spans.extend(m.span() for m in re.finditer(re.escape(value), path))
    for _, _, pattern in PATTERNS:
        spans.extend(m.span() for m in pattern.finditer(path))
    shown = b""
    position = 0
    for start, end in sorted(spans):
        if start < position:
            position = max(position, end)
            continue
        shown += path[position:start] + b"[REDACTED]"
        position = end
    shown += path[position:]
    return json.dumps(os.fsdecode(shown), ensure_ascii=True)


def inspect(path, data, values, context):
    labels = set()
    if sensitive_path(path):
        labels.add("local environment file")
    for label, literals, pattern in PATTERNS:
        if label == "private key" and path.startswith(KEY_FIXTURE_DIR):
            continue
        if (
            any(literal in data for literal in literals) and pattern.search(data)
        ) or pattern.search(path):
            labels.add(label)
    if any(value in data or value in path for value in values):
        labels.add("local credential value")
    for label in sorted(labels):
        print(
            f"Secret check: {context} {display_path(path, values)}: {label} (redacted)",
            file=sys.stderr,
        )
    return bool(labels)


def changed_blobs(*args):
    """Yield (path, blob ID) for each result of a raw -z diff that is a file."""
    fields = git(*args).split(b"\0")
    for index in range(0, len(fields) - 1, 2):
        meta = fields[index]
        if not meta:
            break
        # One leading colon per parent; a merge's combined form lists a path
        # only when it differs from every parent.
        parents = len(meta) - len(meta.lstrip(b":"))
        parts = meta.lstrip(b":").split()
        mode = parts[parents]
        oid = parts[2 * parents + 1]
        if mode in (b"000000", b"160000") or not oid.strip(b"0"):
            continue
        yield fields[index + 1], oid.decode()


class Blobs:
    """One long-lived `git cat-file --batch`, instead of a process per blob."""

    def __init__(self):
        self.proc = subprocess.Popen(
            ["git", "cat-file", "--batch"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )

    def read(self, oid):
        try:
            self.proc.stdin.write(oid.encode() + b"\n")
            self.proc.stdin.flush()
            header = self.proc.stdout.readline().split()
            if len(header) != 3 or header[1] != b"blob":
                raise InspectionError(UNREADABLE)
            size = int(header[2])
            data = self.proc.stdout.read(size + 1)[:-1]
        except (OSError, ValueError):
            raise InspectionError(UNREADABLE) from None
        if len(data) != size:
            raise InspectionError(UNREADABLE)
        return data

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.proc.stdin.close()
        self.proc.wait()


# scripts/check-cjk.py reads the same blobs through git, Blobs, targets and
# InspectionError, so keep their names and the "staged" context in step.
def targets(commits):
    """Yield (context, path, blob ID) for the staged index or for each commit."""
    if commits is None:
        for path, oid in changed_blobs(
            "diff", "--cached", "--raw", "-z", "--no-renames"
        ):
            yield "staged", path, oid
        return
    seen = set()
    for commit in commits:
        if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", commit):
            raise InspectionError("invalid commit ID")
        # -c is the plain raw form for a commit with one parent, and lists only
        # what differs from every parent for a merge.
        for path, oid in changed_blobs(
            "diff-tree",
            "-r",
            "-z",
            "--no-renames",
            "--no-commit-id",
            "--root",
            "-c",
            commit,
        ):
            if (path, oid) not in seen:
                seen.add((path, oid))
                yield commit[:12], path, oid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--commits", action="store_true", help="read commit IDs on stdin"
    )
    args = parser.parse_args()
    root = Path(os.fsdecode(git("rev-parse", "--show-toplevel").rstrip(b"\n")))
    values = local_values(root)
    commits = sys.stdin.read().splitlines() if args.commits else None
    failed = False
    with Blobs() as blobs:
        for context, path, oid in targets(commits):
            failed |= inspect(path, blobs.read(oid), values, context)
    if failed:
        print(
            "Remove the credential from the staged content or outgoing history before retrying.",
            file=sys.stderr,
        )
    return int(failed)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (InspectionError, OSError):
        print(
            "Secret check failed: inspection unavailable; no content was printed.",
            file=sys.stderr,
        )
        sys.exit(2)
