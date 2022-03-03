// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef OBJECT_UTILS_SYMBOLS_FILE_H_
#define OBJECT_UTILS_SYMBOLS_FILE_H_

#include <llvm/Demangle/Demangle.h>

#include <filesystem>
#include <memory>
#include <string>

#include "GrpcProtos/symbol.pb.h"
#include "OrbitBase/Result.h"

namespace orbit_object_utils {

struct ObjectFileInfo {
  // For ELF, this is the load bias of the executable segment. For PE/COFF, we use ImageBase here,
  // so that our address computations are consistent between what we do for ELF and for COFF.
  uint64_t load_bias = 0;
  // This is the offset of the executable segment when loaded into memory. For ELF, as we defined
  // the load bias based on the executable segment, this is also the offset of the executable
  // segment in the file. For PE/COFF, this is in general different from the offset of the .text
  // section in the file.
  uint64_t executable_segment_offset = 0;
};

struct FunctionSymbol {
  std::string name;
  std::string demangled_name;
  uint32_t rva;
  uint32_t size;
};

struct DebugSymbols {
  DebugSymbols() = default;
  DebugSymbols(DebugSymbols&& debug_symbols) = default;
  DebugSymbols& operator=(DebugSymbols&& debug_symbols) = default;

  std::string symbols_file_path;
  std::string pdb_parser_name;
  uint64_t parse_time_ms = 0;

  uint64_t load_bias;
  std::vector<FunctionSymbol> function_symbols;
};

class SymbolsFile {
 public:
  SymbolsFile() = default;
  virtual ~SymbolsFile() = default;

  // For ELF files, the string returned by GetBuildId() is the standard build id that can be found
  // in the .note.gnu.build-id section, formatted as a human-readable string.
  // PE/COFF object files are uniquely identified by the PDB debug info consisting of a GUID and
  // age. The build id is formed from these to provide a string that uniquely identifies this object
  // file and the corresponding PDB debug info. The build id for PDB files is formed in the same
  // way.
  [[nodiscard]] virtual std::string GetBuildId() const = 0;

  [[nodiscard]] virtual const std::filesystem::path& GetFilePath() const = 0;

  [[nodiscard]] virtual ErrorMessageOr<DebugSymbols> LoadDebugSymbolsInternal() = 0;

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadDebugSymbols(
      google::protobuf::Arena* arena);

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadDebugSymbols() {
    return LoadDebugSymbols(nullptr);
  };
};

// Create a symbols file from the file at symbol_file_path. Additional info about the corresponding
// module can be passed in via object_file_info. This is necessary for PDB files, where information
// such as the load bias cannot be determined from the PDB file alone but is needed to compute the
// right addresses for symbols.
ErrorMessageOr<std::unique_ptr<SymbolsFile>> CreateSymbolsFile(
    const std::filesystem::path& symbol_file_path, const ObjectFileInfo& object_file_info);

}  // namespace orbit_object_utils

#endif  // OBJECT_UTILS_SYMBOLS_FILE_H_