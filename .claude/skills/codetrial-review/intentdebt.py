"""What the pinned diff and the session already say about three kinds of debt.

Storey's triple debt model (arXiv 2603.22106) names technical debt (in the code),
cognitive debt (in people: shared understanding eroding) and intent debt (in
artifacts: the why that was never written down). This module reads the signals
the review already holds for each and phrases them as observations and
questions for the contributor. It decides nothing.

What it promises:

- No score. The paper offers no validated measure of any of the three, so none
  is invented here; a signal is a fact about this change, or a question.
- Nothing is written for the contributor. The paper warns against automating
  understanding, so a gap is named and the contributor answers it.
- Every line named is a line of the pinned diff, as a citation the page can open.
- Heuristics are labeled as such. The test, document and guard patterns are
  plain text and path matching that knows no language; they miss things and
  mis-file others, which is why each result says what it found, not what it means.
"""

import re

MAX_CHECKS = 10  # removed checks listed; the rest are counted
MAX_TESTS = 4
SOURCE_SUFFIXES = (
    ".rs", ".py", ".js", ".mjs", ".cjs", ".ts", ".tsx", ".jsx", ".c", ".h", ".cc",
    ".cpp", ".hpp", ".go", ".java", ".kt", ".swift", ".rb", ".sh", ".css", ".html",
)  # fmt: skip
DOC_SUFFIXES = (".md", ".rst", ".adoc")
DOC_DIRS = {"docs", "doc", "adr", "rfcs"}
TEST_DIRS = {"tests", "test", "__tests__", "spec", "specs"}
TEST_NAME = re.compile(r"(^test_)|(_test\.)|(\.test\.)|(\.spec\.)|(_spec\.)")
# A line that is a check or an early exit, in the languages this repository uses
# and the ones next to them. It is anchored at the start of the line, so a
# comment or a string that mentions `if` is not one.
GUARD = re.compile(
    r"^(?:\}\s*)?(?:else\s+)?(?:"
    r"(?:if|elif|unless|guard|ensure|require|raise|throw|except|catch)\b|"
    r"(?:debug_)?assert(?:_\w+)?(?:!|\b)|panic!|unreachable!|"
    r"return\s+(?:Err|None|false|null|nil|undefined)\b)"
)
ASSERTION = re.compile(
    r"\b(?:assert(?:_\w+|[A-Z]\w*)?(?:!|\b)|(?:expect|should)\b)|\.(?:expect|unwrap)\("
)


def kind(path):
    """ "test", "intent" (a document), "source" or "other", from the path alone."""
    parts = path.lower().split("/")
    name = parts[-1]
    if any(p in TEST_DIRS for p in parts[:-1]) or TEST_NAME.search(name):
        return "test"
    if name.endswith(DOC_SUFFIXES) or any(p in DOC_DIRS for p in parts[:-1]):
        return "intent"
    return "source" if name.endswith(SOURCE_SUFFIXES) else "other"


def signals(files):
    """The paths by kind, and the removed checks, of a diffmodel reading's files."""
    out = {"source": [], "test": [], "intent": [], "other": []}
    removed, tests = [], {"removed": [], "added": 0}
    for item in files:
        if item.get("binary"):
            continue
        what = kind(item["path"])
        out[what].append(item["path"])
        rows = [row for hunk in item["hunks"] for row in hunk["lines"]]
        added = {row["t"].strip() for row in rows if row["k"] == "a"}
        deleted = {row["t"].strip() for row in rows if row["k"] == "d"}
        for row in rows:
            text = row["t"].strip()
            if what == "source" and row["k"] == "d" and GUARD.match(text):
                # The same text added back is a move or a reformat, not a loss.
                if text not in added:
                    removed.append(
                        {"path": item["path"], "line": row["o"], "text": text}
                    )
            if what == "test" and ASSERTION.search(text):
                # A line removed and added back is a move: it is neither lost
                # nor new, so both counts leave it out.
                if row["k"] == "a":
                    if text not in deleted:
                        tests["added"] += 1
                elif row["k"] == "d" and text not in added:
                    tests["removed"].append(
                        {"path": item["path"], "line": row["o"], "text": text}
                    )
    out["removed_checks"] = removed
    out["removed_assertions"] = tests["removed"]
    out["added_assertions"] = tests["added"]
    return out


