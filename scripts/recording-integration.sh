#!/bin/sh
set -eu

# The credentialed acceptance for recording. Nothing in the default test suite
# runs this: it needs the isolated LiveKit project, staging bucket and Shared
# Drive from task 0, and it creates and deletes real media.
#
# The judgement lives in `tests/recording_integration.rs`, not here. This script
# gets a real recording made, measures the file with `ffprobe`, and writes what
# it measured; that test decides whether the numbers are acceptable. A harness
# whose rules are buried in shell nobody can run without credentials is a
# harness nobody can check.
#
# Phases, comma separated:
#
#   media    make a recording and measure the file (task 7a)
#   delivery let the delivery queue hand it over and time the completion (7b)
#   cleanup  expire it and prove both artifacts are gone (7b)
#
# What this script creates is a temporary directory, and its trap removes that
# on success and on failure both. The media itself belongs to the pipeline,
# which deletes it on its own schedule; the cleanup phase is what proves it did.

usage()
{
    cat >&2 << 'USAGE'
usage: recording-integration.sh --phase=media[,delivery,cleanup]

Environment, every phase:
  CODETRIAL_RECORDING_INTEGRATION=1        this costs money and creates media
  CODETRIAL_RECORDING_GCS_BUCKET           the isolated staging bucket
  CODETRIAL_RECORDING_DRIVE_ID             the isolated Shared Drive
  CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON the service account, from outside the repo
  CODETRIAL_RECORDING_ID                   the recording under acceptance; the media
                                           phase prints how to obtain it

Media phase only:
  CODETRIAL_RECORDING_TEMPLATE_BASE_URL    the public origin serving the template
  CODETRIAL_CANDIDATE_SECONDS              attested seconds of candidate camera on screen
  CODETRIAL_AUDIO_TRACKS                   attested count of distinct audio sources
  CODETRIAL_AVATAR_RENDERED                attested true or false: was the VRM rendering

Delivery phase only:
  CODETRIAL_RECORDING_RECIPIENT_EMAIL      the verified recipient address

Cleanup phase only:
  CODETRIAL_DB_PATH                        the isolated staging database

Optional:
  CODETRIAL_RECORDING_ROOM_SECONDS         default 60, how long the room is up
  CODETRIAL_RECORDING_ACCEPTANCE_JSON      default target/recording-acceptance.json
  CODETRIAL_RECORDING_GCS_PREFIX           default codetrial, the staged object prefix
  CODETRIAL_RECORDING_CLEANUP_TIMEOUT_SECONDS  default 300
USAGE
    exit 2
}

phases=
while [ $# -gt 0 ]; do
    case "$1" in
        --phase=*) phases=${1#--phase=} ;;
        -h | --help) usage ;;
        *)
            echo "unknown argument: $1" >&2
            usage
            ;;
    esac
    shift
done
[ -n "$phases" ] || usage

runs()
{
    case ",$phases," in
        *",$1,"*) return 0 ;;
        *) return 1 ;;
    esac
}

# Checked before anything else, so a typo cannot half-run an acceptance. The
# phases are a fixed set rather than free text for the same reason:
# `--phase=med` should be a refusal, not a run that does nothing and exits 0.
#
# Whitespace is refused rather than trimmed, because the two readers of this
# list tokenize differently: the loop below splits on it and `runs` does not.
# `--phase='media, delivery'` used to validate both and then run only the media
# phase, which is the half-run this check exists to prevent.
case "$phases" in
    *[[:space:]]*)
        echo "phases are comma separated with no spaces: $phases" >&2
        usage
        ;;
esac
for phase in $(printf '%s' "$phases" | tr ',' ' '); do
    case "$phase" in
        media | delivery | cleanup) ;;
        *)
            echo "unknown phase: $phase" >&2
            usage
            ;;
    esac
done

if [ "${CODETRIAL_RECORDING_INTEGRATION:-}" != "1" ]; then
    echo "refusing to run: set CODETRIAL_RECORDING_INTEGRATION=1 to say you have the isolated project" >&2
    exit 2
fi

