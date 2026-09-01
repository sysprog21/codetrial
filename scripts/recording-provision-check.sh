#!/bin/sh
set -eu

# Verify the isolated recording environment without retaining credentials or
# resource identifiers in the checkout. The auditor reads IAM policies; the
# delivery account only proves access to the bucket and Shared Drive it uses.

need() {
  name=$1
  value=$2
  [ -n "$value" ] || {
    echo "missing required environment variable: $name" >&2
    exit 2
  }
}

for tool in gcloud curl mktemp python3; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "required command is unavailable: $tool" >&2
    exit 2
  }
done

need CODETRIAL_RECORDING_PROJECT "${CODETRIAL_RECORDING_PROJECT:-}"
need CODETRIAL_RECORDING_GCS_BUCKET "${CODETRIAL_RECORDING_GCS_BUCKET:-}"
need CODETRIAL_RECORDING_DRIVE_ID "${CODETRIAL_RECORDING_DRIVE_ID:-}"
need CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON "${CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON:-}"
need CODETRIAL_RECORDING_IAM_AUDITOR_JSON "${CODETRIAL_RECORDING_IAM_AUDITOR_JSON:-}"
need CODETRIAL_RECORDING_TEMPLATE_BASE_URL "${CODETRIAL_RECORDING_TEMPLATE_BASE_URL:-}"

project=$CODETRIAL_RECORDING_PROJECT
bucket=$CODETRIAL_RECORDING_GCS_BUCKET
drive_id=$CODETRIAL_RECORDING_DRIVE_ID
delivery_json=$CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON
auditor_json=$CODETRIAL_RECORDING_IAM_AUDITOR_JSON
template_origin=${CODETRIAL_RECORDING_TEMPLATE_BASE_URL%/}
unset CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON CODETRIAL_RECORDING_IAM_AUDITOR_JSON

case "$project" in
  *[!A-Za-z0-9-]*)
    echo "CODETRIAL_RECORDING_PROJECT has an invalid project id" >&2
    exit 2
    ;;
esac
case "$bucket" in
  *[!a-z0-9._-]*)
    echo "CODETRIAL_RECORDING_GCS_BUCKET has an invalid bucket name" >&2
    exit 2
    ;;
esac
case "$drive_id" in
  *[!A-Za-z0-9_-]*)
    echo "CODETRIAL_RECORDING_DRIVE_ID has an invalid Drive id" >&2
    exit 2
    ;;
esac
case "$template_origin" in
  https://*) ;;
  *)
    echo "template base URL must be an https origin" >&2
    exit 2
    ;;
