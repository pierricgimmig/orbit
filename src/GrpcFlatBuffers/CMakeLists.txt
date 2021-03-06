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

target_include_directories(GrpcFlatBuffers PUBLIC
        ${CMAKE_CURRENT_LIST_DIR}/include)

target_sources(GrpcFlatBuffers PUBLIC
        include/GrpcFlatBuffers/Constants.h)

target_sources(GrpcFlatBuffers PRIVATE
        capture.fbs
        code_block.fbs
        module.fbs
        process.fbs
        producer_side_services.fbs
        services.fbs
        services_ggp.fbs
        symbol.fbs
        tracepoint.fbs)

#get_target_property(FLATBUFFERS_SCHEMAS GrpcFlatBuffers SOURCES)

set(FLATBUFFER_OUTPUT_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/flatbuffers_codegen")
add_custom_command(
        OUTPUT "${FLATBUFFER_OUTPUT_DIRECTORY}"
        COMMAND ${CMAKE_COMMAND} ARGS -E make_directory "${FLATBUFFER_OUTPUT_DIRECTORY}")

# FlatBuffers
set(GENERATED_INCLUDES_DIR "${CMAKE_CURRENT_BINARY_DIR}/flatbuffers_codegen/")

file(GLOB_RECURSE FLATBUFFERS_SCHEMAS ${CMAKE_CURRENT_LIST_DIR}/*.fbs)
message(STATUS "FLATBUFFERS_SCHEMAS: ${FLATBUFFERS_SCHEMAS}")

find_program(FLATBUFFERS_FLATC_EXECUTABLE flatc)
message( "FLATBUFFERS_FLATC_EXECUTABLE = ${FLATBUFFERS_FLATC_EXECUTABLE}")

build_flatbuffers("${FLATBUFFERS_SCHEMAS}"
                  ""
                  GENERATED_INCLUDES_
                  ""
                  "${GENERATED_INCLUDES_DIR}"
                  ""
                  "")

add_dependencies(GrpcFlatBuffers GENERATED_INCLUDES_)

target_link_libraries(GrpcFlatBuffers PUBLIC CONAN_PKG::flatbuffers)
