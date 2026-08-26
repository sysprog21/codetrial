#!/usr/bin/env python3
"""Prove the study-plan tooling refuses bad input and scaffolds the right shape.

`plan_sections` and `check_plan_slugs` are what stand between a bad answer
from LeetCode and a rewritten `scripts/top-interview-150.json`, so what this
gate asserts is that each of them raises a RuntimeError naming itself, rather
than an AttributeError or a TypeError from somewhere inside a comprehension.

Two rules keep it honest. Every case is a whole valid plan with one thing
wrong, so the guard in the label is the one that can fire: cases small enough
to trip the slug checks as well proved nothing about the guard they were named
for. And every case pins the phrase its error carries, because several of
these responses would trip a later guard too, and "some RuntimeError" cannot
tell a working guard from a deleted one.

Deleting any guard in either function turns this gate red. Keep it that way.
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def load_generator() -> object:
    spec = importlib.util.spec_from_file_location(
        "gen_problems", ROOT / "scripts" / "gen-problems.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def plan(groups: object) -> dict:
    return {"data": {"studyPlanV2Detail": {"planSubGroups": groups}}}


def group(name: object, slugs: list[object]) -> dict:
    return {"name": name, "questions": [{"titleSlug": slug} for slug in slugs]}


def scaffold_failures(gen: object) -> list:
    """`--scaffold` turns a LeetCode detail into the two entries a human edits.

    What is checked is the mapping a human would otherwise redo by hand: the
    argTypes derived from metaData param types, and the flat exampleTestcases
    lines chunked back into one case per parameter tuple. An off-by-one there
    produces cases that look plausible and pair the wrong inputs.
    """
    detail = {
        "difficulty": "Medium",
        "codeSnippets": [
            {"langSlug": "python3", "code": "class Solution:\n    pass\n"},
            {"langSlug": "rust", "code": "// not a language this repo ships"},
        ],
        "exampleTestcases": "[1,2,3]\n2\n4\n[9]\n1\n1",
        "metaData": json.dumps(
            {
                "name": "reverseBetween",
                "params": [
                    {"name": "head", "type": "ListNode"},
                    {"name": "left", "type": "integer"},
                    {"name": "right", "type": "integer"},
                ],
                "return": {"type": "ListNode"},
            }
        ),
    }
    problem, judge = gen.scaffold_entries("some-slug", "Some Slug", detail)

    failures = []
    if judge["argTypes"] != ["linkedList", None, None]:
        failures.append(f"scaffold: argTypes came out {judge['argTypes']!r}")
    if judge["outputType"] != "linkedList":
        failures.append(f"scaffold: outputType came out {judge.get('outputType')!r}")
    if judge["cases"] != [
        {"input": [[1, 2, 3], 2, 4]},
        {"input": [[9], 1, 1]},
    ]:
        failures.append(f"scaffold: cases chunked as {judge['cases']!r}")
    if any("expected" in case for case in judge["cases"]):
        failures.append("scaffold: invented an expected value it cannot know")
    if problem["statement"] or problem["examples"]:
        failures.append("scaffold: filled in prose the fetcher never asked for")
    if list(problem["starterCode"]) != ["python"]:
        failures.append(f"scaffold: starters came out {list(problem['starterCode'])}")
    return failures


def main() -> int:
    gen = load_generator()
    slugs = gen.read_manifest()
    whole = group("All", slugs)

    # label -> (response, the phrase the raised error has to carry)
    rejected = {
        "non-dict body": ([], "no top-interview-150 plan"),
        "graphql errors": (
            {"errors": [{"message": "Cannot query field"}]},
            "Cannot query field",
        ),
        "non-dict graphql error": (
            {"errors": ["Cannot query field"]},
            "Cannot query field",
        ),
        "null plan": (
            {"data": {"studyPlanV2Detail": None}},
            "no top-interview-150 plan",
        ),
        "non-dict data": ({"data": []}, "no top-interview-150 plan"),
        "plan with no groups": (plan(None), "plan diverges from problem-bank"),
        "scalar planSubGroups": (plan(5), "plan diverges from problem-bank"),
        "string planSubGroups": (plan("abc"), "plan diverges from problem-bank"),
        "scalar questions": (
            plan([whole, {"name": "B", "questions": 7}]),
            "unnamed or empty",
        ),
        "non-dict group": (plan([whole, "Two Pointers"]), "unnamed or empty"),
        "null questions": (
            plan([whole, {"name": "B", "questions": None}]),
            "unnamed or empty",
        ),
        "empty questions": (plan([whole, group("B", [])]), "unnamed or empty"),
        "blank group name": (plan([group("  ", slugs)]), "unnamed or empty"),
        "null group name": (plan([group(None, slugs)]), "unnamed or empty"),
        "non-dict question": (
            plan([{"name": "A", "questions": [*slugs]}]),
            "missing a slug",
        ),
        "null titleSlug": (plan([group("A", [None, *slugs[1:]])]), "missing a slug"),
        "duplicate slugs": (
            plan([group("A", [*slugs, slugs[0]])]),
            "duplicate slugs in the plan",
        ),
        "plan drops a problem": (
            plan([group("A", slugs[:-1])]),
            "plan diverges from problem-bank",
        ),
        "plan adds a problem": (
            plan([group("A", ["not-a-problem", *slugs[1:]])]),
            "plan diverges from problem-bank",
        ),
    }

    def sections_for(body: object) -> None:
        """Everything sync_study_plan does to a response before it writes."""
        sections = gen.plan_sections(body)
        gen.check_plan_slugs([s for section in sections for s in section["slugs"]])

    failures = []
    for label, (body, phrase) in rejected.items():
        try:
            sections_for(body)
            failures.append(f"{label}: accepted the plan instead of raising")
        except RuntimeError as error:
            if phrase not in str(error):
                failures.append(
                    f"{label}: a different guard fired, wanted {phrase!r}, got {error}"
                )
        except Exception as error:  # noqa: BLE001 - the point is that this cannot happen
            failures.append(
                f"{label}: raised {type(error).__name__} rather than RuntimeError: {error}"
            )

    # The mirror: a plan that agrees with the bank has to survive all of it.
    try:
        sections = gen.plan_sections(plan([whole]))
        if sections != [{"topic": "All", "slugs": slugs}]:
            failures.append(f"valid plan: parsed as {sections!r}")
        sections_for(plan([whole]))
    except Exception as error:  # noqa: BLE001
        failures.append(f"valid plan: {type(error).__name__}: {error}")

    failures += scaffold_failures(gen)

    if failures:
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1

    print(f"check-study-plan-guards: {len(rejected)} bad plans rejected, scaffold ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
