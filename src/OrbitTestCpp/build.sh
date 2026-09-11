#!/usr/bin/env bash
# Builds OrbitTestCpp with orbit.h alone: nothing is linked. At run time the
# header loads liborbit_api.so by the search order in orbit.h; in the source
# tree point ORBIT_API_LIB at rust/target/release/liborbit_api.so (the e2e
# harness does). ORBIT_STATIC=1 ./build.sh links liborbit_api.a instead.
set -euo pipefail
cd "$(dirname "$0")"
ROOT=$(git -C . rev-parse --show-toplevel)
INCLUDE="$ROOT/rust/crates/orbit-api/include"
[ -f "$ROOT/rust/target/release/liborbit_api.so" ] || (cd "$ROOT/rust" && cargo build --release -p orbit-api)
if [[ ${ORBIT_STATIC:-0} == 1 ]]; then
  NATIVE_LIBS=(-lpthread -ldl -lm)
  if [[ $(uname -s) == Darwin ]]; then NATIVE_LIBS=(-lSystem -liconv); fi
  ${CXX:-c++} -O2 -g -std=c++17 -Wall -Wextra -DORBIT_STATIC -I"$INCLUDE" \
    -o OrbitTestCpp OrbitTestCpp.cpp "$ROOT/rust/target/release/liborbit_api.a" "${NATIVE_LIBS[@]}"
  echo "built $(pwd)/OrbitTestCpp (static, liborbit_api.a linked in)"
else
  DL=(-ldl); if [[ $(uname -s) == Darwin ]]; then DL=(); fi
  ${CXX:-c++} -O2 -g -std=c++17 -Wall -Wextra -I"$INCLUDE" -o OrbitTestCpp OrbitTestCpp.cpp -lpthread "${DL[@]}"
  echo "built $(pwd)/OrbitTestCpp (header-only; loads liborbit_api at run time)"
fi
