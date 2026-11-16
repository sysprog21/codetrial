#!/bin/sh
# FAKE_FORBID_RUN names a file this stub appends to and then fails, for a case
# that must prove the tool was never run.
[ -z "${FAKE_FORBID_RUN:-}" ] || {
    echo ran >> "$FAKE_FORBID_RUN"
    exit 99
}

# Stands in for rustc; FAKE_RUSTC_VERSION is the number `-V` reports and
# FAKE_RUSTC_BROKEN makes `-V` fail and FAKE_RUSTC_ODD makes it succeed with a
# line that has no version.
[ -z "${FAKE_RUSTC_BROKEN:-}" ] || {
    echo "rustc is broken" >&2
    exit 1
}
[ -z "${FAKE_RUSTC_ODD:-}" ] || {
    echo "a wrapper banner"
    exit 0
}
echo "rustc ${FAKE_RUSTC_VERSION:-1.100.0} (0123abcd 2026-01-01)"
