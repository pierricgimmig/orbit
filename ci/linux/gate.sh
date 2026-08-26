#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Decide whether the nightly matrix should run.
#
# workflow_dispatch always builds.
# schedule compares the current tree to the last completed run of this
# workflow on main. A red matrix still counts as a baseline: many cells are
# expected to fail, and nightly must no-op unless relevant files changed.

set -euo pipefail

EVENT_NAME="${1:-${GITHUB_EVENT_NAME:-}}"
OUTPUT="${GITHUB_OUTPUT:-/dev/stdout}"

write() {
  printf '%s\n' "$1" >> "${OUTPUT}"
}

summarize() {
  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    printf '%s\n' "$1" >> "${GITHUB_STEP_SUMMARY}"
  fi
}

if [[ "${EVENT_NAME}" == "workflow_dispatch" ]]; then
  echo "workflow_dispatch: always run the matrix"
  write "run=true"
  write "reason=workflow_dispatch"
  write "baseline="
  summarize "workflow_dispatch: running the matrix on this ref (change gate skipped)."
  exit 0
fi

WORKFLOW_FILE="${WORKFLOW_FILE:-linux-distro-matrix.yml}"
BRANCH="${GATE_BRANCH:-main}"
CURRENT_SHA="${GITHUB_SHA:-$(git rev-parse HEAD)}"
CURRENT_RUN="${GITHUB_RUN_ID:-}"

PATHS=(
  src
  bazel
  third_party
  .bazelrc
  .bazelversion
  MODULE.bazel
  MODULE.bazel.lock
  ci/linux
  .github/workflows/linux-distro-matrix.yml
)

if ! command -v gh >/dev/null 2>&1; then
  echo "gh is not available; running the matrix"
  write "run=true"
  write "reason=gh-unavailable"
  write "baseline="
  summarize "gh unavailable; running the matrix."
  exit 0
fi

# Last completed run on main, success or failure. Cancelled / skipped runs
# are ignored. The current run never appears as completed, but filter it
# anyway. Failures count so a red compatibility table does not force a
# full re-run every night.
runs_json="$(
  gh run list \
    --workflow "${WORKFLOW_FILE}" \
    --branch "${BRANCH}" \
    --limit 50 \
    --json databaseId,headSha,conclusion,status,event
)"

last_sha="$(
  jq -r --arg id "${CURRENT_RUN}" '
    [
      .[]
      | select((.databaseId | tostring) != $id)
      | select(.status == "completed")
      | select(.conclusion == "success" or .conclusion == "failure")
    ]
    | .[0].headSha // empty
  ' <<<"${runs_json}"
)"

if [[ -z "${last_sha}" ]]; then
  echo "No previous completed run on ${BRANCH}; running the matrix (first night)"
  write "run=true"
  write "reason=no-previous-run"
  write "baseline="
  summarize "No previous completed run of this workflow on \`${BRANCH}\`; running the matrix (first night)."
  exit 0
fi

if ! git cat-file -e "${last_sha}^{commit}" 2>/dev/null; then
  echo "Baseline ${last_sha} is not in this clone; fetching"
  if ! git fetch --depth=1 origin "${last_sha}"; then
    echo "Cannot fetch baseline ${last_sha}; running the matrix"
    write "run=true"
    write "reason=missing-baseline"
    write "baseline=${last_sha}"
    summarize "Could not fetch baseline \`${last_sha}\`; running the matrix."
    exit 0
  fi
fi

changed="$(git diff --name-only "${last_sha}" "${CURRENT_SHA}" -- "${PATHS[@]}" || true)"

if [[ -z "${changed}" ]]; then
  echo "no changes since ${last_sha}, skipped"
  write "run=false"
  write "reason=no-changes"
  write "baseline=${last_sha}"
  summarize "no changes since \`${last_sha}\`, skipped"
  exit 0
fi

echo "Relevant changes since ${last_sha}:"
printf '%s\n' "${changed}"
write "run=true"
write "reason=changes"
write "baseline=${last_sha}"
{
  echo "Relevant changes since \`${last_sha}\`; running the matrix."
  echo
  echo '```'
  printf '%s\n' "${changed}"
  echo '```'
} | {
  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    cat >> "${GITHUB_STEP_SUMMARY}"
  else
    cat
  fi
}
