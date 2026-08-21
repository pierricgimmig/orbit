# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Protobuf and gRPC code generation for Orbit's two proto packages.

Orbit's .proto files import each other by bare file name ("capture.proto"),
which means protoc has to run with the package directory itself on the import
path, while the generated C++ headers are included with a module prefix
("GrpcProtos/capture.pb.h"). proto_library cannot express both at once, so
protoc is driven directly -- the same way cmake/grpc_helper.cmake does it.

Both proto packages are self-contained: nothing outside them imports their
files, and neither imports the other. One generation step per package is
therefore exactly the right granularity.
"""

load("@rules_cc//cc:defs.bzl", "cc_library")

def orbit_proto_library(name, srcs, prefix, grpc = False, visibility = None):
    """Generates and compiles C++ (and optionally gRPC) code for `srcs`.

    Args:
      name: name of the resulting cc_library.
      srcs: the .proto files, all in this package.
      prefix: directory the generated headers are placed under, which is also
        the prefix they are included with (e.g. "GrpcProtos").
      grpc: also run the grpc_cpp_plugin and link gRPC.
      visibility: visibility of the cc_library.
    """
    bases = [src[:-len(".proto")] for src in srcs]

    generated_srcs = ["%s/%s.pb.cc" % (prefix, base) for base in bases]
    generated_hdrs = ["%s/%s.pb.h" % (prefix, base) for base in bases]
    if grpc:
        generated_srcs += ["%s/%s.grpc.pb.cc" % (prefix, base) for base in bases]
        generated_hdrs += ["%s/%s.grpc.pb.h" % (prefix, base) for base in bases]

    protoc = "@protobuf//:protoc"
    plugin = "@grpc//src/compiler:grpc_cpp_plugin"

    command = [
        "OUT=$(RULEDIR)/{prefix}",
        "mkdir -p $$OUT",
        " ".join([
            "$(execpath {protoc})",
            "--cpp_out=$$OUT",
        ] + ([
            "--plugin=protoc-gen-grpc=$(execpath {plugin})",
            "--grpc_out=$$OUT",
        ] if grpc else []) + [
            # Orbit's protos import each other by bare file name.
            "-I $$(dirname $$(echo $(SRCS) | cut -d' ' -f1))",
            "$(SRCS)",
        ]),
    ]

    native.genrule(
        name = name + "_gen",
        srcs = srcs,
        outs = generated_srcs + generated_hdrs,
        cmd = " && ".join(command).format(
            prefix = prefix,
            protoc = protoc,
            plugin = plugin,
        ),
        message = "Generating protobuf sources for %s" % name,
        tools = [protoc] + ([plugin] if grpc else []),
    )

    cc_library(
        name = name,
        srcs = generated_srcs,
        hdrs = generated_hdrs,
        # Generated headers live in $(RULEDIR)/<prefix>, so the package's own
        # output directory is what has to be on the include path.
        includes = ["."],
        visibility = visibility,
        deps = ["@protobuf//:protobuf"] + (["@grpc//:grpc++_unsecure"] if grpc else []),
    )
