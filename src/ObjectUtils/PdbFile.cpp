// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ObjectUtils/PdbFile.h"

#include <llvm/DebugInfo/CodeView/CVSymbolVisitor.h>
#include <llvm/DebugInfo/CodeView/CodeView.h>
#include <llvm/DebugInfo/CodeView/GUID.h>
#include <llvm/DebugInfo/CodeView/SymbolDeserializer.h>
#include <llvm/DebugInfo/CodeView/SymbolVisitorCallbackPipeline.h>
#include <llvm/DebugInfo/CodeView/SymbolVisitorCallbacks.h>
#include <llvm/DebugInfo/PDB/Native/DbiStream.h>
#include <llvm/DebugInfo/PDB/Native/ModuleDebugStream.h>
#include <llvm/DebugInfo/PDB/Native/NativeSession.h>
#include <llvm/DebugInfo/PDB/Native/PDBFile.h>
#include <llvm/DebugInfo/PDB/PDB.h>
#include <llvm/DebugInfo/PDB/PDBSymbolExe.h>
#include <llvm/DebugInfo/PDB/PDBTypes.h>
#include <llvm/Demangle/Demangle.h>

#include <memory>

#include "OrbitBase/Logging.h"
#include "OrbitBase/Result.h"
#include "symbol.pb.h"

using orbit_grpc_protos::ModuleSymbols;
using orbit_grpc_protos::SymbolInfo;

namespace orbit_object_utils {

class SymbolVisitor : public llvm::codeview::SymbolVisitorCallbacks {
 public:
  SymbolVisitor(ModuleSymbols* module_symbols) : module_symbols_(module_symbols) {}

  llvm::Error visitSymbolBegin(llvm::codeview::CVSymbol& record) override {
    return llvm::Error::success();
  }
  llvm::Error visitSymbolBegin(llvm::codeview::CVSymbol& record, uint32_t offset) override {
    return llvm::Error::success();
  }
  llvm::Error visitSymbolEnd(llvm::codeview::CVSymbol& record) override {
    return llvm::Error::success();
  }

  llvm::Error visitKnownRecord(llvm::codeview::CVSymbol& record,
                               llvm::codeview::ProcSym& proc) override {
    SymbolInfo symbol_info;
    symbol_info.set_name(proc.Name.str());
    symbol_info.set_demangled_name(llvm::demangle(proc.Name.str()));
    // TODO: This address is an RVA, need to adjust?
    symbol_info.set_address(proc.CodeOffset);
    symbol_info.set_size(proc.CodeSize);
    *(module_symbols_->add_symbol_infos()) = std::move(symbol_info);

    return llvm::Error::success();
  }

 private:
  ModuleSymbols* module_symbols_;
};

class PdbFileImpl : public PdbFile {
 public:
  PdbFileImpl(std::filesystem::path file_path, std::unique_ptr<llvm::pdb::IPDBSession> session);

  [[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> LoadDebugSymbols() override;
  [[nodiscard]] const std::filesystem::path& GetFilePath() const override { return file_path_; }

  [[nodiscard]] const std::array<uint8_t, 16> GetGuid() const override {
    constexpr int kGuidSize = 16;
    std::array<uint8_t, kGuidSize> guid;
    auto global_scope = session_->getGlobalScope();
    llvm::codeview::GUID g = global_scope->getGuid();
    std::copy(&(g.Guid[0]), &(g.Guid[kGuidSize - 1]), &guid[0]);
    return guid;
  }

  [[nodiscard]] uint32_t GetAge() const override { return session_->getGlobalScope()->getAge(); }

 private:
  std::filesystem::path file_path_;
  std::unique_ptr<llvm::pdb::IPDBSession> session_;
};

PdbFileImpl::PdbFileImpl(std::filesystem::path file_path,
                         std::unique_ptr<llvm::pdb::IPDBSession> session)
    : file_path_(std::move(file_path)), session_(std::move(session)) {}

[[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ModuleSymbols> PdbFileImpl::LoadDebugSymbols() {
  ModuleSymbols module_symbols;
  module_symbols.set_symbols_file_path(file_path_.string());

  llvm::pdb::NativeSession* native_session = static_cast<llvm::pdb::NativeSession*>(session_.get());
  llvm::pdb::PDBFile& pdb_file = native_session->getPDBFile();

  if (!pdb_file.hasPDBDbiStream()) {
    return ErrorMessage("PDB file does not have a DBI stream.");
  }

  auto& dbi = pdb_file.getPDBDbiStream();
  const auto& modules = dbi->modules();

  for (uint32_t index = 0; index < modules.getModuleCount(); ++index) {
    auto modi = modules.getModuleDescriptor(index);
    uint16_t modi_stream = modi.getModuleStreamIndex();

    if (modi_stream == llvm::pdb::kInvalidStreamIndex) {
      continue;
    }

    auto mod_stream_data = pdb_file.createIndexedStream(modi_stream);
    llvm::pdb::ModuleDebugStreamRef mod_debug_stream(modi, std::move(mod_stream_data));

    mod_debug_stream.reload();

    llvm::codeview::SymbolVisitorCallbackPipeline pipeline;
    llvm::codeview::SymbolDeserializer deserializer(nullptr,
                                                    llvm::codeview::CodeViewContainer::Pdb);
    pipeline.addCallbackToPipeline(deserializer);
    SymbolVisitor symbol_visitor(&module_symbols);
    pipeline.addCallbackToPipeline(symbol_visitor);

    llvm::codeview::CVSymbolVisitor visitor(pipeline);

    auto symbol_stream = mod_debug_stream.getSymbolsSubstream();
    auto& symbol_array = mod_debug_stream.getSymbolArray();

    // TODO: Handle error.
    llvm::Error error = visitor.visitSymbolStream(symbol_array, symbol_stream.Offset);
  }
  return module_symbols;
}

ErrorMessageOr<std::unique_ptr<PdbFile>> CreatePdbFile(const std::filesystem::path& file_path) {
  llvm::StringRef pdb_path(file_path.string());
  std::unique_ptr<llvm::pdb::IPDBSession> session;
  llvm::Error error =
      llvm::pdb::loadDataForPDB(llvm::pdb::PDB_ReaderType::Native, pdb_path, session);
  if (error) {
    // TODO: Add llvm error to error message.
    return ErrorMessage(absl::StrFormat("Unable to load PDB file \"%s\":", file_path.string()));
  }
  return std::make_unique<PdbFileImpl>(file_path, std::move(session));
}
}  // namespace orbit_object_utils