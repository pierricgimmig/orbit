# Copyright (c) 2020 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

cmake_minimum_required(VERSION 3.15)

set(REQUIRED_LLVM_LIBS LLVMDebugInfoCodeView
                        LLVMDebugInfoDWARF
                        LLVMDebugInfoPDB
                        LLVMObject
                        LLVMSymbolize)

# For ARM64 cross-compilation, use system LLVM instead of Conan's llvm-core
# (Conan's llvm-core doesn't support cross-compilation)
if(CMAKE_SYSTEM_PROCESSOR MATCHES "aarch64")
  find_package(LLVM 14 REQUIRED CONFIG
    HINTS /opt/llvm-arm64/lib/cmake/llvm
          /usr/lib/llvm-14/cmake
          /usr/lib/aarch64-linux-gnu/cmake/llvm-14)
  message(STATUS "Found LLVM ${LLVM_VERSION} for ARM64 at ${LLVM_DIR}")
  set(ORBIT_USE_SYSTEM_LLVM TRUE)
endif()

#find_llvm_libs("${REQUIRED_LLVM_LIBS}")

project(ObjectUtils)
add_library(ObjectUtils STATIC)

target_sources(
  ObjectUtils
  PUBLIC include/ObjectUtils/CoffFile.h
         include/ObjectUtils/ElfFile.h
         include/ObjectUtils/ObjectFile.h
         include/ObjectUtils/PdbFile.h
         include/ObjectUtils/SymbolsFile.h
         include/ObjectUtils/WindowsBuildIdUtils.h)

target_sources(
  ObjectUtils
  PRIVATE
        CoffFile.cpp
        ElfFile.cpp
        PdbFile.cpp
        PdbFileLlvm.h
        PdbFileLlvm.cpp
        ObjectFile.cpp
        SymbolsFile.cpp
        WindowsBuildIdUtils.cpp)

if (WIN32)
target_sources(
  ObjectUtils
  PRIVATE PdbFileDia.h
          PdbFileDia.cpp
          PdbUtilsDia.cpp)
target_sources(
  ObjectUtils
  PUBLIC include/ObjectUtils/PdbUtilsDia.h)
endif()

target_include_directories(ObjectUtils PUBLIC ${CMAKE_CURRENT_LIST_DIR}/include)

target_link_libraries(
  ObjectUtils
  PUBLIC GrpcProtos
         OrbitBase
         Introspection
         absl::base
         absl::flat_hash_map
         absl::flat_hash_set
         absl::memory
         absl::str_format
         absl::strings
         absl::synchronization
         absl::time
         absl::span
         )

# Link LLVM - use system LLVM on ARM64, Conan's llvm-core otherwise
if(ORBIT_USE_SYSTEM_LLVM)
  # Use SYSTEM to suppress warnings from LLVM headers (old-style casts, etc.)
  target_include_directories(ObjectUtils SYSTEM PUBLIC ${LLVM_INCLUDE_DIRS})
  target_compile_definitions(ObjectUtils PUBLIC ${LLVM_DEFINITIONS})
  # LLVM is built without RTTI by default, so disable RTTI for files that use LLVM types with dynamic_cast
  set_source_files_properties(PdbFileLlvm.cpp PROPERTIES COMPILE_FLAGS "-fno-rtti")
  llvm_map_components_to_libnames(LLVM_LIBS
    DebugInfoCodeView DebugInfoDWARF DebugInfoPDB Object Symbolize)
  target_link_libraries(ObjectUtils PUBLIC ${LLVM_LIBS})
else()
  target_link_libraries(ObjectUtils PUBLIC llvm-core::llvm-core)
endif()

if (WIN32)
target_link_libraries(ObjectUtils PUBLIC DIASDK::DIASDK)

add_custom_command(TARGET ObjectUtils POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E make_directory "${CMAKE_RUNTIME_OUTPUT_DIRECTORY}/"
    COMMAND ${CMAKE_COMMAND} -E copy_if_different "${DIASDK_DLL}" "${CMAKE_RUNTIME_OUTPUT_DIRECTORY}/")
endif()

add_executable(ObjectUtilsTests)

target_sources(ObjectUtilsTests PRIVATE
        CoffFileTest.cpp
        ElfFileTest.cpp
        ObjectFileTest.cpp
        PdbFileTest.h
        PdbFileLlvmTest.cpp
        SymbolsFileTest.cpp
        WindowsBuildIdUtilsTest.cpp)

if (WIN32)
target_sources(ObjectUtilsTests PRIVATE
        PdbFileDiaTest.cpp)
endif()

target_link_libraries(
  ObjectUtilsTests
  PRIVATE ObjectUtils
          TestUtils
          GTest_Main)

register_test(ObjectUtilsTests)

