#!/usr/bin/env python3
"""Contract tests for scripts/scope-label.py against a stub `gh`."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "scope-label.py"

# Answers the read of every pull request from $PAGES_FILE (failing when $PAGES
# says so), the later files of a pull request from $FILES_FILE (failing when
# $FILES says so), the label list from $LABELS_FILE, and records every write in
# $CALLS. A write to pull request $WRITE_FAIL fails.
STUB = """#!/bin/sh
case "$*" in
*graphql*)
  [ "$PAGES" = fail ] && { echo "gh: HTTP 502" >&2; exit 1; }
  cat "$PAGES_FILE" ;;
*/files*)
  [ "$FILES" = fail ] && { echo "gh: HTTP 502 files" >&2; exit 1; }
  cat "$FILES_FILE" ;;
"api -X DELETE "*|*"labels[]="*)
  case "$*" in *"/issues/$WRITE_FAIL/"*) echo "gh: HTTP 403 write" >&2; exit 1 ;; esac
  echo "$*" >> "$CALLS" ;;
*/labels*) cat "$LABELS_FILE" ;;
esac
"""


def load():
    import importlib.util

    spec = importlib.util.spec_from_file_location("scope_label", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


scope = load()


def pr(number=1, paths=(), labels=(), total=None):
    return {
        "number": number,
        "labels": {"nodes": [{"name": name} for name in labels]},
        "files": {
            "totalCount": len(paths) if total is None else total,
            "nodes": [{"path": path} for path in paths],
        },
    }


def sweep(
    prs,
    *args,
    pages="ok",
    files="ok",
    later=(),
    labels=None,
    write_fail="",
    per_page=50,
):
    """Run the script on PRS, PER_PAGE to a page as --paginate prints them
    back to back; (exit code, stdout, stderr, writes)."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        (tmp / "gh").write_text(STUB)
        (tmp / "gh").chmod(0o755)
        chunks = [prs[i : i + per_page] for i in range(0, len(prs), per_page)] or [[]]
        text = ""
        for i, chunk in enumerate(chunks):
            more = i < len(chunks) - 1
            page = {
                "data": {
                    "repository": {
                        "pullRequests": {
                            "pageInfo": {
                                "hasNextPage": more,
                                "endCursor": f"c{i}" if more else None,
                            },
                            "nodes": chunk,
                        }
                    }
                }
            }
            text += json.dumps(page) + "\n"
        (tmp / "pages.json").write_text(text)
        (tmp / "files.txt").write_text("".join(f"{path}\n" for path in later))
        present = scope.LABELS if labels is None else labels
        (tmp / "labels.txt").write_text("".join(f"{name}\n" for name in present))
        env = dict(
            os.environ,
            PATH=f"{tmp}{os.pathsep}{os.environ['PATH']}",
            PAGES=pages,
            FILES=files,
            PAGES_FILE=str(tmp / "pages.json"),
            FILES_FILE=str(tmp / "files.txt"),
            LABELS_FILE=str(tmp / "labels.txt"),
            CALLS=str(tmp / "calls"),
            WRITE_FAIL=write_fail,
            SCOPE_LABEL_PAUSE="0",
        )
        result = subprocess.run(
            [sys.executable, str(SCRIPT), *args, "o/r"],
            capture_output=True,
            text=True,
            env=env,
            check=False,
        )
        calls = tmp / "calls"
        writes = calls.read_text().splitlines() if calls.exists() else []
        return result.returncode, result.stdout, result.stderr, writes


class Areas(unittest.TestCase):
    def test_a_narrower_rule_wins_over_the_broader_one(self):
        self.assertEqual(scope.area("src/web/token.rs"), "area: livekit")
        self.assertEqual(scope.area("src/web/auth.rs"), "area: backend")
        self.assertEqual(scope.area("web/judges/x.json"), "area: problem bank")
        for path in (
            "web/problem-pages.json",
            "web/problem-topics.json",
            "web/problems/two-sum.json",
        ):
            self.assertEqual(scope.area(path), "area: problem bank", path)
        self.assertEqual(scope.area("web/app.js"), "area: frontend")
        self.assertEqual(
            scope.area("src/agent/problem_variants.rs"), "area: problem bank"
        )
        self.assertEqual(scope.area("src/agent/report.rs"), "area: interviewer")
        self.assertEqual(scope.area("src/runtime.rs"), "area: backend")

    def test_a_path_no_rule_matches_adds_nothing(self):
        for path in (
            "tests/web/contract.rs",
            "tests/golden/task-posed.json",
            "Cargo.lock",
        ):
            self.assertIsNone(scope.area(path), path)

    def test_every_rule_is_reachable(self):
        for i, (rule, label) in enumerate(scope.AREAS):
            shadow = [r for r, _ in scope.AREAS[:i] if rule.startswith(r)]
            self.assertEqual(shadow, [], f"{rule} sits below {shadow}")


