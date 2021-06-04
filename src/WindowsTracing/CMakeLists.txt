# Copyright (c) 2021 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

cmake_minimum_required(VERSION 3.15)

project(WindowsTracing)

add_library(WindowsTracing STATIC)

target_compile_definitions(WindowsTracing PUBLIC WIN32_LEAN_AND_MEAN)
target_compile_definitions(WindowsTracing PUBLIC INITGUID)


target_compile_options(WindowsTracing PRIVATE ${STRICT_COMPILE_FLAGS})

target_compile_features(WindowsTracing PUBLIC cxx_std_17)

target_include_directories(WindowsTracing PUBLIC
        ${CMAKE_CURRENT_LIST_DIR}/include)

target_include_directories(WindowsTracing PRIVATE
        ${CMAKE_CURRENT_LIST_DIR})

target_sources(WindowsTracing PRIVATE
        include/WindowsTracing/Etw.h
        include/WindowsTracing/EventCallbacks.h
        include/WindowsTracing/EventGuid.h
        include/WindowsTracing/EventTracer.h
        include/WindowsTracing/EventTypes.h
        include/WindowsTracing/Tracer.h
        include/WindowsTracing/TracerListener.h
        include/WindowsTracing/WindowsTracing.h)

target_sources(WindowsTracing PRIVATE
        EventCallbacks.cpp
        EventTracer.cpp
        Tracer.cpp
        WindowsTracing.cpp)

target_include_directories(WindowsTracing INTERFACE ${CMAKE_CURRENT_SOURCE_DIR}/include)

target_link_libraries(WindowsTracing PUBLIC
        GrpcProtos
        OrbitBase
        CONAN_PKG::abseil
        CONAN_PKG::grpc
        CONAN_PKG::Outcome)
