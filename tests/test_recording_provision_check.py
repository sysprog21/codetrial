#!/usr/bin/env python3
"""Contract tests for the credentialed recording provisioning checker."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
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


# What `web/recording/index.html` declares, and the only thing that tells the
# template apart from any other page a 200 could be serving.
TEMPLATE_BODY = '<div id="recording-ready" data-ready="false" hidden></div>'


INDIRECT_REFUSAL = "may or may not contain"

DELIVERY_MEMBER = f"serviceAccount:{DELIVERY_EMAIL}"
DELIVERY_BINDING = {
    "role": "roles/storage.objectAdmin",
    "members": [DELIVERY_MEMBER],
}
# A shared-drive membership shows up on every file in the drive, as a
# permission whose details say it came from the drive rather than from the file.
INHERITED_ORGANIZER = {
    "type": "user",
    "role": "organizer",
    "emailAddress": DELIVERY_EMAIL,
    "permissionDetails": [
        {"permissionType": "member", "role": "organizer", "inherited": True}
    ],
}


def direct_reader(**overrides):
    """What `create_permission` in `src/delivery.rs` writes on a delivered file.

    A function rather than a constant because the expiry is read off the clock,
    and every caller that cares overrides it anyway.
    """
    return {
        "type": "user",
        "role": "reader",
        "emailAddress": "candidate@example.test",
        "expirationTime": (datetime.now(timezone.utc) + timedelta(hours=1)).isoformat(),
        "permissionDetails": [
            {"permissionType": "file", "role": "reader", "inherited": False}
        ],
    } | overrides


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
        "iamConfiguration": {
            "uniformBucketLevelAccess": {"enabled": True},
            # The setting that survives somebody later granting allUsers, and so
            # the one a correctly provisioned bucket carries.
            "publicAccessPrevention": "enforced",
        },
        "lifecycle": {
            "rule": [{"action": {"type": "Delete"}, "condition": {"age": 1}}]
        },
        # What a bucket returns once soft delete has been turned off, which is
        # what "correctly provisioned" means here: the API reports the disabled
        # state as a zero duration rather than by dropping the field.
        "softDeletePolicy": {"retentionDurationSeconds": "0"},
    } | overrides


# The two fakes, at module level rather than inside `run_check`, because they
# are the readable specification of what the script fetches and neither closes
# over anything: every value a test varies arrives through a `FAKE_*` variable.
# They model three things the first version did not, because a mutation run
# showed fifteen real defects surviving without them: which key is currently
# active, whether a request carried its credential, and that any one call can
# fail. Each `need_active` below is an invariant the script states and nothing
# checked.
FAKE_GCLOUD = """#!/bin/sh
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
  *"iam service-accounts get-iam-policy"*)
      need_active auditor "read the delivery account's own IAM policy"
      fails delivery-account-iam
      cat "$FAKE_ROOT/delivery-account-iam.json" ;;
  *) echo "unexpected gcloud invocation: $*" >&2; exit 99 ;;
esac
"""

FAKE_CURL = """#!/bin/sh
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

