#!/bin/sh

# Judge the commit messages this push would publish. The commit-msg hook only
# sees a message as it is written, so a rebase, an amend, or a commit made with
# --no-verify reaches the remote unread. This is the last place to catch that
# while the history is still local and cheap to rewrite.
#
# It also checks the formatting of a topic branch with scripts/indent.sh, the
# pre-commit hook's own check, for the commits that never passed through it.
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
publish_only=0
if [ "$remote" = --publish-review ]; then

    # Run by hand, for a branch already pushed: judge its head as a push would
    # and send the record, which only a push of the branch carries otherwise.
    [ "$#" -eq 2 ] || [ "$#" -eq 3 ] || {
        echo "Usage: $0 --publish-review <remote> [<branch on the remote>]" >&2
        exit 1
    }
    publish_only=1
    remote=$2
    url=$(git remote get-url --push "$remote") || exit 1
    branch=$(git symbolic-ref --quiet HEAD) || {
        echo "HEAD is detached; check out the topic branch the pull request is from." >&2
        exit 1
    }
    target=${3:-$branch}
    target=refs/heads/${target#refs/heads/}
    [ "$target" != refs/heads/main ] || {
        echo "A pull request comes from a topic branch, not main; name it: $0 --publish-review $remote <branch>" >&2
        exit 1
    }
    head=$(git rev-parse HEAD) || exit 1

    # The record is for the head the pull request has, so the two must agree,
    # read from where the record goes: the push URL, which may differ from the
    # one fetched from.
    listed=$(GIT_TERMINAL_PROMPT=0 git ls-remote "$url" "$target") || {
        echo "Cannot read ${target#refs/heads/} from $remote; check that you can reach it." >&2
        exit 1
    }
    there=$(printf '%s\n' "$listed" | awk -v ref="$target" '$2 == ref { print $1; exit }')
    if [ -z "$there" ]; then
        echo "$remote has no branch ${target#refs/heads/}." >&2
        echo "If the pull request's branch has another name there, name it: $0 --publish-review $remote <branch>" >&2
        echo "If it was never pushed, push it, which sends the record with it." >&2
        exit 1
    fi
    if [ "$there" != "$head" ]; then
        echo "$remote has $(printf %.12s "$there") on ${target#refs/heads/}, not HEAD $(printf %.12s "$head")." >&2
        echo "Push the branch instead, which sends the record with it: git push $remote HEAD:$target" >&2
        exit 1
    fi
    refs="$branch $head $target $head"
else
    refs=$(cat)
fi

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

# Formatting is the pre-commit hook's, and a rebase, a rechain or --no-verify
# publishes commits it never saw. This is where those paths meet, so the files a
# push publishes are judged here by the same script, against the index a
# checkout of the pushed head would have. $1 is that head, $2 the commit the
# push builds on, empty when it builds on nothing.
check_format()
{
    dir=$(mktemp -d) || return 1
    (
        for variable in $(git rev-parse --local-env-vars); do
            # The object store is how the pushed head is found.
            case $variable in
                GIT_OBJECT_DIRECTORY | GIT_ALTERNATE_OBJECT_DIRECTORIES) ;;
                *) unset "$variable" ;;
            esac
        done
        GIT_INDEX_FILE=$dir/index
        export GIT_INDEX_FILE
        mkdir "$dir/tree" \
            && git read-tree "$1" \
            && git checkout-index -a --prefix="$dir/tree/" \
            && "$script_dir/indent.sh" --staged "$dir/tree" \
                "${2:-$(git hash-object -t tree /dev/null)}" > /dev/null
    )
    status=$?
    rm -rf "$dir"
    return "$status"
}

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
note=
test_note=
review_notes=refs/notes/codetrial-review
publish_review=0
review_heads=

