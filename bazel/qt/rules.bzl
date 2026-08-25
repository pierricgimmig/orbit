# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Build rules for Qt 5's code generators.

CMake's AUTOMOC/AUTOUIC/AUTORCC scan sources at build time. Bazel needs the
generated files declared up front, so the moc-able headers, .ui forms and .qrc
resource files are listed explicitly on the target instead.
"""

load("@bazel_skylib//rules:write_file.bzl", "write_file")
load("@rules_cc//cc:defs.bzl", "cc_binary", "cc_library", "cc_test")

def _tool_env(ctx):
    """LD_LIBRARY_PATH for the Qt tools, which link libQt5Core."""
    directories = {f.dirname: None for f in ctx.files._tool_runtime}
    return {"LD_LIBRARY_PATH": ":".join(directories.keys())}

def _include_relative_name(path, include_dirs):
    """The spelling a header has in an #include directive.

    `path` is workspace-relative; `include_dirs` are the workspace-relative
    directories on the consuming target's include path, longest match first.
    Headers that are not under any of them keep their workspace-relative path,
    which resolves through the -iquote entry Bazel adds for the workspace root.
    """
    for directory in sorted(include_dirs, key = len, reverse = True):
        prefix = directory + "/"
        if path.startswith(prefix):
            return path[len(prefix):]

    # A private header, reachable through the -iquote path Bazel adds for the
    # workspace root.
    return path

def _qt_moc_impl(ctx):
    include_dirs = [
        ctx.label.package + "/" + d if d != "." else ctx.label.package
        for d in ctx.attr.include_dirs
    ]

    outputs = []
    for header in ctx.files.hdrs:
        output = ctx.actions.declare_file("moc_%s.cpp" % header.basename[:-len(".h")])
        outputs.append(output)
        ctx.actions.run(
            executable = ctx.file._moc,
            arguments = [
                header.path,
                "-o",
                output.path,
                # Without this moc emits the path it was handed, which is not
                # resolvable from the generated file's own directory.
                "-f",
                _include_relative_name(header.short_path, include_dirs),
                "--no-notes",
            ] + ["-D" + d for d in ctx.attr.defines],
            inputs = depset([header], transitive = [depset(ctx.files._tool_runtime)]),
            outputs = [output],
            env = _tool_env(ctx),
            mnemonic = "QtMoc",
            progress_message = "Running moc on %s" % header.short_path,
        )

    return [DefaultInfo(files = depset(outputs))]

qt_moc = rule(
    implementation = _qt_moc_impl,
    doc = "Runs moc over headers that declare Q_OBJECT, Q_GADGET or Q_NAMESPACE.",
    attrs = {
        "hdrs": attr.label_list(allow_files = [".h"], mandatory = True),
        "include_dirs": attr.string_list(
            default = ["."],
            doc = "Package-relative include directories, used to spell the " +
                  "#include moc writes into the generated file.",
        ),
        "defines": attr.string_list(),
        "_moc": attr.label(
            default = "@qt5//:moc",
            allow_single_file = True,
            cfg = "exec",
        ),
        "_tool_runtime": attr.label(default = "@qt5//:tool_runtime", cfg = "exec"),
    },
)

def _qt_uic_impl(ctx):
    outputs = []
    for form in ctx.files.ui:
        output = ctx.actions.declare_file("ui_%s.h" % form.basename[:-len(".ui")])
        outputs.append(output)
        ctx.actions.run(
            executable = ctx.file._uic,
            arguments = [form.path, "-o", output.path],
            inputs = depset([form], transitive = [depset(ctx.files._tool_runtime)]),
            outputs = [output],
            env = _tool_env(ctx),
            mnemonic = "QtUic",
            progress_message = "Running uic on %s" % form.short_path,
        )

    return [DefaultInfo(files = depset(outputs))]

qt_uic = rule(
    implementation = _qt_uic_impl,
    doc = "Compiles Qt Designer .ui forms into ui_<name>.h headers.",
    attrs = {
        "ui": attr.label_list(allow_files = [".ui"], mandatory = True),
        "_uic": attr.label(
            default = "@qt5//:uic",
            allow_single_file = True,
            cfg = "exec",
        ),
        "_tool_runtime": attr.label(default = "@qt5//:tool_runtime", cfg = "exec"),
    },
)

def _qt_rcc_impl(ctx):
    output = ctx.actions.declare_file("qrc_%s.cpp" % ctx.attr.name)
    ctx.actions.run(
        executable = ctx.file._rcc,
        arguments = [
            "--name",
            ctx.attr.resource_name or ctx.attr.name,
            "--output",
            output.path,
            ctx.file.qrc.path,
        ],
        # rcc resolves the <file> entries relative to the .qrc file itself.
        inputs = depset(
            [ctx.file.qrc] + ctx.files.data,
            transitive = [depset(ctx.files._tool_runtime)],
        ),
        outputs = [output],
        env = _tool_env(ctx),
        mnemonic = "QtRcc",
        progress_message = "Running rcc on %s" % ctx.file.qrc.short_path,
    )

    return [DefaultInfo(files = depset([output]))]

qt_rcc = rule(
    implementation = _qt_rcc_impl,
    doc = "Compiles a .qrc resource collection into a C++ source file.",
    attrs = {
        "qrc": attr.label(allow_single_file = [".qrc"], mandatory = True),
        "data": attr.label_list(allow_files = True),
        "resource_name": attr.string(),
        "_rcc": attr.label(
            default = "@qt5//:rcc",
            allow_single_file = True,
            cfg = "exec",
        ),
        "_tool_runtime": attr.label(default = "@qt5//:tool_runtime", cfg = "exec"),
    },
)

def qt_cc_library(
        name,
        srcs = [],
        hdrs = [],
        moc_hdrs = [],
        ui = [],
        include_dirs = ["."],
        moc_defines = [],
        **kwargs):
    """A cc_library that also runs moc and uic over the parts that need it.

    Args:
      name: target name.
      srcs: ordinary C++ sources.
      hdrs: exported headers.
      moc_hdrs: headers declaring Q_OBJECT/Q_GADGET/Q_NAMESPACE. These are
        also exported, so they do not need to be repeated in `hdrs`.
      ui: Qt Designer forms. The generated ui_<name>.h lands in the package's
        output directory, which `includes` puts on the header search path.
      include_dirs: package-relative include directories.
      moc_defines: preprocessor defines moc needs to see.
      **kwargs: forwarded to cc_library.
    """
    generated_srcs = []
    generated_hdrs = []

    if moc_hdrs:
        qt_moc(
            name = name + "_moc",
            hdrs = moc_hdrs,
            include_dirs = include_dirs,
            defines = moc_defines,
            testonly = kwargs.get("testonly", False),
        )
        generated_srcs.append(name + "_moc")

    if ui:
        qt_uic(
            name = name + "_uic",
            ui = ui,
            testonly = kwargs.get("testonly", False),
        )
        generated_hdrs.append(name + "_uic")

    cc_library(
        name = name,
        srcs = srcs + generated_srcs,
        hdrs = hdrs + moc_hdrs + generated_hdrs,
        includes = kwargs.pop("includes", []) + include_dirs,
        **kwargs
    )


# Qt looks for its platform plugin under the prefix baked into libQt5Core at
# build time, which points at a system-wide install that is not there. Tests
# get pointed at the plugins in their own runfiles instead.
#
# A test's working directory is <runfiles>/_main, and Label.workspace_root
# names the repository directory, so the plugins are one level up from there.
_QT5_REPOSITORY = Label("@qt5//:plugins").workspace_root.removeprefix("external/")

QT_TEST_ENV = {
    "QT_PLUGIN_PATH": "../%s/usr/lib/x86_64-linux-gnu/qt5/plugins" % _QT5_REPOSITORY,
    # Widget tests must not need a display; this is what cmake/tests.cmake sets.
    "QT_QPA_PLATFORM": "offscreen",
}

def qt_cc_test(name, data = [], env = {}, **kwargs):
    """A cc_test that can create a QGuiApplication."""
    cc_test(
        name = name,
        data = data + ["@qt5//:plugins"],
        env = dict(QT_TEST_ENV, **env),
        **kwargs
    )


def _qt_plugin_tree_impl(ctx):
    """Mirrors Qt's plugin directory into this package's output directory."""
    marker = "/qt5/plugins/"
    outputs = []
    for plugin in ctx.files.plugins:
        index = plugin.path.find(marker)
        if index == -1:
            fail("%s is not under a qt5/plugins directory" % plugin.path)
        output = ctx.actions.declare_file(
            "%s/%s" % (ctx.attr.name, plugin.path[index + len(marker):]),
        )
        ctx.actions.symlink(output = output, target_file = plugin)
        outputs.append(output)

    return [DefaultInfo(
        files = depset(outputs),
        runfiles = ctx.runfiles(files = outputs),
    )]

qt_plugin_tree = rule(
    implementation = _qt_plugin_tree_impl,
    doc = "Places Qt's plugins where an application next to them can find them.",
    attrs = {
        "plugins": attr.label(default = "@qt5//:plugins", allow_files = True),
    },
)

def qt_application(name, deps = [], data = [], **kwargs):
    """A cc_binary that finds Qt's plugins without Qt being installed.

    libQt5Core has the prefix of the Ubuntu package baked into it and looks for
    plugins under it, which is no help when Qt was never installed. Qt reads a
    qt.conf next to the executable before falling back to that prefix, so one is
    written there pointing at a plugin directory that is populated alongside it.

    Finding the platform plugin is not enough to load it: see @qt5//:platform_libs
    for why an application has to link its dependencies itself.

    Args:
      name: name of the binary.
      deps: as for cc_binary.
      data: as for cc_binary.
      **kwargs: forwarded to cc_binary.
    """
    qt_plugin_tree(name = name + ".plugins")

    write_file(
        name = name + ".qt_conf",
        out = "qt.conf",
        content = [
            "[Paths]",
            "Plugins = %s.plugins" % name,
            "",
        ],
    )

    cc_binary(
        name = name,
        data = data + [
            name + ".plugins",
            name + ".qt_conf",
        ],
        deps = deps + ["@qt5//:platform_libs"],
        **kwargs
    )
