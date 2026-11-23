#!/usr/bin/env python3
"""Label each open pull request with the areas its changed files touch, and
take an area label off once the pull request no longer touches that area.

Each changed path takes the label of the first rule in AREAS it starts with,
so a directory rule ends in "/" and matches every path under it. A path no rule matches adds nothing, so a test or a lock file
follows the code beside it rather than labelling the pull request by itself.
Only the labels in AREAS are ever added or removed; any other label, set by a
person or by Dependabot, is left alone.

A pull request with more than 100 changed files has the rest read through the
REST API, which lists up to 3000. A pull request whose files cannot all be read
is left as it is.

    scope-label.py [--dry-run] [REPO]

Prints one line per pull request it relabels, and one per pull request it
skips because the API lists only part of its files. Exit 2 when the read fails
or a label in AREAS is missing from REPO, having written nothing, or when a
label write fails, after the other writes went ahead.
"""

import argparse
import json
import os
import subprocess
import sys
import time
from urllib.parse import quote

# First match wins, so a narrower rule sits above the broader one it carves
# out of: src/web/token.rs is LiveKit's, the rest of src/web/ the backend's.
# web/ is what the browser loads; src/web/ is the Rust server it talks to.
AREAS = [
    ("problem-bank/", "area: problem bank"),
    ("web/judges/", "area: problem bank"),
    ("web/problem-categories.json", "area: problem bank"),
    ("web/problem-pages.json", "area: problem bank"),
    ("web/problem-topics.json", "area: problem bank"),
    ("web/problems/", "area: problem bank"),
    ("src/agent/problem_", "area: problem bank"),
    ("src/livekit/", "area: livekit"),
    ("src/livekit.rs", "area: livekit"),
    ("src/dispatch.rs", "area: livekit"),
    ("src/token.rs", "area: livekit"),
    ("src/web/token.rs", "area: livekit"),
    ("vendor/", "area: livekit"),
    ("src/recording/", "area: recording"),
    ("src/peer_audio/", "area: recording"),
    ("src/agent/", "area: interviewer"),
    ("src/agent.rs", "area: interviewer"),
    ("src/gemini/", "area: interviewer"),
    ("src/gemini.rs", "area: interviewer"),
    ("src/tasks/", "area: task mode"),
    ("web/", "area: frontend"),
    ("src/web/", "area: backend"),
    ("desktop/", "area: desktop"),
    ("docs/", "area: docs"),
    ("README.md", "area: docs"),
    ("CONTRIBUTING.md", "area: docs"),
    (".claude/", "area: skills"),
    (".github/", "area: ci"),
    ("scripts/", "area: ci"),
    # Last, so every narrower area under src/ above keeps its own label.
    ("src/", "area: backend"),
]

LABELS = sorted({label for _, label in AREAS})

QUERY = """
query($owner: String!, $name: String!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequests(states: OPEN, first: 50, after: $endCursor) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number
        labels(first: 100) { nodes { name } }
        files(first: 100) { totalCount nodes { path } }
      }
    }
  }
}
"""

# Seconds between tries of a failed read; the tests set it to 0.
PAUSE = float(os.environ.get("SCOPE_LABEL_PAUSE", "5"))


def area(path):
    """The label of the first rule matching PATH, or None."""
    for rule, label in AREAS:
        if path.startswith(rule):
            return label
    return None


def wanted(paths):
    return sorted({label for label in map(area, paths) if label})


def changes(prs):
    """(number, labels to add, labels to remove) for each pull request whose
    area labels differ from its files."""
    out = []
    for pr in sorted(prs, key=lambda p: p["number"]):
        if pr["paths"] is None:
            print(f"#{pr['number']}: skip (more files than the API lists)")
            continue
        has = {label["name"] for label in pr["labels"]["nodes"]} & set(LABELS)
        want = set(wanted(pr["paths"]))
        add, remove = sorted(want - has), sorted(has - want)
        if add or remove:
            out.append((pr["number"], add, remove))
    return out


def gh(*args):
    return subprocess.run(["gh", *args], capture_output=True, text=True, check=False)


def read(*args):
    """gh's answer to a read, tried three times, as ready-label.py does."""
    for attempt in range(3):
        if attempt:
            time.sleep(PAUSE)
        result = gh(*args)
        if result.returncode == 0:
            break
    return result


def pages(text):
    """The JSON documents --paginate prints back to back, one per page."""
    decoder, pos = json.JSONDecoder(), 0
    while True:
        while pos < len(text) and text[pos].isspace():
            pos += 1
        if pos == len(text):
            return
        page, pos = decoder.raw_decode(text, pos)
        yield page["data"]["repository"]


def open_prs(repo):
    """Every open pull request with its labels and all its changed paths, or
    None when a read fails."""
    owner, name = repo.split("/", 1)
    result = read(
        "api",
        "graphql",
        "--paginate",
        "-F",
        f"owner={owner}",
        "-F",
        f"name={name}",
        "-f",
        f"query={QUERY}",
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        return None
    prs = [pr for page in pages(result.stdout) for pr in page["pullRequests"]["nodes"]]
    for pr in prs:
        files = pr.pop("files")
        pr["paths"] = [f["path"] for f in files["nodes"]]
        if files["totalCount"] <= len(files["nodes"]):
            continue
        result = read(
            "api",
            "--paginate",
            f"repos/{repo}/pulls/{pr['number']}/files?per_page=100",
            "--jq",
            ".[].filename",
        )
        if result.returncode != 0:
            sys.stderr.write(result.stderr)
            return None
        # One name per line: a path may hold a space. Past 3000 files the
        # listing stops short, and a label taken off for a file it left out
        # would be wrong, so that pull request keeps the labels it has.
        paths = result.stdout.splitlines()
        pr["paths"] = paths if len(paths) >= files["totalCount"] else None
    return prs


def missing_labels(repo):
    """The labels in AREAS that REPO lacks, or None when the read fails."""
    result = read(
        "api", "--paginate", f"repos/{repo}/labels?per_page=100", "--jq", ".[].name"
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        return None
    present = set(result.stdout.splitlines())
    return [label for label in LABELS if label not in present]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("repo", nargs="?", default="sysprog21/codetrial")
    args = parser.parse_args(argv)

    if not args.dry_run:
        missing = missing_labels(args.repo)
        if missing is None:
            return 2
        if missing:
            print(
                f"missing labels in {args.repo}: {', '.join(missing)}", file=sys.stderr
            )
            return 2
    prs = open_prs(args.repo)
    if prs is None:
        return 2

    status = 0
    for number, add, remove in changes(prs):
        said = [f"+{label}" for label in add] + [f"-{label}" for label in remove]
        print(f"#{number}: {' '.join(said)}")
        if args.dry_run:
            continue
        calls = []
        if add:
            fields = [arg for label in add for arg in ("-f", f"labels[]={label}")]
            calls.append(
                (
                    "api",
                    f"repos/{args.repo}/issues/{number}/labels",
                    *fields,
                    "--silent",
                )
            )
        for label in remove:
            calls.append(
                (
                    "api",
                    "-X",
                    "DELETE",
                    f"repos/{args.repo}/issues/{number}/labels/{quote(label, safe='')}",
                    "--silent",
                )
            )
        for call in calls:
            result = gh(*call)
            if result.returncode:
                sys.stderr.write(result.stderr)
                status = 2
    return status


if __name__ == "__main__":
    sys.exit(main())
