# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Qt 5 as unpacked from the pinned Ubuntu packages.

Each Qt module is exposed as a cc_library that links the SONAME-named shared
object, so the DT_NEEDED entry the linker records matches the file Bazel stages
into the runfiles tree.
"""

load("@rules_cc//cc:defs.bzl", "cc_library")

package(default_visibility = ["//visibility:public"])

licenses(["notice"])  # LGPL-3.0, see usr/share/doc/*/copyright.

_INCLUDE = "usr/include/{ARCH}/qt5"

_LIB = "usr/lib/{ARCH}"

# Shared libraries Qt itself needs that are not part of a default Ubuntu
# install. They carry no headers; they only have to be present at load time.
cc_library(
    name = "support_libs",
    srcs = [
        _LIB + "/libdouble-conversion.so.3",
        _LIB + "/libmd4c.so.0",
        _LIB + "/libpcre2-16.so.0",
    ],
)

# What the xcb platform plugin needs to load.
#
# libqxcb.so is dlopened by Qt and carries no RUNPATH, so the loader can only
# resolve its dependencies against what is already in the process. Linking
# these into the application puts them there: libQt5XcbQpa satisfies libqxcb,
# and the executable's own RUNPATH then covers libQt5XcbQpa's xcb dependencies.
#
# Nothing in the application references a symbol from any of them, so the link
# has to be told to keep them.
cc_library(
    name = "platform_libs",
    srcs = [
        _LIB + "/libQt5DBus.so.5",
        _LIB + "/libQt5XcbQpa.so.5",
        _LIB + "/libxcb-icccm.so.4",
        _LIB + "/libxcb-image.so.0",
        _LIB + "/libxcb-keysyms.so.1",
        _LIB + "/libxcb-render-util.so.0",
        _LIB + "/libxcb-util.so.1",
        _LIB + "/libxcb-xinerama.so.0",
        _LIB + "/libxcb-xinput.so.0",
        _LIB + "/libxcb-xkb.so.1",
        _LIB + "/libxkbcommon-x11.so.0",
    ],
    linkopts = ["-Wl,--no-as-needed"],
)

cc_library(
    name = "Core",
    srcs = [_LIB + "/libQt5Core.so.5"],
    hdrs = glob([
        _INCLUDE + "/QtCore/**",
    ]),
    includes = [
        _INCLUDE,
        _INCLUDE + "/QtCore",
    ],
    defines = ["QT_CORE_LIB"],
    linkopts = ["-lpthread"],
    deps = [":support_libs"],
)

cc_library(
    name = "Gui",
    srcs = [_LIB + "/libQt5Gui.so.5"],
    hdrs = glob([_INCLUDE + "/QtGui/**"]),
    defines = ["QT_GUI_LIB"],
    includes = [_INCLUDE + "/QtGui"],
    deps = [
        ":Core",
        "@opengl",
    ],
)

cc_library(
    name = "Widgets",
    srcs = [_LIB + "/libQt5Widgets.so.5"],
    hdrs = glob([_INCLUDE + "/QtWidgets/**"]),
    defines = ["QT_WIDGETS_LIB"],
    includes = [_INCLUDE + "/QtWidgets"],
    deps = [":Gui"],
)

cc_library(
    name = "Network",
    srcs = [_LIB + "/libQt5Network.so.5"],
    hdrs = glob([_INCLUDE + "/QtNetwork/**"]),
    defines = ["QT_NETWORK_LIB"],
    includes = [_INCLUDE + "/QtNetwork"],
    deps = [":Core"],
)

cc_library(
    name = "Test",
    srcs = [_LIB + "/libQt5Test.so.5"],
    hdrs = glob([_INCLUDE + "/QtTest/**"]),
    defines = ["QT_TESTLIB_LIB"],
    includes = [_INCLUDE + "/QtTest"],
    deps = [":Widgets"],
)

# Qt's own plugins. QT_QPA_PLATFORM=offscreen resolves against this tree, so
# tests that create a QGuiApplication need it in their runfiles.
filegroup(
    name = "plugins",
    srcs = glob([_LIB + "/qt5/plugins/**"]),
)

filegroup(
    name = "plugins_dir_marker",
    srcs = [_LIB + "/qt5/plugins/platforms/libqoffscreen.so"],
)

# The code generators. `moc` and `rcc` are statically bootstrapped; `uic` links
# libQt5Core, so every tool is wrapped with the same library-path handling by
# //bazel/qt:rules.bzl.
filegroup(
    name = "moc",
    srcs = ["usr/lib/qt5/bin/moc"],
)

filegroup(
    name = "uic",
    srcs = ["usr/lib/qt5/bin/uic"],
)

filegroup(
    name = "rcc",
    srcs = ["usr/lib/qt5/bin/rcc"],
)

filegroup(
    name = "tool_runtime",
    srcs = [_LIB + "/libQt5Core.so.5"] + glob([
        _LIB + "/libdouble-conversion.so.3",
        _LIB + "/libpcre2-16.so.0",
    ]),
)
