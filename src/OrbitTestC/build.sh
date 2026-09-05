#!/usr/bin/env bash
# Builds OrbitTestC against the Rust orbit-api static library.
set -euo pipefail
cd "$(dirname "$0")"
ROOT=$(git -C . rev-parse --show-toplevel)
LIB="$ROOT/rust/target/release/liborbit_api.a"
[ -f "$LIB" ] || (cd "$ROOT/rust" && cargo build --release -p orbit-api)
gcc -O2 -g -std=c11 -Wall -Wextra -I"$ROOT/rust/crates/orbit-api/include" \
  -o OrbitTestC OrbitTestC.c "$LIB" -lpthread -ldl -lm
echo "built $(pwd)/OrbitTestC"
