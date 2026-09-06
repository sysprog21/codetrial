#!/bin/sh
set -eu

# Verify the isolated recording environment without retaining credentials or
# resource identifiers in the checkout. The auditor reads IAM policies; the
# delivery account only proves access to the bucket and Shared Drive it uses.

need()
{
    name=$1
    value=$2
    [ -n "$value" ] || {
        echo "missing required environment variable: $name" >&2
        exit 2
    }
}

for tool in gcloud curl mktemp python3; do
    command -v "$tool" > /dev/null 2>&1 || {
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
template_fetch_port=443
case "$template_host" in
    *:*)
        template_fetch_port=${template_host#*:}
        case "$template_fetch_port" in
            [1-9] | [1-9][0-9] | [1-9][0-9][0-9] | [1-9][0-9][0-9][0-9] | [1-9][0-9][0-9][0-9][0-9]) ;;
            *)
                echo "template base URL has an invalid port" >&2
                exit 2
                ;;
        esac
        [ "$template_fetch_port" -le 65535 ] || {
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

# The address is printed, not merely approved, so the fetch below can be pinned
# to it. Resolving here and letting curl resolve again is a check on one answer
# and a connection to another: a short-TTL record passes this and then sends the
# fetch somewhere else. `--resolve` binds the two together.
template_address=$(
    python3 - "$template_name" << 'PY'
import ipaddress
import socket
import sys

host = sys.argv[1]
try:
    addresses = {result[4][0] for result in socket.getaddrinfo(host, None)}
except OSError as error:
    # Not just gaierror: every other OSError out of getaddrinfo used to reach
    # the operator as a traceback rather than as this sentence.
    raise SystemExit(f"template base URL host cannot be resolved: {error}")

try:
    parsed = [ipaddress.ip_address(address) for address in addresses]
except ValueError as error:
    # An IPv6 answer can carry a %scope suffix that ip_address refuses.
    raise SystemExit(f"template base URL resolved to an unreadable address: {error}")

if any(not address.is_global for address in parsed):
    raise SystemExit("template base URL must resolve only to public addresses")

# Sorted within one family, because IPv4Address and IPv6Address do not compare
# with each other and a dual-stack host would otherwise raise a TypeError here
# rather than answer. IPv4 first: it is the one every network can reach.
parsed.sort(key=lambda address: (address.version, address))

# Sorted, so the pinned address does not depend on set iteration order and the
# same host does not reach a different server between two runs. Bracketed when
# it is IPv6, because that is the spelling `curl --resolve` takes and an
# AAAA-only origin would otherwise fail a check it should pass.
pinned = parsed[0]
print(f"[{pinned}]" if pinned.version == 6 else pinned)
PY
)

umask 077
work=$(mktemp -d)

# QUIT is in the list because it was not, and `/bin/sh` here is dash, where a
# trap that omits it leaves both service-account private keys and both
# bearer-token config files on disk after a Ctrl-\\. The handler exits rather
# than falling through: a caught INT used to return to the next command with
# `$work` already deleted, which reported a missing template file instead of an
# interruption and exited 0.
trap 'rm -rf "$work"' EXIT
trap 'rm -rf "$work"; exit 130' HUP INT QUIT TERM
export CLOUDSDK_CONFIG="$work/gcloud"
mkdir "$CLOUDSDK_CONFIG"
delivery_key=$work/delivery-service-account.json
auditor_key=$work/iam-auditor.json
printf '%s' "$delivery_json" > "$delivery_key"
printf '%s' "$auditor_json" > "$auditor_key"
unset delivery_json auditor_json

service_account_email()
{
    python3 - "$1" "$2" << 'PY'
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
gcloud auth activate-service-account --key-file="$auditor_key" --quiet > /dev/null
gcloud storage buckets get-iam-policy "gs://$bucket" --format=json > "$work/bucket-iam.json"

# A project policy cannot show grants inherited from folders or the
# organization. This command returns the complete ancestor chain, and failure is
# a failed audit rather than a reason to assume there are no broad grants.
gcloud projects get-ancestors-iam-policy "$project" --format=json > "$work/ancestors-iam.json"

# --raw, because the checks below read the JSON API's own field names. Without
# it gcloud standardises the resource into uniform_bucket_level_access and
# lifecycle_config, and both reads come back empty against a bucket that is
# configured correctly.
gcloud storage buckets describe "gs://$bucket" --raw --format=json > "$work/bucket.json"

# The bucket name is configured on its own, so nothing so far ties it to the
# project whose ancestor policies were just audited. A bucket owned elsewhere
# inherits that other project's grants, and none of them appear above.
project_number=$(gcloud projects describe "$project" --format="value(projectNumber)")

# The token goes in a config file rather than in `-H` on the command line so it
# stays out of `ps`. That format has its own escaping, though, and nothing was
# checking the value against it: a token carrying a quote silently truncated the
# header, and one carrying a newline had its remainder parsed as further curl
# options, which was proved end to end by making curl write its body to a chosen
# path. Real Google tokens are drawn from this alphabet; anything else is a
# `gcloud` that printed something besides the token, and this refuses rather
# than concatenates it.
write_bearer_config()
{
    # A `case` glob and not `grep -qx`: with a multi-line value `grep -x`
    # succeeds when any single line matches, so a token whose second line was a
    # curl directive passed a check written to stop exactly that. This tests the
    # whole string, and a newline is outside the set like any other character.
    case "$2" in
        '' | *[!A-Za-z0-9._~+/=-]*)
            echo "the $3 access token is not a bare token; refusing to build a curl config" >&2
            exit 2
            ;;
    esac
    printf 'header = "Authorization: Bearer %s"\n' "$2" > "$1"
}

# The delivery account's own resource policy, which is a different policy from
# the project's and appears in neither the bucket's nor any ancestor's. A
# binding here hands somebody else the delivery identity entire: objectAdmin on
# the recordings and organizer on the drive, reached by minting a token rather
# than by holding a role anything above would have shown. Least privilege that
# never asks who may become the account is not a claim about access at all.
gcloud iam service-accounts get-iam-policy "$delivery_email" \
    --project="$project" --format=json > "$work/delivery-account-iam.json"

auditor_token=$(gcloud auth print-access-token)
auditor_curl=$work/auditor-curl.conf
write_bearer_config "$auditor_curl" "$auditor_token" auditor

# Follow the pages. A shared drive caps permissions.list at 100 per page, and
# naming only permissions() in fields drops nextPageToken, so a truncated answer
# looked exactly like a complete one: a second grant for this account on page
# two left the count at one and the audit reported an exactness it never saw.
#
# One prefix for both permission listings, the drive's membership below and the
# per-file grants further down, because the second was a copy of the first and
# the copy is where the safeguard goes missing. Trimming `nextPageToken` out of
# either mask is silent: the API stops reporting the token, the page-two
# refusals below stop being reachable, and a link share on page two passes as a
# complete answer again. Sharing the spelling means a trim reaches both listings
# at once, where the tests can see it.
permissions_query="supportsAllDrives=true&pageSize=100&fields=nextPageToken,permissions"
permission_fields="id,type,role,emailAddress,domain,deleted"
drive_query="$permissions_query($permission_fields)"
drive_url="https://www.googleapis.com/drive/v3/files/$drive_id/permissions?$drive_query"
page=0
while :; do
    page=$((page + 1))
    [ "$page" -le 50 ] || {
        echo "Shared Drive permissions did not stop paging after $((page - 1)) pages" >&2
        exit 1
    }
    curl --fail --silent --show-error --max-time 20 --config "$auditor_curl" \
        "$drive_url" > "$work/drive-page-$page.json"
    token=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("nextPageToken") or "")' \
        "$work/drive-page-$page.json")
    [ -n "$token" ] || break
    drive_url="https://www.googleapis.com/drive/v3/files/$drive_id/permissions?$drive_query&pageToken=$token"
