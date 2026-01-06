# Copyright (c) 2024 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

cmake_minimum_required(VERSION 3.15)

project(py-spy-ffi LANGUAGES C CXX)

# py-spy-ffi is only available on Linux (requires ptrace)
if(NOT CMAKE_SYSTEM_NAME STREQUAL "Linux")
  message(STATUS "py-spy-ffi: Skipping (not Linux)")
  return()
endif()

# Find Rust toolchain
find_program(CARGO_EXECUTABLE cargo HINTS $ENV{HOME}/.cargo/bin)
if(NOT CARGO_EXECUTABLE)
  message(WARNING "py-spy-ffi: cargo not found, Python profiling will be disabled")
  return()
endif()

# Determine build type for Cargo
if(CMAKE_BUILD_TYPE STREQUAL "Debug")
  set(CARGO_BUILD_TYPE "debug")
  set(CARGO_BUILD_FLAG "")
else()
  set(CARGO_BUILD_TYPE "release")
  set(CARGO_BUILD_FLAG "--release")
endif()

# Determine target triple for cross-compilation
if(CMAKE_CROSSCOMPILING)
  if(CMAKE_SYSTEM_PROCESSOR MATCHES "aarch64")
    set(CARGO_TARGET "aarch64-unknown-linux-gnu")
    set(CARGO_TARGET_FLAG "--target=${CARGO_TARGET}")
    set(CARGO_TARGET_DIR "${CMAKE_CURRENT_SOURCE_DIR}/target/${CARGO_TARGET}/${CARGO_BUILD_TYPE}")
  else()
    message(WARNING "py-spy-ffi: Unsupported cross-compilation target: ${CMAKE_SYSTEM_PROCESSOR}")
    return()
  endif()
else()
  set(CARGO_TARGET_FLAG "")
  set(CARGO_TARGET_DIR "${CMAKE_CURRENT_SOURCE_DIR}/target/${CARGO_BUILD_TYPE}")
endif()

# Output library name
set(PYSPY_FFI_LIB "${CARGO_TARGET_DIR}/libpyspy_ffi.a")

# Custom command to build the Rust library
add_custom_command(
  OUTPUT ${PYSPY_FFI_LIB}
  COMMAND ${CMAKE_COMMAND} -E env
    "CC=${CMAKE_C_COMPILER}"
    "CXX=${CMAKE_CXX_COMPILER}"
    "AR=${CMAKE_AR}"
    ${CARGO_EXECUTABLE} build ${CARGO_BUILD_FLAG} ${CARGO_TARGET_FLAG}
  WORKING_DIRECTORY ${CMAKE_CURRENT_SOURCE_DIR}
  COMMENT "Building py-spy-ffi with Cargo"
  DEPENDS
    ${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml
    ${CMAKE_CURRENT_SOURCE_DIR}/src/lib.rs
  VERBATIM
)

# Custom target that depends on the library
add_custom_target(pyspy_ffi_build DEPENDS ${PYSPY_FFI_LIB})

# Create imported library target
add_library(pyspy_ffi STATIC IMPORTED GLOBAL)
set_target_properties(pyspy_ffi PROPERTIES
  IMPORTED_LOCATION ${PYSPY_FFI_LIB}
  INTERFACE_INCLUDE_DIRECTORIES ${CMAKE_CURRENT_SOURCE_DIR}/include
)
add_dependencies(pyspy_ffi pyspy_ffi_build)

# Link against pthread and dl (required by Rust runtime)
target_link_libraries(pyspy_ffi INTERFACE pthread dl)
