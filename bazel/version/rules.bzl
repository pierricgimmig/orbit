# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Expands OrbitVersion.cpp.in from Bazel's workspace status."""

def _version_file_impl(ctx):
    output = ctx.actions.declare_file(ctx.attr.out)

    ctx.actions.run(
        executable = ctx.executable._expand,
        arguments = [ctx.info_file.path, ctx.file.template.path, output.path],
        inputs = [ctx.info_file, ctx.file.template],
        outputs = [output],
        mnemonic = "OrbitVersion",
        progress_message = "Generating %s" % output.short_path,
    )

    return [DefaultInfo(files = depset([output]))]

orbit_version_file = rule(
    implementation = _version_file_impl,
    doc = "Expands a version template using --workspace_status_command output.",
    attrs = {
        "template": attr.label(allow_single_file = True, mandatory = True),
        "out": attr.string(mandatory = True),
        "_expand": attr.label(
            default = "//bazel/version:expand_version",
            executable = True,
            cfg = "exec",
        ),
    },
)
