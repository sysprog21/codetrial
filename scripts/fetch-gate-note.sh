#!/bin/sh
# Print the gate note a pushed head commit carries, fetched from the head
# repository as data. This is the only place a fork's notes are fetched: the
# maintainer sweep (through gate-evidence.sh) and the CI preflight are to call
# it, so the bounds below are written once.
#
# Usage: fetch-gate-note.sh OWNER NAME SHA
#
# OWNER and NAME come from the pull request API's head repository and SHA from
# its head commit, as separate arguments; no URL is ever taken from PR text. The
# script validates all three, builds https://github.com/OWNER/NAME.git itself,
# and calls git with an argument vector only: nothing is passed through eval or
# a shell string, no name is used as a path, the repository is never checked
# out, and nothing from it is run.
#
# Exit 0 prints the note on stdout. Any other status prints one line on stderr:
#   2  bad usage, or an owner, name, sha or timeout that fails validation
#   3  the repository has no refs/notes/codetrial-gate
#   4  the repository is deleted, private, or refuses an anonymous fetch
#   5  the fetch failed or timed out (offline, rate limited, a server error)
#   6  the ref exists and the commit has no note
#   7  the note is over 16384 bytes, or was left out by the size filter
# Callers treat 3, 6 and 7 as no note and 4 and 5 as indeterminate.
#
# FETCH_GATE_NOTE_TIMEOUT may lower the 60 second limit (tests); it cannot raise
# it. GitHub honours --filter=blob:limit, so a blob over the limit is not sent
# at all; the explicit size check below covers a server that ignores it.
# GIT_NO_LAZY_FETCH stops git from fetching an omitted blob afterwards (git 2.44
# and later; an older git is bounded by the timeout alone). The filter limits
# each blob, not the fork's whole notes tree: the timeout is the only bound on
# the bytes a fork can make this fetch.

set -eu

LIMIT=16384
REF=refs/notes/codetrial-gate

fail()
{
    code=$1
    shift
    echo "fetch-gate-note: $*" >&2
    exit "$code"
}

[ $# -eq 3 ] || fail 2 "usage: fetch-gate-note.sh OWNER NAME SHA"
owner=$1
name=$2
sha=$3

# An owner or repository name: letters, digits, dot, underscore and dash, not
# starting with a dot or a dash and without two dots in a row, at most 100
# characters. The glob runs under the C locale so a range cannot match more.
LC_ALL=C
export LC_ALL
valid_name()
{
    case $1 in
        '' | .* | -* | *..*) return 1 ;;
        *[!A-Za-z0-9._-]*) return 1 ;;
    esac
    [ "${#1}" -le 100 ]
}
valid_name "$owner" || fail 2 "the owner is not a GitHub account name"
valid_name "$name" || fail 2 "the repository name is not a GitHub repository name"
case $sha in
    *[!0-9a-f]*) fail 2 "the sha is not lowercase hex" ;;
esac
case ${#sha} in
    40 | 64) ;;
    *) fail 2 "the sha is not 40 or 64 hex digits" ;;
esac

seconds=60
if [ "${FETCH_GATE_NOTE_TIMEOUT+set}" = set ]; then
    case $FETCH_GATE_NOTE_TIMEOUT in
        '' | *[!0-9]* | 0*) fail 2 "FETCH_GATE_NOTE_TIMEOUT is not a number of seconds" ;;
    esac
    [ "$FETCH_GATE_NOTE_TIMEOUT" -le 60 ] \
        || fail 2 "FETCH_GATE_NOTE_TIMEOUT cannot exceed 60"
    seconds=$FETCH_GATE_NOTE_TIMEOUT
fi
if command -v timeout > /dev/null 2>&1; then
    limit=timeout
elif command -v gtimeout > /dev/null 2>&1; then
    limit=gtimeout
else
    fail 5 "neither timeout nor gtimeout is installed, so the fetch cannot be bounded"
fi

