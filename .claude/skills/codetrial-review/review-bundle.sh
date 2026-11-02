#!/bin/sh
# Gather what a review reads into one directory, so every reviewer, the
# verification and the posting work from the same snapshot. Writes nothing to
# GitHub, and nothing outside <dir> but the objects a local mode with no base
# fetches into the repository.
#
# Usage: review-bundle.sh <pr> <dir> [repo] [--since <sha>]
#        review-bundle.sh --local <dir> [<base>]
#        Prints the manifest (also saved as <dir>/manifest.json). Exits 3 when
#        the PR moved during two consecutive fetches, 2 on bad arguments.
#
# <dir> receives:
#   pr.json      title, body, author, head, base, updatedAt, linked issues; the changed
#                files come from the REST listing, which pages past the 100
#                that gh pr view --json files stops at, up to GitHub's 3000;
#                past that the manifest's files_complete is false
#   pr.diff      the diff as GitHub serves it at that head; review-anchors.sh
#                reads it again when posting, and the posting scripts refuse
#                a head that moved, so the two cannot disagree
#   head/<path>  each changed file as it is at the head, read once here so no
#                reviewer fetches it again; the manifest's not_at_head names
#                the files with no copy (deleted, or a read that failed)
#   numbered.txt the same diff with each right-side line numbered, so a
#                reviewer reads the line a comment would land on instead of
#                counting from a hunk header
#   threads.tsv  every review thread: resolved, outdated, path, line, author,
#                first comment flattened to one line
#   talk.json    reviews and top-level comments
#   comments.json every inline review comment, replies included, since a point
#                a human made in a reply is as raised as one opening a thread
#   since.diff, since.txt
#                with --since, the diff from the head an earlier review read to
#                this one, numbered the same way, when that head is an
#                ancestor of this one; a force push or a merged base reads as
#                a full review instead, and the manifest says why
#   experience-start.json pins the local experience run to this manifest/diff;
#   experience-status.json reports whether begin succeeded
#   experience.md gives bounded private prior cases without migrating history;
#   experience-context-status.json reports that independent read outcome
# A rerun into the same <dir> replaces what an earlier head left there.
#
# Local mode with no base reviews the working tree against HEAD when it is dirty
# (untracked files count), else HEAD against its merge base with upstream main
# fetched by URL (as codetrial-contribute's related.sh does, so a fork's remote
# names do not matter). A base named is always the base: a dirty tree is then
# diffed from it whole, committed and uncommitted work together. It writes
# pr.diff, numbered.txt and the manifest only; the files at the head are the
# checkout. Untracked files are named in the manifest, not diffed, and
# `committed` says whether the diff is exactly what HEAD holds, which is when a
# review of it speaks for a push of HEAD. Local bundles also embed commit_sizes
# collected against the full supplied or resolved branch base; a dirty HEAD-only
# preview leaves that coverage unknown.
#
# The manifest's outside_reviewers is "allowed" only for a diff the account
# running this wrote (its own PRs, and a local one whose commits all carry the
# configured git email), which is the only kind lenses.md lets an outside
# reviewer read. Uncommitted changes are the user's own.
#
# The manifest maps each changed file to the rule files in rules/ that the
# patterns in rules/paths give it, and lists the files no pattern covers: those
# are read against the general rules only, which the report says.

set -eu
umask 077

usage='usage: review-bundle.sh <pr> <dir> [repo] [--since <sha>] | --local <dir> [<base>]'
here=$(dirname "$0")
mode='pr'
since=
base=
case ${1:-} in
    --local)
        mode=local
        dir=${2:?$usage}
        base=${3:-}
        ;;
    *)
        pr=${1:?$usage}
        dir=${2:?$usage}
        shift 2
        repo=sysprog21/codetrial

        # An empty repo argument still takes its place, so --since after it is
        # read rather than silently dropped.
        case ${1-} in
            --since) ;;
            '') [ $# -eq 0 ] || shift ;;
            *)
                repo=$1
                shift
                ;;
        esac
        if [ "${1:-}" = --since ]; then
            since=${2:?$usage}
        elif [ -n "${1:-}" ]; then
            echo "$usage" >&2
            exit 2
        fi
        case $pr in
            '' | *[!0-9]*)
                echo "review-bundle.sh: $pr is not a pull request number" >&2
                exit 2
                ;;
        esac
        case $since in
            *[!0-9a-f]*)
                echo "review-bundle.sh: $since is not a commit" >&2
                exit 2
                ;;
        esac
        # Shorter is no abbreviation of the head, only a prefix it shares.
        if [ -n "$since" ] && [ "${#since}" -lt 7 ]; then
            echo "review-bundle.sh: --since needs at least 7 hex digits" >&2
            exit 2
        fi
        ;;
esac
mkdir -p "$dir"

