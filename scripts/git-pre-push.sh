#!/bin/sh

# Judge the commit messages this push would publish. The commit-msg hook only
# sees a message as it is written, so a rebase, an amend, or a commit made with
# --no-verify reaches the remote unread. This is the last place to catch that
# while the history is still local and cheap to rewrite.
#
# It also refuses new work on the main branch of any other copy of this
# repository, which in practice is a fork a pull request is about to be opened
# from. The same rule runs in .github/workflows/check.yml for a contributor who
# never installed the hooks.

set -u
export GIT_NO_REPLACE_OBJECTS=1

remote=${1:-}
url=${2:-}

# Read the pushed refs first: the tag sync below decides from them, and the
# transport must not get a chance to consume them.
refs=$(cat)

# Release tags follow origin, and only a push to origin is held up by it, so an
# origin that is slow or unreachable cannot block a push elsewhere. A push that
# itself updates one of the tags keeps the local value. Missing tags are skipped
# after discovery. Both network calls share one deadline.
origin_url=$(git remote get-url origin 2> /dev/null) || origin_url=
if [ -n "$origin_url" ] && {
    [ "$remote" = origin ] || [ "$url" = "$origin_url" ]
} \
    && ! printf '%s\n' "$refs" \
    | awk '$3 ~ /^refs\/tags\/(latest|desktop-latest)$/ { found = 1 } END { exit !found }'; then
    bound=
    for tool in timeout gtimeout; do
        if command -v "$tool" > /dev/null 2>&1; then
            bound="$tool -k 5 55"
            break
        fi
    done

    # Without a tool to enforce the deadline (stock macOS has neither) the sync
    # is skipped rather than risking an unbounded wait on every push.
    if [ -z "$bound" ]; then
        echo "Note: release tags not synchronized; install timeout or gtimeout to enable it." >&2
        bound=skip
    fi
    # The child shell expands its own discovery results, not this shell.
    # shellcheck disable=SC2086,SC2016
    [ "$bound" = skip ] || GIT_TERMINAL_PROMPT=0 $bound sh -c '
        available=$(git ls-remote --refs origin refs/tags/latest refs/tags/desktop-latest) || exit 1
        set --
        while read -r oid ref; do
            case "$ref" in
                refs/tags/latest | refs/tags/desktop-latest) set -- "$@" "+$ref:$ref" ;;
            esac
        done << EOF
$available
EOF
        [ "$#" -eq 0 ] || git fetch --no-tags origin "$@"
    ' < /dev/null || {
        echo "Push rejected: cannot synchronize release tags from origin." >&2
        echo "Check origin connectivity, then retry." >&2
        exit 1
    }
fi

# From the repository, not from $0: git invokes the hook through the wrapper in
# .git/hooks, so dirname of $0 names that directory and not scripts/. Hooks run
# with the working tree root as the working directory.
script_dir=$(git rev-parse --show-toplevel)/scripts
zero=0000000000000000000000000000000000000000
failed=0

# For a new branch the remote has no tip to diff against, so "new" means every
# commit not already published somewhere. Scoped to this remote when it has
# refs, because a commit already on another remote is not this push's to judge.
published="--remotes"
if [ -n "$remote" ] \
    && [ -n "$(git for-each-ref --count=1 --format='%(refname)' "refs/remotes/$remote/")" ]; then
    published="--remotes=$remote"
fi

# Known by its URL, because a clone names it origin or upstream depending on
# whether it was cloned from the fork or from here. Anchored at the host so a
# look-alike such as notgithub.com is not taken for it, with an optional port
# for GitHub's SSH fallback at ssh.github.com:443. Bracketed dots rather than
# escaped ones, since awk -v reads backslashes as string escapes.
upstream_url='(^|[@/.])github[.]com(:[0-9]+)?[:/]sysprog21/codetrial([.]git)?/?$'
upstream=$(git remote -v \
    | awk -v pattern="$upstream_url" 'tolower($2) ~ pattern { print $1; exit }')
to_upstream=0
printf '%s\n' "$url" | grep -Eiq "$upstream_url" && to_upstream=1