class ScopeLabel(unittest.TestCase):
    def test_the_areas_of_the_files_are_added_together(self):
        code, out, _, writes = sweep(
            [pr(7, ["problem-bank/judges.json", "web/app.js"])]
        )
        self.assertEqual(code, 0)
        self.assertEqual(out, "#7: +area: frontend +area: problem bank\n")
        self.assertEqual(len(writes), 1)
        self.assertIn("labels[]=area: problem bank", writes[0])
        self.assertIn("labels[]=area: frontend", writes[0])

    def test_a_label_already_right_writes_nothing(self):
        code, out, _, writes = sweep([pr(paths=["docs/x.md"], labels=["area: docs"])])
        self.assertEqual((code, out, writes), (0, "", []))

    def test_an_area_no_longer_touched_comes_off_and_other_labels_stay(self):
        before = ["area: frontend", "area: docs", "rust", "ready for maintainer review"]
        code, out, _, writes = sweep([pr(3, ["docs/x.md"], before)])
        self.assertEqual(code, 0)
        self.assertEqual(out, "#3: -area: frontend\n")
        self.assertEqual(
            writes,
            ["api -X DELETE repos/o/r/issues/3/labels/area%3A%20frontend --silent"],
        )

    def test_files_past_the_first_hundred_are_read_too(self):
        first = ["tests/t.rs"] * 100
        code, out, _, _ = sweep([pr(2, first, total=101)], later=[*first, "desktop/x"])
        self.assertEqual((code, out), (0, "#2: +area: desktop\n"))

    def test_pull_requests_past_the_first_page_are_labelled_too(self):
        prs = [pr(1, ["docs/x.md"]), pr(2, ["web/app.js"]), pr(3, ["desktop/x"])]
        code, out, _, _ = sweep(prs, per_page=2)
        self.assertEqual(code, 0)
        self.assertEqual(
            out, "#1: +area: docs\n#2: +area: frontend\n#3: +area: desktop\n"
        )

    def test_a_file_list_the_api_cut_short_leaves_the_labels_alone(self):
        first = ["tests/t.rs"] * 100
        listed = [*first, "desktop/x"] * 15
        code, out, _, writes = sweep(
            [pr(4, first, ["area: web", "area: docs"], total=4000)], later=listed
        )
        self.assertEqual(code, 0)
        self.assertEqual(out, "#4: skip (more files than the API lists)\n")
        self.assertEqual(writes, [])

    def test_a_path_with_a_space_stays_one_path(self):
        first = ["tests/t.rs"] * 100
        code, out, _, _ = sweep(
            [pr(5, first, total=101)], later=[*first, "notes src/agent/x.rs"]
        )
        self.assertEqual((code, out), (0, ""))

    def test_dry_run_reports_without_writing_or_reading_labels(self):
        code, out, _, writes = sweep([pr(paths=["docs/x.md"])], "--dry-run", labels=[])
        self.assertEqual((code, out, writes), (0, "#1: +area: docs\n", []))

    def test_a_missing_label_stops_the_run_before_any_write(self):
        code, _, err, writes = sweep(
            [pr(paths=["docs/x.md"])], labels=["area: frontend"]
        )
        self.assertEqual((code, writes), (2, []))
        self.assertIn("area: docs", err)

    def test_a_failed_read_writes_nothing(self):
        code, _, err, writes = sweep([pr(paths=["docs/x.md"])], pages="fail")
        self.assertEqual((code, writes), (2, []))
        self.assertIn("502", err)

    def test_a_failed_read_of_the_later_files_writes_nothing(self):
        first = ["tests/t.rs"] * 100
        code, _, err, writes = sweep(
            [pr(6, first, ["area: desktop"], total=101)], files="fail"
        )
        self.assertEqual((code, writes), (2, []))
        self.assertIn("502 files", err)

    def test_a_failed_write_still_lets_the_others_through(self):
        prs = [pr(1, ["docs/x.md"]), pr(2, ["web/app.js"])]
        code, _, err, writes = sweep(prs, write_fail="1")
        self.assertEqual(code, 2)
        self.assertEqual(len(writes), 1)
        self.assertIn("issues/2/labels", writes[0])
        self.assertIn("403", err)


if __name__ == "__main__":
    unittest.main()
