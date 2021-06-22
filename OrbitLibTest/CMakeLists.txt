# Copyright (c) 2020 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

project(OrbitTest)

add_executable(OrbitTest)

target_sources(OrbitTest PRIVATE main.cpp)

target_include_directories(OrbitTest PRIVATE ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(OrbitTest PRIVATE OrbitAsm OrbitLib)

get_cmake_property(_variableNames VARIABLES)
list (SORT _variableNames)
foreach (_variableName ${_variableNames})
    message(STATUS "${_variableName}=${${_variableName}}")
endforeach()
 

add_custom_command(TARGET OrbitTest POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_directory
        "${PROJECT_BINARY_DIR}/../OrbitLib/$<CONFIGURATION>"
        $<TARGET_FILE_DIR:OrbitTest>)