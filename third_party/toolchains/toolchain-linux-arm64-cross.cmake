# CMake toolchain file for ARM64 (AArch64) cross-compilation
# Target: Raspberry Pi 4/5 (Cortex-A72/A76)
#
# Usage:
#   cmake -DCMAKE_TOOLCHAIN_FILE=third_party/toolchains/toolchain-linux-arm64-cross.cmake ..

# Target system
set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSTEM_PROCESSOR aarch64)

# Cross compiler
set(CMAKE_C_COMPILER aarch64-linux-gnu-gcc)
set(CMAKE_CXX_COMPILER aarch64-linux-gnu-g++)

# Library search paths for cross-compiled ARM64 libraries
# Note: Do NOT use CMAKE_SYSROOT - the multiarch library paths have absolute
# references in linker scripts that conflict with sysroot path prefixing.
set(CMAKE_FIND_ROOT_PATH /usr/aarch64-linux-gnu /usr/lib/aarch64-linux-gnu)

# Search behavior: find programs on host, libraries/includes on target
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)

# ARM64 optimization flags for Cortex-A72 (RPi 4) / Cortex-A76 (RPi 5)
# Using cortex-a72 as the baseline since it's compatible with both
set(CMAKE_C_FLAGS_INIT "-mcpu=cortex-a72")
set(CMAKE_CXX_FLAGS_INIT "-mcpu=cortex-a72")

# Build configuration
set(CMAKE_BUILD_TYPE Release CACHE STRING "build type" FORCE)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON CACHE BOOL "generate compile_commands.json" FORCE)

# Mark as cross-compiling (this disables tests in cmake/tests.cmake)
set(CMAKE_CROSSCOMPILING TRUE)
