#!/bin/sh

# Install one binary out of a GitHub release, checked against the digest the
# release records.
#
# Two workflow steps install a tool this way and the part that matters is the
# same in both: an asset the API records no digest for is an asset this cannot
# check, and a download nothing verified is not worth installing. Written into
# each step, that rule was two copies of a safety check, where a later fix to
# one leaves the other as it was -- the shape of bug the workflow linter those
# steps install exists to catch elsewhere.
#
# The digest is read from the API rather than pinned here. A hash written into
# this file has to be recomputed by hand on every upstream republish, and it
# fails as two hex strings that differ, which says nothing about what moved.
# What that buys is a check on the transfer, since the digest does not come from
# the CDN path the tarball travels; what it does not buy is a frozen version,
# which is the caller's business and is why the tag is an argument.
#
# CI's installer rather than a general one: it writes to /usr/local/bin through
# sudo, which is where a runner's PATH already looks and which a runner grants
# without a password.

set -eu

if [ "$#" -ne 4 ]; then
    echo "usage: ${0##*/} REPO TAG ASSET BINARY" >&2
    exit 2
fi

repo=$1
tag=$2
asset=$3
binary=$4

meta=$(gh api "repos/$repo/releases/tags/$tag")

digest=$(printf '%s' "$meta" \
    | jq -r --arg n "$asset" '.assets[] | select(.name == $n) | .digest // ""')
url=$(printf '%s' "$meta" \
    | jq -r --arg n "$asset" \
        '.assets[] | select(.name == $n) | .browser_download_url // ""')
if [ -z "$digest" ] || [ -z "$url" ]; then
    echo "no digest or download URL recorded for $asset" >&2
    exit 1
fi

# Named by mktemp rather than after the asset: two of these run in one job, and
# a fixed name in /tmp is a collision waiting for a third.
archive=$(mktemp)
trap 'rm -f "$archive"' EXIT

curl -fsSL -o "$archive" "$url"
echo "${digest#sha256:}  $archive" | sha256sum -c -
sudo tar -xzf "$archive" -C /usr/local/bin "$binary"