done

python3 - "$work/drive-permissions.json" "$work"/drive-page-*.json << 'PY_MERGE'
import json
import sys

out, *pages = sys.argv[1:]
permissions = []
for page in pages:
    permissions.extend(json.load(open(page, encoding="utf-8")).get("permissions", []))
json.dump({"permissions": permissions}, open(out, "w", encoding="utf-8"))
PY_MERGE

# The files on the drive, and the permissions on each. The membership audit
# above is about who holds the drive; this is about who can reach one recording,
# which is where `create_permission` in `src/delivery.rs` writes its grants and
# where a link share actually lives. The previous version refused `anyone` at
# the drive root and said that settled bearer-URL access, which was an
# overclaim: only users and groups can be drive members, so that branch could
# not fire for the case it named.
#
# Paged, with a ceiling. The first version refused the second page outright on
# the reasoning that a day of recordings fits in one, which is a false-fail the
# moment a busy day produces a hundred and one of them. The ceiling stays,
# because this walks a tree while an operator waits and an unbounded listing is
# how that becomes a hang, but it is high enough that reaching it means the
# check is pointed at the wrong drive.
files_query="corpora=drive&driveId=$drive_id&includeItemsFromAllDrives=true&supportsAllDrives=true"
files_query="$files_query&pageSize=100&fields=nextPageToken,files(id)"
files_url="https://www.googleapis.com/drive/v3/files?$files_query"
files_page=0
: > "$work/drive-file-ids.txt"
while :; do
    files_page=$((files_page + 1))
    [ "$files_page" -le 20 ] || {
        echo "the Shared Drive holds more than $(((files_page - 1) * 100)) files;" \
            "this audit reads the permissions on each, and a staging drive under a" \
            "24-hour retention rule holding that many is itself the finding" >&2
        exit 1
    }
    curl --fail --silent --show-error --max-time 20 --config "$auditor_curl" \
        "$files_url" > "$work/drive-files-$files_page.json"

    # The ids land in a file rather than a `for` word list. A word list is a
    # command substitution, and `set -e` does not reach into one, so a refusal
    # raised in there printed its message and let the run continue to exit 0.
    python3 -c 'import json,sys