tmp=$(mktemp -d) || fail 5 "cannot create a scratch directory"
trap 'rm -rf "$tmp"' EXIT
trap 'exit 5' HUP INT TERM
mkdir "$tmp/home" "$tmp/repo"
url=https://github.com/$owner/$name.git

# A clean environment: none of the caller's GIT_* variables (a GIT_DIR or
# GIT_CONFIG_* would redirect the call), an empty home with no config, and no
# credential, so the fetch is anonymous and never prompts. Proxy variables and
# the caller's CA bundle (GIT_SSL_CAINFO, GIT_SSL_CAPATH) are left as they are,
# since a network that intercepts TLS needs them; GIT_SSL_NO_VERIFY is not.
for var in $(env | sed -n 's/^\(GIT_[A-Za-z0-9_]*\)=.*/\1/p'); do
    case $var in GIT_SSL_CAINFO | GIT_SSL_CAPATH) continue ;; esac
    unset "$var"
done
run_git()
{
    HOME=$tmp/home TMPDIR=$tmp GIT_TERMINAL_PROMPT=0 GIT_CONFIG_NOSYSTEM=1 \
        GIT_CONFIG_GLOBAL=/dev/null GIT_NO_LAZY_FETCH=1 \
        "$limit" "$seconds" git -C "$tmp/repo" \
        -c protocol.allow=never -c protocol.https.allow=always \
        -c credential.helper= -c core.askPass= -c gc.auto=0 \
        -c transfer.fsckObjects=true "$@"
}

run_git init -q --bare > /dev/null 2>&1 || fail 5 "cannot create a scratch repository"

# Only the notes ref, one commit deep, no tags, no submodules. Blobs of 16385
# bytes or more are left out by the server: the limit is exclusive.
status=0
err=$(run_git fetch -q --no-tags --depth=1 --no-recurse-submodules \
    --filter=blob:limit=$((LIMIT + 1)) "$url" "$REF:$REF" 2>&1 > /dev/null) || status=$?
if [ "$status" -ne 0 ]; then
    case $status in
        124 | 137) fail 5 "the fetch of $owner/$name timed out after ${seconds}s" ;;
    esac
    case $err in
        *"couldn't find remote ref"*) fail 3 "$owner/$name has no $REF" ;;
        *"could not read Username"* | *[Nn]"ot found"* | *"Authentication failed"* | *"Repository not found"*)
            fail 4 "$owner/$name is gone or not readable anonymously"
            ;;
    esac
    fail 5 "the fetch of $owner/$name failed"
fi

# Status 1 with git's "no note found" is a confirmed absence; a timeout or any
# other failure is not an answer about the note.
status=0
blob=$(run_git notes --ref="$REF" list "$sha" 2> "$tmp/notes.err") || status=$?
if [ "$status" -ne 0 ]; then
    case $status in
        124 | 137) fail 5 "reading the notes of $owner/$name timed out" ;;
    esac
    grep -q 'no note found' "$tmp/notes.err" \
        || fail 5 "the notes of $owner/$name could not be read"
    fail 6 "$owner/$name has no gate note for $sha"
fi
case $blob in
    '' | *[!0-9a-f]*) fail 6 "$owner/$name has no gate note for $sha" ;;
esac

# The size comes before any read. A blob the filter left out cannot be sized
# without a lazy fetch, which is disabled; --batch-check reports it as missing
# with a zero status, so a missing blob is told apart from a probe that failed.
status=0
probe=$(printf '%s\n' "$blob" | run_git cat-file --batch-check 2> /dev/null) || status=$?
case $status in
    0) ;;
    124 | 137) fail 5 "sizing the gate note timed out" ;;
    *) fail 5 "the gate note could not be sized" ;;
esac
case $probe in
    *" missing") fail 7 "the gate note is over $LIMIT bytes or was not sent" ;;
esac
size=${probe##* }
case $size in
    '' | *[!0-9]*) fail 5 "the gate note could not be sized" ;;
esac
[ "$size" -le "$LIMIT" ] || fail 7 "the gate note is $size bytes, over the $LIMIT limit"

run_git cat-file blob "$blob" || fail 7 "the gate note could not be read"
