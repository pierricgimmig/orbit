#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license in LICENSE.
set -euo pipefail
cd "$(dirname "$0")/../.."
output=${1:-dist/frida}
mode=${2:-all}
case "$mode" in all|--agent-only) ;; *) echo "Usage: $0 [output-directory] [--agent-only]" >&2; exit 2 ;; esac
mkdir -p "$output"
output=$(cd "$output" && pwd)
./tools/frida/devkits.sh
if [[ "$mode" == all ]]; then
  cargo +1.88.0 build --locked --release --manifest-path rust/crates/orbit-service/Cargo.toml
  cp rust/crates/orbit-service/target/release/orbit-service "$output/"
fi
cargo +1.88.0 build --locked --release --manifest-path rust/crates/orbit-frida-helper/Cargo.toml
cargo +1.88.0 build --locked --release --manifest-path rust/Cargo.toml -p orbit-frida-agent --features native
if [[ $(uname -s) == Darwin ]]; then library=liborbit_frida_agent.dylib; else library=liborbit_frida_agent.so; fi
cp "rust/target/release/$library" "$output/"
cp rust/crates/orbit-frida-helper/target/release/orbit-frida-helper "$output/"
mkdir -p "$output/licenses"
cp tools/frida/licenses/* "$output/licenses/"
echo "Native Frida helper and agent ready in $output."
