#!/usr/bin/env python3
"""Generate web/problems.js from the top-level problem bank."""

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
OUTPUT = ROOT / "web" / "problems.js"
MANIFEST = ROOT / "scripts" / "top-interview-150.json"
CACHE = ROOT / "problem-bank" / "leetcode"
ENDPOINT = "https://leetcode.com/graphql"
LIST_ENDPOINT = "https://leetcode.com/api/problems/all/"

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
    parser.add_argument("--check", action="store_true", help="fail if web/problems.js is stale")
    parser.add_argument("--fetch", action="store_true", help="fetch LeetCode metadata into problem-bank/leetcode")
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--delay-ms", type=int, default=250)
    parser.add_argument("--force", action="store_true")
    return parser.parse_args()


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
    request = urllib.request.Request(url, data=data, headers=headers, method="POST" if data else "GET")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:
        raise RuntimeError(f"{url} returned {error.code}") from error


def read_manifest() -> list[str]:
    manifest = read_json(MANIFEST)
    slugs = [slug for section in manifest["sections"] for slug in section["slugs"]]
    if len(slugs) != 150:
        raise RuntimeError(f"expected 150 slugs, got {len(slugs)}")
    if len(set(slugs)) != len(slugs):
        raise RuntimeError("duplicate slugs in manifest")
    return slugs


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
    body = request_json(ENDPOINT, body={"query": DETAIL_QUERY, "variables": {"titleSlug": slug}})
    detail = body.get("data", {}).get("question")
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
    print(json.dumps({"requested": len(wanted), "fetched": fetched, "cached": cached, "skipped": skipped, "cacheDir": str(CACHE)}, indent=2))


def generated() -> str:
    problems = read_json(SOURCE)
    return "\n".join(
        [
            f"export const problems = {json.dumps(problems, indent=2)};",
            "",
            "export function getProblem(id) {",
            "  return problems.find((problem) => problem.id === id) || problems.find((problem) => problem.id === \"two-sum\");",
            "}",
            "",
        ]
    )


def main() -> int:
    args = parse_args()
    if args.fetch:
        fetch(args.limit, args.delay_ms, args.force)

    output_text = generated()
    if args.check:
        if OUTPUT.read_text() != output_text:
            print("web/problems.js is stale; run: python3 scripts/gen-problems.py", file=sys.stderr)
            return 1
        return 0
    OUTPUT.write_text(output_text)
    print(f"updated {OUTPUT.relative_to(ROOT)} from {SOURCE.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
