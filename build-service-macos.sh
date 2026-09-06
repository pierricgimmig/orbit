#!/usr/bin/env bash
# Build the service, embedded viewer, and manual instrumentation SDK on macOS.
set -euo pipefail
cd "$(dirname "$0")"
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
[[ -f src/OrbitLiveViewer/viewer-dist/index.html ]]
mkdir -p "$output/include"
service_bins=(); static_libs=(); dynamic_libs=()
for target in "${targets[@]}"; do
  rustup target add --toolchain 1.88.0 "$target"
  cargo +1.88.0 build --locked --release --target "$target" \
    --manifest-path rust/crates/orbit-service/Cargo.toml
  cargo +1.88.0 build --locked --release --target "$target" \
    --manifest-path rust/Cargo.toml -p orbit-api -p orbit-test-rust
  service_bins+=("rust/crates/orbit-service/target/$target/release/orbit-service")
  static_libs+=("rust/target/$target/release/liborbit_api.a")
  dynamic_libs+=("rust/target/$target/release/liborbit_api.dylib")
done
if [[ ${#targets[@]} == 1 ]]; then
  cp "${service_bins[0]}" "$output/orbit-service"
  cp "${static_libs[0]}" "$output/liborbit_api.a"
  cp "${dynamic_libs[0]}" "$output/liborbit_api.dylib"
else
  lipo -create "${service_bins[@]}" -output "$output/orbit-service"
  lipo -create "${static_libs[@]}" -output "$output/liborbit_api.a"
  lipo -create "${dynamic_libs[@]}" -output "$output/liborbit_api.dylib"
fi
# A relocatable SDK; consumers using the dylib provide their own rpath.
install_name_tool -id @rpath/liborbit_api.dylib "$output/liborbit_api.dylib"
codesign --force --sign - "$output/orbit-service"
codesign --force --sign - "$output/liborbit_api.dylib"
cp rust/crates/orbit-api/include/orbit.h "$output/include/"
cp src/OrbitTestPython/orbit.py "$output/"
cp docs/building_macos.md "$output/README.md"
echo "Built $output; run $output/orbit-service --host 127.0.0.1 --serve 3000"
