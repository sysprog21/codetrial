#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/scripts/session-cookie.sh"
. "$ROOT/scripts/wait-for-server.sh"
TMP=$(mktemp -d)
SERVER_LOG="$TMP/web.log"
CONFIG_PATH="$TMP/codetrial.env.local"
printf '%s\n' \
    'LIVEKIT_URL=wss://example.livekit.cloud' \
    'LIVEKIT_API_KEY=server-check-key' \
    'LIVEKIT_API_SECRET=server-check-secret' > "$CONFIG_PATH"

cleanup()
{
    if [ "${SERVER_PID:-}" ]; then
        kill "$SERVER_PID" 2> /dev/null || true
        wait "$SERVER_PID" 2> /dev/null || true
    fi
    rm -r "$TMP" 2> /dev/null || true
}
trap cleanup EXIT INT TERM

# Anything that serves `web/` needs the fetched vendor binaries, and this script
# launches its own server rather than going through the Makefile. Without this a
# fresh clone serves a MediaPipe loader whose .wasm and model 404, and face
# detection reports itself unavailable instead of failing: the exact shape of
# the bug that started this, where the binaries were missing and everything
# still looked like it worked.
"$ROOT/scripts/fetch-vendor.sh"

if [ "${CODETRIAL_WEB_URL:-}" ]; then
    BASE_URL=${CODETRIAL_WEB_URL%/}
else

    # A cold build can take minutes. Keep it outside the server's readiness
    # deadline, and ahead of choosing a port that could be taken meanwhile, so
    # compiler errors reach the caller before anything is launched.
    cargo build --manifest-path "$ROOT/Cargo.toml" --bin codetrial

    PORT=${PORT:-}
    if [ -z "$PORT" ]; then
        PORT=$(node -e "const s=require('net').createServer();s.listen(0,'127.0.0.1',()=>{console.log(s.address().port);s.close();});")
    fi
    BASE_URL="http://127.0.0.1:$PORT"

    if curl -fsS "$BASE_URL" > /dev/null 2>&1; then
        echo "Port $PORT is already serving HTTP; set PORT to a free port." >&2
        exit 1
    fi

    env -u LIVEKIT_URL -u LIVEKIT_API_KEY -u LIVEKIT_API_SECRET -u GOOGLE_API_KEY \
        SESSION_SECRET=browser-check-session \
        CODETRIAL_DB_PATH="$TMP/accounts.db" \
        cargo run --quiet --manifest-path "$ROOT/Cargo.toml" -- web --config "$CONFIG_PATH" --web-addr "127.0.0.1:$PORT" --web-dir "$ROOT/web" \
        > "$SERVER_LOG" 2>&1 &
    SERVER_PID=$!
fi

wait_for_server "$BASE_URL" 60 "$SERVER_LOG" "${SERVER_PID:-}" "$TMP/home.html" || exit 1

grep -F "Start Interview" "$TMP/home.html" > /dev/null
grep -F 'data-problem="template-delimiter-audit"' "$TMP/home.html" > /dev/null
grep -F "Start interview" "$TMP/home.html" > /dev/null

session_json=$(curl -fsS "$BASE_URL/api/session")

guest_mode_available=$(printf '%s' "$session_json" | node -e '
let data = "";
process.stdin.on("data", chunk => data += chunk);
process.stdin.on("end", () => {
    const session = JSON.parse(data);
    process.stdout.write(String(session.guestModeAvailable === true));
});
')

status=$(curl -sS \
    -X POST \
    -H "Content-Type: application/json" \
    -d '{"problemId":"two-sum","durationMin":45}' \
    -o "$TMP/token.json" \
    -w "%{http_code}" \
    "$BASE_URL/api/token")

if [ "$guest_mode_available" = "true" ]; then
    test "$status" = "200"
    grep -F '"token"' "$TMP/token.json" > /dev/null
else
    test "$status" = "403"
    grep -F "Guest interviews are disabled" "$TMP/token.json" > /dev/null
fi

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

test "$status" = "200"
grep -F '"token"' "$TMP/signed-token.json" > /dev/null
