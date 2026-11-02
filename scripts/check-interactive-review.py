#!/usr/bin/env python3
"""Check local interactive-review evidence without starting a model or browser."""

import argparse
import contextlib
import fcntl
import functools
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import tempfile


LARGE_COMMIT = 200  # added plus deleted lines in one commit
PR_TOTAL = 500  # added plus deleted lines in the whole pull request's net diff
ROOT = Path(__file__).resolve().parents[1]
SKILL = ROOT / ".claude/skills/codetrial-review"
NOTES = "refs/notes/codetrial-review"


@functools.lru_cache(maxsize=None)
def _load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def module(path):
    # One instance per script, however many callers name it differently, so a
    # push check does not re-execute commit-sizes.py for every diff it takes.
    return _load(Path(path).stem.replace("-", "_"), str(path))


def git(*args, data=None):
    return subprocess.check_output(
        ["git", "--no-lazy-fetch", *args],
        input=data,
        stderr=subprocess.PIPE,
        env={**os.environ, "GIT_TERMINAL_PROMPT": "0", "GIT_NO_REPLACE_OBJECTS": "1"},
    )


def resolve(ref):
    return (
        git("rev-parse", "--verify", "--end-of-options", ref + "^{commit}")
        .decode()
        .strip()
    )


def common():
    return Path(git("rev-parse", "--git-common-dir").decode().strip()).resolve()


def load(path):
    if path.stat().st_size > 8 * 1024 * 1024:
        raise ValueError("Review evidence exceeds the supported size")
    value = json.loads(path.read_text())
    if not isinstance(value, dict):
        raise ValueError("Review evidence is not a JSON object")
    return value


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


VERIFY_ERRORS = (OSError, ValueError, KeyError, TypeError, sqlite3.Error)


def needs_review(total, large, unknown):
    return bool(large or total > PR_TOTAL or unknown)


@functools.lru_cache(maxsize=4)
def patch(base, head):
    return module(SKILL / "commit-sizes.py").diff(base, head)


def lines(raw):
    """Text lines in a numstat, and the paths it could not count (binary)."""
    total, unmeasured = 0, []
    for row in raw.split(b"\0"):
        if not row:
            continue
        added, deleted, path = row.split(b"\t", 2)
        if b"-" in (added, deleted):
            unmeasured.append(path.decode(errors="replace"))
        else:
            total += int(added) + int(deleted)
    return total, unmeasured


def measure(base, head):
    sizer = module(SKILL / "commit-sizes.py")
    sizes = sizer.collect(base, head)
    unknown = [c for c in sizes["commits"] if not c["complete"]]
    # The whole branch is read under the base's attributes, and again under the
    # head's when a .gitattributes differs between them. Either side can hide
    # lines by calling a file binary: the head by adding the rule, the base by
    # carrying one that a commit then removes, so every commit looks small while
    # the sum is not. Count the larger, and treat a path either reading cannot
    # count as unmeasured, as for a commit.
    readings = [sizes["base"]]
    if git(
        "diff", "--name-only", sizes["base"], sizes["head"], "--", "*.gitattributes"
    ).strip():
        readings.append(sizes["head"])
    total, unmeasured = 0, {}
    for attributes in readings:
        counted, paths = lines(
            sizer.diff(
                sizes["base"], sizes["head"], numstat=True, attributes=attributes
            )
        )
        total = max(total, counted)
        unmeasured.update(dict.fromkeys(paths))
    if unmeasured and not unknown:
        unknown = [
            {
                "sha": "whole branch",
                "files": [
                    {"path": path, "additions": None, "deletions": None}
                    for path in unmeasured
                ],
                "known_lines": 0,
                "complete": False,
            }
        ]
    large = [c for c in sizes["commits"] if c["known_lines"] >= LARGE_COMMIT]
    return sizes["base"], sizes["head"], total, large, unknown


# A push measures once and then asks again by resolved SHAs for the marker and
# the verification; every measurement costs a temporary repository per commit.
_scopes = {}


def scope(base, head):
    if (base, head) not in _scopes:
        result = measure(base, head)
        _scopes[(base, head)] = _scopes[(result[0], result[1])] = result
    return _scopes[(base, head)]


