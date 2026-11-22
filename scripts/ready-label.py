#!/usr/bin/env python3
"""Keep the "ready for maintainer review" label on the open pull requests whose
author has answered every review thread, and off every other one.

A pull request is ready when it is not a draft, merges cleanly, has been
reviewed by a person, every thread a reviewer opened is resolved, nobody reviewed its head
without leaving a thread, and its head's checks are green with a gates lane
among them. The label comes off as soon as one stops holding, so a push takes it
off while the checks run and a new review takes it off until the author answers
it. The threads and checks are read as the triage sweep reads them
(.claude/skills/codetrial-triage/pr-sweep.sh), so the label and a sweep agree
on whose move it is:

- A thread the author opened is a note to reviewers, not a finding left for the
  author, so it neither counts toward nor blocks the label. A bot's open
  thread blocks it like a person's, but only a person's thread or review
  makes the pull request reviewed: the label tells a maintainer a person's
  findings were answered, not only a bot's.
- An outdated thread that nobody resolved is still open: the push may have
  moved the line without answering it.
- Resolving is the author's answer, so it counts whoever pressed it. A changes
  request does not block either: the label is how the author says the
  requested work is done and the reviewer is owed a look.
- A person's latest review keeps the label off when it was left on the current
  head with no thread of its own, whether it approves, requests changes or
  comments: there is nothing to resolve, so only a push answers it. A reply to
  an older thread opens none, and one to a resolved thread leaves it resolved.
  A bot's review does not count, or a bot reviewing every push would hold the
  label off for good.
- Only the latest run of each check name stands, since a rerun leaves the old
  one beside it. A skipped check is neither a pass nor a failure, but a head on
  which no gates lane passed was not tested, so it is not ready. A workflow
  run held for a maintainer's approval is not ready either, and since it often
  has no check run, the held runs are read from Actions too.

Threads past the first 100 are read with a further query for that pull
request. When the answer still cannot be read in full (GitHub is still
computing mergeability, more than 50 checks, more than 100 reviews with no
person's among the newest, a review whose first 20 comments all reply to older
threads, a thread count that changed between the two thread reads) and nothing
that was read says "not", the label is left as it is, for the next run to
settle. A thread resolved or reopened between the reads is taken as its page
read it, as one changed just after the run would be, and the next run sees
it. Anything read that does say "not" (a draft, a conflict, an open
thread, a failing check) still takes it off. A read that fails is tried twice
more before the run gives up, and a label is added only if the head is still
the one that was read.

    ready-label.py [--dry-run] [REPO]

Prints one line per label it adds, removes or skips. Exit 2 when the read fails
or the label cannot be read in REPO (missing, or the API refused; gh's message
says which), having written nothing, or when a label write or a head lookup
fails, after the other writes went ahead.
"""

import argparse
import json
import os
import subprocess
import sys
import time
from urllib.parse import quote

LABEL = "ready for maintainer review"

QUERY = """
query($owner: String!, $name: String!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequests(states: OPEN, first: 50, after: $endCursor) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number isDraft mergeable headRefOid author { login }
        headRefName headRepository { nameWithOwner }
        labels(first: 100) { nodes { name } }
        commits(last: 1) { nodes { commit {
          statusCheckRollup { contexts(first: 50) { totalCount nodes {
            __typename
            ... on CheckRun { databaseId name status conclusion }
            ... on StatusContext { context state } } } } } } }
        reviews(last: 100) { totalCount nodes {
          author { __typename login } state submittedAt
          commit { oid } comments(first: 20) { totalCount nodes { replyTo { id } } } } }
        reviewThreads(first: 100) { totalCount
          # Aliased: --paginate follows the first pageInfo in the answer, and
          # that has to be the pull requests'.
          threadPages: pageInfo { hasNextPage endCursor }
          nodes { ...thread } }
      }
    }
  }
}
"""

