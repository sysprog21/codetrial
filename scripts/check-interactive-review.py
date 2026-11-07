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


@functools.lru_cache(maxsize=None)
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


def read_marker(head):
    """The marker note on `head` as a JSON object, or None.

    In CI the note comes from the fork, so it is data of any shape and size.
    """
    try:
        found = git("notes", "--ref=" + NOTES, "show", head)
    except subprocess.CalledProcessError:
        return None
    if len(found) > 64 * 1024:
        return None
    try:
        value = json.loads(found)
    except ValueError:
        return None
    return value if isinstance(value, dict) else None


def record_marker(base, head, *, started=False):
    result = status(base, head)
    if not result["required"]:
        return
    head, base = result["head"], result["base"]
    with evidence_lock(head):
        old = read_marker(head) or {}
        started = started or (
            old.get("head") == head
            and old.get("base") == base
            and old.get("workbench_started") is True
        )
        ids = re.findall(
            rb"(?m)^Change-Id: (I[0-9a-f]{40})$",
            git("show", "--no-notes", "-s", "--format=%B", head),
        )
        marker = {k: v for k, v in result.items() if k != "evidence"}
        if result["status"] == "complete":
            # What CI compares with the diff it computes itself, and the
            # answers by digest only, so the review fingerprint covers them
            # without the note carrying a word of them.
            marker["diff_hash"] = hashlib.sha256(patch(base, head)).hexdigest()
            marker["answer_digest"] = load(evidence_path(head))["answer_digest"]
            marker["review_fingerprint"] = review_fingerprint(
                head, base, marker["diff_hash"], marker["answer_digest"]
            )
        marker.update(
            schema_version=1,
            change_id=ids[0].decode() if len(ids) == 1 else None,
            workbench_started=bool(started),
        )
        if marker.get("review_fingerprint") and old.get(
            "review_fingerprint"
        ) == marker.get("review_fingerprint"):
            # The same review of the same code: its signature still holds, and
            # signing it again on every push would only move the notes ref.
            for field in ("signature", "key_fingerprint"):
                if field in old:
                    marker[field] = old[field]
        if old != marker:
            write_marker(head, marker)


def write_marker(head, marker):
    payload = (json.dumps(marker, indent=2) + "\n").encode()
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


# The review is signed on the contributor's machine with the SSH key already on
# their GitHub account: one signature over one fingerprint, checked in CI
# against the keys GitHub publishes for the pull request's author.
PROJECT = "sysprog21/codetrial"
NAMESPACE = "codetrial-review"
KEY_FINGERPRINT = re.compile(r"SHA256:[A-Za-z0-9+/]{43}")
LOGIN = re.compile(r"[A-Za-z0-9][A-Za-z0-9-]{0,38}")
# A key of its own, registered as a Signing Key, leaves an HTTPS push or the
# key a contributor pushes with untouched; the check accepts both kinds.
# A record sent without a push of the branch does not rerun the check.
PUBLISH = "sh scripts/git-pre-push.sh --publish-review <fork-remote> [<branch>]"
RERUN = "then close and reopen the pull request to rerun this check"
SETUP = (
    "Set up a signing key once: (1) ssh-keygen -t ed25519 -f"
    " ~/.ssh/codetrial_signing; (2) on GitHub, Settings > SSH and GPG keys >"
    " New SSH key, Key type: Signing Key, paste ~/.ssh/codetrial_signing.pub;"
    " (3) git config codetrial.signingKey ~/.ssh/codetrial_signing."
)


@functools.lru_cache(maxsize=4)
def code_owners(base):
    """The GitHub logins .github/CODEOWNERS names at `base`, lowercased.

    Read from the base commit, in the hook as in CI, so a branch cannot name
    its own author. Teams (@org/team) are not resolved.
    """
    try:
        text = git("show", f"{base}:.github/CODEOWNERS").decode(errors="replace")
    except subprocess.CalledProcessError:
        return frozenset()
    names = set()
    for line in text.splitlines():
        for token in line.split("#", 1)[0].split()[1:]:
            if token.startswith("@") and "/" not in token:
                names.add(token[1:].lower())
    return frozenset(names)


