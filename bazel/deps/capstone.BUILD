# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Capstone disassembly framework.

cs.c pulls in every architecture module header unconditionally and selects
between them with CAPSTONE_HAS_*, so all architectures are compiled in -- the
same configuration the Conan package provided.
"""

load("@rules_cc//cc:defs.bzl", "cc_library")

package(default_visibility = ["//visibility:public"])

licenses(["notice"])  # BSD-3-Clause

_ARCHITECTURES = [
    "ARM",
    "ARM64",
    "AARCH64",
    "M68K",
    "MIPS",
    "POWERPC",
    "SPARC",
    "SYSTEMZ",
    "X86",
    "XCORE",
    "TMS320C64X",
    "M680X",
    "EVM",
    "MOS65XX",
    "WASM",
    "BPF",
    "RISCV",
    "SH",
    "TRICORE",
]

_DEFINES = ["CAPSTONE_HAS_" + arch for arch in _ARCHITECTURES] + [
    # Use malloc/free rather than Capstone's own allocator hooks.
    "CAPSTONE_USE_SYS_DYN_MEM",
]

cc_library(
    name = "capstone",
    srcs = glob(
        [
            "*.c",
            "*.h",
            "arch/*/*.c",
            "arch/*/*.h",
            "arch/*/*.inc",
        ],
        exclude = ["test*.c"],
    ),
    hdrs = glob(["include/capstone/*.h"]),
    # Third-party C that predates most of the warnings Orbit turns on.
    copts = ["-w"],
    defines = _DEFINES,
    # cs.c and the arch modules include <platform.h> from the repository root.
    includes = [
        ".",
        "include",
    ],
    local_defines = _DEFINES,
)