need()
{
    eval "value=\${$1:-}"
    [ -n "$value" ] || {
        echo "missing required environment variable: $1" >&2
        exit 2
    }
}
for name in \
    CODETRIAL_RECORDING_GCS_BUCKET \
    CODETRIAL_RECORDING_DRIVE_ID \
    CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON; do
    need "$name"
done

# The template and the measurement belong to the media phase alone. A cleanup
# re-run polls two provider URLs for a 404; making it install ffmpeg and stand
# up a public origin first fails it on something it was never going to use.
#
# The room is nobody's business here. The operator's own server opens it, and
# this script only reads what that server has already staged, which is why it
# asks for no LiveKit credentials at all.
if runs media; then
    need CODETRIAL_RECORDING_TEMPLATE_BASE_URL

    # `ffprobe` is how the file is measured, and a run that gets to the end and
    # then cannot measure anything has spent a provisioned project for nothing.
    command -v ffprobe > /dev/null 2>&1 || {
        echo "required command is unavailable: ffprobe" >&2
        echo "install it with your package manager, for example: brew install ffmpeg" >&2
        exit 2
    }
fi
for tool in curl gcloud python3; do
    command -v "$tool" > /dev/null 2>&1 || {
        echo "required command is unavailable: $tool" >&2
        exit 2
    }
done

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

case "${CODETRIAL_RECORDING_ROOM_SECONDS:-60}" in
    '' | *[!0-9]*)
        echo "CODETRIAL_RECORDING_ROOM_SECONDS takes seconds" >&2
        exit 2
        ;;
esac
room_seconds=${CODETRIAL_RECORDING_ROOM_SECONDS:-60}
acceptance=${CODETRIAL_RECORDING_ACCEPTANCE_JSON:-$ROOT/target/recording-acceptance.json}
template_origin=${CODETRIAL_RECORDING_TEMPLATE_BASE_URL:-}
template_origin=${template_origin%/}
gcs_prefix=${CODETRIAL_RECORDING_GCS_PREFIX:-codetrial}
while [ "${gcs_prefix%/}" != "$gcs_prefix" ]; do gcs_prefix=${gcs_prefix%/}; done
umask 077
work=$(mktemp -d)

# One trap, on every exit, over what this script created: a temporary directory
# with a copy of the file in it.
#
# It deliberately does not delete the staged object. That object belongs to the
# recording pipeline, which deletes it itself when the delivery finishes or when
# retention comes for it, and a harness that removed it would be deleting media
# out from under the thing it is supposed to be proving. The cleanup phase is
# what checks that the pipeline did its own deleting.
cleanup()
{
    status=$?
    rm -rf "$work"
    exit $status
}
trap cleanup EXIT INT TERM

# The egress service account is also the delivery account. Keep its key and
# short-lived token in the private work directory rather than in a developer's
# active gcloud configuration or in a command line.
export CLOUDSDK_CONFIG="$work/gcloud"
mkdir "$CLOUDSDK_CONFIG"
key_file=$work/recording-service-account.json
printf '%s' "$CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON" > "$key_file"
unset CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON
gcloud auth activate-service-account --key-file="$key_file" --quiet > /dev/null
token=$(gcloud auth print-access-token)
curl_config=$work/curl.conf
printf 'header = "Authorization: Bearer %s"\n' "$token" > "$curl_config"

drive_get()
{
    curl --fail --silent --show-error --max-time 20 --config "$curl_config" "$1"
}

update_acceptance()
{
    python3 - "$acceptance" "$1" << 'PY'
import json
import sys

path, update_path = sys.argv[1:]
try:
    document = json.load(open(path, encoding="utf-8"))
    update = json.load(open(update_path, encoding="utf-8"))
except (OSError, json.JSONDecodeError) as error:
    raise SystemExit(f"cannot update recording acceptance: {error}")
if document.get("recording_id") != update.pop("recording_id"):
    raise SystemExit("acceptance document belongs to a different recording")
document.update(update)
with open(path, "w", encoding="utf-8") as output:
    json.dump(document, output, indent=2)
    output.write("\n")
PY
}