def is_code_owner(login, base):
    """Code owners may skip the interactive review; for them it is opt-in."""
    return bool(login and LOGIN.fullmatch(login) and login.lower() in code_owners(base))


def review_fingerprint(head, base, diff_hash, answer_digest):
    """What is signed: this project, this exact code, these answers."""
    return digest(
        {
            "project": PROJECT,
            "head": head,
            "base": base,
            "diff_hash": diff_hash,
            "answer_digest": answer_digest,
        }
    )


def config(name):
    try:
        return git("config", "--get", name).decode().strip()
    except subprocess.CalledProcessError:
        return ""


def signing_key(scratch):
    """The key to sign with: codetrial.signingKey, then user.signingKey when Git
    already signs with SSH, then the first key in ssh-agent."""
    value = config("codetrial.signingKey")
    if not value and config("gpg.format") == "ssh":
        value = config("user.signingKey")
    if value.startswith("key::"):
        literal = scratch / "literal.pub"
        literal.write_text(value[5:] + "\n")
        return literal
    if value:
        return Path(value).expanduser()
    agent = subprocess.run(["ssh-add", "-L"], capture_output=True, text=True)
    if agent.returncode == 0 and agent.stdout.strip():
        first = scratch / "agent.pub"
        first.write_text(agent.stdout.splitlines()[0] + "\n")
        return first
    raise ValueError(f"No SSH key to sign the interactive review with. {SETUP}")


def sign_marker(head):
    """Sign the completed marker on `head` on this machine."""
    with evidence_lock(head):
        marker = read_marker(head)
        if not marker or marker.get("status") != "complete":
            raise ValueError("No completed review marker to sign")
        with tempfile.TemporaryDirectory(prefix="codetrial-sign-") as scratch:
            key = signing_key(Path(scratch))
            listed = subprocess.run(
                ["ssh-keygen", "-lf", str(key)], capture_output=True, text=True
            )
            found = KEY_FINGERPRINT.search(listed.stdout)
            if not found:
                raise ValueError(
                    f"Cannot sign the interactive review with {key}. {SETUP}"
                )
            # record_marker keeps a signature only while the review it covers
            # is unchanged, so the same key need not sign it again.
            if marker.get("signature") and marker.get("key_fingerprint") == found.group(
                0
            ):
                return found.group(0)
            made = subprocess.run(
                ["ssh-keygen", "-Y", "sign", "-f", str(key), "-n", NAMESPACE],
                input=marker["review_fingerprint"].encode(),
                capture_output=True,
            )
        if made.returncode != 0:
            raise ValueError(f"Cannot sign the interactive review with {key}. {SETUP}")
        marker.update(signature=made.stdout.decode(), key_fingerprint=found.group(0))
        write_marker(head, marker)
    return found.group(0)


class Problem(str):
    """Why a review record does not pass; `kind` picks the advice for it:
    "outage" (GitHub did not answer), "no_key" or "mismatch" (the signing
    setup), or "missing" (the review itself)."""

    def __new__(cls, text, kind="missing"):
        problem = super().__new__(cls, text)
        problem.kind = kind
        return problem


