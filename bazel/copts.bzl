# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Compiler options shared by every first-party Orbit target.

These mirror the warning configuration the CMake build applies project-wide.
They are deliberately *not* set in .bazelrc: third-party code fetched through
Bazel modules is not expected to satisfy them.
"""

_GCC_WARNINGS = [
    "-Werror=all",
    # gcc does not behave consistently here; what is fine with gcc9 fails with
    # gcc10 and vice versa. See https://github.com/google/orbit/issues/1624.
    "-Wno-stringop-truncation",
    "-Werror=float-conversion",
    "-Werror=format=2",
    "-Werror=ignored-attributes",
    "-Werror=old-style-cast",
    "-Werror=unused-parameter",
    "-Werror=unused-variable",
    "-Werror=sign-compare",
    # These seem to be buggy in GCC 11 and 12:
    "-Wno-maybe-uninitialized",
    "-Wno-uninitialized",
    "-Wno-stringop-overflow",
]

_CLANG_WARNINGS = [
    "-Werror=all",
    "-Werror=abstract-final-class",
    "-Werror=float-conversion",
    "-Werror=format=2",
    "-Werror=ignored-attributes",
    "-Werror=implicit-fallthrough",
    "-Werror=inconsistent-missing-override",
    "-Werror=old-style-cast",
    "-Werror=unused-parameter",
    "-Werror=unused-variable",
    "-Werror=writable-strings",
    "-Werror=sign-compare",
    "-Werror=thread-safety",
    "-Werror=defaulted-function-deleted",
    # Required by Google-internal builds.
    "-Werror=ctad-maybe-unsupported",
]

_MSVC_WARNINGS = [
    "/WX",
    "/utf-8",
    "/experimental:external",
    "/external:anglebrackets",
    "/external:W0",
]

# Orbit includes third-party headers with angle brackets --
# `#include <absl/strings/str_cat.h>` -- which is what the Conan-based build
# established. Bazel publishes a module's headers relative to its repository
# root through -iquote, which only satisfies quoted includes, so the roots of
# the dependencies that do not already declare an `includes` path of their own
# are added as system include directories here.
#
# Label.workspace_root resolves to the canonical repository directory, so this
# keeps working when module versions (and with them the directory names) change.
_SYSTEM_INCLUDE_ROOTS = [
    Label("@abseil-cpp//absl/base:base"),
]

# Third-party code vendored into this repository. Bazel's
# external_include_paths feature only covers other repositories, so the
# directories cmake/Find*.cmake marks SYSTEM are listed here instead.
_VENDORED_SYSTEM_INCLUDES = [
    "third_party/concurrentqueue",
    "third_party/gte",
    "third_party/libbase/include",
    "third_party/libcutils/include",
    "third_party/liblog/include",
    "third_party/libprocinfo/include",
    "third_party/libunwindstack/include",
    "third_party/lzma1900/C",
    "third_party/xxHash-r42",
]

ORBIT_SYSTEM_INCLUDES = [
    "-isystem" + label.workspace_root
    for label in _SYSTEM_INCLUDE_ROOTS
] + ["-isystem" + path for path in _VENDORED_SYSTEM_INCLUDES]

# Defined on every platform to keep the builds as similar as possible, even
# though only Windows needs them. This is what add_definitions() in the
# top-level CMakeLists.txt does.
_DEFINES = [
    "-DNOMINMAX",
    "-DUNICODE",
    "-D_UNICODE",
]

ORBIT_COPTS = ORBIT_SYSTEM_INCLUDES + _DEFINES + select({
    "@rules_cc//cc/compiler:msvc-cl": _MSVC_WARNINGS,
    "@rules_cc//cc/compiler:clang": _CLANG_WARNINGS,
    "@rules_cc//cc/compiler:gcc": _GCC_WARNINGS,
    "//conditions:default": [],
})

# Bazel applies `copts` to C and C++ alike, and gcc rejects the C++-only
# warnings when compiling C. Targets that are pure C use this instead.
ORBIT_C_COPTS = ORBIT_SYSTEM_INCLUDES + _DEFINES + select({
    "@rules_cc//cc/compiler:msvc-cl": _MSVC_WARNINGS,
    "//conditions:default": [
        "-Werror=all",
        "-Werror=format=2",
        "-Werror=ignored-attributes",
        "-Werror=unused-parameter",
        "-Werror=unused-variable",
        "-Werror=sign-compare",
    ],
})