# The template, before the room. A recording of a 404 is a recording, and it
# takes the whole timeout to find out.
if runs media; then
    template_url="$template_origin/recording/index.html"

    # The exit code is kept rather than discarded. `%{http_code}` is already 200
    # once the headers arrive, so a transfer that dies part-way through the body
    # still prints 200 while curl exits non-zero, and `|| true` accepted that
    # fraction of a page as proof the page is served. Measured against a
    # stalling server: status 200, exit 28, 258 of 5258 bytes, and the fragment
    # still carried the marker the grep below looks for.
    if status=$(curl -s -o "$work/template.html" -w '%{http_code}' --max-time 30 "$template_url"); then
        transfer_exit=
    else
        transfer_exit=$?
    fi
    [ -z "$transfer_exit" ] || {
        echo "the template fetch did not complete: $template_url answered $status" \
            "after curl exit $transfer_exit" >&2
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
    echo "template ok: $template_url"

    echo "the media phase needs an interview to record."
    cat >&2 << 'MEDIA'
Not automated here, and deliberately so: the recording under test is a person
sitting an interview, with a camera, a voice and an interviewer answering. What
this script cannot do is be that person.

Run one, with this checkout's server pointed at the isolated project:

  1. Start the server with the recording block configured.
  2. Take an interview with a camera on for at least ten seconds.
  3. End it, and wait for the recording to reach `ready`.
  4. Note the recording id from `GET /api/interviews/{id}/recording`.

Then measure the file this script can reach, with the id and the three facts
the file cannot answer for itself:

  CODETRIAL_RECORDING_ID=<id> \
  CODETRIAL_CANDIDATE_SECONDS=<seconds the candidate was on screen> \
  CODETRIAL_AUDIO_TRACKS=<distinct audio sources heard> \
  CODETRIAL_AVATAR_RENDERED=<true|false> \
    ./scripts/recording-integration.sh --phase=media

The measurement below runs when CODETRIAL_RECORDING_ID is set.
MEDIA
    [ -n "${CODETRIAL_RECORDING_ID:-}" ] || exit 2
    for name in \
        CODETRIAL_CANDIDATE_SECONDS \
        CODETRIAL_AUDIO_TRACKS \
        CODETRIAL_AVATAR_RENDERED; do
        need "$name"
    done
    case "$CODETRIAL_CANDIDATE_SECONDS" in
        . | *[!0-9.]* | *.*.*)
            echo "CODETRIAL_CANDIDATE_SECONDS must be a non-negative number" >&2
            exit 2
            ;;
    esac
    case "$CODETRIAL_AUDIO_TRACKS" in
        *[!0-9]*)
            echo "CODETRIAL_AUDIO_TRACKS must be a non-negative integer" >&2
            exit 2
            ;;
    esac
    case "$CODETRIAL_AVATAR_RENDERED" in
        true | false) ;;
        *)
            echo "CODETRIAL_AVATAR_RENDERED must be true or false" >&2
            exit 2
            ;;
    esac

    recording_id=$CODETRIAL_RECORDING_ID
    object=$gcs_prefix/$recording_id.mp4
    gcloud storage cp "gs://$CODETRIAL_RECORDING_GCS_BUCKET/$object" "$work/recording.mp4" > /dev/null || {
        echo "no staged object at gs://$CODETRIAL_RECORDING_GCS_BUCKET/$object" >&2
        exit 1
    }

    ffprobe -v error -print_format json -show_streams -show_format "$work/recording.mp4" \
        > "$work/probe.json"

    # Three facts the file cannot answer: how long the candidate was on screen,
    # how many sources were heard, and whether the local avatar rendered. They
    # are operator attestations, not measurements. Requiring each declaration
    # keeps a missing attestation from borrowing a plausible value from ffprobe.
    #
    # Making them measurements means the recording template reporting them from
    # inside the Egress browser, keyed to the recording id, through a route this
    # server would have to trust. That is a bigger piece of work than the rest
    # of this file and it belongs with the run itself.
    mkdir -p "$(dirname "$acceptance")"
    CODETRIAL_PROBE=$work/probe.json \
        CODETRIAL_RECORDING_ID=$recording_id \
        CODETRIAL_ROOM_SECONDS=$room_seconds \
        python3 - "$acceptance" << 'PY'
import json, os, sys

probe = json.load(open(os.environ["CODETRIAL_PROBE"]))
video = next((s for s in probe["streams"] if s.get("codec_type") == "video"), {})