# Answer a Drive listing the way the API answers one: through the request's own
# `fields` mask, withholding every field the request did not name, at every
# level of the mask. Serving the whole fixture regardless was how dropping
# `permissionDetails` from a query stayed invisible to the whole suite, and it
# is not a property one field can be spot-checked for: the earlier version
# string-matched `deleted,permissionDetails(permissionType,role,inherited))`,
# which passed for the wrong reason if the mask were reordered and said nothing
# at all about `nextPageToken`. Withholding by the mask covers the nesting bug
# it was written for -- a sibling `permissions(...),permissionDetails(...)`
# spelling withholds `permissionDetails` from every permission -- and covers
# the fields nothing had spoken for.
serve()
{
  answer=$1
  shift
  python3 - "$answer" "$@" << 'PROJECT'
import json
import sys
import urllib.parse

answer_path, *arguments = sys.argv[1:]
url = next(argument for argument in arguments if "://" in argument and "?" in argument)
query = urllib.parse.parse_qs(url.split("?", 1)[1])

# A shared-drive file is not visible to a request that has not opted in, and
# the real API answers 404 rather than serving it. Without this the flag could
# be dropped from either query and every test still passed.
if query.get("supportsAllDrives") != ["true"]:
    print(json.dumps({"error": {"code": 404, "message": "File not found"}}))
    raise SystemExit(22)


# A Drive fields mask as {name: submask or None}, and where it ended.
def parse_mask(text, start=0):
    fields, name, index = {}, "", start
    while index < len(text):
        character = text[index]
        index += 1
        if character == "(":
            fields[name], index = parse_mask(text, index)
            name = ""
        elif character == ")":
            break
        elif character == ",":
            if name:
                fields[name] = None
            name = ""
        else:
            name += character
    if name:
        fields[name] = None
    return fields, index


def project(value, mask):
    if mask is None:
        return value
    if isinstance(value, list):
        return [project(item, mask) for item in value]
    if isinstance(value, dict):
        return {
            key: project(item, mask[key]) for key, item in value.items() if key in mask
        }
    return value


mask, _ = parse_mask(query["fields"][0])
print(json.dumps(project(json.load(open(answer_path, encoding="utf-8")), mask)))
PROJECT
}

case "$*" in
  *"/files/$FAKE_DRIVE_ID/permissions?"*pageToken=*)
      unauthenticated; fails drive-permissions
      serve "$FAKE_ROOT/drive-page-2.json" "$@" ;;
  *"/files/$FAKE_DRIVE_ID/permissions?"*)
      unauthenticated; fails drive-permissions; serve "$FAKE_ROOT/drive.json" "$@" ;;
  *"/drive/v3/files?"*pageToken=*)
      unauthenticated; fails drive-files
      serve "$FAKE_ROOT/drive-files-page-2.json" "$@" ;;
  *"/drive/v3/files?"*)
      unauthenticated; fails drive-files; serve "$FAKE_ROOT/drive-files.json" "$@" ;;
  *"/drive/v3/files/"*"/permissions?"*)
      unauthenticated; fails file-permissions
      # Per-file answers, so one run can hold a folder and the file beneath it
      # and show that the folder's own grant is refused where the child's
      # inherited copy of it is skipped. The shared fixture is the fallback,
      # because almost every test varies one drive-wide shape rather than the
      # relationship between two files.
      file=
      for argument in "$@"; do
        case $argument in
          *"/drive/v3/files/"*"/permissions?"*)
              file=${argument#*"/drive/v3/files/"}
              file=${file%%/permissions*} ;;
        esac
      done
      answer=$FAKE_ROOT/file-permissions-$file.json
      [ -f "$answer" ] || answer=$FAKE_ROOT/file-permissions.json
      serve "$answer" "$@" ;;
  *"/recording/index.html"*)
      printf '%s\\n' "$*" >> "$FAKE_ROOT/curl-args.log"
      printf '%s' "$FAKE_TEMPLATE_BODY" > "$out"
      printf '%s' "$FAKE_TEMPLATE_STATUS"
      [ "$FAKE_TEMPLATE_TRANSFER" = complete ] || exit 28
      ;;
  *) echo "unexpected curl invocation: $*" >&2; exit 99 ;;
