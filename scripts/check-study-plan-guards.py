#!/usr/bin/env python3
"""Prove every study-plan guard refuses a malformed response.

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
        assert gen.plan_sections(plan([whole])) == [{"topic": "All", "slugs": slugs}]
        sections_for(plan([whole]))
    except Exception as error:  # noqa: BLE001
        failures.append(f"valid plan: {type(error).__name__}: {error}")

    if failures:
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1

    print(f"check-study-plan-guards: {len(rejected)} bad plans rejected")
    return 0


if __name__ == "__main__":
    sys.exit(main())
