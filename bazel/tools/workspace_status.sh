#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Emits the values //src/OrbitVersion bakes into the binary. Bazel runs this on
# every invocation and only re-runs the actions that consume it when a value
# actually changes, which is what cmake/version.cmake approximates by hand with
# its .state file.
#
# Everything here is derived from the commit rather than from wall-clock time,
# so an unchanged checkout produces an unchanged binary.

set -euo pipefail

git_or() {
  git "$@" 2>/dev/null || true
}

# Release tags in this repository are written v1.0.2; upstream has used a bare
# 1.x as well, so match both.
version_string="$(git_or describe --always --match '1.*' --match 'v1.*')"
commit_hash="$(git_or show -s --format=%H)"
commit_date="$(git_or show -s --format=%cd --date=format-local:'%Y-%m-%dT%H:%M:%SZ')"

# The version numbers are parsed off the tag, with the optional v dropped.
numeric_version="${version_string#v}"
major_version="$(sed -n 's/^\([0-9]\+\)\..*/\1/p' <<<"${numeric_version}")"
minor_version="$(sed -n 's/^[0-9]\+\.\([0-9]\+\).*/\1/p' <<<"${numeric_version}")"

echo "STABLE_ORBIT_VERSION_STRING ${version_string:-unknown}"
echo "STABLE_ORBIT_MAJOR_VERSION ${major_version:-0}"
echo "STABLE_ORBIT_MINOR_VERSION ${minor_version:-0}"
echo "STABLE_ORBIT_COMMIT_HASH ${commit_hash:-unknown}"
echo "STABLE_ORBIT_BUILD_TIMESTAMP ${commit_date:-unknown}"
echo "STABLE_ORBIT_COMPILER $("${CC:-cc}" --version 2>/dev/null | head -1)"
echo "STABLE_ORBIT_BUILD_MACHINE $(uname -n)"
echo "STABLE_ORBIT_BUILD_OS_NAME $(uname -s)"
echo "STABLE_ORBIT_BUILD_OS_RELEASE $(uname -r)"
echo "STABLE_ORBIT_BUILD_OS_VERSION $(uname -v)"
echo "STABLE_ORBIT_BUILD_OS_PLATFORM $(uname -m)"
