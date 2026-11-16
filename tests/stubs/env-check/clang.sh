#!/bin/sh
# Stands in for a C++ compiler; FAKE_CLANG_VERSION is the major version it
# reports, in the shape both Clang and Chromium's build print.
echo "clang version ${FAKE_CLANG_VERSION:-21}.1.0 (stub)"
