#!/usr/bin/env python3
"""Audit the full lockfile, or only vulnerabilities a proposed merge adds."""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


# The root's lockfile, then the desktop app's, which is a crate of its own and
# is audited only where it exists.
LOCKFILES = ("Cargo.lock", "desktop/Cargo.lock")


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args])


def audit(lockfile, database, fetch):
    command = [
        "cargo",
        "audit",
        "--json",
        "--db",
        str(database),
        "--file",
        str(lockfile),
    ]
    if not fetch:
        command.append("--no-fetch")
    # A single configuration must govern both trees; do not load PR ignore rules.
    result = subprocess.run(
        command, cwd=lockfile.parent, capture_output=True, text=True
    )
    if result.stderr:
        print(result.stderr, file=sys.stderr, end="")
    try:
        report = json.loads(result.stdout)
        vulnerabilities = report["vulnerabilities"]
        warnings = report.get("warnings", {})
        if (
            not isinstance(vulnerabilities["list"], list)
            or type(vulnerabilities["found"]) is not bool
            or type(vulnerabilities["count"]) is not int
            or not isinstance(warnings, dict)
            or not all(isinstance(items, list) for items in warnings.values())
        ):
            raise ValueError("invalid audit report schema")
        findings = {
            (
                item["advisory"]["id"],
                item["package"]["name"],
                item["package"]["version"],
            )
            for item in vulnerabilities["list"]
        }
        valid = (
            vulnerabilities["found"] == bool(findings)
            and vulnerabilities["count"] == len(vulnerabilities["list"])
            and result.returncode == int(bool(findings))
            and all(
                isinstance(value, str) and value for key in findings for value in key
            )
        )
        if not valid:
            raise ValueError("inconsistent audit report or exit status")
        return findings, warnings
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError(f"audit did not complete: {error}") from error


def scan(trees):
    """Return head findings, those the base already had, warnings and the DB revision.

    `trees` holds one (head, previous, informational) per lockfile. All of them
    are read against the one database snapshot the first audit fetches, so no
    lockfile is judged by a newer advisory list than another.
    """
    findings, inherited, warnings, revision = set(), set(), {}, None
    with tempfile.TemporaryDirectory(prefix="codetrial-audit-") as directory:
        scratch = Path(directory)
        database = scratch / "advisory-db"
        lockfile = scratch / "Cargo.lock"
        for head, previous, informational in trees:
            lockfile.write_bytes(head)
            found, found_warnings = audit(lockfile, database, revision is None)
            if revision is None:
                revision = git(database, "rev-parse", "HEAD").decode().strip()
            for kind, items in found_warnings.items():
                warnings.setdefault(kind, []).extend(items)
            findings |= found
            if informational:
                inherited |= found
            elif previous is not None:
                lockfile.write_bytes(previous)
                inherited |= audit(lockfile, database, False)[0]
    return findings, inherited, warnings, revision


def render(lines, findings, added, warnings, revision):
    lines.append(f"RustSec database: `{revision}`")
    lines.append(f"\nVulnerabilities: {len(findings)}; blocking: {len(added)}")
    fingerprint = hashlib.sha256(json.dumps(sorted(findings)).encode()).hexdigest()
    lines.append(f"Findings fingerprint: `{fingerprint}`")
    for finding in sorted(findings):
        advisory, crate, version = finding
        blocking = finding in added
        lines.append(
            f"- [{advisory}](https://rustsec.org/advisories/{advisory}.html): "
            f"`{crate} {version}` ({'blocking' if blocking else 'inherited'})"
        )
        if blocking:
            message = (
                f"{advisory} {crate} {version}".replace("%", "%25")
                .replace("\r", "%0D")
                .replace("\n", "%0A")
            )
            print(f"::error::{message}", file=sys.stderr)
    if warnings:
        lines.append(
            "\nNon-blocking cargo-audit warnings (details in job log): "
            + ", ".join(f"{kind}: {len(items)}" for kind, items in warnings.items())
        )
        print(json.dumps(warnings, indent=2), file=sys.stderr)
    if findings:
        lines.append(
            "\nMaintainers track remediation in [the security audit issue]"
            "(https://github.com/sysprog21/codetrial/issues?q="
            "is%3Aissue+is%3Aopen+author%3Aapp%2Fgithub-actions+"
            "in%3Atitle+%22Dependency+security+audit%22) "
            "and [Dependabot alerts](https://github.com/sysprog21/codetrial/security/dependabot)."
        )


def base_lockfile(root, base, name):
    """`name` as the base has it, or None when the base has no such file."""
    if name != LOCKFILES[0]:
        present = subprocess.run(
            ["git", "-C", str(root), "cat-file", "-e", f"{base}:{name}"],
            capture_output=True,
        )
        if present.returncode != 0:
            # A lockfile the merge adds has nothing to inherit: every finding
            # in it is the merge's own. A base that does not exist at all
            # still fails, on the root's lockfile below.
            return None
    # Failing to read the base is fatal: it decides whether errors may pass.
    return git(root, "show", f"{base}:{name}")


def run(root, base, lines):
    trees = []
    for name in LOCKFILES:
        path = root / name
        if name != LOCKFILES[0] and not path.exists():
            continue
        head = path.read_bytes()
        previous = base_lockfile(root, base, name) if base else None
        trees.append((head, previous, previous == head))
    informational = all(unchanged for _, _, unchanged in trees)
    if informational:
        mode = "informational (unchanged lockfile)"
    else:
        mode = "new vulnerabilities in proposed merge" if base else "full"
    lines.append("Mode: " + mode)
    try:
        findings, inherited, warnings, revision = scan(trees)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        if not informational:
            raise
        lines.append(
            f"\nScan incomplete: {error}. No dependency change; informational only."
        )
        print(
            "::warning::Security scan incomplete; unchanged lockfile, see audit summary."
        )
        return 0, None
    added = findings - inherited
    render(lines, findings, added, warnings, revision)
    return int(bool(added)), bool(findings)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--base", help="base tip SHA of the tested merge, not its merge-base"
    )
    parser.add_argument(
        "--report", type=Path, help="write a Markdown report for security tracking"
    )
    args = parser.parse_args()
    if args.base == "":
        parser.error("--base must name the tested merge's base commit")
    root = Path(__file__).resolve().parents[1]
    lines = ["# Dependency security audit", ""]
    findings = None
    try:
        status, findings = run(root, args.base, lines)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        lines.append(f"\nScan incomplete: {error}")
        print(
            "::error::Security scan did not complete; see audit summary.",
            file=sys.stderr,
        )
        status = 2
    report = "\n".join(lines) + "\n"
    print(report)
    if args.report:
        # GitHub issue bodies accept at most 65,536 characters.
        args.report.write_text(
            report
            if len(report) <= 60000
            else report[:60000]
            + "\n\nReport truncated; see the security audit workflow log for all findings.\n"
        )
    if summary_path := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary_path, "a") as summary:
            summary.write(report)
    output_path = os.environ.get("GITHUB_OUTPUT")
    if findings is not None and output_path:
        with open(output_path, "a") as output:
            output.write(f"findings={str(findings).lower()}\n")
    return status


if __name__ == "__main__":
    sys.exit(main())
