#!/bin/sh

# Download the vendored files a FETCH manifest names, and nothing else.
#
# SHA256SUMS stays the contract; a manifest only records where the bytes come
# from. Bytes are hashed in a temp file and moved into the vendor tree only
# after they match the pin, so an interrupted download or a bad mirror cannot
# leave the browser a wasm nobody pinned. A file already present and matching is
# left alone, which makes this a no-op on every checkout after the first.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VENDOR="$ROOT/web/vendor"

[ -d "$VENDOR" ] || exit 0

# Releases no longer embed or serve this model; browser Cache API owns it now.
# Remove the ignored file left by pre-migration checkouts on their next build.
rm -f "$VENDOR/avatar/jim.vrm"

download() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    echo "fetch-vendor: need curl or wget to fetch $1" >&2
    return 1
  fi
}

# macOS ships shasum but not sha256sum; most Linux distros ship the reverse.
# Bind one at startup rather than probing per file: a machine with neither
# should say so before the first download, not hash an empty string afterwards
# and blame the mirror. sha256sum is tried first because shasum is a Perl
# script, so it is the slower of the two when both are present.
if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  echo "fetch-vendor: need sha256sum or shasum" >&2
  exit 1
fi

# An interrupted download leaves a `.part` file behind, and verify-vendor
# rejects any file in the tree SHA256SUMS does not pin, so it would fail the
# gate until someone deleted it by hand. Only this run's own temp is removed:
# sweeping the directory would delete the in-flight temp of a concurrent run,
# which is the race the per-process name below exists to avoid.
#
# Inline rather than a `cleanup` function the way the other scripts here write
# it. A function only the trap calls looks unreachable to shellcheck, and the
# two versions in play disagree about what to call that: 0.9.0, which is what
# the CI runner has, reports SC2317 on the body, while 0.11.0 reports SC2329 on
# the declaration. Suppressing both spellings is a note that goes stale on the
# next release. Having no function to misread does not.
tmp=
trap '[ -z "$tmp" ] || rm -f "$tmp"' EXIT INT TERM HUP

fetch_manifest() {
  manifest=$1
  dir=$(dirname "$manifest")
  sums="$dir/SHA256SUMS"

  if [ ! -f "$sums" ]; then
    echo "fetch-vendor: ${manifest#"$ROOT"/} has no SHA256SUMS beside it" >&2
    return 1
  fi

  base=""
  while read -r line || [ -n "$line" ]; do
    case "$line" in
      '' | '#'*) continue ;;
    esac

    if [ -z "$base" ]; then
      base=$line
      continue
    fi

    # One filename per line, serving as both the URL suffix and the path under
    # the vendor directory, so it takes only what a vendored filename can be.
    # This rejects a stray carriage return from a CRLF checkout, which would
    # otherwise 404 against a URL that looks correct in the error message; it
    # rejects a `/` before one can write outside `web/vendor/`; and it rejects a
    # second field, since the space comes through in the match.
    #
    # There used to be a two-field form here, for the one asset whose upstream
    # name differed from the one this tree served. That was the avatar model,
    # which the browser now fetches for itself, and no manifest has used a
    # second field since.
    name=$line
    case "$name" in
      *[!A-Za-z0-9._-]* | .*)
        echo "fetch-vendor: ${manifest#"$ROOT"/} names an unusable file: $line" >&2
        return 1
        ;;
    esac

    want=$(awk -v name="$name" '$2 == name { print $1 }' "$sums")
    if [ -z "$want" ]; then
      echo "fetch-vendor: $name is fetched but not pinned in ${sums#"$ROOT"/}" >&2
      return 1
    fi

    if [ -f "$dir/$name" ] && [ "$(sha256 "$dir/$name")" = "$want" ]; then
      continue
    fi

    echo "fetch-vendor: $name"

    # Per-process, because `make -j` can run the fetch-vendor prerequisite of
    # build, serve, and web concurrently and a fixed name is a race.
    tmp="$dir/.$name.$$.part"
    download "$base$name" "$tmp" || {
      rm -f "$tmp"
      return 1
    }

    got=$(sha256 "$tmp")
    if [ "$got" != "$want" ]; then
      rm -f "$tmp"
      echo "fetch-vendor: $name hashed $got, pinned $want" >&2
      return 1
    fi

    if ! mv "$tmp" "$dir/$name"; then

      # errexit is suppressed for this whole function: it is called in a
      # `|| status=1` list. Without this check a failed move left the old bytes
      # in place, dropped a `.part` file that then fails verify-vendor forever,
      # and still exited 0.
      rm -f "$tmp"
      echo "fetch-vendor: could not install $name into ${dir#"$ROOT"/}" >&2
      return 1
    fi
  done <"$manifest"
}

status=0
for manifest in "$VENDOR"/FETCH "$VENDOR"/*/FETCH; do
  [ -f "$manifest" ] || continue
  fetch_manifest "$manifest" || status=1
done

exit "$status"
