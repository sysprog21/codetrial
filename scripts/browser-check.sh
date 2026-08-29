#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/scripts/session-cookie.sh"
BROWSER_CHECK_AGENT=${BROWSER_CHECK_AGENT:-home}

# Flows: voice (default), report, barge, avatar. `avatar` is dispatched before
# the agent mode, so it needs no credentials and ignores BROWSER_CHECK_AGENT.
BROWSER_CHECK_FLOW=${BROWSER_CHECK_FLOW:-voice}
case $BROWSER_CHECK_FLOW in
  voice | report | barge | avatar) ;;
  *)
    echo "BROWSER_CHECK_FLOW must be voice, report, barge, or avatar." >&2
    exit 2
    ;;
esac
if [ "${BROWSER_CHECK_COMPILER_EXPLORER_BASE_URL+x}" ]; then
  COMPILER_EXPLORER_BASE_URL_ARG=$BROWSER_CHECK_COMPILER_EXPLORER_BASE_URL
else
  COMPILER_EXPLORER_BASE_URL_ARG=__default__
fi
TMP=$(mktemp -d)
SERVER_LOG="$TMP/web.log"
BARGE_AUDIO_FILE=${BROWSER_CHECK_BARGE_AUDIO_FILE:-}
USE_AGENT_SERVE=0
DISPATCH_MODE=0
LOGIN_DB_PATH="$TMP/accounts.db"
LOGIN_SESSION_SECRET=browser-check-session
CONFIG_PATH="$TMP/codetrial.env.local"

cleanup() {
  if [ "${SERVER_PID:-}" ]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -r "$TMP" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

if [ "$BROWSER_CHECK_AGENT" = "rust" ] || [ "$BROWSER_CHECK_AGENT" = "dispatch" ]; then
  CONFIG_ENV=${CODETRIAL_CONFIG_ENV:-}
  if [ -z "$CONFIG_ENV" ]; then
    for candidate in "$ROOT/config/codetrial.env.local" "$ROOT/codetrial.env.local"; do
      [ -f "$candidate" ] && { CONFIG_ENV=$candidate; break; }
    done
  fi
  if [ -f "${CONFIG_ENV:-}" ]; then
    set -a
    . "$CONFIG_ENV"
    set +a
  fi
  : "${LIVEKIT_URL:?set LIVEKIT_URL for credentialed browser check}"
  : "${LIVEKIT_API_KEY:?set LIVEKIT_API_KEY for credentialed browser check}"
  : "${LIVEKIT_API_SECRET:?set LIVEKIT_API_SECRET for credentialed browser check}"
  : "${GOOGLE_API_KEY:?set GOOGLE_API_KEY for credentialed browser check}"
fi

case $BROWSER_CHECK_AGENT in
  home | offline)
    printf '%s\n' \
      'LIVEKIT_URL=wss://example.livekit.cloud' \
      'LIVEKIT_API_KEY=browser-check-key' \
      'LIVEKIT_API_SECRET=browser-check-secret' >"$CONFIG_PATH"
    ;;
  rust | dispatch)

    # The operator's own file, named rather than copied. `provider_dir` is the
    # config file's directory, so a copy in $TMP has no `codetrial.env.<id>`
    # siblings beside it and the pool collapses to the primary project. The
    # credentialed check is the only one that can exercise pooling at all, and a
    # copy is exactly what stops it: an external server minting a pooled room
    # would hand it to an agent that has never heard of that provider.
    #
    # A config assembled from the environment is the fallback, for the runner
    # that has the four variables and no file to point at.
    if [ -f "${CONFIG_ENV:-}" ]; then
      CONFIG_PATH=$CONFIG_ENV
    else
      printf 'LIVEKIT_URL=%s\nLIVEKIT_API_KEY=%s\nLIVEKIT_API_SECRET=%s\nGOOGLE_API_KEY=%s\n' \
        "$LIVEKIT_URL" "$LIVEKIT_API_KEY" "$LIVEKIT_API_SECRET" "$GOOGLE_API_KEY" >"$CONFIG_PATH"
    fi
    ;;
