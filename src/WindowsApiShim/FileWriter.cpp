#include "FileWriter.h"

#include <cppwin32/code_writers.h>
#include <cppwin32/file_writers.h>
//#include <cppwin32/settings.h>
#include <cppwin32/task_group.h>
#include <cppwin32/text_writer.h>
#include <cppwin32/type_dependency_graph.h>
#include <cppwin32/type_writers.h>

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

// Dummy global to fix linking error.
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
    method_signature signature{method};
    constexpr const int32_t kX64 = 2;
    if ((arch_flags & kX64) == 0) return false;
  }
  return true;
}

std::map<MethodDef, ModuleRef> GetMethodDeftoModuleRefMap(const database& db) {
  std::map<MethodDef, ModuleRef> method_to_module_map;
  for (const ImplMap& impl_map : db.get_table<ImplMap>()) {
    ModuleRef module_ref = db.get_table<ModuleRef>()[impl_map.ImportScope().index()];
    const MethodDef& method_def = db.get_table<MethodDef>()[impl_map.MemberForwarded().index()];
    method_to_module_map.emplace(method_def, module_ref);
  }
  return method_to_module_map;
}

void write_class_method_with_orbit_instrumentation(cppwin32::writer& w,
                                                   method_signature const& method_signature) {
  auto const format = R"(    % __stdcall ORBIT_IMPL_%(%) noexcept
    {
        %
        %g_api_table.%(%);
        %%
    }
)";
  w.write(format, cppwin32::bind<cppwin32::write_method_return>(method_signature),
          method_signature.method().Name(),
          cppwin32::bind<cppwin32::write_method_params>(method_signature),
          cppwin32::bind<cppwin32::write_orbit_instrumentation>(method_signature),
          cppwin32::bind<cppwin32::write_consume_return_type>(method_signature),
          method_signature.method().Name(),
          cppwin32::bind<cppwin32::write_method_args>(method_signature),
          cppwin32::bind<cppwin32::write_orbit_instrumentation_ret>(method_signature),
          cppwin32::bind<cppwin32::write_consume_return_statement>(method_signature));
}

// Orbit
void write_class_impl(cppwin32::writer& w, TypeDef const& type) {
  auto abi_guard = w.push_abi_types(true);
  auto ns_guard = w.push_full_namespace(true);

  w.write(R"(extern "C"
{
)");
  //        auto const format = R"xyz(% __stdcall ORBIT_IMPL_%(%) noexcept {
  //)xyz";

  for (auto&& method : type.MethodList()) {
    if (!is_x64_arch(method)) continue;
    method_signature signature{method};
    write_class_method_with_orbit_instrumentation(w, signature);
    w.write("\n");
  }
  w.write(R"(}
)");
  w.write("\n");
}

void write_class_api_table(cppwin32::writer& w, TypeDef const& type) {
  auto const format = R"xyz(    % (__stdcall *%)(%) noexcept;
)xyz";
  w.write("\nstruct ApiTable {\n");
  for (auto&& method : type.MethodList()) {
    if (!is_x64_arch(method)) continue;

    auto generic_param = method.GenericParam();
    for (auto i = generic_param.first; i != generic_param.second; ++i) {
      PRINT_VAR(i.Name());
      // PRINT_VAR(i.TypeNamespaceAndName().second);
    }

    if (method.Flags().Access() == winmd::reader::MemberAccess::Public) {
      method_signature signature{method};
      w.write(format, cppwin32::bind<cppwin32::write_abi_return>(signature.return_signature()),
              method.Name(), cppwin32::bind<cppwin32::write_abi_params>(signature));
    }
  }
  w.write("};\nextern ApiTable g_api_table;\n\n");
  w.write(
      "bool GetOrbitShimFunctionInfo(const char* function_key, OrbitShimFunctionInfo& "
      "out_function_info);\n");
}

void write_class_abi(cppwin32::writer& w, TypeDef const& type) {
  auto abi_guard = w.push_abi_types(true);
  auto ns_guard = w.push_full_namespace(true);

  w.write(R"(extern "C"
{
)");
  auto const format = R"xyz(    % __stdcall ORBIT_IMPL_%(%) noexcept;
)xyz";

  for (auto&& method : type.MethodList()) {
    if (method.Flags().Access() == winmd::reader::MemberAccess::Public && is_x64_arch(method)) {
      method_signature signature{method};
      w.write(format, cppwin32::bind<cppwin32::write_abi_return>(signature.return_signature()),
              method.Name(), cppwin32::bind<cppwin32::write_abi_params>(signature));
    }
  }
  w.write(R"(}
)");

  write_class_api_table(w, type);
  w.write("\n");
}

