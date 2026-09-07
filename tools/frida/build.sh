#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
set -euo pipefail
cd "$(dirname "$0")/../.."
output=${1:-dist/frida}
mode=${2:-all}
case "$mode" in all|--agent-only|--runtime-only) ;; *) echo "Usage: $0 [output-directory] [--agent-only|--runtime-only]" >&2; exit 2 ;; esac
mkdir -p "$output"
output=$(cd "$output" && pwd)
if [[ "$mode" == all ]]; then
  cargo +1.88.0 build --locked --release --manifest-path rust/crates/orbit-service/Cargo.toml
  cp rust/crates/orbit-service/target/release/orbit-service "$output/"
fi
if [[ "$mode" != --runtime-only ]]; then
  cargo +1.88.0 build --locked --release --manifest-path rust/Cargo.toml -p orbit-frida-agent
  if [[ $(uname -s) == Darwin ]]; then library=liborbit_frida_agent.dylib; else library=liborbit_frida_agent.so; fi
  cp "rust/target/release/$library" "$output/"
fi
# Frida Core/Gum are supplied by the pinned native Python wheel. Python carries
# control messages only; target callbacks execute native C and Rust.
python3 -m venv "$output/frida-python"
"$output/frida-python/bin/python" -m pip install --only-binary=:all: 'frida==17.17.0'
echo "Frida runtime ready in $output (requires Python on this host)."