# Everything either mode or memory.py context writes, so a rerun in another mode
# or at another head reads nothing an earlier one left: context reads pr.json
# whenever it is there.
rm -rf "$dir/head" "$dir/since.diff" "$dir/since.txt" "$dir/pr.json" \
    "$dir/talk.json" "$dir/comments.json" "$dir/threads.tsv" "$dir/history.md"

# The diff with each right-side line numbered: " 42 +added", " 43 context", "
# -deleted". A section with no hunk (binary, mode only) keeps its header.
number()
{
    awk "$(cat "$here/diff-path.awk")"'
        /^diff --git / { header = 1; print ""; print "## " git_path($0); next }
        header && /^\+\+\+ \/dev\/null/ { print "(deleted)"; next }
        header && /^(---|\+\+\+) / { next }
        /^@@ / {
            header = 0
            split($3, r, ","); right = substr(r[1], 2) + 0
            print; next
        }
        header { next }
        /^\\/ { next }
        /^\+/ { printf "%6d +%s\n", right++, substr($0, 2); next }
        /^-/ { printf "       -%s\n", substr($0, 2); next }
        { printf "%6d  %s\n", right++, substr($0, 2) }
    '
}

# Each changed path followed by the rule files rules/paths gives it, one
# tab-separated row per path. A pattern is a shell case pattern, so * crosses a
# slash; every matching line counts, not only the first.
rules_for()
{
    while IFS= read -r path; do
        found=
        while read -r pattern rule; do
            case $pattern in '' | '#'*) continue ;; esac
            # shellcheck disable=SC2254 # the pattern is meant to glob
            case $path in
                $pattern)
                    case " $found " in
                        *" $rule "*) ;;
                        *) found="$found${found:+ }$rule" ;;
                    esac
                    ;;
            esac
        done < "$here/rules/paths"
        printf '%s\t%s\n' "$path" "$found"
    done
}

