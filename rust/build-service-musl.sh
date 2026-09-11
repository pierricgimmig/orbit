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
# orbit-service is its own workspace (see its Cargo.toml).
cargo build --release --target "$TARGET" --manifest-path crates/orbit-service/Cargo.toml

BIN="crates/orbit-service/target/$TARGET/release/orbit-service"
echo
echo "Built: $BIN"
file "$BIN"
echo -n "ldd:  "; ldd "$BIN" 2>&1 || true
echo "size: $(du -h "$BIN" | cut -f1)"

# The Frida engine needs a native glibc agent and a native Frida Core control helper.
# The service executable itself remains fully static.
../tools/frida/build.sh "$PWD/$(dirname "$BIN")" --agent-only

# The manual-instrumentation library and its header ship beside the service:
# orbit.h and the Python package load liborbit_api from the directory that
# holds orbit-service. Built for glibc, not musl, because it is loaded into
# ordinary applications, not into the static service.
cargo build --release -p orbit-api
cp target/release/liborbit_api.so crates/orbit-api/include/orbit.h "$(dirname "$BIN")/"
echo "api:  $(dirname "$BIN")/liborbit_api.so + orbit.h"
