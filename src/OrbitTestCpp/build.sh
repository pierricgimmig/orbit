#!/usr/bin/env bash
# Builds OrbitTestCpp against the Rust orbit-api static library.
set -euo pipefail
cd "$(dirname "$0")"
ROOT=$(git -C . rev-parse --show-toplevel)
LIB="$ROOT/rust/target/release/liborbit_api.a"
[ -f "$LIB" ] || (cd "$ROOT/rust" && cargo build --release -p orbit-api)
g++ -O2 -g -std=c++17 -Wall -Wextra -I"$ROOT/rust/crates/orbit-api/include" \
  -o OrbitTestCpp OrbitTestCpp.cpp "$LIB" -lpthread -ldl -lm
echo "built $(pwd)/OrbitTestCpp"
