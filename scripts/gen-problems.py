#!/usr/bin/env python3
"""Generate the per-problem statement and judge files the browser fetches."""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "problem-bank" / "problems.json"
JUDGE_SOURCE = ROOT / "problem-bank" / "judges.json"
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
DIRS = (OUTPUT_DIR, JUDGE_OUTPUT_DIR)
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


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--check", action="store_true", help="fail if the generated files are stale"
    )
    parser.add_argument(
        "--fetch",
        action="store_true",
        help="fetch LeetCode metadata into problem-bank/leetcode",
    )
    parser.add_argument(
        "--sync-study-plan",
        action="store_true",
        help="sync the Top Interview 150 manifest from LeetCode",
    )
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--delay-ms", type=int, default=250)
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()
    # --check is the CI gate and only reads. Both other modes reach the network
    # and write: sync rewrites the manifest, fetch fills the cache. Either one
    # ahead of --check has it report on staleness it just caused.
    if args.check and (args.fetch or args.sync_study_plan):
        parser.error("--check only reads; run it on its own")
    return args


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


def read_json(path: Path) -> object:
    return json.loads(path.read_text())


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"{json.dumps(value, indent=2)}\n")


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


def check_plan_slugs(slugs: list[str]) -> None:
    """What any copy of the study plan has to satisfy to be usable here.

    The count is not checked: naming exactly the bank already fixes it, and a
    second literal 150 is a second thing to update. `tests/browser/
    leetcode-import.test.js` owns that number, because how many problems ship
    is a product decision rather than a consistency one.
    """
    unique = set(slugs)
    if len(unique) != len(slugs):
        repeated = sorted({slug for slug in slugs if slugs.count(slug) > 1})
        raise RuntimeError(f"duplicate slugs in the plan: {repeated}")
    bank = {problem["id"] for problem in read_json(SOURCE)}
    if unique != bank:
        raise RuntimeError(
            "plan diverges from problem-bank; port it first. "
            f"plan adds {sorted(unique - bank)}, plan drops {sorted(bank - unique)}"
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
    # `or []` rather than a get() default: these keys come back present-and-null,
    # so a default never fires and `for x in None` raises TypeError instead of
    # the error the guards are written to report.
    sections = []
    for group in field(plan, "planSubGroups") or []:
        topic = field(group, "name")
        members = [
            field(question, "titleSlug") for question in field(group, "questions") or []
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
        fetched += 1
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


def generated() -> dict[Path, str]:
    """Every file this script owns, as path -> exact contents."""
    files: dict[Path, str] = {}
    for problem in read_json(SOURCE):
        files[OUTPUT_DIR / f"{problem['id']}.json"] = (
            json.dumps(problem, indent=2) + "\n"
        )
    for problem_id, judge in read_json(JUDGE_SOURCE).items():
        files[JUDGE_OUTPUT_DIR / f"{problem_id}.json"] = (
            json.dumps(judge, indent=2) + "\n"
        )
    return files


def orphans(files: dict[Path, str]) -> list[Path]:
    """Generated files this run would not write.

    A problem removed from the bank leaves its statement and its judge on disk
    otherwise, and for the judge that means an answer key still being served for
    a problem nobody can be given any more.
    """
    on_disk = (path for directory in DIRS for path in sorted(directory.glob("*.json")))
    return [path for path in on_disk if path not in files]


def drifted(files: dict[Path, str]) -> list[str]:
    """What `--check` reports, as repo-relative paths, in either direction."""
    changed = [
        str(path.relative_to(ROOT))
        for path, text in files.items()
        if not path.exists() or path.read_text() != text
    ]
    removed = [
        f"{path.relative_to(ROOT)} (no longer in the bank)" for path in orphans(files)
    ]
    return sorted(changed + removed)


def main() -> int:
    args = parse_args()
    if args.sync_study_plan:
        sync_study_plan()
    if args.fetch:
        fetch(args.limit, args.delay_ms, args.force)

    files = generated()
    if args.check:
        try:
            read_manifest()
        except RuntimeError as error:
            print(f"scripts/top-interview-150.json: {error}", file=sys.stderr)
            return 1
        stale = drifted(files)
        if stale:
            print(
                "stale generated files; run: python3 scripts/gen-problems.py",
                file=sys.stderr,
            )
            for path in stale:
                print(f"  {path}", file=sys.stderr)
            return 1
        return 0

    for directory in DIRS:
        directory.mkdir(parents=True, exist_ok=True)
    for path in orphans(files):
        path.unlink()
    for path, text in files.items():
        path.write_text(text)
    print(
        f"updated {len(files)} files under {' and '.join(str(d.relative_to(ROOT)) for d in DIRS)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
