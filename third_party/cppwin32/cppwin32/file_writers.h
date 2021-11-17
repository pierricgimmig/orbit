#pragma once

namespace cppwin32
{
    static void write_namespace_0_h(std::string_view const& ns, cache::namespace_members const& members)
    {
        writer w;
        w.type_namespace = ns;

  {
    auto wrap = wrap_type_namespace(w, ns);

    w.write("#pragma region enums\n");
    w.write_each<write_enum>(members.enums);
    w.write("#pragma endregion enums\n\n");

    w.write("#pragma region forward_declarations\n");
    w.write_each<write_forward>(members.structs);
    w.write_each<write_forward>(members.interfaces);
    w.write("#pragma endregion forward_declarations\n\n");

    w.write("#pragma region delegates\n");
    write_delegates(w, members.delegates);
    w.write("#pragma endregion delegates\n\n");
  }
  {
    auto wrap = wrap_impl_namespace(w);

    w.write("#pragma region guids\n");
    w.write_each<write_guid>(members.interfaces);
    w.write("#pragma endregion guids\n\n");
  }

  write_close_file_guard(w);
  w.swap();

  write_preamble(w);
  write_open_file_guard(w, ns, '0');

        for (auto&& depends : w.depends)
        {
            auto guard = wrap_type_namespace(w, depends.first);
            w.write_each<write_forward>(depends.second);
        }

  w.save_header('0');
}

    static void write_namespace_1_h(std::string_view const& ns, cache::namespace_members const& members)
    {
        writer w;
        w.type_namespace = ns;
        
        w.write("#include \"win32/impl/complex_structs.h\"\n");

  {
    auto wrap = wrap_type_namespace(w, ns);

    w.write("#pragma region interfaces\n");
    // write_interfaces(w, members.interfaces);
    w.write("#pragma endregion interfaces\n\n");
  }

  write_close_file_guard(w);
  w.swap();
  write_preamble(w);
  write_open_file_guard(w, ns, '1');

        for (auto&& depends : w.depends)
        {
            w.write_depends(depends.first, '0');
        }

  w.write_depends(w.type_namespace, '0');
  w.save_header('1');
}

    static void write_namespace_2_h(std::string_view const& ns, cache::namespace_members const& members)
    {
        writer w;
        w.type_namespace = ns;

    static void write_namespace_h(std::string_view const& ns, cache::namespace_members const& members)
    {
        writer w;
        w.type_namespace = ns;
        {
            auto wrap = wrap_type_namespace(w, ns);

    w.write("#pragma region methods\n");
    w.write_each<write_class>(members.classes);
    w.write("#pragma endregion methods\n\n");
  }

  write_close_file_guard(w);
  w.swap();
  write_preamble(w);
  write_open_file_guard(w, ns);
  write_version_assert(w);

        w.write_depends(w.type_namespace, '2');
        // Workaround for https://github.com/microsoft/cppwin32/issues/2
        for (auto&& extern_depends : w.extern_depends)
        {
            auto guard = wrap_type_namespace(w, extern_depends.first);
            w.write_each<write_extern_forward>(extern_depends.second);
        }
        w.save_header();
    }

inline bool is_x64_struct(const TypeDef& method) {
  auto const attr =
      get_attribute(method, "Windows.Win32.Interop", "SupportedArchitectureAttribute");
  if (attr) {
    CustomAttributeSig attr_sig = attr.Value();
    // PRINT_VAR(attr_sig.FixedArgs().size());
    // PRINT_VAR(attr_sig.NamedArgs().size());
    FixedArgSig fixed_arg = attr_sig.FixedArgs()[0];
    ElemSig elem_sig = std::get<ElemSig>(fixed_arg.value);
    ElemSig::EnumValue enum_value = std::get<ElemSig::EnumValue>(elem_sig.value);
    auto const arch_flags = std::get<int32_t>(enum_value.value);
    // PRINT_VAR(arch_flags);
    // PRINT_VAR("Found Supported Arch Attr");
    // method_signature signature{ method };
    // PRINT_VAR(method.Name());
    if ((arch_flags & 2) == 0)  // x64
      return false;
  }
  return true;
}

        write_close_file_guard(w);
        w.swap();
        write_preamble(w);
        write_open_file_guard(w, ns, '2');

        w.write_depends(w.type_namespace, '1');
        // Workaround for https://github.com/microsoft/cppwin32/issues/2
        for (auto&& extern_depends : w.extern_depends)
        {
            auto guard = wrap_type_namespace(w, extern_depends.first);
            w.write_each<write_extern_forward>(extern_depends.second);
        }
        w.save_header('2');
    }
  }

    static void write_namespace_h(std::string_view const& ns, cache::namespace_members const& members)
    {
        writer w;
        w.type_namespace = ns;
        {
            auto wrap = wrap_type_namespace(w, ns);

  write_close_file_guard(w);
  w.swap();

        write_close_file_guard(w);
        w.swap();
        write_preamble(w);
        write_open_file_guard(w, ns);
        write_version_assert(w);

        w.write_depends(w.type_namespace, '2');
        // Workaround for https://github.com/microsoft/cppwin32/issues/2
        for (auto&& extern_depends : w.extern_depends)
        {
            auto guard = wrap_type_namespace(w, extern_depends.first);
            w.write_each<write_extern_forward>(extern_depends.second);
        }
        w.save_header();
    }

    static void write_complex_structs_h(cache const& c)
    {
        writer w;

        type_dependency_graph graph;
        for (auto&& [ns, members] : c.namespaces())
        {
            for (auto&& s : members.structs)
            {
                graph.add_struct(s);
            }
        }

        graph.walk_graph([&](TypeDef const& type)
            {
                if (!is_nested(type))
                {
                    auto guard = wrap_type_namespace(w, type.TypeNamespace());
                    write_struct(w, type);
                }
            });

  w.write("#pragma once\n\n");
  w.write("#include <array>\n\n");
  w.write(GetWindowsApiFunctionDefinition());

  std::map<const MethodDef, std::string> method_def_to_namespace;

        for (auto&& depends : w.depends)
        {
            w.write_depends(depends.first, '0');
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
      std::string module_name = ToLower(module_ref.Name());
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
    w.write("constexpr std::array<const WindowsApiFunction, %> kFunctions = {{\n",
            db.get_table<ImplMap>().size());
    for (const auto& [key, function_info] : function_key_to_info) {
      w.write("  {\"%_%\", \"%\"},\n", function_info.module, function_info.name,
              function_info.name_space);
    }

    static void write_complex_interfaces_h(cache const& c)
    {
        writer w;

        type_dependency_graph graph;
        for (auto&& [ns, members] : c.namespaces())
        {
            for (auto&& s : members.interfaces)
            {
                graph.add_interface(s);
            }
        }

        graph.walk_graph([&](TypeDef const& type)
            {
                if (!is_nested(type))
                {
                    auto guard = wrap_type_namespace(w, type.TypeNamespace());
                    write_interface(w, type);
                }
            });

constexpr const char* namespace_dispatcher_header =
    R"(// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "NamespaceDispatcher.h"
)";

        for (auto&& depends : w.depends)
        {
            w.write_depends(depends.first, '1');
        }
        // Workaround for https://github.com/microsoft/cppwin32/issues/2
        for (auto&& extern_depends : w.extern_depends)
        {
            auto guard = wrap_type_namespace(w, extern_depends.first);
            w.write_each<write_extern_forward>(extern_depends.second);
        }

        w.flush_to_file(settings.output_folder + "win32/impl/complex_interfaces.h");
    }
}