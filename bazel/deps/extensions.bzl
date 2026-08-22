# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Module extensions for dependencies that are not Bazel modules."""

load("//bazel/deps:deb.bzl", "deb_list", "deb_packages")
load("//bazel/deps:dia_sdk.bzl", "dia_sdk")
load("//bazel/deps:debs.bzl", "OPENGL_DEBS", "QT5_DEBS")

# Ubuntu's multiarch directory, which every path inside the Qt packages is
# qualified by.
_ARCH = "x86_64-linux-gnu"

def _qt5_impl(_module_ctx):
    deb_packages(
        name = "qt5",
        packages = deb_list(QT5_DEBS),
        build_file = "//bazel/qt:qt5.BUILD.tpl",
        substitutions = {"{ARCH}": _ARCH},
    )

qt5 = module_extension(
    implementation = _qt5_impl,
    doc = "Materializes Qt 5 from the pinned Ubuntu packages in debs.bzl.",
)

def _opengl_impl(_module_ctx):
    deb_packages(
        name = "opengl",
        packages = deb_list(OPENGL_DEBS),
        build_file = "//bazel/deps:opengl.BUILD.tpl",
        substitutions = {"{ARCH}": _ARCH},
    )

opengl = module_extension(
    implementation = _opengl_impl,
    doc = "Materializes the OpenGL headers and link-time libraries.",
)


def _dia_sdk_impl(_module_ctx):
    dia_sdk(name = "dia_sdk")

dia = module_extension(
    implementation = _dia_sdk_impl,
    doc = "Locates the DIA SDK that ships with Visual Studio.",
)
