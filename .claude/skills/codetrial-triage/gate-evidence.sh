#!/bin/sh
# Read-only evidence check for one pull request at one head: gather what
# scripts/check-gate-evidence.py needs as data and print its verdict.
#
# Usage: gate-evidence.sh <repo> <pr> <head-sha>
#        Prints one JSON object on stdout:
#          {verdict, reason, head}
#        verdict is pass (the evidence holds, or no receipt is owed), ask (the
#        receipt is missing or wrong: draft the comment that asks the
#        contributor to install the toolchain and run ./scripts/test.sh) or
#        indeterminate (the answer cannot be given: never approve). The exit
#        status is the checker's: 0, 1 or 2.
#
# <head-sha> is the head the caller looked at. The pull request is read again
# here, and a head that differs, now or when the gathering ends, is
# indeterminate: a force-push changes what the evidence is about. A caller that
# approves reads this immediately before the approval call.
#
# Everything from the pull request is data: file names, branch names and the
# note reach the checker as files and arguments, never through eval or a shell
# string, and the head repository is never checked out. The base toolchain file
# comes from the base branch, never the head; the head's is read only when the
# change edits it. The note is fetched by scripts/fetch-gate-note.sh from the
# owner and name the API reports. GATE_EVIDENCE_FETCH names another fetcher and
# GATE_EVIDENCE_PYTHON another interpreter, for tests; neither can be set from a
# pull request.

set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
checker=$root/scripts/check-gate-evidence.py
fetcher=${GATE_EVIDENCE_FETCH:-$root/scripts/fetch-gate-note.sh}

sha_re='^([0-9a-f]{40}|[0-9a-f]{64})$'
head=

emit()
{
    jq -nc --arg verdict "$1" --arg reason "$3" --arg head "$head" \
        '{verdict: $verdict, reason: $reason, head: $head}'
    exit "$2"
}

indeterminate()
{
    emit indeterminate 2 "$1"
}

if [ $# -ne 3 ]; then
    echo "usage: gate-evidence.sh <repo> <pr> <head-sha>" >&2
    exit 2
fi
repo=$1
pr=$2
want=$3
printf '%s\n' "$repo" | grep -Eq '^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$' \
    || indeterminate "the repository name is not owner/name"
case $pr in
    '' | *[!0-9]*) indeterminate "the pull request number is not a number" ;;
esac
printf '%s\n' "$want" | grep -Eq "$sha_re" || indeterminate "the head sha is not hex"
head=$want

# The checker needs tomllib, Python 3.11; the default python3 may be older.
python=
for candidate in "${GATE_EVIDENCE_PYTHON:-}" python3.13 python3.12 python3.11 \
    "${HOME:-/nonexistent}/.local/bin/python3.12" python3; do
    [ -n "$candidate" ] || continue
    if "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 11))' 2> /dev/null; then
        python=$candidate
        break
    fi
done
[ -n "$python" ] || indeterminate "no Python 3.11 or later was found to run the evidence check"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# rust-toolchain.toml at a ref of the base repository, as raw text into a file.
contents()
{
    gh api -X GET -H 'Accept: application/vnd.github.raw+json' \
        "repos/$repo/contents/rust-toolchain.toml" -f ref="$1" > "$2"
}

# A field of the pull request; a missing or unreadable one is the empty string.
pull=$(gh api "repos/$repo/pulls/$pr") || indeterminate "cannot read the pull request"
field()
{
    printf '%s\n' "$pull" | jq -r "$1 // empty" 2> /dev/null || true
}
live=$(field .head.sha)
[ "$live" = "$want" ] || indeterminate "the head moved from $want to ${live:-unknown}"
state=$(field .state)
[ "$state" = open ] || indeterminate "the pull request is $state"
base_ref=$(field .base.ref)
changed=$(field .changed_files)
author_type=$(field .user.type)
head_owner=$(field .head.repo.owner.login)
head_name=$(field .head.repo.name)
case $changed in
    '' | *[!0-9]*) indeterminate "the changed file count is unreadable" ;;
esac
[ -n "$base_ref" ] || indeterminate "the base branch is unreadable"
[ -n "$author_type" ] || indeterminate "the author type is unreadable"

# The changed paths, a NUL after every one, including the last. A rename names
# both ends, since moving a file out of the Rust scope changes it too. The list
# is cut when the entries read are fewer than the PR's count.
gh api --paginate "repos/$repo/pulls/$pr/files" > "$work/files.json" \
    || indeterminate "cannot read the changed files"
read_count=$(jq -s '[.[][]] | length' "$work/files.json" 2> /dev/null) \
    || indeterminate "the changed files are unreadable"
if jq -se 'any(.[][]; (.filename, (.previous_filename // "")) | explode | any(. == 0))' \
    "$work/files.json" > /dev/null 2>&1; then
    indeterminate "a changed path holds a NUL byte"
fi
if [ "$read_count" -ne "$changed" ]; then
    indeterminate "the changed-file list is truncated: $read_count of $changed paths"
fi
jq -j '.[] | .filename, "\u0000", ((.previous_filename // empty) | ., "\u0000")' \
    "$work/files.json" > "$work/paths" || indeterminate "the changed paths are unreadable"

set -- --changed-files-file "$work/paths" --author-type "$author_type"
scope=$("$python" -I "$checker" --scope-only -z < "$work/paths" 2> "$work/scope.err") \
    || indeterminate "$(head -n 1 "$work/scope.err")"

# The checker always wants the head's tree, and a receipt names it.
tree=$(gh api "repos/$repo/git/commits/$want" --jq .tree.sha) \
    || indeterminate "cannot read the head commit's tree"
set -- "$@" --head-tree "$tree"

if [ "$scope" = in ] && [ "$author_type" != Bot ]; then
    contents "$base_ref" "$work/base-toolchain" \
        || indeterminate "cannot read rust-toolchain.toml from the base branch"
    set -- "$@" --base-toolchain-file "$work/base-toolchain"
    if jq -se 'any(.[][]; .filename == "rust-toolchain.toml"
        or .previous_filename == "rust-toolchain.toml")' "$work/files.json" > /dev/null 2>&1; then
        contents "$want" "$work/head-toolchain" \
            || indeterminate "cannot read rust-toolchain.toml from the head"
        set -- "$@" --head-toolchain-file "$work/head-toolchain"
    fi

    # No note is an answer (the checker asks for one); a repository that cannot
    # be read, or a fetch that failed, is not.
    if [ -z "$head_owner" ] || [ -z "$head_name" ]; then
        indeterminate "the head repository is gone, so its gate note cannot be read"
    fi
    status=0
    sh "$fetcher" "$head_owner" "$head_name" "$want" > "$work/note" 2> "$work/note.err" || status=$?
    case $status in
        0) set -- "$@" --receipt-file "$work/note" ;;
        3 | 6 | 7) ;;
        *) indeterminate "$(head -n 1 "$work/note.err")" ;;
    esac
fi

status=0
"$python" -I "$checker" "$@" > "$work/out" 2> "$work/err" || status=$?
# A push that landed while this ran would make the evidence about an old head.
after=$(gh api "repos/$repo/pulls/$pr" --jq .head.sha) || indeterminate "cannot read the pull request again"
[ "$after" = "$want" ] || indeterminate "the head moved while the evidence was gathered"
message=$(head -n 1 "$work/err")
case $status in
    0) emit pass 0 "$(tail -n 1 "$work/out")" ;;
    1) emit ask 1 "$message" ;;
    *) indeterminate "${message:-the evidence check failed}" ;;
esac
