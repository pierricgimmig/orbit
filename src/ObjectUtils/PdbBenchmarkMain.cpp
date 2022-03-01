// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <absl/strings/str_cat.h>
#include <absl/strings/str_format.h>

#include <fstream>
#include <map>

#include "ObjectUtils/PdbFile.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/MakeUniqueForOverwrite.h"
#include "OrbitBase/Result.h"
#include "OrbitBase/ExecutablePath.h"

using orbit_object_utils::DebugSymbols;
using orbit_object_utils::FunctionSymbol;
using orbit_object_utils::ObjectFileInfo;
using orbit_object_utils::PdbParser;

ErrorMessageOr<orbit_object_utils::DebugSymbols> CreatePdbFile(const std::filesystem::path pdb_path,
                                                               PdbParser parser,
                                                               const std::string& parser_name) {
  ObjectFileInfo object_file_info;
  auto symbol_result = orbit_object_utils::CreatePdbFile(pdb_path, object_file_info, parser);
  if (symbol_result.has_error()) {
    return ErrorMessage(absl::StrFormat("Creating \"%s\" pdb file: %s", pdb_path.string(),
                                        symbol_result.error().message()));
  }

  ORBIT_SCOPED_TIMED_LOG("%s - %s", pdb_path.string(), parser_name);
  return symbol_result.value()->LoadDebugSymbolsInternal();
}

// Outputs a text representation of the debug symbols sorted by rva.
inline ErrorMessageOr<void> OutputSymbolsToTextFile(const DebugSymbols& module_symbols,
                                          std::filesystem::path output_file_path) {
  std::map<uint64_t, const FunctionSymbol*> symbols_by_rva;
  std::multimap<uint64_t /*rva*/, const FunctionSymbol*> symbols_by_rva_multimap;
  std::map<uint64_t, uint64_t> duplicate_rva_to_count;
  for (const FunctionSymbol& function_symbol : module_symbols.function_symbols) {
    if (symbols_by_rva.find(function_symbol.rva) != symbols_by_rva.end()) {
      ++duplicate_rva_to_count[function_symbol.rva];
    }

    symbols_by_rva[function_symbol.rva] = &function_symbol;
    symbols_by_rva_multimap.emplace(std::make_pair(function_symbol.rva, &function_symbol));
  }

  /*for(auto [duplicate_rva, count] : duplicate_rva_to_count) {
    ORBIT_ERROR("%x was found %u times", duplicate_rva, count);
  }*/
  
  ORBIT_LOG("NumFunctionSymbols: %u", module_symbols.function_symbols.size());
  ORBIT_LOG("Num unique FunctionSymbols: %u", symbols_by_rva.size());

  std::filesystem::path parent_path = output_file_path.parent_path();
  if (!parent_path.empty()) {
    std::filesystem::create_directories(parent_path);
  }

  std::ofstream ofs(output_file_path);

  if (ofs.fail()) {
    return ErrorMessage(absl::StrFormat("Could not create file \"%s\"", output_file_path.string()));
  }

  for (const auto& [rva, symbol] : symbols_by_rva_multimap) {
    ofs << std::setw(16) << std::hex << rva << " " << symbol->name << ", "
        << symbol->demangled_name << std::endl;
  }

  ofs.close();

  return outcome::success();
}

int main(int argc, char* argv[]) {
  if (argc < 2) {
    ORBIT_ERROR("Please specify at least one path to a pdb file.");
    return -1;
  }

  std::vector<std::filesystem::path> pdb_paths;
  for (int i = 1; i < argc; ++i) {
    std::filesystem::path path = argv[i];
    if (!std::filesystem::exists(path)) {
      ORBIT_ERROR("Could not find \"%s\", skipping.", path.string());
      continue;
    }
    pdb_paths.push_back(path);
  }

  if (pdb_paths.empty()) {
    ORBIT_ERROR("No valid pdb files were found, exiting.");
    return -1;
  }

  std::map<PdbParser, std::string> parser_sequence = {
      {PdbParser::Llvm, "Llvm"}, {PdbParser::Dia, "Dia"}, {PdbParser::Raw, "Raw"}};

  bool raw_only = false;
  if (raw_only) {
    parser_sequence = {{PdbParser::Raw, "Raw"}};
  }

  std::set<std::string> generated_files;

  /*while (true)*/ {
    for (const std::filesystem::path& pdb_path : pdb_paths) {
      for (const auto& [parser, parser_name] : parser_sequence) {
        auto result = CreatePdbFile(pdb_path, parser, parser_name);

        if(result.has_error()) {
          ORBIT_ERROR("Creating pdb file: %s", result.error().message().c_str());
          continue;
        }

        std::string output_file_name =
            absl::StrFormat("%s_%s.txt", pdb_path.filename().string(), parser_name);

        // Output text representation once per pdb-parser pair.
        if (generated_files.find(output_file_name) == generated_files.end()) {
          generated_files.insert(output_file_name);
          std::filesystem::path output_path = orbit_base::GetExecutableDir() / output_file_name;
          OutputSymbolsToTextFile(result.value(), output_path);
        }
      }
    }
  }
}