esac
template_host=${template_origin#https://}

# Structure only. Which addresses the host stands for is settled below by
# resolving it, so no spelling of a private address needs a pattern here.
case "$template_host" in
  '' | :* | */* | *\?* | *\#* | *@* | \[*)
    echo "template base URL must be a bare https host with no path or userinfo" >&2
    exit 2
    ;;
esac
case "$template_host" in
  *:*)
    template_port=${template_host#*:}
    case "$template_port" in
      [1-9] | [1-9][0-9] | [1-9][0-9][0-9] | [1-9][0-9][0-9][0-9] | [1-9][0-9][0-9][0-9][0-9]) ;;
      *)
        echo "template base URL has an invalid port" >&2
        exit 2
        ;;
    esac
    [ "$template_port" -le 65535 ] || {
      echo "template base URL has an invalid port" >&2
      exit 2
    }
    ;;
esac

# Egress fetches this origin from the public internet, so what matters is where
# the name points, not how it is spelled: `localhost` and `2130706433` are both
# 127.0.0.1 and neither looks private. Resolve it before either credential is
# loaded and refuse any answer that is not globally routable.
template_name=${template_host%%:*}
python3 - "$template_name" <<'PY'
import ipaddress
import socket
import sys

host = sys.argv[1]
try:
    addresses = {result[4][0] for result in socket.getaddrinfo(host, None)}
except socket.gaierror as error:
    raise SystemExit(f"template base URL host cannot be resolved: {error}")

if any(not ipaddress.ip_address(address).is_global for address in addresses):
    raise SystemExit("template base URL must resolve only to public addresses")
PY

umask 077
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
export CLOUDSDK_CONFIG="$work/gcloud"
mkdir "$CLOUDSDK_CONFIG"
delivery_key=$work/delivery-service-account.json
auditor_key=$work/iam-auditor.json
printf '%s' "$delivery_json" >"$delivery_key"
printf '%s' "$auditor_json" >"$auditor_key"
unset delivery_json auditor_json

service_account_email() {
  python3 - "$1" "$2" <<'PY'
import json
import sys

path, label = sys.argv[1:]
try:
    key = json.load(open(path, encoding="utf-8"))
except (OSError, json.JSONDecodeError) as error:
    raise SystemExit(f"{label} JSON is unreadable: {error}")

email = key.get("client_email")
if not isinstance(email, str) or not email.endswith(".gserviceaccount.com"):
    raise SystemExit(f"{label} has no service-account client_email")
print(email)
PY
}

# A key's client_email carries its own project, and the delivery address is
# pinned to this project's exactly below, so the recorded project_id has nothing
# left to disagree about.
delivery_email=$(service_account_email "$delivery_key" "delivery service-account JSON")
auditor_email=$(service_account_email "$auditor_key" "IAM auditor JSON")
expected_delivery_email="codetrial-recording@$project.iam.gserviceaccount.com"
[ "$delivery_email" = "$expected_delivery_email" ] || {
  echo "delivery service account must be $expected_delivery_email" >&2
  exit 2
}
[ "$delivery_email" != "$auditor_email" ] || {
  echo "CODETRIAL_RECORDING_IAM_AUDITOR_JSON must identify a different account" >&2
  exit 2
}

# Policy reads must use the independent auditor. A failed read is a failed
# check, not a reason to assume the delivery account has no broader grant.
gcloud auth activate-service-account --key-file="$auditor_key" --quiet >/dev/null
gcloud storage buckets get-iam-policy "gs://$bucket" --format=json >"$work/bucket-iam.json"

# A project policy cannot show grants inherited from folders or the
# organization. This command returns the complete ancestor chain, and failure is
# a failed audit rather than a reason to assume there are no broad grants.
gcloud projects get-ancestors-iam-policy "$project" --format=json >"$work/ancestors-iam.json"

# --raw, because the checks below read the JSON API's own field names. Without
# it gcloud standardises the resource into uniform_bucket_level_access and
# lifecycle_config, and both reads come back empty against a bucket that is
# configured correctly.
gcloud storage buckets describe "gs://$bucket" --raw --format=json >"$work/bucket.json"

# The bucket name is configured on its own, so nothing so far ties it to the
# project whose ancestor policies were just audited. A bucket owned elsewhere
# inherits that other project's grants, and none of them appear above.
project_number=$(gcloud projects describe "$project" --format="value(projectNumber)")
auditor_token=$(gcloud auth print-access-token)
auditor_curl=$work/auditor-curl.conf
printf 'header = "Authorization: Bearer %s"\n' "$auditor_token" >"$auditor_curl"
curl --fail --silent --show-error --max-time 20 --config "$auditor_curl" \
  "https://www.googleapis.com/drive/v3/files/$drive_id/permissions?supportsAllDrives=true&fields=permissions(id,type,role,emailAddress,deleted)" \
  >"$work/drive-permissions.json"

python3 - "$delivery_email" "$work/bucket-iam.json" "$work/ancestors-iam.json" \
  "$work/bucket.json" "$work/drive-permissions.json" "$project_number" <<'PY'
import json
import sys

delivery, bucket_iam_path, ancestors_iam_path, bucket_path, drive_path, project_number = sys.argv[1:]
member = f"serviceAccount:{delivery}"

def read(path):
    try:
        return json.load(open(path, encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"unreadable audit response {path}: {error}")

bucket_iam = read(bucket_iam_path)
ancestors_iam = read(ancestors_iam_path)
bucket = read(bucket_path)
drive = read(drive_path)

def member_roles(policy):
    roles = []
    for binding in policy.get("bindings", []):
        members = binding.get("members", [])
        # These identities either include every authenticated service account
        # or need a separate directory lookup to exclude this one. Treating
        # them as unrelated would turn an exact-access audit into a guess.
        if any(
            principal in {"allUsers", "allAuthenticatedUsers"}
            or principal.startswith(("group:", "domain:", "principalSet:"))
            for principal in members
        ):
            raise SystemExit("IAM has public or indirect bindings; exact access is not auditable")
        if member in members:
            if binding.get("condition") is not None:
                raise SystemExit(f"{member} has a conditional IAM binding; exact access is not auditable")
            roles.append(binding.get("role"))
    return roles

bucket_roles = member_roles(bucket_iam)
if bucket_roles != ["roles/storage.objectAdmin"]:
    raise SystemExit(f"{member} bucket roles are {bucket_roles!r}, expected exactly ['roles/storage.objectAdmin']")
if not isinstance(ancestors_iam, list) or not ancestors_iam:
    raise SystemExit("ancestor IAM audit response is not a non-empty policy list")
ancestor_roles = []
for entry in ancestors_iam:
    if not isinstance(entry, dict):
        raise SystemExit("ancestor IAM audit response is not a list of policies")

    # The label names the ancestor in the refusal below and does nothing else,
    # so it must not decide whether the audit can run: gcloud calls it id, the
    # resource-manager APIs call it resource, and an audit that refuses over
    # the spelling refuses every real policy chain.
    label = entry.get("id") or entry.get("resource") or "an unnamed ancestor"
    policy = entry.get("policy")
    if not isinstance(policy, dict):
        raise SystemExit(f"ancestor IAM policy for {label} is unreadable")
    roles = member_roles(policy)
    if roles:
        ancestor_roles.append((label, roles))
if ancestor_roles:
    raise SystemExit(f"{member} has forbidden ancestor IAM roles: {ancestor_roles!r}")

owner = bucket.get("projectNumber")
if not project_number or str(owner) != str(project_number):
    raise SystemExit(f"bucket belongs to project number {owner!r}, not {project_number!r}")

if bucket.get("iamConfiguration", {}).get("uniformBucketLevelAccess", {}).get("enabled") is not True:
    raise SystemExit("bucket must enable Uniform Bucket-Level Access")

# Exactly `age: 1` and nothing beside it. A rule carrying a prefix, storage
# class or version filter deletes some objects at a day old and leaves the rest,
# and this script is not told which prefix the recordings use, so it cannot say
# whether a filtered rule covers them.
rules = bucket.get("lifecycle", {}).get("rule", [])
if not any(
    rule.get("action") == {"type": "Delete"} and rule.get("condition") == {"age": 1}
    for rule in rules
):
    raise SystemExit("bucket has no unconditional 24-hour Delete lifecycle rule")

matches = [
    permission
    for permission in drive.get("permissions", [])
    if not permission.get("deleted") and permission.get("emailAddress") == delivery
]
if len(matches) != 1 or matches[0].get("type") != "user" or matches[0].get("role") != "organizer":
    raise SystemExit(f"{delivery} must have exactly one active Shared Drive organizer permission")
PY
echo "IAM, lifecycle, and Shared Drive membership are least-privilege"

# Access is checked as the delivery account itself, not inferred from the
# auditor's broad ability to inspect policy.
gcloud auth activate-service-account --key-file="$delivery_key" --quiet >/dev/null
gcloud storage ls "gs://$bucket" >/dev/null
echo "GCS bucket is accessible"
delivery_token=$(gcloud auth print-access-token)
delivery_curl=$work/delivery-curl.conf
printf 'header = "Authorization: Bearer %s"\n' "$delivery_token" >"$delivery_curl"
curl --fail --silent --show-error --max-time 20 --config "$delivery_curl" \
  "https://www.googleapis.com/drive/v3/files?corpora=drive&driveId=$drive_id&includeItemsFromAllDrives=true&supportsAllDrives=true&pageSize=1&fields=files(id)" \
  >/dev/null
echo "Shared Drive is accessible"

curl --fail --silent --show-error --max-time 30 \
  "$template_origin/recording/index.html" >/dev/null
echo "Recording template is publicly reachable"
