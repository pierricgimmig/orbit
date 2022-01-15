// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "FileWriter.h"

#include <absl/strings/ascii.h>
#include <absl/strings/match.h>
#include <absl/strings/str_format.h>
#include <cppwin32/code_writers.h>
#include <cppwin32/file_writers.h>
#include <cppwin32/settings.h>
#include <cppwin32/task_group.h>
#include <cppwin32/text_writer.h>
#include <cppwin32/type_dependency_graph.h>
#include <cppwin32/type_writers.h>

#include "OrbitBase/Logging.h"

using cppwin32::method_signature;
using cppwin32::task_group;
using winmd::reader::cache;
using winmd::reader::CustomAttribute;
using winmd::reader::CustomAttributeSig;
using winmd::reader::database;
using winmd::reader::ElemSig;
using winmd::reader::FixedArgSig;
using winmd::reader::ImplMap;
using winmd::reader::MethodDef;
using winmd::reader::ModuleRef;
using winmd::reader::TypeDef;

namespace cppwin32 {
settings_type settings;
}

namespace orbit_windows_api_shim {

namespace {

/*
internal enum Architecture
{
    None = 0,
    X86 = 1,
    X64 = 2,
    Arm64 = 4,
    All = Architecture.X64 | Architecture.X86 | Architecture.Arm64,
}
*/

bool is_x64_arch(const MethodDef& method) {
  const CustomAttribute attr =
      get_attribute(method, "Windows.Win32.Interop", "SupportedArchitectureAttribute");
  if (attr) {
    CustomAttributeSig attr_sig = attr.Value();
    const FixedArgSig& fixed_arg = attr_sig.FixedArgs()[0];
    const ElemSig& elem_sig = std::get<ElemSig>(fixed_arg.value);
    const ElemSig::EnumValue& enum_value = std::get<ElemSig::EnumValue>(elem_sig.value);
    int32_t const arch_flags = std::get<int32_t>(enum_value.value);
    // None = 0, X86 = 1, X64 = 2, Arm64 = 4.
    constexpr const int32_t kX64 = 2;
    if ((arch_flags & kX64) == 0) return false;
  }
  return true;
}

template <typename T>
bool IsListEmpty(const T& list) {
  return list.first == list.second;
}

void write_class_method_with_orbit_instrumentation(cppwin32::writer& w,
                                                   method_signature const& method_signature,
                                                   const MetaDataHelper& metadata_helper) {
  auto const format = R"(    % __stdcall ORBIT_IMPL_%(%) noexcept
    {
        %
        %g_api_table.%(%);
        %%
    }
)";
  w.write(format, cppwin32::bind<cppwin32::write_method_return>(method_signature),
          metadata_helper.GetFunctionNameFromMethodDef(method_signature.method()),
          cppwin32::bind<cppwin32::write_method_params>(method_signature),
          cppwin32::bind<cppwin32::write_orbit_instrumentation>(method_signature),
          cppwin32::bind<cppwin32::write_consume_return_type>(method_signature),
          metadata_helper.GetFunctionNameFromMethodDef(method_signature.method()),
          cppwin32::bind<cppwin32::write_method_args>(method_signature),
          cppwin32::bind<cppwin32::write_orbit_instrumentation_ret>(method_signature),
          cppwin32::bind<cppwin32::write_consume_return_statement>(method_signature));
}

void write_class_impl(cppwin32::writer& w, TypeDef const& type,
                      const MetaDataHelper& metadata_helper) {
  auto abi_guard = w.push_abi_types(true);
  auto ns_guard = w.push_full_namespace(true);

  w.write("extern \"C\"\n{\n");

  for (auto&& method : type.MethodList()) {
    if (!is_x64_arch(method)) continue;
    method_signature signature{method};
    write_class_method_with_orbit_instrumentation(w, signature, metadata_helper);
    w.write("\n");
  }
  w.write("}\n\n");
}

void write_class_api_table(cppwin32::writer& w, TypeDef const& type,
                           const MetaDataHelper& metadata_helper) {
  auto const format = R"xyz(    % (__stdcall *%)(%) noexcept;
)xyz";

