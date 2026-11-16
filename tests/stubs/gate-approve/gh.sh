#!/bin/sh
# gh for the gate-evidence and approval tests. It logs every call to $FAKE_LOG
# and serves the pull request, its files, the head commit and the two
# rust-toolchain.toml files from $FAKE_DIR. After the first read of the pull
# request, pull-after.json (when it exists) answers instead, as a push between
# the check and the approval would. Any write is logged as `WRITE`.
echo "$*" >> "$FAKE_LOG"
dir=$FAKE_DIR

pull()
{
    reads=$(cat "$dir/reads" 2> /dev/null || echo 0)
    echo $((reads + 1)) > "$dir/reads"
    if [ "$reads" -ge 1 ] && [ -f "$dir/pull-after.json" ]; then
        cat "$dir/pull-after.json"
    else
        cat "$dir/pull.json"
    fi
}

case "$*" in
    "api --paginate repos/"*"/pulls/"*"/files")
        [ -z "${FAKE_FILES_FAIL:-}" ] || exit 1
        cat "$dir/files.json"
        ;;
    "api repos/"*"/pulls/"*" -f "* | "api repos/"*"/pulls/"*" -F "*)
        # A field with no method is a POST, whatever the path.
        echo WRITE >> "$FAKE_LOG"
        ;;
    "api repos/"*"/pulls/"*)
        [ -z "${FAKE_PULL_FAIL:-}" ] || exit 1
        case "$*" in
            *"--jq .head.sha") pull | jq -r .head.sha ;;
            *) pull ;;
        esac
        ;;
    "api repos/"*"/git/commits/"*" --jq .tree.sha")
        [ -z "${FAKE_TREE_FAIL:-}" ] || exit 1
        cat "$dir/tree"
        ;;
    "api -X GET -H "*"/contents/rust-toolchain.toml -f ref="*)
        all="$*"
        ref=${all##*-f ref=}
        if [ "$ref" = "$(jq -r .head.sha "$dir/pull.json")" ]; then
            cat "$dir/head-toolchain"
        else
            [ -z "${FAKE_BASE_TOOLCHAIN_FAIL:-}" ] || exit 1
            cat "$dir/base-toolchain"
        fi
        ;;
    "api -X POST "*)
        echo WRITE >> "$FAKE_LOG"
        ;;
    *)
        echo "unexpected gh call: $*" >&2
        exit 1
        ;;
esac