esac

if [ "${BROWSER_CHECK_VALIDATE_ENV_ONLY:-}" ]; then
  exit 0
fi

# Anything that serves `web/` needs the fetched vendor binaries, and this script
# launches its own server rather than going through the Makefile. Without this a
# fresh clone serves a MediaPipe loader whose .wasm and model 404, and face
# detection reports itself unavailable instead of failing: the exact shape of
# the bug that started this, where the binaries were missing and everything
# still looked like it worked.
"$ROOT/scripts/fetch-vendor.sh"

# Resolved after the env-only exit above, which never needs Playwright. Comes
# from the repo's own devDependencies so the usual path is `npm install` with no
# environment variable. PLAYWRIGHT_PATH stays an override for a Playwright
# installed elsewhere.
if [ -z "${PLAYWRIGHT_PATH:-}" ]; then
  PLAYWRIGHT_PATH=$(node -e "console.log(require.resolve('playwright', { paths: ['$ROOT'] }))" 2>/dev/null || true)
fi
if [ -z "$PLAYWRIGHT_PATH" ]; then
  echo "Playwright is not installed. From the repo root:" >&2
  echo "  npm install && npx playwright install chromium" >&2
  echo "Or set PLAYWRIGHT_PATH to an existing Playwright module." >&2

  # Its own code, so a caller can tell "no browser here" from "the check
  # failed". tests/web.rs skips on this one and fails on everything else;
  # without the distinction that test had to stay #[ignore] forever, which is
  # how it went unrun long enough for one of its branches to rot.
  exit 3
fi

if [ "${CODETRIAL_WEB_URL:-}" ]; then
  BASE_URL=${CODETRIAL_WEB_URL%/}
else
  PORT=${PORT:-}
  if [ -z "$PORT" ]; then
    PORT=$(node -e "const s=require('net').createServer();s.listen(0,'127.0.0.1',()=>{console.log(s.address().port);s.close();});")
  fi
  BASE_URL="http://127.0.0.1:$PORT"

  if curl -fsS "$BASE_URL" >/dev/null 2>&1; then
    echo "Port $PORT is already serving HTTP; set PORT to a free port." >&2
    exit 1
  fi

  case $BROWSER_CHECK_AGENT in
    home | offline)
      env -u LIVEKIT_URL -u LIVEKIT_API_KEY -u LIVEKIT_API_SECRET -u GOOGLE_API_KEY \
        SESSION_SECRET="$LOGIN_SESSION_SECRET" \
        CODETRIAL_DB_PATH="$LOGIN_DB_PATH" \
        cargo run --quiet --manifest-path "$ROOT/Cargo.toml" -- web --config "$CONFIG_PATH" --web-addr "127.0.0.1:$PORT" --web-dir "$ROOT/web" \
        >"$SERVER_LOG" 2>&1 &
      ;;
    dispatch)

      # The production shape: no fixed room, so the server invents one per
      # request and has to put an interviewer in it by itself. This is the mode
      # that a missing dispatch breaks, and nothing else covers it.
      unset INTERVIEW_ROOM_NAME
      DISPATCH_MODE=1
      SESSION_SECRET="$LOGIN_SESSION_SECRET" \
      CODETRIAL_DB_PATH="$LOGIN_DB_PATH" \
      cargo run --quiet --manifest-path "$ROOT/Cargo.toml" -- web --config "$CONFIG_PATH" --web-addr "127.0.0.1:$PORT" --web-dir "$ROOT/web" \
        >"$SERVER_LOG" 2>&1 &
      ;;
    rust)
      INTERVIEW_ROOM_NAME=${INTERVIEW_ROOM_NAME:-interview-browser-$(date +%s)-$$}
      export INTERVIEW_ROOM_NAME
      USE_AGENT_SERVE=1
      SESSION_SECRET="$LOGIN_SESSION_SECRET" \
      CODETRIAL_DB_PATH="$LOGIN_DB_PATH" \
      cargo run --quiet --manifest-path "$ROOT/Cargo.toml" -- serve --config "$CONFIG_PATH" --web-addr "127.0.0.1:$PORT" --web-dir "$ROOT/web" \
        >"$SERVER_LOG" 2>&1 &
      ;;
    *)
      echo "BROWSER_CHECK_AGENT must be home, offline, rust, or dispatch." >&2
      exit 2
      ;;
  esac
  SERVER_PID=$!