def signed_by(fingerprint, signature, login, keys, unavailable=False):
    """Why `signature` over `fingerprint` is not the author's, or None.

    `unavailable` says GitHub did not answer for some of the keys, so a failure
    is reported as that and not as the contributor's missing setup.
    """
    who = login if LOGIN.fullmatch(login or "") else "the author"
    lines = [
        line.strip()
        for line in (keys or "").splitlines()
        if line.strip().startswith(("ssh-", "ecdsa-", "sk-"))
    ]
    outage = Problem(
        f"could not read the SSH keys of {who} and the commits' authors from"
        " GitHub to check the signature",
        "outage",
    )
    if not lines:
        return (
            outage
            if unavailable
            else Problem(
                f"neither {who} nor the commits' authors have an SSH key on GitHub"
                " to check the signature against",
                "no_key",
            )
        )
    with tempfile.TemporaryDirectory(prefix="codetrial-verify-") as scratch:
        allowed = Path(scratch) / "allowed_signers"
        allowed.write_text(
            "".join(f'author namespaces="{NAMESPACE}" {line}\n' for line in lines)
        )
        sig = Path(scratch) / "review.sig"
        sig.write_text(signature)
        checked = subprocess.run(
            ["ssh-keygen", "-Y", "verify", "-f", str(allowed), "-I", "author"]
            + ["-n", NAMESPACE, "-s", str(sig)],
            input=fingerprint.encode(),
            capture_output=True,
        )
    if checked.returncode == 0:
        return None
    if unavailable:
        return outage
    return Problem(
        f"does not verify against any SSH key on GitHub of {who} or the"
        " commits' authors",
        "mismatch",
    )


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


def report(base, head):
    """Why the change is large, or None when it is under the thresholds.

    The text holds only counts and commit identities, never a path or other
    text from the branch, because the workflow prints it as a workflow command.
    """
    base, head, total, large, unknown = scope(base, head)
    if not needs_review(total, large, unknown):
        return None
    reasons = []
    if total > PR_TOTAL:
        reasons.append(f"the pull request changes {total} lines (more than {PR_TOTAL})")
    if large:
        commits = ", ".join(f"{c['sha'][:12]} ({c['known_lines']})" for c in large)
        reasons.append(
            f"{len(large)} commit(s) change {LARGE_COMMIT} lines or more: {commits}"
        )
    if unknown:
        reasons.append(
            f"{len(unknown)} commit(s) change files that cannot be counted as text"
        )
    return "This is a large change: " + "; ".join(reasons)


def is_ancestor(older, newer):
    try:
        git("merge-base", "--is-ancestor", older, newer)
    except subprocess.CalledProcessError:
        return False
    return True


def updated_from(sha, base):
    """The commit `sha` merged `base`'s history into, when that is all it did.

    GitHub's "Update branch" makes such a merge, which carries no record of its
    own. It counts only when its second parent is on the pull request's base
    and its tree is the clean merge of its parents.
    """
    chain = module(ROOT / "scripts/hashchain.py")
    _, _, tree, parents = chain.commit(sha)
    if len(parents) != 2 or not is_ancestor(parents[1], base):
        return None
    try:
        chain.check_merge(tree, parents)
    except (ValueError, subprocess.CalledProcessError):
        return None
    return parents[0]


def recorded(base, head, login=None, keys=None, unavailable=False):
    """Why no completed review was pushed for `head`, or None when one was.

    The pre-push hook writes the marker once the local evidence verifies,
    signs it and sends it with the branch. CI reads it from the fork as data:
    it is a declaration, signed with a key on the author's account, of a
    completed review of exactly this code. It does not show who wrote the
    answers or that anyone understood them.
    """
    # FIXME: nothing yet checks that the author can explain the change; the
    # approver's question about one passage was withdrawn until a replacement
    # is decided.
    tip = head
    marker = read_marker(tip)
    while marker is None:
        earlier = updated_from(tip, base)
        if earlier is None:
            return Problem(f"no review record was pushed for {head[:12]}")
        tip, marker = earlier, read_marker(earlier)
    start = marker.get("base")
    if marker.get("head") != tip or marker.get("status") != "complete":
        return Problem(
            f"the record on {tip[:12]} is not a completed review of that commit"
        )
    if not (
        isinstance(start, str)
        and re.fullmatch("[0-9a-f]{40}", start)
        and is_ancestor(start, base)
        and is_ancestor(start, tip)
    ):
        return Problem(f"the record on {tip[:12]} was made against another base")
    diff_hash = hashlib.sha256(patch(start, tip)).hexdigest()
    answers = marker.get("answer_digest")
    fingerprint = (
        review_fingerprint(tip, start, diff_hash, answers)
        if isinstance(answers, str) and re.fullmatch("[0-9a-f]{64}", answers)
        else None
    )
    if (
        fingerprint is None
        or marker.get("diff_hash") != diff_hash
        or marker.get("review_fingerprint") != fingerprint
    ):
        return Problem(f"the record on {tip[:12]} was made for different code")
    signature = marker.get("signature")
    if (
        not isinstance(signature, str)
        or "-----BEGIN SSH SIGNATURE-----" not in signature
    ):
        return Problem(f"the record on {tip[:12]} is not signed")
    problem = signed_by(fingerprint, signature, login, keys, unavailable)
    if problem is None:
        return None
    if problem.kind == "mismatch":
        key = marker.get("key_fingerprint")
        key = (
            key if isinstance(key, str) and KEY_FINGERPRINT.fullmatch(key) else "a key"
        )
        return Problem(
            f"the record on {tip[:12]} is signed with {key}, which {problem}",
            problem.kind,
        )
    return problem


