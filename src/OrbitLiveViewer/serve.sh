#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# `bazel run //:live` / `bazel run //src/OrbitLiveViewer:serve`

set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: serve.sh ORBIT_LIVE_SERVICE [args...]" >&2
  exit 2
fi

BIN="$1"
shift

has_port=0
for arg in "$@"; do
  if [[ "$arg" == "--http-port" || "$arg" == "--http_port" ]]; then
    has_port=1
    break
  fi
done

echo "http://127.0.0.1:44766/"
if [[ "$has_port" -eq 0 ]]; then
  exec "$BIN" --http-port 44766 "$@"
fi
exec "$BIN" "$@"
