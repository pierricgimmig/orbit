// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Gate 2 of docs/rust-port-plan.html: run both ElfFile backends over every ELF
// file it can find and compare them.
//
// No diffing script is needed, because ORBIT_OBJECT_BACKEND=both does the
// comparison in-process and aborts on the first disagreement, naming the
// method and the symbol. This tool only supplies the corpus.
//
//   ORBIT_OBJECT_BACKEND=both bazel-bin/rust/tools/differential/elf_corpus
//       src/ObjectUtils/testdata bazel-bin /usr/lib/x86_64-linux-gnu
//
// The curated testdata is a dozen small binaries chosen to exercise specific
// code paths. A real libQt5Core.so exercises combinations nobody wrote a test
// for, which is the point.

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#include <filesystem>
#include <memory>
#include <string>
#include <vector>

#include "ObjectUtils/ElfFile.h"
#include "RustElfFile.h"
#include "OrbitBase/Result.h"

namespace {

[[nodiscard]] bool LooksLikeElf(const std::filesystem::path& path) {
  FILE* file = fopen(path.c_str(), "rb");
  if (file == nullptr) return false;
  char magic[4] = {};
  const size_t read = fread(magic, 1, sizeof(magic), file);
  fclose(file);
  return read == sizeof(magic) && magic[0] == 0x7f && magic[1] == 'E' && magic[2] == 'L' &&
         magic[3] == 'F';
}

struct Totals {
  int files = 0;
  int loaded = 0;
  int rejected = 0;
  long long symbols = 0;
};

void Visit(const std::filesystem::path& path, Totals* totals) {
  if (!LooksLikeElf(path)) return;
  // ORBIT_OBJECT_BACKEND=both aborts on a disagreement, so the last path
  // printed here is the file that caused it.
  if (getenv("ORBIT_CORPUS_VERBOSE") != nullptr) {
    fprintf(stderr, "visiting %s\n", path.c_str());
    fflush(stderr);
  }
  ++totals->files;

  ErrorMessageOr<std::unique_ptr<orbit_object_utils::ElfFile>> elf_file =
      orbit_object_utils::CreateElfFile(path);
  if (elf_file.has_error()) {
    ++totals->rejected;
    return;
  }
  ++totals->loaded;

  // Touching every ported method is what makes the comparison meaningful; the
  // metadata ones are checked at construction, these two are not.
  const auto debug_symbols = elf_file.value()->LoadDebugSymbols();
  if (debug_symbols.has_value()) totals->symbols += debug_symbols.value().symbol_infos_size();
  const auto dynsym = elf_file.value()->LoadSymbolsFromDynsym();
  if (dynsym.has_value()) totals->symbols += dynsym.value().symbol_infos_size();
  const auto unwind = elf_file.value()->LoadEhOrDebugFrameEntriesAsSymbols();
  if (unwind.has_value()) totals->symbols += unwind.value().symbol_infos_size();
  const auto fallback = elf_file.value()->LoadDynamicLinkingSymbolsAndUnwindRangesAsSymbols();
  if (fallback.has_value()) totals->symbols += fallback.value().symbol_infos_size();
}

}  // namespace

int main(int argc, char** argv) {
  if (argc < 2) {
    fprintf(stderr, "usage: %s <dir-or-file>...\n", argv[0]);
    return 2;
  }

  const char* backend = getenv("ORBIT_OBJECT_BACKEND");
  printf("backend: %s\n", backend == nullptr ? "cpp (default)" : backend);

  Totals totals;
  for (int i = 1; i < argc; ++i) {
    const std::filesystem::path root{argv[i]};
    std::error_code error;
    if (std::filesystem::is_directory(root, error)) {
      auto options = std::filesystem::directory_options::skip_permission_denied;
      for (auto it = std::filesystem::recursive_directory_iterator{root, options, error};
           it != std::filesystem::recursive_directory_iterator{}; it.increment(error)) {
        if (error) break;
        if (it->is_regular_file(error)) Visit(it->path(), &totals);
      }
    } else {
      Visit(root, &totals);
    }
  }

  printf("elf files seen   %d\n", totals.files);
  printf("loaded           %d\n", totals.loaded);
  printf("rejected         %d\n", totals.rejected);
  printf("symbols compared %lld\n", totals.symbols);

  uint64_t differing = 0;
  uint64_t compared = 0;
  orbit_object_utils_rust::GetDemanglingDivergence(&differing, &compared);
  if (compared > 0) {
    printf("demangling compared %llu\n", static_cast<unsigned long long>(compared));
    printf("demangling differing %llu (%.4f%%)\n", static_cast<unsigned long long>(differing),
           100.0 * static_cast<double>(differing) / static_cast<double>(compared));
  }
  return 0;
}