for entry in json.load(open(sys.argv[1])).get("files", []):
    print(entry["id"])' "$work/drive-files-$files_page.json" >> "$work/drive-file-ids.txt"
    files_token=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("nextPageToken") or "")' \
        "$work/drive-files-$files_page.json")
    [ -n "$files_token" ] || break
    files_url="https://www.googleapis.com/drive/v3/files?$files_query&pageToken=$files_token"
done

# `permissionDetails` is the difference between a grant made on this file and
# one the drive's membership already carries, and it is the only field that says
# so. Without it every `user` row looked alike, so a recording shared directly
# with a colleague as a writer read the same as the delivery account's own
# inherited organizer, and passed. `expirationTime` alongside them, because a
# reader that never lapses is a standing copy of somebody's interview and the
# top-level listing does not say so.
file_query="$permissions_query($permission_fields,expirationTime,permissionDetails(permissionType,role,inherited))"

# curl writes each answer and nothing else runs in here. Reading them back is
# the main audit block's job, which it reaches by globbing these filenames, so a
# drive of a hundred recordings costs a hundred requests rather than a hundred
# requests and a hundred interpreter launches.
while read -r file_id; do
    [ -n "$file_id" ] || continue

    # The same alphabet `CODETRIAL_RECORDING_DRIVE_ID` is held to above, and for
    # the same reason twice over: this id is pasted into a query string, where a
    # `?` or a `#` would rewrite the request, and into a filename under `$work`,
    # where a `/` would put the answer somewhere the reader below never globs.
    # Refused rather than escaped, because an id outside this set is not a shape
    # Drive produces and guessing what it meant is how an audit reports on a
    # file it did not read.
    case "$file_id" in
        *[!A-Za-z0-9_-]*)
            echo "the Shared Drive listed a file id this audit cannot read: $file_id" >&2
            exit 1
            ;;
    esac
    curl --fail --silent --show-error --max-time 20 --config "$auditor_curl" \
        "https://www.googleapis.com/drive/v3/files/$file_id/permissions?$file_query" \
        > "$work/file-$file_id.json"
done < "$work/drive-file-ids.txt"

# Counted through `wc -l` with the padding stripped: only GNU `wc` writes a bare
# number when it reads a redirect, and the BSD one pads to a width, which put
# eight spaces in the middle of this sentence.
files_read=$(wc -l < "$work/drive-file-ids.txt" | tr -d ' \t')
echo "read the permissions on $files_read Shared Drive files"

python3 - "$delivery_email" "$work/bucket-iam.json" "$work/ancestors-iam.json" \
    "$work/bucket.json" "$work/drive-permissions.json" "$project_number" \
    "$work/delivery-account-iam.json" "$work" << 'PY'
import glob
import json
import os
import sys
from datetime import datetime, timedelta, timezone

(
    delivery,
    bucket_iam_path,
    ancestors_iam_path,
    bucket_path,
    drive_path,
    project_number,
    delivery_account_iam_path,
    work,
) = sys.argv[1:]
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
delivery_account_iam = read(delivery_account_iam_path)

