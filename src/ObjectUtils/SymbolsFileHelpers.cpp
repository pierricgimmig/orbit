// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The parts of SymbolsFile that are pure post-processing over the protobuf.
//
// Split out of SymbolsFile.cpp so they can live in ObjectUtilsHeaders, which
// the Rust shims depend on. CreateSymbolsFile cannot: it calls the object-file
// factories, which the shims implement.

#include <algorithm>
#include <vector>

#include "GrpcProtos/symbol.pb.h"
#include "ObjectUtils/SymbolsFile.h"

namespace orbit_object_utils {

bool SymbolsFile::SymbolInfoLessByAddress(const orbit_grpc_protos::SymbolInfo& lhs,
                                          const orbit_grpc_protos::SymbolInfo& rhs) {
  return lhs.address() < rhs.address();
}

void SymbolsFile::DeduceDebugSymbolMissingSizesAsDistanceFromNextSymbol(
    std::vector<orbit_grpc_protos::SymbolInfo>* symbol_infos) {
  // There might be functions for which we don't have sizes in the symbol information (like COFF
  // symbol table, or PDB public symbols). For these, compute the size as the distance from the
  // address of the next function.
  std::sort(symbol_infos->begin(), symbol_infos->end(), &SymbolInfoLessByAddress);

  for (size_t i = 0; i < symbol_infos->size(); ++i) {
    orbit_grpc_protos::SymbolInfo& symbol_info = symbol_infos->at(i);
    if (symbol_info.size() != kUnknownSymbolSize) {
      // This function symbol already has a size.
      continue;
    }

    if (i < symbol_infos->size() - 1) {
      // Deduce the size as the distance from the next function's address.
      symbol_info.set_size(symbol_infos->at(i + 1).address() - symbol_info.address());
    } else {
      // If the last symbol doesn't have a size, we can't deduce it, and we just set it to zero.
      symbol_info.set_size(0);
    }
  }
}

}  // namespace orbit_object_utils
