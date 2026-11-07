#!/usr/bin/env python3
"""Prepare bounded review-memory batches and promote attested artifacts.

The collector runs with read permissions. Signing jobs verify opaque packages;
only promotion and maintenance need repository write access. Checkpoints are
expiring Actions artifacts, never entries in the PR build cache.
"""

import argparse
import contextlib
import gzip
import hashlib
import http.client
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shutil
import signal
import selectors
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import zipfile
import zlib
from datetime import timedelta
from types import SimpleNamespace

SPEC = importlib.util.spec_from_file_location(
    "publisher_memory", Path(__file__).with_name("memory.py")
)
memory = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(memory)
_say = memory.METER.say
memory.METER.say = lambda text, err=False: _say(text, err=True)

CHECKPOINT_FORMAT = 2
CHECKPOINT_PREFIX = "review-memory-checkpoint-"
READY_PREFIX = "review-memory-ready-"
SIGN_JOB = "review-memory-sign-checkpoint"
WORKFLOW = ".github/workflows/review-memory.yml"
RELEASE_ASSETS = ("memory.db.gz", "manifest.json", "SHA256SUMS")
CHECKPOINT_FILES = {"checkpoint.json", "checkpoint.db.gz"}
CHECKPOINT_MARKS = memory.SNAPSHOT_MARKS | {
    "walk",
    "backfill_detail",
    "authority_source",
    "authority_attempt",
    "detail_complete",
    "detail_attempt",
    "sweep_attempt",
}


def digest(path):
    value = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            memory.remaining()
            value.update(block)
    return value.hexdigest()


@contextlib.contextmanager
def budget(seconds):
    old_deadline = memory.DEADLINE
    memory.DEADLINE = time.monotonic() + seconds
    old_handler = None
    if (
        hasattr(signal, "setitimer")
        and threading.current_thread() is threading.main_thread()
    ):

        def expired(signum, frame):
            raise memory.BudgetExhausted(
                "publisher budget exhausted; checkpoint committed progress"
            )

        old_handler = signal.signal(signal.SIGALRM, expired)
        signal.setitimer(signal.ITIMER_REAL, seconds)
    try:
        yield
    finally:
        if old_handler is not None:
            signal.setitimer(signal.ITIMER_REAL, 0)
            signal.signal(signal.SIGALRM, old_handler)
        memory.DEADLINE = old_deadline


def command(*arguments, binary=False, limit=None, timeout=60):
    maximum = limit if limit is not None else 8 * 1024 * 1024
    timeout = min(memory.remaining() or timeout, timeout)
    deadline = time.monotonic() + timeout
    with tempfile.TemporaryFile() as errors:
        process = subprocess.Popen(
            ["gh", *map(str, arguments)], stdout=subprocess.PIPE, stderr=errors
        )
        chunks, size = [], 0
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                while True:
                    seconds = deadline - time.monotonic()
                    if seconds <= 0 or not selector.select(seconds):
                        raise subprocess.TimeoutExpired("gh", timeout)
                    block = os.read(process.stdout.fileno(), 64 * 1024)
                    if not block:
                        break
                    size += len(block)
                    if size > maximum:
                        raise memory.Fail(
                            "GitHub artifact exceeds its byte limit",
                            code="invalid-checkpoint",
                        )
                    chunks.append(block)
            process.wait(timeout=max(0.001, deadline - time.monotonic()))
            if process.returncode:
                errors.seek(0)
                why = errors.read(64 * 1024).decode(errors="replace").lower()
                code = (
                    "auth"
                    if any(
                        word in why for word in ("401", "bad credentials", "auth login")
                    )
                    else "network"
                )
                if "rate limit" in why:
                    code = "rate-limit"
                raise memory.Fail(
                    "gh {} failed (exit {})".format(arguments[0], process.returncode),
                    code=code,
                )
            output = b"".join(chunks)
            return output if binary else output.decode()
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()
            process.stdout.close()


def json_file(path):
    if Path(path).stat().st_size > 64 * 1024:
        raise memory.Fail("artifact manifest exceeds 64 KiB", code="invalid-checkpoint")
    with open(path) as source:
        return json.load(
            source,
            object_pairs_hook=memory.unique_object,
            parse_constant=memory.reject_constant,
        )


