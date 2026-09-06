#!/usr/bin/env python3
"""Provider-free contract tests for the recording lifecycle harness."""

import json
import os
import subprocess
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "recording-integration.sh"
RECORDING_ID = "recording_123"
RECIPIENT = "candidate@example.test"


def reader(hours=23, **overrides):
    """The recipient's live reader grant, with only what a test varies replaced."""
    return {
        "id": "permission-1",
        "type": "user",
        "role": "reader",
        "emailAddress": RECIPIENT,
        "expirationTime": (
            datetime.now(timezone.utc) + timedelta(hours=hours)
        ).isoformat(),
    } | overrides


# One recording, delivered and readable by its recipient. Every test starts here
# and names only what it changes.
DELIVERED = {
    "files.json": {
        "files": [
            {
                "id": "file-1",
                "name": f"{RECORDING_ID}.mp4",
                "createdTime": (
                    datetime.now(timezone.utc) - timedelta(minutes=5)
                ).isoformat(),
            }
        ]
    },
    "permissions.json": {"permissions": [reader()]},
}


class RecordingHarnessTests(unittest.TestCase):
    def executable(self, directory: Path, name: str, text: str) -> None:
        path = directory / name
        path.write_text(text)
        path.chmod(0o755)

    def run_harness(
        self,
        files=None,
        *,
        phase="delivery,cleanup",
        include_db=True,
        include_recipient=True,
        cleanup_timeout="1",
        cleanup_drive_status="404",
        cleanup_gcs_status="404",
        gcs_prefix=None,
        template_exit="0",
    ):
        files = files or DELIVERED
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bin_dir = root / "bin"
            bin_dir.mkdir()
            for name, value in files.items():
                (root / name).write_text(json.dumps(value))
            self.executable(
                bin_dir,
                "gcloud",
                """#!/bin/sh
case "$*" in
  *'activate-service-account'*) exit 0 ;;
  *'print-access-token'*) echo token ;;
  *) exit 0 ;;
esac
""",
            )
            self.executable(bin_dir, "ffprobe", "#!/bin/sh\nexit 0\n")
            self.executable(bin_dir, "sqlite3", "#!/bin/sh\necho 1\n")
            self.executable(
                bin_dir,
                "curl",
                r"""#!/bin/sh
args="$*"
if echo "$args" | grep -q '%{http_code}'; then
  if echo "$args" | grep -q 'storage.googleapis.com'; then
    case "$args" in *'codetrial%2F%2F'*) echo 200 ;; *) echo "$FAKE_CLEANUP_GCS_STATUS" ;; esac
  elif echo "$args" | grep -q '/drive/v3/files/file-1?'; then echo "$FAKE_CLEANUP_DRIVE_STATUS"
  else
    out=; previous=; for word in "$@"; do [ "$previous" = -o ] && out=$word; previous=$word; done
    [ -z "$out" ] || printf '<div id="recording-ready"></div>' > "$out"
    echo 200
    exit "$FAKE_TEMPLATE_EXIT"
  fi
elif echo "$args" | grep -q '/permissions?'; then cat "$FAKE_ROOT/permissions.json"
elif echo "$args" | grep -q '/drive/v3/files?'; then cat "$FAKE_ROOT/files.json"
else exit 1
fi
""",
            )
            acceptance = root / "acceptance.json"
            acceptance.write_text(json.dumps({"recording_id": RECORDING_ID}))
            (root / "db.sqlite").touch()
            env = os.environ | {
                "PATH": f"{bin_dir}:{os.environ['PATH']}",
                "FAKE_ROOT": str(root),
                "CODETRIAL_RECORDING_INTEGRATION": "1",
                "CODETRIAL_RECORDING_GCS_BUCKET": "bucket",
                "CODETRIAL_RECORDING_DRIVE_ID": "drive",
                "CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON": "{}",
                "CODETRIAL_RECORDING_TEMPLATE_BASE_URL": "https://recording.example",
                "LIVEKIT_URL": "wss://livekit.example",
                "LIVEKIT_API_KEY": "key",
                "LIVEKIT_API_SECRET": "secret",
                "CODETRIAL_RECORDING_ID": RECORDING_ID,
                "CODETRIAL_RECORDING_RECIPIENT_EMAIL": RECIPIENT,
                "CODETRIAL_DB_PATH": str(root / "db.sqlite"),
                "CODETRIAL_RECORDING_ACCEPTANCE_JSON": str(acceptance),
                "CODETRIAL_RECORDING_CLEANUP_TIMEOUT_SECONDS": cleanup_timeout,
                "FAKE_CLEANUP_DRIVE_STATUS": cleanup_drive_status,
                "FAKE_CLEANUP_GCS_STATUS": cleanup_gcs_status,
                "FAKE_TEMPLATE_EXIT": template_exit,
            }
            if not include_db:
                env.pop("CODETRIAL_DB_PATH")
            if not include_recipient:
                env.pop("CODETRIAL_RECORDING_RECIPIENT_EMAIL")
            if gcs_prefix is not None:
                env["CODETRIAL_RECORDING_GCS_PREFIX"] = gcs_prefix
            result = subprocess.run(
                ["sh", SCRIPT, f"--phase={phase}"],
                env=env,
                text=True,
                capture_output=True,
            )
            return result, json.loads(acceptance.read_text())

    def test_delivery_and_cleanup_write_the_same_lifecycle_document(self):
        result, document = self.run_harness()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(document["drive_file_id"], "file-1")
        self.assertEqual(
            document["cleanup_status"],
            {"drive_file_absent": True, "gcs_object_absent": True},
        )

    def test_delivery_refuses_a_non_unique_drive_file(self):
        result, _ = self.run_harness(
            {
                "files.json": {
                    "files": [
                        {
                            "id": "one",
                            "name": f"{RECORDING_ID}.mp4",
                            "createdTime": "2026-09-01T10:00:00Z",
                        },
                        {
                            "id": "two",
                            "name": f"{RECORDING_ID}.mp4",
                            "createdTime": "2026-09-01T10:00:00Z",
                        },
                    ]
                },
                "permissions.json": {"permissions": []},
            }
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exactly one Drive file", result.stderr)

    def test_delivery_does_not_require_the_cleanup_database(self):
        result, document = self.run_harness(phase="delivery", include_db=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("drive_file_id", document)
        self.assertNotIn("cleanup_status", document)

    def test_trailing_gcs_prefix_matches_the_server_object_name(self):
        result, _ = self.run_harness(gcs_prefix="codetrial/")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_delivery_refuses_a_permission_more_than_24_hours_out(self):
        files = DELIVERED | {"permissions.json": {"permissions": [reader(hours=25)]}}
        result, _ = self.run_harness(files, phase="delivery")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("active and within 24 hours of verification", result.stderr)

    def test_delivery_refuses_an_expired_permission(self):
        files = DELIVERED | {"permissions.json": {"permissions": [reader(hours=-1)]}}
        result, _ = self.run_harness(files, phase="delivery")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("active and within 24 hours of verification", result.stderr)

    def test_delivery_refuses_anything_but_one_expiring_reader(self):
        # Two readers means somebody else can still open it, and a reader with
        # no expiry never stops being able to. Both are the same refusal.
        grant = reader()
        for label, permissions in [
            ("two readers", [grant, dict(grant, id="permission-2")]),
            ("no expiry", [{k: v for k, v in grant.items() if k != "expirationTime"}]),
        ]:
            with self.subTest(label=label):
                result, _ = self.run_harness(
                    DELIVERED | {"permissions.json": {"permissions": permissions}},
                    phase="delivery",
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("exactly one expiring reader permission", result.stderr)

    def test_delivery_refuses_a_reader_granted_to_anybody_else(self):
        # Counting only the recipient's permissions made "exactly one reader"
        # mean "exactly one of theirs", so a second account holding the file
        # read as a correctly scoped delivery.
        grant = reader()
        stranger = reader(id="permission-2", emailAddress="stranger@example.test")
        result, _ = self.run_harness(
            DELIVERED | {"permissions.json": {"permissions": [grant, stranger]}},
            phase="delivery",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exactly one expiring reader permission", result.stderr)

    def test_delivery_refuses_a_permission_listing_it_cannot_see_the_end_of(self):
        # Exactly one reader is an exactness claim, and a truncated listing
        # cannot support one. A delivered file should never reach a page of
        # permissions, so this is a refusal rather than a paging loop.
        truncated = DELIVERED | {
            "permissions.json": {
                "nextPageToken": "page-2",
                "permissions": [reader()],
            }
        }
        result, _ = self.run_harness(truncated, phase="delivery")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("more permissions than one page", result.stderr)

    def test_delivery_accepts_the_recipient_however_drive_folded_the_address(self):
        # Drive answers with the address it normalised, not the spelling the
        # operator exported, and a case difference is not a different person.
        grant = reader(emailAddress=RECIPIENT.upper())
        result, document = self.run_harness(
            DELIVERED | {"permissions.json": {"permissions": [grant]}}, phase="delivery"
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("permission_expires_at", document)

    def test_a_truncated_template_transfer_is_refused(self):
        # `%{http_code}` is 200 as soon as the headers arrive, so a body that
        # dies part-way through still reports 200 while curl exits non-zero, and
        # the fragment that did arrive can still carry the marker the content
        # check greps for. The media preflight used to swallow that exit code.
        result, _ = self.run_harness(phase="media", template_exit="28")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("did not complete", result.stderr)

    def test_a_served_template_passes_the_media_preflight(self):
        # The other side of it, so the refusal above is not simply the media
        # phase failing for its own reasons: a complete 200 carrying the marker
        # reaches `template ok`.
        result, _ = self.run_harness(phase="media")
        self.assertIn("template ok", result.stdout)

    def test_a_phase_list_with_spaces_is_refused_rather_than_half_run(self):
        # The validation loop splits on whitespace and `runs` does not, so this
        # used to validate both phases and then run only the first.
        result, document = self.run_harness(phase="delivery, cleanup")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("comma separated with no spaces", result.stderr)
        self.assertNotIn("drive_file_id", document)

    def test_cleanup_alone_does_not_need_a_live_reader_permission(self):
        # Cleanup runs after the grant has lapsed, which is the state it exists
        # to prove. Demanding a live one would refuse to verify the deletion.
        expired = DELIVERED | {"permissions.json": {"permissions": [reader(hours=-1)]}}
        result, document = self.run_harness(
            expired, phase="cleanup", include_recipient=False
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            document["cleanup_status"],
            {"drive_file_absent": True, "gcs_object_absent": True},
        )
        self.assertNotIn("permission_expires_at", document)

    def test_cleanup_refuses_to_claim_success_while_an_artifact_remains(self):
        for statuses, expected in [
            ({"cleanup_drive_status": "200"}, "Drive=200 GCS=404"),
            ({"cleanup_gcs_status": "200"}, "Drive=404 GCS=200"),
        ]:
            with self.subTest(statuses=statuses):
                result, document = self.run_harness(cleanup_timeout="0", **statuses)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(f"cleanup timed out: {expected}", result.stderr)
                self.assertNotIn("cleanup_status", document)


if __name__ == "__main__":
    unittest.main()