  if (!IsListEmpty(type.MethodList())) {
    w.write("\nstruct ApiTable {\n");
    for (auto&& method : type.MethodList()) {
      if (!is_x64_arch(method)) continue;

      if (method.Flags().Access() == winmd::reader::MemberAccess::Public) {
        method_signature signature{method};
        w.write(format, cppwin32::bind<cppwin32::write_abi_return>(signature.return_signature()),
                metadata_helper.GetFunctionNameFromMethodDef(method),
                cppwin32::bind<cppwin32::write_abi_params>(signature));
      }
    }
    w.write("};\nextern ApiTable g_api_table;\n\n");
  }
}

void write_class_abi(cppwin32::writer& w, TypeDef const& type,
                     const MetaDataHelper& metadata_helper) {
  auto abi_guard = w.push_abi_types(true);
  auto ns_guard = w.push_full_namespace(true);

  if (!IsListEmpty(type.MethodList())) {
    w.write(R"(extern "C"
{
)");
    auto const format = R"xyz(    % __stdcall ORBIT_IMPL_%(%) noexcept;
)xyz";

    for (const winmd::reader::MethodDef& method : type.MethodList()) {
      if (method.Flags().Access() == winmd::reader::MemberAccess::Public && is_x64_arch(method)) {
        method_signature signature{method};
        std::string function_name = metadata_helper.GetFunctionNameFromMethodDef(method);
        w.write(format, cppwin32::bind<cppwin32::write_abi_return>(signature.return_signature()),
                function_name, cppwin32::bind<cppwin32::write_abi_params>(signature));
      }
    }
    w.write(R"(}
)");

    write_class_api_table(w, type, metadata_helper);

    w.write("\n");
  }

  w.write(
      "  bool GetOrbitShimFunctionInfo(const char* function_key, OrbitShimFunctionInfo& "
      "out_function_info);\n");
}

void WriteNamespaceGetOrbitShimFunctionInfo(cppwin32::writer& w, TypeDef const& type,
                                            const MetaDataHelper& metadata_helper) {
  w.write(
      "\nbool GetOrbitShimFunctionInfo(const char* function_key, OrbitShimFunctionInfo& "
      "out_function_info)\n{\n");

  if (!IsListEmpty(type.MethodList())) {
    w.write(
        "  static std::unordered_map<std::string, std::function<void(OrbitShimFunctionInfo&)>> "
        "function_dispatcher = {\n");

    for (auto&& method : type.MethodList()) {
      if (!is_x64_arch(method)) continue;
      w.write("    ORBIT_DISPATCH_ENTRY(%),\n",
              metadata_helper.GetFunctionNameFromMethodDef(method));
    }

    w.write("  };\n\n");
    w.write("  auto it = function_dispatcher.find(function_key);\n");
    w.write("  if(it != function_dispatcher.end()) {\n");
    w.write("    it->second(out_function_info);\n    return true;\n  }\n\n");
  }

  w.write("  ORBIT_ERROR(\"Could not find function \"%s\" in current namespace\", function_key);\n");
  w.write("  return false;\n}\n\n");
}

const database* FindWin32Database(const cache& cache) {
  constexpr const char* kWin32MetaDataFileName = "Windows.Win32.winmd";
  for (const database& db : cache.databases()) {
    std::filesystem::path path(db.path());
    if (path.filename() == kWin32MetaDataFileName) {
      return &db;
    }
  }

  ERROR("Could not find win32 metadata database \"%s\"", kWin32MetaDataFileName);
  return nullptr;
}

inline const char* GetWindowsApiFunctionDefinition() {
  return R"(struct WindowsApiFunction {
  const char* function_key = nullptr;
  const char* name_space = nullptr;
};

)";
}

struct FunctionInfo {
  std::string name;
  std::string module;
  std::string name_space;
};

