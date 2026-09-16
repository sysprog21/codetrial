"""The problem bank: where it lives, what it holds, and reading it back."""

from __future__ import annotations

import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


SOURCE = ROOT / "problem-bank" / "problems.json"


JUDGE_SOURCE = ROOT / "problem-bank" / "judges.json"


# The exercise a candidate is actually given. A candidate shown the practice
# problem as published recognises it and recites an answer, which is not what an
# interview measures, so the page carries a scenario written around the same
# contract and the source title, wording and constraints stay on the server.
VARIANT_SOURCE = ROOT / "problem-bank" / "variants.json"


# Solution notes for the report reviewer, with the license they came under.
GUIDE_SOURCE = ROOT / "problem-bank" / "guides.json"


# One file per problem, fetched on demand, rather than one module holding all
# 150. Two reasons, and the second is the important one:
#
#   - A candidate downloaded 288 KB of statements and 215 KB of judges to work
#     on one problem, parsed as JavaScript source rather than as JSON.
#   - The judge holds the expected output of every test case. Shipping all of
#     them handed a candidate the answers to the other 149 problems along with
#     their own. Their own is still there, because the runner is in the browser;
#     this bounds the exposure rather than closing it. See the note on
#     `apply_test_results` in src/agent.rs for where that boundary is drawn.
OUTPUT_DIR = ROOT / "web" / "problems"


JUDGE_OUTPUT_DIR = ROOT / "web" / "judges"


PAGE_MAP_OUTPUT = ROOT / "web" / "problem-pages.json"


# The exercise a link naming nothing the bank has opens, `DEFAULT_PROBLEM_ID` in
# src/agent/problems.rs. Marked in the page map rather than written into the
# loader, which every page serves: the loader would otherwise carry a published
# id on every load.
DEFAULT_PROBLEM_ID = "two-sum"


DIRS = (OUTPUT_DIR, JUDGE_OUTPUT_DIR)


RUST_TOPICS = ROOT / "src" / "agent" / "problem_topics.rs"


RUST_VARIANTS = ROOT / "src" / "agent" / "problem_variants.rs"


RUST_GUIDES = ROOT / "src" / "agent" / "problem_guides.rs"

RUST_RUBRICS = ROOT / "src" / "agent" / "problem_rubrics.rs"


REACTO_STAGES = ("repeat", "example", "algorithm", "coding", "test", "optimizations")


NEUTRAL_DIRECTIONS = {
    "repeat": "Ask the candidate to restate the inputs, outputs, constraints, and ambiguities.",
    "example": "Ask the candidate to choose and trace an ordinary example and a boundary case.",
    "algorithm": "Ask for the candidate's approach, correctness argument, and complexity.",
    "coding": "Ask the candidate to implement their stated approach and explain major decisions.",
    "test": "Ask the candidate to predict useful cases and expected results before running them.",
    "optimizations": "Ask for complexity, an uncovered edge case, and a justified optimization or cleanup.",
}


# No competencies: they are the topic tags, and "Dynamic Programming" beside
# the problem names the technique before the candidate has said a word.
PUBLIC_METADATA_KEYS = {
    "difficulty",
    "reactoStages",
    "followUpDirections",
}


VARIANT_KEYS = {
    "title",
    "brief",
    "contract",
    "examples",
    "clarifications",
    "followUps",
    "hints",
}


MANIFEST = ROOT / "scripts" / "top-interview-150.json"


CACHE = ROOT / "problem-bank" / "leetcode"


ENDPOINT = "https://leetcode.com/graphql"


LIST_ENDPOINT = "https://leetcode.com/api/problems/all/"


STUDY_PLAN_QUERY = """
query studyPlanV2Detail($planSlug: String!) {
  studyPlanV2Detail(planSlug: $planSlug) {
    planSubGroups {
      name
      questions {
        titleSlug
      }
    }
  }
}"""


DETAIL_QUERY = """
query questionData($titleSlug: String!) {
  question(titleSlug: $titleSlug) {
    questionFrontendId
    difficulty
    isPaidOnly
    codeSnippets {
      langSlug
      code
    }
    exampleTestcases
    metaData
  }
}"""


def named(value: object) -> bool:
    """True when a GraphQL field arrived as a usable, non-blank string."""
    return isinstance(value, str) and bool(value.strip())