# One answer per file, named by the id the shell asked for, so the id comes back
# off the basename and no second pass had to write it down. `glob` answers an
# empty list on a drive with nothing delivered yet, which is the normal state of
# a freshly provisioned one and would have been a missing-file error for any
# spelling that passed the names in.
drive_files = {
    os.path.basename(path)[len("file-") : -len(".json")]: read(path)
    for path in sorted(glob.glob(os.path.join(work, "file-*.json")))
}

# Which member spellings this script can resolve, rather than which ones it
# cannot. The denylist this replaces had grown to eight prefixes and a
# substring in one sitting, and every spelling nobody had thought of failed
# open: an unrecognised member was silently filed as "not the delivery account"
# and never reached the verdict. IAM member types are added by Google, so that
# list was structurally behind, and `projectEditor:` sat outside it for as long
# as this check has existed while granting its role to every project editor.
#
# The inverted form is also smaller, and it subsumes the one case that proves a
# prefix test cannot work on its own: `serviceAccount:PROJECT.svc.id.goog[NS/SA]`
# is Google's legacy member for every GKE pod running as a Kubernetes service
# account, so it names a set while sitting under an allowed prefix. A bracket
# cannot appear in an address, which is what separates the two.
RESOLVABLE_PREFIXES = (
    "user:",
    "serviceAccount:",
    "deleted:user:",
    "deleted:serviceAccount:",
)


def names_one_identity(principal):
    """One account this script could go and look at, or something broader."""
    if not principal.startswith(RESOLVABLE_PREFIXES):
        return False
    address = principal.split(":", 1)[1]
    return "@" in address and "[" not in address


def ensure_auditable(policy):
    """Refuse a policy holding a grant this script cannot read exactly.

    Whole-policy, not per-member. A member it cannot resolve either includes
    every authenticated service account, needs a directory lookup to rule this
    one out, or is a spelling nobody here has seen; in all three cases treating
    it as unrelated would turn the audit into a guess. That last case is why
    the test is an allowlist: the version that named the broad spellings
    instead filed every unknown one as harmless.
    """
    for binding in policy.get("bindings", []):
        indirect = [
            principal
            for principal in binding.get("members", [])
            if not names_one_identity(principal)
        ]
        if indirect:
            # Named, because the operator has to go and look. Whether the
            # delivery account is inside one of these is a directory question
            # this script cannot ask, and a role granted here reaches the
            # bucket whatever the bucket policy says.
            raise SystemExit(
                f"{binding.get('role')} is granted to {', '.join(indirect)}, which may or may not "
                f"contain {member}; resolve the membership or grant the role directly"
            )
        if member in binding.get("members", []) and binding.get("condition") is not None:
            raise SystemExit(f"{member} has a conditional IAM binding; exact access is not auditable")


def member_roles(policy):
    """The roles this member holds, once the policy is known to be readable."""
    return [
        binding.get("role")
        for binding in policy.get("bindings", [])
        if member in binding.get("members", [])
    ]

ensure_auditable(bucket_iam)
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
    ensure_auditable(policy)
    roles = member_roles(policy)
    if roles:
        ancestor_roles.append((label, roles))
if ancestor_roles:
    raise SystemExit(f"{member} has forbidden ancestor IAM roles: {ancestor_roles!r}")

# Nobody may become the delivery account. A binding on the account's own
# resource is how impersonation is granted, and it appears in none of the
# policies above, so every check so far could pass while a person held
# `roles/iam.serviceAccountTokenCreator` and could mint tokens for the identity
# that holds objectAdmin on the recordings and organizer on the drive. The
# whole policy is refused rather than a named list of roles: TokenCreator,
# ServiceAccountUser, the two key-admin roles and `setIamPolicy` on the
# resource all reach the same place by different routes, and a dedicated
# delivery account has no reason to carry any binding at all.
ensure_auditable(delivery_account_iam)
impersonators = []
for binding in delivery_account_iam.get("bindings", []):
    # `deleted:` members are tombstones IAM keeps in the policy after the
    # principal is gone. They grant nothing, and refusing over one would fail an
    # account whose stray grant had already been removed, which is exactly the
    # state this check is asking the operator to reach.
    live = [
        principal
        for principal in binding.get("members", [])
        if not principal.startswith("deleted:")
    ]
    if live:
        impersonators.append((binding.get("role"), live))
