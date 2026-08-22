# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""OpenGL headers and the GL/GLX link-time symlinks.

Ubuntu ships libGL.so.1 in a default install but keeps GL/gl.h and the
unversioned libGL.so the linker resolves -lGL against in the -dev packages.
Both come from pinned .deb archives here so that a checkout builds without
anything having to be installed first.
"""

load("@rules_cc//cc:defs.bzl", "cc_library")

package(default_visibility = ["//visibility:public"])

licenses(["notice"])  # MIT (libglvnd) / MIT (Mesa headers)

cc_library(
    name = "opengl",
    srcs = [
        "usr/lib/{ARCH}/libGL.so.1",
        "usr/lib/{ARCH}/libGLX.so.0",
    ],
    hdrs = glob([
        "usr/include/GL/**/*.h",
        "usr/include/KHR/**/*.h",
    ]),
    includes = ["usr/include"],
)
