// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// NOTE: This file is an adaptation of Microsoft's main.cpp from
// https://github.com/microsoft/cppwin32.

#include <cppwin32/base.h>
#include <cppwin32/winmd/winmd_reader.h>

#include <cppwin32/cmd_reader.h>
#include <cppwin32/settings.h>
#include <cppwin32/task_group.h>
#include <cppwin32/text_writer.h>
#include <cppwin32/type_dependency_graph.h>
#include <cppwin32/type_writers.h>
#include <cppwin32/code_writers.h>
#include <cppwin32/file_writers.h>
#include <unordered_set>

#include <absl/flags/flag.h>
#include <absl/flags/internal/flag.h>
#include <absl/flags/parse.h>

#pragma optimize("", off)

ABSL_FLAG(std::vector<std::string>, input, {}, "Metadata files to consume");
ABSL_FLAG(std::string, output, "", "Output directory for generated code");
ABSL_FLAG(std::string, base_dir, "", "Directory containing base.h");

using namespace std::filesystem;

static const std::filesystem::path GetCppWin32Dir() {
  return std::filesystem::absolute("../../third_party/cppwin32/");
}

static const std::filesystem::path GetMetadataDir() {
  return std::filesystem::absolute("../../third_party/winmd/");
}

static const std::filesystem::path GetOutputDir() {
  return std::filesystem::absolute("../src/WindowsApiShim/generated/");
}

namespace cppwin32 {
settings_type settings;

struct usage_exception {};

static constexpr option options[]{
    {"input", 0, option::no_max, "<spec>", "Windows metadata to include in projection"},
    {"reference", 0, option::no_max, "<spec>", "Windows metadata to reference from projection"},
    {"output", 0, 1, "<path>", "Location of generated projection and component templates"},
    {"verbose", 0, 0, {}, "Show detailed progress information"},
    {"pch", 0, 1, "<name>", "Specify name of precompiled header file (defaults to pch.h)"},
    {"include", 0, option::no_max, "<prefix>", "One or more prefixes to include in input"},
    {"exclude", 0, option::no_max, "<prefix>", "One or more prefixes to exclude from input"},
    {"base", 0, 0, {}, "Generate base.h unconditionally"},
    {"help", 0, option::no_max, {}, "Show detailed help with examples"},
    {"?", 0, option::no_max, {}, {}},
    {"library", 0, 1, "<prefix>", "Specify library prefix (defaults to win32)"},
    {"filter"},          // One or more prefixes to include in input (same as -include)
    {"license", 0, 0},   // Generate license comment
    {"brackets", 0, 0},  // Use angle brackets for #includes (defaults to quotes)
};

static void print_usage(writer& w) {
  static auto printColumns = [](writer& w, std::string_view const& col1,
                                std::string_view const& col2) {
    w.write_printf("  %-20s%s\n", col1.data(), col2.data());
  };

  static auto printOption = [](writer& w, option const& opt) {
    if (opt.desc.empty()) {
      return;
    }
    printColumns(w, w.write_temp("-% %", opt.name, opt.arg), opt.desc);
  };

  auto format = R"(
C++/Win32 v%
Copyright (c) Microsoft Corporation. All rights reserved.

  cppwin32.exe [options...]

Options:

%  ^@<path>             Response file containing command line options

Where <spec> is one or more of:

  path                Path to winmd file or recursively scanned folder
)";
  w.write(format, CPPWIN32_VERSION_STRING, bind_each(printOption, options));
}

static void process_args(reader const& args) {
  settings.verbose = args.exists("verbose");
  settings.fastabi = args.exists("fastabi");

  settings.input = args.files("input", database::is_database);
  settings.reference = args.files("reference", database::is_database);

  settings.component = args.exists("component");
  settings.base = args.exists("base");

  settings.license = args.exists("license");
  settings.brackets = args.exists("brackets");

  std::filesystem::path output_folder = args.value("output");
  std::filesystem::create_directories(output_folder / "win32/impl");
  settings.output_folder = std::filesystem::canonical(output_folder).string();
  settings.output_folder += '\\';

  for (auto&& include : args.values("include")) {
    settings.include.insert(include);
  }

  for (auto&& include : args.values("filter")) {
    settings.include.insert(include);
  }

  for (auto&& exclude : args.values("exclude")) {
    settings.exclude.insert(exclude);
  }

  if (settings.component) {
    settings.component_overwrite = args.exists("overwrite");
    settings.component_name = args.value("name");

    if (settings.component_name.empty()) {
      // For compatibility with C++/WinRT 1.0, the component_name defaults to the *first*
      // input, hence the use of values() here that will return the args in input order.

      auto& values = args.values("input");

      if (!values.empty()) {
        settings.component_name = path(values[0]).filename().replace_extension().string();
      }
    }

    settings.component_pch = args.value("pch", "pch.h");
    settings.component_prefix = args.exists("prefix");
    settings.component_lib = args.value("library", "win32");
    settings.component_opt = args.exists("optimize");
    settings.component_ignore_velocity = args.exists("ignore_velocity");

    if (settings.component_pch == ".") {
      settings.component_pch.clear();
    }

    auto component = args.value("component");

    if (!component.empty()) {
      create_directories(component);
      settings.component_folder = canonical(component).string();
      settings.component_folder += '\\';
    }
  }
}

static auto get_files_to_cache() {
  std::vector<std::string> files;
  files.insert(files.end(), settings.input.begin(), settings.input.end());
  files.insert(files.end(), settings.reference.begin(), settings.reference.end());
  return files;
}

static int run(int const argc, const char* argv[]) {
  int result{};
  writer w;

  try {
    auto const start_time = std::chrono::high_resolution_clock::now();

    reader args{argc, argv, options};

    if (!args || args.exists("help") || args.exists("?")) {
      throw usage_exception{};
    }

    process_args(args);
    cache c{get_files_to_cache()};
    task_group group;

    w.flush_to_console();

    for (auto&& [ns, members] : c.namespaces()) {
      if (ns == "") {
        continue;
      }
      group.add([&, &ns = ns, &members = members] {
        write_namespace_0_h(ns, members);
        write_namespace_1_h(ns, members);
        write_namespace_h_no_impl(ns, members);
        write_namespace_cpp(ns, members);
      });
    }
    group.add([&c] { write_complex_structs_h(c); });
    group.add([&c] { write_complex_interfaces_h(c); });
    group.add([&c] { write_manifest_h(c); });

    std::filesystem::path base_h_file_name = GetCppWin32Dir() / "cppwin32" / "base.h";
    std::filesystem::copy_file(base_h_file_name, settings.output_folder + "win32/" + "base.h",
                               std::filesystem::copy_options::overwrite_existing);
  } catch (usage_exception const&) {
    print_usage(w);
  } catch (std::exception const& e) {
    w.write("cppwin32 : error %\n", e.what());
    result = 1;
  }

  w.flush_to_console(result == 0);
  return result;
}
}

int main(int const argc, char* argv[]) {
  std::string win32_metadata_file_name = (GetMetadataDir() / "Windows.Win32.winmd").string();
  std::string win32_interop_file_name = (GetMetadataDir() / "Windows.Win32.Interop.winmd").string();
  std::string output_dir = GetOutputDir().string();
  
  const char* args[] = {argv[0],
                  "-input",
                  win32_metadata_file_name.c_str(), win32_interop_file_name.c_str(),
                  "-output", output_dir.c_str()};
    cppwin32::run(sizeof(args)/sizeof(args[0]), args);
}
