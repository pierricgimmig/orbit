# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Outcome, in its standalone single-header form.

This is the same library the Conan-based build pulled in as outcome/2.2.9. The
shim in //third_party/Outcome exists only for builds that have to fall back to
the copy of Outcome that ships inside Boost; it is not used here.
"""

load("@rules_cc//cc:defs.bzl", "cc_library")

package(default_visibility = ["//visibility:public"])

licenses(["notice"])  # Apache-2.0

cc_library(
    name = "outcome",
    hdrs = ["single-header/outcome.hpp"],
    includes = ["single-header"],
)

exports_files(["Licence.txt"])
