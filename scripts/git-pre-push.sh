#!/bin/sh

# Judge the commit messages this push would publish. The commit-msg hook only
# sees a message as it is written, so a rebase, an amend, or a commit made with
# --no-verify reaches the remote unread. This is the last place to catch that
# while the history is still local and cheap to rewrite.

set -u

remote=${1:-}

# From the repository, not from $0: git invokes the hook through the symlink in
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

while read -r local_ref local_sha remote_ref remote_sha; do
    [ -n "${local_ref:-}" ] || continue
    [ "$local_sha" != "$zero" ] || continue
    git cat-file -e "${local_sha}^{commit}" 2> /dev/null || continue

    if [ "$remote_sha" = "$zero" ]; then
        commits=$(git rev-list --no-merges "$local_sha" --not "$published")
    else
        commits=$(git rev-list --no-merges "${remote_sha}..${local_sha}")
    fi || {
        echo "Push rejected: cannot list commits for $local_ref" >&2
        failed=1
        continue
    }

    [ -n "$commits" ] || continue
    printf '%s\n' "$commits" | "$script_dir/check-commit-log.sh" || {
        echo "Push rejected for $local_ref -> $remote_ref." >&2
        failed=1
    }
done

exit "$failed"