THREAD = """
fragment thread on PullRequestReviewThread {
  isResolved comments(first: 1) { nodes { author { __typename login } } }
}
"""

# The threads of one pull request after the first 100.
LATER_THREADS = """
query($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $endCursor) {
        pageInfo { hasNextPage endCursor }
        nodes { ...thread }
      }
    }
  }
}
"""

# Seconds between tries of a failed read; the tests set it to 0.
PAUSE = float(os.environ.get("READY_LABEL_PAUSE", "5"))

# STALE is a run GitHub gave up on after it sat unfinished for days: it never
# passed, so it cannot stand for a passing gates lane.
FAILED = {"FAILURE", "TIMED_OUT", "CANCELLED", "STARTUP_FAILURE", "STALE"}


def check_state(ctx):
    """Name, kind and state (ok, skipped, pending, held, failing) of one check."""
    if ctx.get("__typename") == "CheckRun":
        conclusion = ctx.get("conclusion")
        if ctx.get("status") != "COMPLETED":
            state = "pending"
        elif conclusion == "ACTION_REQUIRED":
            state = "held"
        elif conclusion in FAILED:
            state = "failing"
        elif conclusion in ("SKIPPED", "NEUTRAL"):
            state = "skipped"
        else:
            state = "ok"
        return ctx.get("name"), "run", ctx.get("databaseId") or 0, state
    status = ctx.get("state")
    if status in ("PENDING", "EXPECTED"):
        state = "pending"
    elif status in ("FAILURE", "ERROR"):
        state = "failing"
    else:
        state = "ok"
    return ctx.get("context"), "status", 0, state


def latest_checks(contexts):
    """(name, state) of the latest check of each kind and name: a rerun
    supersedes the run before it. A commit status and a check run that share a
    name stay apart, so a passing one cannot hide a failing one."""
    latest = {}
    for ctx in contexts:
        name, kind, run_id, state = check_state(ctx)
        key = (kind, name)
        if key not in latest or run_id >= latest[key][0]:
            latest[key] = (run_id, state)
    return sorted((name, state) for (_, name), (_, state) in latest.items())


