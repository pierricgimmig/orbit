# Copyright (c) 2021 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

cmake_minimum_required(VERSION 3.15)

project(WindowsApiTracing)

add_library(WindowsApiTracing STATIC)

target_compile_features(WindowsApiTracing PUBLIC cxx_std_17)

target_include_directories(WindowsApiTracing PUBLIC
        ${CMAKE_CURRENT_LIST_DIR}/include)

target_sources(WindowsApiTracing PUBLIC
        ${CMAKE_CURRENT_LIST_DIR}/include/WindowsApiTracing/WindowsApiTracing.h)

target_sources(WindowsApiTracing PRIVATE
        ${CMAKE_CURRENT_LIST_DIR}/WindowsApiTracing.cpp)

target_link_libraries(WindowsApiTracing PUBLIC
        OrbitBase
        WindowsUtils
        CONAN_PKG::abseil)

add_executable(WindowsApiTracingTests)

target_sources(WindowsApiTracingTests PRIVATE
        WindowsApiTracingTest.cpp)

target_link_libraries(WindowsApiTracingTests PRIVATE
        TestUtils
        WindowsApiTracing
        GTest::GTest
        GTest::Main)

SET_TARGET_PROPERTIES(WindowsApiTracingTests PROPERTIES LINK_FLAGS "/MANIFESTUAC:\"level='requireAdministrator' uiAccess='false'\"")

register_test(WindowsApiTracingTests)