def rate(value):
    if not value or "/" not in value:
        return 0.0
    numerator, denominator = value.split("/", 1)
    denominator = float(denominator or 0)
    return float(numerator) / denominator if denominator else 0.0

document = {
    "recording_id": os.environ["CODETRIAL_RECORDING_ID"],
    "width": int(video.get("width") or 0),
    "height": int(video.get("height") or 0),
    "fps": rate(video.get("avg_frame_rate") or video.get("r_frame_rate")),
    "bitrate_kbps": float(probe.get("format", {}).get("bit_rate") or 0) / 1000.0,
    "duration_seconds": float(probe.get("format", {}).get("duration") or 0),
    "expected_duration_seconds": float(os.environ["CODETRIAL_ROOM_SECONDS"]),
    # Operator attestations, not file measurements: an MP4 of a composited
    # layout cannot identify whose face or sources were present.
    "candidate_video_seconds": float(os.environ["CODETRIAL_CANDIDATE_SECONDS"]),
    "audio_tracks": int(os.environ["CODETRIAL_AUDIO_TRACKS"]),
    "avatar_rendered": os.environ["CODETRIAL_AVATAR_RENDERED"] == "true",
}
json.dump(document, open(sys.argv[1], "w"), indent=2)
print(json.dumps(document, indent=2))
PY
    echo "wrote $acceptance"
    echo "now run: CODETRIAL_RECORDING_INTEGRATION=1 cargo test --test recording_integration media_acceptance"
fi

if runs delivery || runs cleanup; then
    need CODETRIAL_RECORDING_ID
    if runs delivery; then
        need CODETRIAL_RECORDING_RECIPIENT_EMAIL
    fi
    if runs cleanup; then
        need CODETRIAL_DB_PATH
    fi
    recording_id=$CODETRIAL_RECORDING_ID
    filename=$recording_id.mp4
    object=$gcs_prefix/$filename
    query=$(
        python3 - "$filename" "$CODETRIAL_RECORDING_DRIVE_ID" << 'PY'
import sys
from urllib.parse import urlencode

name, drive_id = sys.argv[1:]
print(urlencode({
    "q": f"name = '{name}' and trashed = false",
    "corpora": "drive",
    "driveId": drive_id,
    "includeItemsFromAllDrives": "true",
    "supportsAllDrives": "true",
    "fields": "files(id,name,createdTime)",
}))
PY
    )
    drive_get "https://www.googleapis.com/drive/v3/files?$query" > "$work/drive-files.json"
    drive_file_id=$(
        python3 - "$work/drive-files.json" "$work/delivery.json" "$recording_id" "$filename" << 'PY'
import json
import sys

listing_path, delivery_path, recording_id, filename = sys.argv[1:]
files = json.load(open(listing_path, encoding="utf-8")).get("files", [])
matches = [item for item in files if item.get("name") == filename and item.get("id")]
if len(matches) != 1:
    raise SystemExit(f"expected exactly one Drive file named {filename}, found {len(matches)}")
file = matches[0]
if not isinstance(file.get("createdTime"), str):
    raise SystemExit("Drive file has no createdTime")
delivery = {
    "recording_id": recording_id,
    "drive_file_id": file["id"],
    "delivery_created_at": file["createdTime"],
}
json.dump(delivery, open(delivery_path, "w", encoding="utf-8"))
print(delivery["drive_file_id"])
PY
    )

    # Only delivery judges the grant. Cleanup runs after it, and the whole point
    # of cleanup is that the grant has lapsed by then, so requiring a live one
    # would refuse to prove the deletion this pipeline exists to prove.
    if runs delivery; then
        permission_query='supportsAllDrives=true&pageSize=100&fields=nextPageToken,permissions(id,type,role,emailAddress,expirationTime,deleted)'
        drive_get "https://www.googleapis.com/drive/v3/files/$drive_file_id/permissions?$permission_query" > "$work/permissions.json"
        python3 - "$work/delivery.json" "$work/permissions.json" "$CODETRIAL_RECORDING_RECIPIENT_EMAIL" << 'PY'
import datetime as dt
import json
import sys

