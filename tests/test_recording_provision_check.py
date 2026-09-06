#!/usr/bin/env python3
"""Contract tests for the credentialed recording provisioning checker."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "recording-provision-check.sh"
DELIVERY_EMAIL = "codetrial-recording@staging-project.iam.gserviceaccount.com"


PROJECT_NUMBER = "1234567890"


# What `gcloud storage buckets describe` returns without `--raw`: the resource
# standardised into its own field names. The checker reads the JSON API's names,
# so this shape has to make it refuse. The nested contents do not matter here;
# the absence of `iamConfiguration` and `lifecycle` is the whole point.
STANDARDISED_BUCKET = {
    # The ownership field is here so that a run without `--raw` reaches the
    # reads this fixture exists to exercise. Without it the run died earlier, on
    # a missing projectNumber, and the nine tests that "guarded" `--raw` never
    # saw the uniform-access or lifecycle messages at all.
    "projectNumber": PROJECT_NUMBER,
    "uniform_bucket_level_access": True,
    "lifecycle_config": {
        "rule": [{"action": {"type": "Delete"}, "condition": {"age": 1}}]
    },
}


PROJECT_NUMBER = "1234567890"


# What `web/recording/index.html` declares, and the only thing that tells the
# template apart from any other page a 200 could be serving.
TEMPLATE_BODY = '<div id="recording-ready" data-ready="false" hidden></div>'


INDIRECT_REFUSAL = "may or may not contain"

DELIVERY_MEMBER = f"serviceAccount:{DELIVERY_EMAIL}"
DELIVERY_BINDING = {
    "role": "roles/storage.objectAdmin",
    "members": [DELIVERY_MEMBER],
}
DELIVERY_ORGANIZER = {
    "emailAddress": DELIVERY_EMAIL,
    "type": "user",
    "role": "organizer",
}


def bucket_iam_policy(*extra):
    """The correct bucket policy, plus whatever binding a test is about."""
    return {"bindings": [DELIVERY_BINDING, *extra]}


def ancestor_policy(*bindings, label="projects/staging-project"):
    """One ancestor in the chain `get-ancestors-iam-policy` returns."""
    return {"resource": label, "policy": {"bindings": list(bindings)}}


def drive_permissions(*extra):
    """The delivery account's own organizer grant, plus the row under test."""
    return {"permissions": [DELIVERY_ORGANIZER, *extra]}


def bucket_resource(**overrides):
    """A correctly provisioned bucket, with only what a test varies replaced."""
    return {
        "projectNumber": PROJECT_NUMBER,
        "iamConfiguration": {"uniformBucketLevelAccess": {"enabled": True}},
        "lifecycle": {
            "rule": [{"action": {"type": "Delete"}, "condition": {"age": 1}}]
        },
        # What a bucket returns once soft delete has been turned off, which is
        # what "correctly provisioned" means here: the API reports the disabled
        # state as a zero duration rather than by dropping the field.
        "softDeletePolicy": {"retentionDurationSeconds": "0"},
    } | overrides


class RefusalAssertions(unittest.TestCase):
    """Shared by both cases, because both drive the script and read a refusal."""

    def assertRefused(self, result, message):
        """A refusal is a non-zero exit and a sentence naming what was wrong.

        Both halves, every time: an exit code alone would be satisfied by the
        script falling over somewhere else entirely, which is how a check that
        had stopped running would still look like a check that passed.
        """
        self.assertNotEqual(result.returncode, 0, result.stderr)
        self.assertIn(message, result.stderr)


