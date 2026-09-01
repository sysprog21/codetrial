#!/usr/bin/env python3
"""Contract tests for the credentialed recording provisioning checker."""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "recording-provision-check.sh"
DELIVERY_EMAIL = "codetrial-recording@staging-project.iam.gserviceaccount.com"


# What `gcloud storage buckets describe` returns without `--raw`: the resource
# standardised into its own field names. The checker reads the JSON API's names,
# so this shape has to make it refuse. The nested contents do not matter here;
# the absence of `iamConfiguration` and `lifecycle` is the whole point.
STANDARDISED_BUCKET = {
    "uniform_bucket_level_access": True,
    "lifecycle_config": {"rule": [{"action": {"type": "Delete"}, "condition": {"age": 1}}]},
}


PROJECT_NUMBER = "1234567890"


def bucket_resource(**overrides):
    """A correctly provisioned bucket, with only what a test varies replaced."""
    return {
        "projectNumber": PROJECT_NUMBER,
        "iamConfiguration": {"uniformBucketLevelAccess": {"enabled": True}},
        "lifecycle": {"rule": [{"action": {"type": "Delete"}, "condition": {"age": 1}}]},
    } | overrides


class ProvisionCheckTests(unittest.TestCase):
    def write_executable(self, directory: Path, name: str, contents: str) -> None:
        path = directory / name
        path.write_text(contents)
        path.chmod(0o755)

    def run_check(self, *, bucket_iam=None, ancestors_iam=None, bucket=None, drive=None, fail="", delivery_email=DELIVERY_EMAIL, auditor_email="auditor@staging-project.iam.gserviceaccount.com", template_origin="https://1.1.1.1"):
        if bucket_iam is None:
            bucket_iam = {
                "bindings": [{"role": "roles/storage.objectAdmin", "members": [f"serviceAccount:{DELIVERY_EMAIL}"]}]
            }
        if ancestors_iam is None:
            ancestors_iam = [{"resource": "projects/staging-project", "policy": {"bindings": []}}]
        if bucket is None:
            bucket = bucket_resource()
        if drive is None:
            drive = {"permissions": [{"emailAddress": DELIVERY_EMAIL, "type": "user", "role": "organizer"}]}

        with tempfile.TemporaryDirectory() as temporary:
            temporary_path = Path(temporary)
            fake_bin = temporary_path / "bin"
            fake_bin.mkdir()
            files = {
                "bucket-standardised.json": STANDARDISED_BUCKET,
                "bucket-iam.json": bucket_iam,
                "ancestors-iam.json": ancestors_iam,
                "bucket.json": bucket,
                "drive.json": drive,
            }
            for name, value in files.items():
                (temporary_path / name).write_text(json.dumps(value))
            self.write_executable(
                fake_bin,
                "gcloud",
                """#!/bin/sh
case "$*" in
  *"auth activate-service-account"*) exit 0 ;;
  *"auth print-access-token"*) printf 'token\\n' ;;
  *"storage buckets get-iam-policy"*) cat "$FAKE_ROOT/bucket-iam.json" ;;
  *"projects describe"*) printf '%s\\n' "$FAKE_PROJECT_NUMBER" ;;
  *"projects get-ancestors-iam-policy"*) [ "$FAKE_FAIL" != ancestors-iam ] || exit 1; cat "$FAKE_ROOT/ancestors-iam.json" ;;
  *"storage buckets describe"*--raw* | *--raw*"storage buckets describe"*) cat "$FAKE_ROOT/bucket.json" ;;
  *"storage buckets describe"*) cat "$FAKE_ROOT/bucket-standardised.json" ;;
  *"storage ls"*) ;;
  *) echo "unexpected gcloud invocation: $*" >&2; exit 99 ;;
esac
""",
            )
            self.write_executable(
                fake_bin,
                "curl",
                """#!/bin/sh
case "$*" in
  *"/permissions?"*) cat "$FAKE_ROOT/drive.json" ;;
  *"/drive/v3/files?"*) printf '{"files": []}\\n' ;;
  *"/recording/index.html"*) ;;
  *) echo "unexpected curl invocation: $*" >&2; exit 99 ;;
esac
""",
            )
            environment = os.environ | {
                "PATH": f"{fake_bin}:{os.environ['PATH']}",
                "FAKE_ROOT": str(temporary_path),
                "FAKE_FAIL": fail,
                "FAKE_PROJECT_NUMBER": PROJECT_NUMBER,
                "CODETRIAL_RECORDING_PROJECT": "staging-project",
                "CODETRIAL_RECORDING_GCS_BUCKET": "staging-recordings",
                "CODETRIAL_RECORDING_DRIVE_ID": "drive_123",
                "CODETRIAL_RECORDING_TEMPLATE_BASE_URL": template_origin,
                "CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON": json.dumps(
                    {"project_id": "staging-project", "client_email": delivery_email}
                ),
                "CODETRIAL_RECORDING_IAM_AUDITOR_JSON": json.dumps(
                    {"project_id": "staging-project", "client_email": auditor_email}
                ),
            }
            return subprocess.run(["sh", SCRIPT], env=environment, text=True, capture_output=True, check=False)

    def test_exact_least_privilege_contract_passes(self):
        result = self.run_check()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("least-privilege", result.stdout)
        self.assertIn("Recording template is publicly reachable", result.stdout)

    def test_missing_bucket_role_is_refused(self):
        result = self.run_check(bucket_iam={"bindings": []})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected exactly", result.stderr)

    def test_extra_bucket_role_is_refused(self):
        result = self.run_check(bucket_iam={"bindings": [
            {"role": "roles/storage.objectAdmin", "members": [f"serviceAccount:{DELIVERY_EMAIL}"]},
            {"role": "roles/storage.admin", "members": [f"serviceAccount:{DELIVERY_EMAIL}"]},
        ]})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("bucket roles", result.stderr)

    def test_public_bucket_bindings_are_refused(self):
        for principal in ["allUsers", "allAuthenticatedUsers"]:
            with self.subTest(principal=principal):
                result = self.run_check(bucket_iam={"bindings": [
                    {"role": "roles/storage.objectAdmin", "members": [f"serviceAccount:{DELIVERY_EMAIL}"]},
                    {"role": "roles/storage.admin", "members": [principal]},
                ]})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("public or indirect", result.stderr)

    def test_empty_ancestor_response_is_refused(self):
        # The one shape that could pass by checking nothing: the role loop runs
        # zero times and reports no forbidden grants, which reads exactly like a
        # clean audit.
        for empty in [[], {}]:
            with self.subTest(empty=empty):
                result = self.run_check(ancestors_iam=empty)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("non-empty policy list", result.stderr)

    def test_conditional_binding_is_refused(self):
        # A condition can grant the role only inside a time window or a resource
        # prefix, so the binding no longer says what access the account has.
        result = self.run_check(bucket_iam={"bindings": [
            {
                "role": "roles/storage.objectAdmin",
                "members": [f"serviceAccount:{DELIVERY_EMAIL}"],
                "condition": {"title": "window", "expression": "request.time < timestamp('2027-01-01T00:00:00Z')"},
            },
        ]})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("conditional IAM binding", result.stderr)

    def test_group_ancestor_binding_is_refused(self):
        result = self.run_check(ancestors_iam=[{"resource": "projects/staging-project", "policy": {"bindings": [
            {"role": "roles/storage.admin", "members": ["group:delivery@example.test"]},
        ]}}])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("public or indirect", result.stderr)

    def test_project_wide_role_is_refused(self):
        result = self.run_check(ancestors_iam=[{"resource": "projects/staging-project", "policy": {"bindings": [
            {"role": "roles/viewer", "members": [f"serviceAccount:{DELIVERY_EMAIL}"]},
        ]}}])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("forbidden ancestor IAM roles", result.stderr)

    def test_inherited_organization_role_is_refused(self):
        result = self.run_check(ancestors_iam=[
            {"resource": "projects/staging-project", "policy": {"bindings": []}},
            {"resource": "organizations/123", "policy": {"bindings": [
                {"role": "roles/storage.admin", "members": [f"serviceAccount:{DELIVERY_EMAIL}"]},
            ]}},
        ])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("organizations/123", result.stderr)

    def test_ancestor_role_is_refused_whichever_label_gcloud_uses(self):
        # gcloud names the ancestor `id`; the resource-manager APIs name it
        # `resource`. The grant is what the audit refuses, not the spelling.
        for label in ["id", "resource"]:
            with self.subTest(label=label):
                result = self.run_check(ancestors_iam=[
                    {label: "organizations/123", "policy": {"bindings": [
                        {"role": "roles/storage.admin", "members": [f"serviceAccount:{DELIVERY_EMAIL}"]},
                    ]}},
                ])
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("organizations/123", result.stderr)

    def test_ancestor_grant_is_refused_with_no_label_at_all(self):
        result = self.run_check(ancestors_iam=[{"policy": {"bindings": [
            {"role": "roles/storage.admin", "members": [f"serviceAccount:{DELIVERY_EMAIL}"]},
        ]}}])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("forbidden ancestor IAM roles", result.stderr)

    def test_missing_24_hour_lifecycle_is_refused(self):
        result = self.run_check(bucket=bucket_resource(lifecycle={"rule": []}))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("24-hour Delete lifecycle", result.stderr)

    def test_bucket_owned_by_another_project_is_refused(self):
        # The ancestor audit walked this project. A bucket owned elsewhere
        # inherits grants from a chain nothing here looked at.
        result = self.run_check(bucket=bucket_resource(projectNumber="9999999999"))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("belongs to project number", result.stderr)

    def test_filtered_lifecycle_rule_is_refused(self):
        # Each of these deletes something at a day old while leaving other
        # objects behind, and nothing here says which prefix the recordings use.
        for extra in [{"matchesPrefix": ["other/"]}, {"matchesStorageClass": ["NEARLINE"]},
                      {"numNewerVersions": 2}]:
            with self.subTest(extra=extra):
                result = self.run_check(bucket=bucket_resource(
                    lifecycle={"rule": [{"action": {"type": "Delete"},
                                         "condition": {"age": 1, **extra}}]}))
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("unconditional 24-hour Delete lifecycle", result.stderr)

    def test_bucket_without_uniform_access_is_refused(self):
        result = self.run_check(bucket=bucket_resource(
            iamConfiguration={"uniformBucketLevelAccess": {"enabled": False}}))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Uniform Bucket-Level Access", result.stderr)

    def test_non_organizer_drive_membership_is_refused(self):
        result = self.run_check(drive={"permissions": [
            {"emailAddress": DELIVERY_EMAIL, "type": "user", "role": "writer"},
        ]})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Shared Drive organizer", result.stderr)

    def test_unobservable_ancestor_iam_is_refused(self):
        result = self.run_check(fail="ancestors-iam")
        self.assertNotEqual(result.returncode, 0)

    def test_delivery_account_cannot_audit_itself(self):
        result = self.run_check(auditor_email=DELIVERY_EMAIL)
        self.assertEqual(result.returncode, 2)
        self.assertIn("different account", result.stderr)

    def test_non_dedicated_delivery_account_is_refused(self):
        result = self.run_check(
            delivery_email="recording-delivery@staging-project.iam.gserviceaccount.com"
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("delivery service account must be", result.stderr)

    def test_template_origin_that_resolves_to_loopback_is_refused(self):
        # This alternate numeric spelling resolves to 127.0.0.1 but bypasses a
        # textual `127.*` check, so it proves the resolver check is active.
        result = self.run_check(template_origin="https://2130706433")
        self.assertEqual(result.returncode, 1)
        self.assertIn("must resolve only to public addresses", result.stderr)

    def test_template_origin_named_localhost_is_refused(self):
        result = self.run_check(template_origin="https://localhost")
        self.assertEqual(result.returncode, 1)
        self.assertIn("must resolve only to public addresses", result.stderr)

    def test_template_origin_that_resolves_to_private_address_is_refused(self):
        result = self.run_check(template_origin="https://10.0.0.1")
        self.assertEqual(result.returncode, 1)
        self.assertIn("must resolve only to public addresses", result.stderr)


if __name__ == "__main__":
    unittest.main()