def durable(run_id, head, diff_hash, manifest_hash):
    if not isinstance(run_id, str) or not re.fullmatch("[0-9a-f]{32}", run_id):
        raise ValueError("Invalid review run identity")
    pending = common() / "codetrial-review/pending" / run_id
    config, receipt = load(pending / "pending.json"), load(pending / "receipt.json")
    if (
        any(
            config.get(k) != v
            for k, v in {
                "run_id": run_id,
                "head": head,
                "diff_hash": diff_hash,
                "manifest_hash": manifest_hash,
            }.items()
        )
        or any(
            receipt.get(k) != v
            for k, v in {
                "run_id": run_id,
                "head": head,
                "outcome": "complete",
            }.items()
        )
        or receipt.get("record_outcome") not in ("recorded", "already-recorded")
    ):
        raise ValueError("Review lifecycle receipt does not match this code")
    with sqlite3.connect(
        Path(config["db"]).resolve().as_uri() + "?mode=ro", uri=True
    ) as db:
        row = db.execute(
            "SELECT head,diff_hash,committed,outcome,skill,skills FROM local_review_runs WHERE run_id=?",
            (run_id,),
        ).fetchone()
    if (
        not row
        or row[:4] != (head, diff_hash, 1, "complete")
        or (
            row[4] != "codetrial-review"
            and "codetrial-review" not in json.loads(row[5])
        )
    ):
        raise ValueError("No matching durable code-review record")


def evidence_path(head):
    if not re.fullmatch("[0-9a-f]{40}", head):
        raise ValueError("Full commit identity required")
    return common() / "codetrial-review/interactive" / (head + ".json")


