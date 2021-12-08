# Copyright (c) 2020 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# To make OrbitAsioServer and OrbitGrpcServer available for fuzz-testing
# the target OrbitWindowsServiceLib was introduced. OrbitService still exists,
# but only builds main.cpp and links to OrbitWindowsServiceLib.
project(WindowsService)

add_library(ServiceLib STATIC)
target_compile_options(ServiceLib PRIVATE ${STRICT_COMPILE_FLAGS})

target_sources(ServiceLib PRIVATE
        OrbitGrpcServer.cpp
        OrbitGrpcServer.h
        OrbitService.cpp
        OrbitService.h)

if(CMAKE_CXX_COMPILER_ID MATCHES "MSVC")
  set_target_properties(ServiceLib PROPERTIES COMPILE_FLAGS /wd4127)
endif()

target_include_directories(ServiceLib PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})

target_link_libraries(ServiceLib PUBLIC
        GrpcProtos
        Introspection
        WindowsCaptureService
        WindowsProcessService
        WindowsTracing
        WindowsUtils
        ObjectUtils
        OrbitVersion)

project(OrbitService)
add_executable(OrbitService main.cpp)
target_link_libraries(OrbitService PRIVATE ServiceLib)
target_compile_options(OrbitService PRIVATE ${STRICT_COMPILE_FLAGS})

SET_TARGET_PROPERTIES(OrbitService PROPERTIES LINK_FLAGS "/MANIFESTUAC:\"level='requireAdministrator' uiAccess='false'\" /SUBSYSTEM:CONSOLE")