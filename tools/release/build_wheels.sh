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
# cargo-zigbuild, ziglang, python3 with `build` and `wheel`. zig cross-links
# every target from a Linux host; on a macOS host the mac targets use Apple's
# own toolchain, which cross-links arm64 and x86_64 natively.
#
# The library is built against glibc 2.17 (manylinux2014) so the wheel installs
# on any modern Linux; the service binary is a fully static musl build.
#
# The orbit-profiler wheel always includes the Frida companions. The service
# loads liborbit_frida_agent and spawns orbit-frida-helper from the directory
# that contains the orbit-service executable. This script builds those for the
# wheel's target (glibc or macOS, not the musl service triple). Set
# ORBIT_COMPANIONS_DIR to skip that build and copy a directory already laid
# out the way tools/frida/build.sh <dir> writes it.
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

# One release version, stamped in five places. Refuse to build if they drift:
# orbit-profiler pins orbit-api to the exact version because the two must
# speak the same ring protocol, and a stale pin would install a mismatch.
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/rust/crates/orbit-api/Cargo.toml" | head -1)
drift() { echo "version drift: $1 (orbit-api Cargo.toml says $VERSION)" >&2; exit 1; }
grep -q "^version = \"$VERSION\"" "$API_PKG/pyproject.toml" || drift "$API_PKG/pyproject.toml"
grep -q "^version = \"$VERSION\"" "$SVC_PKG/pyproject.toml" || drift "$SVC_PKG/pyproject.toml"
grep -q "\"orbit-api==$VERSION\"" "$SVC_PKG/pyproject.toml" || drift "orbit-profiler must pin orbit-api==$VERSION"
grep -q "__version__ = \"$VERSION\"" "$API_PKG/orbit_api/__init__.py" || drift "$API_PKG/orbit_api/__init__.py"
grep -q "__version__ = \"$VERSION\"" "$SVC_PKG/orbit_profiler/__init__.py" || drift "$SVC_PKG/orbit_profiler/__init__.py"
echo "release version $VERSION"

# key -> "lib_target service_target platform_tag lib_file"
target_spec() {
  case "$1" in
    linux-x86_64)  echo "x86_64-unknown-linux-gnu  x86_64-unknown-linux-musl  manylinux_2_17_x86_64   liborbit_api.so" ;;
    linux-aarch64) echo "aarch64-unknown-linux-gnu aarch64-unknown-linux-musl manylinux_2_17_aarch64  liborbit_api.so" ;;
    macos-x86_64)  echo "x86_64-apple-darwin       x86_64-apple-darwin        macosx_11_0_x86_64      liborbit_api.dylib" ;;
    macos-arm64)   echo "aarch64-apple-darwin      aarch64-apple-darwin       macosx_11_0_arm64       liborbit_api.dylib" ;;
    windows-x86_64) echo "x86_64-pc-windows-msvc   x86_64-pc-windows-msvc     win_amd64               orbit_api.dll" ;;
    *) echo "unknown platform key '$1' (use linux-x86_64 linux-aarch64 macos-x86_64 macos-arm64 windows-x86_64)" >&2; return 1 ;;
  esac
}

# On a macOS host, Apple's toolchain links both mac architectures; zig is for
# cross-linking from Linux (and for musl, where it needs no musl toolchain).
native_mac() { [[ $(uname -s) == Darwin && "$1" == macos-* ]]; }
# Windows wheels are built on Windows (Git Bash) with MSVC: no zig, no cross.
native_windows() { [[ "$1" == windows-* ]]; }
# The service binary's file name on the platform being built.
service_file() { if [[ "$1" == windows-* ]]; then echo orbit-service.exe; else echo orbit-service; fi; }

# Frida's devkits and the helper are per-target glibc/macOS binaries, not musl.
companion_triple() {
  case "$1" in
    linux-x86_64)  echo "x86_64-unknown-linux-gnu" ;;
    linux-aarch64) echo "aarch64-unknown-linux-gnu" ;;
    macos-x86_64)  echo "x86_64-apple-darwin" ;;
    macos-arm64)   echo "aarch64-apple-darwin" ;;
    windows-x86_64) echo "x86_64-pc-windows-msvc" ;;
    *) echo "unknown platform key '$1'" >&2; return 1 ;;
  esac
}

# build_frida_companions <key> <dest>
# Writes liborbit_frida_agent.*, orbit-frida-helper, and licenses/ into dest.
build_frida_companions() {
  local key=$1 dest=$2 triple lib host
  triple=$(companion_triple "$key")
  "$ROOT/tools/frida/devkits.sh" "$triple"
  rustup target add "$triple" >/dev/null 2>&1 || true
  host=$(rustc -vV | sed -n 's/^host: //p')
  echo "-- Frida companions ($triple)"
  local -a cargo
  if [[ "$triple" != "$host" ]] && ! native_mac "$key" && ! native_windows "$key"; then
    cargo=(cargo zigbuild --release --target "$triple")
  else
    cargo=(cargo build --release --target "$triple")
  fi
  "${cargo[@]}" --manifest-path "$ROOT/rust/crates/orbit-frida-helper/Cargo.toml" >/dev/null
  "${cargo[@]}" --manifest-path "$ROOT/rust/Cargo.toml" -p orbit-frida-agent --features native >/dev/null
  local helper=orbit-frida-helper
  case "$key" in
    macos-*) lib=liborbit_frida_agent.dylib ;;
    windows-*) lib=orbit_frida_agent.dll; helper=orbit-frida-helper.exe ;;
    *) lib=liborbit_frida_agent.so ;;
  esac
  mkdir -p "$dest/licenses"
  cp "$ROOT/rust/target/$triple/release/$lib" "$dest/"
  cp "$ROOT/rust/crates/orbit-frida-helper/target/$triple/release/$helper" "$dest/"
  chmod 755 "$dest/$helper"
  cp "$ROOT/tools/frida/licenses/"* "$dest/licenses/"
}

