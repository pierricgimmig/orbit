#!/usr/bin/env python3
"""Generate BUILD.bazel files from Orbit's CMakeLists.txt files.

The CMake build is highly regular -- every module declares one library, an
optional test binary, and lists its sources and dependencies explicitly -- so
the port is mechanical. This script does that translation once; the generated
files are checked in and edited by hand from then on. It exists so the port can
be re-derived and diffed against CMake, not as part of the build.

Usage: python3 bazel/tools/cmake2bazel.py [module ...]
"""

import argparse
import os
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]

# ---------------------------------------------------------------- CMake parsing

_COMMAND = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)\s*\(", re.M)


def strip_comments(text):
    out = []
    for line in text.splitlines():
        stripped = line.lstrip()
        if stripped.startswith("#"):
            continue
        # CMake has no inline-comment escaping to worry about in this codebase.
        hash_pos = line.find("#")
        if hash_pos >= 0 and '"' not in line[:hash_pos]:
            line = line[:hash_pos]
        out.append(line)
    return "\n".join(out)


def parse_commands(text):
    """Yields (command, [args]) in source order."""
    text = strip_comments(text)
    pos = 0
    while True:
        match = _COMMAND.search(text, pos)
        if not match:
            return
        name = match.group(1)
        depth = 1
        i = match.end()
        while i < len(text) and depth:
            if text[i] == "(":
                depth += 1
            elif text[i] == ")":
                depth -= 1
            i += 1
        body = text[match.end():i - 1]
        yield name.lower(), tokenize(body)
        pos = i


def tokenize(body):
    """Splits a CMake argument list, honouring double quotes."""
    tokens, current, in_quotes = [], "", False
    for char in body:
        if char == '"':
            in_quotes = not in_quotes
        elif char.isspace() and not in_quotes:
            if current:
                tokens.append(current)
                current = ""
        else:
            current += char
    if current:
        tokens.append(current)
    return tokens


# The configuration the generated BUILD files describe. Windows-only sources
# are emitted behind a select() where the module has any; everything else is
# evaluated for a Linux build with the GUI enabled, which is what CMakeLists.txt
# at the repository root selects.
CONDITIONS = {
    "WIN32": False,
    "MSVC": False,
    "APPLE": False,
    "UNIX": True,
    "WITH_GUI": True,
    "WITH_VULKAN": False,
    "CMAKE_CROSSCOMPILING": False,
}


def evaluate(tokens):
    """Evaluates the subset of if() expressions the CMakeLists actually use."""
    if not tokens:
        return False
    if tokens[0] == "NOT":
        return not evaluate(tokens[1:])
    if "AND" in tokens:
        index = tokens.index("AND")
        return evaluate(tokens[:index]) and evaluate(tokens[index + 1:])
    if "OR" in tokens:
        index = tokens.index("OR")
        return evaluate(tokens[:index]) or evaluate(tokens[index + 1:])
    if len(tokens) >= 3 and tokens[1] in ("STREQUAL", "MATCHES", "VERSION_GREATER_EQUAL"):
        # Compiler-version and processor probes: none of them gate sources that
        # matter for the x86-64 Linux build, and the ones that do are handled by
        # the caller inspecting `unhandled`.
        return "CMAKE_SYSTEM_PROCESSOR" in tokens[0] and "x86_64" in tokens[2]
    name = tokens[0].strip("${}")
    return CONDITIONS.get(name, False)


class Target:
    def __init__(self, name, kind):
        self.name = name
        self.kind = kind  # "library", "executable"
        self.shared = False
        self.output_name = None
        self.srcs = []
        self.win_srcs = []
        self.hdrs = []
        self.protos = []
        self.ui = []
        self.qrc = []
        self.public_includes = []
        self.private_includes = []
        self.deps = []
        self.private_deps = []
        self.defines = []
        self.copts = []
        self.automoc = False
        self.autouic = False
        self.autorcc = False
        self.is_test = False
        self.test_properties = []
        self.grpc = False


def parse_module(path):
    """Parses one CMakeLists.txt into Target objects."""
    text = path.read_text()
    targets = {}
    order = []
    unhandled = []
    stack = [True]  # if()/else() nesting; stack[-1] is "currently active"
    windows_branch = [False]

    def active():
        return all(stack)

    def target_for(name):
        if name not in targets:
            targets[name] = Target(name, "library")
            order.append(name)
        return targets[name]

    for command, args in parse_commands(text):
        if command == "if":
            branch_is_windows = args[:1] == ["WIN32"] or args[:2] == ["NOT", "WIN32"]
            windows_branch.append(args[:1] == ["WIN32"])
            stack.append(evaluate(args))
            continue
        if command == "elseif":
            windows_branch[-1] = args[:1] == ["WIN32"]
            stack[-1] = evaluate(args)
            continue
        if command == "else":
            windows_branch[-1] = not windows_branch[-1] and False
            stack[-1] = not stack[-1]
            continue
        if command == "endif":
            stack.pop()
            windows_branch.pop()
            continue

        in_windows_branch = windows_branch[-1]
        if not active():
            # Windows-only sources are still collected, so that the headers
            # they declare can be kept out of the Linux target.
            if not (in_windows_branch and
                    command in ("add_library", "add_executable", "target_sources")):
                continue

        if command in ("cmake_minimum_required", "project", "include",
                       "enable_testing", "iwyu_add_dependency", "message",
                       "find_package", "add_subdirectory", "install",
                       "add_dependencies", "file", "get_target_property",
                       "get_filename_component", "find_program", "list",
                       "string", "set"):
            continue

        if command in ("add_library", "add_executable"):
            name = args[0]
            kind = "library" if command == "add_library" else "executable"
            target = target_for(name)
            target.kind = kind
            target.shared = "SHARED" in args[1:] or "MODULE" in args[1:]
            rest = [a for a in args[1:]
                    if a not in ("STATIC", "SHARED", "OBJECT", "INTERFACE",
                                 "MODULE", "EXCLUDE_FROM_ALL")]
            add_sources(target, rest, in_windows_branch)
            continue

        if command == "target_sources":
            add_sources(target_for(args[0]),
                        [a for a in args[1:]
                         if a not in ("PUBLIC", "PRIVATE", "INTERFACE")],
                        in_windows_branch)
            continue

        if command == "target_include_directories":
            target = target_for(args[0])
            scope = "PUBLIC"
            for arg in args[1:]:
                if arg in ("PUBLIC", "PRIVATE", "INTERFACE"):
                    scope = arg
                elif arg == "SYSTEM":
                    continue
                else:
                    directory = normalize_dir(arg)
                    if directory is None:
                        unhandled.append(("include_directories", arg))
                    elif scope == "PRIVATE":
                        target.private_includes.append(directory)
                    else:
                        target.public_includes.append(directory)
            continue

        if command == "target_link_libraries":
            target = target_for(args[0])
            scope = "PUBLIC"
            for arg in args[1:]:
                if arg in ("PUBLIC", "PRIVATE", "INTERFACE"):
                    scope = arg
                elif arg.startswith("${"):
                    unhandled.append(("link", arg))
                else:
                    bucket = target.private_deps if scope == "PRIVATE" else target.deps
                    bucket.append(arg)
            continue

        if command == "target_compile_definitions":
            target = target_for(args[0])
            for arg in args[1:]:
                if arg in ("PUBLIC", "PRIVATE", "INTERFACE"):
                    continue
                target.defines.append(arg.lstrip("-D"))
            continue

        if command == "target_compile_options":
            target = target_for(args[0])
            target.copts += [a for a in args[1:]
                             if a not in ("PUBLIC", "PRIVATE", "INTERFACE")
                             and not a.startswith("${")]
            continue

        if command == "set_target_properties":
            target = target_for(args[0])
            for index, arg in enumerate(args):
                if arg in ("AUTOMOC", "AUTOUIC", "AUTORCC") and \
                        index + 1 < len(args) and args[index + 1] == "ON":
                    setattr(target, arg.lower(), True)
                if arg == "OUTPUT_NAME" and index + 1 < len(args):
                    target.output_name = args[index + 1]
            continue

        if command == "register_test":
            target = target_for(args[0])
            target.is_test = True
            if "PROPERTIES" in args:
                target.test_properties = args[args.index("PROPERTIES") + 1:]
            continue

        if command == "grpc_helper":
            target_for(args[0]).grpc = True
            continue

        if command == "protobuf_generate":
            target = target_for(args[args.index("TARGET") + 1])
            protos = args[args.index("PROTOS") + 1:]
            if "PROTOC_OUT_DIR" in protos:
                protos = protos[:protos.index("PROTOC_OUT_DIR")]
            target.protos += protos
            continue

        unhandled.append((command, " ".join(args[:3])))

    return [targets[name] for name in order], unhandled


_SOURCE_SUFFIXES = (".cpp", ".cc", ".c", ".S", ".asm")


def add_sources(target, files, windows_only):
    for name in files:
        name = name.replace("${CMAKE_CURRENT_LIST_DIR}/", "")
        name = name.replace("${CMAKE_CURRENT_SOURCE_DIR}/", "")
        if name.startswith("${"):
            continue
        if name.endswith(".proto"):
            target.protos.append(name)
        elif name.endswith(".ui"):
            target.ui.append(name)
        elif name.endswith(".qrc"):
            target.qrc.append(name)
        elif name.endswith(_SOURCE_SUFFIXES):
            (target.win_srcs if windows_only else target.srcs).append(name)
        elif name.endswith((".h", ".hpp", ".inl")):
            if not windows_only:
                target.hdrs.append(name)
        else:
            target.srcs.append(name)


def normalize_dir(raw):
    for prefix in ("${CMAKE_CURRENT_LIST_DIR}", "${CMAKE_CURRENT_SOURCE_DIR}"):
        if raw.startswith(prefix):
            remainder = raw[len(prefix):].strip("/")
            return remainder or "."
    if raw.startswith("${CMAKE_CURRENT_BINARY_DIR}"):
        return "."  # generated headers land in the package's output directory
    if not raw.startswith("${"):
        return raw.rstrip("/") or "."
    return None


# ------------------------------------------------------------ dependency table

ABSL = {
    "base": "base",
    "bind_front": "functional:bind_front",
    "flags": "flags:flag",
    "flags_parse": "flags:parse",
    "flags_usage": "flags:usage",
    "flat_hash_map": "container:flat_hash_map",
    "flat_hash_set": "container:flat_hash_set",
    "hash": "hash",
    "memory": "memory",
    "meta": "meta:type_traits",
    "span": "types:span",
    "str_format": "strings:str_format",
    "strings": "strings",
    "synchronization": "synchronization",
    "time": "time",
}

EXTERNAL = {
    "GTest::gtest": "@googletest//:gtest",
    "GTest::gmock": "@googletest//:gtest",
    "gtest": "@googletest//:gtest",
    "gmock": "@googletest//:gtest",
    "GTest_Main": "//src/Test:gtest_main",
    "GTest::Main": "//src/Test:gtest_main",
    "GTest::QtCoreMain": "//src/Test:gtest_qtcore_main",
    "GTest_QtCoreMain": "//src/Test:gtest_qtcore_main",
    "GTest::QtGuiMain": "//src/Test:gtest_qtgui_main",
    "GTest_QtGuiMain": "//src/Test:gtest_qtgui_main",
    "grpc::grpc": "@grpc//:grpc++_unsecure",
    "protobuf::protobuf": "@protobuf//:protobuf",
    "capstone::capstone": "@capstone//:capstone",
    "outcome::outcome": "@outcome//:outcome",
    "gte::gte": "//third_party/gte",
    "concurrentqueue::concurrentqueue": "//third_party/concurrentqueue",
    "xxHash::xxHash": "//third_party/xxHash-r42:xxhash",
    "LZMA::LZMA": "//third_party/lzma1900:lzma",
    "libunwindstack": "//third_party/libunwindstack",
    "libbase": "//third_party/libbase",
    "libprocinfo": "//third_party/libprocinfo",
    "liblog_static": "//third_party/liblog",
    "liblog_shared": "//third_party/liblog",
    "pyspy_ffi": "//third_party/py-spy-ffi",
    "Libssh2::libssh2": "@libssh2//:libssh2",
    "ZLIB::ZLIB": "@zlib//:zlib",
    "llvm-core::llvm-core": "//bazel/deps:llvm",
    "Qt5::Core": "@qt5//:Core",
    "Qt5::Gui": "@qt5//:Gui",
    "Qt5::Widgets": "@qt5//:Widgets",
    "Qt5::Network": "@qt5//:Network",
    "Qt5::Test": "@qt5//:Test",
    "OpenGL::GL": None,  # provided by @qt5//:Gui's linkopts
    "OpenGL::GLX": None,
    "Threads::Threads": None,  # Bazel's default link options already cover this
    "std::filesystem": None,  # part of libstdc++ since GCC 9
    "dl": None,
    "pthread": None,
}

# Library targets whose CMake name does not match their directory.
TARGET_TO_LABEL = {
    "OrbitServiceLib": "//src/Service:OrbitServiceLib",
    "OrbitClientGgpLib": "//src/OrbitClientGgp:OrbitClientGgpLib",
    "OrbitCaptureGgpClientLib": "//src/OrbitCaptureGgpClient:OrbitCaptureGgpClientLib",
    "IntegrationTestCommons": "//src/LinuxTracingIntegrationTests:IntegrationTestCommons",
}


# Dependencies Bazel needs spelled out that CMake did not: Conan exposed all of
# a package's headers behind one include directory, so a target could include
# absl/hash/hash_testing.h while only linking absl::hash.
EXTRA_DEPS = {
    "ClientDataTests": ["@abseil-cpp//absl/hash:hash_testing"],
    "OrbitBaseTests": ["@abseil-cpp//absl/hash:hash_testing"],
}

# Runtime files a target locates relative to its own executable. CMake gets
# these for free by dropping every binary in bin/ and every library in lib/;
# Bazel keeps each package's outputs in its own directory, so the ones that are
# looked up at runtime have to be declared. Labels in another package are
# copied in by COPIED_BINARIES below.
EXTRA_DATA = {
    "OrbitService": [
        ":liborbit.so",
        ":liborbituserspaceinstrumentation.so",
    ],
    "OrbitServiceIntegrationTests": [":libIntegrationTestPuppetSharedObject.so"],
    "QtUtilsTests": [":FakeCliProgram"],
    "SessionSetupTests": [":OrbitService"],
    "UserSpaceInstrumentationTests": [
        ":libUserSpaceInstrumentationTestLib.so",
        ":liborbituserspaceinstrumentation.so",
    ],
}

# Tests that wait on hard-coded timeouts and start missing them once 30-odd
# other tests are running alongside them.
FLAKY_TESTS = {
    # ConnectToLocalWidgetTest waits 500ms for a 250ms timer to have fired.
    "SessionSetupTests",
}

# copy_file rules a package needs so that EXTRA_DATA entries above resolve
# next to the binary that looks for them.
COPIED_BINARIES = {
    "SessionSetup": [("OrbitService", "//src/Service:OrbitService")],
    "Service": [
        ("liborbit.so", "//src/Api:liborbit.so"),
        ("liborbituserspaceinstrumentation.so",
         "//src/UserSpaceInstrumentation:liborbituserspaceinstrumentation.so"),
    ],
}


def dependency_label(name, package):
    if name.startswith("absl::"):
        suffix = ABSL.get(name[len("absl::"):])
        if suffix is None:
            raise SystemExit("unmapped abseil target: " + name)
        return "@abseil-cpp//absl/" + (suffix if ":" in suffix
                                       else suffix + ":" + suffix)
    if name in EXTERNAL:
        return EXTERNAL[name]
    if name in TARGET_TO_LABEL:
        return TARGET_TO_LABEL[name]
    directory = REPO / "src" / name
    if directory.is_dir():
        return "//src/" + name
    if ("//src/" + name) == package:
        return None
    raise SystemExit("unmapped dependency %r in %s" % (name, package))


# ------------------------------------------------------------------- rendering

LICENSE_HEADER = """# Copyright (c) %s The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""

# Modules whose BUILD file is written by hand because the CMake definition uses
# code generation the translator does not model.
# Modules the root CMakeLists.txt does not add on Linux.
SKIPPED = {
    # Commented out upstream (libprotobuf-mutator is not wired up).
    "FuzzingUtils",
    # Guarded by WITH_VULKAN, which the root CMakeLists.txt leaves undefined.
    "OrbitTriggerCaptureVulkanLayer",
    "OrbitVulkanLayer",
    "VulkanTutorial",
    # if(WIN32) branches.
    "WindowsCaptureService",
    "WindowsProcessLauncherService",
    "WindowsProcessService",
    "WindowsTracing",
    "WindowsUtils",
}

HAND_WRITTEN = {
    "ApiInterface",   # the C compile test needs the header as an explicit dep
    "ClientProtos",   # protobuf_generate
    "GrpcProtos",     # grpc_helper
    "Orbit",          # AUTORCC over ../../icons
    "OrbitVersion",   # GenerateVersionFile
    "Test",           # three gtest main variants
}


def render_list(name, values, indent="    ", raw=False):
    if not values:
        return ""
    quote = (lambda v: v) if raw else (lambda v: '"%s"' % v)
    values = sorted(set(values)) if not raw else values
    if len(values) == 1:
        return "%s%s = [%s],\n" % (indent, name, quote(values[0]))
    lines = "".join("%s    %s,\n" % (indent, quote(v)) for v in values)
    return "%s%s = [\n%s%s],\n" % (indent, name, lines, indent)


_QT_MACRO = re.compile(r"^\s*(Q_OBJECT|Q_GADGET|Q_NAMESPACE)\b", re.M)


def has_qt_macro(path):
    try:
        return bool(_QT_MACRO.search(path.read_text(errors="ignore")))
    except OSError:
        return False


def package_forms(directory):
    """Every Qt Designer form in the module.

    AUTOUIC finds these by scanning sources for ui_*.h includes, so CMakeLists
    only lists some of them.
    """
    return sorted(str(p.relative_to(directory)) for p in directory.rglob("*.ui"))


def package_headers(directory):
    """Every header in the module, minus test data and Windows-only sources."""
    found = []
    for path in sorted(directory.rglob("*.h")):
        relative = path.relative_to(directory)
        if relative.parts[0] in ("testdata", "resources"):
            continue
        found.append(str(relative))
    return found


def render_target(target, module_name, directory, windows_headers, extra_hdrs):
    package = "//src/" + module_name
    qt = target.automoc or target.autouic or target.autorcc

    deps = []
    for name in target.deps + target.private_deps:
        label = dependency_label(name, package)
        if label:
            deps.append(label)
    deps = sorted(set(deps + EXTRA_DEPS.get(target.name, [])))

    if qt and target.autouic:
        target.ui = sorted(set(target.ui) | set(package_forms(directory)))

    hdrs = list(target.hdrs) + extra_hdrs
    hdrs = [h for h in dict.fromkeys(hdrs) if h not in windows_headers]
    moc_hdrs = [h for h in hdrs if qt and has_qt_macro(directory / h)]
    plain_hdrs = [h for h in hdrs if h not in moc_hdrs]

    includes = list(dict.fromkeys(target.public_includes))
    private_includes = [d for d in dict.fromkeys(target.private_includes)
                        if d not in includes]
    if target.ui and "." not in includes:
        includes.append(".")

    copts = ["ORBIT_COPTS"]
    if target.kind == "executable":
        # Binaries have no dependents, so there is nothing to keep private.
        includes += private_includes
        private_includes = []

    is_test = target.is_test
    needs_qt_runtime = any(
        d.startswith("@qt5//") or d.endswith("gtest_qtgui_main") or
        d.endswith("gtest_qtcore_main")
        for d in deps
    )
    if target.kind == "executable":
        rule = ("qt_cc_test" if needs_qt_runtime else "cc_test") if is_test \
            else "cc_binary"
    elif target.shared:
        # CMake's SHARED. cc_binary(linkshared) is how Bazel produces a .so
        # under a name of our choosing, with the static deps linked in.
        rule = "cc_binary"
    else:
        rule = "qt_cc_library" if qt else "cc_library"

    if rule in ("cc_binary", "cc_test", "qt_cc_test") or target.shared:
        # Binaries have no hdrs attribute; their headers are just sources.
        target.srcs += plain_hdrs + moc_hdrs
        plain_hdrs, moc_hdrs = [], []

    name = target.name
    if target.shared:
        name = "lib%s.so" % (target.output_name or target.name)

    lines = ["%s(\n" % rule, '    name = "%s",\n' % name]
    lines.append(render_list("srcs", target.srcs))
    lines.append(render_list("hdrs", plain_hdrs))
    if rule == "qt_cc_library":
        lines.append(render_list("moc_hdrs", moc_hdrs))
        lines.append(render_list("ui", target.ui))
    if private_includes:
        # CMake's PRIVATE include directories: -I rather than `includes`, so
        # that they are not propagated to dependents.
        lines.append("    copts = ORBIT_COPTS + [\n")
        for directory in private_includes:
            path = "src/%s" % module_name if directory == "." \
                else "src/%s/%s" % (module_name, directory)
            lines.append('        "-I%s",\n' % path)
        lines.append("    ],\n")
    else:
        lines.append("    copts = ORBIT_COPTS,\n")
    data = EXTRA_DATA.get(target.name, [])
    if is_test and (directory / "testdata").is_dir():
        lines.append('    data = glob(["testdata/**"])%s,\n'
                     % ("".join(' + ["%s"]' % d for d in data)))
    elif data:
        lines.append(render_list("data", data))
    lines.append(render_list("defines", target.defines))
    if is_test and (directory / "testdata").is_dir():
        lines.append('    env = {"ORBIT_OVERRIDE_TESTDATA_PATH": "src/%s/testdata"},\n'
                     % module_name)
    if target.name in FLAKY_TESTS:
        lines.append("    flaky = True,\n")
    if rule == "qt_cc_library":
        lines.append(render_list("include_dirs", includes or ["."]))
    else:
        lines.append(render_list("includes", includes))
    if target.shared:
        lines.append("    linkshared = True,\n")
        lines.append("    linkstatic = True,\n")
    lines.append(render_list("deps", deps))
    lines.append(")\n")
    return "".join(part for part in lines if part)


def render_module(module_name, targets, directory):
    windows_headers = set()
    for target in targets:
        windows_headers.update(target.win_srcs)

    declared = set()
    for target in targets:
        declared.update(target.hdrs)
        declared.update(target.srcs)

    # CMake does not need private headers listed; Bazel does.
    primary = next((t for t in targets if t.kind == "library"), None)
    extra = [h for h in package_headers(directory)
             if h not in declared and h not in windows_headers]
    if primary is None and extra:
        primary = targets[0]

    body = []
    for target in targets:
        body.append(render_target(
            target, module_name, directory, windows_headers,
            extra if target is primary else []))

    copied = COPIED_BINARIES.get(module_name, [])
    if copied:
        body.insert(0, "\n".join(
            'copy_file(\n'
            '    name = "copy_%s",\n'
            '    src = "%s",\n'
            '    out = "%s",\n'
            '    is_executable = True,\n'
            ')\n' % (name.replace(".", "_"), label, name)
            for name, label in copied).rstrip("\n") + "\n")

    text = "".join(body)
    rules = sorted({r for r in ("cc_binary", "cc_library", "cc_test")
                    if re.search(r"^%s\(" % r, text, re.M)})

    loads = ['load("@rules_cc//cc:defs.bzl", %s)\n'
             % ", ".join('"%s"' % r for r in rules)] if rules else []
    qt_rules = sorted({r for r in ("qt_cc_library", "qt_cc_test")
                       if re.search(r"^%s\(" % r, text, re.M)})
    if qt_rules:
        loads.append('load("//bazel/qt:rules.bzl", %s)\n'
                     % ", ".join('"%s"' % r for r in qt_rules))
    if "copy_file(" in text:
        loads.append('load("@bazel_skylib//rules:copy_file.bzl", "copy_file")\n')
    loads.append('load("//bazel:copts.bzl", "ORBIT_COPTS")\n')

    return "%s\n%s\n%s\n\n%s" % (
        LICENSE_HEADER % 2020,
        "".join(sorted(loads)),
        'package(default_visibility = ["//visibility:public"])',
        "\n".join(body).rstrip() + "\n",
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("modules", nargs="*")
    parser.add_argument("--report", action="store_true",
                        help="only print constructs the translator skipped")
    parser.add_argument("--stdout", action="store_true")
    options = parser.parse_args()

    modules = options.modules or sorted(
        p.name for p in (REPO / "src").iterdir()
        if (p / "CMakeLists.txt").exists() and p.name not in SKIPPED)

    for module_name in modules:
        directory = REPO / "src" / module_name
        targets, unhandled = parse_module(directory / "CMakeLists.txt")
        if options.report:
            if unhandled:
                print("%s:" % module_name)
                for command, detail in unhandled:
                    print("    %-28s %s" % (command, detail))
            continue
        if not options.modules and module_name in HAND_WRITTEN | SKIPPED:
            continue
        text = render_module(module_name, targets, directory)
        if options.stdout:
            print("=== src/%s/BUILD.bazel ===" % module_name)
            print(text)
        else:
            (directory / "BUILD.bazel").write_text(text)
            print("wrote src/%s/BUILD.bazel" % module_name)

    return 0


if __name__ == "__main__":
    sys.exit(main())
