#!/bin/sh
# Stands in for rustup. Only `toolchain list` is understood, and like the real
# one it must not be asked from a directory with a pin, where rustup would try
# to install the pinned toolchain. FAKE_RUSTUP_LIST is what it prints and
# FAKE_RUSTUP_BROKEN makes it fail.
[ "$*" = "toolchain list" ] || exit 2
if [ -f rust-toolchain.toml ]; then
    echo "rustup was run inside a directory with a pin" >&2
    exit 1
fi
[ -z "${FAKE_RUSTUP_BROKEN:-}" ] || exit 1
printf '%s\n' "${FAKE_RUSTUP_LIST:-}"