@contextlib.contextmanager
def evidence_lock(head):
    # Replacing a certificate and revoking one for an older session must not
    # interleave: sessions of one repository share this lock, not a bundle.
    path = evidence_path(head)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with open(path.parent / ".lock", "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield


def fsync_dir(directory):
    fd = os.open(directory, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic(path, value):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    text = json.dumps(value, indent=2) + "\n"
    if len(text.encode()) > 8 * 1024 * 1024:
        raise ValueError(
            "Review evidence exceeds the supported size; split the change so each part can be certified"
        )
    fd, temporary = tempfile.mkstemp(prefix=".interactive-", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as output:
            output.write(text)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        fsync_dir(path.parent)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def certify(bundle):
    bundle = Path(bundle).resolve()
    manifest_bytes = (bundle / "manifest.json").read_bytes()
    manifest = json.loads(manifest_bytes)
    if (
        manifest.get("pr") is not None
        or manifest.get("committed") is not True
        or manifest.get("untracked")
        or manifest.get("files_complete") is False
    ):
        raise ValueError(
            "Interactive evidence requires a complete committed local PR bundle"
        )
    against = manifest.get("against", "")
    if not against.endswith("...HEAD"):
        raise ValueError("Review the full branch against its PR base")
    head = resolve(manifest["head"])
    base = git("merge-base", resolve(against[:-7]), head).decode().strip()
    diff_bytes = (bundle / "pr.diff").read_bytes()
    if patch(base, head) != diff_bytes:
        raise ValueError("Bundled diff is not the committed PR diff")
    app = module(SKILL / "workbench.py").Workbench(bundle, bundle / "questions.json")
    snapshot = app.snapshot()
    if not 1 <= len(snapshot["questions"]) <= 3:
        raise ValueError(
            "Select one key-change question before certifying interactive review"
        )
    if snapshot["status"] != "explanation_complete":
        raise ValueError("Contributor explanation still needs verified feedback")
    run_id = load(bundle / "experience-start.json")["run_id"]
    diff_hash = hashlib.sha256(diff_bytes).hexdigest()
    manifest_hash = hashlib.sha256(manifest_bytes).hexdigest()
    durable(run_id, head, diff_hash, manifest_hash)
    proof = {
        k: snapshot[k]
        for k in (
            "identity",
            "answer_digest",
            "answers",
            "feedback",
            "questions",
            "reflection",
            "submission",
        )
    }
    proof.update(
        head=head,
        base=base,
        tree=git("rev-parse", head + "^{tree}").decode().strip(),
        run_id=run_id,
        diff_hash=diff_hash,
        manifest_hash=manifest_hash,
        manifest=manifest,
        diff_text=diff_bytes.decode(),
    )
    # Serialize certification with edits that revoke an earlier approval.
    with app.locked():
        latest = app.state()
        if (
            latest["revision"] != snapshot["revision"]
            or latest["feedback"] != snapshot["feedback"]
            or (bundle / "manifest.json").read_bytes() != manifest_bytes
            or (bundle / "pr.diff").read_bytes() != diff_bytes
        ):
            raise ValueError("Interactive review changed while it was being certified")
        with evidence_lock(head):
            atomic(evidence_path(head), proof)
        record_marker(base, head)


def reflected(value, identity, answer_digest, feedback):
    return bool(
        isinstance(value, dict)
        and isinstance(feedback, dict)
        and value.get("identity") == identity
        and value.get("answer_digest") == answer_digest
        and value.get("feedback_digest") == digest(feedback)
        and value.get("grade") in ("got", "partly", "missed")
        and isinstance(value.get("correction"), str)
        and len(value["correction"]) <= 4000
        and (value["grade"] == "got" or value["correction"].strip())
    )


def verify(head, base):
    proof = load(evidence_path(head))
    if (
        proof.get("head") != head
        or proof.get("base") != base
        or proof.get("tree") != git("rev-parse", head + "^{tree}").decode().strip()
    ):
        raise ValueError("Interactive review is for a different head or PR base")
    current = hashlib.sha256(patch(base, head)).hexdigest()
    if (
        current != proof["diff_hash"]
        or hashlib.sha256(proof["diff_text"].encode()).hexdigest() != current
    ):
        raise ValueError("Interactive review diff no longer matches")
    if (
        digest([proof["manifest"], proof["diff_text"], proof["questions"]])
        != proof["identity"]
        or digest(proof["answers"]) != proof["answer_digest"]
        or not 1 <= len(proof["questions"]) <= 3
    ):
        raise ValueError("Interactive explanation evidence changed")
    submission = proof.get("submission") or {}
    if (
        submission.get("identity") != proof["identity"]
        or submission.get("answer_digest") != proof["answer_digest"]
        or submission.get("answers") != proof["answers"]
        or submission.get("focus") != proof["questions"]
        or submission.get("head") != head
    ):
        raise ValueError("Contributor submission does not match the pinned explanation")
    feedback = proof["feedback"]
    if (
        not reflected(
            proof.get("reflection"), proof["identity"], proof["answer_digest"], feedback
        )
        or feedback.get("identity") != proof["identity"]
        or feedback.get("answer_digest") != proof["answer_digest"]
    ):
        raise ValueError(
            "Interactive reference and contributor reflection do not match these answers"
        )
    durable(proof["run_id"], head, current, proof["manifest_hash"])


def revoke(head, identity):
    path = evidence_path(head)
    with evidence_lock(head):
        if not path.exists() or load(path).get("identity") != identity:
            return
        base = load(path)["base"]
        path.unlink()
        fsync_dir(path.parent)
    record_marker(base, head)


def status(base, head):
    base, head, total, large, unknown = scope(base, head)
    required = needs_review(total, large, unknown)
    result = {
        "base": base,
        "head": head,
        "pr_text_lines": total,
        "large_commits": [
            {"sha": c["sha"], "text_lines": c["known_lines"]} for c in large
        ],
        "coverage_complete": not unknown,
        "unmeasured_commits": [
            {
                "sha": c["sha"],
                "paths": [
                    f["path"]
                    for f in c["files"]
                    if f["additions"] is None or f["deletions"] is None
                ],
            }
            for c in unknown
        ],
        "required": required,
        "status": "not_required",
        "evidence": None,
    }
    if required:
        result["status"] = "pending"
        try:
            verify(head, base)
        except VERIFY_ERRORS:
            pass
        else:
            result["status"] = "complete"
            result["evidence"] = str(evidence_path(head))
    return result


def record_marker(base, head, *, started=False):
    result = status(base, head)
    if not result["required"]:
        return
    head, base = result["head"], result["base"]
    with evidence_lock(head):
        old = subprocess.run(
            ["git", "--no-lazy-fetch", "notes", "--ref=" + NOTES, "show", head],
            capture_output=True,
            env={
                **os.environ,
                "GIT_NO_REPLACE_OBJECTS": "1",
                "GIT_TERMINAL_PROMPT": "0",
            },
        )
        try:
            old = json.loads(old.stdout) if old.returncode == 0 else {}
        except ValueError:
            old = {}
        started = started or (
            isinstance(old, dict)
            and old.get("head") == head
            and old.get("base") == base
            and old.get("workbench_started") is True
        )
        ids = re.findall(
            rb"(?m)^Change-Id: (I[0-9a-f]{40})$",
            git("show", "--no-notes", "-s", "--format=%B", head),
        )
        marker = {k: v for k, v in result.items() if k != "evidence"}
        marker.update(
            schema_version=1,
            change_id=ids[0].decode() if len(ids) == 1 else None,
            workbench_started=bool(started),
        )
        payload = (json.dumps(marker, indent=2) + "\n").encode()
        if old == marker:
            return
        try:
            git(
                "-c",
                "commit.gpgsign=false",
                "notes",
                "--ref=" + NOTES,
                "add",
                "-f",
                "-F",
                "-",
                head,
                data=payload,
            )
        except subprocess.CalledProcessError as error:
            raise ValueError(
                "Cannot record interactive-review Git note; check Git identity and note-ref locks"
            ) from error


def record_workbench(bundle):
    bundle = Path(bundle)
    manifest = load(bundle / "manifest.json")
    against = manifest.get("against", "")
    if (
        manifest.get("pr") is not None
        or manifest.get("committed") is not True
        or manifest.get("untracked")
        or manifest.get("files_complete") is False
        or not against.endswith("...HEAD")
    ):
        return
    # A preview or synthetic fixture must not annotate an unrelated real commit.
    if manifest["head"] != resolve("HEAD"):
        return
    base = git("merge-base", resolve(against[:-7]), manifest["head"]).decode().strip()
    if patch(base, manifest["head"]) != (bundle / "pr.diff").read_bytes():
        raise ValueError("Workbench diff does not match the committed PR range")
    record_marker(base, manifest["head"], started=True)


DOC = "docs/large-change-explanation.md#troubleshooting"
# A maintainer adds this label to a pull request they have seen explained. Only
# people with triage access can, so an author cannot add or strip it.
LABEL = "explained"


def check_labels(base, head, labels):
    """Refuse a change that needs the interactive review and has no label.

    CI measures the change itself, from the commits and the diff, so nothing in
    the description can be edited out. It cannot see the contributor's local
    evidence, so the proof that the change was explained is a maintainer's label.
    """
    base, head, total, large, unknown = scope(base, head)
    if not needs_review(total, large, unknown):
        return False
    if LABEL not in labels:
        raise ValueError(
            "[interactive-review/required] This change needs the interactive "
            f"review ({len(large)} commits at {LARGE_COMMIT} lines or more, "
            f"PR diff {total} lines, {len(unknown)} unmeasured). A maintainer "
            f"adds the '{LABEL}' label once it has been explained. See {DOC}"
        )
    return True


def dependabot_exempt(base, head, login, kind):
    """Whether this pull request is Dependabot's and nothing else is in it.

    The opener comes from GitHub (a commit's author line can say anything), and
    every commit must also be Dependabot's, so a person's commit pushed onto its
    branch gets no exemption.
    """
    chain = module(ROOT / "scripts/hashchain.py")
    if not chain.exempt_pr(login, kind):
        return False
    commits = git("rev-list", f"{base}..{head}").decode().split()
    return bool(commits) and len(chain.bots(commits)) == len(commits)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command",
        choices=["staged", "push", "certify", "status", "mark", "pr"],
    )
    parser.add_argument("--base")
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--bundle")
    parser.add_argument(
        "--labels", default="[]", help="pr: the pull request's labels, as a JSON list"
    )
    parser.add_argument("--author-login", help="pr: the pull request opener")
    parser.add_argument("--author-type", help="pr: the opener's account type")
    args = parser.parse_args()
    try:
        if args.command == "staged":
            count, _ = lines(
                git(
                    "diff",
                    "--cached",
                    "--numstat",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-renames",
                    "-z",
                )
            )
            if count >= LARGE_COMMIT:
                print(
                    f"Note: staged changes have {count} added + deleted text lines; commit first, then run /codetrial-review before pushing."
                )
        elif args.command == "certify":
            if not args.bundle:
                parser.error("certify requires --bundle")
            certify(args.bundle)
            print("Interactive review evidence retained for this committed head.")
        elif args.command in ("status", "mark", "pr"):
            if not args.base:
                parser.error(args.command + " requires --base")
            if args.command == "pr":
                if args.author_login and dependabot_exempt(
                    args.base, args.head, args.author_login, args.author_type
                ):
                    print("Dependabot's own pull request; no explanation to ask for.")
                    return 0
                labels = json.loads(args.labels)
                if not isinstance(labels, list):
                    raise ValueError("--labels must be a JSON list of names")
                if check_labels(args.base, args.head, labels):
                    print(f"A maintainer's '{LABEL}' label covers this change.")
                else:
                    print("This change is under the interactive review thresholds.")
                return 0
            if args.command == "mark":
                record_marker(args.base, args.head)
            else:
                print(json.dumps(status(args.base, args.head), indent=2))
        else:
            if not args.base:
                parser.error("push requires --base")
            base, head, total, large, unknown = scope(args.base, args.head)
            if needs_review(total, large, unknown):
                record_marker(base, head)
                try:
                    verify(head, base)
                except VERIFY_ERRORS as error:
                    triggers = ", ".join(
                        f"{c['sha']} ({c['known_lines']} lines)" for c in large
                    ) or ("unmeasured changes" if unknown else "PR aggregate only")
                    raise ValueError(
                        f"Interactive review required ({len(large)} commits >=200 lines; PR diff {total} lines). Unmeasured commits: {len(unknown)}. Trigger: {triggers}. Run /codetrial-review on the full committed branch; finish its web explanation and durable report before retrying. {error} See {DOC}"
                    ) from error
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        sqlite3.Error,
        subprocess.CalledProcessError,
    ) as error:
        # Do not print Git stderr, answer text or database records from a hook.
        parser.exit(
            1,
            f"Interactive review check failed: {error if not isinstance(error, subprocess.CalledProcessError) else 'cannot inspect local history; fetch the PR base first'}\n",
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
