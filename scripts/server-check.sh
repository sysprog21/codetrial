#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/scripts/session-cookie.sh"
TMP=$(mktemp -d)
SERVER_LOG="$TMP/web.log"

cleanup() {
  if [ "${SERVER_PID:-}" ]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -r "$TMP" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

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

  env -u LIVEKIT_URL -u LIVEKIT_API_KEY -u LIVEKIT_API_SECRET -u GOOGLE_API_KEY \
    CODETRIAL_SKIP_CONFIG=1 \
    SESSION_SECRET=browser-check-session \
    CODETRIAL_DB_PATH="$TMP/accounts.db" \
    cargo run --quiet --manifest-path "$ROOT/Cargo.toml" -- web --web-addr "127.0.0.1:$PORT" --web-dir "$ROOT/web" \
    >"$SERVER_LOG" 2>&1 &
  SERVER_PID=$!
fi

i=0
while ! curl -fsS "$BASE_URL" >"$TMP/home.html" 2>/dev/null; do
  i=$((i + 1))
  if [ "$i" -gt 60 ]; then
    if [ -s "$SERVER_LOG" ]; then
      cat "$SERVER_LOG" >&2
    fi
    exit 1
  fi
  sleep 1
done

grep -F "Practice a live technical interview" "$TMP/home.html" >/dev/null
grep -F "Valid Parentheses" "$TMP/home.html" >/dev/null
grep -F "Start interview" "$TMP/home.html" >/dev/null

status=$(curl -sS \
  -X POST \
  -H "Content-Type: application/json" \
  -d '{"problemId":"two-sum","durationMin":45}' \
  -o "$TMP/token.json" \
  -w "%{http_code}" \
  "$BASE_URL/api/token")

test "$status" = "401"
grep -F "GitHub username" "$TMP/token.json" >/dev/null

session_cookie=${SERVER_CHECK_SESSION_COOKIE:-}
if [ -z "$session_cookie" ]; then
  session_cookie=$(login_session_cookie "$BASE_URL" server-check) || exit 2
fi

status=$(curl -sS \
  -X POST \
  -H "Content-Type: application/json" \
  -H "Cookie: $session_cookie" \
  -d '{"problemId":"two-sum","durationMin":45}' \
  -o "$TMP/signed-token.json" \
  -w "%{http_code}" \
  "$BASE_URL/api/token")

test "$status" = "500"
grep -F "missing LiveKit credentials" "$TMP/signed-token.json" >/dev/null
