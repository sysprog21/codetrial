#!/bin/sh
set -eu

# Checks the resources task 0 provisions without writing credentials or IDs to
# the repository. Use a service-account key outside this checkout.

need() {
  name=$1
  value=$2
  [ -n "$value" ] || {
    echo "missing required environment variable: $name" >&2
    exit 2
  }
}

for tool in gcloud curl mktemp; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "required command is unavailable: $tool" >&2
    exit 2
  }
done

need CODETRIAL_RECORDING_GCS_BUCKET "${CODETRIAL_RECORDING_GCS_BUCKET:-}"
need CODETRIAL_RECORDING_DRIVE_ID "${CODETRIAL_RECORDING_DRIVE_ID:-}"
need CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON "${CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON:-}"
service_account_json=$CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON
unset CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON
need CODETRIAL_RECORDING_TEMPLATE_BASE_URL "${CODETRIAL_RECORDING_TEMPLATE_BASE_URL:-}"

template_origin=${CODETRIAL_RECORDING_TEMPLATE_BASE_URL%/}
case "$template_origin" in
  https://*) ;;
  *)
    echo "template base URL must be an https origin" >&2
    exit 2
    ;;
esac
template_host=${template_origin#https://}
template_host_lower=$(printf '%s' "$template_host" | tr '[:upper:]' '[:lower:]')
case "$template_host_lower" in
  '' | :* | */* | *\?* | *\#* | *@* | \[* | localhost | localhost:* | localhost. | localhost.:* | *.localhost | *.localhost:* | *.localhost. | *.localhost.:* | 127.* | 0.0.0.0 | 0.0.0.0:*)
    echo "template base URL must not be loopback or include a path" >&2
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

umask 077
config_dir=$(mktemp -d)
trap 'rm -rf "$config_dir"' EXIT HUP INT TERM
export CLOUDSDK_CONFIG="$config_dir"
key_file="$config_dir/service-account.json"
printf '%s' "$service_account_json" >"$key_file"
unset service_account_json

gcloud auth activate-service-account \
  --key-file="$key_file" --quiet >/dev/null

gcloud storage ls "gs://$CODETRIAL_RECORDING_GCS_BUCKET" >/dev/null
echo "GCS bucket is accessible"

token=$(gcloud auth print-access-token)
curl_config="$config_dir/curl.conf"
printf 'header = "Authorization: Bearer %s"\n' "$token" >"$curl_config"
curl --fail --silent --show-error --max-time 20 \
  --config "$curl_config" \
  "https://www.googleapis.com/drive/v3/files?corpora=drive&driveId=$CODETRIAL_RECORDING_DRIVE_ID&includeItemsFromAllDrives=true&supportsAllDrives=true&pageSize=1&fields=files(id)" \
  >/dev/null
echo "Shared Drive is accessible"
