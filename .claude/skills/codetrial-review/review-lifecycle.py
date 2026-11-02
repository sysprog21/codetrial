#!/usr/bin/env python3
"""Explicit, private begin/finish/retry lifecycle for local skill reports."""

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
import subprocess
import sys
import tempfile
import time
import uuid

MEMORY = Path(__file__).with_name("memory.py")
SHA = re.compile("[0-9a-f]{40}")
RUN_ID = re.compile("[0-9a-f]{32}")
MAX_JSON = 1024 * 1024
LOCK_WAIT = 5.0


@functools.lru_cache(maxsize=1)
def memory_api():
    spec = importlib.util.spec_from_file_location("lifecycle_memory", MEMORY)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load(path, maximum=MAX_JSON, with_hash=False):
    return (
        memory_api()
        .experience_module()
        .read_json(path, maximum=maximum, with_hash=with_hash)
    )


def check_paths(report, api):
    if isinstance(report, dict):
        api.experience_module().check_report_paths(report, api)


def private_dir(path):
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    path.chmod(0o700)
    return path


def atomic(path, value, compact=False):
    path = Path(path)
    # Compact, unescaped UTF-8 keeps a stored report within the reader's size limit.
    text = json.dumps(
        value,
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
        **({"separators": (",", ":")} if compact else {"indent": 2}),
    )
    text += "\n"
    if len(text.encode("utf-8")) > MAX_JSON:
        raise ValueError("stored JSON exceeds size limit")
    descriptor, temporary = tempfile.mkstemp(prefix=".lifecycle-", dir=str(path.parent))
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            output.write(text)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, str(path))
        directory = os.open(str(path.parent), os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


@contextlib.contextmanager
def locked(path):
    descriptor = os.open(str(path), os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        os.fchmod(descriptor, 0o600)
        # Parallel reviewers close out together; wait briefly before refusing.
        deadline, pause = time.monotonic() + LOCK_WAIT, 0.005
        while True:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(pause)
                pause = min(pause * 2, 0.1)
        yield
    finally:
        os.close(descriptor)


def moved(saved, current):
    return any(saved.get(key) != value for key, value in current.items())


def identity(bundle):
    manifest, manifest_hash = load(
        bundle / "manifest.json", maximum=8 * 1024 * 1024, with_hash=True
    )
    files = manifest.get("files", [])
    if (
        not isinstance(files, list)
        or len(files) > memory_api().experience_module().MAX_PATHS
    ):
        raise ValueError("bundle file list is not reviewable")
    check_paths({"paths": [item["path"] for item in files]}, memory_api())
    head = manifest.get("head")
    if not isinstance(head, str) or not SHA.fullmatch(head):
        raise ValueError("bundle needs full manifest head")
    return {
        "head": head,
        "diff_hash": memory_api().experience_module().file_sha256(bundle / "pr.diff"),
        "manifest_hash": manifest_hash,
    }, manifest


def begin(bundle):
    bundle = Path(bundle).resolve(strict=True)
    # Do not chmod a user's existing bundle directory; only our files are private.
    with locked(bundle / ".experience-lifecycle.lock"):
        current, _ = identity(bundle)
        path = bundle / "experience-start.json"
        start = load(path) if path.exists() else {}
        if moved(start, current) or not RUN_ID.fullmatch(str(start.get("run_id", ""))):
            start = dict(current, run_id=uuid.uuid4().hex)
            atomic(path, start)
        elif not path.is_symlink():
            path.chmod(0o600)
        return start


def common_dir():
    result = subprocess.run(
        ["git", "rev-parse", "--git-common-dir"],
        capture_output=True,
        text=True,
        check=True,
    )
    return Path(result.stdout.strip()).resolve()


def marker(root, head):
    with locked(root / ".reviewed.lock"):
        path = root / "reviewed"
        lines = path.read_text().splitlines() if path.exists() else []
        if head not in lines:
            descriptor = os.open(
                str(path), os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600
            )
            with os.fdopen(descriptor, "a", encoding="utf-8") as output:
                output.write(head + "\n")
                output.flush()
                os.fsync(output.fileno())


def failed_attempt(pending):
    attempt = pending / "attempt.json"
    return (
        attempt.is_file()
        and not (pending / "receipt.json").exists()
        and load(attempt).get("returncode") != 0
    )


def finish(args, api):
    bundle = Path(args.bundle).resolve(strict=True)
    with locked(bundle / ".experience-lifecycle.lock"):
        start_path = bundle / "experience-start.json"
        if not start_path.is_file():
            raise ValueError(
                "missing experience start; run begin before reviewing the bundle"
            )
        start = load(start_path)
        current, _ = identity(bundle)
        if moved(start, current):
            raise ValueError(
                "bundle changed since begin; original start retained; prepare and review a new run"
            )
        if not RUN_ID.fullmatch(str(start.get("run_id", ""))):
            raise ValueError("invalid experience start run ID")
    report = load(args.report)
    check_paths(report, api)
    report = api.experience_module().clean(report, api)
    if not isinstance(report, dict) or report.get("head") != start["head"]:
        raise ValueError("report head differs from started bundle")
    # Reject an invalid report before it becomes a pending run that blocks correction.
    api.experience_module().normalize(
        report,
        argparse.Namespace(
            bundle=str(bundle),
            skill=args.skill,
            expected_manifest_hash=start["manifest_hash"],
            expected_diff_hash=start["diff_hash"],
            run_id=start["run_id"],
        ),
        api,
    )
    root = private_dir(common_dir() / "codetrial-review")
    pending = private_dir(private_dir(root / "pending") / start["run_id"])
    db = str(
        Path(
            args.db or os.environ.get("CODETRIAL_MEMORY") or api.default_db()
        ).resolve()
    )
    config = dict(
        start,
        bundle=str(bundle),
        db=db,
        repo=api.repository_name(args.repo),
        skill=args.skill,
        common_dir=str(root.parent),
    )
    with locked(pending / ".lock"):
        stale = (pending / "pending.json").exists() and (
            load(pending / "pending.json") != config
            or load(pending / "report.json") != report
        )
        if stale and not failed_attempt(pending):
            raise ValueError(
                "run already has another report/config; preserve pending run"
            )
        if stale:
            # An interrupted recorder may have committed, so keep what it ran on.
            kept = private_dir(private_dir(pending / "superseded") / uuid.uuid4().hex)
            for name in ("report.json", "pending.json"):
                atomic(kept / name, load(pending / name), compact=name == "report.json")
        if stale or not (pending / "pending.json").exists():
            atomic(pending / "report.json", report, compact=True)
            atomic(pending / "pending.json", config)
    return retry(pending, api)


def retry(pending, api):
    pending = Path(pending).resolve(strict=True)
    with locked(pending / ".lock"):
        config = load(pending / "pending.json")
        if not RUN_ID.fullmatch(str(config.get("run_id", ""))):
            raise ValueError("invalid pending run ID")
        bundle = Path(config["bundle"])
        current, manifest = identity(bundle)
        if moved(config, current):
            raise ValueError("original bundle changed; pending run retained")
        report = load(pending / "report.json")
        check_paths(report, api)
        if report.get("head") != current["head"]:
            raise ValueError("pending report head differs from bundle")
        command = [
            sys.executable,
            str(MEMORY),
            "--db",
            config["db"],
            "--repo",
            api.repository_name(config["repo"]),
            "experience",
            "record",
            "--bundle",
            str(bundle),
            "--report",
            str(pending / "report.json"),
            "--run-id",
            config["run_id"],
            "--skill",
            config["skill"],
            "--expected-manifest-hash",
            config["manifest_hash"],
            "--expected-diff-hash",
            config["diff_hash"],
        ]
        result = subprocess.run(command, capture_output=True, text=True)
        atomic(
            pending / "attempt.json",
            {
                "returncode": result.returncode,
                "stdout": api.safe_text(result.stdout),
                "stderr": api.safe_text(result.stderr),
            },
        )
        if result.returncode:
            print(
                json.dumps(
                    {
                        "outcome": "pending",
                        "pending": str(pending),
                        "reason": "record failed; inspect attempt.json and retry explicitly",
                    }
                )
            )
            return result.returncode if result.returncode > 0 else 1
        response = json.loads(result.stdout)
        if (
            response.get("outcome") not in ("recorded", "already-recorded")
            or response.get("run_id") != config["run_id"]
        ):
            raise ValueError(
                "record returned no matching durable receipt; pending retained"
            )
        # Recheck after subprocess: a changed bundle must never certify a push.
        after, latest = identity(bundle)
        if after != current or latest != manifest:
            raise ValueError(
                "bundle moved during recording; no reviewed marker written"
            )
        receipt = {
            "run_id": config["run_id"],
            "head": current["head"],
            "outcome": report["outcome"],
            "record_outcome": response["outcome"],
        }
        atomic(pending / "receipt.json", receipt)
        if (
            manifest.get("pr") is None
            and manifest.get("committed") is True
            and report.get("outcome") == "complete"
            and (
                config["skill"] == "codetrial-review"
                or "codetrial-review" in report.get("skills_used", [])
            )
        ):
            root = private_dir(Path(config["common_dir"]) / "codetrial-review")
            marker(root, current["head"])
            explanation = bundle / "explanation.json"
            if explanation.exists():
                try:
                    state = json.loads(explanation.read_text())
                except (OSError, ValueError):
                    state = {}
                if explained(state):
                    result = subprocess.run(
                        [
                            sys.executable,
                            str(
                                Path(__file__).resolve().parents[3]
                                / "scripts/check-interactive-review.py"
                            ),
                            "certify",
                            "--bundle",
                            str(bundle),
                        ],
                        capture_output=True,
                        text=True,
                    )
                    if result.returncode:
                        # The review is recorded either way; the caller still
                        # needs the receipt, and the code tells it certification
                        # did not happen.
                        print(json.dumps(dict(receipt, pending=str(pending))))
                        print(
                            "Code review recorded; interactive evidence was not certified: "
                            + result.stderr,
                            file=sys.stderr,
                        )
                        return result.returncode
                else:
                    print(
                        "Code review recorded; interactive evidence was not certified: "
                        "the explanation has no feedback and reflection for its current answers.",
                        file=sys.stderr,
                    )
        print(json.dumps(dict(receipt, pending=str(pending))))
        return 0


def explained(state):
    """Whether the explanation holds feedback and a reflection for its current answers.

    Whatever is in the file, a malformed one is "not explained" and not a crash:
    this runs after the review's receipt and marker are written.
    """
    if not isinstance(state, dict):
        return False
    feedback = state.get("feedback")
    reflection = state.get("reflection")
    if not isinstance(feedback, dict) or not isinstance(reflection, dict):
        return False
    try:
        answers = json.dumps(state.get("answers", {}), sort_keys=True)
    except (TypeError, ValueError):
        return False
    digest = hashlib.sha256(answers.encode()).hexdigest()
    return (
        reflection.get("answer_digest") == digest
        and feedback.get("answer_digest") == digest
    )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--retry", metavar="PENDING_DIRECTORY")
    sub = parser.add_subparsers(dest="action")
    start = sub.add_parser("begin")
    start.add_argument("--bundle", required=True)
    end = sub.add_parser("finish")
    end.add_argument("--bundle", required=True)
    end.add_argument("--report", required=True)
    end.add_argument("--db")
    end.add_argument("--repo", default="sysprog21/codetrial")
    end.add_argument("--skill", default="codetrial-review")
    args = parser.parse_args(argv)
    if bool(args.retry) == bool(args.action):
        parser.error("choose begin, finish, or --retry PENDING_DIRECTORY")
    api = memory_api()
    try:
        if args.retry:
            return retry(args.retry, api)
        if args.action == "begin":
            print(json.dumps(begin(args.bundle)))
            return 0
        return finish(args, api)
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        AttributeError,
        subprocess.SubprocessError,
    ) as error:
        print("review lifecycle: " + api.safe_label(str(error)), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