esac
"""


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
        delivery_account_iam=None,
        drive_files=None,
        drive_files_page_2=None,
        file_permissions=None,
        file_permissions_by_id=None,
    ):
        if bucket_iam is None:
            bucket_iam = bucket_iam_policy()
        if ancestors_iam is None:
            ancestors_iam = [ancestor_policy()]
        if bucket is None:
            bucket = bucket_resource()
        if drive is None:
            drive = drive_permissions()
        if delivery_account_iam is None:
            # A dedicated delivery account carries no binding of its own.
            delivery_account_iam = {"bindings": []}
        if drive_files is None:
            drive_files = {"files": [{"id": "file-1", "name": "recording-1.mp4"}]}
        if drive_files_page_2 is None:
            drive_files_page_2 = {"files": []}
        if file_permissions is None:
            # What a delivered recording carries: the drive membership the
            # delivery account holds, inherited onto every file, plus the one
            # expiring reader the delivery path grants the candidate.
            file_permissions = {"permissions": [INHERITED_ORGANIZER, direct_reader()]}
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
                "delivery-account-iam.json": delivery_account_iam,
                "drive-files.json": drive_files,
                "drive-files-page-2.json": drive_files_page_2,
                "file-permissions.json": file_permissions,
            } | {
                # Keyed by file id, for the tests that need two files on one
                # drive to differ. Everything else falls back to the shared
                # answer above.
                f"file-permissions-{file_id}.json": permissions
                for file_id, permissions in (file_permissions_by_id or {}).items()
            }
            for name, value in files.items():
                (temporary_path / name).write_text(json.dumps(value))
            self.write_executable(fake_bin, "gcloud", FAKE_GCLOUD)
            self.write_executable(fake_bin, "curl", FAKE_CURL)
            environment = os.environ | {
                "PATH": f"{fake_bin}:{os.environ['PATH']}",
                "FAKE_ROOT": str(temporary_path),
                "FAKE_FAIL": fail,
                "FAKE_PROJECT_NUMBER": PROJECT_NUMBER,
                "FAKE_TEMPLATE_STATUS": template_status,
                "FAKE_TEMPLATE_BODY": template_body,
                "FAKE_TEMPLATE_TRANSFER": template_transfer,
                "FAKE_TOKEN": token,
                "FAKE_DRIVE_ID": "drive_123",
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

    def test_a_binding_on_the_delivery_account_itself_is_refused(self):
        # Impersonation is granted on the account's own resource policy, which
        # is neither the bucket's nor any ancestor's, so every other check here
        # passes while somebody can mint tokens for the identity that holds
        # objectAdmin on the recordings and organizer on the drive.
        for role in [
            "roles/iam.serviceAccountTokenCreator",
            "roles/iam.serviceAccountUser",
            "roles/iam.serviceAccountKeyAdmin",
            "roles/owner",
        ]:
            with self.subTest(role=role):
                result = self.run_check(
                    delivery_account_iam={
                        "bindings": [
                            {"role": role, "members": ["user:someone@example.test"]}
                        ]
                    }
                )
                self.assertRefused(result, "must carry none")
                self.assertIn(role, result.stderr)

    def test_an_empty_binding_on_the_delivery_account_does_not_refuse(self):
        # A role with no members grants nothing, and gcloud emits the shape.
        result = self.run_check(
            delivery_account_iam={
                "bindings": [{"role": "roles/iam.serviceAccountTokenCreator"}]
            }
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_the_delivery_account_policy_is_read_as_the_auditor(self):
        # It is a policy read, so it belongs to the independent auditor like
        # every other one; reading it as the delivery account would be the
        # account auditing who may become it.
        result = self.run_check()
        self.assertEqual(result.returncode, 0, result.stderr)
        reads = [
            line
            for line in result.gcloud_log.splitlines()
            if "service-accounts get-iam-policy" in line
        ]
        self.assertEqual(len(reads), 1, result.gcloud_log)

    def test_a_bucket_without_public_access_prevention_is_refused(self):
        # The setting that survives a later mistake. Without it, a grant to
        # allUsers is still something somebody can make, and this audit only
        # ever reports the policy as it stands on the day it runs.
        for prevention in ["inherited", None]:
            with self.subTest(prevention=prevention):
                configuration = {"uniformBucketLevelAccess": {"enabled": True}}
                if prevention is not None:
                    configuration["publicAccessPrevention"] = prevention
                result = self.run_check(
                    bucket=bucket_resource(iamConfiguration=configuration)
                )
                self.assertRefused(result, "public access prevention")

    def test_a_revertible_uniform_access_window_is_reported_not_refused(self):
        # Uniform access can be turned back off within 90 days of being set, so
        # on a bucket provisioned today it is a setting rather than a property.
        # Refusing would fail every bucket for its first three months, which is
        # every bucket this check sees on the day somebody provisions it.
        result = self.run_check(
            bucket=bucket_resource(
                iamConfiguration={
                    "uniformBucketLevelAccess": {
                        "enabled": True,
                        "lockedTime": "2027-01-01T00:00:00Z",
                    },
                    "publicAccessPrevention": "enforced",
                }
            )
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("can be reverted until 2027-01-01", result.stdout)

    def test_a_recording_shared_by_link_is_refused(self):
        # The membership audit is about who holds the drive. This is about who
        # can reach one recording, which is where the delivery path writes its
        # grants and where a link share actually lives.
        for permission in [
            {"type": "anyone", "role": "reader"},
            {"type": "domain", "domain": "example.test", "role": "reader"},
            {"type": "group", "emailAddress": "eng@example.test", "role": "reader"},
        ]:
            with self.subTest(permission=permission):
                result = self.run_check(
                    file_permissions={"permissions": [INHERITED_ORGANIZER, permission]}
                )
                self.assertRefused(result, "never by holding a link")
                self.assertIn("file-1", result.stderr)

    def test_a_recording_shared_directly_with_a_colleague_is_refused(self):
        # Not a link share and not a group: one named person given standing
        # access to somebody's interview. Every row here is `user`, so the type
        # check above cannot see it, and it is the shape the delivery path never
        # produces.
        for role in ["writer", "commenter", "fileOrganizer", "organizer"]:
            with self.subTest(role=role):
                result = self.run_check(
                    file_permissions={
                        "permissions": [
                            INHERITED_ORGANIZER,
                            {
                                "type": "user",
                                "role": role,
                                "emailAddress": "colleague@example.test",
                                "permissionDetails": [
                                    {
                                        "permissionType": "file",
                                        "role": role,
                                        "inherited": False,
                                    }
                                ],
                            },
                        ]
                    }
                )
                self.assertRefused(result, "shared directly with")
                self.assertIn("colleague@example.test", result.stderr)

    def test_a_direct_reader_whose_expiry_cannot_be_read_is_refused(self):
        # Absent, naive, unparseable and empty all mean the same thing: this
        # grant does not lapse on its own as far as the audit can tell. The
        # naive one is why the guard exists rather than the comparison alone,
        # because comparing it raises "can't compare offset-naive and
        # offset-aware datetimes" out of the script as a traceback.
        for expiration_time in [None, "2026-09-07T12:00:00", "not-a-timestamp", ""]:
            with self.subTest(expiration_time=expiration_time):
                reader = direct_reader(expirationTime=expiration_time)
                result = self.run_check(file_permissions={"permissions": [reader]})
                self.assertRefused(result, "without a valid expirationTime")
                self.assertNotIn("Traceback", result.stderr)

    def test_a_direct_reader_expiring_beyond_the_retention_window_is_refused(self):
        expiration_time = (datetime.now(timezone.utc) + timedelta(hours=25)).isoformat()
        result = self.run_check(
            file_permissions={
                "permissions": [direct_reader(expirationTime=expiration_time)]
            }
        )
        self.assertRefused(result, "more than 24 hours out")

    def test_an_expired_direct_reader_does_not_refuse(self):
        # A lapsed grant confers nothing, which is the state the expiry exists
        # to reach, and Drive does not document how promptly it drops one from
        # this listing. Refusing over it would fail a drive for being correct.
        expiration_time = (datetime.now(timezone.utc) - timedelta(hours=1)).isoformat()
        result = self.run_check(
            file_permissions={
                "permissions": [direct_reader(expirationTime=expiration_time)]
            }
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_the_expiry_bound_tolerates_clock_skew_between_two_hosts(self):
        # Delivery stamps the expiry as its own host's now plus the retention,
        # and this audit compares it against its own clock, so an exact bound
        # refuses a correct drive whenever the two disagree by a second.
        expiration_time = (
            datetime.now(timezone.utc) + timedelta(hours=24, minutes=1)
        ).isoformat()
        result = self.run_check(
            file_permissions={
                "permissions": [direct_reader(expirationTime=expiration_time)]
            }
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_a_grant_inherited_from_a_folder_is_not_read_as_this_file_s_own(self):
        # A share made on a folder inside the drive reaches every file under it
        # as a `file` entry carrying `inherited`. Reading that as this file's
        # own delivery grant let one folder share pass as many, and would have
        # refused a legitimate folder reader on every child at once. The folder
        # is listed by the same query and audited in its turn.
        result = self.run_check(
            file_permissions={
                "permissions": [
                    INHERITED_ORGANIZER,
                    {
                        "type": "user",
                        "role": "writer",
                        "emailAddress": "colleague@example.test",
                        "permissionDetails": [
                            {
                                "permissionType": "file",
                                "role": "writer",
                                "inherited": True,
                            }
                        ],
                    },
                ]
            }
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_a_merged_membership_and_file_grant_is_judged_on_the_file_entry(self):
        # One grantee can hold a drive membership and a grant on this file at
        # once, and Drive reports a single merged top-level role for the pair
        # whose meaning it does not document. The entry says what was granted
        # here, so a top-level role of `reader` does not excuse a `writer` on
        # the file.
        result = self.run_check(
            file_permissions={
                "permissions": [
                    {
                        "type": "user",
                        "role": "reader",
                        "emailAddress": "colleague@example.test",
                        "permissionDetails": [
                            {
                                "permissionType": "member",
                                "role": "reader",
                                "inherited": True,
                            },
                            {
                                "permissionType": "file",
                                "role": "writer",
                                "inherited": False,
                            },
                        ],
                    }
                ]
            }
        )
        self.assertRefused(result, "shared directly with")
        self.assertIn("as writer", result.stderr)

    def test_every_own_entry_is_judged_not_only_the_first(self):
        # Drive is not documented to report one `file` source per grantee, and
        # judging on `own[0]` made the verdict turn on the order of an array:
        # this pair passed with the reader ahead of the organizer and refused
        # with them the other way round. The other two listings in this script
        # quantify over all their rows; so does this one.
        entries = [
            {"permissionType": "file", "role": "reader", "inherited": False},
            {"permissionType": "file", "role": "organizer", "inherited": False},
        ]
        for order in [entries, entries[::-1]]:
            with self.subTest(first=order[0]["role"]):
                result = self.run_check(
                    file_permissions={
                        "permissions": [direct_reader(permissionDetails=list(order))]
                    }
                )
                self.assertRefused(result, "shared directly with")
                self.assertIn("as organizer", result.stderr)

    def test_the_expiry_format_the_delivery_path_actually_writes_is_accepted(self):
        # `create_permission` in `src/delivery.rs` formats the stamp with a
        # trailing `Z`, and every other fixture here uses `isoformat`, which
        # writes `+00:00`. `fromisoformat` only learned `Z` in 3.11 and this
        # script names no Python floor, so the one form production emits was
        # the one form nothing exercised: deleting the normalisation left the
        # suite green and broke the check on any older interpreter. Lowercase
        # is as valid in RFC 3339 and Drive is not documented to prefer either.
        expiry = datetime.now(timezone.utc) + timedelta(hours=1)
        for stamp in [
            expiry.strftime("%Y-%m-%dT%H:%M:%SZ"),
            expiry.strftime("%Y-%m-%dT%H:%M:%Sz"),
        ]:
            with self.subTest(stamp=stamp):
                result = self.run_check(
                    file_permissions={
                        "permissions": [direct_reader(expirationTime=stamp)]
                    }
                )
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_permission_details_that_are_not_a_list_of_objects_are_refused(self):
        # Fails closed either way, because a traceback exits non-zero too. But a
        # traceback says the audit broke, not what the drive is carrying, and an
        # audit whose output cannot tell those apart tells whoever runs it the
        # wrong thing. The same principle `aware_expiry` states for a naive
        # stamp, applied to the field read just above it.
        for details in ["file", {"permissionType": "file"}, ["file"], [None]]:
            with self.subTest(details=details):
                result = self.run_check(
                    file_permissions={
                        "permissions": [direct_reader(permissionDetails=details)]
                    }
                )
                self.assertRefused(result, "that this audit cannot read")
                self.assertNotIn("Traceback", result.stderr)

    def test_file_permissions_that_do_not_fit_one_page_are_refused(self):
        # The rule the membership listing already carries, on the listing that
        # had nothing holding it: a listing whose end has not been seen is not
        # an exactness claim. This is also the half that observes
        # `nextPageToken` surviving in the query's fields mask, because the fake
        # withholds what the mask does not name, so a mask that stops asking for
        # the token cannot report one and this refusal stops being reachable.
        result = self.run_check(
            file_permissions={
                "permissions": [direct_reader()],
                "nextPageToken": "page-2",
            }
        )
        self.assertRefused(result, "cannot see the end of them")
        self.assertIn("file-1", result.stderr)

    def test_a_folder_share_is_refused_on_the_folder_that_carries_it(self):
        # Skipping an inherited `file` entry rests on the folder that owns the
        # grant being listed and audited in its turn, and nothing showed that:
        # the sibling test proves only that the child's copy is skipped. Here
        # the drive holds a folder and the file beneath it, the child carries
        # the inherited copy and passes, and the refusal names the folder.
        share = {
            "type": "user",
            "role": "writer",
            "emailAddress": "colleague@example.test",
        }
        result = self.run_check(
            drive_files={"files": [{"id": "folder-1"}, {"id": "file-1"}]},
            file_permissions_by_id={
                "folder-1": {
                    "permissions": [
                        INHERITED_ORGANIZER,
                        share
                        | {
                            "permissionDetails": [
                                {
                                    "permissionType": "file",
                                    "role": "writer",
                                    "inherited": False,
                                }
                            ]
                        },
                    ]
                },
                "file-1": {
                    "permissions": [
                        INHERITED_ORGANIZER,
                        share
                        | {
                            "permissionDetails": [
                                {
                                    "permissionType": "file",
                                    "role": "writer",
                                    "inherited": True,
                                }
                            ]
                        },
                        direct_reader(),
                    ]
                },
            },
        )
        self.assertRefused(result, "shared directly with")
        self.assertIn("folder-1", result.stderr)

    def test_a_drive_writer_holding_a_file_reader_is_still_a_writer(self):
        # Effective access is both grants together. Judging only the `file`
        # entry read this pair as the expiring reader the delivery path writes,
        # and the same person could write the recording through the membership
        # the other entry names. The skip above still returns a pure member to
        # the membership audit, so this only reaches somebody who holds a direct
        # grant here as well, which the delivery account never does.
        result = self.run_check(
            file_permissions={
                "permissions": [
                    INHERITED_ORGANIZER,
                    direct_reader(
                        emailAddress="colleague@example.test",
                        role="writer",
                        permissionDetails=[
                            {
                                "permissionType": "member",
                                "role": "writer",
                                "inherited": True,
                            },
                            {
                                "permissionType": "file",
                                "role": "reader",
                                "inherited": False,
                            },
                        ],
                    ),
                ]
            }
        )
        self.assertRefused(result, "shared directly with")
        self.assertIn("writer", result.stderr)

    def test_the_drive_membership_inherited_onto_a_file_does_not_refuse(self):
        # The organizer grant reaches every file in the drive, and refusing it
        # here would fail the correctly provisioned case: it is the drive
        # membership audit's business, and that audit has already run.
        result = self.run_check(file_permissions={"permissions": [INHERITED_ORGANIZER]})
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_a_grant_with_no_details_is_read_as_direct(self):
        # `permissionDetails` is populated on shared-drive items, so its absence
        # is not something to resolve in the passing direction.
        result = self.run_check(
            file_permissions={
                "permissions": [
                    {
                        "type": "user",
                        "role": "writer",
                        "emailAddress": "colleague@example.test",
                    }
                ]
            }
        )
        self.assertRefused(result, "shared directly with")

    def test_a_deleted_file_permission_does_not_refuse(self):
        result = self.run_check(
            file_permissions={
                "permissions": [{"type": "anyone", "role": "reader", "deleted": True}]
            }
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_a_file_id_outside_the_alphabet_is_refused(self):
        # The id is pasted into a query string and into a filename under the
        # work directory, so a `?` rewrites the request and a `/` writes the
        # answer where the reader never globs for it -- either way the audit
        # would report on a file whose permissions it never read. The same
        # alphabet the Drive id itself is held to.
        for file_id in ["../escape", "one?two", "a/b"]:
            with self.subTest(file_id=file_id):
                result = self.run_check(drive_files={"files": [{"id": file_id}]})
                self.assertRefused(result, "cannot read")

    def test_every_file_on_the_drive_is_read(self):
        result = self.run_check(
            drive_files={
                "files": [
                    {"id": "file-1", "name": "one.mp4"},
                    {"id": "file-2", "name": "two.mp4"},
                ]
            }
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("permissions on 2 Shared Drive files", result.stdout)
        for file_id in ["file-1", "file-2"]:
            self.assertTrue(
                any(
                    f"/files/{file_id}/permissions" in line
                    for line in result.curl_log.splitlines()
                ),
                f"{file_id} was never read: {result.curl_log}",
            )

    def test_a_second_page_of_drive_files_is_read(self):
        # Refusing the second page outright was a false-fail the moment a busy
        # day produced a hundred and one recordings. The ceiling stays, because
        # this reads the permissions on every file while an operator waits.
        result = self.run_check(
            drive_files={"files": [{"id": "file-1"}], "nextPageToken": "page-2"},
            drive_files_page_2={"files": [{"id": "file-2"}]},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("permissions on 2 Shared Drive files", result.stdout)

    def test_a_file_listing_that_never_stops_paging_is_refused(self):
        result = self.run_check(
            drive_files={"files": [{"id": "file-1"}], "nextPageToken": "page-2"},
            drive_files_page_2={"files": [{"id": "file-2"}], "nextPageToken": "again"},
        )
        self.assertRefused(result, "this audit reads the permissions on each")

    def test_a_truncated_file_permission_listing_is_refused(self):
        # Exactly the rule the drive-membership listing already carries: a page
        # with a token is a listing whose end this script has not seen, and a
        # link share on page two passed while it was read as a complete answer.
        result = self.run_check(
            file_permissions={
                "permissions": [
                    {
                        "type": "user",
                        "role": "reader",
                        "emailAddress": "candidate@example.test",
                    }
                ],
                "nextPageToken": "more",
            }
        )
        self.assertRefused(result, "do not fit one page")

    def test_a_deleted_principal_on_the_delivery_account_does_not_refuse(self):
        # IAM keeps a tombstone in the policy after a principal is deleted. It
        # grants nothing, and refusing over one would fail an account whose
        # stray grant had already been removed, which is the state this check
        # is asking the operator to reach.
        result = self.run_check(
            delivery_account_iam={
                "bindings": [
                    {
                        "role": "roles/iam.serviceAccountTokenCreator",
                        "members": ["deleted:user:gone@example.test?uid=123"],
                    }
                ]
            }
        )
        self.assertEqual(result.returncode, 0, result.stderr)

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