def field(value: object, key: str) -> object:
    """One level into a GraphQL object, or None when the shape is wrong.

    A plain `.get` assumes every level really is an object. When one is not,
    the caller gets an AttributeError from deep inside a comprehension instead
    of the RuntimeError its guards were written to raise.
    """
    return value.get(key) if isinstance(value, dict) else None


def listed(value: object, key: str) -> list:
    """A list-valued GraphQL field, or empty when it is anything else.

    `field` alone is not enough for the two keys that get iterated: a scalar
    there means `for x in 5`, which is a TypeError raised from inside a
    comprehension rather than the guard the caller documented. Empty is the
    same answer null already gave, so a wrong-typed field fails the slug checks
    below rather than inventing a third behaviour.
    """
    found = field(value, key)
    return found if isinstance(found, list) else []


def read_json(path: Path) -> object:
    return json.loads(path.read_text())


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"{json.dumps(value, indent=2)}\n")


def repeated(items: list) -> list:
    return sorted({item for item in items if items.count(item) > 1})


def invalid_origins(problems: object) -> list[str]:
    """Problem ids whose origin is neither imported nor authored here."""
    if not isinstance(problems, list):
        return []
    return sorted(
        problem.get("id", "?")
        for problem in problems
        if not isinstance(problem, dict)
        or problem.get("origin", "leetcode") not in {"leetcode", "original"}
    )


# Which judge argType a LeetCode metaData type needs, derived from the entries
# already in judges.json. `Node` is deliberately absent: the same name covers
# graph nodes, next-pointer trees, random-pointer lists and quad trees, and only
# the statement says which, so a scaffold that guessed would be worse than one
# that says it does not know.
ARG_TYPES = {
    "ListNode": "linkedList",
    "ListNode[]": "linkedListArray",
    "TreeNode": "binaryTree",
}


# LeetCode's language slug for each language this repo ships a starter for.
STARTER_LANGS = {
    "python": "python3",
    "javascript": "javascript",
    "c": "c",
    "cpp": "cpp",
    "java": "java",
}


def validated_problems(source: Path = SOURCE) -> list[dict]:
    problems = read_json(source)
    if not isinstance(problems, list):
        raise RuntimeError("problem bank must be a JSON array")
    invalid = invalid_origins(problems)
    if invalid:
        raise RuntimeError(f"problem bank has invalid origins: {invalid}")
    seen: set[str] = set()
    for problem in problems:
        problem_id = problem.get("id") if isinstance(problem, dict) else None
        if not named(problem_id) or problem_id in seen:
            raise RuntimeError(f"missing or duplicate problem id: {problem_id!r}")
        seen.add(problem_id)
        origin = problem.get("origin", "leetcode")
        if origin == "original" and ("title" in problem or "examples" in problem):
            raise RuntimeError(
                f"{problem_id}: an original problem has no published title or examples"
            )
        if origin == "leetcode" and not named(problem.get("title")):
            raise RuntimeError(
                f"{problem_id}: an imported problem needs its published title"
            )
        for field_name in ("summary", "optimal", "pitfalls"):
            if not named(problem.get(field_name)):
                raise RuntimeError(f"{problem_id}: missing rubric {field_name}")
        if problem.get("difficulty") not in {"Easy", "Medium", "Hard"}:
            raise RuntimeError(f"{problem_id}: unknown difficulty")
        topics = problem.get("topics")
        if not isinstance(topics, list) or not 1 <= len(topics) <= 8:
            raise RuntimeError(f"{problem_id}: topics must contain 1..8 values")
        cleaned = [topic.strip() for topic in topics if named(topic)]
        if len(cleaned) != len(topics) or len(set(cleaned)) != len(cleaned):
            raise RuntimeError(f"{problem_id}: topics must be non-empty and unique")
    return problems


def public_metadata(problem: dict) -> dict:
    return {
        "difficulty": problem["difficulty"],
        "reactoStages": list(REACTO_STAGES),
        "followUpDirections": [
            {"stage": stage, "direction": NEUTRAL_DIRECTIONS[stage]}
            for stage in REACTO_STAGES
        ],
    }


def compact(value: object) -> str:
    return json.dumps(value, separators=(",", ":"))