def verdict(pr):
    """("ready" | "not" | "unknown", reason) for one pull request.

    Whatever the query cut off (more than 100 threads or reviews, more than 50
    checks, the comments of a long review) or GitHub has not computed yet (the
    mergeability) is recorded rather than returned, so a definite "not" read
    from the rest still takes the label off: reviews only pile up, a bot's on
    every push, and an early "unknown" would keep the label on for good. Only
    an answer that would otherwise be "ready" turns into "unknown"."""
    rollup = (pr["commits"]["nodes"] or [{}])[0].get("commit", {})
    contexts = (rollup.get("statusCheckRollup") or {}).get("contexts") or {}
    threads = pr["reviewThreads"]
    cuts = []
    if pr["isDraft"]:
        return "not", "draft"
    if pr["mergeable"] == "UNKNOWN":
        cuts.append("mergeability unknown")
    elif pr["mergeable"] != "MERGEABLE":
        return "not", "conflicts"
    # Each cut can hide only its own kind of answer: past the threads or
    # reviews read a review may still be there, and past the checks read a
    # gates lane may be.
    # reviews(last: 100) holds the newest reviews, so an older one past it
    # cannot be anyone's latest on this head and hides only a review at all.
    # Threads read in two queries that do not add up to the count the first
    # one gave changed between them.
    threads_cut = threads["totalCount"] != len(threads["nodes"])
    reviews_cut = threads_cut or pr["reviews"]["totalCount"] > 100
    checks_cut = contexts.get("totalCount", 0) > 50
    if threads_cut:
        cuts.append("threads changed while they were read")
    if checks_cut:
        cuts.append("more than 50 checks")

    author = (pr.get("author") or {}).get("login")

    def opener(thread):
        first = thread["comments"]["nodes"]
        return (first[0].get("author") if first else None) or {}

    reviewer = [t for t in threads["nodes"] if opener(t).get("login") != author]
    by_person = [t for t in reviewer if opener(t).get("__typename") != "Bot"]

    def login(node):
        # A deleted account reads as no author at all.
        return (node.get("author") or {}).get("login") or "ghost"

    reviews = sorted(
        (
            r
            for r in pr["reviews"]["nodes"]
            if (r.get("author") or {}).get("__typename") != "Bot"
            and login(r) != author
            and r["state"] in ("COMMENTED", "CHANGES_REQUESTED", "APPROVED")
        ),
        key=lambda r: r["submittedAt"] or "",
    )
    if not by_person and not reviews:
        if not reviews_cut:
            return "not", "not reviewed"
        cuts.append("no person's review among those read")
    unresolved = sum(1 for t in reviewer if not t["isResolved"])
    if unresolved:
        return "not", f"{unresolved} open thread(s)"
    # A review left on this head with no thread of its own asked nothing the
    # author could resolve: the reviewer has looked at this code, and only a
    # push gives them something new to look at. A reply to an older thread
    # opens nothing either, and a reply to a resolved thread leaves it
    # resolved, so only comments that reply to nothing count. Each reviewer's
    # latest review counts, so one reviewer's later review does not hide
    # another's; the reviews read are the latest ones, so each reviewer seen
    # there is seen at their latest.
    latest = {login(r): r for r in reviews}
    waiting = []
    for name, r in sorted(latest.items()):
        if (r.get("commit") or {}).get("oid") != pr["headRefOid"]:
            continue
        comments = r["comments"]
        if any(not c.get("replyTo") for c in comments["nodes"]):
            continue
        if comments["totalCount"] > len(comments["nodes"]):
            cuts.append(
                f"more than {len(comments['nodes'])} comments in a review by {name}"
            )
            continue
        waiting.append(name)
    if waiting:
        return "not", f"reviewed by {', '.join(waiting)} since the last push"

    checks = latest_checks(contexts.get("nodes") or [])
    blocking = [
        f"{state}: {name}"
        for name, state in checks
        if state in ("pending", "held", "failing")
    ]
    # A run held for approval often has no check run at all.
    blocking += [
        f"held: {name}"
        for name in pr.get("heldRuns", [])
        if f"held: {name}" not in blocking
    ]
    if blocking:
        return "not", ", ".join(blocking)
    if not checks_cut and not any(
        state == "ok" and name.startswith("gates (") for name, state in checks
    ):
        return "not", "no gates lane passed"
    if cuts:
        return "unknown", "; ".join(cuts)
    return "ready", f"all {len(reviewer)} thread(s) resolved, checks green"


def changes(prs):
    """(number, "add" | "remove", reason, head read) for each pull request to
    relabel."""
    out = []
    for pr in sorted(prs, key=lambda p: p["number"]):
        has = any(label["name"] == LABEL for label in pr["labels"]["nodes"])
        answer, why = verdict(pr)
        if answer == "ready" and not has:
            out.append((pr["number"], "add", why, pr["headRefOid"]))
        elif answer == "not" and has:
            out.append((pr["number"], "remove", why, pr["headRefOid"]))
    return out


def gh(*args):
    return subprocess.run(["gh", *args], capture_output=True, text=True, check=False)


def read(*args):
    """gh's answer to a read, tried three times: a query this heavy meets a 502
    now and then, and a scheduled run that fails mails whoever last changed
    the cron. A refused token or a bad query still fails on the third try."""
    for attempt in range(3):
        if attempt:
            time.sleep(PAUSE)
        result = gh(*args)
        if result.returncode == 0:
            break
    return result


def pages(text):
    """The JSON documents --paginate prints back to back, one per page."""
    decoder, pos = json.JSONDecoder(), 0
    while True:
        while pos < len(text) and text[pos].isspace():
            pos += 1
        if pos == len(text):
            return
        page, pos = decoder.raw_decode(text, pos)
        yield page["data"]["repository"]