def validate_cursor(cursor, depth=0):
    if cursor is None:
        return
    if (
        depth > 3
        or not isinstance(cursor, dict)
        or not {"since", "page"} <= cursor.keys()
    ):
        raise memory.Fail("invalid checkpoint cursor", code="invalid-checkpoint")
    if set(cursor) - {"since", "page", "anchor", "restarts", "validate", "next"}:
        raise memory.Fail("unknown checkpoint cursor field", code="incompatible")
    if cursor["since"] is not None:
        memory.timestamp(cursor["since"])
    if type(cursor["page"]) is not int or not 1 <= cursor["page"] <= 1000000:
        raise memory.Fail("invalid checkpoint cursor page", code="invalid-checkpoint")
    if (
        type(cursor.get("restarts", 0)) is not int
        or not 0 <= cursor.get("restarts", 0) <= 5
    ):
        raise memory.Fail(
            "invalid checkpoint cursor restarts", code="invalid-checkpoint"
        )
    if "validate" in cursor and type(cursor["validate"]) is not bool:
        raise memory.Fail(
            "invalid checkpoint cursor validation", code="invalid-checkpoint"
        )
    if "anchor" in cursor:
        if not isinstance(cursor["anchor"], list) or len(cursor["anchor"]) > 100:
            raise memory.Fail(
                "invalid checkpoint page anchor", code="invalid-checkpoint"
            )
        for pair in cursor["anchor"]:
            if (
                not isinstance(pair, list)
                or len(pair) != 2
                or type(pair[0]) is not int
                or pair[0] <= 0
            ):
                raise memory.Fail(
                    "invalid checkpoint anchor identity", code="invalid-checkpoint"
                )
            memory.timestamp(pair[1])
    if cursor.get("validate") and (
        "anchor" not in cursor or "next" not in cursor or cursor["page"] < 2
    ):
        raise memory.Fail(
            "incomplete checkpoint validation cursor", code="invalid-checkpoint"
        )
    if "next" in cursor:
        validate_cursor(cursor["next"], depth + 1)


def validate_sync(db, repo):
    for row in db.execute("SELECT * FROM sync_state"):
        name, value = row["pass"], row["mark"]
        if name not in CHECKPOINT_MARKS:
            raise memory.Fail("unexpected checkpoint sync mark", code="incompatible")
        memory.timestamp(row["at"])
        if name in memory.SNAPSHOT_MARKS or name == "detail_complete":
            memory.timestamp(value)
        elif name == "backfill_detail":
            if value != repo:
                raise memory.Fail(
                    "checkpoint backfill repository mismatch", code="invalid-checkpoint"
                )
        elif name == "walk":
            walk = json.loads(value, object_pairs_hook=memory.unique_object)
            if (
                not isinstance(walk, dict)
                or set(walk) != {"repo", "since", "started", "listing", "cursor"}
                or walk["repo"] != repo
            ):
                raise memory.Fail("invalid checkpoint walk", code="invalid-checkpoint")
            memory.timestamp(walk["started"])
            if walk["since"] is not None:
                memory.timestamp(walk["since"])
                if not memory.mark(db, "poll") or walk["since"] != memory.overlap(
                    memory.mark(db, "poll")
                ):
                    raise memory.Fail(
                        "checkpoint walk does not match its poll cursor",
                        code="invalid-checkpoint",
                    )
            if type(walk["listing"]) is not int or not 0 <= walk["listing"] <= len(
                memory.LISTINGS
            ):
                raise memory.Fail(
                    "invalid checkpoint listing position", code="invalid-checkpoint"
                )
            validate_cursor(walk["cursor"])
        else:
            allowed = {
                "authority_source": {"push-list", "badge-fallback"},
                "authority_attempt": {"success", "unavailable"},
                "detail_attempt": {"complete", "partial"},
                "sweep_attempt": {"running", "success", "incomplete"},
            }
            if value not in allowed[name]:
                raise memory.Fail(
                    "invalid checkpoint attempt state", code="invalid-checkpoint"
                )


def checkpoint_manifest(data, repo):
    if (
        not isinstance(data, dict)
        or data.get("artifact") != "review-memory-checkpoint"
        or data.get("repository") != repo
    ):
        raise memory.Fail(
            "not a checkpoint for this repository", code="invalid-checkpoint"
        )
    if (
        type(data.get("checkpoint_format")) is not int
        or data.get("checkpoint_format") != CHECKPOINT_FORMAT
        or type(data.get("source_format")) is not int
        or data.get("source_format") != memory.SOURCE_FORMAT
    ):
        raise memory.Fail("unsupported checkpoint format", code="incompatible")
    if type(data.get("index_version")) is not int or data["index_version"] < 1:
        raise memory.Fail("unsupported checkpoint index", code="incompatible")
    for name in ("run_id", "run_attempt"):
        if type(data.get(name)) is not int or data[name] < 1:
            raise memory.Fail("invalid checkpoint sequence", code="invalid-checkpoint")
    if type(data.get("pending")) is not bool:
        raise memory.Fail("checkpoint has no pending state", code="invalid-checkpoint")
    memory.timestamp(data.get("created_at"))
    if type(data.get("badge_fallback_approved")) is not bool or data.get(
        "authority_source"
    ) not in ("push-list", "badge-fallback", "unknown"):
        raise memory.Fail(
            "invalid checkpoint authority policy", code="invalid-checkpoint"
        )
    for name, length in (("generator_commit", 40), ("sha256", 64)):
        if not isinstance(data.get(name), str) or not re.fullmatch(
            "[0-9a-f]{" + str(length) + "}", data[name]
        ):
            raise memory.Fail("invalid checkpoint digest", code="invalid-checkpoint")
    for name in ("parent_sha256", "consent_release_sha256"):
        parent = data.get(name)
        if parent is not None and (
            not isinstance(parent, str) or not re.fullmatch(r"[0-9a-f]{64}", parent)
        ):
            raise memory.Fail(
                "invalid checkpoint lineage digest", code="invalid-checkpoint"
            )
    for name, maximum in (
        ("compressed_bytes", memory.MAX_SNAPSHOT_BYTES),
        ("database_bytes", memory.MAX_DATABASE_BYTES),
    ):
        if type(data.get(name)) is not int or not 0 < data[name] <= maximum:
            raise memory.Fail("invalid checkpoint size", code="invalid-checkpoint")
    return data