void WriteNamespaceGetOrbitShimFunctionInfo(cppwin32::writer& w, TypeDef const& type) {
  w.write(
      "\nbool GetOrbitShimFunctionInfo(const char* function_key, OrbitShimFunctionInfo& "
      "out_function_info)\n{\n");
  w.write(
      "  static std::unordered_map<std::string, std::function<void(OrbitShimFunctionInfo&)>> "
      "function_dispatcher = {\n");
  for (auto&& method : type.MethodList()) {
    if (!is_x64_arch(method)) continue;
    w.write("    ORBIT_DISPATCH_ENTRY(%),\n", method.Name());
  }
  w.write("  };\n\n");

  w.write("  auto it = function_dispatcher.find(function_key);\n");
  w.write("  if(it != function_dispatcher.end()) {\n");
  w.write("    it->second(out_function_info);\n    return true;\n  }\n\n");
  w.write("  ORBIT_ERROR(\"Could not find function %s in current namespace\", function_key);\n");
  w.write("  return false;\n}\n\n");
}

}  // namespace

FileWriter::FileWriter(std::vector<std::filesystem::path> input, std::filesystem::path output_dir) {
  std::filesystem::create_directories(output_dir / "win32/impl");
  win32_database_ = GetWin32Database();
  method_def_to_module_ref_map = GetMethodDeftoModuleRefMap(*win32_database_);
}

void FileWriter::WriteCodeFiles() {
  task_group group;
  for (auto&& [ns, members] : cache_->namespaces()) {
    if (ns == "") {
      continue;
    }
    group.add([&, &ns = ns, &members = members] {
      cppwin32::write_namespace_0_h(ns, members);
      cppwin32::write_namespace_1_h(ns, members);
      WriteNamespaceHeader(ns, members);
      WriteNamespaceCpp(ns, members);
    });
  }

  group.add([this] { cppwin32::write_complex_structs_h(*cache_); });
  group.add([this] { cppwin32::write_complex_interfaces_h(*cache_); });
  group.add([this] { cppwin32::write_manifest_h(*cache_); });
  group.add([this] { cppwin32::write_namespace_dispatch_cpp(*cache_); });
}

const database* FileWriter::GetWin32Database() {
  for (const database& db : cache_->databases()) {
    std::filesystem::path path(db.path());
    if (path.filename() == "Windows.Win32.winmd") {
      return &db;
    }
  }
  return nullptr;
}

void FileWriter::WriteNamespaceHeader(std::string_view const& ns,
                                      cache::namespace_members const& members) {
  cppwin32::writer w;
  w.type_namespace = ns;
  {
    auto wrap = wrap_type_namespace(w, ns);
    w.write("#pragma region methods\n");
    w.write_each<cppwin32::write_class_abi>(members.classes);
    w.write("#pragma endregion methods\n\n");
  }

  write_close_file_guard(w);
  w.swap();
  write_preamble(w);
  write_open_file_guard(w, ns);
  write_version_assert(w);
  w.write_depends(w.type_namespace, '0');

  // Workaround for https://github.com/microsoft/cppwin32/issues/2
  for (auto&& extern_depends : w.extern_depends) {
    auto guard = wrap_type_namespace(w, extern_depends.first);
    w.write_each<cppwin32::write_extern_forward>(extern_depends.second);
  }
  w.save_header();
}

void FileWriter::WriteNamespaceCpp(std::string_view const& ns,
                                   cache::namespace_members const& members) {
  cppwin32::writer w;
  w.type_namespace = ns;

  {
    auto wrap = wrap_type_namespace(w, ns);

    if (!members.classes.empty()) w.write("ApiTable g_api_table;\n\n");
    w.write("#pragma region abi_methods\n");
    w.write_each<write_class_impl>(members.classes);
    w.write("#pragma endregion abi_methods\n\n");

    // HookFunction dispatch.
    w.write_each<WriteNamespaceGetOrbitShimFunctionInfo>(members.classes);
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