# The manifest both modes print: $dir/files.json is [{path, additions,
# deletions}], each file gets its rule files, and the argument adds what only
# one mode knows.
manifest()
{
    jq -r '.[].path' "$dir/files.json" | rules_for > "$dir/rules.tsv"
    jq --rawfile rules "$dir/rules.tsv" --argjson extra "$1" '
      ([$rules | split("\n")[] | select(length > 0) | split("\t")
        | {(.[0]): (.[1] | split(" ") | map(select(length > 0)))}] | add // {}) as $r
      | map(. + {rules: $r[.path]}) as $files
      | {size: {files: ($files | length),
                additions: ([$files[].additions // 0] | add // 0),
                deletions: ([$files[].deletions // 0] | add // 0)},
         files: $files,
         unruled: [$files[] | select(.rules == []) | .path]} + $extra' \
        "$dir/files.json" > "$dir/manifest.json"
    rm -f "$dir/rules.tsv" "$dir/files.json"
    if ! python3 "$here/review-lifecycle.py" begin --bundle "$dir" > "$dir/experience-status.json" 2> "$dir/experience-begin.stderr"; then
        printf '%s\n' '{"outcome":"unavailable","reason":"experience begin failed","diagnostics":"experience-begin.stderr"}' > "$dir/experience-status.json"
    fi
    if ! python3 "$here/memory.py" experience context --bundle "$dir" > "$dir/experience-context-status.json" 2> "$dir/experience-context.stderr"; then
        printf '%s\n' 'Local experience unavailable; inspect experience-context-status.json and preparation diagnostics. Continue current-code verification.' > "$dir/experience.md"
        printf '%s\n' '{"outcome":"unavailable","reason":"local experience read failed","diagnostics":"experience-context.stderr"}' > "$dir/experience-context-status.json"
    fi
    cat "$dir/manifest.json"
}

if [ "$mode" = local ]; then
    untracked=$(git ls-files --others --exclude-standard)
    dirty=$(git status --porcelain --untracked-files=no)
    if [ -z "$base" ] && [ -n "$untracked$dirty" ]; then
        against=HEAD
    elif [ -n "$dirty" ]; then
        against=$base
    else
        if [ -z "$base" ]; then

            # Offline, the local copy of upstream main is the next best base,
            # and saying so beats failing on git's network error alone.
            if git fetch -q "https://github.com/sysprog21/codetrial.git" main; then
                base=$(git merge-base HEAD FETCH_HEAD)
            else
                remote=$(git remote -v | awk '$2 ~ /github[.]com[:\/]sysprog21\/codetrial([.]git)?$/ \
                    && $3 == "(fetch)" { print $1; exit }')
                base=$(git merge-base HEAD "${remote:-origin}/main") || {
                    echo "review-bundle.sh: cannot reach upstream main and have no copy of it; name a base" >&2
                    exit 2
                }
                echo "review-bundle.sh: upstream unreachable; using ${remote:-origin}/main as it was last fetched" >&2
            fi
        fi
        against="$base...HEAD"
    fi
    # Only base...HEAD is exactly what HEAD holds.
    case $against in
        *...HEAD) committed=true range=${against%...HEAD}..HEAD ;;
        HEAD) committed=false range= ;;
        *) committed=false range=$against..HEAD ;;
    esac

    # A branch checked out from a contributor's PR holds their commits. This is
    # a deterrent, not authentication: a commit can carry any author email.
    me=$(git config user.email 2> /dev/null) || me=
    outside=refused
    if [ -n "$me" ] && ! git log --format=%ae "${range:-HEAD..HEAD}" | grep -qvxF "$me"; then
        outside=allowed
    fi

    # The diff as the scripts parse it, whatever the user's diff config: no
    # color, no external driver, the a/ and b/ prefixes.
    python3 "$here/commit-sizes.py" --diff "$against" > "$dir/pr.diff"
    number < "$dir/pr.diff" > "$dir/numbered.txt"
    # -z, so a path git would quote reaches the rule patterns as it is.
    python3 "$here/commit-sizes.py" --diff "$against" --numstat > "$dir/numstat.raw"
    jq -Rs 'split("\u0000")
        | map(select(length > 0) | split("\t")
          | {path: .[2], additions: (.[0] | tonumber? // null),
             deletions: (.[1] | tonumber? // null)})' "$dir/numstat.raw" > "$dir/files.json"
    rm -f "$dir/numstat.raw"
    printf '%s\n' '{"commits":[],"complete":false}' > "$dir/commit-sizes.json"
    if [ -n "$base" ]; then
        if ! python3 "$here/commit-sizes.py" --base "$base" --head HEAD > "$dir/commit-sizes.json"; then
            echo 'review-bundle.sh: commit-size coverage incomplete; inspect the full PR range before submission' >&2
        fi
    fi
    manifest "$(jq -n --arg head "$(git rev-parse HEAD)" --arg against "$against" \
        --slurpfile sizes "$dir/commit-sizes.json" \
        --arg untracked "$untracked" --argjson committed "$committed" \
        --arg outside "$outside" '{pr: null,
        head: $head, against: $against, committed: $committed,
        outside_reviewers: $outside,
        untracked: ($untracked | split("\n") | map(select(length > 0))),
        since: null, commit_sizes: $sizes[0]}')"
    exit 0
fi

# gh pr diff takes no revision, so the metadata is read on both sides of the
# diff and the two must name the same head and base; a push in between means the
# diff may belong to either, and it is read again once.
fields=title,body,author,headRefOid,baseRefOid,updatedAt,closingIssuesReferences,reviews,comments,changedFiles
attempt=0
while :; do
    gh pr view "$pr" --repo "$repo" --json "$fields" > "$dir/pr.json"
    gh api "repos/$repo/pulls/$pr/files?per_page=100" --paginate \
        --jq '.[] | {path: .filename, additions, deletions, status}' \
        > "$dir/files.jsonl"
    gh pr diff "$pr" --repo "$repo" > "$dir/pr.diff"
    after=$(gh pr view "$pr" --repo "$repo" --json headRefOid,baseRefOid \
        --jq '.headRefOid + " " + .baseRefOid')
    [ "$(jq -r '.headRefOid + " " + .baseRefOid' "$dir/pr.json")" = "$after" ] && break
    attempt=$((attempt + 1))
    [ "$attempt" -lt 2 ] || {
        echo "review-bundle.sh: #$pr moved twice while it was read; try again later" >&2
        exit 3
    }
done
head=$(jq -r .headRefOid "$dir/pr.json")
number < "$dir/pr.diff" > "$dir/numbered.txt"
jq '{reviews, comments}' "$dir/pr.json" > "$dir/talk.json"

# Each changed file at the head, eight reads at a time. A deleted file has no
# copy, and neither has one whose read failed; both are named, not guessed at.
mkdir -p "$dir/head"
jq -r 'select(.status == "removed") | .path' "$dir/files.jsonl" > "$dir/head.missing"
jq -j 'select(.status != "removed")
    | .path, "\u0000", (.path | split("/") | map(@uri) | join("/")), "\u0000"' \
    "$dir/files.jsonl" > "$dir/head.list"
# A PR that only deletes has nothing to read, and xargs would run once anyway.
# shellcheck disable=SC2016 # expanded by the inner sh, from its arguments
[ ! -s "$dir/head.list" ] || xargs -0 -n 2 -P 8 sh -c '
        out=$0/head/$3
        mkdir -p "$(dirname "$out")"
        gh api "repos/$1/contents/$4?ref=$2" -H "Accept: application/vnd.github.raw" \
            > "$out" 2> /dev/null || {
            rm -f "$out"
            printf "%s\n" "$3" >> "$0/head.missing"
        }' "$dir" "$repo" "$head" < "$dir/head.list"
rm -f "$dir/head.list"

# Into a file first: in a pipe set -e sees only jq, and a refused read would
# slurp into [] and read as a PR nobody has commented on.
gh api "repos/$repo/pulls/$pr/comments?per_page=100" --paginate \
    --jq '.[] | {path, line: (.line // .original_line), author: .user.login,
        reply: (.in_reply_to_id != null),
        commit: (if .line then .commit_id else .original_commit_id end), body}' \
    > "$dir/comments.jsonl"
jq -s . "$dir/comments.jsonl" > "$dir/comments.json"
rm -f "$dir/comments.jsonl"

# --paginate prints one document per page, each turned into rows by --jq.
# shellcheck disable=SC2016 # GraphQL variables, not shell ones
gh api graphql --paginate -F n="$pr" -F owner="${repo%%/*}" -F name="${repo#*/}" \
    -f query='query($n: Int!, $owner: String!, $name: String!, $endCursor: String) {
  repository(owner: $owner, name: $name) { pullRequest(number: $n) {
    reviewThreads(first: 100, after: $endCursor) {
      pageInfo { hasNextPage endCursor }
      nodes { isResolved isOutdated path line originalLine
        comments(first: 1) { nodes { author { login } body } } } } } } }' \
    --jq '.data.repository.pullRequest.reviewThreads.nodes[]
      | [.isResolved, .isOutdated, .path, (.line // .originalLine),
         .comments.nodes[0].author.login,
         (.comments.nodes[0].body | gsub("<!--[\\s\\S]*?-->"; "") | gsub("\\s+"; " ")
          | ltrimstr(" ") | .[:300])] | @tsv' \
    > "$dir/threads.tsv"

# An earlier review's head counts only when this head descends from it: then the
# commits since are the new work. Otherwise history was rewritten, and the whole
# diff is new to a reviewer.
incremental=null
# A short sha of the head itself is the head: nothing new, not a rewrite.
case $head in "$since"*) since= ;; esac
if [ -n "$since" ]; then

    # A merge in the commits since (the base merged in, say) carries other work
    # into the comparison, so only a merge-free run is new work alone.
    compared=$(gh api "repos/$repo/compare/$since...$head" --jq '.status + " "
        + ([.commits[] | select((.parents | length) > 1)] | length | tostring)' \
        2> /dev/null) || compared=unknown
    status=${compared%% *}
    if [ "$status" = ahead ] && [ "$compared" != "ahead 0" ]; then
        status=merged
    fi
    if [ "$compared" = "ahead 0" ]; then
        gh api "repos/$repo/compare/$since...$head" \
            -H "Accept: application/vnd.github.diff" > "$dir/since.diff"
        number < "$dir/since.diff" > "$dir/since.txt"
        incremental=$(jq -n --arg from "$since" '{from: $from, status: "ahead"}')
    else
        incremental=$(jq -n --arg from "$since" --arg status "$status" '
            {from: $from, status: $status, why: (if $status == "merged"
              then "a merge since then brings in other work, so the whole diff is reviewed"
              else "not an ancestor of the head, so the whole diff is reviewed" end)}')
    fi
fi

jq -s 'map({path, additions, deletions})' "$dir/files.jsonl" > "$dir/files.json"

# An outside reviewer can read the gh login beside the checkout, and a diff can
# carry instructions to make it: only a PR the account running this wrote may go
# to one. A failed lookup refuses.
viewer=$(gh api user --jq .login 2> /dev/null) || viewer=

# GitHub lists at most 3000 files of a PR: past that the manifest says so,
# rather than a review reading part of the change as all of it.
listed=$(jq -s length "$dir/files.jsonl")
changed=$(jq -r '.changedFiles // 0' "$dir/pr.json")
complete=true
if [ "$listed" -lt "$changed" ]; then
    complete=false
    echo "review-bundle.sh: #$pr changes $changed files and GitHub listed $listed; the rest are not in this bundle" >&2
fi
manifest "$(jq --argjson pr "$pr" --argjson since "$incremental" \
    --arg viewer "$viewer" --rawfile missing "$dir/head.missing" \
    --argjson complete "$complete" '{pr: $pr,
    head: .headRefOid, base: .baseRefOid, title, author: .author.login,
    outside_reviewers: (if $viewer != "" and .author.login == $viewer
      then "allowed" else "refused" end), updatedAt,
    closes: [.closingIssuesReferences[]?.number], files_complete: $complete,
    not_at_head: ($missing | split("\n") | map(select(length > 0)) | sort),
    since: $since}' "$dir/pr.json")"
rm -f "$dir/head.missing" "$dir/files.jsonl"
