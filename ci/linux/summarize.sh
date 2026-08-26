#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Write a GitHub Step Summary table from ci-results/*.json.

set -euo pipefail

RESULTS_DIR="${1:-ci-results}"
OUT="${GITHUB_STEP_SUMMARY:-/dev/stdout}"

escape() {
  printf '%s' "$1" | tr '\n' ' ' | sed 's/|/\\|/g'
}

if [[ ! -d "${RESULTS_DIR}" ]]; then
  echo "No result artifacts." >> "${OUT}"
  exit 0
fi

# download-artifact with merge-multiple may nest files.
mapfile -t files < <(find "${RESULTS_DIR}" -name '*.json' -type f | sort)

if [[ ${#files[@]} -eq 0 ]]; then
  echo "No result JSON files." >> "${OUT}"
  exit 0
fi

{
  echo '| Distro | Version | Codename | Status | Error |'
  echo '| --- | --- | --- | --- | --- |'
  for f in "${files[@]}"; do
    distro="$(jq -r '.distro // ""' "${f}")"
    version="$(jq -r '.version // ""' "${f}")"
    codename="$(jq -r '.codename // ""' "${f}")"
    status="$(jq -r '.status // ""' "${f}")"
    error="$(jq -r '.error // ""' "${f}")"
    printf '| %s | %s | %s | %s | %s |\n' \
      "$(escape "${distro}")" \
      "$(escape "${version}")" \
      "$(escape "${codename}")" \
      "$(escape "${status}")" \
      "$(escape "${error}")"
  done
} >> "${OUT}"
