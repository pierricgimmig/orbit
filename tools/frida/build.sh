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
  cargo +1.88.0 build --locked --release -p orbit-service
  cp target/release/orbit-service "$output/"
fi
cargo +1.88.0 build --locked --release -p orbit-frida-helper
cargo +1.88.0 build --locked --release -p orbit-frida-agent --features native
if [[ $(uname -s) == Darwin ]]; then library=liborbit_frida_agent.dylib; else library=liborbit_frida_agent.so; fi
cp "target/release/$library" "$output/"
cp target/release/orbit-frida-helper "$output/"
rm -rf "$output/frida-python" # obsolete generated runtime from the scripted integration
mkdir -p "$output/licenses"
cp tools/frida/licenses/* "$output/licenses/"
echo "Native Frida helper and agent ready in $output."
