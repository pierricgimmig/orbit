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

#include "ObjectUtils/CoffFile.h"
#include "ObjectUtils/ElfFile.h"
#include "RustElfFile.h"
#include "OrbitBase/Result.h"

namespace {

enum class Kind { kNeither, kElf, kPe };

[[nodiscard]] Kind ClassifyByMagic(const std::filesystem::path& path) {
  FILE* file = fopen(path.c_str(), "rb");
  if (file == nullptr) return Kind::kNeither;
  char magic[4] = {};
  const size_t read = fread(magic, 1, sizeof(magic), file);
  fclose(file);
  if (read < 2) return Kind::kNeither;
  if (read == sizeof(magic) && magic[0] == 0x7f && magic[1] == 'E' && magic[2] == 'L' &&
      magic[3] == 'F') {
    return Kind::kElf;
  }
  // PE images start with the DOS stub's "MZ".
  if (magic[0] == 'M' && magic[1] == 'Z') return Kind::kPe;
  return Kind::kNeither;
}

struct Totals {
  int files = 0;
  int loaded = 0;
  int rejected = 0;
  int pe_files = 0;
  int pe_loaded = 0;
  long long symbols = 0;
  long long line_lookups = 0;
};

// Line info is per address, so a whole-file sweep would dominate the run.
// Sampling the first few function addresses of each file with debug info
// still crosses every code path and stays affordable.
constexpr int kLineInfoSamplesPerFile = 32;

void VisitPe(const std::filesystem::path& path, Totals* totals) {
  ++totals->pe_files;
  ErrorMessageOr<std::unique_ptr<orbit_object_utils::CoffFile>> coff_file =
      orbit_object_utils::CreateCoffFile(path);
  if (coff_file.has_error()) return;
  ++totals->pe_loaded;

  // The metadata is compared at construction; these are the loaders that are
  // not, and they still delegate, so they only exercise the C++ path today.
  const auto debug_symbols = coff_file.value()->LoadDebugSymbols();
  if (debug_symbols.has_value()) totals->symbols += debug_symbols.value().symbol_infos_size();
  const auto exports = coff_file.value()->LoadSymbolsFromExportTable();
  if (exports.has_value()) totals->symbols += exports.value().symbol_infos_size();
  const auto exceptions = coff_file.value()->LoadExceptionTableEntriesAsSymbols();
  if (exceptions.has_value()) totals->symbols += exceptions.value().symbol_infos_size();
  const auto fallback = coff_file.value()->LoadDynamicLinkingSymbolsAndUnwindRangesAsSymbols();
  if (fallback.has_value()) totals->symbols += fallback.value().symbol_infos_size();
  (void)coff_file.value()->HasExportTable();
}

void Visit(const std::filesystem::path& path, Totals* totals) {
  const Kind kind = ClassifyByMagic(path);
  if (kind == Kind::kNeither) return;
  // ORBIT_OBJECT_BACKEND=both aborts on a disagreement, so the last path
  // printed here is the file that caused it.
  if (getenv("ORBIT_CORPUS_VERBOSE") != nullptr) {
    fprintf(stderr, "visiting %s\n", path.c_str());
    fflush(stderr);
  }

  if (kind == Kind::kPe) {
    VisitPe(path, totals);
    return;
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

  // GetLineInfo asserts on the presence of debug info, so only ask when there
  // is some.
  if (elf_file.value()->HasDebugInfo() && debug_symbols.has_value()) {
    int sampled = 0;
    for (const auto& symbol : debug_symbols.value().symbol_infos()) {
      if (sampled++ >= kLineInfoSamplesPerFile) break;
      (void)elf_file.value()->GetLineInfo(symbol.address());
      ++totals->line_lookups;
    }
  }
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
  printf("pe files seen    %d\n", totals.pe_files);
  printf("pe loaded        %d\n", totals.pe_loaded);
  printf("loaded           %d\n", totals.loaded);
  printf("rejected         %d\n", totals.rejected);
  printf("symbols compared %lld\n", totals.symbols);
  printf("line lookups compared %lld\n", totals.line_lookups);

  uint64_t differing = 0;
  uint64_t compared = 0;
  orbit_object_utils_rust::GetDemanglingDivergence(&differing, &compared);
  if (compared > 0) {
    printf("demangling compared %llu\n", static_cast<unsigned long long>(compared));
    printf("demangling differing %llu (%.4f%%)\n", static_cast<unsigned long long>(differing),
           100.0 * static_cast<double>(differing) / static_cast<double>(compared));
  }
  const uint64_t no_line = orbit_object_utils_rust::GetLineInfoWithoutLineNumberCount();
  if (totals.line_lookups > 0) {
    printf("line results with no line number %llu\n",
           static_cast<unsigned long long>(no_line));
  }
  return 0;
}
