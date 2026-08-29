#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
ENV_FILE="${CODETRIAL_ENV:-$ROOT/config/codetrial.env.local}"

# Named rather than sourced. The binary requires a config file of its own and no
# longer reads credentials out of the environment alone, so sourcing this one
# would leave `CODETRIAL_ENV` pointing somewhere the binary never looks. When it
# is absent the binary runs its own search and reports the miss.
if [ -f "$ENV_FILE" ]; then
  set -- --config "$ENV_FILE" "$@"
fi

cargo run --manifest-path "$ROOT/Cargo.toml" -- check-gemini "$@"
