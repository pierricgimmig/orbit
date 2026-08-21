#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

if ! command -v rustup >/dev/null; then
  echo "rustup is required to add wasm32-unknown-unknown" >&2
  exit 1
fi

rustup target add wasm32-unknown-unknown

BINDGEN_VER="0.2.100"
if ! command -v wasm-bindgen >/dev/null || ! wasm-bindgen --version | grep -q "$BINDGEN_VER"; then
  echo "Installing wasm-bindgen-cli ${BINDGEN_VER}"
  cargo install wasm-bindgen-cli --version "$BINDGEN_VER" --locked
fi

cargo build -p orbit-live-viewer --target wasm32-unknown-unknown --release --features webgpu

wasm-bindgen \
  --target web \
  --out-dir "$ROOT/viewer-dist" \
  --out-name orbit_live_viewer \
  "$ROOT/target/wasm32-unknown-unknown/release/orbit_live_viewer.wasm"

echo "Wrote $ROOT/viewer-dist/orbit_live_viewer.js and .wasm"
echo "Rebuild orbit-live-ffi / OrbitService so rust-embed picks up the pack."
