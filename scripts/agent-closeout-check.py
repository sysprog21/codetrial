#!/usr/bin/env python3
"""Say whether a claim that the gate passed has a receipt behind it.

`scripts/test.sh` writes a receipt per lane (verdict, skipped count, tree
fingerprint) under the git directory; this compares them with the tree now.

    agent-closeout-check.py --receipts   # print the status, for a closeout
    agent-closeout-check.py              # a Claude Code Stop hook

As a Stop hook it prints a `systemMessage` when the work tree has changes no
passing receipt covers. It never blocks and always exits 0. A receipt shows a
run happened on this tree, not that it was complete; CI recomputes the verdict.
"""

import contextlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ALL_LANES = {"rust", "portable", "browser"}


def git(*args):
    done = subprocess.run(
        ["git", *args], cwd=ROOT, capture_output=True, text=True, check=False
    )
    return done.stdout.strip() if done.returncode == 0 else None


def receipts_folder():
    common = git("rev-parse", "--git-common-dir") or ".git"
    return (ROOT / common / "codetrial-review" / "gate-receipts").resolve()


def receipts():
    found = []
    for path in sorted(receipts_folder().glob("*.json")):
        try:
            found.append(json.loads(path.read_text()))
        except (OSError, ValueError):
            continue
    return found


def tree():
    done = subprocess.run(
        [str(ROOT / "scripts" / "tree-fingerprint.sh")],
        capture_output=True,
        text=True,
        check=False,
    )
    return done.stdout.strip() if done.returncode == 0 else None


def status(now=None):
    """(covered, text): whether passing receipts name this exact tree."""
    now = now or tree()
    if now is None:
        return False, "gate receipts: not a git work tree, nothing to match"
    mine = [r for r in receipts() if r.get("tree") == now]
    passed = [r for r in mine if r.get("passed") is True]
    failed = [r for r in mine if r.get("passed") is False]
    if not mine:
        return False, (
            "gate receipts: none matches this tree, so do not report the gate as "
            "passed; run ./scripts/test.sh or say it was not run"
        )
    have = {r["lane"] for r in passed}
    ran = ", ".join(f"{r['lane']} ({r.get('skipped', 0)} skipped)" for r in passed)
    # Lane receipts are overwritten per lane, so a pass and a failure coexist
    # only as `all` against a single lane: the later run is the one that counts, and a tie is not
    # given to the pass.
    all_ran_at = min(
        (r.get("time", "") for r in passed if r["lane"] == "all"), default=""
    )
    superseded = any(r.get("time", "") >= all_ran_at for r in failed if "all" in have)
    if (ALL_LANES <= have or "all" in have) and not superseded:
        note = (
            "; a skipped check is a check that did not run: say which, from the "
            "run's own summary"
            if any(r.get("skipped") for r in passed)
            else ""
        )
        return True, f"gate receipts: passed on this tree: {ran}{note}"
    parts = [f"passed on this tree: {ran}"] if passed else []
    if failed:
        names = "; ".join(f"{r['lane']}: {r.get('failed', '')}" for r in failed)
        parts.append(f"the last run on this tree failed ({names})")
    missing = ALL_LANES - have - {r["lane"] for r in failed}
    if missing and "all" not in have:
        parts.append(f"not yet run here: {', '.join(sorted(missing))}")
    return False, (
        "gate receipts: " + "; ".join(parts) + ", so do not report the gate as passed"
    )


def main(argv):
    if "--receipts" in argv:
        print(status()[1])
        return 0
    try:
        event = json.load(sys.stdin)
    except ValueError:
        return 0
    if event.get("stop_hook_active"):
        return 0
    if not git("status", "--porcelain"):
        return 0
    now = tree()
    covered, text = status(now)
    # Once per tree: a reminder that repeats every turn is read as noise, and
    # the next edit changes the tree and earns another.
    marker = receipts_folder() / ".reminded"
    reminded = marker.read_text().strip() if marker.exists() else ""
    if not covered and now and reminded != now:
        with contextlib.suppress(OSError):  # a reminder is not worth a hook error
            marker.parent.mkdir(parents=True, exist_ok=True)
            marker.write_text(now + "\n")
        print(
            json.dumps(
                {
                    "systemMessage": text
                    + ". Closeout: experience-close.sh records this invocation."
                }
            )
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
