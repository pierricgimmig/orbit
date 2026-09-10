#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Builds the pip artifacts for one or more platforms:
#
#   orbit-api        the pure-Python instrumentation package with a prebuilt
#                    liborbit_api bundled in (a platform wheel).
#   orbit-profiler   the orbit-service binary, liborbit_api and orbit.h in a
#                    platform wheel, with an `orbit-service` console command.
#
# Both wheels for a platform are cut from ONE build so the ring protocol
# version matches between the library that writes and the service that reads.
#
#   tools/release/build_wheels.sh linux-x86_64 [linux-aarch64 macos-arm64 ...]
#
# Wheels land in dist/wheels/. Needs, on PATH or in a venv: cargo, rustup,
# cargo-zigbuild, ziglang, python3 with `build` and `wheel`. On a native macOS
# runner the mac targets link without zig; elsewhere zig cross-links them.
#
# The library is built against glibc 2.17 (manylinux2014) so the wheel installs
# on any modern Linux; the service binary is a fully static musl build.
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT=$(pwd)

API_PKG="$ROOT/rust/crates/orbit-api/python"
SVC_PKG="$ROOT/rust/crates/orbit-service/python"
HEADER="$ROOT/rust/crates/orbit-api/include/orbit.h"
OUT="$ROOT/dist/wheels"
GLIBC="2.17" # manylinux2014 floor

# Ship stripped: the workspace release profile keeps debuginfo (useful for dev,
# 8x the size for a wheel). Strip at link time so no per-arch strip tool is
# needed for cross builds.
export CARGO_PROFILE_RELEASE_STRIP=symbols

command -v cargo-zigbuild >/dev/null || { echo "need cargo-zigbuild (pip install cargo-zigbuild ziglang)" >&2; exit 1; }
python3 -c "import build, wheel" 2>/dev/null || { echo "need python 'build' and 'wheel' (pip install build wheel)" >&2; exit 1; }

# key -> "lib_target service_target manylinux_or_macos_plat_tag lib_file"
target_spec() {
  case "$1" in
    linux-x86_64)  echo "x86_64-unknown-linux-gnu  x86_64-unknown-linux-musl  manylinux_2_17_x86_64   liborbit_api.so" ;;
    linux-aarch64) echo "aarch64-unknown-linux-gnu aarch64-unknown-linux-musl manylinux_2_17_aarch64  liborbit_api.so" ;;
    macos-x86_64)  echo "x86_64-apple-darwin       x86_64-apple-darwin        macosx_11_0_x86_64      liborbit_api.dylib" ;;
    macos-arm64)   echo "aarch64-apple-darwin      aarch64-apple-darwin       macosx_11_0_arm64       liborbit_api.dylib" ;;
    *) echo "unknown platform key '$1' (use linux-x86_64 linux-aarch64 macos-x86_64 macos-arm64)" >&2; return 1 ;;
  esac
}

# Build a platform wheel from a pure-Python package: build py3-none-any, then
# stamp the platform tag onto it. The package's .so/binary rides as data.
build_and_tag() {
  local pkg=$1 plat=$2 distname=$3
  rm -rf "$pkg/build" "$pkg"/*.egg-info "$pkg/dist"
  ( cd "$pkg" && python3 -m build --wheel --no-isolation --outdir dist >/dev/null )
  local anywheel
  anywheel=$(echo "$pkg"/dist/*-py3-none-any.whl)
  python3 -m wheel tags --python-tag py3 --abi-tag none --platform-tag "$plat" --remove "$anywheel" >/dev/null
  mkdir -p "$OUT"
  mv "$pkg"/dist/*-"$plat".whl "$OUT/"
  echo "  $distname -> $OUT/$(ls "$OUT" | grep -- "-$plat.whl" | grep "${distname%-*}" | tail -1)"
}

for key in "$@"; do
  read -r lib_target svc_target plat lib_file <<<"$(target_spec "$key")"
  echo "== $key ($plat) =="

  rustup target add "$lib_target" "$svc_target" >/dev/null 2>&1 || true

  # 1. liborbit_api, against the glibc floor on Linux (zig picks the version).
  echo "-- liborbit_api ($lib_target)"
  if [[ "$key" == linux-* ]]; then
    cargo zigbuild --release --manifest-path "$ROOT/rust/Cargo.toml" -p orbit-api --target "$lib_target.$GLIBC" >/dev/null
  else
    cargo zigbuild --release --manifest-path "$ROOT/rust/Cargo.toml" -p orbit-api --target "$lib_target" >/dev/null
  fi
  lib="$ROOT/rust/target/$lib_target/release/$lib_file"

  # 2. orbit-service. On Linux the musl target is static and self-contained;
  # zig links it (and the mac targets) cross without a per-arch toolchain.
  echo "-- orbit-service ($svc_target)"
  cargo zigbuild --release --target "$svc_target" \
      --manifest-path "$ROOT/rust/crates/orbit-service/Cargo.toml" >/dev/null
  svc="$ROOT/rust/crates/orbit-service/target/$svc_target/release/orbit-service"

  # 3. Stage the artifacts into each package and build the wheels.
  cp "$lib" "$API_PKG/orbit_api/$lib_file"
  build_and_tag "$API_PKG" "$plat" "orbit_api"

  cp "$svc" "$SVC_PKG/orbit_profiler/orbit-service"
  cp "$lib" "$SVC_PKG/orbit_profiler/$lib_file"
  cp "$HEADER" "$SVC_PKG/orbit_profiler/orbit.h"
  build_and_tag "$SVC_PKG" "$plat" "orbit_profiler"

  # Leave the trees clean: the bundled binaries are build products, not source.
  rm -f "$API_PKG/orbit_api/$lib_file" \
        "$SVC_PKG/orbit_profiler/orbit-service" \
        "$SVC_PKG/orbit_profiler/$lib_file" "$SVC_PKG/orbit_profiler/orbit.h"
done

echo
echo "wheels in $OUT:"
ls -1 "$OUT"