def owner_exempt(base, head, login, authors):
    """Why a code owner's pull request still needs the review, or None.

    The exemption is for a code owner's own work: the opener and the GitHub
    account of every commit's author must be code owners, so a contributor's
    commits do not pass by a code owner opening the pull request. `authors`
    maps each commit of the pull request, as GitHub lists them, to the login
    GitHub links its author to ("" for none); None when GitHub did not say.
    The list must name exactly the commits checked here, so one read after the
    branch moved, or cut off at GitHub's 250 commits, does not count.

    GitHub links an author by the email in the commit, which anyone can write;
    this keeps an honest code owner from excusing someone else's work by
    opening it, and is not proof of who wrote a commit.
    """
    if authors is None:
        return (
            f"{login} is a code owner, but GitHub did not list the commits'"
            " authors, so the exemption could not be confirmed"
        )
    commits = set(git("rev-list", f"{base}..{head}").decode().split())
    if set(authors) != commits:
        return (
            f"{login} is a code owner, but the {len(authors)} commits GitHub"
            f" listed are not the {len(commits)} checked here (the branch"
            " moved, or it has more than GitHub lists), so the exemption could"
            " not be confirmed"
        )
    others = [
        name or "an author linked to no GitHub account" for name in authors.values()
    ]
    others = sorted({name for name in others if not is_code_owner(name, base)})
    if others:
        return (
            f"{login} is a code owner, but commits here are by {', '.join(others)},"
            " so the interactive review applies"
        )
    return None


def pr_result(base, head, login=None, keys=None, unavailable=False, authors=None):
    """The one line the workflow prints, and whether the check passes."""
    base, head = resolve(base), resolve(head)
    reasons = report(base, head)
    if reasons is None:
        return "This change is under the interactive review thresholds.", True
    not_exempt = None
    if is_code_owner(login, base):
        not_exempt = owner_exempt(base, head, login, authors)
        if not_exempt is None:
            return (
                "::notice title=Large change::"
                + reasons
                + f". {login} and every commit's author are code owners, for"
                f" whom the interactive review is optional. See {DOC}"
            ), True
    problem = recorded(base, head, login, keys, unavailable)
    if problem is not None:
        if problem.kind == "outage":
            advice = (
                " This is GitHub not answering, not your setup: close and reopen"
                " the pull request to rerun this check."
            )
        elif problem.kind in ("no_key", "mismatch"):
            advice = (
                " The review is signed on your machine with an SSH key on your"
                f" GitHub account. {SETUP} Then sign and send the record again"
                f" with {PUBLISH}, and {RERUN}."
            )
        else:
            advice = (
                " A large change needs the interactive review before it is"
                " pushed; the pre-push hook checks for it, signs its record and"
                " sends it with the branch, so a missing record usually means the"
                " hooks were not installed or were skipped. Run make hooks and"
                " finish /codetrial-review on this branch. If that changed the"
                " branch, push it: the record goes with it. If not, send the"
                f" record with {PUBLISH}, and {RERUN}."
            )
        return (
            "::error title=Interactive review missing::"
            + reasons
            + (f"; {not_exempt}" if not_exempt else "")
            + f", and {problem}."
            + advice
            + f" See {DOC}"
        ), False
    return (
        "::notice title=Large change::"
        + reasons
        + ". Its interactive review was recorded at push."
        f" See {DOC}"
    ), True


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


