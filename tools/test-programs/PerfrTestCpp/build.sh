#!/usr/bin/env bash
# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Builds PerfrTestCpp with perfr.h alone: nothing is linked. At run time the
# header loads libperfr_api.so by the search order in perfr.h; in the source
# tree point PERFR_API_LIB at target/release/libperfr_api.so (the e2e
# harness does). PERFR_STATIC=1 ./build.sh links libperfr_api.a instead.
set -euo pipefail
cd "$(dirname "$0")"
ROOT=$(git -C . rev-parse --show-toplevel)
INCLUDE="$ROOT/crates/perfr-api/include"
API_LIBRARY="$ROOT/target/release/libperfr_api.so"
if [[ $(uname -s) == Darwin ]]; then API_LIBRARY="$ROOT/target/release/libperfr_api.dylib"; fi
[ -f "$API_LIBRARY" ] || (cd "$ROOT" && cargo build --release -p perfr-api)
if [[ ${PERFR_STATIC:-0} == 1 ]]; then
  NATIVE_LIBS=(-lpthread -ldl -lm)
  if [[ $(uname -s) == Darwin ]]; then NATIVE_LIBS=(-lSystem -liconv); fi
  ${CXX:-c++} -O2 -g -std=c++17 -Wall -Wextra -DPERFR_STATIC -I"$INCLUDE" \
    -o PerfrTestCpp PerfrTestCpp.cpp "$ROOT/target/release/libperfr_api.a" "${NATIVE_LIBS[@]}"
  echo "built $(pwd)/PerfrTestCpp (static, libperfr_api.a linked in)"
else
  # dlopen is in libSystem on macOS. Keep the array nonempty for Bash 3.2
  # with nounset: expanding an empty array there is an unbound-variable error.
  DL=(-ldl); if [[ $(uname -s) == Darwin ]]; then DL=(-lSystem); fi
  ${CXX:-c++} -O2 -g -std=c++17 -Wall -Wextra -I"$INCLUDE" -o PerfrTestCpp PerfrTestCpp.cpp -lpthread "${DL[@]}"
  echo "built $(pwd)/PerfrTestCpp (header-only; loads libperfr_api at run time)"
fi
