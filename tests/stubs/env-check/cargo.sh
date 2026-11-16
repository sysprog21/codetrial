#!/bin/sh
# FAKE_FORBID_RUN names a file this stub appends to and then fails, for a case
# that must prove the tool was never run.
[ -z "${FAKE_FORBID_RUN:-}" ] || {
    echo ran >> "$FAKE_FORBID_RUN"
    exit 99
}

# Stands in for cargo. FAKE_CARGO_VERSION is the number `-V` reports and
# FAKE_CARGO_MISSING names the subcommands whose `--version` fails, the way a
# toolchain installed without those components does. FAKE_CARGO_BROKEN makes
# `-V` itself fail.
case "$1" in
    -V)
        [ -z "${FAKE_CARGO_BROKEN:-}" ] || exit 1
        echo "cargo ${FAKE_CARGO_VERSION:-1.100.0} (stub)"
        ;;
    clippy | fmt)
        case " ${FAKE_CARGO_MISSING:-} " in
            *" $1 "*)
                echo "error: no such command: \`$1\`" >&2
                exit 101
                ;;
        esac
        echo "cargo-$1 ${FAKE_CARGO_VERSION:-1.100.0} (stub)"
        ;;
    *)
        exit 101
        ;;
esac
