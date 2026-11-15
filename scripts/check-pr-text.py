#!/usr/bin/env python3
"""Hold a pull request's title and description to CONTRIBUTING.md.

The title is a commit subject with no body behind it, so scripts/git-commit-msg.sh
judges it with --subject and the two cannot disagree. A pull request of one
commit reuses that commit's subject, which is what a reader of the merge commit
and of the log expects to match.

The description is judged only where CONTRIBUTING.md is mechanical: a pull
request that finishes an issue ends with Close #N alone on the last line, it has
no Refs #N, and it credits no AI tool, which the commit message script decides
too. What a bot writes into the description (the review summary between its
"auto-generated description" comments) is not the author's and is not read.
Nothing in a description says who wrote those comments, so an author can type
them to hide text from this check; it holds a cooperating author to the rules,
and a reviewer reading the description still sees the rest.

    check-pr-text.py --title TEXT [--body-file FILE] [--head-subject TEXT]

--head-subject is the subject of the only commit, given when there is one.
Exit 1 lists every refusal; 0 says nothing.
"""

import argparse
import importlib.util
from pathlib import Path
import re
import subprocess
import sys

SCRIPTS = Path(__file__).resolve().parent


def _load(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# Who counts as Dependabot is decided in scripts/hashchain.py alone, and what an
# issue reference looks like in scripts/commit-reflow.py, the hook's shapes.
HASHCHAIN = _load("hashchain", "hashchain.py")
ISSUE = _load("commit_reflow", "commit-reflow.py").ISSUE

# The bot's block runs from its opening comment to its closing one; author text
# after it is still the author's.
BOT_BLOCK = re.compile(
    r"(?is)<!--\s*This is an auto-generated description.*?"
    r"(?:<!--\s*End of auto-generated description[^>]*-->|\Z)"
)
COMMENT = re.compile(r"(?s)<!--.*?-->")
FENCE = re.compile(r"(?ms)^[ \t]*(```|~~~).*?^[ \t]*\1[ \t]*$")
# GitHub's own closing keywords, since any of them closes the issue whatever
# the house spelling is.
CLOSING = re.compile(
    r"(?i)\b(close[sd]?|fix(e[sd])?|resolve[sd]?)\b:?\s+(" + ISSUE + ")"
)
REFS = re.compile(r"(?i)\b(refs?|references?)\b:?\s+(" + ISSUE + ")")
CLOSE_LINE = re.compile(r"(?i)closes? #\d+")
# The template's last line, left as it came.
EMPTY_CLOSE = re.compile(r"(?i)closes?\s*#?")
# A rule or an empty heading closes a section and says nothing.
FILLER = re.compile(r"-{3,}|\*{3,}|#+")


def judge_message(flag, text):
    result = subprocess.run(
        ["sh", str(SCRIPTS / "git-commit-msg.sh"), flag, "-"],
        input=text + "\n",
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode == 4:
        raise SystemExit("check-pr-text: scripts/git-commit-msg.sh could not run")
    return result.returncode, result.stderr.splitlines()


def title_problems(title, head_subject):
    problems = []
    status, lines = judge_message("--subject", title)
    if status:
        problems += [
            "Title: " + line.replace("Commit message: ", "", 1)
            for line in lines
            if not line.startswith("Commit message: note")
        ]
    if head_subject is not None and title != head_subject:
        problems.append(
            "Title: a single-commit pull request reuses its commit subject: "
            + head_subject
        )
    return problems


def author_text(body):
    body = body.replace("\r\n", "\n")
    body = BOT_BLOCK.sub("", body)
    body = COMMENT.sub("", body)
    return FENCE.sub("", body)


def body_problems(body):
    text = author_text(body)
    lines = [line.strip() for line in text.split("\n") if line.strip()]
    while lines and FILLER.fullmatch(lines[-1]):
        lines.pop()
    problems = []
    if REFS.search(text):
        problems.append(
            "Description: no Refs #N. Close the issue this finishes, or split a "
            "larger issue into sub-issues and close one here"
        )
    for line in lines[:-1]:
        if CLOSING.search(line):
            problems.append(
                "Description: Close #N goes alone on the last line, not here: "
                + line[:80]
            )
    if lines:
        last = lines[-1]
        if EMPTY_CLOSE.fullmatch(last):
            problems.append(
                "Description: the last line is the template's empty Close #. "
                "Fill in the issue, or delete the line when this finishes none"
            )
        elif CLOSING.search(last) and not CLOSE_LINE.fullmatch(last):
            problems.append(
                "Description: end with Close #N alone on the last line: " + last[:80]
            )
    # The attribution rule is bit 2 of the commit message verdict; the rest of
    # that verdict is about a message's shape, which a description does not have.
    status, _ = judge_message("--stored", text.strip() or ".")
    if status & 2:
        problems.append(
            "Description: remove the AI attribution: it names a tool, and the "
            "project credits people"
        )
    return problems


def main():
    parser = argparse.ArgumentParser(
        description=__doc__.split("\n\n")[0],
        epilog=__doc__.split("\n\n", 1)[1],
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--title", required=True)
    parser.add_argument("--body-file")
    parser.add_argument("--head-subject")
    parser.add_argument("--author", help="the opener's login, as GitHub reports it")
    parser.add_argument("--author-type", help="the opener's account type")
    options = parser.parse_args()
    # Dependabot writes its own title and body, and nothing it offers makes
    # either fit, as with its commit messages.
    if HASHCHAIN.exempt_pr(options.author, options.author_type):
        print("A pull request Dependabot opened is not held to the house text.")
        return 0
    problems = title_problems(options.title, options.head_subject)
    if options.body_file:
        body = Path(options.body_file).read_text(encoding="utf-8", errors="replace")
        problems += body_problems(body)
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        print(
            "Edit the pull request's title or description; the check runs again "
            "on the edit. See CONTRIBUTING.md, Pull requests.",
            file=sys.stderr,
        )
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
