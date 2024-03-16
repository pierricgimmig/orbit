# Copyright (c) 2021 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

cmake_minimum_required(VERSION 3.15)

project(WindowsUserSpaceInstrumentation)

add_library(WindowsUserSpaceInstrumentation SHARED)

set_target_properties(WindowsUserSpaceInstrumentation PROPERTIES OUTPUT_NAME "OrbitUserSpaceInstrumentation")

target_include_directories(WindowsUserSpaceInstrumentation PUBLIC
        ${CMAKE_CURRENT_LIST_DIR}/include)

target_sources(WindowsUserSpaceInstrumentation PUBLIC
        include/WindowsUserSpaceInstrumentation/WindowsUserSpaceInstrumentation.h)

target_sources(WindowsUserSpaceInstrumentation PRIVATE
        FunctionCallEventProducer.h
        WindowsUserSpaceInstrumentation.cpp
        )

target_link_libraries(WindowsUserSpaceInstrumentation PUBLIC
        CaptureEventProducer
        GrpcProtos
        OrbitBase
        ProducerSideChannel
        hijk)