delivery = json.load(open(sys.argv[1], encoding="utf-8"))
listing = json.load(open(sys.argv[2], encoding="utf-8"))
if listing.get("nextPageToken"):
    raise SystemExit("the delivered file has more permissions than one page; exactly one reader is not observable")
permissions = listing.get("permissions", [])


def address(item):
    """The permission's address, folded: Drive stores what it normalised."""
    value = item.get("emailAddress")
    return value.strip().casefold() if isinstance(value, str) else None


# Every reader on the file, not the recipient's readers. Selecting by address
# first made "exactly one reader" mean "exactly one of theirs", so a reader
# granted to a second account read as a correctly scoped delivery.
readers = [
    item
    for item in permissions
    if not item.get("deleted") and item.get("role") == "reader"
]
if (
    len(readers) != 1
    or readers[0].get("type") != "user"
    or address(readers[0]) != sys.argv[3].strip().casefold()
    or not isinstance(readers[0].get("expirationTime"), str)
):
    raise SystemExit("expected exactly one expiring reader permission for CODETRIAL_RECORDING_RECIPIENT_EMAIL")
def parse(value):
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


created, checked, expires = (
    parse(delivery["delivery_created_at"]),
    dt.datetime.now(dt.timezone.utc),
    parse(readers[0]["expirationTime"]),
)
if not (created < expires and checked < expires <= checked + dt.timedelta(hours=24)):
    raise SystemExit("reader permission is not active and within 24 hours of verification")
delivery["permission_expires_at"] = readers[0]["expirationTime"]
delivery["delivery_verified_at"] = checked.isoformat()
json.dump(delivery, open(sys.argv[1], "w", encoding="utf-8"))
PY
        echo "delivery verified: $drive_file_id"
    else
        echo "cleanup will watch Drive file $drive_file_id"
    fi
    update_acceptance "$work/delivery.json"

    if runs cleanup; then
        "$ROOT/scripts/recording-cleanup.sh" --db "$CODETRIAL_DB_PATH" --expire "$recording_id"
        timeout=${CODETRIAL_RECORDING_CLEANUP_TIMEOUT_SECONDS:-300}
        case "$timeout" in '' | *[!0-9]*)
            echo "CODETRIAL_RECORDING_CLEANUP_TIMEOUT_SECONDS takes seconds" >&2
            exit 2
            ;;
        esac
        deadline=$(($(date +%s) + timeout))

        # The object name carries a slash the JSON API wants encoded, and it
        # does not change between polls, so encode it once rather than once a
        # poll.
        escaped_object=$(
            python3 - "$object" << 'PY'
import sys
from urllib.parse import quote

print(quote(sys.argv[1], safe=""))
PY
        )
        drive_url="https://www.googleapis.com/drive/v3/files/$drive_file_id?supportsAllDrives=true"
        gcs_url="https://storage.googleapis.com/storage/v1/b/$CODETRIAL_RECORDING_GCS_BUCKET/o/$escaped_object"
        while :; do
            drive_status=$(curl -s -o /dev/null -w '%{http_code}' --max-time 20 --config "$curl_config" "$drive_url" || true)
            gcs_status=$(curl -s -o /dev/null -w '%{http_code}' --max-time 20 --config "$curl_config" "$gcs_url" || true)
            [ "$drive_status" = 404 ] && [ "$gcs_status" = 404 ] && break
            [ "$(date +%s)" -lt "$deadline" ] || {
                echo "cleanup timed out: Drive=$drive_status GCS=$gcs_status" >&2
                exit 1
            }
            sleep 5
        done
        python3 - "$work/cleanup.json" "$recording_id" << 'PY'
import json
import sys

path, recording_id = sys.argv[1:]
cleanup = {"drive_file_absent": True, "gcs_object_absent": True}
json.dump({"recording_id": recording_id, "cleanup_status": cleanup}, open(path, "w"))
PY
        update_acceptance "$work/cleanup.json"
        echo "cleanup verified: Drive file and GCS object are absent"
        echo "now run: CODETRIAL_RECORDING_INTEGRATION=1 CODETRIAL_RECORDING_LIFECYCLE_ACCEPTANCE=1 cargo test --test recording_integration lifecycle_acceptance"
    fi
fi
