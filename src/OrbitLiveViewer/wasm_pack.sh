#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Genrule helper for `bazel build //:wasm`. Resolves the source-tree
# build_wasm.sh (so viewer-dist/ is updated in the checkout), then copies
# the pack into the genrule outs under bazel-bin.

set -euo pipefail

if [[ $# -lt 3 ]]; then
  echo "usage: wasm_pack.sh BUILD_WASM_SH OUT_JS OUT_WASM" >&2
  exit 2
fi

BUILD_WASM_SH="$1"
OUT_JS="$2"
OUT_WASM="$3"

# Bazel's strict action env hides ~/.cargo. Recover a host cargo/rustup.
if [[ -z "${HOME:-}" ]]; then
  HOME="$(getent passwd "$(id -un)" | cut -d: -f6 || true)"
  export HOME
fi
export PATH="${HOME:+$HOME/.cargo/bin:}/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin:${PATH:-}"

SCRIPT="$BUILD_WASM_SH"
if [[ -L "$SCRIPT" ]]; then
  SCRIPT="$(readlink -f "$SCRIPT")"
elif command -v realpath >/dev/null; then
  SCRIPT="$(realpath "$SCRIPT")"
fi
ROOT="$(cd "$(dirname "$SCRIPT")" && pwd)"

if [[ ! -x "$ROOT/build_wasm.sh" && -f "$ROOT/build_wasm.sh" ]]; then
  chmod +x "$ROOT/build_wasm.sh"
fi
"$ROOT/build_wasm.sh"

cp "$ROOT/viewer-dist/orbit_live_viewer.js" "$OUT_JS"
cp "$ROOT/viewer-dist/orbit_live_viewer_bg.wasm" "$OUT_WASM"
