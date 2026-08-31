#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Builds the Orbit capture service as a fully static, self-contained musl
# executable -- no shared libraries, runs on any x86-64 Linux with no
# runtime dependencies. The crates are pure Rust plus libc, so rustc's
# self-contained musl target links statically with no system musl toolchain.
set -euo pipefail
cd "$(dirname "$0")"

# cargo is normally on PATH; fall back to the rustup default location.
command -v cargo >/dev/null 2>&1 || export PATH="$HOME/.cargo/bin:$PATH"

TARGET=x86_64-unknown-linux-musl
# rust-toolchain.toml pins the target, so rustup fetches std automatically.
cargo build --release --target "$TARGET" -p orbit-service

BIN="target/$TARGET/release/orbit-service"
echo
echo "Built: $BIN"
file "$BIN"
echo -n "ldd:  "; ldd "$BIN" 2>&1 || true
echo "size: $(du -h "$BIN" | cut -f1)"
