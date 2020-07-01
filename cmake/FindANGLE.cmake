# Copyright (c) 2020 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set(DIR third_party/ANGLE)

add_library(
  ANGLE OBJECT
  ${DIR}/GLES2/gl2.h
  ${DIR}/GLES2/gl2ext.h
  ${DIR}/GLES2/gl2ext_angle.h
  ${DIR}/GLES2/gl2platform.h
  ${DIR}/KHR/khrplatform.h)

target_include_directories(ANGLE SYSTEM PUBLIC ${DIR})
set_target_properties(ANGLE PROPERTIES LINKER_LANGUAGE CXX)

add_library(ANGLE::ANGLE ALIAS ANGLE)
