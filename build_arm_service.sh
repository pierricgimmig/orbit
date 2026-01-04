#!/bin/bash
# Build OrbitService for ARM64 (Raspberry Pi 4/5)
# This script cross-compiles OrbitService using Docker or Podman

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

IMAGE_NAME="orbit-arm64-cross"
BUILD_DIR="build_arm64"

# Use docker if available, otherwise podman
if command -v docker &> /dev/null; then
  CONTAINER_CMD="docker"
elif command -v podman &> /dev/null; then
  CONTAINER_CMD="podman"
else
  echo "Error: Neither docker nor podman found. Please install one of them."
  exit 1
fi

echo "Using container runtime: $CONTAINER_CMD"

echo "=== Building ARM64 Cross-Compilation Docker Image ==="
$CONTAINER_CMD build -f docker/Dockerfile.arm64-cross -t "$IMAGE_NAME" .

echo "=== Cross-Compiling OrbitService for ARM64 ==="
$CONTAINER_CMD run --rm -v "$(pwd)":/orbit -w /orbit -e CMAKE_PREFIX_PATH="/orbit/build_arm64/generators;/opt/llvm-arm64" "$IMAGE_NAME" bash -c '
  mkdir -p build_arm64
  cd build_arm64
  conan install .. \
    --profile:build=default \
    --profile:host=/orbit/conan/profiles/arm64-linux-cross \
    --output-folder=. \
    --build=missing

  echo "CMAKE_PREFIX_PATH=$CMAKE_PREFIX_PATH"

  # Clear CMake cache to ensure fresh configuration
  rm -rf CMakeCache.txt CMakeFiles

  cmake .. \
    -DCMAKE_TOOLCHAIN_FILE=generators/conan_toolchain.cmake \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_CROSSCOMPILING=TRUE \
    -DCMAKE_FIND_ROOT_PATH_MODE_PACKAGE=BOTH \
    -DLLVM_DIR:PATH=/opt/llvm-arm64/lib/cmake/llvm \
    -DWITH_GUI=OFF \
    -DWITH_VULKAN=OFF \
    -G Ninja
  ninja OrbitService
'

echo "=== Build Complete ==="
echo "Binary location: $BUILD_DIR/bin/OrbitService"
echo ""
echo "To deploy to Raspberry Pi:"
echo "  scp $BUILD_DIR/bin/OrbitService pi@raspberrypi:/opt/orbitprofiler/"
