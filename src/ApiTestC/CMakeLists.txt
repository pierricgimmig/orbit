# Copyright (c) 2020 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

cmake_minimum_required(VERSION 3.15)

project(ApiTestC C)

add_executable(ApiTestC)

# We explicitly add the API include directory only as opposed to depending on the API module
# directly to emulate the real world scenario where user will only include Orbit.h without
# explicitly linking with liborbit.so, which is dynamically loaded.
target_include_directories(ApiTestC PUBLIC ${CMAKE_CURRENT_LIST_DIR}/../Api/include/)

target_sources(ApiTestC PRIVATE main.c ApiTestC.h ApiTestC.c)

if (NOT WIN32)
  target_link_libraries(ApiTestC PRIVATE m)
endif()
