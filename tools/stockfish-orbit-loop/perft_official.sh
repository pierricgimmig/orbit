#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Wrapper around official Stockfish tests/perft.sh.
# That script spawn(s) ./stockfish and needs `expect`, so we run it from src/.
#
#   ./tools/stockfish-orbit-loop/perft_official.sh
#
# Depth-7 cases search billions of nodes. Budget a long time; the suite's
# default correctness check is the smoke subset in run_suite.py.

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="${ROOT}/stockfish/src"
SCRIPT="${ROOT}/stockfish/tests/perft.sh"

if [[ ! -x "${SRC}/stockfish" ]]; then
  echo "missing ${SRC}/stockfish — run ./docs/stockfish-orbit-loop/build.sh --build" >&2
  exit 1
fi
if [[ ! -f "${SCRIPT}" ]]; then
  echo "missing ${SCRIPT}" >&2
  exit 1
fi
if ! command -v expect >/dev/null; then
  echo "expect is required for official perft.sh (apt install expect)" >&2
  exit 1
fi

cd "${SRC}"
exec bash "${SCRIPT}"