def read_capped(path, limit=256 * 1024):
    """A file the workflow wrote from GitHub's answers, read up to `limit`."""
    with open(path, "rb") as source:
        return source.read(limit).decode(errors="replace")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command",
        choices=["staged", "push", "certify", "status", "mark", "pr", "marker"],
    )
    parser.add_argument("--base")
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--bundle")
    parser.add_argument("--author-login", help="pr: the pull request opener")
    parser.add_argument("--author-type", help="pr: the opener's account type")
    parser.add_argument(
        "--author-keys",
        help="pr: a file of the public SSH keys of the opener and commit authors",
    )
    parser.add_argument(
        "--commit-authors",
        help="pr: a file of '<sha> <login>' lines, each commit of the pull"
        " request and the GitHub login its author is linked to; absent when"
        " GitHub did not list them",
    )
    parser.add_argument(
        "--keys-unavailable",
        action="store_true",
        help="pr: GitHub did not answer for some of those keys",
    )
    args = parser.parse_args()
    try:
        if args.command == "marker":
            # For the pre-push hook, which sends only a completed record.
            print((read_marker(resolve(args.head)) or {}).get("status", ""))
        elif args.command == "staged":
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
                keys = read_capped(args.author_keys) if args.author_keys else None
                authors = None
                if args.commit_authors:
                    entries = (
                        line.strip().partition(" ")
                        for line in read_capped(args.commit_authors).splitlines()
                    )
                    authors = {sha: name.strip() for sha, _, name in entries if sha}
                line, passed = pr_result(
                    args.base,
                    args.head,
                    args.author_login,
                    keys,
                    args.keys_unavailable,
                    authors,
                )
                print(line)
                return 0 if passed else 1
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
                    owner = config("codetrial.codeOwner")
                    if is_code_owner(owner, base):
                        print(
                            f"Interactive review skipped: {owner} is a code owner,"
                            " for whom it is optional; /codetrial-review still"
                            " runs it and this push then sends its record. The"
                            " pull request's check does not read this setting: it"
                            " passes without a record only when the account that"
                            " opens the pull request and the GitHub account of"
                            " every commit's author are code owners."
                        )
                        return 0
                    triggers = ", ".join(
                        f"{c['sha']} ({c['known_lines']} lines)" for c in large
                    ) or ("unmeasured changes" if unknown else "PR aggregate only")
                    raise ValueError(
                        f"Interactive review required ({len(large)} commits >=200 lines; PR diff {total} lines). Unmeasured commits: {len(unknown)}. Trigger: {triggers}. Run /codetrial-review on the full committed branch; finish its web explanation and durable report before retrying. {error} See {DOC}"
                    ) from error
                key = sign_marker(head)
                print(
                    f"Interactive review of {head[:12]} signed with {key}; the"
                    " pull request check accepts it only if this key is on your"
                    " GitHub account, as an authentication or a signing key."
                )
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        sqlite3.Error,
        subprocess.CalledProcessError,
    ) as error:
        # Do not print Git stderr, answer text or database records from a hook.
        if args.command == "pr":
            # Printed as a workflow command, so nothing from the branch goes in.
            parser.exit(
                1,
                "::error title=Cannot check the interactive review::The change's"
                " history could not be read; rerun the job, and if it fails again"
                f" a maintainer looks at the job log. See {DOC}\n",
            )
        detail = (
            "cannot inspect local history; fetch the PR base first"
            if isinstance(error, subprocess.CalledProcessError)
            else error
        )
        parser.exit(1, f"Interactive review check failed: {detail}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
