# Copyright (c) 2020 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

cmake_minimum_required(VERSION 3.15)

project(GrpcFlatBuffers CXX)

add_library(GrpcFlatBuffers STATIC)

SET_TARGET_PROPERTIES(GrpcFlatBuffers PROPERTIES LINKER_LANGUAGE CXX)

target_compile_options(GrpcFlatBuffers PRIVATE ${STRICT_COMPILE_FLAGS})
target_compile_definitions(GrpcFlatBuffers PRIVATE -D_WIN32_WINNT=0x0700)
target_compile_definitions(GrpcFlatBuffers PRIVATE -DNTDDI_VERSION=0x06030000)

set(FLATBUFFER_SCHEMAS
        capture.fbs
        code_block.fbs
        module.fbs
        process.fbs
        producer_side_services.fbs
        services.fbs
        services_ggp.fbs
        symbol.fbs
        tracepoint.fbs)

target_sources(GrpcFlatBuffers PUBLIC
        include/GrpcFlatBuffers/Constants.h
        ${FLATBUFFER_SCHEMAS})

set(FLATBUFFER_CODE_GEN_ROOT_DIR "${CMAKE_CURRENT_BINARY_DIR}/flatbuffers_codegen/")
set(FLATBUFFER_CODE_GEN_DIR "${FLATBUFFER_CODE_GEN_ROOT_DIR}flatbuffers")

file(MAKE_DIRECTORY ${FLATBUFFER_CODE_GEN_DIR})

find_program(FLATBUFFERS_FLATC_EXECUTABLE flatc)
include("${FLATBUFFERS_FLATC_EXECUTABLE}/../cmake/BuildFlatBuffers.cmake")

build_flatbuffers("${FLATBUFFER_SCHEMAS}"
                  ""
                  FLATBUFFER_CODE_GEN_DIR
                  ""
                  "${FLATBUFFER_CODE_GEN_DIR}"
                  ""
                  "")

target_include_directories(GrpcFlatBuffers PUBLIC
        ${CMAKE_CURRENT_LIST_DIR}/include
        ${FLATBUFFER_CODE_GEN_ROOT_DIR})

add_dependencies(GrpcFlatBuffers FLATBUFFER_CODE_GEN_DIR)

target_link_libraries(GrpcFlatBuffers PUBLIC CONAN_PKG::flatbuffers)