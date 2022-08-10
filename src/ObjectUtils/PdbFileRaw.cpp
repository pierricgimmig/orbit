// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "PdbFileRaw.h"

#include <absl/memory/memory.h>
#include <absl/strings/match.h>
#include <llvm/Demangle/Demangle.h>

#include "OrbitBase/ExecutablePath.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/UniqueResource.h"

#include <Windows.h>

// raw_pdb
typedef void* HANDLE;
#include <Examples/ExampleMemoryMappedFile.h>
#include <PDB.h>

#include "Examples/ExampleTimedScope.h"
#include "PDB_InfoStream.h"

using orbit_grpc_protos::ModuleSymbols;
using orbit_grpc_protos::SymbolInfo;

namespace orbit_object_utils {

PdbFileRaw::PdbFileRaw(std::filesystem::path file_path, const ObjectFileInfo& object_file_info)
    : file_path_(std::move(file_path)), object_file_info_(object_file_info) {
  std::string pdb_file_path = file_path_.string();
  pdb_file_path_w_ = std::wstring(pdb_file_path.begin(), pdb_file_path.end());
}

ErrorMessageOr<DebugSymbols> PdbFileRaw::LoadDebugSymbolsInternal()
{
  // ORBIT_SCOPED_TIMED_LOG("PdbFileRaw::LoadDebugSymbols");
  DebugSymbols debug_symbols;
  debug_symbols.symbols_file_path = file_path_.string();
  debug_symbols.load_bias = object_file_info_.load_bias;

  MemoryMappedFile::Handle raw_pdb_file_handle_ = MemoryMappedFile::Open(pdb_file_path_w_.c_str());
  if (raw_pdb_file_handle_.baseAddress == nullptr) {
    return ErrorMessage("Could not create memory mapped pdb file.");
  }

  orbit_base::unique_resource pdb_file_closer(raw_pdb_file_handle_, MemoryMappedFile::Close);

  PDB::RawFile raw_pdb_file_ = PDB::CreateRawFile(raw_pdb_file_handle_.baseAddress);

  if (PDB::HasValidDBIStream(raw_pdb_file_) != PDB::ErrorCode::Success) {
    return ErrorMessage("Invalid DBI stream");
  }

  const PDB::DBIStream dbi_stream = PDB::CreateDBIStream(raw_pdb_file_);

  if (dbi_stream.HasValidImageSectionStream(raw_pdb_file_) != PDB::ErrorCode::Success) {
    return ErrorMessage("DBI stream has invalid image section stream");
  }

  if (dbi_stream.HasValidPublicSymbolStream(raw_pdb_file_) != PDB::ErrorCode::Success) {
    return ErrorMessage("DBI stream has invalid public symbol stream");
  }

  if (dbi_stream.HasValidGlobalSymbolStream(raw_pdb_file_) != PDB::ErrorCode::Success) {
    return ErrorMessage("DBI stream has invalid global symbol stream");
  }

  if (dbi_stream.HasValidSectionContributionStream(raw_pdb_file_) != PDB::ErrorCode::Success) {
    return ErrorMessage("DBI stream has invalid section contribution stream");
  }

    // in order to keep the example easy to understand, we load the PDB data serially.
  // note that this can be improved a lot by reading streams concurrently.

  // prepare the image section stream first. it is needed for converting section + offset into an
  // RVA
  const PDB::ImageSectionStream imageSectionStream =
      dbi_stream.CreateImageSectionStream(raw_pdb_file_);

  // prepare the module info stream for grabbing function symbols from modules
  const PDB::ModuleInfoStream moduleInfoStream = dbi_stream.CreateModuleInfoStream(raw_pdb_file_);

  // prepare symbol record stream needed by the public stream
  const PDB::CoalescedMSFStream symbolRecordStream =
      dbi_stream.CreateSymbolRecordStream(raw_pdb_file_);

  // note that we only use unordered_set in order to keep the example code easy to understand.
  // using other hash set implementations like e.g. abseil's Swiss Tables
  // (https://abseil.io/about/design/swisstables) is *much* faster.
  std::vector<FunctionSymbol>& functionSymbols = debug_symbols.function_symbols;
  std::unordered_set<uint32_t> seenFunctionRVAs;

  // start by reading the module stream, grabbing every function symbol we can find.
  // in most cases, this gives us ~90% of all function symbols already, along with their size.
  {
    // ORBIT_SCOPED_TIMED_LOG("Storing function symbols from modules");

    const PDB::ArrayView<PDB::ModuleInfoStream::Module> modules = moduleInfoStream.GetModules();

    for (const PDB::ModuleInfoStream::Module& module : modules) {
      if (!module.HasSymbolStream()) {
        continue;
      }

      const PDB::ModuleSymbolStream moduleSymbolStream = module.CreateSymbolStream(raw_pdb_file_);
      moduleSymbolStream.ForEachSymbol([&functionSymbols, &seenFunctionRVAs, &imageSectionStream](
                                           const PDB::CodeView::DBI::Record* record) {
        // only grab function symbols from the module streams
        const char* name = nullptr;
        uint32_t rva = 0u;
        uint32_t size = 0u;
        if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_THUNK32) {
          if (record->data.S_THUNK32.thunk ==
              PDB::CodeView::DBI::ThunkOrdinal::TrampolineIncremental) {
            return;
            // we have never seen incremental linking thunks stored inside a S_THUNK32 symbol, but
            // better safe than sorry
            name = "ILT";
            rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_THUNK32.section,
                                                               record->data.S_THUNK32.offset);
            size = 5u;
          }
        } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_TRAMPOLINE) {
          return;
          // incremental linking thunks are stored in the linker module
          name = "ILT";
          rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_TRAMPOLINE.thunkSection,
                                                             record->data.S_TRAMPOLINE.thunkOffset);
          size = 5u;
        } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_LPROC32) {
          name = record->data.S_LPROC32.name;
          rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_LPROC32.section,
                                                             record->data.S_LPROC32.offset);
          size = record->data.S_LPROC32.codeSize;
        } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_GPROC32) {
          name = record->data.S_GPROC32.name;
          rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_GPROC32.section,
                                                             record->data.S_GPROC32.offset);
          size = record->data.S_GPROC32.codeSize;
        } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_LPROC32_ID) {
          name = record->data.S_LPROC32_ID.name;
          rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_LPROC32_ID.section,
                                                             record->data.S_LPROC32_ID.offset);
          size = record->data.S_LPROC32_ID.codeSize;
        } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_GPROC32_ID) {
          name = record->data.S_GPROC32_ID.name;
          rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_GPROC32_ID.section,
                                                             record->data.S_GPROC32_ID.offset);
          size = record->data.S_GPROC32_ID.codeSize;
        }

        if (rva == 0u) {
          return;
        }

        functionSymbols.push_back(FunctionSymbol{name, "",  rva, size});
        seenFunctionRVAs.emplace(rva);
      });
    }
  }

  // we don't need to touch global symbols in this case.
  // most of the data we need can be obtained from the module symbol streams, and the global symbol
  // stream only offers data symbols on top of that, which we are not interested in. however, there
  // can still be public function symbols we haven't seen yet in any of the modules, especially for
  // PDBs that don't provide module-specific information.

  // read public symbols
  const PDB::PublicSymbolStream publicSymbolStream =
      dbi_stream.CreatePublicSymbolStream(raw_pdb_file_);
  {
    // ORBIT_SCOPED_TIMED_LOG("Storing public function symbols");

    const PDB::ArrayView<PDB::HashRecord> hashRecords = publicSymbolStream.GetRecords();
    const size_t count = hashRecords.GetLength();

    for (const PDB::HashRecord& hashRecord : hashRecords) {
      const PDB::CodeView::DBI::Record* record =
          publicSymbolStream.GetRecord(symbolRecordStream, hashRecord);
      if ((PDB_AS_UNDERLYING(record->data.S_PUB32.flags) &
           PDB_AS_UNDERLYING(PDB::CodeView::DBI::PublicSymbolFlags::Function)) == 0u) {
        // ignore everything that is not a function
        continue;
      }

      const uint32_t rva = imageSectionStream.ConvertSectionOffsetToRVA(
          record->data.S_PUB32.section, record->data.S_PUB32.offset);
      if (rva == 0u) {
        // certain symbols (e.g. control-flow guard symbols) don't have a valid RVA, ignore those
        continue;
      }

      // check whether we already know this symbol from one of the module streams
      const auto it = seenFunctionRVAs.find(rva);
      if (it != seenFunctionRVAs.end()) {
        // we know this symbol already, ignore it
        continue;
      }

      // this is a new function symbol, so store it.
      // note that we don't know its size yet.
      functionSymbols.push_back(FunctionSymbol{record->data.S_PUB32.name, "", rva, 0u});
    }
  }

  // we still need to find the size of the public function symbols.
  // this can be deduced by sorting the symbols by their RVA, and then computing the distance
  // between the current and the next symbol. this works since functions are always mapped to
  // executable pages, so they aren't interleaved by any data symbols.
  // ORBIT_SCOPED_TIMED_LOG("std::sort function symbols");
  {
    std::sort(
        functionSymbols.begin(), functionSymbols.end(),
        [](const FunctionSymbol& lhs, const FunctionSymbol& rhs) { return lhs.rva < rhs.rva; });
  }

  const size_t symbolCount = functionSymbols.size();
  if (symbolCount != 0u) {
    // ORBIT_SCOPED_TIMED_LOG("Computing function symbol sizes");

    size_t foundCount = 0u;

    // we have at least 1 symbol.
    // compute missing symbol sizes by computing the distance from this symbol to the next.
    // note that this includes "int 3" padding after the end of a function. if you don't want that,
    // but the actual number of bytes of the function's code, your best bet is to use a disassembler
    // instead.
    for (size_t i = 0u; i < symbolCount - 1u; ++i) {
      FunctionSymbol& currentSymbol = functionSymbols[i];
      if (currentSymbol.size != 0u) {
        // the symbol's size is already known
        continue;
      }

      const FunctionSymbol& nextSymbol = functionSymbols[i + 1u];
      const size_t size = nextSymbol.rva - currentSymbol.rva;
      ++foundCount;
    }

    // we know have the sizes of all symbols, except the last.
    // this can be found by going through the contributions, if needed.
    FunctionSymbol& lastSymbol = functionSymbols[symbolCount - 1u];
    if (lastSymbol.size != 0u) {
      // bad luck, we can't deduce the last symbol's size, so have to consult the contributions
      // instead. we do a linear search in this case to keep the code simple.
      const PDB::SectionContributionStream sectionContributionStream =
          dbi_stream.CreateSectionContributionStream(raw_pdb_file_);
      const PDB::ArrayView<PDB::DBI::SectionContribution> sectionContributions =
          sectionContributionStream.GetContributions();
      for (const PDB::DBI::SectionContribution& contribution : sectionContributions) {
        const uint32_t rva =
            imageSectionStream.ConvertSectionOffsetToRVA(contribution.section, contribution.offset);
        if (rva == 0u) {
          printf("Contribution has invalid RVA\n");
          continue;
        }

        if (rva == lastSymbol.rva) {
          lastSymbol.size = contribution.size;
          break;
        }

        if (rva > lastSymbol.rva) {
          // should have found the contribution by now
          printf("Unknown contribution for symbol %s at RVA 0x%X", lastSymbol.name.c_str(),
                 lastSymbol.rva);
          break;
        }
      }
    }
  }

  return debug_symbols;
}

