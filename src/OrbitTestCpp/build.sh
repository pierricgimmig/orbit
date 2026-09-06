#!/usr/bin/env bash
# Builds OrbitTestCpp against the Rust orbit-api static library.
set -euo pipefail
cd "$(dirname "$0")"
ROOT=$(git -C . rev-parse --show-toplevel)
LIB="$ROOT/rust/target/release/liborbit_api.a"
NATIVE_LIBS=(-lpthread -ldl -lm)
if [[ $(uname -s) == Darwin ]]; then NATIVE_LIBS=(-lSystem -liconv); fi
[ -f "$LIB" ] || (cd "$ROOT/rust" && cargo build --release -p orbit-api)
${CXX:-c++} -O2 -g -std=c++17 -Wall -Wextra -I"$ROOT/rust/crates/orbit-api/include" \
  -o OrbitTestCpp OrbitTestCpp.cpp "$LIB" "${NATIVE_LIBS[@]}"
echo "built $(pwd)/OrbitTestCpp"