if impersonators:
    raise SystemExit(
        f"the delivery account's own IAM policy grants {impersonators!r}; a binding here "
        "lets its holder mint tokens for the recording identity, which no policy above "
        "would show, so the account must carry none"
    )

owner = bucket.get("projectNumber")
if not project_number or str(owner) != str(project_number):
    raise SystemExit(f"bucket belongs to project number {owner!r}, not {project_number!r}")

iam_configuration = bucket.get("iamConfiguration", {})
if iam_configuration.get("uniformBucketLevelAccess", {}).get("enabled") is not True:
    raise SystemExit("bucket must enable Uniform Bucket-Level Access")

# Uniform access can be turned back off within 90 days of being switched on, so
# on a freshly created bucket it is a setting rather than a property. The lock
# time is when that window closes; it is reported rather than refused, because
# refusing would fail every bucket for its first three months, which is every
# bucket this check will ever see on the day it is provisioned.
locked_until = iam_configuration.get("uniformBucketLevelAccess", {}).get("lockedTime")
if locked_until:
    print(f"uniform bucket-level access can be reverted until {locked_until}")

# The one setting that survives a later mistake. Without it, a grant to
# allUsers is a thing somebody can still make; `inherited` means the
# organization policy is carrying it and this bucket is not.
prevention = iam_configuration.get("publicAccessPrevention")
if prevention != "enforced":
    # `inherited` can be safe, when an ancestor organization policy enforces
    # prevention. This script does not read that policy and will not certify a
    # protection it has not seen, so it refuses rather than warns: the remedy is
    # one setting on this bucket, it is strictly safer than what it replaces,
    # and an operator is never stuck here.
    raise SystemExit(
        f"bucket public access prevention is {prevention!r}, not 'enforced'; set it on "
        "the bucket, because inherited is only safe if an ancestor organization policy "
        "enforces it and this audit does not read that policy"
    )

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

# A one-day Delete rule is not on its own a promise that anything is deleted,
# and the rule alone was being read as if it were. Each setting below is a way a
# bucket passes every check above while a candidate's recording is still
# retrievable a week later, which is the one guarantee this environment exists
# to make.
#
# Soft delete is the one that fires by default: Cloud Storage enables it on new
# buckets with a seven-day retention, and a lifecycle Delete moves an object
# into exactly that state rather than ending it. So a bucket nobody
# deliberately configured keeps every recording restorable for a week after the
# rule has run.
#
# Absent is refused rather than accepted. Whether disabling the policy omits the
# field or reports a zero duration is not something this script can settle from
# outside, and resolving that ambiguity in the passing direction would mean an
# audit whose whole point is that soft delete is off reporting success without
# having seen it off. `str()` because the JSON API types the duration as a
# number and gcloud has rendered it both ways.
soft_delete = bucket.get("softDeletePolicy", {}).get("retentionDurationSeconds")
if soft_delete is None:
    raise SystemExit(
        "could not determine the bucket's soft-delete state: no "
        "softDeletePolicy.retentionDurationSeconds in the bucket resource, and an "
        "absent field is not proof that soft delete is off"
    )
if str(soft_delete) != "0":
    raise SystemExit(
        f"bucket retains soft-deleted objects for {soft_delete}s, so the lifecycle "
        "rule would leave every recording restorable for that long; set the "
        "soft-delete retention duration to 0"
    )

# Three ways to defer the Delete action rather than forbid it, which comes to
# the same thing here: the object stays until the deferral ends, and none of
# these ends within a day. A locked retention policy cannot be shortened
# afterwards at all. Per-object holds and per-object retention do not appear in
# the bucket resource and so are not visible here, which is why
# `defaultEventBasedHold` matters most of the three: it is the one that puts a
# hold on every object written from now on.
def refuse_deferral(field, description):
    raise SystemExit(
        f"bucket has {description} ({field}={bucket.get(field)!r}), which defers the "
        "lifecycle Delete past the retention this environment promises; the staging "
        "bucket must carry none"
    )


if bucket.get("retentionPolicy"):
    refuse_deferral("retentionPolicy", "a retention policy")
if (bucket.get("objectRetention") or {}).get("mode") == "Enabled":
    refuse_deferral("objectRetention", "object retention")
if bucket.get("defaultEventBasedHold") is True:
    refuse_deferral("defaultEventBasedHold", "a default event-based hold")