def held_runs(repo):
    """The workflow runs held for a maintainer's approval, or None when the API
    fails."""
    result = read(
        "api",
        "--paginate",
        f"repos/{repo}/actions/runs?status=action_required&event=pull_request"
        "&per_page=100",
        "--jq",
        ".workflow_runs[] | {name, head_sha, head_branch,"
        " head_repo: .head_repository.full_name}",
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        return None
    return [json.loads(line) for line in result.stdout.splitlines() if line.strip()]


def open_prs(repo):
    """Every open pull request with all its threads and the names of its runs
    held for approval, or None when the API fails."""
    owner, name = repo.split("/", 1)
    names = ("-F", f"owner={owner}", "-F", f"name={name}")
    result = read(
        "api", "graphql", "--paginate", *names, "-f", f"query={QUERY}{THREAD}"
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        return None
    prs = [pr for page in pages(result.stdout) for pr in page["pullRequests"]["nodes"]]
    for pr in prs:
        threads = pr["reviewThreads"]
        more = threads.pop("threadPages", None) or {}
        if not more.get("hasNextPage"):
            continue
        result = read(
            "api",
            "graphql",
            "--paginate",
            *names,
            "-F",
            f"number={pr['number']}",
            "-f",
            f"endCursor={more['endCursor']}",
            "-f",
            f"query={LATER_THREADS}{THREAD}",
        )
        if result.returncode != 0:
            sys.stderr.write(result.stderr)
            return None
        for page in pages(result.stdout):
            threads["nodes"] += page["pullRequest"]["reviewThreads"]["nodes"]
    held = held_runs(repo)
    if held is None:
        return None
    # A run on a fork carries no pull request, so it is tied to one by head
    # commit, branch and repository, as the triage sweep ties it; the commit
    # alone would let two forks' pull requests at one commit share a run.
    for pr in prs:
        head = (
            pr["headRefOid"],
            pr.get("headRefName"),
            (pr.get("headRepository") or {}).get("nameWithOwner"),
        )
        pr["heldRuns"] = sorted(
            {
                r["name"]
                for r in held
                if (r["head_sha"], r["head_branch"], r["head_repo"]) == head
            }
        )
    return prs


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("repo", nargs="?", default="sysprog21/codetrial")
    args = parser.parse_args(argv)

    encoded = quote(LABEL, safe="")
    if not args.dry_run:
        lookup = read("api", f"repos/{args.repo}/labels/{encoded}", "--silent")
        if lookup.returncode:
            sys.stderr.write(lookup.stderr)
            print(f'cannot read label "{LABEL}" in {args.repo}', file=sys.stderr)
            return 2
    prs = open_prs(args.repo)
    if prs is None:
        return 2

    status = 0
    for number, action, why, head in changes(prs):
        if action == "add" and not args.dry_run:
            # A push after the read would put the label on a head whose
            # checks have not started. A removal needs no such check: the
            # next run settles it either way.
            now = read("api", f"repos/{args.repo}/pulls/{number}", "--jq", ".head.sha")
            if now.returncode:
                sys.stderr.write(now.stderr)
                status = 2
                continue
            if now.stdout.strip() != head:
                print(f"#{number}: skip add (head moved since the read)")
                continue
        print(f"#{number}: {action} ({why})")
        if args.dry_run:
            continue
        if action == "add":
            call = (
                "api",
                f"repos/{args.repo}/issues/{number}/labels",
                "-f",
                f"labels[]={LABEL}",
                "--silent",
            )
        else:
            call = (
                "api",
                "-X",
                "DELETE",
                f"repos/{args.repo}/issues/{number}/labels/{encoded}",
                "--silent",
            )
        result = gh(*call)
        if result.returncode:
            sys.stderr.write(result.stderr)
            status = 2
    return status


if __name__ == "__main__":
    sys.exit(main())