def write_checkpoint(
    db, destination, args, pending, parent=None, approved=False, consent_release=None
):
    memory.validate_projection(db)
    directory = Path(destination)
    directory.mkdir(parents=True, exist_ok=False)
    path = directory / "checkpoint.db"
    clean = memory.connect(str(path), args.repo)
    try:
        memory.copy_public_raw(db, clean)
        clean.executemany(
            "INSERT INTO authority VALUES (?)",
            db.execute("SELECT login FROM authority"),
        )
        clean.executemany(
            "INSERT INTO stale VALUES (?)", db.execute("SELECT number FROM stale")
        )
        for row in db.execute("SELECT * FROM sync_state"):
            if row["pass"] in CHECKPOINT_MARKS:
                clean.execute("INSERT INTO sync_state VALUES (?,?,?)", tuple(row))
        memory.rebuild_all(clean)
        clean.commit()
        validate_sync(clean, args.repo)
        counts = memory.snapshot_counts(clean)
    finally:
        clean.close()
    zipped = directory / "checkpoint.db.gz"
    with open(path, "rb") as source, open(zipped, "wb") as out:
        with gzip.GzipFile(fileobj=out, mode="wb", filename="", mtime=0) as compressed:
            shutil.copyfileobj(source, compressed)
    data = {
        "artifact": "review-memory-checkpoint",
        "checkpoint_format": CHECKPOINT_FORMAT,
        "source_format": memory.SOURCE_FORMAT,
        "index_version": memory.VERSION,
        "repository": args.repo,
        "generator_commit": args.sha,
        "run_id": args.run_id,
        "run_attempt": args.run_attempt,
        "created_at": memory.now(),
        "parent_sha256": parent,
        "pending": pending,
        "authority_source": memory.mark(db, "authority_source") or "unknown",
        "badge_fallback_approved": approved,
        "consent_release_sha256": consent_release,
        "counts": counts,
        "sha256": digest(zipped),
        "compressed_bytes": zipped.stat().st_size,
        "database_bytes": path.stat().st_size,
    }
    checkpoint_manifest(data, args.repo)
    (directory / "checkpoint.json").write_text(
        json.dumps(data, sort_keys=True, indent=2) + "\n"
    )
    path.unlink()
    return data


def open_checkpoint(directory, repo):
    directory = Path(directory)
    data = checkpoint_manifest(json_file(directory / "checkpoint.json"), repo)
    zipped = directory / "checkpoint.db.gz"
    if (
        zipped.stat().st_size != data["compressed_bytes"]
        or digest(zipped) != data["sha256"]
    ):
        raise memory.Fail("checkpoint digest mismatch", code="invalid-checkpoint")
    path = directory / "checkpoint.db"
    size = 0
    with gzip.open(zipped, "rb") as source, open(path, "wb") as out:
        while True:
            memory.remaining()
            block = source.read(64 * 1024)
            if not block:
                break
            size += len(block)
            if size > data["database_bytes"]:
                raise memory.Fail(
                    "checkpoint expands beyond its limit", code="invalid-checkpoint"
                )
            out.write(block)
    if size != data["database_bytes"]:
        raise memory.Fail(
            "checkpoint database size mismatch", code="invalid-checkpoint"
        )
    db = sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True)
    db.row_factory = sqlite3.Row
    try:
        db.execute("PRAGMA trusted_schema=OFF")
        expected = memory.connect(":memory:", repo)
        try:
            if memory.schema_objects(db) != memory.schema_objects(expected):
                raise memory.Fail("unsupported checkpoint schema", code="incompatible")
        finally:
            expected.close()
        if db.execute("PRAGMA integrity_check").fetchone()[0] != "ok":
            raise memory.Fail(
                "checkpoint integrity check failed", code="invalid-checkpoint"
            )
        if db.execute("SELECT name FROM repository WHERE id=1").fetchone()[0] != repo:
            raise memory.Fail(
                "checkpoint database repository mismatch", code="invalid-checkpoint"
            )
        if db.execute("PRAGMA user_version").fetchone()[0] != data["index_version"]:
            raise memory.Fail(
                "checkpoint index version mismatch", code="invalid-checkpoint"
            )
        for table in ("history", "decisions", "decision_sources"):
            if db.execute("SELECT 1 FROM " + table + " LIMIT 1").fetchone():
                raise memory.Fail(
                    "checkpoint includes local history or annotations",
                    code="invalid-checkpoint",
                )
        validate_sync(db, repo)
        if (memory.mark(db, "authority_source") or "unknown") != data[
            "authority_source"
        ]:
            raise memory.Fail(
                "checkpoint authority metadata mismatch", code="invalid-checkpoint"
            )
        memory.validate_projection(db)
        memory.check_public_credentials(db)
        if memory.snapshot_counts(db) != data["counts"]:
            raise memory.Fail("checkpoint counts mismatch", code="invalid-checkpoint")
    finally:
        db.close()
    return path, data


