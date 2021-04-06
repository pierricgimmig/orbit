# Copyright (c) 2020 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

cmake_minimum_required(VERSION 3.15)

project(Introspection)
add_library(Introspection STATIC)

target_compile_options(Introspection PRIVATE ${STRICT_COMPILE_FLAGS})

target_include_directories(Introspection PUBLIC
        ${CMAKE_CURRENT_LIST_DIR}/include)

target_include_directories(Introspection PRIVATE
        ${CMAKE_CURRENT_LIST_DIR})

target_sources(
  Introspection
  PUBLIC include/Introspection/DebugUtils.h)

target_sources(
  Introspection
  PRIVATE DebugUtils.cpp)

target_include_directories(Introspection PUBLIC ${CMAKE_CURRENT_LIST_DIR})

target_link_libraries(
  Introspection
  PUBLIC OrbitBase
         CONAN_PKG::abseil)

target_compile_features(Introspection PUBLIC cxx_std_17)

add_executable(IntrospectionTests)

target_compile_options(IntrospectionTests PRIVATE ${STRICT_COMPILE_FLAGS})

target_sources(IntrospectionTests PRIVATE
    DebugUtilsTest.cpp)

target_link_libraries(
  IntrospectionTests
  PRIVATE Introspection
          GTest::Main
          CONAN_PKG::abseil)

register_test(IntrospectionTests)
