// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

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

using namespace std::filesystem;

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

void dump_namespace_from_cache(const cache& c) {
  for (const database& db : c.databases()) {
    PRINT_VAR(db.get_table<Module>().size());
    PRINT_VAR(db.get_table<Module>()[0].Name());
    PRINT_VAR(db.get_table<ModuleRef>().size());
    PRINT_VAR(db.get_table<MethodDef>().size());
    PRINT_VAR(db.get_table<ImplMap>().size());

    for (const ImplMap& impl_map : db.get_table<ImplMap>()) {
      PRINT_VAR(impl_map.ImportName());
      PRINT_VAR(impl_map.ImportScope().index());
      std::string_view module_name =
          db.get_table<ModuleRef>()[impl_map.ImportScope().index()].Name();
      PRINT_VAR(module_name);
    }

    for (const ModuleRef& module_ref : db.get_table<ModuleRef>()) {
      PRINT_VAR(module_ref.Name());
    }
  }
}

// -input "C:/git/win32metadata/bin/Windows.Win32.winmd" "C:/git/win32metadata/bin/Windows.Win32.Interop.winmd" 
// -output "C:/git/cppwin32/output"
static int run(int const argc, char* argv[]) {
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

    // dump_namespace_from_cache(c);

    for (auto&& [ns, members] : c.namespaces()) {
      group.add([&, &ns = ns, &members = members] {
        write_namespace_0_h(ns, members);
        write_namespace_1_h(ns, members);
        // write_namespace_2_h(ns, members);
        write_namespace_h_no_impl(ns, members);
        write_namespace_cpp(ns, members);
      });
    }
    group.add([&c] { write_complex_structs_h(c); });
    group.add([&c] { write_complex_interfaces_h(c); });
    group.add([&c] { write_manifest_h(c); });

    std::filesystem::copy_file("C:\\git\\orbit\\third_party\\cppwin32\\cppwin32\\base.h", settings.output_folder + "win32/" + "base.h",
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

// -input "C:/git/win32metadata/bin/Windows.Win32.winmd" "C:/git/win32metadata/bin/Windows.Win32.Interop.winmd" 
// -output "C:/git/cppwin32/output"
int main(int const argc, char* argv[]) {
  char* args[] = {"blah", "-input", "C:/git/win32metadata/bin/Windows.Win32.winmd",
                        "C:/git/win32metadata/bin/Windows.Win32.Interop.winmd", "-output", "generated"};
    cppwin32::run(sizeof(args)/sizeof(args[0]), args);
}
