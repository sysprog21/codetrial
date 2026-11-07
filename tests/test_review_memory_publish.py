#!/usr/bin/env python3
"""The publisher's checkpoint and opaque privilege-boundary contracts."""

import importlib.util
import io
import gzip
import json
import tempfile
import subprocess
import sys
import unittest
import zipfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "review_memory_publish", ROOT / ".claude/skills/codetrial-review/publish-memory.py"
)
SUPPORT = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SUPPORT)
SUPPORT.loader.exec_module(support)


class PublisherTests(unittest.TestCase):
    def setUp(self):
        self.publisher = importlib.util.module_from_spec(SPEC)
        SPEC.loader.exec_module(self.publisher)
        self.memory = self.publisher.memory
        support.quiet_meter(self.memory)
        # A ::warning:: printed here would also surface as a real annotation.
        real_print = print
        self.publisher.print = lambda *args, file=None, **kwargs: (
            None if file is sys.stderr else real_print(*args, file=file, **kwargs)
        )
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name)
        self.args = SimpleNamespace(
            repo=self.memory.REPO, sha="a" * 40, run_id=123, run_attempt=1
        )

    def checkpoint(self):
        destination = self.base / "checkpoint"
        with self.memory.connect(":memory:") as db:
            self.memory.store_raw(
                db,
                "files:1",
                1,
                "files",
                [
                    {
                        "filename": "src/a.rs",
                        "patch": "@@ -1 +1 @@\n+https://user:password@host",
                    }
                ],
            )
            db.execute("INSERT INTO stale VALUES (1)")
            db.execute(
                "INSERT INTO decisions VALUES (1,'local','private note','[]','2026-01-01')"
            )
            self.memory.mark(db, "sweep_attempt", "running")
            data = self.publisher.write_checkpoint(db, destination, self.args, True)
        return destination, data

    def test_checkpoint_preserves_progress_and_projects_without_local_notes(self):
        directory, data = self.checkpoint()
        path, restored = self.publisher.open_checkpoint(directory, self.memory.REPO)
        self.assertEqual(restored, data)
        with self.memory.connect(str(path)) as db:
            self.assertEqual(self.memory.mark(db, "sweep_attempt"), "running")
            self.assertEqual(db.execute("SELECT number FROM stale").fetchone()[0], 1)
            self.assertIsNone(db.execute("SELECT 1 FROM decisions").fetchone())
            self.assertEqual(data["counts"]["redacted_records"], 1)
            self.memory.check_public_credentials(db)

    def test_same_schema_older_index_checkpoint_resumes_without_refetch(self):
        directory, data = self.checkpoint()
        path = directory / "checkpoint.db"
        path.write_bytes(gzip.decompress((directory / "checkpoint.db.gz").read_bytes()))
        with self.memory.connect(str(path)) as db:
            raw = [tuple(row) for row in db.execute("SELECT * FROM raw ORDER BY key")]
            db.execute("PRAGMA user_version=8")
        blob = directory / "checkpoint.db.gz"
        blob.write_bytes(gzip.compress(path.read_bytes(), mtime=0))
        data.update(
            index_version=8,
            sha256=self.publisher.digest(blob),
            compressed_bytes=blob.stat().st_size,
        )
        (directory / "checkpoint.json").write_text(json.dumps(data))
        path.unlink()
        restored, _ = self.publisher.open_checkpoint(directory, self.memory.REPO)
        with patch.object(
            self.memory, "gh", side_effect=AssertionError("migration refetched source")
        ):
            with self.memory.connect(str(restored)) as db:
                self.assertEqual(
                    db.execute("PRAGMA user_version").fetchone()[0], self.memory.VERSION
                )
                self.assertEqual(
                    [
                        tuple(row)
                        for row in db.execute("SELECT * FROM raw ORDER BY key")
                    ],
                    raw,
                )
                self.assertEqual(self.memory.mark(db, "sweep_attempt"), "running")
                self.assertEqual(
                    db.execute("SELECT number FROM stale").fetchone()[0], 1
                )

    def test_checkpoint_corruption_is_rejected_before_sqlite_open(self):
        directory, _ = self.checkpoint()
        (directory / "checkpoint.db.gz").write_bytes(b"corrupt")
        with patch.object(
            self.publisher,
            "sqlite3",
            new=SimpleNamespace(
                connect=lambda *a, **k: self.fail("opened corrupt database")
            ),
        ):
            with self.assertRaisesRegex(self.memory.Fail, "digest mismatch"):
                self.publisher.open_checkpoint(directory, self.memory.REPO)

    def test_privileged_verification_never_opens_sqlite(self):
        _, _ = self.checkpoint()
        with patch.object(
            self.publisher,
            "sqlite3",
            new=SimpleNamespace(
                connect=lambda *a, **k: self.fail("privileged SQLite open")
            ),
        ):
            result = self.publisher.verify_package(
                self.base, self.memory.REPO, self.args.sha, 123, 1
            )
        self.assertTrue(result["checkpoint"])
        self.assertFalse(result["ready"])

    def test_privileged_handoff_rejects_other_run(self):
        self.checkpoint()
        with self.assertRaisesRegex(self.memory.Fail, "does not belong"):
            self.publisher.verify_package(
                self.base, self.memory.REPO, self.args.sha, 124, 1
            )

    def test_checkpoint_is_never_a_ready_release(self):
        _, data = self.checkpoint()
        with self.assertRaisesRegex(self.memory.Fail, "not a review memory release"):
            self.memory.validate_manifest(data, self.memory.REPO)

    def test_checkpoint_manifest_rejects_boolean_format(self):
        _, data = self.checkpoint()
        for field in ("checkpoint_format", "source_format", "index_version"):
            with self.subTest(field=field), self.assertRaises(self.memory.Fail):
                self.publisher.checkpoint_manifest(
                    {**data, field: True}, self.memory.REPO
                )

    def test_checkpoint_zip_rejects_traversal_and_unexpected_files(self):
        blob = io.BytesIO()
        with zipfile.ZipFile(blob, "w") as archive:
            archive.writestr("../checkpoint.json", "{}")
            archive.writestr("checkpoint.db.gz", "fake")
        with self.assertRaises(self.memory.Fail):
            self.publisher.unpack_checkpoint(blob.getvalue(), self.base / "unpacked")

    def test_download_byte_limit_stops_the_producer(self):
        processes = []

        def launch(arguments, **kwargs):
            process = subprocess.Popen(
                [
                    sys.executable,
                    "-c",
                    "import sys; sys.stdout.buffer.write(b'x' * (8 * 1024 * 1024))",
                ],
                **kwargs,
            )
            processes.append(process)
            return process

        with patch.object(
            self.publisher,
            "subprocess",
            SimpleNamespace(
                Popen=launch,
                PIPE=subprocess.PIPE,
                TimeoutExpired=subprocess.TimeoutExpired,
            ),
        ):
            with self.assertRaisesRegex(self.memory.Fail, "byte limit"):
                self.publisher.command("api", "test", binary=True, limit=1024)
        self.assertIsNotNone(processes[0].poll())

    def test_retention_requires_usable_database_assets(self):
        data = {"compressed_bytes": 100, "sha256": "a" * 64}
        for assets in (
            [],
            [{"name": "memory.db.gz", "size": 100, "digest": "sha256:" + "b" * 64}],
        ):
            with self.subTest(assets=assets), self.assertRaises(self.memory.Fail):
                self.publisher.verify_release_blob(
                    self.memory.REPO, {"assets": assets}, data, self.base
                )
        self.publisher.verify_release_blob(
            self.memory.REPO,
            {
                "assets": [
                    {
                        "name": "memory.db.gz",
                        "size": 100,
                        "digest": "sha256:" + "a" * 64,
                    }
                ]
            },
            data,
            self.base,
        )

    def test_retention_ignores_unverified_releases_and_keeps_the_newest_usable_one(
        self,
    ):
        releases = [
            {
                "tag_name": "review-memory-20261025T00000{}Z-{}".format(i, i),
                "draft": False,
                "assets": [],
            }
            for i in range(1, 6)
        ]
        releases.append(
            {
                "tag_name": "review-memory-20261025T000009Z-9",
                "draft": False,
                "assets": [],
            }
        )

        def manifest(repo, release, directory):
            if release["tag_name"].endswith("-9"):
                raise self.memory.Fail("unverified", code="unverified")
            return {
                "generated_at": "2026-10-25T00:00:00Z",
                "release_id": release["tag_name"],
            }

        with (
            patch.object(self.publisher, "managed_releases", return_value=releases),
            patch.object(self.publisher, "release_manifest", side_effect=manifest),
            patch.object(self.publisher, "verify_release_blob"),
            patch.object(self.publisher, "remove_release") as command,
        ):
            result = self.publisher.maintain(self.memory.REPO)
        self.assertEqual(set(result["removed"]), {r["tag_name"] for r in releases[:4]})
        self.assertEqual(command.call_count, 4)
        self.assertEqual(
            result["unverified_untouched"][0]["tag"], releases[-1]["tag_name"]
        )

    def test_memory_discovery_crosses_more_than_one_hundred_product_releases(self):
        managed = {"tag_name": "review-memory-20261025T000001Z-1"}
        releases = [{"tag_name": "v0.0." + str(i)} for i in range(110)] + [managed]
        with patch.object(self.memory, "gh", return_value=releases) as api:
            self.assertEqual(
                self.publisher.managed_releases(self.memory.REPO), [managed]
            )
        self.assertTrue(api.call_args.kwargs["paginate"])

    def test_downgrade_consent_survives_resume_but_not_a_new_push_list_baseline(self):
        latest = {"authority_source": "push-list", "sha256": "a" * 64}
        checkpoint = {
            "authority_source": "badge-fallback",
            "badge_fallback_approved": True,
            "consent_release_sha256": "a" * 64,
        }
        self.assertTrue(self.publisher.authority_approved(latest, checkpoint, False))
        self.assertFalse(
            self.publisher.authority_approved(
                {**latest, "sha256": "b" * 64}, checkpoint, False
            )
        )
        self.assertTrue(
            self.publisher.authority_approved(
                {**latest, "sha256": "b" * 64}, checkpoint, True
            )
        )
        self.assertFalse(self.publisher.authority_approved(latest, None, False))
        self.assertTrue(self.publisher.authority_approved(None, checkpoint, False))

    def restore_fixture(
        self, signing=True, event="schedule", verify_error=None, suffix=False
    ):
        directory, _ = self.checkpoint()
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, "w") as archive:
            for name in self.publisher.CHECKPOINT_FILES:
                archive.write(directory / name, name)
        artifact = {
            "id": 100,
            "name": self.publisher.CHECKPOINT_PREFIX + "123-1",
            "expired": False,
            "size_in_bytes": len(stream.getvalue()),
            "workflow_run": {
                "id": 123,
                "head_branch": "main",
                "head_repository_id": 99,
            },
        }
        asked = []

        def api(endpoint, paginate=False):
            asked.append(endpoint)
            if endpoint.endswith("/actions/artifacts?per_page=100"):
                return [{"artifacts": [artifact]}]
            if endpoint.endswith("/actions/runs/123"):
                return {
                    "path": self.publisher.WORKFLOW
                    + ("@refs/heads/main" if suffix else ""),
                    "event": event,
                    "head_branch": "main",
                    "head_sha": self.args.sha,
                    "run_attempt": 2,
                }
            if "/jobs?" in endpoint:
                return [
                    {
                        "jobs": [
                            {
                                "name": self.publisher.SIGN_JOB,
                                "conclusion": "success"
                                if signing and "/attempts/2/" in endpoint
                                else "failure",
                            }
                        ]
                    }
                ]
            raise AssertionError(endpoint)

        destination = self.base / "restored"
        destination.mkdir()
        with (
            patch.object(self.memory, "gh", side_effect=api),
            patch.object(
                self.publisher, "command", return_value=stream.getvalue()
            ) as download,
            patch.object(self.memory, "verify_artifact", side_effect=verify_error),
        ):
            if verify_error:
                with self.assertRaises(self.memory.Fail):
                    self.publisher.checkpoint_from_actions(
                        self.memory.REPO, 99, destination
                    )
                result = None
            else:
                result = self.publisher.checkpoint_from_actions(
                    self.memory.REPO, 99, destination
                )
        return result, asked, download

    def test_checkpoint_can_be_signed_by_a_successful_later_rerun_attempt(self):
        restored, asked, _ = self.restore_fixture()
        self.assertEqual(restored[1]["run_attempt"], 1)
        self.assertTrue(any("/attempts/2/jobs" in endpoint for endpoint in asked))

    def test_main_ref_suffixed_workflow_path_restores(self):
        restored, _, _ = self.restore_fixture(suffix=True)
        self.assertIsNotNone(restored)

    def test_abandoned_draft_requires_bot_age_completed_main_run_and_commit(self):
        release = {
            "tag_name": "review-memory-20261025T000000Z-123",
            "draft": True,
            "author": {"login": "github-actions[bot]"},
            "target_commitish": "a" * 40,
            "created_at": "2026-01-01T00:00:00Z",
        }
        run = {
            "path": self.publisher.WORKFLOW,
            "event": "schedule",
            "head_branch": "main",
            "head_sha": "a" * 40,
            "status": "completed",
        }
        with patch.object(self.memory, "gh", return_value=run):
            self.assertTrue(
                self.publisher.abandoned_owned_draft(self.memory.REPO, release)
            )
            self.assertFalse(
                self.publisher.abandoned_owned_draft(
                    self.memory.REPO, {**release, "author": {"login": "someone"}}
                )
            )
            self.assertFalse(
                self.publisher.abandoned_owned_draft(
                    self.memory.REPO, {**release, "created_at": self.memory.now()}
                )
            )
        for change in (
            {"status": "in_progress"},
            {"head_sha": "b" * 40},
            {"event": "pull_request"},
        ):
            with patch.object(self.memory, "gh", return_value={**run, **change}):
                self.assertFalse(
                    self.publisher.abandoned_owned_draft(self.memory.REPO, release)
                )

    def test_failed_signer_never_restores_artifact(self):
        restored, _, download = self.restore_fixture(signing=False)
        self.assertIsNone(restored)
        download.assert_not_called()

    def test_untrusted_trigger_never_restores_artifact(self):
        restored, _, download = self.restore_fixture(event="pull_request")
        self.assertIsNone(restored)
        download.assert_not_called()

    def test_bad_attestation_stops_checkpoint_restore(self):
        self.restore_fixture(
            verify_error=self.memory.Fail("unverified", code="unverified")
        )

    def release_fixture(self):
        args = self.prepare_args()
        directory = Path(args.output) / "ready"
        directory.mkdir(parents=True)
        (directory / "memory.db.gz").write_bytes(b"opaque signed database")
        stamp = self.memory.now()
        tag = (
            self.memory.SNAPSHOT_PREFIX
            + self.memory.timestamp(stamp).strftime("%Y%m%dT%H%M%SZ")
            + "-123"
        )
        data = {
            "artifact": "review-memory",
            "repository": self.memory.REPO,
            "source_format": self.memory.SOURCE_FORMAT,
            "index_version": self.memory.VERSION,
            "generator_commit": args.sha,
            "release_id": tag,
            "generated_at": stamp,
            "passes": {"poll": stamp, "sweep": stamp},
            "counts": {
                "raw": 1,
                "threads": 1,
                "documents": 0,
                "stale_prs": 0,
                "incomplete_files": 0,
                "redacted_records": 0,
            },
            "authority_source": "push-list",
            "compressed_bytes": (directory / "memory.db.gz").stat().st_size,
            "database_bytes": 4096,
            "sha256": self.publisher.digest(directory / "memory.db.gz"),
        }
        (directory / "manifest.json").write_text(json.dumps(data))
        (directory / "SHA256SUMS").write_text(data["sha256"] + "  memory.db.gz\n")
        return args, tag, directory

    def exercise_promotion(self, fail_create=False, move_head=False):
        args, tag, directory = self.release_fixture()
        state = {"created": False, "published": False, "tip_reads": 0}

        def api(endpoint, paginate=False):
            if endpoint.endswith("/git/ref/heads/main"):
                state["tip_reads"] += 1
                return {
                    "object": {
                        "sha": "b" * 40
                        if move_head and state["tip_reads"] > 1
                        else args.sha
                    }
                }
            if "/git/matching-refs/tags/" in endpoint:
                return []
            if endpoint.endswith("/releases?per_page=100"):
                if not state["created"]:
                    return []
                assets = [
                    {
                        "name": name,
                        "size": (directory / name).stat().st_size,
                        "digest": "sha256:" + self.publisher.digest(directory / name),
                    }
                    for name in ("manifest.json", "memory.db.gz", "SHA256SUMS")
                ]
                return [
                    {
                        "tag_name": tag,
                        "draft": not state["published"],
                        "target_commitish": args.sha,
                        "author": {"login": "github-actions[bot]"},
                        "assets": assets,
                    }
                ]
            raise AssertionError(
                "unexpected API including draft-by-tag lookup: " + endpoint
            )

        def write(*arguments, **kwargs):
            if arguments[:2] == ("release", "create"):
                state["created"] = True
                if fail_create:
                    raise subprocess.TimeoutExpired("gh", 600)
            elif arguments[:2] == ("release", "edit"):
                state["published"] = True
            elif arguments[:2] == ("release", "delete"):
                state["created"] = False
            return ""

        with (
            patch.object(self.memory, "gh", side_effect=api),
            patch.object(self.memory, "verify_artifact"),
            patch.object(self.publisher, "command", side_effect=write) as command,
        ):
            if fail_create:
                with self.assertRaises(subprocess.TimeoutExpired):
                    self.publisher.promote(args)
                result = None
            else:
                result = self.publisher.promote(args)
        return result, state, command

    def test_promotion_finds_draft_by_listing_and_preserves_latest(self):
        result, state, command = self.exercise_promotion()
        self.assertEqual(result["outcome"], "published")
        self.assertTrue(state["published"])
        for call in command.call_args_list:
            if call.args[:2] in (("release", "create"), ("release", "edit")):
                self.assertIn("--latest=false", call.args)

    def test_timed_out_upload_cleans_its_draft_even_when_tag_is_already_missing(self):
        _, state, command = self.exercise_promotion(fail_create=True)
        self.assertFalse(state["created"])
        self.assertTrue(
            any(
                call.args[:2] == ("release", "delete")
                for call in command.call_args_list
            )
        )

    def test_main_moving_during_upload_skips_promotion_and_cleans_draft(self):
        result, state, _ = self.exercise_promotion(move_head=True)
        self.assertEqual(result["code"], "superseded")
        self.assertFalse(state["created"])
        self.assertFalse(state["published"])

    def prepare_args(self):
        return SimpleNamespace(
            **vars(self.args),
            output=str(self.base / "output"),
            work_dir=None,
            fresh=False,
            force=False,
            allow_badge_fallback=False,
            budget=30,
            batch=1,
        )

    def test_failed_restore_never_becomes_cold_collection(self):
        args = self.prepare_args()
        with (
            patch.object(self.memory, "gh", return_value={"id": 1, "private": False}),
            patch.object(
                self.publisher,
                "latest_release",
                side_effect=self.memory.Fail("unverified", code="unverified"),
            ),
            patch.object(self.memory, "cmd_backfill") as collector,
        ):
            with self.assertRaises(self.memory.Fail):
                self.publisher.prepare(args)
        collector.assert_not_called()
        self.assertFalse((Path(args.output) / "checkpoint").exists())

    def test_interrupted_first_collection_preserves_checkpoint_without_release(self):
        args = self.prepare_args()
        args.fresh = True
        with (
            patch.object(self.memory, "gh", return_value={"id": 1, "private": False}),
            patch.object(self.publisher, "latest_release", return_value=None),
            patch.object(
                self.memory,
                "cmd_backfill",
                side_effect=self.memory.RateLimited("quota"),
            ),
            patch.object(self.memory, "build_snapshot") as builder,
        ):
            result = self.publisher.prepare(args)
        self.assertTrue(result["checkpoint"])
        self.assertFalse(result["ready"])
        self.assertEqual(result["code"], "rate-limit")
        builder.assert_not_called()
        manifest = self.publisher.json_file(
            Path(args.output) / "checkpoint/checkpoint.json"
        )
        self.assertTrue(manifest["pending"])

    def test_detail_api_failure_cannot_publish_after_normal_collector_return(self):
        args = self.prepare_args()
        args.fresh = True
        stamp = self.memory.now()
        issue = {
            "number": 1,
            "title": "PR",
            "body": "Fix retry",
            "state": "open",
            "pull_request": {},
            "user": {"login": "maint", "type": "User"},
            "author_association": "OWNER",
            "html_url": "https://github.com/sysprog21/codetrial/pull/1",
            "created_at": stamp,
            "updated_at": stamp,
        }

        def api(endpoint, paginate=False):
            if endpoint == "repos/" + self.memory.REPO:
                return {"private": False, "id": 99}
            if "/collaborators?" in endpoint:
                return [{"login": "maint"}]
            if "/issues?" in endpoint:
                return [issue]
            if "/issues/comments?" in endpoint or "/pulls/comments?" in endpoint:
                return []
            if endpoint.endswith("/pulls/1"):
                return {"changed_files": 0}
            if "/pulls/1/reviews?" in endpoint:
                raise self.memory.Fail("upstream unavailable")
            raise AssertionError(endpoint)

        with (
            patch.object(self.memory, "gh", side_effect=api),
            patch.object(self.publisher, "latest_release", return_value=None),
            patch.object(self.memory, "build_snapshot") as builder,
        ):
            result = self.publisher.prepare(args)
        self.assertEqual(result["outcome"], "partial")
        self.assertFalse(result["ready"])
        self.assertTrue(result["checkpoint"])
        builder.assert_not_called()
        data = self.publisher.json_file(
            Path(args.output) / "checkpoint/checkpoint.json"
        )
        self.assertEqual(data["counts"]["stale_prs"], 1)

    def test_only_attested_incompatible_checkpoint_can_be_rebuilt_automatically(self):
        for code in ("incompatible", "unverified"):
            args = self.prepare_args()
            args.output += "-" + code
            with (
                patch.object(
                    self.memory, "gh", return_value={"private": False, "id": 99}
                ),
                patch.object(self.publisher, "latest_release", return_value=None),
                patch.object(
                    self.publisher,
                    "checkpoint_from_actions",
                    side_effect=self.memory.Fail("checkpoint", code=code),
                ),
                patch.object(
                    self.memory,
                    "cmd_backfill",
                    side_effect=self.memory.RateLimited("quota"),
                ) as collector,
            ):
                if code == "incompatible":
                    self.assertTrue(self.publisher.prepare(args)["checkpoint"])
                    collector.assert_called_once()
                else:
                    with self.assertRaises(self.memory.Fail):
                        self.publisher.prepare(args)
                    collector.assert_not_called()

    def test_no_release_and_verification_failure_are_distinct(self):
        for code in ("no-release", "incompatible", "unverified", "network"):
            with (
                self.subTest(code=code),
                patch.object(
                    self.memory,
                    "snapshot_manifests",
                    side_effect=self.memory.Fail("test", code=code),
                ),
            ):
                if code in ("no-release", "incompatible"):
                    self.assertIsNone(
                        self.publisher.latest_release(self.memory.REPO, str(self.base))
                    )
                else:
                    with self.assertRaises(self.memory.Fail):
                        self.publisher.latest_release(self.memory.REPO, str(self.base))
