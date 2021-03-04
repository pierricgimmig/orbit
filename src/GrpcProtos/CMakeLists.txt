# Copyright (c) 2020 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

cmake_minimum_required(VERSION 3.15)

project(GrpcProtos)

add_library(GrpcProtos STATIC)
target_compile_options(GrpcProtos PRIVATE ${STRICT_COMPILE_FLAGS})
target_compile_definitions(GrpcProtos PRIVATE -D_WIN32_WINNT=0x0700)
target_compile_definitions(GrpcProtos PRIVATE -DNTDDI_VERSION=0x06030000)

target_include_directories(GrpcProtos PUBLIC
        ${CMAKE_CURRENT_LIST_DIR}/include)

target_sources(GrpcProtos PUBLIC
        include/GrpcProtos/Constants.h)

target_sources(GrpcProtos PRIVATE
        capture.proto
        code_block.proto
        module.proto
        process.proto
        producer_side_services.proto
        services.proto
        services_ggp.proto
        symbol.proto
        tracepoint.proto)

# FlatBuffers
set(GENERATED_INCLUDES_DIR "${CMAKE_CURRENT_BINARY_DIR}/grpc_codegen/flatbuffers/")

file(GLOB_RECURSE FLATBUFFERS_SCHEMAS ${CMAKE_CURRENT_LIST_DIR}/*.fbs)

set(FLATBUFFERS_FLATC_SCHEMA_EXTRA_ARGS
        "--proto-namespace-suffix" "blah")

set(FLATC_ARGS "${FLATBUFFERS_FLATC_SCHEMA_EXTRA_ARGS}")

build_flatbuffers("${FLATBUFFERS_SCHEMAS}"
                  ""
                  GENERATED_INCLUDES
                  ""
                  "${GENERATED_INCLUDES_DIR}"
                  ""
                  "")

message(STATUS "FLATBUFFERS_SCHEMAS: ${FLATBUFFERS_SCHEMAS}")

add_dependencies(GrpcProtos GENERATED_INCLUDES)

target_link_libraries(GrpcProtos PUBLIC CONAN_PKG::flatbuffers)

grpc_helper(GrpcProtos)