def unpack_checkpoint(blob, destination):
    with zipfile.ZipFile(io.BytesIO(blob)) as archive:
        entries = archive.infolist()
        if (
            len(entries) != len(CHECKPOINT_FILES)
            or {entry.filename for entry in entries} != CHECKPOINT_FILES
        ):
            raise memory.Fail(
                "checkpoint archive has unexpected paths", code="invalid-checkpoint"
            )
        for entry in entries:
            maximum = (
                64 * 1024
                if entry.filename.endswith(".json")
                else memory.MAX_SNAPSHOT_BYTES
            )
            if (
                entry.file_size > maximum
                or (entry.external_attr >> 16) & 0o170000 == 0o120000
            ):
                raise memory.Fail(
                    "checkpoint archive has an unsafe entry", code="invalid-checkpoint"
                )
            with (
                archive.open(entry) as source,
                open(Path(destination) / entry.filename, "wb") as out,
            ):
                size = 0
                while True:
                    block = source.read(64 * 1024)
                    if not block:
                        break
                    size += len(block)
                    if size > maximum:
                        raise memory.Fail(
                            "checkpoint archive exceeds its limit",
                            code="invalid-checkpoint",
                        )
                    out.write(block)


def checkpoint_from_actions(repo, repository_id, directory):
    pages = memory.gh(
        "repos/{}/actions/artifacts?per_page=100".format(repo), paginate=True
    )
    candidates = []
    for page in pages:
        for artifact in page.get("artifacts", []):
            match = re.fullmatch(
                re.escape(CHECKPOINT_PREFIX) + r"([0-9]+)-([0-9]+)", artifact["name"]
            )
            run = artifact.get("workflow_run") or {}
            if (
                match
                and not artifact["expired"]
                and run.get("head_branch") == "main"
                and run.get("head_repository_id") == repository_id
                and run.get("id") == int(match[1])
            ):
                candidates.append((int(match[1]), int(match[2]), artifact))
    for run_id, attempt, artifact in sorted(
        candidates, reverse=True, key=lambda item: item[:2]
    ):
        run = memory.gh("repos/{}/actions/runs/{}".format(repo, run_id))
        if (
            run.get("path") not in (WORKFLOW, WORKFLOW + "@refs/heads/main")
            or run.get("event") not in ("schedule", "workflow_dispatch")
            or run.get("head_branch") != "main"
        ):
            continue
        signed = False
        for signing_attempt in range(
            int(run.get("run_attempt", attempt)), attempt - 1, -1
        ):
            jobs = memory.gh(
                "repos/{}/actions/runs/{}/attempts/{}/jobs?per_page=100".format(
                    repo, run_id, signing_attempt
                ),
                paginate=True,
            )
            if any(
                job.get("name") == SIGN_JOB and job.get("conclusion") == "success"
                for page in jobs
                for job in page.get("jobs", [])
            ):
                signed = True
                break
        if not signed:
            continue
        if artifact["size_in_bytes"] > memory.MAX_SNAPSHOT_BYTES + 2 * 1024 * 1024:
            raise memory.Fail(
                "checkpoint archive is oversized", code="invalid-checkpoint"
            )
        blob = command(
            "api",
            "repos/{}/actions/artifacts/{}/zip".format(repo, int(artifact["id"])),
            binary=True,
            limit=memory.MAX_SNAPSHOT_BYTES + 2 * 1024 * 1024,
            timeout=180,
        )
        unpack_checkpoint(blob, directory)
        manifest = Path(directory) / "checkpoint.json"
        memory.verify_artifact(str(manifest), repo)
        data = checkpoint_manifest(json_file(manifest), repo)
        if (data["run_id"], data["run_attempt"], data["generator_commit"]) != (
            run_id,
            attempt,
            run["head_sha"],
        ):
            raise memory.Fail(
                "checkpoint does not match its Actions run", code="invalid-checkpoint"
            )
        memory.verify_artifact(
            str(Path(directory) / "checkpoint.db.gz"), repo, data["generator_commit"]
        )
        return open_checkpoint(directory, repo)
    return None


def latest_release(repo, directory):
    try:
        manifests = memory.snapshot_manifests(repo, directory, strict=True)
    except memory.Fail as error:
        if error.code in ("no-release", "incompatible"):
            return None
        raise
    return max(
        (data for data, _ in manifests),
        key=lambda data: (data["generated_at"], data["release_id"]),
    )