static void write_manifest_h(cache const& c) {
  cppwin32::writer w;

  w.write("#pragma once\n\n");
  w.write("#include <array>\n\n");
  w.write(GetWindowsApiFunctionDefinition());

  std::map<const MethodDef, std::string> method_def_to_namespace;

  for (auto&& [ns, members] : c.namespaces()) {
    for (const TypeDef& c : members.classes) {
      for (const MethodDef& method : c.MethodList()) {
        if (method.Flags().Access() == winmd::reader::MemberAccess::Public) {
          method_def_to_namespace.emplace(method, std::string(ns));
          // check emplace took place
        }
      }
    }
  }

  for (const database& db : c.databases()) {
    std::filesystem::path path(db.path());
    if (path.filename() != "Windows.Win32.winmd") continue;

    std::map<std::string, FunctionInfo> function_key_to_info;
    for (const ImplMap& impl_map : db.get_table<ImplMap>()) {
      std::string function_name(impl_map.ImportName());
      ModuleRef module_ref = db.get_table<ModuleRef>()[impl_map.ImportScope().index()];
      std::string module_name = absl::AsciiStrToLower(module_ref.Name());
      std::string function_key = module_name + "_" + function_name;

      const MethodDef& method_def = db.get_table<MethodDef>()[impl_map.MemberForwarded().index()];

      FunctionInfo& function_info = function_key_to_info[function_key];
      // Check insertion took place.
      function_info.name = function_name;
      function_info.module = module_name;
      function_info.name_space = method_def_to_namespace.at(method_def);
    }

    w.write("// num namespaces: %\n", c.namespaces().size());
    w.write("// num modules: %\n\n", db.get_table<ModuleRef>().size());
    w.write("constexpr std::array<const WindowsApiFunction, %> kWindowsApiFunctions = {{\n",
            db.get_table<ImplMap>().size());
    for (const auto& [key, function_info] : function_key_to_info) {
      w.write("  {\"%__%\", \"%\"},\n", function_info.module, function_info.name,
              function_info.name_space);
    }

    w.write("}};\n\n");
  }

  w.flush_to_file(cppwin32::settings.output_folder + "win32/manifest.h");
}

constexpr const char* namespace_dispatcher_header =
    R"(// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "NamespaceDispatcher.h"
)";

constexpr const char* namespace_dispatcher_0 = R"(

// NOTE: This file was generated by Orbit's WindowsApiShimGenerator.

namespace orbit_windows_api_shim {

  bool GetOrbitShimFunctionInfo(const char* function_key,
                                OrbitShimFunctionInfo& out_function_info) {
    std::optional<std::string> name_space = WindowsApiHelper::Get().GetNamespaceFromFunctionKey(function_key);
    if (!name_space.has_value()) return false;
    typedef std::function<bool(const char* function_key,
                                 OrbitShimFunctionInfo& out_function_info)>
          FunctionType;

      static const std::unordered_map<std::string, FunctionType> dispatcher = {{
          // NAMESPACE_DISPATCH_ENTRIES
)";

constexpr const char* namespace_dispatcher_1 = R"(
      }};

      const auto function_it = dispatcher.find("win32." + name_space.value());
      if (function_it == dispatcher.end()) return false;
      return function_it->second(function_key, out_function_info);
    }
}  // namespace orbit_windows_api_shim
)";

void write_namespace_dispatch_cpp(cache const& c) {
  cppwin32::writer w;
  w.write(namespace_dispatcher_header);

  // Include all namespace headers.
  for (auto&& [ns, members] : c.namespaces()) {
    if (ns == "") continue;
    w.write("#include \"win32/%.h\"\n", ns);
  }

  w.write(namespace_dispatcher_0);

  // function_to_namespace
  for (auto&& [ns, members] : c.namespaces()) {
    if (ns == "") continue;
    if (members.classes.size() == 0) continue;
    std::string name_space = absl::StrReplaceAll(ns, {{".", "::"}});
    w.write("          ADD_NAMESPACE_DISPATCH_ENTRY(win32::%),\n", name_space);
  }

  w.write(namespace_dispatcher_1);

  w.flush_to_file(cppwin32::settings.output_folder + "win32/impl/NamespaceDispatcher.cpp");
}

}  // namespace

