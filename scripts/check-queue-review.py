#!/usr/bin/env python3
"""Recheck each pull request represented in a merge group for the
interactive review it needs, and keep a pull request from editing the workflow
that does so."""

import argparse
import importlib.util
import json
import os
import subprocess
from urllib.parse import urlencode

WORKFLOW = ".github/workflows/pr-description.yml"
CHECK = "interactive-review"
# The workflow skips itself while the base has none of these (the pull request
# that adds the gate), so a pull request that deletes one would switch the check
# off for every later run. Changing one is for maintainer review; it cannot go.
GATE = (
    "scripts/check-interactive-review.py",
    "scripts/check-queue-review.py",
    ".claude/skills/codetrial-review/commit-sizes.py",
)


def git(*args):
    return subprocess.run(
        ["git", "--no-lazy-fetch", *args],
        env={**os.environ, "GIT_NO_REPLACE_OBJECTS": "1", "GIT_TERMINAL_PROMPT": "0"},
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        universal_newlines=True,
    ).stdout.strip()


def api(path):
    pages = json.loads(
        subprocess.check_output(["gh", "api", "--paginate", "--slurp", path], text=True)
    )
    return [item for page in pages for item in page]


def within(tip, parents):
    """`tip` and its ancestors among the commits of the group."""
    seen, todo = set(), [tip]
    while todo:
        sha = todo.pop()
        if sha not in seen and sha in parents:
            seen.add(sha)
            todo.extend(parents[sha])
    return seen


def check(base, head, pulls, checker):
    # Only merge commits may be supplied by GitHub itself. Every ordinary
    # commit must belong to a live PR whose exact head is in this group.
    git("merge-base", "--is-ancestor", base, head)
    parents = {}
    for line in git("rev-list", "--parents", f"{base}..{head}").splitlines():
        sha, *above = line.split()
        parents[sha] = above
    pending = {sha for sha, above in parents.items() if len(above) < 2}
    included = []
    for pull in pulls:
        tip = pull["head"]["sha"]
        if tip not in parents:
            continue
        included.append(pull)
        user = pull["user"]
        if not checker.dependabot_exempt(base, tip, user["login"], user.get("type")):
            checker.check_labels(base, tip, [x["name"] for x in pull.get("labels", [])])
        pending.difference_update(within(tip, parents))
    if not included or pending:
        raise ValueError(
            "Merge group contains commits without a matching open PR head"
            + (": " + ", ".join(sha[:12] for sha in sorted(pending)) if pending else "")
        )
    return [pull["number"] for pull in included]


def committed(rev, path):
    """The file at `rev`, or None when it is not there. Any other failure raises."""
    if not git("ls-tree", "--name-only", rev, "--", path):
        return None
    return git("show", f"{rev}:{path}")


def guard(base, head, approved):
    """Why this pull request may not change the workflow that guards the queue.

    A merge group runs the workflow file as the queue ref has it, edits from the
    pull request included, and a job its own `if:` skips counts as passing. The
    pull-request run is the base branch's workflow, so it is where an edit is
    caught: the pull request may not change the file from where it branched, and
    may not add another workflow that claims the check, unless a maintainer
    named this exact head as approved. Any other workflow that so much as names
    the required check could be supplying it without running the checker, so a
    match anywhere in the file is refused, whatever YAML form it takes. Comparing with the branch point rather
    than the base tip keeps every older pull request green after an approved
    change lands. Dependabot gets no exception: it does edit this file.
    """
    if head == approved:
        return []
    problems = []
    start = git("merge-base", base, head)
    if committed(start, WORKFLOW) != committed(head, WORKFLOW):
        problems.append(f"{WORKFLOW} was changed")
    for path in GATE:
        if committed(start, path) is not None and committed(head, path) is None:
            problems.append(f"{path} was removed")
    folder = os.path.dirname(WORKFLOW)
    # -z: without it a path with unusual bytes comes back quoted, and the lookup
    # of that quoted name fails the whole guard.
    for path in filter(
        None, git("ls-tree", "-z", "--name-only", head, folder + "/").split("\0")
    ):
        if path != WORKFLOW and CHECK in (committed(head, path) or "").lower():
            problems.append(f"{path} names the {CHECK} check")
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", required=True)
    parser.add_argument("--branch", help="queue: the branch the group merges into")
    parser.add_argument("--repo", help="queue: owner/name")
    parser.add_argument(
        "--guard", action="store_true", help="check the queue workflow is unchanged"
    )
    parser.add_argument(
        "--approved", default="", help="guard: a maintainer-approved head"
    )
    args = parser.parse_args()
    if args.guard:
        try:
            problems = guard(args.base, args.head, args.approved)
        except (subprocess.CalledProcessError, UnicodeDecodeError) as error:
            # Not being able to compare is not a pass.
            parser.exit(
                1, f"Cannot compare the queue workflow with its branch point: {error}\n"
            )
        if problems:
            parser.exit(
                1,
                "The workflow that guards the merge queue may not change in a pull "
                "request:\n  "
                + "\n  ".join(problems)
                + "\nA maintainer approves such a change by setting "
                "CODETRIAL_POLICY_APPROVED_HEAD to the exact head.\n",
            )
        print("The queue workflow is the base branch's.")
        return
    if not (args.branch and args.repo):
        parser.error("the queue check requires --branch and --repo")
    spec = importlib.util.spec_from_file_location(
        "interactive_review",
        os.path.join(os.path.dirname(__file__), "check-interactive-review.py"),
    )
    checker = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(checker)
    query = urlencode({"state": "open", "base": args.branch, "per_page": 100})
    try:
        pulls = api(f"repos/{args.repo}/pulls?{query}")
    except (subprocess.CalledProcessError, OSError, ValueError) as error:
        parser.exit(1, f"Cannot list the open pull requests: {error}\n")
    print(
        "Verified merge-group pull requests:",
        check(args.base, args.head, pulls, checker),
    )


if __name__ == "__main__":
    main()
