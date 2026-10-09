"""Strict task sidecars; shared by instructor tooling and contract tests."""

from __future__ import annotations

import re
from copy import deepcopy

from .bank import ROOT, named, read_json
from .rules import posed

PROMPT_VERSION = 1
RUBRIC_PROFILE = "task-engagement-v1"
PROMPT_VARIABLES = {"title", "language", "target", "phase"}
DEFAULTS = read_json(ROOT / "problem-bank/task-defaults.json")
OMITTED = object()
DEFAULT_CHECKS = DEFAULTS["understandingChecks"]
DEFAULT_PROMPT = DEFAULTS["interactionPrompt"]
SIDECAR_KEYS = {
    "promptVersion",
    "interactionPrompt",
    "completionTargets",
    "understandingChecks",
    "requiredCases",
    "maxHintRungs",
    "rubricProfile",
}
IDENTIFIER = re.compile(r"[a-z][a-z0-9-]{0,63}\Z")
VARIABLE = re.compile(r"\{\{([^{}]*)\}\}")


def hint_ceiling(variant: dict) -> int:
    """The most hint rungs a task may allow: the default, or a shorter ladder."""
    return min(DEFAULTS["maxHintRungs"], len(variant["hints"]))


def first_time(seen: set, value, message: str) -> None:
    """Record `value`, refusing it with `message` if it was seen before."""
    if value in seen:
        raise ValueError(message)
    seen.add(value)


def exact_object(value: object, keys: set[str], where: str) -> dict:
    if not isinstance(value, dict) or set(value) != keys:
        raise ValueError(f"{where}: expected exactly {sorted(keys)}")
    return value


def text(value: object, where: str, limit: int = 1024) -> str:
    if not named(value) or len(value.encode("utf-8")) > limit:
        raise ValueError(f"{where}: expected nonblank text of at most {limit} bytes")
    return value


def identifier(value: object, where: str) -> str:
    if not isinstance(value, str) or not IDENTIFIER.fullmatch(value):
        raise ValueError(f"{where}: invalid identifier")
    return value


def rows(value: object, where: str, minimum: int, maximum: int) -> list:
    if not isinstance(value, list) or not minimum <= len(value) <= maximum:
        raise ValueError(f"{where}: expected {minimum}..{maximum} entries")
    return value


def validated_sidecar(
    problem: dict, judge: dict, variant: dict, sidecar=OMITTED
) -> dict:
    """Resolve labels exactly; omission selects defaults, never repairs bad input."""
    if sidecar is OMITTED:
        sidecar = {
            "promptVersion": PROMPT_VERSION,
            "interactionPrompt": DEFAULT_PROMPT,
            "completionTargets": [],
            "understandingChecks": deepcopy(DEFAULT_CHECKS),
            "requiredCases": [],
            "maxHintRungs": hint_ceiling(variant),
            "rubricProfile": RUBRIC_PROFILE,
        }
    exact_object(sidecar, SIDECAR_KEYS, "sidecar")
    if (
        type(sidecar["promptVersion"]) is not int
        or sidecar["promptVersion"] != PROMPT_VERSION
    ):
        raise ValueError("unknown promptVersion")
    if sidecar["rubricProfile"] != RUBRIC_PROFILE:
        raise ValueError("unknown rubricProfile")
    prompt = text(
        sidecar["interactionPrompt"], "interactionPrompt", DEFAULTS["promptBytes"]
    )
    if any(variable not in PROMPT_VARIABLES for variable in VARIABLE.findall(prompt)):
        raise ValueError("unknown prompt variable")
    remainder = VARIABLE.sub("VARIABLE", prompt)
    if "{{" in remainder or "}}" in remainder:
        raise ValueError("malformed prompt variable")
    hint_limit = sidecar["maxHintRungs"]
    if type(hint_limit) is not int or not 0 <= hint_limit <= (hint_ceiling(variant)):
        raise ValueError("maxHintRungs exceeds the variant ladder")
    shipped, _ = posed(problem, judge, variant)
    # The starter is the attempt's first revision, held to the runtime's
    # capture limit.
    python_starter = shipped.get("starterCode", {}).get("python", "")
    if len(python_starter.encode("utf-8")) > DEFAULTS["codeBytes"]:
        raise ValueError("starter exceeds the code byte limit")
    seen = set()
    markers = set()
    for target in rows(sidecar["completionTargets"], "completionTargets", 0, 8):
        exact_object(target, {"id", "language", "marker", "goal"}, "completionTarget")
        target_id = identifier(target["id"], "completionTarget.id")
        first_time(seen, target_id, "duplicate completion target id")
        if target["language"] != DEFAULTS["language"]:
            raise ValueError("completion target language must be python")
        marker = text(target["marker"], "marker", 128)
        if not re.fullmatch(r"[A-Z][A-Z0-9_]*", marker):
            raise ValueError("invalid completion marker")
        starter = shipped.get("starterCode", {}).get(target["language"], "")
        if (
            marker in markers
            or len(re.findall(rf"\b{re.escape(marker)}\b", starter)) != 1
        ):
            raise ValueError("missing or duplicated completion marker")
        markers.add(marker)
        text(target["goal"], "goal")
    seen = set()
    for check in rows(
        sidecar["understandingChecks"],
        "understandingChecks",
        DEFAULTS["coreChecks"],
        DEFAULTS["coreChecks"],
    ):
        exact_object(check, {"id", "question"}, "understandingCheck")
        check_id = identifier(check["id"], "understandingCheck.id")
        first_time(seen, check_id, "duplicate check id")
        text(check["question"], "question")
    labels = [case["label"] for case in judge["cases"]]
    required = rows(sidecar["requiredCases"], "requiredCases", 0, 20)
    seen = set()
    for label in required:
        text(label, "requiredCases label")
        if labels.count(label) != 1:
            raise ValueError("unresolved, ambiguous or repeated required case")
        first_time(seen, label, "unresolved, ambiguous or repeated required case")
    return deepcopy(sidecar)


def validated_sidecars(
    problems: list[dict], judges: dict, variants: dict, sidecars: object
) -> dict:
    """Reject orphan customizations, including unknown top-level task ids."""
    if not isinstance(sidecars, dict):
        raise ValueError("sidecars must be an object")
    ids = {problem["id"] for problem in problems}
    if set(sidecars) - ids:
        raise ValueError("unknown task id in sidecars")
    return {
        problem["id"]: validated_sidecar(
            problem,
            judges[problem["id"]],
            variants[problem["id"]],
            sidecars[problem["id"]] if problem["id"] in sidecars else OMITTED,
        )
        for problem in problems
    }
