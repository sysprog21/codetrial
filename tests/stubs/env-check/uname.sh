#!/bin/sh
# Stands in for uname on a host the suite chooses, so a case does not depend on
# the machine it runs on.
kernel=${FAKE_UNAME_S:-Linux}
machine=${FAKE_UNAME_M:-x86_64}
case "${1:-}" in
    -s) echo "$kernel" ;;
    -m) echo "$machine" ;;
    *) echo "$kernel $machine" ;;
esac
