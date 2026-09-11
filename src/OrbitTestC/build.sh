#!/usr/bin/env bash
# Builds OrbitTestC with orbit.h alone: nothing is linked. At run time the
# header loads liborbit_api.so by the search order in orbit.h; in the source
# tree point ORBIT_API_LIB at rust/target/release/liborbit_api.so (the e2e
# harness does). ORBIT_STATIC=1 ./build.sh links liborbit_api.a instead, the
# shape a musl or otherwise dlopen-less program uses.
set -euo pipefail
cd "$(dirname "$0")"
ROOT=$(git -C . rev-parse --show-toplevel)
INCLUDE="$ROOT/rust/crates/orbit-api/include"
[ -f "$ROOT/rust/target/release/liborbit_api.so" ] || (cd "$ROOT/rust" && cargo build --release -p orbit-api)
if [[ ${ORBIT_STATIC:-0} == 1 ]]; then
  NATIVE_LIBS=(-lpthread -ldl -lm)
  if [[ $(uname -s) == Darwin ]]; then NATIVE_LIBS=(-lSystem -liconv); fi
  ${CC:-cc} -O2 -g -std=c11 -Wall -Wextra -DORBIT_STATIC -I"$INCLUDE" \
    -o OrbitTestC OrbitTestC.c "$ROOT/rust/target/release/liborbit_api.a" "${NATIVE_LIBS[@]}"
  echo "built $(pwd)/OrbitTestC (static, liborbit_api.a linked in)"
else
  DL=(-ldl); if [[ $(uname -s) == Darwin ]]; then DL=(); fi
  ${CC:-cc} -O2 -g -std=c11 -Wall -Wextra -I"$INCLUDE" -o OrbitTestC OrbitTestC.c -lpthread "${DL[@]}"
  echo "built $(pwd)/OrbitTestC (header-only; loads liborbit_api at run time)"
fi