# cargo_for <key> <manifest> <target> [extra cargo args]
cargo_for() {
  local key=$1 manifest=$2 target=$3; shift 3
  if native_mac "$key" || native_windows "$key"; then
    cargo build --release --manifest-path "$manifest" --target "$target" "$@" >/dev/null
  else
    cargo zigbuild --release --manifest-path "$manifest" --target "$target" "$@" >/dev/null
  fi
}

# Build a platform wheel from a pure-Python package: build py3-none-any, then
# stamp the platform tag onto it. The package's .so/binary rides as data, and
# py3-none-<platform> means one wheel serves every Python 3.
build_and_tag() {
  local pkg=$1 plat=$2
  rm -rf "$pkg/build" "$pkg"/*.egg-info "$pkg/dist"
  ( cd "$pkg" && python3 -m build --wheel --no-isolation --outdir dist >/dev/null 2>&1 )
  local anywheel tagged
  anywheel=$(echo "$pkg"/dist/*-py3-none-any.whl)
  python3 -m wheel tags --python-tag py3 --abi-tag none --platform-tag "$plat" --remove "$anywheel" >/dev/null
  tagged=$(echo "$pkg"/dist/*-"$plat".whl)
  mkdir -p "$OUT"
  mv "$tagged" "$OUT/"
  rm -rf "$pkg/build" "$pkg"/*.egg-info "$pkg/dist"
  echo "  $(basename "$tagged")"
}

for key in "$@"; do
  read -r lib_target svc_target plat lib_file <<<"$(target_spec "$key")"
  echo "== $key ($plat) =="

  rustup target add "$lib_target" "$svc_target" >/dev/null 2>&1 || true

  # 1. liborbit_api, against the glibc floor on Linux (zig picks the version).
  echo "-- liborbit_api ($lib_target)"
  if [[ "$key" == linux-* ]]; then
    cargo_for "$key" "$ROOT/rust/Cargo.toml" "$lib_target.$GLIBC" -p orbit-api
  else
    cargo_for "$key" "$ROOT/rust/Cargo.toml" "$lib_target" -p orbit-api
  fi
  lib="$ROOT/rust/target/$lib_target/release/$lib_file"

  # 2. orbit-service. The musl target is static and self-contained.
  echo "-- orbit-service ($svc_target)"
  cargo_for "$key" "$ROOT/rust/crates/orbit-service/Cargo.toml" "$svc_target"
  svc="$ROOT/rust/crates/orbit-service/target/$svc_target/release/$(service_file "$key")"

  # 3. Stage the artifacts into each package and build the wheels.
  cp "$lib" "$API_PKG/orbit_api/$lib_file"
  build_and_tag "$API_PKG" "$plat"

  cp "$svc" "$SVC_PKG/orbit_profiler/$(service_file "$key")"
  cp "$lib" "$SVC_PKG/orbit_profiler/$lib_file"
  cp "$HEADER" "$SVC_PKG/orbit_profiler/orbit.h"
  staged_companions=()
  companions_src=${ORBIT_COMPANIONS_DIR:-}
  companions_tmp=
  if [[ -z "$companions_src" ]]; then
    companions_tmp=$(mktemp -d)
    build_frida_companions "$key" "$companions_tmp"
    companions_src=$companions_tmp
  fi
  shopt -s nullglob
  for f in "$companions_src"/liborbit_frida_agent.* "$companions_src"/orbit_frida_agent.dll "$companions_src"/orbit-frida-helper "$companions_src"/orbit-frida-helper.exe; do
    cp "$f" "$SVC_PKG/orbit_profiler/"
    staged_companions+=("$SVC_PKG/orbit_profiler/$(basename "$f")")
  done
  shopt -u nullglob
  if [[ -d "$companions_src/licenses" ]]; then
    cp -r "$companions_src/licenses" "$SVC_PKG/orbit_profiler/licenses"; staged_companions+=("$SVC_PKG/orbit_profiler/licenses")
  fi
  [[ -z "${companions_tmp}" ]] || rm -rf "$companions_tmp"
  if [[ ! -f "$SVC_PKG/orbit_profiler/orbit-frida-helper" && ! -f "$SVC_PKG/orbit_profiler/orbit-frida-helper.exe" ]] || \
     [[ ! -f "$SVC_PKG/orbit_profiler/liborbit_frida_agent.so" && ! -f "$SVC_PKG/orbit_profiler/liborbit_frida_agent.dylib" && ! -f "$SVC_PKG/orbit_profiler/orbit_frida_agent.dll" ]]; then
    echo "Frida companions were not staged for $key" >&2
    exit 1
  fi
  echo "-- Frida companions: ${#staged_companions[@]} item(s)"
  build_and_tag "$SVC_PKG" "$plat"

  # Leave the trees clean: the bundled binaries are build products, not source.
  rm -rf "$API_PKG/orbit_api/$lib_file" \
         "$SVC_PKG/orbit_profiler/$(service_file "$key")" \
         "$SVC_PKG/orbit_profiler/$lib_file" "$SVC_PKG/orbit_profiler/orbit.h" \
         "${staged_companions[@]}"
done

echo
echo "wheels in $OUT:"
ls -1 "$OUT"
