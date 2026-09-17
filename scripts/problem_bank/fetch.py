"""Refreshing the bank from LeetCode: the study plan, details and scaffolds."""

from __future__ import annotations

import json
import sys
import time
import urllib.error
import urllib.request

from .bank import (
    ARG_TYPES,
    CACHE,
    DETAIL_QUERY,
    ENDPOINT,
    LIST_ENDPOINT,
    MANIFEST,
    SOURCE,
    STARTER_LANGS,
    STUDY_PLAN_QUERY,
    field,
    invalid_origins,
    listed,
    named,
    read_json,
    repeated,
    write_json,
)


def request_json(url: str, *, body: object | None = None) -> object:
    data = None
    headers = {
        "Accept": "application/json",
        "User-Agent": "Mozilla/5.0",
        "Referer": "https://leetcode.com/",
    }
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(
        url, data=data, headers=headers, method="POST" if data else "GET"
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:
        raise RuntimeError(f"{url} returned {error.code}") from error


def check_plan_slugs(slugs: list[str], source=SOURCE) -> None:
    """What any copy of the study plan has to satisfy to be usable here.

    The count is not checked: naming exactly the bank already fixes it, and a
    second literal 150 is a second thing to update. `tests/browser/
    leetcode-import.test.js` owns that number, because how many problems ship
    is a product decision rather than a consistency one.
    """
    unique = set(slugs)
    if len(unique) != len(slugs):
        raise RuntimeError(f"duplicate slugs in the plan: {repeated(slugs)}")
    problems = read_json(source)
    invalid = invalid_origins(problems)
    if invalid:
        raise RuntimeError(f"problem-bank has invalid origins: {invalid}")
    bank = {problem["id"] for problem in problems}
    imported = {
        problem["id"]
        for problem in problems
        if problem.get("origin", "leetcode") == "leetcode"
    }
    originals = bank - imported
    if unique != imported:
        raise RuntimeError(
            "plan diverges from problem-bank; port it first. "
            f"plan adds {sorted(unique - imported)}, plan drops {sorted(imported - unique)}; "
            f"originals {sorted(originals)} stay outside the plan"
        )


def read_manifest() -> list[str]:
    """The study-plan slugs, checked against the bank they are supposed to name.

    Three callers need the same answer, so the rule lives here rather than in
    each: `--fetch` walks these slugs, `--check` gates on them, and the sync
    holds a freshly downloaded plan to the same standard before overwriting the
    file. Before this, only the sync checked, and only when a human ran it with
    a network.
    """
    manifest = read_json(MANIFEST)
    slugs = [slug for section in manifest["sections"] for slug in section["slugs"]]
    check_plan_slugs(slugs)
    return slugs


def plan_sections(body: object) -> list[dict]:
    """The sections a study-plan response describes, or a RuntimeError saying why not.

    Pure, and separate from the sync for that reason: no network, no files, so
    the guards below can be exercised directly instead of through a function
    that would write the tracked manifest as a side effect of being tested.
    """
    errors = field(body, "errors")
    if errors:
        first = errors[0] if isinstance(errors, list) else errors
        raise RuntimeError(f"studyPlanV2Detail: {field(first, 'message') or first}")
    plan = field(field(body, "data"), "studyPlanV2Detail")
    if not plan:
        raise RuntimeError("LeetCode has no top-interview-150 plan")
    sections = []
    for group in listed(plan, "planSubGroups"):
        topic = field(group, "name")
        members = [
            field(question, "titleSlug") for question in listed(group, "questions")
        ]
        if not named(topic) or not members:
            raise RuntimeError(f"plan section is unnamed or empty: {topic!r}")
        if not all(named(slug) for slug in members):
            raise RuntimeError(f"plan section {topic!r} is missing a slug")
        sections.append({"topic": topic, "slugs": members})
    return sections


def sync_study_plan() -> None:
    """Refresh the manifest from the live study plan.

    Everything that can refuse runs before the write, because the manifest is
    checked in: a bad response has to leave the committed file alone rather
    than replace it with something the next reader reconstructs from history.
    """
    body = request_json(
        ENDPOINT,
        body={
            "query": STUDY_PLAN_QUERY,
            "variables": {"planSlug": "top-interview-150"},
        },
    )
    sections = plan_sections(body)
    slugs = [slug for section in sections for slug in section["slugs"]]
    check_plan_slugs(slugs)
    write_json(
        MANIFEST,
        {
            "source": "https://leetcode.com/studyplan/top-interview-150/",
            "sections": sections,
        },
    )
    print(f"synced {len(slugs)} problems in {len(sections)} sections")


def plan_drift() -> int:
    """Say how the published plan differs from what this repo holds."""
    body = request_json(
        ENDPOINT,
        body={
            "query": STUDY_PLAN_QUERY,
            "variables": {"planSlug": "top-interview-150"},
        },
    )
    listed = [slug for section in plan_sections(body) for slug in section["slugs"]]
    # A set would fold a repeated slug away and report a plan the sync then
    # refuses as in sync.
    if duplicates := repeated(listed):
        print(f"plan repeats: {', '.join(duplicates)}", file=sys.stderr)
        return 1
    live = set(listed)
    problems = read_json(SOURCE)
    invalid = invalid_origins(problems)
    if invalid:
        raise RuntimeError(f"problem-bank has invalid origins: {invalid}")
    bank = {
        problem["id"]
        for problem in problems
        if problem.get("origin", "leetcode") == "leetcode"
    }
    added, dropped = sorted(live - bank), sorted(bank - live)
    if not added and not dropped:
        print(f"in sync: {len(live)} problems")
        return 0
    for slug in added:
        print(f"plan adds:  {slug}")
    for slug in dropped:
        print(f"plan drops: {slug}")
    print(
        "\nport these first, then run --sync-study-plan. For each added slug:\n"
        "  python3 scripts/gen-problems.py --scaffold SLUG",
        file=sys.stderr,
    )
    return 1


def problem_title(slug: str, force: bool) -> str:
    body = cached_problem_list(force)
    for item in body["stat_status_pairs"]:
        if item["stat"]["question__title_slug"] == slug:
            return item["stat"]["question__title"]
    raise RuntimeError(f"{slug}: not in LeetCode's problem list")


def scaffold(slug: str, force: bool) -> int:
    """Print what LeetCode can supply for a problem this repo does not have.

    Not the whole entry, and it says so where it stops. The statement is left
    empty because the fetcher does not ask for LeetCode's prose, and the judge
    cases carry inputs without expected values because `exampleTestcases` is
    inputs only. Both have to be written by someone who has read the problem.
    """
    CACHE.mkdir(parents=True, exist_ok=True)
    _, detail = cached_detail(slug, force)
    if detail.get("isPaidOnly"):
        raise RuntimeError(f"{slug}: paid-only, so the runner cannot serve it")
    problem, judge = scaffold_entries(slug, problem_title(slug, force), detail)

    print(f"# problem-bank/problems.json entry for {slug}")
    print(json.dumps(problem, indent=2))
    print(f"\n# problem-bank/judges.json entry for {slug}")
    print(json.dumps(judge, indent=2))
    missing = ["statement", "examples", "constraints", "topics", *scaffold_gaps(judge)]
    print(
        f"\nstill yours to write: {', '.join(missing)}, an expected value for each "
        f"of the {len(judge['cases'])} cases, and the house starter comment, which "
        "LeetCode's snippets leave as an empty body.",
        file=sys.stderr,
    )
    return 0


def scaffold_gaps(judge: dict) -> list[str]:
    """The judge fields a scaffold cannot infer and a port author must add.

    Each is silent otherwise: the judge still loads, and grades the wrong thing.
    """
    gaps = []
    if None in judge["argTypes"] and "Node" in judge["paramTypes"]:
        gaps.append("argTypes for Node")
    if judge["returnType"] == "Node" and "outputType" not in judge:
        gaps.append("outputType for the Node it returns")
    # An in-place problem returns nothing, so the exact checker would compare
    # that nothing rather than the argument the candidate changed.
    if judge["returnType"] == "void":
        gaps.append("outputParam or outputPrefixParam naming the argument it changes")
    return gaps


def scaffold_entries(slug: str, title: str, detail: dict) -> tuple[dict, dict]:
    """The two bank entries a LeetCode detail can fill in on its own.

    Pure, so the shape can be checked without the network. What it leaves empty
    it leaves empty on purpose: the fetcher does not ask for LeetCode's prose,
    and `exampleTestcases` carries inputs without the expected values, so the
    statement and every `expected` are written by someone who read the problem.
    """
    meta = json.loads(detail["metaData"])
    snippets = {item["langSlug"]: item["code"] for item in detail["codeSnippets"]}
    params = meta.get("params", [])

    problem = {
        "id": slug,
        "title": title,
        "difficulty": detail["difficulty"],
        "topics": [],
        "statement": [],
        "examples": [],
        "constraints": [],
        "starterCode": {
            language: snippets[wanted]
            for language, wanted in STARTER_LANGS.items()
            if wanted in snippets
        },
    }

    # exampleTestcases is one value per line, the params in order, repeating.
    lines = detail["exampleTestcases"].splitlines()
    width = max(len(params), 1)
    cases = [
        {"input": [json.loads(value) for value in lines[at : at + width]]}
        for at in range(0, len(lines) - width + 1, width)
    ]
    judge = {
        "kind": "function",
        "entry": meta["name"],
        "checker": "exact",
        "argTypes": [ARG_TYPES.get(param["type"]) for param in params],
        "cases": cases,
        "paramNames": [param["name"] for param in params],
        "paramTypes": [param["type"] for param in params],
        "returnType": meta["return"]["type"],
    }
    output = ARG_TYPES.get(meta["return"]["type"])
    if output:
        judge["outputType"] = output

    return problem, judge


def cached_problem_list(force: bool) -> object:
    file = CACHE / "all-problems.json"
    if file.exists() and not force:
        return read_json(file)
    body = request_json(LIST_ENDPOINT)
    write_json(file, body)
    return body


def paid_slugs(force: bool) -> set[str]:
    body = cached_problem_list(force)
    return {
        item["stat"]["question__title_slug"]
        for item in body["stat_status_pairs"]
        if item["paid_only"]
    }


def cached_detail(slug: str, force: bool) -> tuple[bool, object]:
    file = CACHE / f"{slug}.json"
    if file.exists() and not force:
        return True, read_json(file)
    body = request_json(
        ENDPOINT, body={"query": DETAIL_QUERY, "variables": {"titleSlug": slug}}
    )
    detail = field(field(body, "data"), "question")
    if not detail:
        raise RuntimeError(f"{slug}: missing question detail")
    write_json(file, detail)
    return False, detail


def fetch(limit: int | None, delay_ms: int, force: bool) -> None:
    CACHE.mkdir(parents=True, exist_ok=True)
    slugs = read_manifest()
    wanted = slugs[:limit] if limit is not None else slugs
    paid = paid_slugs(force)
    fetched = cached = 0
    skipped: list[str] = []
    for index, slug in enumerate(wanted):
        if slug in paid:
            skipped.append(slug)
            continue
        was_cached, detail = cached_detail(slug, force)
        if detail.get("isPaidOnly"):
            skipped.append(slug)
            continue
        fetched += int(not was_cached)
        cached += int(was_cached)
        if not was_cached and index + 1 < len(wanted):
            time.sleep(delay_ms / 1000)
    print(
        json.dumps(
            {
                "requested": len(wanted),
                "fetched": fetched,
                "cached": cached,
                "skipped": skipped,
                "cacheDir": str(CACHE),
            },
            indent=2,
        )
    )
