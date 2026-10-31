#!/usr/bin/env bash
# Copyright (c) 2026 The perfr Authors. All rights reserved.
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
  cargo +1.88.0 build --locked --release -p perfr
  cp target/release/perfr "$output/"
fi
cargo +1.88.0 build --locked --release -p perfr-frida-helper
cargo +1.88.0 build --locked --release -p perfr-frida-agent --features native
if [[ $(uname -s) == Darwin ]]; then library=libperfr_frida_agent.dylib; else library=libperfr_frida_agent.so; fi
# The development build points output at target/release itself; cp refuses to
# copy a file onto itself, so skip the copies that would be no-ops.
for built in "target/release/$library" target/release/perfr-frida-helper; do
  [[ "$built" -ef "$output/$(basename "$built")" ]] || cp "$built" "$output/"
done
rm -rf "$output/frida-python" # obsolete generated runtime from the scripted integration
mkdir -p "$output/licenses"
cp tools/frida/licenses/* "$output/licenses/"
echo "Native Frida helper and agent ready in $output."