# With versioning on, deleting an object writes a noncurrent version instead of
# removing it. The lifecycle rule does reach those versions, but only on a
# second pass, so a recording outlives the day the rule is written for. The
# sharper reason is this repository's own: `delete_object` in `src/delivery.rs`
# issues the object delete with no `generation`, which under versioning archives
# the live version rather than ending it, so turning versioning on converts the
# server's authoritative delete into a no-op nothing else would notice.
if bucket.get("versioning", {}).get("enabled") is True:
    raise SystemExit("bucket must not enable Object Versioning")

# Every active permission on the drive, not only the rows naming the delivery
# account. The audit used to filter on the delivery address first and count
# second, so "exactly one active organizer" meant "exactly one for this
# account" and said nothing about who else held the drive. The message claimed
# the stronger thing.
#
# One branch rather than three: this drive stages candidate recordings and
# nothing else, so its membership has to be individually named accounts, and
# `group`, `domain`, `anyone` and any type Google adds later all fall out of
# the same test. A group is a legitimate way to run a shared drive in general,
# which is why the remedy is spelled out rather than called unauditable.
#
# The scope, stated because the previous wording overclaimed it: this is the
# drive's own membership. Link sharing on delivered recordings lives on the
# permissions of the files inside the drive, which this audit does not list and
# does not speak for.
active = [
    permission
    for permission in drive.get("permissions", [])
    if not permission.get("deleted")
]
unnamed = [
    permission for permission in active if permission.get("type") != "user"
]
if unnamed:
    described = ", ".join(
        f"{permission.get('type')} "
        f"{permission.get('emailAddress') or permission.get('domain') or 'anyone'}"
        f" as {permission.get('role')}"
        for permission in unnamed
    )
    raise SystemExit(
        f"the Shared Drive has non-user members ({described}); this drive's "
        "membership must be individually named accounts, so that the audit can say "
        "who holds it"
    )

# The retention the contract promises, which is `RETENTION_SECONDS` in
# `src/recording/queue.rs` said again here because a shell audit cannot read a
# Rust constant. `tests/test_recording_provision_check.py` pins this line to
# that one, so the copy cannot drift without a test saying so.
retention = timedelta(hours=24)

# Slack on the bound, and only on this bound. Delivery stamps the expiry as its
# own host's now plus the retention, and this audit compares that against the
# auditor host's clock, so an exact ceiling refuses a correctly provisioned
# drive whenever the two disagree by a second. The acceptance checks in
# `scripts/recording-integration.sh` and `tests/recording_integration.rs` carry
# no such slack and want none: every stamp they compare was written by one host
# in one run, so there is no skew for slack to absorb.
skew = timedelta(minutes=5)

# One instant for the whole audit, read before the loop. Recomputed per
# permission, a drive of a hundred files judged its last file against a later
# ceiling than its first, so the same grant could pass or fail depending on
# where in the listing it sat.
ceiling = datetime.now(timezone.utc) + retention + skew


def aware_expiry(value):
    """`expirationTime` as an aware datetime, or None if it is not one.

    A naive stamp is refused here rather than compared below, because comparing
    it against an aware `now` raises out of the script as a traceback instead
    of as a refusal.

    The same normalisation as `parse` in `scripts/recording-integration.sh`,
    which reads this same field off the same API. Not shared, because these are
    two standalone scripts with no common file between them; kept in step by
    both being written against what `create_permission` in `src/delivery.rs`
    emits, which is the `Z` form.
    """
    if not isinstance(value, str):
        return None
    # `fromisoformat` learned `Z` in 3.11, and this script names no Python
    # floor. Lowercase `z` is as valid in RFC 3339 as the uppercase one and
    # Drive is not documented to prefer either, so both are folded rather than
    # one being refused as unparseable.
    text = value.strip()
    if text[-1:] in ("Z", "z"):
        text = text[:-1] + "+00:00"
    try:
        expiry = datetime.fromisoformat(text)
    except ValueError:
        return None
    # `tzinfo` alone: fromisoformat only ever attaches a `datetime.timezone`,
    # whose `utcoffset()` never answers None, so testing that too was testing
    # a branch nothing can reach.
    return expiry if expiry.tzinfo is not None else None


