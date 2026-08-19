#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
ENV_FILE="${CODETRIAL_ENV:-$ROOT/config/codetrial.env.local}"

if [ -f "$ENV_FILE" ]; then
  set -a
  . "$ENV_FILE"
  set +a
fi

cargo run --manifest-path "$ROOT/Cargo.toml" -- check-gemini "$@"