# A topic branch about to become a pull request is the moment a review is
# cheapest. Small changes get an advisory note; large topic branches require
# matching durable code-review and interactive-explanation evidence, and a push
# the policy covers is refused when there is no base to measure it against.
reviewed=$(git rev-parse --git-common-dir)/codetrial-review/reviewed
review_skill=$(git rev-parse --show-toplevel)/.claude/skills/codetrial-review/SKILL.md

# Fully qualified, so a local branch or tag named origin/main cannot shadow the
# remote-tracking ref, and never built from a URL, which names no such ref. When
# it is missing the change cannot be measured, so a push that the review policy
# covers is refused below until the base is fetched.
review_policy=refs/remotes/${upstream:-$remote}/main
git rev-parse --verify --quiet "$review_policy^{commit}" > /dev/null 2>&1 \
    || review_policy=
review_path=.claude/skills/codetrial-review/SKILL.md
# The description block for the pull request, written when the check passes.
block_dir=$(git rev-parse --git-common-dir)/codetrial-review

# One block per pushed head: a push of several branches would otherwise leave
# only the last branch's block, and the others would look unreviewed.
blocks=
note=

while read -r local_ref local_sha remote_ref remote_sha; do
    [ -n "${local_ref:-}" ] || continue
    [ "$local_sha" != "$zero" ] || continue
    git cat-file -e "${local_sha}^{commit}" 2> /dev/null || continue
    # A published upstream policy must not be reset by the outgoing branch.
    set -- verify --tip "$local_sha"
    if [ -n "$upstream" ] \
        && git cat-file -e "refs/remotes/$upstream/main:scripts/hashchain-base" 2> /dev/null; then
        anchor=$(git show "refs/remotes/$upstream/main:scripts/hashchain-base") || exit 1
        set -- "$@" --anchor "$anchor"
    fi
    if ! python3 "$script_dir/hashchain.py" "$@"; then
        echo "Push rejected: Change-Id hashchain failed for $local_ref." >&2
        failed=1
        continue
    fi

    # A pull request follows its head branch, so one opened from a fork's main
    # takes in every later push there, and the contributor cannot start a second
    # change until it merges. Syncing that main with upstream is still allowed:
    # it carries only commits upstream already has. Without an upstream remote
    # there is nothing to tell the two apart, and the check in CI covers the
    # pull request instead.
    if [ "$remote_ref" = refs/heads/main ] && [ "$to_upstream" -eq 0 ] \
        && [ -n "$upstream" ] \
        && [ -n "$(git rev-list -n 1 "$local_sha" --not "--remotes=$upstream")" ]; then
        echo "Push rejected: main on $remote would carry commits $upstream does not have." >&2
        echo "Push a topic branch instead: git push $remote HEAD:refs/heads/<topic>" >&2
        echo "Only syncing? Run git fetch $upstream first." >&2
        failed=1
        continue
    fi

    # The remote tip is whatever the other side advertised, which a clone that
    # has not fetched since does not have. Judging what is unpublished anywhere
    # is the same question asked a wider way, and it beats refusing the push
    # over an object nobody here can read.
    if [ "$remote_sha" = "$zero" ] \
        || ! git cat-file -e "${remote_sha}^{commit}" 2> /dev/null; then
        commits=$(git rev-list --no-merges "$local_sha" --not "$published")
    else
        commits=$(git rev-list --no-merges "${remote_sha}..${local_sha}")
    fi || {
        echo "Push rejected: cannot list commits for $local_ref" >&2
        failed=1
        continue
    }

    # Include merge resolutions and commits whose tip later removes a secret.
    # What is already published is not going out again, so a merge of upstream
    # work does not rescan it, and a first push to a new remote does not rescan
    # the whole history another remote holds.
    set -- "$local_sha" --not
    if git cat-file -e "${remote_sha}^{commit}" 2> /dev/null; then
        set -- "$@" "$remote_sha"
    fi
    set -- "$@" "$published"
    secret_commits=$(git rev-list "$@") || {
        echo "Push rejected: cannot inspect outgoing history." >&2
        failed=1
        continue
    }
    if [ -n "$secret_commits" ] && ! command -v python3 > /dev/null 2>&1; then
        echo "Push rejected: python3 is required for the secret check." >&2
        failed=1
        continue
    fi
    if [ -n "$secret_commits" ] && ! printf '%s\n' "$secret_commits" \
        | python3 "$script_dir/check-secrets.py" --commits; then
        echo "Push rejected: outgoing history failed the secret check." >&2
        failed=1
        continue
    fi

    # No new commit to judge (a head already published, or one only a merge
    # moved) still gets the review note below.
    if [ -n "$commits" ] \
        && ! printf '%s\n' "$commits" | "$script_dir/check-commit-log.sh"; then
        echo "Push rejected for $local_ref -> $remote_ref." >&2
        failed=1
        continue
    fi

    # A topic branch only: main has its own rule above, and a tag never heads a
    # pull request.
    case $remote_ref in
        refs/heads/main) ;;
        refs/heads/*)
            review_enabled=0
            if [ -e "$review_skill" ] || [ -e "${review_skill%/*}/commit-sizes.py" ] \
                || git cat-file -e "$local_sha:$review_path" 2> /dev/null \
                || {
                    [ -n "$review_policy" ] \
                        && git cat-file -e "$review_policy:$review_path" 2> /dev/null
                }; then
                review_enabled=1
            fi

            # Skipping the check for want of a base would let any large change
            # through, so the base has to be there.
            if [ "$review_enabled" -eq 1 ] && [ -z "$review_policy" ]; then
                echo "Push rejected: no remote-tracking main to measure $local_ref against." >&2
                if git remote | grep -qxF "${upstream:-$remote}"; then
                    echo "Run git fetch ${upstream:-$remote}, then push again." >&2
                else
                    echo "Add the upstream remote and fetch it, then push again: git remote add upstream https://github.com/sysprog21/codetrial && git fetch upstream" >&2
                fi
                failed=1
                continue
            fi
            if [ "$review_enabled" -eq 1 ]; then

                # Only this push's block is worth pasting; one from an earlier
                # push, or from another head, would describe something else.
                pr_block=$block_dir/pr-description-$(printf '%.12s' "$local_sha").md

                # The plain name belongs to a single-head push that passed; one
                # from an earlier push must not outlive a refused or wider push.
                rm -f "$pr_block" "$block_dir/pr-description.md"
                if ! command -v python3 > /dev/null 2>&1 \
                    || ! python3 "$script_dir/check-interactive-review.py" push \
                        --head "$local_sha" --base "$review_policy" --out "$pr_block"; then
                    echo "Push rejected: interactive review check failed for $local_ref." >&2
                    failed=1
                    continue
                fi
                [ ! -f "$pr_block" ] || blocks="$blocks$pr_block
"
            fi
            if [ -z "$note" ] && [ "$review_enabled" -eq 1 ] \
                && ! grep -qx "$local_sha" "$reviewed" 2> /dev/null; then
                note="Note: $(git rev-parse --short "$local_sha") has no review on record;"
                note="$note in Claude Code, /codetrial-review checks it before a pull request does."
            fi
            ;;
    esac
done << EOF
$refs
EOF

if [ "$failed" -eq 0 ] && [ -n "$blocks" ]; then
    count=$(printf '%s' "$blocks" | grep -c . || true)
    if [ "$count" -eq 1 ]; then
        # The usual push: one branch, and one file with a name to remember.
        only=$(printf '%s' "$blocks" | head -n 1)
        if cp "$only" "$block_dir/pr-description.md"; then
            echo "Pull request description: paste the interactive review block from" >&2
            echo "  $block_dir/pr-description.md" >&2
            echo "into the pull request, replacing the placeholder block." >&2
        else
            echo "Push rejected: cannot write $block_dir/pr-description.md." >&2
            failed=1
        fi
    else
        echo "Pull request descriptions: each pushed head has its own interactive review block:" >&2
        printf '%s' "$blocks" | sed 's/^/  /' >&2
    fi
fi

# Said once the whole push is judged, so a push refused over another ref does
# not also advise a review of a head that is not going anywhere.
[ "$failed" -ne 0 ] || [ -z "$note" ] || echo "$note" >&2

exit "$failed"
