#!/usr/bin/env bash
# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Build the service, embedded viewer, and manual instrumentation SDK on macOS.
set -euo pipefail
cd "$(dirname "$0")/../.."
if [[ $(uname -s) != Darwin ]]; then
  echo "Run this script on macOS with Xcode Command Line Tools installed." >&2
  exit 1
fi
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-11.0}
targets=()
case ${1:-} in
  '') targets=("$(rustc +1.88.0 -vV | sed -n 's/^host: //p')"); output=dist/macos ;;
  --universal) targets=(aarch64-apple-darwin x86_64-apple-darwin); output=dist/macos-universal ;;
  *) echo "Usage: $0 [--universal]" >&2; exit 2 ;;
esac
[[ -f crates/perfr-live-viewer/viewer-dist/index.html ]]
mkdir -p "$output/include"
service_bins=(); static_libs=(); dynamic_libs=(); frida_libs=(); frida_helpers=()
for target in "${targets[@]}"; do
  rustup target add --toolchain 1.88.0 "$target"
  ./tools/frida/devkits.sh "$target"
  cargo +1.88.0 build --locked --release --target "$target" \
    -p perfr
  cargo +1.88.0 build --locked --release --target "$target" \
    -p perfr-api -p perfr-test-rust -p perfr-frida-agent --features perfr-frida-agent/native
  cargo +1.88.0 build --locked --release --target "$target" -p perfr-frida-helper
  frida_helpers+=("target/$target/release/perfr-frida-helper")
  service_bins+=("target/$target/release/perfr")
  static_libs+=("target/$target/release/libperfr_api.a")
  dynamic_libs+=("target/$target/release/libperfr_api.dylib")
  frida_libs+=("target/$target/release/libperfr_frida_agent.dylib")
done
if [[ ${#targets[@]} == 1 ]]; then
  cp "${service_bins[0]}" "$output/perfr"
  cp "${static_libs[0]}" "$output/libperfr_api.a"
  cp "${dynamic_libs[0]}" "$output/libperfr_api.dylib"
  cp "${frida_libs[0]}" "$output/libperfr_frida_agent.dylib"
  cp "${frida_helpers[0]}" "$output/perfr-frida-helper"
else
  lipo -create "${service_bins[@]}" -output "$output/perfr"
  lipo -create "${static_libs[@]}" -output "$output/libperfr_api.a"
  lipo -create "${dynamic_libs[@]}" -output "$output/libperfr_api.dylib"
  lipo -create "${frida_libs[@]}" -output "$output/libperfr_frida_agent.dylib"
  lipo -create "${frida_helpers[@]}" -output "$output/perfr-frida-helper"
fi
# A relocatable SDK; consumers using the dylib provide their own rpath.
# perfr.h loads libperfr_api.dylib from the directory that holds perfr.
cp crates/perfr-api/include/perfr.h "$output/perfr.h"
install_name_tool -id @rpath/libperfr_api.dylib "$output/libperfr_api.dylib"
codesign --force --sign - "$output/perfr"
codesign --force --sign - "$output/libperfr_api.dylib"
codesign --force --sign - "$output/libperfr_frida_agent.dylib"
codesign --force --sign - "$output/perfr-frida-helper"
rm -rf "$output/frida-python" # obsolete generated runtime
mkdir -p "$output/licenses"
cp tools/frida/licenses/* "$output/licenses/"
cp crates/perfr-api/include/perfr.h "$output/include/"
cp crates/perfr-api/python/perfr_api/__init__.py "$output/perfr_api.py"
cp docs/building_macos.md "$output/README.md"
echo "Built $output; run $output/perfr --host 127.0.0.1 --serve 3000"