ErrorMessageOr<void> PdbFileRaw::RetrieveAgeAndGuid() {
  // ORBIT_SCOPED_TIMED_LOG("PdbFileRaw::RetrieveAgeAndGuid");

  MemoryMappedFile::Handle raw_pdb_file_handle = MemoryMappedFile::Open(pdb_file_path_w_.c_str());
  if (raw_pdb_file_handle.baseAddress == nullptr) {
    return ErrorMessage("Could not create memory mapped pdb file.");
  }

  orbit_base::unique_resource pdb_file_closer(raw_pdb_file_handle, MemoryMappedFile::Close);

  if (PDB::ValidateFile(raw_pdb_file_handle.baseAddress) != PDB::ErrorCode::Success) {
    return ErrorMessage("Failed raw_pdb file validation");
  }

  PDB::RawFile raw_pdb_file_ = PDB::CreateRawFile(raw_pdb_file_handle.baseAddress);
  PDB::InfoStream pdb_info_stream(raw_pdb_file_);
  age_ = pdb_info_stream.GetHeader()->age;
  static_assert(sizeof(guid_) == sizeof(GUID));
  std::memcpy(guid_.data(), &pdb_info_stream.GetHeader()->guid, sizeof(GUID));

  return outcome::success();
}

void PdbFileRaw::SetDemangledNames(orbit_grpc_protos::ModuleSymbols* module_symbols) {
  for (SymbolInfo& symbol_info : *module_symbols->mutable_symbol_infos()) {
    symbol_info.set_demangled_name(symbol_info.demangled_name());
  }
}

ErrorMessageOr<std::unique_ptr<PdbFile>> PdbFileRaw::CreatePdbFile(
    const std::filesystem::path& file_path, const ObjectFileInfo& object_file_info) {
  auto pdb_file_raw = absl::WrapUnique<PdbFileRaw>(new PdbFileRaw(file_path, object_file_info));
  OUTCOME_TRY(pdb_file_raw->RetrieveAgeAndGuid());
  return std::move(pdb_file_raw);
}

}  // namespace orbit_object_utils