fi

if [ "$BROWSER_CHECK_FLOW" = "barge" ] && [ -z "$BARGE_AUDIO_FILE" ]; then
  if ! command -v say >/dev/null 2>&1 || ! command -v ffmpeg >/dev/null 2>&1; then
    echo "BROWSER_CHECK_FLOW=barge requires BROWSER_CHECK_BARGE_AUDIO_FILE, or macOS say plus ffmpeg." >&2
    exit 2
  fi
  say -o "$TMP/barge.aiff" "I need to interrupt now. I am still here. Please stop and listen."
  ffmpeg -hide_banner -loglevel error \
    -f lavfi -t 6 -i anullsrc=r=48000:cl=mono \
    -i "$TMP/barge.aiff" \
    -filter_complex "[0:a][1:a]concat=n=2:v=0:a=1,apad=pad_dur=15" \
    -ar 48000 -ac 1 "$TMP/barge.wav"
  BARGE_AUDIO_FILE="$TMP/barge.wav"
fi

i=0
until curl -fsS "$BASE_URL" >/dev/null 2>&1; do
  i=$((i + 1))
  if [ "$i" -gt 60 ]; then
    if [ -s "$SERVER_LOG" ]; then
      cat "$SERVER_LOG" >&2
    fi
    exit 1
  fi
  sleep 1
done

SESSION_COOKIE=${BROWSER_CHECK_SESSION_COOKIE:-}
if [ -z "$SESSION_COOKIE" ]; then
  SESSION_COOKIE=$(login_session_cookie "$BASE_URL" browser-check) || exit 2
fi

# `ROOT="$ROOT"` reads to shellcheck as a self-assignment whose value the
# argument below will not see. Right about the mechanism, wrong about the
# intent: the argument wants this shell's ROOT, and the prefix exists to hand
# node the same value. Nothing here wants the forked one.
# shellcheck disable=SC2097,SC2098
BASE_URL="$BASE_URL" \
  BROWSER_CHECK_AGENT="$BROWSER_CHECK_AGENT" \
  BROWSER_CHECK_FLOW="$BROWSER_CHECK_FLOW" \
  BROWSER_CHECK_CAPTURE="${BROWSER_CHECK_CAPTURE:-}" \
  BROWSER_CHECK_BARGE_AUDIO_FILE="$BARGE_AUDIO_FILE" \
  BROWSER_CHECK_COMPILER_EXPLORER_BASE_URL="$COMPILER_EXPLORER_BASE_URL_ARG" \
  BROWSER_CHECK_SESSION_COOKIE="$SESSION_COOKIE" \
  BROWSER_CHECK_USE_AGENT_SERVE="$USE_AGENT_SERVE" \
  BROWSER_CHECK_CONFIG_PATH="$CONFIG_PATH" \
  INTERVIEW_ROOM_NAME="${INTERVIEW_ROOM_NAME:-}" \
  PLAYWRIGHT_PATH="$PLAYWRIGHT_PATH" \
  ROOT="$ROOT" \
  SERVER_LOG="$SERVER_LOG" \
  node "$ROOT/scripts/browser-check.cjs"

# The flow above only proves the candidate reached an interviewer. In dispatch
# mode it must have been this server that sent one, so check its own log rather
# than infer it from the browser.
if [ "$DISPATCH_MODE" = "1" ]; then
  if ! grep -q "codetrial dispatch room=" "$SERVER_LOG"; then
    echo "server never dispatched an interviewer for the room it minted:" >&2
    cat "$SERVER_LOG" >&2
    exit 1
  fi
  if ! grep -q "starting interview: room=" "$SERVER_LOG"; then
    echo "the dispatched interviewer never started the interview:" >&2
    cat "$SERVER_LOG" >&2
    exit 1
  fi
fi
