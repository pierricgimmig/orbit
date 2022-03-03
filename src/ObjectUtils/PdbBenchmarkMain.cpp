// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <absl/strings/str_cat.h>
#include <absl/strings/str_format.h>

#include <fstream>
#include <map>

#include "ObjectUtils/PdbFile.h"
#include "OrbitBase/ExecutablePath.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/MakeUniqueForOverwrite.h"
#include "OrbitBase/Profiling.h"
#include "OrbitBase/Result.h"

using orbit_object_utils::DebugSymbols;
using orbit_object_utils::FunctionSymbol;
using orbit_object_utils::ObjectFileInfo;
using orbit_object_utils::PdbParser;

ErrorMessageOr<orbit_object_utils::DebugSymbols> LoadPdbFile(const std::filesystem::path pdb_path,
                                                               PdbParser parser,
                                                               const std::string& parser_name) {
  ObjectFileInfo object_file_info;
  OUTCOME_TRY(auto pdb_file, orbit_object_utils::CreatePdbFile(pdb_path, object_file_info, parser));

  uint64_t start_ns = orbit_base::CaptureTimestampNs();
  OUTCOME_TRY(DebugSymbols debug_symbols, pdb_file->LoadDebugSymbolsInternal());
  debug_symbols.parse_time_ms = (orbit_base::CaptureTimestampNs() - start_ns) * 0.000'0001;
  debug_symbols.pdb_parser_name = parser_name;

  ORBIT_LOG("%s: Parsed %u functions in %u ms", parser_name, debug_symbols.function_symbols.size(),
            debug_symbols.parse_time_ms );

  return debug_symbols;
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
    ofs << std::setw(16) << std::hex << rva << " " << symbol->name << ", " << symbol->demangled_name
        << std::endl;
  }

  ofs.close();

  return outcome::success();
}

int main(int argc, char* argv[]) {
  std::vector<std::filesystem::path> pdb_paths;

  if (argc < 2) {
    std::vector<std::filesystem::path> test_pdb_paths = {
        orbit_base::GetExecutableDir()  / "Orbit.pdb",
    };

    for (std::filesystem::path& path : test_pdb_paths) {
      if (std::filesystem::exists(path)) {
        pdb_paths.push_back(path);
      }
    }

    if (pdb_paths.empty()) {
      ORBIT_ERROR("Please specify at least one path to a pdb file.");
      return -1;
    }
  }

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
      ORBIT_LOG("%s (%u bytes):", pdb_path.string(), std::filesystem::file_size(pdb_path));
      for (const auto& [parser, parser_name] : parser_sequence) {
        auto result = LoadPdbFile(pdb_path, parser, parser_name);

        if (result.has_error()) {
          ORBIT_ERROR("Creating pdb file: %s", result.error().message().c_str());
          continue;
        }

        std::string output_file_name =
            absl::StrFormat("%s_%s.txt", pdb_path.filename().string(), parser_name);

        // Output text representation once per pdb-parser pair.
        if (generated_files.find(output_file_name) == generated_files.end()) {
          generated_files.insert(output_file_name);
          std::filesystem::path output_path = orbit_base::GetExecutableDir() / output_file_name;
          //OutputSymbolsToTextFile(result.value(), output_path);
        }
      }
    }
  }
}