# Every permission on every file, which is where a delivered recording is
# actually reachable from. The delivery path writes one expiring `user` reader
# per file, so a `user` grant is expected and anything else is a link share or
# a domain-wide grant on a candidate's interview.
for file_id, answer in drive_files.items():
    # A page carrying a token is a listing whose end this script has not seen,
    # and it was being read as a complete one, so a link share sitting on page
    # two of a file's permissions passed. Refused rather than paged: a recording
    # with more than a hundred permissions on it is not a shape this environment
    # produces, and saying so beats walking it.
    if answer.get("nextPageToken"):
        raise SystemExit(
            f"the permissions on file {file_id} do not fit one page; this audit "
            "cannot see the end of them"
        )
    for permission in answer.get("permissions", []):
        if permission.get("deleted"):
            continue
        if permission.get("type") != "user":
            raise SystemExit(
                f"the file {file_id} on the Shared Drive is shared with "
                f"{permission.get('type')} as {permission.get('role')}; a delivered "
                "recording must be reachable only by named accounts, never by holding "
                "a link"
            )

        # A grant the drive's membership carries is the membership audit's, as
        # far as that audit goes: it refuses a non-user member and pins the
        # delivery account's sole organizer, and does not refuse an extra named
        # member. Who may sit on the drive is a provisioning policy rather than
        # something this run can settle. What is left is a grant somebody made
        # on this recording, and the
        # delivery path makes exactly one shape of those: an expiring reader for
        # the candidate. Anything else is a person given standing access to
        # somebody's interview.
        #
        # `inherited` is the discriminator, not `permissionType` on its own. A
        # grant made on a folder inside the drive surfaces on every file under
        # it as a `file` entry carrying `inherited`, so reading that as this
        # file's own delivery grant let one folder share pass as many. The
        # folder is listed by the same query and audited in its turn, where the
        # grant is its own and is not inherited, so nothing goes unchecked by
        # skipping it here.
        #
        # No details at all is still read as direct: the field is populated on
        # shared-drive items, and its absence is not a thing to resolve in the
        # passing direction.
        details = permission.get("permissionDetails")
        # Shape checked rather than assumed. A `details` that is not a list of
        # objects reached `.get` on a string and left the run on an
        # `AttributeError`, which exits non-zero and so fails closed, but a
        # traceback is not a finding: it says the audit broke, not what the
        # drive is carrying. Same principle `aware_expiry` states above.
        if details is not None and (
            not isinstance(details, list)
            or not all(isinstance(detail, dict) for detail in details)
        ):
            raise SystemExit(
                f"the file {file_id} on the Shared Drive reports permissionDetails "
                f"for {permission.get('emailAddress')} that this audit cannot read "
                f"({details!r})"
            )
        own = [
            detail
            for detail in details or []
            if detail.get("permissionType") == "file" and not detail.get("inherited")
        ]
        if details and not own:
            continue

        # Every source this grantee has on this file, judged together. Drive
        # reports one merged top-level role whose meaning it does not document,
        # so the entries are read instead; but reading only the `file` ones
        # dropped the membership half, and somebody holding `writer` on the
        # drive and an expiring `reader` here came out as a reader. They can
        # write the recording. Both roles are compared, and the entry that
        # decided this permission is a direct grant at all is still the
        # non-inherited `file` one, so a pure member never reaches this line:
        # the skip above returns it to the membership audit, which is where a
        # drive-wide role belongs.
        #
        # Every own entry, not the first. Drive is not documented to report one
        # `file` source per grantee, and judging on `own[0]` made the verdict
        # turn on the order of an array: the same pair of entries passed with
        # the reader ahead of the organizer and refused with them the other way
        # round. The other two listings here quantify over all their rows; so
        # does this one now.
        membership = [
            detail.get("role")
            for detail in details or []
            if detail.get("permissionType") == "member"
        ]
        granted = [detail.get("role") for detail in own] + membership or [
            permission.get("role")
        ]
        standing = [role for role in granted if role != "reader"]
        if standing:
            raise SystemExit(
                f"the file {file_id} on the Shared Drive is shared directly with "
                f"{permission.get('emailAddress')} as "
                f"{', '.join(str(role) for role in standing)}; the delivery path "
                "grants an expiring reader and nothing else"
            )

        # The grant has to lapse on its own. A reader that never expires is a
        # standing copy of somebody's interview, and the delivery path sets an
        # expiry precisely so that losing the sweeper does not mean losing the
        # retention promise.
        expiry = aware_expiry(permission.get("expirationTime"))
        if expiry is None:
            raise SystemExit(
                f"the file {file_id} on the Shared Drive has a direct reader for "
                f"{permission.get('emailAddress')} without a valid expirationTime "
                f"({permission.get('expirationTime')!r})"
            )
        # An upper bound only. A grant whose expiry has passed confers nothing,
        # which is the state this check exists to reach, and Drive does not
        # document how promptly it drops a lapsed permission from this listing;
        # refusing over one would fail a drive for having done the right thing.
        if expiry > ceiling:
            raise SystemExit(
                f"the file {file_id} on the Shared Drive has a direct reader for "
                f"{permission.get('emailAddress')} whose expirationTime "
                f"({permission.get('expirationTime')}) is more than "
                f"{int(retention.total_seconds() // 3600)} hours out"
            )