MetaDataHelper::MetaDataHelper(const database& db) {
  // Populate a map of method_def to module_ref.
  for (const ImplMap& impl_map : db.get_table<ImplMap>()) {
    ModuleRef module_ref = db.get_table<ModuleRef>()[impl_map.ImportScope().index()];
    const MethodDef& method_def = db.get_table<MethodDef>()[impl_map.MemberForwarded().index()];
    method_def_to_module_ref_map_.emplace(method_def, module_ref);
  }
}

std::string MetaDataHelper::GetFunctionNameFromMethodDef(
    const winmd::reader::MethodDef& method_def) const {
  // Include the module name so that all function names are globally unique.

  std::string module_name =
      absl::StrReplaceAll(GetModuleNameFromMethodDef(method_def), {{".", "_"}, {"-", "_"}});

  std::string_view function_name = method_def.Name();
  return absl::StrFormat("%s__%s", absl::AsciiStrToLower(module_name), function_name);
}

std::string MetaDataHelper::GetModuleNameFromMethodDef(
    const winmd::reader::MethodDef& method_def) const {
  return std::string(method_def_to_module_ref_map_.at(method_def).Name());
}

FileWriter::FileWriter(std::vector<std::filesystem::path> input, std::filesystem::path output_dir) {
  std::filesystem::create_directories(output_dir / "win32/impl");

  std::vector<std::string> input_files;
  cppwin32::settings.output_folder = output_dir.string();
  for (const auto& path : input) {
    input_files.emplace_back(path.string());
  }
  cache_ = std::make_unique<winmd::reader::cache>(input_files);
  win32_database_ = FindWin32Database(*cache_);
  win32_metadata_helper_ = std::make_unique<MetaDataHelper>(*win32_database_);
}

bool ShouldSkipNamespace(std::string_view name_space) {
  std::vector<std::string> tokens = {"Direct3D",  "Graphics", "Foundation", "Media", "Security", "System.Com", "System.PropertiesSystem", "UI", "Win32"};

  for (const std::string& token : tokens) {
    if (absl::StrContains(name_space, token)) {
      return false;
    }
  }

  if (name_space == "") {
    return true;
  }

  return true;
}

static void write_complex_structs_h(cache const& c) {
  cppwin32::writer w;

  cppwin32::type_dependency_graph graph;
  for (auto&& [ns, members] : c.namespaces()) {
    if (ShouldSkipNamespace(ns)) continue;
    for (auto&& s : members.structs) {
      if (cppwin32::is_x64_struct(s)) graph.add_struct(s);
    }
  }

  graph.walk_graph([&](TypeDef const& type) {
    if (!is_nested(type)) {
      auto guard = wrap_type_namespace(w, type.TypeNamespace());
      write_struct(w, type);
    }
  });

  write_close_file_guard(w);
  w.swap();

  write_preamble(w);
  write_open_file_guard(w, "complex_structs");

  for (auto&& depends : w.depends) {
    w.write_depends(depends.first, '0');
  }

  w.flush_to_file(cppwin32::settings.output_folder + "win32/impl/complex_structs.h");
}

static void write_complex_interfaces_h(cache const& c) {
  cppwin32::writer w;

  cppwin32::type_dependency_graph graph;
  for (auto&& [ns, members] : c.namespaces()) {
    if (ShouldSkipNamespace(ns)) continue;
    for (auto&& s : members.interfaces) {
      graph.add_interface(s);
    }
  }

  graph.walk_graph([&](TypeDef const& type) {
    if (!is_nested(type)) {
      auto guard = wrap_type_namespace(w, type.TypeNamespace());
      write_interface(w, type);
    }
  });

  write_close_file_guard(w);
  w.swap();

  write_preamble(w);
  write_open_file_guard(w, "complex_interfaces");

  for (auto&& depends : w.depends) {
    w.write_depends(depends.first, '1');
  }
  // Workaround for https://github.com/microsoft/cppwin32/issues/2
  for (auto&& extern_depends : w.extern_depends) {
    auto guard = wrap_type_namespace(w, extern_depends.first);
    w.write_each<cppwin32::write_extern_forward>(extern_depends.second);
  }

  w.flush_to_file(cppwin32::settings.output_folder + "win32/impl/complex_interfaces.h");
}


