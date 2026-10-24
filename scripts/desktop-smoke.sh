#!/bin/sh

# Starts the desktop app, checks the server it starts, then kills the app the
# hard way and checks the server went with it.
#
# usage: scripts/desktop-smoke.sh APP
#
# APP is the built `codetrial-desktop`, with the `codetrial` server beside it as
# the bundle puts it. Needs a display: run it under `xvfb-run -a` on Linux. Port
# 3101 must be free, because that is the only port the app uses.
#
# Out of the gate, because it needs a built app and a desktop session. The
# desktop workflow runs it on every platform it packages.
#
# The kill is SIGKILL on purpose. A quit through the window is the app stopping
# its own server; a crash is the server noticing on its own, which is what
# `CODETRIAL_EXIT_WITH_STDIN` is for and what a test of the quit would never
# exercise.
set -u

app=${1:?usage: desktop-smoke.sh APP}
addr=127.0.0.1:3101
work=$(mktemp -d) || exit 2

# Inline, for the reason scripts/fetch-vendor.sh gives: a function only the trap
# calls reads as unreachable to shellcheck.
pid=
trap '[ -z "$pid" ] || kill -9 "$pid" 2> /dev/null; rm -rf "$work"' EXIT HUP INT TERM

fail()
{
    echo "desktop-smoke: $*" >&2
    echo "--- app output" >&2
    cat "$work/app.log" >&2
    exit 1
}

status()
{
    curl -s --connect-timeout 1 -m 2 -o "$work/body" -w '%{http_code}' -H "Host: $1" "http://$addr/" 2> /dev/null
}

# Its own data folder, so a run on a developer's machine does not meet that
# machine's install: Linux takes it from XDG, macOS from HOME. Windows takes it
# from the system, which on a CI runner is empty anyway.
#
# Without the Setup keys too, which a first launch writes into that folder from
# the environment and skips Setup for, so a shell that exports them would fail
# the check below.
mkdir -p "$work/data" "$work/config" "$work/cache"
(
    unset LIVEKIT_URL LIVEKIT_API_KEY LIVEKIT_API_SECRET GOOGLE_API_KEY GOOGLE_API_KEYS
    HOME=$work XDG_DATA_HOME=$work/data XDG_CONFIG_HOME=$work/config \
        XDG_CACHE_HOME=$work/cache exec "$app"
) > "$work/app.log" 2>&1 &
pid=$!

# One deadline for the whole wait, not a count of probes: a listener that
# accepts and then hangs would otherwise stretch each probe to its own limit.
served=
deadline=$(($(date +%s) + 60))
while [ "$(date +%s)" -lt "$deadline" ]; do
    if [ "$(status "$addr")" = 200 ]; then
        served=yes
        break
    fi
    kill -0 "$pid" 2> /dev/null || fail "the app exited before its server answered"
    sleep 0.5
done
[ -n "$served" ] || fail "nothing answered on $addr within a minute"

# A fresh install has no config, so what answers is Setup.
grep -q '<title>CodeTrial - Setup</title>' "$work/body" \
    || fail "$addr answered, but not with the Setup page"

# The name a rebinding page would arrive with. 403 because Setup is serving, and
# Setup refuses any non-loopback name itself; the full app's allowlist would
# answer 421.
code=$(status "rebound.example:3101")
[ "$code" = 403 ] || fail "a foreign Host was answered $code, not 403"

kill -9 "$pid"
wait "$pid" 2> /dev/null
pid=

for _ in $(seq 1 20); do
    [ "$(status "$addr")" = 000 ] && {
        echo "desktop-smoke: the server answered, refused a foreign Host, and exited with the app"
        exit 0
    }
    sleep 0.25
done
fail "the server on $addr outlived the app"