while read -r local_ref local_sha remote_ref remote_sha; do
    [ -n "${local_ref:-}" ] || continue
    [ "$local_sha" != "$zero" ] || continue

    # The review record this hook sends below carries no code and no commit
    # message to judge, and judging it would loop back into this hook.
    [ "$remote_ref" != "$review_notes" ] || continue
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

            # The formatting base is where the branch left main, so a rebase or
            # a force push is judged by its own files and not by everything main
            # took in meanwhile. Without a main to measure from, the remote tip
            # stands in, and for a new branch the parent of its oldest outgoing
            # commit.
            format_base=
            if [ -n "$review_policy" ]; then
                format_base=$(git merge-base "$local_sha" "$review_policy" 2> /dev/null || :)
            fi
            if [ -z "$format_base" ] && git cat-file -e "${remote_sha}^{commit}" 2> /dev/null; then
                format_base=$(git merge-base "$remote_sha" "$local_sha" 2> /dev/null || :)
            fi
            if [ -z "$format_base" ] && [ -n "$secret_commits" ]; then
                format_base=$(printf '%s\n' "$secret_commits" | sed -n '$p')
                format_base=$(git rev-parse --verify --quiet "$format_base^" || :)
            fi

            # indent.sh alone knows which files and settings a formatter reads,
            # and it checks nothing when the branch changes none of them. Merges
            # count: a resolution can bring in a file nobody formatted. A head
            # another remote already holds is judged here too, since the pull
            # request is opened from this one.
            if [ "$local_sha" != "$remote_sha" ] && ! check_format "$local_sha" "$format_base"; then
                echo "Push rejected: $local_ref has unformatted files." >&2
                echo "Fix them in the commit that introduced them; a rebase skips the pre-commit hook." >&2
                failed=1
                continue
            fi

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

                if ! command -v python3 > /dev/null 2>&1 \
                    || ! python3 "$script_dir/check-interactive-review.py" push \
                        --head "$local_sha" --base "$review_policy"; then
                    echo "Push rejected: interactive review check failed for $local_ref." >&2
                    failed=1
                    continue
                fi

                # A large change that passed has a completed, signed marker on
                # its head; CI reads it from this remote. A code owner who
                # skipped the review leaves only a pending one, which stays
                # here.
                if [ "$(python3 "$script_dir/check-interactive-review.py" marker \
                    --head "$local_sha")" = complete ]; then
                    publish_review=1
                    review_heads="$review_heads $local_sha"
                fi
            fi
            if [ -z "$note" ] && [ "$review_enabled" -eq 1 ] \
                && ! grep -qx "$local_sha" "$reviewed" 2> /dev/null; then
                note="Note: $(git rev-parse --short "$local_sha") has no review on record;"
                note="$note in Claude Code, /codetrial-review checks it before a pull request does."
            fi

            # A fix with no test beside it is the habit codetrial-improve asks
            # an agent to break, and CONTRIBUTING.md keeps test bodies under
            # tests/. Advisory: a refactor that pins behavior already covered,
            # or a comment-only edit, legitimately has none.
            if [ -z "$test_note" ] && [ -n "$review_policy" ]; then
                changed=$(git diff --name-only "$review_policy...$local_sha" 2> /dev/null) || changed=
                if printf '%s\n' "$changed" | grep -q '^src/.*[.]rs$' \
                    && ! printf '%s\n' "$changed" | grep -q '^tests/'; then
                    test_note="Note: $local_ref changes src/ and nothing under tests/;"
                    test_note="$test_note write the failing test first (codetrial-improve), or say why none fits in the PR."
                fi
            fi
            ;;
    esac
done << EOF
$refs
EOF

# The interactive-review check reads the marker from the remote the branch goes
# to, so it is sent first and the pull request event finds it there. Records the
# remote already holds for other branches are merged in, not overwritten. A
# record that cannot be sent would turn the check red, so the push stops here.
if [ "$failed" -eq 0 ] && [ "$publish_review" -eq 1 ]; then
    incoming=refs/notes/codetrial-review-incoming

    # The records signed here are kept by blob: when the remote is ahead, the
    # merge fast-forwards to its notes, and -s ours never sees a conflict.
    signed=
    for sha in $review_heads; do
        signed="$signed $sha:$(git notes --ref="$review_notes" list "$sha")"
    done
    if GIT_TERMINAL_PROMPT=0 git fetch -q --no-tags "$url" \
        "+$review_notes:$incoming" 2> /dev/null; then

        # A merge left half done would block every later push, so it is
        # abandoned on failure; the records here are unchanged by it.
        if ! git notes --ref="$review_notes" merge -q -s ours "$incoming"; then
            git notes --ref="$review_notes" merge --abort 2> /dev/null
            failed=1
        fi
        for pair in $signed; do
            sha=${pair%%:*}
            [ "$failed" -eq 0 ] || break
            [ "$(git notes --ref="$review_notes" list "$sha" 2> /dev/null)" = "${pair#*:}" ] \
                || git notes --ref="$review_notes" add -f -C "${pair#*:}" "$sha" \
                || failed=1
        done
        git update-ref -d "$incoming"
    fi
    if [ "$failed" -ne 0 ] \
        || ! GIT_TERMINAL_PROMPT=0 git push -q "$remote" "$review_notes:$review_notes"; then
        echo "Push rejected: cannot send the interactive review record to $remote." >&2
        echo "Check that you can push to $remote, then push again." >&2
        failed=1
    fi
fi

# By hand, say what happened: nothing else would, and a check that has already
# failed does not rerun for a record alone. The messages above say "Push
# rejected", but here no branch was pushed.
if [ "$publish_only" -eq 1 ]; then
    if [ "$failed" -ne 0 ]; then
        echo "Nothing was sent; the branch on $remote is unchanged." >&2
    elif [ "$publish_review" -eq 1 ]; then
        echo "Sent the interactive review record for $(printf %.12s "$head") to $remote." >&2
        echo "Close and reopen the pull request to rerun its interactive-review check." >&2
    else
        # The check above said why, when a code owner skipped the review.
        echo "Nothing sent: $(printf %.12s "$head") has no completed interactive review record to send, and its push check passed without one." >&2
    fi
fi

# Said once the whole push is judged, so a push refused over another ref does
# not also advise a review of a head that is not going anywhere.
[ "$failed" -ne 0 ] || [ -z "$note" ] || echo "$note" >&2
[ "$failed" -ne 0 ] || [ -z "$test_note" ] || echo "$test_note" >&2

exit "$failed"
