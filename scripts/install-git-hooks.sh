#!/bin/sh

# Install wrappers for every scripts/git-*.sh into the repository's hooks
# directory. A worktree shares its hooks directory with its siblings, so each
# wrapper finds the worktree that invoked it instead of naming one by path.

set -u

mode=install
case "${1:-}" in
    "") ;;
    --uninstall) mode=uninstall ;;
    *)
        echo "usage: ${0##*/} [--uninstall]" >&2
        exit 2
        ;;
esac

ROOT=$(git rev-parse --show-toplevel) || exit 1
hooks=$(git rev-parse --path-format=absolute --git-path hooks) || exit 1
default_hooks=$(git rev-parse --path-format=absolute --git-common-dir)/hooks
failed=0

wrapper()
{
    printf '%s\n' '#!/bin/sh' \
        "exec \"\$(git rev-parse --show-toplevel)/scripts/git-$1.sh\" \"\$@\""
}

ours()
{
    [ -f "$1" ] && wrapper "$2" | cmp -s - "$1"
}

# Before wrappers, this installer made absolute links into a worktree. Accept
# only one that names a worktree Git says belongs to this repository, so an
# unrelated local hook remains untouched while an upgrade fixes the old link.
legacy_ours()
{
    [ -L "$1" ] || return 1
    link=$(readlink "$1") || return 1
    base=${link%/scripts/git-"$2".sh}
    [ "$base" != "$link" ] && [ -d "$base" ] || return 1
    worktree=$(git -C "$base" rev-parse --show-toplevel 2> /dev/null) || return 1
    git worktree list --porcelain | grep -Fqx "worktree $worktree"
}

if [ "$mode" = uninstall ]; then
    for target in "$hooks"/*; do
        ours "$target" "${target##*/}" \
            || legacy_ours "$target" "${target##*/}" || continue
        rm -f "$target"
        printf '  RM      %s\n' "${target##*/}"
    done
    exit 0
fi

mkdir -p "$hooks" || exit 1
for hook in "$ROOT"/scripts/git-*.sh; do
    name=${hook##*/git-}
    name=${name%.sh}
    target="$hooks/$name"

    # An existing hook is somebody's, even when it looks like ours: overwriting
    # it is how a local workflow disappears without anyone noticing.
    if ours "$target" "$name"; then

        # Content is not enough: git skips a hook without the executable bit and
        # says nothing, so a wrapper that lost it reads as installed while
        # nothing runs.
        chmod +x "$target" || failed=1
        printf '  OK      %s\n' "$name"
    elif legacy_ours "$target" "$name" \
        && rm -f "$target" \
        && wrapper "$name" > "$target" \
        && chmod +x "$target"; then
        printf '  HOOK    %s\n' "$name"
    elif [ -e "$target" ] || [ -L "$target" ]; then
        printf '  KEEP    %s already exists; remove it to install ours\n' "$target"
    elif wrapper "$name" > "$target" && chmod +x "$target"; then
        printf '  HOOK    %s\n' "$name"
    else
        failed=1
    fi
done

# core.hooksPath sends hooks somewhere else, and a wrapper in the default place
# would then be installed and silent. Say so rather than let it look installed.
if [ "$hooks" != "$default_hooks" ]; then
    printf '  NOTE    core.hooksPath points hooks at %s\n' "$hooks"
fi

exit "$failed"