matches = [
    permission for permission in active if permission.get("emailAddress") == delivery
]
if len(matches) != 1 or matches[0].get("type") != "user" or matches[0].get("role") != "organizer":
    raise SystemExit(f"{delivery} must have exactly one active Shared Drive organizer permission")
PY

# What this line does and does not say, because earlier wordings claimed more
# than the checks above establish. It covers the delivery account's own grants,
# who may become that account, the bucket's own settings, the drive's
# membership, and the permissions on every file the drive holds.
#
# Three things it still does not. Roles held by other named principals are read
# for auditability rather than for whether they should hold them. Per-object
# holds and per-object retention are not in the bucket resource. And uniform
# access is read as enabled, not as irreversible: it can be turned back off
# within ninety days of being set, a window that is open on every bucket this
# check sees on the day it is provisioned, so the lock time is printed instead
# of refused. A direct named grant on one recording is deliberately allowed,
# because the delivery path writes exactly one per file and Recording 7b is what
# proves there is exactly one.
echo "the delivery account's grants and the bucket's retention settings are least-privilege"

# Access is checked as the delivery account itself, not inferred from the
# auditor's broad ability to inspect policy.
gcloud auth activate-service-account --key-file="$delivery_key" --quiet > /dev/null
gcloud storage ls "gs://$bucket" > /dev/null
echo "GCS bucket is accessible"
delivery_token=$(gcloud auth print-access-token)
delivery_curl=$work/delivery-curl.conf
write_bearer_config "$delivery_curl" "$delivery_token" delivery
curl --fail --silent --show-error --max-time 20 --config "$delivery_curl" \
    "https://www.googleapis.com/drive/v3/files?corpora=drive&driveId=$drive_id&includeItemsFromAllDrives=true&supportsAllDrives=true&pageSize=1&fields=files(id)" \
    > /dev/null
echo "Shared Drive is accessible"

# Not `curl --fail`, which exits 0 on a 3xx when nothing follows it, so a
# redirecting origin used to satisfy this line without the template ever being
# fetched. The status is read and compared, and the body is then checked for the
# element the template declares, because a 200 serving somebody else's page is
# the other way this passed. `scripts/recording-integration.sh` proves the same
# origin the same way, and carried the same swallowed exit code until this
# change; the two are meant to stay in step.
template_url="$template_origin/recording/index.html"

# The exit code is kept, not discarded. `%{http_code}` is already 200 once the
# headers arrive, so a transfer that dies part-way through the body still prints
# 200 while curl exits non-zero, and swallowing that with `|| true` accepted a
# fraction of a page as proof the page is served. Measured: a stalled body gave
# `status=200`, exit 28, and 258 of 5258 bytes, which still carried the marker
# the grep below looks for. The `if` is what keeps `set -e` out of it, since a
# refused connection has to reach the status message rather than abort here.
if status=$(curl --silent --show-error --max-time 30 --output "$work/template.html" \
    --resolve "$template_name:$template_fetch_port:$template_address" \
    --write-out '%{http_code}' "$template_url"); then
    template_transfer=complete
else
    template_transfer=$?
fi
[ "$template_transfer" = complete ] || {
    echo "the template fetch did not complete: $template_url answered $status after" \
        "curl exit $template_transfer" >&2
    exit 1
}
[ "$status" = "200" ] || {
    echo "the template is not being served: $template_url answered $status" >&2
    exit 1
}
grep -q 'id="recording-ready"' "$work/template.html" || {
    echo "the page at $template_url is not the recording template" >&2
    exit 1
}
echo "Recording template is publicly reachable"
