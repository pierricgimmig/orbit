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

// raw_pdb
typedef void* HANDLE;
#include <Examples/ExampleMemoryMappedFile.h>
#include <PDB.h>

#include "Examples/ExampleTimedScope.h"
#include "PDB_DBIStream.h"
#include "PDB_InfoStream.h"
#include "PDB_RawFile.h"

using orbit_grpc_protos::ModuleSymbols;
using orbit_grpc_protos::SymbolInfo;

namespace orbit_object_utils {

PdbFileRaw::PdbFileRaw(std::filesystem::path file_path, const ObjectFileInfo& object_file_info)
    : file_path_(std::move(file_path)), object_file_info_(object_file_info) {
  std::string pdb_file_path = file_path_.string();
  pdb_file_path_w_ = std::wstring(pdb_file_path.begin(), pdb_file_path.end());
}

ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> PdbFileRaw::LoadDebugSymbols() {
  ORBIT_SCOPED_TIMED_LOG("PdbFileRaw::LoadDebugSymbols");

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

  // prepare the module info stream for matching contributions against files
  const PDB::ModuleInfoStream moduleInfoStream = dbi_stream.CreateModuleInfoStream(raw_pdb_file_);

  // prepare symbol record stream needed by both public and global streams
  const PDB::CoalescedMSFStream symbolRecordStream =
      dbi_stream.CreateSymbolRecordStream(raw_pdb_file_);

  ModuleSymbols module_symbols;
  module_symbols.set_load_bias(object_file_info_.load_bias);
  module_symbols.set_symbols_file_path(file_path_.string());

  // read public symbols
  const PDB::PublicSymbolStream publicSymbolStream =
      dbi_stream.CreatePublicSymbolStream(raw_pdb_file_);

  std::map<uint32_t, uint32_t> count_by_type;

  {
    ORBIT_SCOPED_TIMED_LOG("Storing public symbols");

    const PDB::ArrayView<PDB::HashRecord> hashRecords = publicSymbolStream.GetRecords();
    const size_t count = hashRecords.GetLength();

    for (const PDB::HashRecord& hashRecord : hashRecords) {
      const PDB::CodeView::DBI::Record* record =
          publicSymbolStream.GetRecord(symbolRecordStream, hashRecord);

      if ((record->data.S_PUB32.flags & PDB::CodeView::DBI::PublicSymbolFlags::Function) ==
          PDB::CodeView::DBI::PublicSymbolFlags::None) {
        // continue;
      }

      ++count_by_type[(uint32_t)record->header.kind];

      const uint32_t rva = imageSectionStream.ConvertSectionOffsetToRVA(
          record->data.S_PUB32.section, record->data.S_PUB32.offset);
      if (rva == 0u) {
        // certain symbols (e.g. control-flow guard symbols) don't have a valid RVA, ignore those
        continue;
      }

      SymbolInfo* symbol_info = module_symbols.add_symbol_infos();
      symbol_info->set_name(record->data.S_PUB32.name);
      symbol_info->set_demangled_name(llvm::demangle(symbol_info->name()));
      symbol_info->set_address(rva + object_file_info_.load_bias);
      symbol_info->set_size(0);

      if (absl::StrContains(symbol_info->demangled_name(), "PrintHelloWorldInternal")) {
        ORBIT_LOG("name : %s", symbol_info->name());
        ORBIT_LOG("name demangled: %s", symbol_info->demangled_name());
        ORBIT_LOG("record->header.size = %u", record->header.size);
        ORBIT_LOG("record->header.kind = %u", (uint32_t)record->header.kind);
        ORBIT_LOG("record->data.S_PUB32.offset = %u", record->data.S_PUB32.offset);
        ORBIT_LOG("record->data.S_PUB32.section = %u", record->data.S_PUB32.section);
        ORBIT_LOG("name : %s", symbol_info->name());
      }
      }

    for (auto [type, count] : count_by_type) {
      ORBIT_LOG("type %u = %u", type, count);
    }
    ORBIT_LOG("Num symbols = %u", module_symbols.symbol_infos().size());
  }

#if 0
  // read global symbols
  const PDB::GlobalSymbolStream globalSymbolStream = dbi_stream.CreateGlobalSymbolStream(raw_pdb_file_);
  {
    ORBIT_SCOPED_TIMED_LOG("Storing global symbols");

    const PDB::ArrayView<PDB::HashRecord> hashRecords = globalSymbolStream.GetRecords();
    const size_t count = hashRecords.GetLength();

    symbols.reserve(symbols.size() + count);

    for (const PDB::HashRecord& hashRecord : hashRecords) {
      const PDB::CodeView::DBI::Record* record =
          globalSymbolStream.GetRecord(symbolRecordStream, hashRecord);

      const char* name = nullptr;
      uint32_t rva = 0u;
      if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_GDATA32) {
        name = record->data.S_GDATA32.name;
        rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_GDATA32.section,
                                                           record->data.S_GDATA32.offset);
      } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_GTHREAD32) {
        name = record->data.S_GTHREAD32.name;
        rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_GTHREAD32.section,
                                                           record->data.S_GTHREAD32.offset);
      } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_LDATA32) {
        name = record->data.S_LDATA32.name;
        rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_LDATA32.section,
                                                           record->data.S_LDATA32.offset);
      } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_LTHREAD32) {
        name = record->data.S_LTHREAD32.name;
        rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_LTHREAD32.section,
                                                           record->data.S_LTHREAD32.offset);
      }

      if (rva == 0u) {
        // certain symbols (e.g. control-flow guard symbols) don't have a valid RVA, ignore those
        continue;
      }

      symbols.push_back(Symbol{name, rva});
    }
  }

  // read module symbols
  {
    ORBIT_SCOPED_TIMED_LOG("Storing symbols from modules");

    const PDB::ArrayView<PDB::ModuleInfoStream::Module> modules = moduleInfoStream.GetModules();

    for (const PDB::ModuleInfoStream::Module& module : modules) {
      if (!module.HasSymbolStream()) {
        continue;
      }

      const PDB::ModuleSymbolStream moduleSymbolStream = module.CreateSymbolStream(raw_pdb_file_);
      moduleSymbolStream.ForEachSymbol(
          [&symbols, &imageSectionStream](const PDB::CodeView::DBI::Record* record) {
            const char* name = nullptr;
            uint32_t rva = 0u;
            if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_THUNK32) {
              // incremental linking thunks are stored in the linker module
              name = "ILT";
              rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_THUNK32.section,
                                                                 record->data.S_THUNK32.offset);
            } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_BLOCK32) {
              // blocks never store a name and are only stored for indicating whether other symbols
              // are children of this block
            } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_LABEL32) {
              // labels don't have a name
            } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_LPROC32) {
              name = record->data.S_LPROC32.name;
              rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_LPROC32.section,
                                                                 record->data.S_LPROC32.offset);
            } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_GPROC32) {
              name = record->data.S_GPROC32.name;
              rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_GPROC32.section,
                                                                 record->data.S_GPROC32.offset);
            } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_LPROC32_ID) {
              name = record->data.S_LPROC32_ID.name;
              rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_LPROC32_ID.section,
                                                                 record->data.S_LPROC32_ID.offset);
            } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_GPROC32_ID) {
              name = record->data.S_GPROC32_ID.name;
              rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_GPROC32_ID.section,
                                                                 record->data.S_GPROC32_ID.offset);
            } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_LDATA32) {
              name = record->data.S_LDATA32.name;
              rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_LDATA32.section,
                                                                 record->data.S_LDATA32.offset);
            } else if (record->header.kind == PDB::CodeView::DBI::SymbolRecordKind::S_LTHREAD32) {
              name = record->data.S_LTHREAD32.name;
              rva = imageSectionStream.ConvertSectionOffsetToRVA(record->data.S_LTHREAD32.section,
                                                                 record->data.S_LTHREAD32.offset);
            }

            if (rva == 0u) {
              // certain symbols (e.g. control-flow guard symbols) don't have a valid RVA, ignore
              // those
              return;
            }

            //symbols.push_back(Symbol{name, rva});
          });
    }
  }
#endif

  return module_symbols;
}

ErrorMessageOr<void> PdbFileRaw::RetrieveAgeAndGuid() {
  ORBIT_SCOPED_TIMED_LOG("PdbFileRaw::RetrieveAgeAndGuid");

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

ErrorMessageOr<std::unique_ptr<PdbFile>> PdbFileRaw::CreatePdbFile(
    const std::filesystem::path& file_path, const ObjectFileInfo& object_file_info) {
  auto pdb_file_raw = absl::WrapUnique<PdbFileRaw>(new PdbFileRaw(file_path, object_file_info));
  OUTCOME_TRY(pdb_file_raw->RetrieveAgeAndGuid());
  return std::move(pdb_file_raw);
}

}  // namespace orbit_object_utils
