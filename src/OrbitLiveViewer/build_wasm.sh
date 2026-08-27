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

# wasm-bindgen-rayon needs a rebuilt std with atomics (not the rustc 1.88
# sysroot). Pin matches the crate's tested nightly. rust-toolchain.toml
# stays 1.88 for native cargo test.
NIGHTLY="${ORBIT_WASM_NIGHTLY:-nightly-2025-11-15}"
rustup toolchain install "$NIGHTLY" --profile minimal --component rust-src
rustup target add wasm32-unknown-unknown --toolchain "$NIGHTLY"

BINDGEN_VER="0.2.100"
if ! command -v wasm-bindgen >/dev/null || ! wasm-bindgen --version | grep -q "$BINDGEN_VER"; then
  echo "Installing wasm-bindgen-cli ${BINDGEN_VER}"
  cargo install wasm-bindgen-cli --version "$BINDGEN_VER" --locked
fi

# orbit-live-viewer is its own Cargo workspace (see its Cargo.toml), so build
# from its directory rather than with -p from the service workspace root.
VIEWER="$ROOT/crates/orbit-live-viewer"

# Atomics + shared memory so SharedArrayBuffer / rayon workers work.
# +mutable-globals is required by older rustc; harmless on 1.87+.
# --import-memory lets wasm-bindgen share one SAB across workers.
export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+atomics,+bulk-memory,+mutable-globals \
  -C link-arg=--shared-memory -C link-arg=--max-memory=1073741824 \
  -C link-arg=--import-memory \
  -C link-arg=--export=__wasm_init_tls -C link-arg=--export=__tls_size \
  -C link-arg=--export=__tls_align -C link-arg=--export=__tls_base"
export CARGO_PROFILE_RELEASE_PANIC=abort

echo "Building wasm pack with ${NIGHTLY} -Z build-std + --features wasm-threads"
rustup run "$NIGHTLY" cargo build \
  --manifest-path "$VIEWER/Cargo.toml" \
  --target wasm32-unknown-unknown \
  --release \
  --features wasm-threads \
  -Z build-std=panic_abort,std

wasm-bindgen \
  --target web \
  --out-dir "$ROOT/viewer-dist" \
  --out-name orbit_live_viewer \
  "$VIEWER/target/wasm32-unknown-unknown/release/orbit_live_viewer.wasm"

echo "Wrote $ROOT/viewer-dist/orbit_live_viewer.js and .wasm"
if [[ -d "$ROOT/viewer-dist/snippets" ]]; then
  echo "Worker snippets: $ROOT/viewer-dist/snippets"
fi
echo "Rebuild OrbitService so orbit-live-server's build script embeds the pack."
