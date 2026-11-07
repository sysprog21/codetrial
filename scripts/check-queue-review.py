#!/usr/bin/env python3
"""Keep a pull request from editing the workflow that the merge queue runs
for the interactive-review check."""

import argparse
import os
import subprocess

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
            # ascii(): a name from the branch is printed, and a newline in it
            # could start a workflow command.
            problems.append(f"{ascii(path)} names the {CHECK} check")
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", required=True)
    parser.add_argument("--approved", default="", help="a maintainer-approved head")
    args = parser.parse_args()
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


if __name__ == "__main__":
    main()
