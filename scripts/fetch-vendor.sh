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

sha256() {
  shasum -a 256 "$1" | cut -d' ' -f1
}

# An interrupted download leaves a `.part` file behind, and verify-vendor
# rejects any file in the tree SHA256SUMS does not pin, so it would fail the
# gate until someone deleted it by hand. Only this run's own temp is removed:
# sweeping the directory would delete the in-flight temp of a concurrent run,
# which is the race the per-process name below exists to avoid.
tmp=
cleanup() {
  [ -z "$tmp" ] || rm -f "$tmp"
}
trap cleanup EXIT INT TERM HUP

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

    # Two fields mean the upstream name differs from the one this tree serves:
    # the avatar is Seed-san.vrm at its source and jim.vrm here, and renaming it
    # locally would be a rename in every reference to it. One field, which is
    # every other line in every manifest, means the two names are the same.
    name=${line%% *}
    remote=${line#"$name"}
    remote=${remote# }
    [ -n "$remote" ] || remote=$name

    # Both halves are used as a URL suffix and as a path under the vendor
    # directory, so each takes only what a vendored filename can be. This
    # rejects a stray carriage return from a CRLF checkout, which would
    # otherwise 404 against a URL that looks correct in the error message, and
    # it rejects a `/` before one can write outside `web/vendor/`. Checking both
    # also rejects a third field, which arrives here glued to the second.
    for field in "$name" "$remote"; do
      case "$field" in
        *[!A-Za-z0-9._-]* | .*)
          echo "fetch-vendor: ${manifest#"$ROOT"/} names an unusable file: $line" >&2
          return 1
          ;;
      esac
    done

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
    download "$base$remote" "$tmp" || {
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