def cite(path, line):
    return {"path": path, "side": "old", "start": line, "end": line}


def plural(n, one, many=None):
    return f"{n} {one if n == 1 else many or one + 's'}"


def lens(found, checks=None, reflection=None, trusted=True):
    """The three layers as questions and observations for the contributor.

    `found` is signals(); `checks` the deterministic checks on the core answer
    (understanding.py), and `reflection` the contributor's own recorded
    assessment, if any. Each layer is {"asks", "signals"}; a signal is
    {"text"} with an optional "cites" list of citations into the old file. With
    `trusted` false (the diff could not be read line by line) no signal points at
    a line, and the Code layer says its observations may be incomplete.
    """
    changed = bool(found["source"])
    code = [
        {
            "text": f"{plural(len(found['source']), 'source file')} and "
            f"{plural(len(found['test']), 'test file')} changed."
        }
    ]
    removed = found["removed_checks"]
    if removed:
        code.append(
            {
                "text": f"{plural(len(removed), 'removed line')} "
                f"{'looks' if len(removed) == 1 else 'look'} like a check or an "
                "early exit. What did each one guard against? (A heuristic: it reads "
                "the first word of the line.)",
                "cites": [cite(r["path"], r["line"]) for r in removed[:MAX_CHECKS]],
            }
        )
    people = []
    if checks and checks.get("grounded") is True:
        people.append({"text": "Your explanation names code from the change."})
    elif checks and checks.get("grounded") is False:
        people.append(
            {"text": "Your explanation does not yet name code from the change."}
        )
    grade = (reflection or {}).get("grade")
    says = {
        "got": "You judged that you covered the reference.",
        "partly": "You judged that you covered part of the reference.",
        "missed": "You judged that you missed the reference or do not know yet.",
    }
    if grade in says:
        people.append({"text": says[grade]})
        if (reflection or {}).get("correction", "").strip():
            people.append(
                {"text": "A gap in understanding is recorded, in your words."}
            )
    else:
        people.append(
            {
                "text": "Comparing the reference with your explanation records what you missed."
            }
        )
    intent = []
    if changed and not found["test"]:
        intent.append(
            {
                "text": "No changed test file was recognized (the page tells tests "
                "apart by path). Which check, existing or new, would fail without "
                "this behavior?"
            }
        )
    if (
        found["removed_assertions"]
        and len(found["removed_assertions"]) > found["added_assertions"]
    ):
        gone = found["removed_assertions"]
        intent.append(
            {
                "text": f"{plural(len(gone), 'assertion-like line')} removed from tests and "
                f"{found['added_assertions']} added. What did they pin?",
                "cites": [cite(r["path"], r["line"]) for r in gone[:MAX_TESTS]],
            }
        )
    if changed and not found["intent"]:
        intent.append(
            {
                "text": "No document, spec or decision record changed. Where will the "
                "why live after this session?"
            }
        )
    if not intent:
        # Only a change with source, a test and a document reaches here; with no
        # source there is nothing to say was or was not pinned.
        intent.append(
            {
                "text": "A test and a document changed with the code."
                if changed
                else "No source file changed here, so there is nothing to pin or record."
            }
        )
    layers = {
        "code": {
            "asks": "Does this make the system harder to change?",
            "signals": code,
        },
        "people": {
            "asks": "Can the team still reason about this change without you?",
            "signals": people,
        },
        "intent": {
            "asks": "Will the next person, or agent, know what this is for?",
            "signals": intent,
        },
    }
    if not trusted:
        # A line the page cannot show is not one to point at.
        for layer in layers.values():
            for signal in layer["signals"]:
                signal.pop("cites", None)
        layers["code"]["signals"].insert(
            0,
            {
                "text": "The diff could not be read line by line, so these "
                "observations may be incomplete."
            },
        )
    return layers