def sweep_cost(db):
    counts = {
        row["kind"]: row["n"]
        for row in db.execute("SELECT kind,count(*) AS n FROM raw GROUP BY kind")
    }
    listings = sum(
        max(1, (counts.get(kind, 0) + size - 1) // size)
        for kind, _, size in memory.LISTINGS
    )
    reviews = db.execute(
        "SELECT count(*) FROM threads WHERE kind='pr' AND deleted IS NULL"
    ).fetchone()[0]
    return 3 * listings + reviews + 50


def authority_approved(latest, checkpoint, explicit):
    if explicit:
        return True
    if latest and latest["authority_source"] == "badge-fallback":
        return True
    consent = bool(checkpoint and checkpoint["badge_fallback_approved"])
    if latest and latest["authority_source"] == "push-list":
        return consent and checkpoint.get("consent_release_sha256") == latest["sha256"]
    return consent


def prepare(args):
    output = Path(args.output).resolve()
    if output.exists():
        raise memory.Fail(
            "publisher output exists; use a fresh directory", code="occupied"
        )
    output.mkdir(parents=True)
    with tempfile.TemporaryDirectory(
        prefix="review-memory-publisher-", dir=args.work_dir
    ) as work:
        work = Path(work)
        db = lock = None
        latest = checkpoint = restored = None
        ready = approved = False
        error_code = error_reason = None
        try:
            with budget(args.budget):
                info = memory.gh("repos/" + args.repo)
                if not isinstance(info, dict) or info.get("private") is not False:
                    raise memory.Fail(
                        "publisher requires a public repository", code="policy"
                    )
                (work / "releases").mkdir()
                (work / "restored").mkdir()
                # Even a fresh data collection keeps the verified authority
                # baseline; it never silently downgrades an existing release.
                latest = latest_release(args.repo, str(work / "releases"))
                if not args.fresh:
                    try:
                        restored = checkpoint_from_actions(
                            args.repo, info["id"], work / "restored"
                        )
                    except memory.Fail as error:
                        if error.code != "incompatible":
                            raise
                        # Only an attested manifest can reach this outcome.
                        # A format migration may rebuild from a compatible release.
                        restored = None
                checkpoint = restored[1] if restored else None
                pending = bool(
                    checkpoint
                    and (
                        checkpoint["pending"]
                        or latest
                        and memory.timestamp(checkpoint["created_at"])
                        > memory.timestamp(latest["generated_at"])
                    )
                )
                recent = latest and memory.timestamp(memory.now()) - memory.timestamp(
                    latest["generated_at"]
                ) < timedelta(days=7)
                if recent and not pending and not args.force and not args.fresh:
                    return {
                        "outcome": "skipped",
                        "code": "recent-release",
                        "checkpoint": False,
                        "ready": False,
                    }
                dbpath = work / "memory.db"
                lock = memory.pass_lock(str(dbpath), required=True)
                db = memory.connect(str(dbpath), args.repo)
                if restored:
                    source = sqlite3.connect(restored[0])
                    try:
                        source.backup(
                            db, pages=128, progress=lambda *unused: memory.remaining()
                        )
                    finally:
                        source.close()
                    memory.prepare(db, args.repo)
                elif latest and not args.fresh:
                    (work / "snapshot").mkdir()
                    path, data = memory.fetch_snapshot(
                        args.repo, str(work / "snapshot")
                    )
                    memory.install_snapshot(db, path, data, args.repo)
                consent_release = (checkpoint or {}).get("consent_release_sha256")
                if args.allow_badge_fallback:
                    consent_release = latest["sha256"] if latest else None
                approved = authority_approved(
                    latest, checkpoint, args.allow_badge_fallback
                )
                collector = SimpleNamespace(repo=args.repo, max=args.batch)
                try:
                    walk = memory.mark(db, "walk")
                    if not memory.mark(db, "poll") or (
                        walk and json.loads(walk)["since"] is None
                    ):
                        memory.cmd_backfill(db, collector)
                    else:
                        memory.cmd_poll(db, collector)
                        memory.cmd_detail(db, collector)
                    source_mode = memory.mark(db, "authority_source")
                    if (
                        source_mode == "push-list"
                        and memory.mark(db, "authority_attempt") == "success"
                    ):
                        approved = False
                        consent_release = None
                    elif approved and source_mode == "push-list":
                        db.execute("DELETE FROM authority")
                        memory.mark(db, "authority_source", "badge-fallback")
                        memory.rebuild_all(db)
                        db.commit()
                    elif source_mode != "push-list" and not approved:
                        raise memory.Fail(
                            "badge authority needs explicit authorization before publication",
                            code="authority-policy",
                        )
                    if not memory.stale_prs(db) and not memory.mark(db, "walk"):
                        quota = memory.gh("rate_limit")["resources"]["core"][
                            "remaining"
                        ]
                        if quota < sweep_cost(db):
                            raise memory.Fail(
                                "not enough quota for a complete sweep; resume next preparation run",
                                code="quota",
                            )
                        memory.cmd_sweep(db, collector)
                    memory.publication_ready(db)
                    ready = True
                except (
                    memory.Fail,
                    memory.BudgetExhausted,
                    OSError,
                    subprocess.TimeoutExpired,
                ) as error:
                    db.rollback()
                    error_code = memory.failure_code(error)
                    error_reason = memory.safe_label(str(error))[:240]
            # This separate phase is not inside the spent collector deadline.
            # Verification failures during restore never reach it.
            with budget(180):
                parent = (
                    checkpoint["sha256"]
                    if checkpoint
                    else (latest["sha256"] if latest else None)
                )
                checkpoint_data = write_checkpoint(
                    db,
                    output / "checkpoint",
                    args,
                    True,
                    parent,
                    approved,
                    consent_release,
                )
                if ready:
                    tag = (
                        memory.SNAPSHOT_PREFIX
                        + memory.timestamp(memory.now()).strftime("%Y%m%dT%H%M%SZ")
                        + "-"
                        + str(args.run_id)
                    )
                    try:
                        memory.build_snapshot(
                            db,
                            SimpleNamespace(
                                repo=args.repo,
                                output=str(output / "ready"),
                                release_id=tag,
                                generator_commit=args.sha,
                            ),
                        )
                    except (
                        memory.Fail,
                        memory.BudgetExhausted,
                        OSError,
                        subprocess.TimeoutExpired,
                    ) as error:
                        ready = False
                        error_code = memory.failure_code(error)
                        error_reason = memory.safe_label(str(error))[:240]
                checkpoint_data["pending"] = not ready
                (output / "checkpoint/checkpoint.json").write_text(
                    json.dumps(checkpoint_data, sort_keys=True, indent=2) + "\n"
                )
            return {
                "outcome": "ready" if ready else "partial",
                "code": error_code or "ready",
                "reason": error_reason,
                "checkpoint": True,
                "ready": ready,
                "checkpoint_name": CHECKPOINT_PREFIX
                + str(args.run_id)
                + "-"
                + str(args.run_attempt),
                "ready_name": READY_PREFIX
                + str(args.run_id)
                + "-"
                + str(args.run_attempt),
            }
        finally:
            if db is not None:
                db.close()
            if lock is not None:
                lock.close()


def verify_package(directory, repo, sha, run_id, run_attempt):
    directory = Path(directory)
    checkpoint_dir = directory / "checkpoint"
    ready_dir = directory / "ready"
    if checkpoint_dir.exists():
        if {path.name for path in checkpoint_dir.iterdir()} != CHECKPOINT_FILES:
            raise memory.Fail(
                "checkpoint package has unexpected files", code="invalid-checkpoint"
            )
        data = checkpoint_manifest(json_file(checkpoint_dir / "checkpoint.json"), repo)
        if (data["generator_commit"], data["run_id"], data["run_attempt"]) != (
            sha,
            run_id,
            run_attempt,
        ):
            raise memory.Fail(
                "checkpoint package does not belong to this run",
                code="invalid-checkpoint",
            )
        blob = checkpoint_dir / "checkpoint.db.gz"
        if (
            blob.stat().st_size != data["compressed_bytes"]
            or digest(blob) != data["sha256"]
        ):
            raise memory.Fail(
                "checkpoint package digest mismatch", code="invalid-checkpoint"
            )
    if ready_dir.exists():
        if {path.name for path in ready_dir.iterdir()} != {
            "manifest.json",
            "memory.db.gz",
            "SHA256SUMS",
        }:
            raise memory.Fail(
                "release package has unexpected files", code="invalid-snapshot"
            )
        data = memory.read_manifest(str(ready_dir / "manifest.json"), repo)
        if data["generator_commit"] != sha or data["release_id"].rsplit("-", 1)[
            1
        ] != str(run_id):
            raise memory.Fail(
                "release package does not belong to this run", code="invalid-snapshot"
            )
        blob = ready_dir / "memory.db.gz"
        if (
            blob.stat().st_size != data["compressed_bytes"]
            or digest(blob) != data["sha256"]
        ):
            raise memory.Fail(
                "release package digest mismatch", code="invalid-snapshot"
            )
        if (ready_dir / "SHA256SUMS").read_text() != data[
            "sha256"
        ] + "  memory.db.gz\n":
            raise memory.Fail("release checksum file mismatch", code="invalid-snapshot")
    return {
        "outcome": "verified",
        "checkpoint": checkpoint_dir.exists(),
        "ready": ready_dir.exists(),
    }


def release_asset(repo, asset, destination, maximum):
    if asset["size"] > maximum:
        raise memory.Fail("release asset is oversized", code="invalid-snapshot")
    blob = command(
        "api",
        "repos/{}/releases/assets/{}".format(repo, int(asset["id"])),
        "-H",
        "Accept: application/octet-stream",
        binary=True,
        limit=maximum,
    )
    Path(destination).write_bytes(blob)


def managed_releases(repo):
    releases = memory.gh("repos/{}/releases?per_page=100".format(repo), paginate=True)
    return [
        release
        for release in releases
        if re.fullmatch(
            re.escape(memory.SNAPSHOT_PREFIX) + r"[0-9]{8}T[0-9]{6}Z-[0-9]+",
            release.get("tag_name", ""),
        )
    ]


def release_manifest(repo, release, directory):
    asset = next(
        (asset for asset in release["assets"] if asset["name"] == "manifest.json"), None
    )
    if asset is None:
        raise memory.Fail("managed release has no manifest", code="unverified")
    path = Path(directory) / "manifest.json"
    release_asset(repo, asset, path, 64 * 1024)
    memory.verify_artifact(str(path), repo)
    data = memory.read_manifest(str(path), repo)
    if data["release_id"] != release["tag_name"]:
        raise memory.Fail("managed release tag mismatch", code="unverified")
    return data


def verify_release_blob(repo, release, data, directory):
    blob = next(
        (asset for asset in release["assets"] if asset["name"] == "memory.db.gz"), None
    )
    if blob is None or blob.get("size") != data["compressed_bytes"]:
        raise memory.Fail(
            "managed release has no complete database asset", code="unverified"
        )
    expected = "sha256:" + data["sha256"]
    if blob.get("digest"):
        if blob["digest"] != expected:
            raise memory.Fail(
                "managed release database digest mismatch", code="unverified"
            )
    else:
        path = Path(directory) / "retention-memory.db.gz"
        release_asset(repo, blob, path, data["compressed_bytes"])
        if digest(path) != data["sha256"]:
            raise memory.Fail(
                "managed release database digest mismatch", code="unverified"
            )
        memory.verify_artifact(str(path), repo, data["generator_commit"])


def remove_release(repo, tag):
    if not re.fullmatch(
        re.escape(memory.SNAPSHOT_PREFIX) + r"[0-9]{8}T[0-9]{6}Z-[0-9]+", tag
    ):
        raise memory.Fail("refusing unmanaged release deletion", code="policy")
    command("release", "delete", tag, "--repo", repo, "--yes")
    delete_tag_ref(repo, tag)


def delete_tag_ref(repo, tag):
    refs = memory.gh("repos/{}/git/matching-refs/tags/{}".format(repo, tag))
    if any(ref["ref"] == "refs/tags/" + tag for ref in refs):
        command(
            "api",
            "--method",
            "DELETE",
            "repos/{}/git/refs/tags/{}".format(repo, tag),
            "--silent",
        )


def abandoned_owned_draft(repo, release):
    if (
        not release.get("draft")
        or (release.get("author") or {}).get("login") != "github-actions[bot]"
    ):
        return False
    sha = release.get("target_commitish", "")
    if not isinstance(sha, str) or not re.fullmatch(r"[0-9a-f]{40}", sha):
        return False
    created = memory.timestamp(release.get("created_at"))
    if memory.timestamp(memory.now()) - created < timedelta(hours=1):
        return False
    run_id = release["tag_name"].rsplit("-", 1)[1]
    run = memory.gh("repos/{}/actions/runs/{}".format(repo, run_id))
    return (
        run.get("path") in (WORKFLOW, WORKFLOW + "@refs/heads/main")
        and run.get("event") in ("schedule", "workflow_dispatch")
        and run.get("head_branch") == "main"
        and run.get("head_sha") == sha
        and run.get("status") == "completed"
    )


def maintain(repo):
    verified = []
    failures = []
    with (
        tempfile.TemporaryDirectory(prefix="review-memory-maintenance-") as directory,
        budget(180),
    ):
        for release in managed_releases(repo):
            try:
                if abandoned_owned_draft(repo, release):
                    remove_release(repo, release["tag_name"])
                    continue
                data = release_manifest(repo, release, directory)
                if not release["draft"]:
                    verify_release_blob(repo, release, data, directory)
            except memory.Fail as error:
                failures.append({"tag": release["tag_name"], "code": error.code})
                print(
                    "::warning title=Review memory maintenance::{} left untouched ({})".format(
                        release["tag_name"], error.code
                    ),
                    file=sys.stderr,
                )
                continue
            if release["draft"]:
                remove_release(repo, release["tag_name"])
            else:
                verified.append((data, release))
        verified.sort(
            key=lambda pair: (pair[0]["generated_at"], pair[0]["release_id"]),
            reverse=True,
        )
        removed = []
        for _, release in verified[1:]:
            remove_release(repo, release["tag_name"])
            removed.append(release["tag_name"])
    return {
        "outcome": "maintained",
        "removed": removed,
        "unverified_untouched": failures,
    }


def find_release(repo, tag):
    return next(
        (release for release in managed_releases(repo) if release["tag_name"] == tag),
        None,
    )


def promote(args):
    directory = Path(args.output) / "ready"
    verify_package(args.output, args.repo, args.sha, args.run_id, args.run_attempt)
    data = memory.read_manifest(str(directory / "manifest.json"), args.repo)
    for name in ("manifest.json", "memory.db.gz"):
        memory.verify_artifact(str(directory / name), args.repo, args.sha)
    tip = memory.gh("repos/{}/git/ref/heads/main".format(args.repo))["object"]["sha"]
    if tip != args.sha:
        return {"outcome": "skipped", "code": "superseded"}
    tag = data["release_id"]
    notes = Path(args.output) / "release-notes.txt"
    notes.write_text(
        "Public review memory for {}.\n\nSource format {}, index version {}.\nCollected {} threads and {} documents; {} incomplete PR file lists.\nPoll checkpoint: {}. Sweep checkpoint: {}.\nAuthority: {}. Generator: {}.\n".format(
            args.repo,
            data["source_format"],
            data["index_version"],
            data["counts"]["threads"],
            data["counts"]["documents"],
            data["counts"]["incomplete_files"],
            data["passes"]["poll"],
            data["passes"]["sweep"],
            data["authority_source"],
            args.sha,
        )
    )
    if find_release(args.repo, tag) is not None:
        raise memory.Fail("release identity already exists", code="occupied")
    refs = memory.gh("repos/{}/git/matching-refs/tags/{}".format(args.repo, tag))
    if any(ref["ref"] == "refs/tags/" + tag for ref in refs):
        raise memory.Fail("release tag identity already exists", code="occupied")
    published = created = False
    try:
        command(
            "release",
            "create",
            tag,
            *(directory / name for name in RELEASE_ASSETS),
            "--repo",
            args.repo,
            "--target",
            args.sha,
            "--title",
            "Review memory " + data["generated_at"],
            "--notes-file",
            notes,
            "--draft",
            "--latest=false",
            timeout=600,
        )
        created = True
        release = find_release(args.repo, tag)
        if release is None or not release["draft"]:
            raise memory.Fail("created draft is missing", code="invalid-snapshot")
        assets = {asset["name"]: asset for asset in release["assets"]}
        for name in RELEASE_ASSETS:
            expected = digest(directory / name)
            if name not in assets:
                raise memory.Fail(
                    "release upload is incomplete", code="invalid-snapshot"
                )
            server_digest = assets[name].get("digest")
            if server_digest:
                if server_digest != "sha256:" + expected:
                    raise memory.Fail(
                        "release uploaded asset digest mismatch",
                        code="invalid-snapshot",
                    )
            else:
                downloaded = Path(args.output) / ("uploaded-" + name)
                release_asset(
                    args.repo,
                    assets[name],
                    downloaded,
                    (directory / name).stat().st_size,
                )
                if digest(downloaded) != expected:
                    raise memory.Fail(
                        "release uploaded bytes mismatch", code="invalid-snapshot"
                    )
        if (
            memory.gh("repos/{}/git/ref/heads/main".format(args.repo))["object"]["sha"]
            != args.sha
        ):
            return {"outcome": "skipped", "code": "superseded"}
        command(
            "release",
            "edit",
            tag,
            "--repo",
            args.repo,
            "--draft=false",
            "--latest=false",
        )
        published = True
    finally:
        if not published:
            try:
                found = find_release(args.repo, tag)
                if (
                    found
                    and found.get("draft")
                    and found.get("target_commitish") == args.sha
                    and (
                        created
                        or (found.get("author") or {}).get("login")
                        == "github-actions[bot]"
                    )
                ):
                    remove_release(args.repo, tag)
            except (memory.Fail, OSError, subprocess.TimeoutExpired):
                print(
                    "review memory: draft cleanup incomplete; inspect " + tag,
                    file=sys.stderr,
                )
    return {"outcome": "published", "tag": tag}


def environment_bool(name):
    value = os.environ.get(name, "false").lower()
    if value not in ("true", "false"):
        raise memory.Fail("invalid publisher boolean input", code="usage")
    return value == "true"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "operation", choices=("prepare", "verify", "publish", "maintain")
    )
    parser.add_argument("--repo", default=os.environ.get("GH_REPO", memory.REPO))
    parser.add_argument(
        "--output", default=os.environ.get("MEMORY_OUTPUT", "memory-artifacts")
    )
    parser.add_argument("--work-dir", default=None)
    parser.add_argument("--sha", default=os.environ.get("GITHUB_SHA"))
    parser.add_argument("--run-id", type=int, default=os.environ.get("GITHUB_RUN_ID"))
    parser.add_argument(
        "--run-attempt",
        type=int,
        default=os.environ.get(
            "MEMORY_RUN_ATTEMPT", os.environ.get("GITHUB_RUN_ATTEMPT", 1)
        ),
    )
    parser.add_argument("--batch", type=int, default=80)
    parser.add_argument("--budget", type=float, default=900)
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--fresh", action="store_true")
    parser.add_argument("--allow-badge-fallback", action="store_true")
    args = parser.parse_args(argv)
    try:
        args.repo = memory.repository_name(args.repo)
        args.force |= environment_bool("MEMORY_FORCE")
        args.fresh |= environment_bool("MEMORY_FRESH")
        args.allow_badge_fallback |= environment_bool("MEMORY_ALLOW_BADGE_FALLBACK")
        if args.operation != "maintain" and (
            not args.sha
            or not re.fullmatch(r"[0-9a-f]{40}", args.sha)
            or not args.run_id
            or args.run_id < 1
            or args.run_attempt < 1
        ):
            raise memory.Fail(
                "publisher needs this run's commit and sequence", code="usage"
            )
        if not 1 <= args.batch <= 500 or not 1 <= args.budget <= 1200:
            raise memory.Fail("publisher batch/budget is out of range", code="usage")
        if args.operation == "prepare":
            result = prepare(args)
        elif args.operation == "verify":
            result = verify_package(
                args.output, args.repo, args.sha, args.run_id, args.run_attempt
            )
        elif args.operation == "publish":
            result = promote(args)
        else:
            result = maintain(args.repo)
        if args.operation == "prepare" and os.environ.get("GITHUB_OUTPUT"):
            with open(os.environ["GITHUB_OUTPUT"], "a") as out:
                for name in ("checkpoint", "ready", "checkpoint_name", "ready_name"):
                    value = result.get(name, False)
                    out.write(
                        "{}={}\n".format(
                            name, str(value).lower() if type(value) is bool else value
                        )
                    )
        print(json.dumps(result, sort_keys=True))
        return 0
    except (
        memory.Fail,
        memory.BudgetExhausted,
        OSError,
        ValueError,
        sqlite3.DatabaseError,
        subprocess.TimeoutExpired,
        zipfile.BadZipFile,
        EOFError,
        zlib.error,
        http.client.HTTPException,
    ) as error:
        print(
            json.dumps(
                {
                    "outcome": "error",
                    "code": memory.failure_code(error),
                    "reason": memory.safe_label(str(error))[:240],
                }
            )
        )
        return 1
    finally:
        memory.METER.stop()


if __name__ == "__main__":
    sys.exit(main())