class ProvisionCheckTests(RefusalAssertions):
    def write_executable(self, directory: Path, name: str, contents: str) -> None:
        path = directory / name
        path.write_text(contents)
        path.chmod(0o755)

    def run_check(
        self,
        *,
        bucket_iam=None,
        ancestors_iam=None,
        bucket=None,
        drive=None,
        drive_page_2=None,
        fail="",
        delivery_email=DELIVERY_EMAIL,
        auditor_email="auditor@staging-project.iam.gserviceaccount.com",
        template_origin="https://1.1.1.1",
        template_status="200",
        template_body=TEMPLATE_BODY,
        template_transfer="complete",
        token="ya29.a0AfB_byExampleToken-_~+/=",
    ):
        if bucket_iam is None:
            bucket_iam = bucket_iam_policy()
        if ancestors_iam is None:
            ancestors_iam = [ancestor_policy()]
        if bucket is None:
            bucket = bucket_resource()
        if drive is None:
            drive = drive_permissions()
        if drive_page_2 is not None:
            drive = drive | {"nextPageToken": "page-2"}
        else:
            drive_page_2 = {"permissions": []}

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
                "drive-page-2.json": drive_page_2,
            }
            for name, value in files.items():
                (temporary_path / name).write_text(json.dumps(value))
            # The fakes model three things the first version did not, because a
            # mutation run showed fifteen real defects surviving without them:
            # which key is currently active, whether a request carried its
            # credential, and that any one call can fail. Each `need_active`
            # below is an invariant the script states and nothing checked.
            self.write_executable(
                fake_bin,
                "gcloud",
                """#!/bin/sh
printf '%s\\n' "$*" >> "$FAKE_ROOT/gcloud.log"

# Which account the last activation selected. The script's stated rule is that
# policy reads use the independent auditor and access proofs use the delivery
# account, and nothing observed either half before.
active=
[ ! -f "$FAKE_ROOT/active" ] || active=$(cat "$FAKE_ROOT/active")

need_active()
{
  [ "$active" = "$1" ] || {
    echo "PERMISSION_DENIED: $2 as ${active:-no account}, expected $1" >&2
    exit 1
  }
}

fails()
{
  [ "$FAKE_FAIL" = "$1" ] || return 0
  echo "simulated failure: $1" >&2
  exit 1
}

case "$*" in
  *"auth activate-service-account"*)
      case "$*" in
        *iam-auditor.json*) printf 'auditor\\n' > "$FAKE_ROOT/active" ;;
        *delivery-service-account.json*) printf 'delivery\\n' > "$FAKE_ROOT/active" ;;
        *) echo "activated an unknown key: $*" >&2; exit 99 ;;
      esac
      ;;
  *"auth print-access-token"*) fails print-access-token; printf '%s\\n' "$FAKE_TOKEN" ;;
  *"storage buckets get-iam-policy"*)
      need_active auditor "read the bucket IAM policy"; fails bucket-iam
      cat "$FAKE_ROOT/bucket-iam.json" ;;
  *"projects describe"*)
      need_active auditor "describe the project"; fails project-describe
      printf '%s\\n' "$FAKE_PROJECT_NUMBER" ;;
  *"projects get-ancestors-iam-policy"*)
      need_active auditor "read ancestor IAM"; fails ancestors-iam
      cat "$FAKE_ROOT/ancestors-iam.json" ;;
  *"storage buckets describe"*--raw* | *--raw*"storage buckets describe"*)
      need_active auditor "describe the bucket"; fails bucket-describe
      cat "$FAKE_ROOT/bucket.json" ;;
  *"storage buckets describe"*)
      need_active auditor "describe the bucket"
      cat "$FAKE_ROOT/bucket-standardised.json" ;;
  *"storage ls"*)
      need_active delivery "list the bucket"; fails storage-ls ;;
  *) echo "unexpected gcloud invocation: $*" >&2; exit 99 ;;
esac
""",
            )
            self.write_executable(
                fake_bin,
                "curl",
                """#!/bin/sh
printf '%s\\n' "$*" >> "$FAKE_ROOT/curl.log"

# --output is read out of the arguments rather than assumed, because the
# template probe writes the body to a file and puts only the status on stdout.
out=/dev/null
previous=
credentialed=no
for argument in "$@"; do
  [ "$previous" != --output ] || out=$argument
  [ "$argument" != --config ] || credentialed=yes
  previous=$argument
done

# A Google API call with no credential is a 401, not a silent success. Without
# this the audit could be made to run unauthenticated and every test still
# passed.
unauthenticated()
{
  [ "$credentialed" = no ] || return 0
  printf '%s' '{"error":{"code":401,"message":"Login Required"}}'
  exit 22
}

fails()
{
  [ "$FAKE_FAIL" = "$1" ] || return 0
  echo "simulated failure: $1" >&2
  exit 22
}

case "$*" in
  *"/permissions?"*pageToken=*) unauthenticated; fails drive-permissions; cat "$FAKE_ROOT/drive-page-2.json" ;;
  *"/permissions?"*) unauthenticated; fails drive-permissions; cat "$FAKE_ROOT/drive.json" ;;
  *"/drive/v3/files?"*) unauthenticated; fails drive-files; printf '{"files": []}\\n' ;;
  *"/recording/index.html"*)
      printf '%s\\n' "$*" >> "$FAKE_ROOT/curl-args.log"
      printf '%s' "$FAKE_TEMPLATE_BODY" > "$out"
      printf '%s' "$FAKE_TEMPLATE_STATUS"
      [ "$FAKE_TEMPLATE_TRANSFER" = complete ] || exit 28
      ;;
  *) echo "unexpected curl invocation: $*" >&2; exit 99 ;;
esac
""",
            )
            environment = os.environ | {
                "PATH": f"{fake_bin}:{os.environ['PATH']}",
                "FAKE_ROOT": str(temporary_path),
                "FAKE_FAIL": fail,
                "FAKE_PROJECT_NUMBER": PROJECT_NUMBER,
                "FAKE_TEMPLATE_STATUS": template_status,
                "FAKE_TEMPLATE_BODY": template_body,
                "FAKE_TEMPLATE_TRANSFER": template_transfer,
                "FAKE_TOKEN": token,
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
            result = subprocess.run(
                ["sh", SCRIPT],
                env=environment,
                text=True,
                capture_output=True,
                check=False,
            )

            # Read before the directory goes away. The template fetch's own
            # arguments are the only way to observe that it was pinned to the
            # address the resolver approved rather than resolving again.
            def readback(name):
                path = temporary_path / name
                return path.read_text() if path.exists() else ""

            result.template_curl = readback("curl-args.log")
            result.gcloud_log = readback("gcloud.log")
            result.curl_log = readback("curl.log")
            return result

    def test_exact_least_privilege_contract_passes(self):
        result = self.run_check()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("least-privilege", result.stdout)
        self.assertIn("Recording template is publicly reachable", result.stdout)

    def test_every_cloud_call_that_fails_stops_the_run(self):
        # A mutation run found that appending `|| true` to any one of these left
        # the whole suite green. A failed read is a failed check, which is the
        # rule the script states at its top and nothing observed.
        #
        # Three of them stay green under `|| true` even now, and that is not a
        # weakness in this test: the three JSON reads have a second gate behind
        # them, so a swallowed gcloud failure leaves an empty file and the run
        # dies on `unreadable audit response` instead. Measured, not assumed.
        # What this test holds is the property that matters, which is that no
        # single cloud failure lets the run reach its verdict.
        for failure in [
            "bucket-iam",
            "ancestors-iam",
            "bucket-describe",
            "project-describe",
            "print-access-token",
            "storage-ls",
            "drive-permissions",
            "drive-files",
        ]:
            with self.subTest(failure=failure):
                result = self.run_check(fail=failure)
                self.assertNotEqual(
                    result.returncode, 0, f"{failure} failed and the run still passed"
                )
                # The last line the script prints. Some of these failures come
                # after the least-privilege verdict, so that line is not the
                # thing that separates a stopped run from a finished one.
                self.assertNotIn("publicly reachable", result.stdout)

    def test_policy_reads_use_the_auditor_and_access_proofs_use_delivery(self):
        # The script's own stated invariant: "Policy reads must use the
        # independent auditor." Activating the wrong key, or dropping either
        # activation, used to pass every test.
        result = self.run_check()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = [line for line in result.gcloud_log.splitlines() if line.strip()]
        order = [
            index
            for index, line in enumerate(calls)
            if "activate-service-account" in line or "storage ls" in line
        ]
        self.assertTrue(order, "no activation was recorded")
        # Both accounts are activated, the auditor first, and the delivery
        # account's own bucket access is proved after its activation.
        self.assertIn("iam-auditor.json", calls[order[0]])
        self.assertTrue(
            any("delivery-service-account.json" in calls[index] for index in order),
            "the delivery account was never activated",
        )
        self.assertTrue(
            any("storage ls" in line for line in calls),
            "the delivery account's bucket access was never proved",
        )

    def test_every_drive_request_carries_a_credential(self):
        # Dropping `--config` from either Drive call made the audit run
        # unauthenticated, and the fake used to answer anyway.
        result = self.run_check()
        self.assertEqual(result.returncode, 0, result.stderr)
        drive_calls = [
            line
            for line in result.curl_log.splitlines()
            if "googleapis.com/drive" in line
        ]
        self.assertGreaterEqual(len(drive_calls), 2, result.curl_log)
        for call in drive_calls:
            self.assertIn("--config", call)

    def test_a_truncated_template_transfer_is_refused(self):
        # `%{http_code}` is 200 as soon as the headers arrive, so a body that
        # dies part-way through still reports 200 while curl exits non-zero.
        # Measured: 258 of 5258 bytes, status 200, exit 28, and the fragment
        # still carried the marker the content check looks for.
        result = self.run_check(template_transfer="truncated")
        self.assertRefused(result, "did not complete")

    def test_a_token_that_is_not_a_bare_token_is_refused(self):
        # The bearer goes into a curl config file to keep it out of `ps`, and
        # that format has escaping of its own: a newline in the value turns the
        # remainder into further curl options.
        # The newline is the case worth keeping: a quote merely truncates the
        # header, while everything after a newline is parsed as further curl
        # options. Both fall through the same glob at the same call site.
        result = self.run_check(token="ya29.abc\noutput = /tmp/stolen")
        self.assertEqual(result.returncode, 2)
        self.assertIn("not a bare token", result.stderr)

    def test_missing_bucket_role_is_refused(self):
        result = self.run_check(bucket_iam={"bindings": []})
        self.assertRefused(result, "expected exactly")

    def test_extra_bucket_role_is_refused(self):
        result = self.run_check(
            bucket_iam={
                "bindings": [
                    {
                        "role": "roles/storage.objectAdmin",
                        "members": [f"serviceAccount:{DELIVERY_EMAIL}"],
                    },
                    {
                        "role": "roles/storage.admin",
                        "members": [f"serviceAccount:{DELIVERY_EMAIL}"],
                    },
                ]
            }
        )
        self.assertRefused(result, "bucket roles")

    def test_public_bucket_bindings_are_refused(self):
        for principal in ["allUsers", "allAuthenticatedUsers"]:
            with self.subTest(principal=principal):
                result = self.run_check(
                    bucket_iam={
                        "bindings": [
                            {
                                "role": "roles/storage.objectAdmin",
                                "members": [f"serviceAccount:{DELIVERY_EMAIL}"],
                            },
                            {"role": "roles/storage.admin", "members": [principal]},
                        ]
                    }
                )
                self.assertRefused(result, INDIRECT_REFUSAL)

    def test_empty_ancestor_response_is_refused(self):
        # The one shape that could pass by checking nothing: the role loop runs
        # zero times and reports no forbidden grants, which reads exactly like a
        # clean audit.
        for empty in [[], {}]:
            with self.subTest(empty=empty):
                result = self.run_check(ancestors_iam=empty)
                self.assertRefused(result, "non-empty policy list")

    def test_conditional_binding_is_refused(self):
        # A condition can grant the role only inside a time window or a resource
        # prefix, so the binding no longer says what access the account has.
        result = self.run_check(
            bucket_iam={
                "bindings": [
                    {
                        "role": "roles/storage.objectAdmin",
                        "members": [f"serviceAccount:{DELIVERY_EMAIL}"],
                        "condition": {
                            "title": "window",
                            "expression": "request.time < timestamp('2027-01-01T00:00:00Z')",
                        },
                    },
                ]
            }
        )
        self.assertRefused(result, "conditional IAM binding")

    def test_group_ancestor_binding_is_refused(self):
        result = self.run_check(
            ancestors_iam=[
                {
                    "resource": "projects/staging-project",
                    "policy": {
                        "bindings": [
                            {
                                "role": "roles/storage.admin",
                                "members": ["group:delivery@example.test"],
                            },
                        ]
                    },
                }
            ]
        )
        self.assertRefused(result, "group:delivery@example.test")
        self.assertIn("resolve the membership", result.stderr)

    def test_project_wide_role_is_refused(self):
        result = self.run_check(
            ancestors_iam=[
                {
                    "resource": "projects/staging-project",
                    "policy": {
                        "bindings": [
                            {
                                "role": "roles/viewer",
                                "members": [f"serviceAccount:{DELIVERY_EMAIL}"],
                            },
                        ]
                    },
                }
            ]
        )
        self.assertRefused(result, "forbidden ancestor IAM roles")

    def test_inherited_organization_role_is_refused(self):
        result = self.run_check(
            ancestors_iam=[
                {"resource": "projects/staging-project", "policy": {"bindings": []}},
                {
                    "resource": "organizations/123",
                    "policy": {
                        "bindings": [
                            {
                                "role": "roles/storage.admin",
                                "members": [f"serviceAccount:{DELIVERY_EMAIL}"],
                            },
                        ]
                    },
                },
            ]
        )
        self.assertRefused(result, "organizations/123")

    def test_ancestor_role_is_refused_whichever_label_gcloud_uses(self):
        # gcloud names the ancestor `id`; the resource-manager APIs name it
        # `resource`. The grant is what the audit refuses, not the spelling.
        for label in ["id", "resource"]:
            with self.subTest(label=label):
                result = self.run_check(
                    ancestors_iam=[
                        {
                            label: "organizations/123",
                            "policy": {
                                "bindings": [
                                    {
                                        "role": "roles/storage.admin",
                                        "members": [f"serviceAccount:{DELIVERY_EMAIL}"],
                                    },
                                ]
                            },
                        },
                    ]
                )
                self.assertRefused(result, "organizations/123")

    def test_ancestor_grant_is_refused_with_no_label_at_all(self):
        result = self.run_check(
            ancestors_iam=[
                {
                    "policy": {
                        "bindings": [
                            {
                                "role": "roles/storage.admin",
                                "members": [f"serviceAccount:{DELIVERY_EMAIL}"],
                            },
                        ]
                    }
                }
            ]
        )
        self.assertRefused(result, "forbidden ancestor IAM roles")

    def test_missing_24_hour_lifecycle_is_refused(self):
        result = self.run_check(bucket=bucket_resource(lifecycle={"rule": []}))
        self.assertRefused(result, "24-hour Delete lifecycle")

    def test_bucket_owned_by_another_project_is_refused(self):
        # The ancestor audit walked this project. A bucket owned elsewhere
        # inherits grants from a chain nothing here looked at.
        result = self.run_check(bucket=bucket_resource(projectNumber="9999999999"))
        self.assertRefused(result, "belongs to project number")

    def test_filtered_lifecycle_rule_is_refused(self):
        # Each of these deletes something at a day old while leaving other
        # objects behind, and nothing here says which prefix the recordings use.
        for extra in [
            {"matchesPrefix": ["other/"]},
            {"matchesStorageClass": ["NEARLINE"]},
            {"numNewerVersions": 2},
        ]:
            with self.subTest(extra=extra):
                result = self.run_check(
                    bucket=bucket_resource(
                        lifecycle={
                            "rule": [
                                {
                                    "action": {"type": "Delete"},
                                    "condition": {"age": 1, **extra},
                                }
                            ]
                        }
                    )
                )
                self.assertRefused(result, "unconditional 24-hour Delete lifecycle")

    def test_soft_deleted_recordings_are_refused(self):
        # The default on a new bucket. A lifecycle Delete moves the object into
        # the soft-deleted state, so every check above passes while the
        # recording stays restorable for the whole retention window.
        result = self.run_check(
            bucket=bucket_resource(
                softDeletePolicy={"retentionDurationSeconds": "604800"}
            )
        )
        self.assertRefused(result, "soft-deleted")
        self.assertIn("604800", result.stderr)

    def test_soft_delete_reported_as_off_is_accepted(self):
        # Both renderings of zero, because the JSON API types the duration as a
        # number and gcloud has been seen to emit it either way.
        for zero in ["0", 0]:
            with self.subTest(zero=zero):
                result = self.run_check(
                    bucket=bucket_resource(
                        softDeletePolicy={"retentionDurationSeconds": zero}
                    )
                )
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_an_unreported_soft_delete_state_is_refused(self):
        # Absence is not proof. Whether disabling the policy omits the field or
        # reports a zero cannot be settled from outside the API, and resolving
        # that in the passing direction would let the audit report soft delete
        # off without ever having seen it off.
        bucket = bucket_resource()
        del bucket["softDeletePolicy"]
        result = self.run_check(bucket=bucket)
        self.assertRefused(result, "could not determine")

    def test_deferred_deletion_mechanisms_are_refused(self):
        # Three ways to defer the Delete action rather than forbid it, which
        # comes to the same thing: the object stays until the deferral ends and
        # none of these ends within a day.
        for override, expected in [
            (
                {"retentionPolicy": {"retentionPeriod": "2592000", "isLocked": True}},
                "a retention policy",
            ),
            ({"objectRetention": {"mode": "Enabled"}}, "object retention"),
            ({"defaultEventBasedHold": True}, "a default event-based hold"),
        ]:
            with self.subTest(override=override):
                result = self.run_check(bucket=bucket_resource(**override))
                self.assertRefused(result, expected)
                self.assertIn("defers the lifecycle", result.stderr)

    def test_an_unlocked_disabled_object_retention_does_not_refuse(self):
        # `objectRetention` is reported on buckets that have it available and
        # unused, so refusing the key's presence rather than its mode would fail
        # a bucket that is configured correctly.
        result = self.run_check(bucket=bucket_resource(objectRetention={}))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_object_versioning_is_refused(self):
        result = self.run_check(bucket=bucket_resource(versioning={"enabled": True}))
        self.assertRefused(result, "Object Versioning")

    def test_bucket_without_uniform_access_is_refused(self):
        result = self.run_check(
            bucket=bucket_resource(
                iamConfiguration={"uniformBucketLevelAccess": {"enabled": False}}
            )
        )
        self.assertRefused(result, "Uniform Bucket-Level Access")

    def test_a_second_page_of_drive_permissions_is_read(self):
        # A shared drive caps this listing at 100 per page. A second grant for
        # the delivery account landing on page two used to leave the count at
        # one, and the audit reported an exactness it had never seen.
        result = self.run_check(
            drive_page_2={
                "permissions": [
                    {"emailAddress": DELIVERY_EMAIL, "type": "user", "role": "writer"},
                ]
            }
        )
        self.assertRefused(result, "exactly one active Shared Drive organizer")

    def test_a_sole_grant_on_the_second_page_is_still_found(self):
        result = self.run_check(
            drive={"permissions": []},
            drive_page_2={
                "permissions": [
                    {
                        "emailAddress": DELIVERY_EMAIL,
                        "type": "user",
                        "role": "organizer",
                    },
                ]
            },
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_every_non_user_drive_member_is_refused(self):
        # One case per type, not per role: the check reads `type` and the role
        # only reaches the message, so looping roles ran the whole script three
        # times over one branch. The delivery account holds its single organizer
        # grant in each, so every assertion the audit used to make is satisfied
        # and only the extra row decides the outcome.
        for permission, named in [
            ({"type": "anyone", "role": "reader"}, "anyone"),
            (
                {"type": "domain", "domain": "example.test", "role": "reader"},
                "example.test",
            ),
            (
                {
                    "type": "group",
                    "emailAddress": "eng@example.test",
                    "role": "writer",
                },
                "eng@example.test",
            ),
        ]:
            with self.subTest(permission=permission):
                result = self.run_check(drive=drive_permissions(permission))
                self.assertRefused(result, "non-user members")
                self.assertIn(named, result.stderr)

    def test_a_deleted_public_permission_does_not_refuse(self):
        # `deleted` rows grant nothing, and refusing over one would fail a drive
        # that had already been corrected.
        result = self.run_check(
            drive=drive_permissions(
                {"type": "anyone", "role": "reader", "deleted": True}
            )
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_other_human_members_do_not_refuse(self):
        # A Shared Drive has people on it. The audit is about the delivery
        # account's grant and about grants nobody can enumerate, not about
        # whether a reviewer has access.
        result = self.run_check(
            drive=drive_permissions(
                {
                    "emailAddress": "reviewer@example.test",
                    "type": "user",
                    "role": "writer",
                }
            )
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_non_organizer_drive_membership_is_refused(self):
        result = self.run_check(
            drive={
                "permissions": [
                    {"emailAddress": DELIVERY_EMAIL, "type": "user", "role": "writer"},
                ]
            }
        )
        self.assertRefused(result, "Shared Drive organizer")

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

    def test_gcs_convenience_members_are_refused(self):
        # Cloud Storage writes these into a bucket policy and they survive
        # uniform bucket-level access. Each expands to everyone holding that
        # basic role on the project, so the delivery account can hold exactly
        # one role while the bucket is open to every project editor. The
        # exactness check below cannot see it: it only reads bindings that name
        # the delivery account.
        #
        # All three in one binding rather than three runs of the script: the
        # refusal names every principal it matched, so a spelling that stopped
        # being caught drops out of the message and fails the assertion.
        members = [
            "projectOwner:staging-project",
            "projectEditor:staging-project",
            "projectViewer:staging-project",
        ]
        result = self.run_check(
            bucket_iam=bucket_iam_policy(
                {"role": "roles/storage.admin", "members": members}
            )
        )
        self.assertRefused(result, INDIRECT_REFUSAL)
        for member in members:
            self.assertIn(member, result.stderr)

    def test_an_unrecognised_member_spelling_is_refused(self):
        # The property the allowlist buys: a spelling nobody enumerated is
        # refused rather than filed as "not the delivery account". Under the
        # denylist this replaced, each of these passed silently while holding a
        # role on the bucket.
        for principal in [
            "principal://iam.googleapis.com/locations/global/workforcePools/p/subject/s",
            "principalHierarchy://iam.googleapis.com/locations/global/pools/p/ou/1",
            "federatedIdentity:some-new-google-spelling",
            "serviceAgent:service-1@gcp-sa-example.iam.gserviceaccount.com",
        ]:
            with self.subTest(principal=principal):
                result = self.run_check(
                    bucket_iam=bucket_iam_policy(
                        {"role": "roles/storage.admin", "members": [principal]}
                    )
                )
                self.assertRefused(result, INDIRECT_REFUSAL)

    def test_a_named_account_is_resolvable_whether_or_not_it_is_deleted(self):
        # The other side of the allowlist: it must not refuse the spellings a
        # real policy is full of, or the check refuses every project and gets
        # loosened back to where it started.
        for principal in [
            "user:reviewer@example.test",
            "serviceAccount:other@staging-project.iam.gserviceaccount.com",
            "deleted:user:gone@example.test?uid=123",
            "deleted:serviceAccount:gone@staging-project.iam.gserviceaccount.com?uid=1",
        ]:
            with self.subTest(principal=principal):
                result = self.run_check(
                    ancestors_iam=[
                        ancestor_policy(
                            {"role": "roles/viewer", "members": [principal]}
                        )
                    ]
                )
                # Resolvable, so the audit proceeds; it holds no role for the
                # delivery account, so the run passes.
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_a_tombstoned_group_binding_is_refused(self):
        # A deleted group is still a set nobody can enumerate, and the live
        # spelling does not cover this one.
        result = self.run_check(
            bucket_iam=bucket_iam_policy(
                {
                    "role": "roles/storage.admin",
                    "members": ["deleted:group:eng@example.test?uid=123"],
                }
            )
        )
        self.assertRefused(result, INDIRECT_REFUSAL)

    def test_principal_set_binding_is_still_refused(self):
        # Held because the prefix list is now a named tuple rather than three
        # inline strings, and a rewrite that drops one would otherwise be
        # invisible: this is the spelling whose scheme slashes follow the colon.
        result = self.run_check(
            bucket_iam=bucket_iam_policy(
                {
                    "role": "roles/storage.admin",
                    "members": [
                        "principalSet://iam.googleapis.com/projects/1/"
                        "locations/global/workloadIdentityPools/p/*"
                    ],
                }
            )
        )
        self.assertRefused(result, INDIRECT_REFUSAL)

    def test_redirecting_template_origin_is_refused(self):
        # `curl --fail` exits 0 on a 3xx when nothing follows the redirect, so
        # this passed while the template was never fetched. Measured: a 302
        # against `curl --fail --silent --show-error` exits 0.
        result = self.run_check(template_status="302", template_body="")
        self.assertRefused(result, "is not being served")
        self.assertIn("302", result.stderr)

    def test_missing_template_is_refused(self):
        result = self.run_check(template_status="404", template_body="")
        self.assertRefused(result, "is not being served")

    def test_a_200_that_is_not_the_recording_template_is_refused(self):
        # An origin serving its own landing page at this path answers 200, and
        # a status check alone would call that the recording template.
        result = self.run_check(template_body="<html><body>hello</body></html>")
        self.assertRefused(result, "is not the recording template")

    def test_the_template_fetch_is_pinned_to_the_verified_address(self):
        # The resolver approves one answer and curl would otherwise resolve
        # again, so a short-TTL record could pass the check and then send the
        # fetch elsewhere. The pin is what makes the guard mean what it says.
        result = self.run_check()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--resolve 1.1.1.1:443:1.1.1.1", result.template_curl)

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


class TemplateAddressGateTests(RefusalAssertions):
    """The resolver gate, run as the script runs it, with DNS stubbed.

    Its own test case because the address families cannot be reached through
    the script's front door: the URL check refuses the brackets an IPv6 literal
    origin needs, and a hostname that resolves to one would make the suite
    depend on the network. So the block is lifted out of the script and driven
    directly, which is the shipped code rather than a copy of it. If the
    heredoc is renamed or moved this fails loudly, which is the intended
    behaviour.
    """

    OPENS = "python3 - \"$template_name\" << 'PY'\n"
    CLOSES = "\nPY\n"

    def gate_source(self):
        script = SCRIPT.read_text()
        start = script.index(self.OPENS) + len(self.OPENS)
        return script[start : script.index(self.CLOSES, start)]

    def resolve(self, answers):
        stub = (
            "import socket\n"
            f"_answers = {answers!r}\n"
            "socket.getaddrinfo = lambda *args, **kwargs: _answers\n"
        )
        return subprocess.run(
            [sys.executable, "-c", stub + self.gate_source(), "template.example"],
            text=True,
            capture_output=True,
            check=False,
        )

    @staticmethod
    def answer(address):
        # The family as a plain int, not the `socket.AF_*` enum: the stub is
        # built by repr into the child's source, and an enum's repr is not
        # valid Python. Only the sockaddr at index 4 is read anyway.
        if ":" in address:
            return (10, 0, 0, "", (address, 0, 0, 0))
        return (2, 0, 0, "", (address, 0))

    def test_a_public_address_is_pinned_in_the_spelling_curl_takes(self):
        # IPv6 has to be bracketed in a `--resolve` entry, so an AAAA-only
        # origin used to fail a check it should pass.
        for address, pinned in [
            ("1.1.1.1", "1.1.1.1"),
            ("2606:4700:4700::1111", "[2606:4700:4700::1111]"),
        ]:
            with self.subTest(address=address):
                result = self.resolve([self.answer(address)])
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), pinned)

    def test_a_dual_stack_host_pins_one_address_and_always_the_same_one(self):
        # Two families cannot be compared with each other, so sorting the raw
        # list would raise here rather than answer. IPv4 first, because it is
        # the one every network can reach.
        answers = [self.answer("2606:4700:4700::1111"), self.answer("1.1.1.1")]
        first = self.resolve(answers)
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(first.stdout.strip(), "1.1.1.1")
        self.assertEqual(self.resolve(answers[::-1]).stdout.strip(), "1.1.1.1")

    def test_a_private_address_in_either_family_is_refused(self):
        for address in ["10.0.0.1", "127.0.0.1", "169.254.169.254", "fd00::1", "::1"]:
            with self.subTest(address=address):
                result = self.resolve([self.answer(address)])
                self.assertRefused(result, "only to public addresses")

    def test_one_private_answer_among_public_ones_is_refused(self):
        # The whole answer has to be public: a round-robin record carrying one
        # internal address is the shape this gate exists for.
        result = self.resolve([self.answer("1.1.1.1"), self.answer("127.0.0.1")])
        self.assertRefused(result, "only to public addresses")


if __name__ == "__main__":
    unittest.main()
