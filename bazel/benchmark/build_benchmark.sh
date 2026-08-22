#!/bin/bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Wrapper so that `bazel run //bazel/benchmark:build_benchmark` works. The
# benchmark measures the checkout it is started from, which is what
# BUILD_WORKSPACE_DIRECTORY points at; it deliberately does not run inside the
# Bazel invocation that started it.
set -euo pipefail
workspace="${BUILD_WORKSPACE_DIRECTORY:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
exec python3 "${workspace}/bazel/benchmark/build_benchmark.py" "$@"