void FileWriter::WriteCodeFiles() {
  task_group group;
  for (auto&& [ns, members] : cache_->namespaces()) {
    if (ShouldSkipNamespace(ns)) continue;

    group.add([&, &ns = ns, &members = members] {
      cppwin32::write_namespace_0_h(ns, members);
      cppwin32::write_namespace_1_h(ns, members);
      WriteNamespaceHeader(ns, members);
      WriteNamespaceCpp(ns, members);
    });
  }

  group.add([this] { write_complex_structs_h(*cache_); });
  group.add([this] { write_complex_interfaces_h(*cache_); });
  group.add([this] { write_namespace_dispatch_cpp(*cache_); });
  group.add([this] { write_manifest_h(*cache_); });
}

static void WriteIncludes(cppwin32::writer& w) {
  w.write_root_include("base");
  w.write("#include \"win32/impl/complex_structs.h\"\n");
  w.write("#include \"win32/impl/complex_interfaces.h\"\n");
  w.write("#include \"WindowsApiShimUtils.h\"\n");
}

void FileWriter::WriteNamespaceHeader(std::string_view const& ns,
                                      cache::namespace_members const& members) {
  cppwin32::writer w;
  w.type_namespace = ns;

  if (!members.classes.empty()) {
    auto wrap = wrap_type_namespace(w, ns);
    w.write_each<write_class_abi>(members.classes, *win32_metadata_helper_);
  }

  write_close_file_guard(w);
  w.swap();
  write_preamble(w);
  write_open_file_guard(w, ns);
  WriteIncludes(w);
  w.write_depends(w.type_namespace, '0');

  // Workaround for https://github.com/microsoft/cppwin32/issues/2
  for (auto&& extern_depends : w.extern_depends) {
    auto guard = wrap_type_namespace(w, extern_depends.first);
    w.write_each<cppwin32::write_extern_forward>(extern_depends.second);
  }
  w.save_header();
}

bool HasMethods(const std::vector<winmd::reader::TypeDef>& classes) {
  for (const auto& type : classes) {
    if (!IsListEmpty(type.MethodList())) return true;
  }
  return false;
}

void FileWriter::WriteNamespaceCpp(std::string_view const& ns,
                                   cache::namespace_members const& members) {
  cppwin32::writer w;
  w.type_namespace = ns;

  {
    auto wrap = wrap_type_namespace(w, ns);

    if (HasMethods(members.classes)) {
      w.write("ApiTable g_api_table;\n\n");
      w.write("#pragma region abi_methods\n");
      w.write_each<write_class_impl>(members.classes, *win32_metadata_helper_);
      w.write("#pragma endregion abi_methods\n\n");
    }

    // HookFunction dispatch.
    w.write_each<WriteNamespaceGetOrbitShimFunctionInfo>(members.classes, *win32_metadata_helper_);
  }

  write_close_file_guard(w);
  w.swap();
  write_preamble(w);
  write_open_file_guard(w, ns, '2');

  w.write_depends(w.type_namespace);
  w.write_depends(w.type_namespace, '1');
  w.write_depends("manifest");
  w.write("\n#include <functional>\n");
  w.write("\n#include <unordered_map>\n");
  w.write("\n");
  // Workaround for https://github.com/microsoft/cppwin32/issues/2
  for (auto&& extern_depends : w.extern_depends) {
    auto guard = wrap_type_namespace(w, extern_depends.first);
    w.write_each<cppwin32::write_extern_forward>(extern_depends.second);
  }

  w.save_cpp();
}

}  // namespace orbit_windows_api_shim
