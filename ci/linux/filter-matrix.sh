#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Print a compact JSON array of versions.json entries, optionally limited to
# one distro and/or one version. Empty filters mean "everything".

set -euo pipefail

DISTRO_FILTER="${1:-}"
VERSION_FILTER="${2:-}"
FILE="${3:-ci/linux/versions.json}"

jq -c --arg d "${DISTRO_FILTER}" --arg v "${VERSION_FILTER}" '
  map(select(
    ($d == "" or .distro == $d) and
    ($v == "" or .version == $v)
  ))
' "${FILE}"
