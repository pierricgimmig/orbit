#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Phase 1 helper for the Stockfish Orbit poster project:
# init the official Stockfish submodule, run the upstream-recommended
# profile-build, and smoke with `./stockfish bench`.
#
# Usage (from the Orbit repo root, or any cwd):
#   ./docs/stockfish-orbit-loop/build.sh           # init + build + bench
#   ./docs/stockfish-orbit-loop/build.sh --smoke   # bench only (binary must exist)
#   ./docs/stockfish-orbit-loop/build.sh --build   # init + rebuild, no bench

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="${ROOT}/stockfish/src"
PINNED_SHA="031dfeb437fa6b06cdbdf4ef89dfb82f6b83c4d3"

MODE="all"
case "${1:-}" in
  --smoke) MODE="smoke" ;;
  --build) MODE="build" ;;
  --help|-h)
    sed -n '2,16p' "$0"
    exit 0
    ;;
  "") ;;
  *)
    echo "unknown argument: $1" >&2
    exit 2
    ;;
esac

init_submodule() {
  if [[ ! -f "${SRC}/Makefile" ]]; then
    git -C "${ROOT}" submodule update --init stockfish
  fi
  local actual
  actual="$(git -C "${ROOT}/stockfish" rev-parse HEAD)"
  if [[ "${actual}" != "${PINNED_SHA}" ]]; then
    echo "warning: stockfish HEAD is ${actual}, expected ${PINNED_SHA}" >&2
  fi
}

build_stockfish() {
  make -C "${SRC}" -j profile-build
}

smoke_bench() {
  if [[ ! -x "${SRC}/stockfish" ]]; then
    echo "missing ${SRC}/stockfish; run without --smoke first" >&2
    exit 1
  fi
  "${SRC}/stockfish" compiler
  "${SRC}/stockfish" bench
}

init_submodule
case "${MODE}" in
  all)
    build_stockfish
    smoke_bench
    ;;
  build)
    build_stockfish
    ;;
  smoke)
    smoke_bench
    ;;
esac
