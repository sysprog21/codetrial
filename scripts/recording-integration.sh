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

usage() {
  cat >&2 <<'USAGE'
usage: recording-integration.sh --phase=media[,delivery,cleanup]

Environment, all required:
  CODETRIAL_RECORDING_INTEGRATION=1        this costs money and creates media
  CODETRIAL_RECORDING_GCS_BUCKET           the isolated staging bucket
  CODETRIAL_RECORDING_DRIVE_ID             the isolated Shared Drive
  CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON the service account, from outside the repo
  CODETRIAL_RECORDING_TEMPLATE_BASE_URL    the public origin serving the template
  LIVEKIT_URL, LIVEKIT_API_KEY, LIVEKIT_API_SECRET  the isolated project

Optional:
  CODETRIAL_RECORDING_ROOM_SECONDS         default 60, how long the room is up
  CODETRIAL_RECORDING_ACCEPTANCE_JSON      default target/recording-acceptance.json
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

# Checked before anything else, so a typo cannot half-run an acceptance. The
# phases are a fixed set rather than free text for the same reason:
# `--phase=med` should be a refusal, not a run that does nothing and exits 0.
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

need() {
  eval "value=\${$1:-}"
  [ -n "$value" ] || {
    echo "missing required environment variable: $1" >&2
    exit 2
  }
}
for name in \
  CODETRIAL_RECORDING_GCS_BUCKET \
  CODETRIAL_RECORDING_DRIVE_ID \
  CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON \
  CODETRIAL_RECORDING_TEMPLATE_BASE_URL \
  LIVEKIT_URL LIVEKIT_API_KEY LIVEKIT_API_SECRET; do
  need "$name"
done

# `ffprobe` is how the file is measured, and a run that gets to the end and then
# cannot measure anything has spent a provisioned project for nothing.
command -v ffprobe >/dev/null 2>&1 || {
  echo "required command is unavailable: ffprobe" >&2
  echo "install it with your package manager, for example: brew install ffmpeg" >&2
  exit 2
}
for tool in curl gcloud python3; do
  command -v "$tool" >/dev/null 2>&1 || {
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
template_origin=${CODETRIAL_RECORDING_TEMPLATE_BASE_URL%/}
work=$(mktemp -d)
object=

# One trap, on every exit, over what this script created: a temporary directory
# with a copy of the file in it.
#
# It deliberately does not delete the staged object. That object belongs to the
# recording pipeline, which deletes it itself when the delivery finishes or when
# retention comes for it, and a harness that removed it would be deleting media
# out from under the thing it is supposed to be proving. The cleanup phase is
# what checks that the pipeline did its own deleting.
cleanup() {
  status=$?
  rm -rf "$work"
  exit $status
}
trap cleanup EXIT INT TERM

# The template, before the room. A recording of a 404 is a recording, and it
# takes the whole timeout to find out.
template_url="$template_origin/recording/index.html"
status=$(curl -s -o "$work/template.html" -w '%{http_code}' --max-time 30 "$template_url" || true)
[ "$status" = "200" ] || {
  echo "the template is not being served: $template_url answered $status" >&2
  exit 1
}
grep -q 'id="recording-ready"' "$work/template.html" || {
  echo "the page at $template_url is not the recording template" >&2
  exit 1
}
echo "template ok: $template_url"

runs() {
  printf '%s' "$phases" | tr ',' '\n' | grep -qx "$1"
}

if runs media; then
  echo "the media phase needs an interview to record."
  cat >&2 <<'MEDIA'
Not automated here, and deliberately so: the recording under test is a person
sitting an interview, with a camera, a voice and an interviewer answering. What
this script cannot do is be that person.

Run one, with this checkout's server pointed at the isolated project:

  1. Start the server with the recording block configured.
  2. Take an interview with a camera on for at least ten seconds.
  3. End it, and wait for the recording to reach `ready`.
  4. Note the recording id from `GET /api/interviews/{id}/recording`.

Then measure the file this script can reach, with the id:

  CODETRIAL_RECORDING_ID=<id> ./scripts/recording-integration.sh --phase=media

The measurement below runs when CODETRIAL_RECORDING_ID is set.
MEDIA
  [ -n "${CODETRIAL_RECORDING_ID:-}" ] || exit 2

  recording_id=$CODETRIAL_RECORDING_ID
  object=${CODETRIAL_RECORDING_GCS_PREFIX:-codetrial}/$recording_id.mp4
  gcloud storage cp "gs://$CODETRIAL_RECORDING_GCS_BUCKET/$object" "$work/recording.mp4" >/dev/null || {
    echo "no staged object at gs://$CODETRIAL_RECORDING_GCS_BUCKET/$object" >&2
    exit 1
  }

  ffprobe -v error -print_format json -show_streams -show_format "$work/recording.mp4" \
    >"$work/probe.json"

  # Three numbers the file cannot answer: how long the room was up, how long the
  # candidate was on screen, and whether the local avatar was rendering. They
  # come from the operator running the interview, which is the ceiling of this
  # harness and is worth stating plainly: they are declarations, not
  # measurements, and somebody who declares them wrongly gets an acceptance that
  # says nothing. Absent, they are zero and false, so the failure mode of not
  # supplying them is a refusal rather than a pass.
  #
  # Making them measurements means the recording template reporting them from
  # inside the Egress browser, keyed to the recording id, through a route this
  # server would have to trust. That is a bigger piece of work than the rest of
  # this file and it belongs with the run itself.
  mkdir -p "$(dirname "$acceptance")"
  CODETRIAL_PROBE=$work/probe.json \
    CODETRIAL_RECORDING_ID=$recording_id \
    CODETRIAL_ROOM_SECONDS=$room_seconds \
    python3 - "$acceptance" <<'PY'
import json, os, sys

probe = json.load(open(os.environ["CODETRIAL_PROBE"]))
video = next((s for s in probe["streams"] if s.get("codec_type") == "video"), {})
audio = [s for s in probe["streams"] if s.get("codec_type") == "audio"]

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
    # Measured by the page, not by the file: an MP4 of a composited layout
    # cannot say whose face was in it.
    "candidate_video_seconds": float(os.environ.get("CODETRIAL_CANDIDATE_SECONDS", 0)),
    "audio_tracks": int(os.environ.get("CODETRIAL_AUDIO_TRACKS", len(audio))),
    "avatar_rendered": os.environ.get("CODETRIAL_AVATAR_RENDERED") == "true",
}
json.dump(document, open(sys.argv[1], "w"), indent=2)
print(json.dumps(document, indent=2))
PY
  echo "wrote $acceptance"
  echo "now run: CODETRIAL_RECORDING_INTEGRATION=1 cargo test --test recording_integration media_acceptance"
fi

if runs delivery || runs cleanup; then
  cat >&2 <<'PENDING'
The delivery and cleanup phases are not implemented here yet, and they need the
same credentialed interview the media phase needs. What they will assert: one
Drive file, a reader permission expiring within 24 hours, and no Drive file and
no GCS object after retention runs.
PENDING
  exit 2
fi
