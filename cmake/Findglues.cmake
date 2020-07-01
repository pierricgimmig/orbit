# Copyright (c) 2020 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set(DIR third_party/glues/source)

# We only include the subset that is needed for Orbit. It is discouraged to add
# stuff here. Ideally get rid of this at some point entirely.
add_library(
  glues OBJECT
  ${DIR}/glu.h
  ${DIR}/glues.h
  ${DIR}/glues_error.c
  ${DIR}/glues_error.h
  ${DIR}/glues_project.c
  ${DIR}/glues_project.h)

target_include_directories(glues SYSTEM PUBLIC ${DIR})
target_compile_definitions(glues PRIVATE GLUES_EXPORTS)
target_link_libraries(glues PUBLIC ANGLE::ANGLE)

add_library(glues::glues ALIAS glues)
