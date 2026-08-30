// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Runtime selection between the C++ and Rust implementations of ElfFile.
//
// ORBIT_OBJECT_BACKEND=cpp   (default, and what an unset variable means)
//                      rust
//                      both  construct both and ORBIT_CHECK that every ported
//                            method agrees
//
// The Rust ElfFile owns a C++ one and forwards the methods that have not been
// ported yet, so this file always builds both. That is what keeps the Rust
// target passing the whole suite from the first commit while the port is only
// partly done.

#include "ElfFileBackend.h"

#include <stdlib.h>

#include <memory>
#include <string>
#include <string_view>
#include <utility>

#include "OrbitBase/File.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/ReadFileToString.h"
#include "OrbitBase/Result.h"

#ifdef __linux
#include "RustCoffFile.h"
#include "RustElfFile.h"
#endif

namespace orbit_object_utils {

namespace {

[[nodiscard]] ObjectBackend ReadBackendFromEnvironment() {
#ifdef __linux
  const char* value = getenv("ORBIT_OBJECT_BACKEND");
  if (value == nullptr) return ObjectBackend::kCpp;

  const std::string_view backend{value};
  if (backend == "rust") return ObjectBackend::kRust;
  if (backend == "both") return ObjectBackend::kBoth;
  if (backend != "cpp" && !backend.empty()) {
    ORBIT_ERROR("Unrecognised ORBIT_OBJECT_BACKEND=\"%s\"; using \"cpp\"", backend);
  }
#endif
  return ObjectBackend::kCpp;
}

#ifdef __linux

// Builds the Rust-backed ElfFile over `buffer`, with a C++ ElfFile behind it
// for the methods that still delegate.
//
// Both implementations must agree about whether the file is loadable at all,
// so in `both` mode a disagreement about success is itself a failure.
[[nodiscard]] ErrorMessageOr<std::unique_ptr<ElfFile>> CreateRustBacked(
    const std::filesystem::path& file_path, const void* data, size_t len,
    ErrorMessageOr<std::unique_ptr<ElfFile>> cpp_result, bool compare) {
  if (compare) {
    // The backends must agree on whether the file is loadable at all, not just
    // on the values they report for one that is.
    std::string rust_error;
    const bool rust_ok = orbit_object_utils_rust::RustElfParses(file_path, data, len, &rust_error);
    if (rust_ok != cpp_result.has_value()) {
      ORBIT_FATAL(
          "ElfFile backends disagree on whether \"%s\" loads: cpp=%s rust=%s\n"
          "  cpp error:  %s\n  rust error: %s",
          file_path.string(), cpp_result.has_value() ? "ok" : "error", rust_ok ? "ok" : "error",
          cpp_result.has_value() ? "-" : cpp_result.error().message(),
          rust_ok ? "-" : rust_error);
    }
  }

  // The delegate is required while methods still forward, so the Rust path
  // cannot succeed where the C++ fails. Returning the C++ error also keeps the
  // exact messages ElfFileTest matches on.
  if (cpp_result.has_error()) return cpp_result.error();

  return orbit_object_utils_rust::CreateRustElfFile(file_path, std::move(cpp_result.value()), data,
                                                    len, compare);
}

#endif  // __linux

#ifdef __linux

// Same shape as CreateRustBacked, for PE images.
[[nodiscard]] ErrorMessageOr<std::unique_ptr<CoffFile>> CreateRustBackedCoff(
    const std::filesystem::path& file_path, const void* data, size_t len,
    ErrorMessageOr<std::unique_ptr<CoffFile>> cpp_result, bool compare) {
  if (compare) {
    std::string rust_error;
    const bool rust_ok = orbit_object_utils_rust::RustCoffParses(file_path, data, len, &rust_error);
    if (rust_ok != cpp_result.has_value()) {
      ORBIT_FATAL(
          "CoffFile backends disagree on whether \"%s\" loads: cpp=%s rust=%s\n"
          "  cpp error:  %s\n  rust error: %s",
          file_path.string(), cpp_result.has_value() ? "ok" : "error", rust_ok ? "ok" : "error",
          cpp_result.has_value() ? "-" : cpp_result.error().message(),
          rust_ok ? "-" : rust_error);
    }
  }

  if (cpp_result.has_error()) return cpp_result.error();

  return orbit_object_utils_rust::CreateRustCoffFile(file_path, std::move(cpp_result.value()),
                                                     data, len, compare);
}

#endif  // __linux

}  // namespace

ObjectBackend SelectedObjectBackend() {
  static const ObjectBackend backend = ReadBackendFromEnvironment();
  return backend;
}

ErrorMessageOr<std::unique_ptr<ElfFile>> CreateElfFile(const std::filesystem::path& file_path) {
  const ObjectBackend backend = SelectedObjectBackend();
  if (backend == ObjectBackend::kCpp) {
    return CreateElfFileCpp(file_path);
  }

#ifdef __linux
  // Rust does not open files. Read the bytes here and hand over a view, which
  // also means the two backends see exactly the same input.
  ErrorMessageOr<std::string> content = orbit_base::ReadFileToString(file_path);
  if (content.has_error()) {
    // Fall back so that a read failure produces the same error the C++ would.
    return CreateElfFileCpp(file_path);
  }

  return CreateRustBacked(file_path, content.value().data(), content.value().size(),
                          CreateElfFileCpp(file_path), backend == ObjectBackend::kBoth);
#else
  return CreateElfFileCpp(file_path);
#endif
}

ErrorMessageOr<std::unique_ptr<CoffFile>> CreateCoffFile(const std::filesystem::path& file_path) {
  const ObjectBackend backend = SelectedObjectBackend();
  if (backend == ObjectBackend::kCpp) {
    return CreateCoffFileCpp(file_path);
  }

#ifdef __linux
  ErrorMessageOr<std::string> content = orbit_base::ReadFileToString(file_path);
  if (content.has_error()) {
    return CreateCoffFileCpp(file_path);
  }

  return CreateRustBackedCoff(file_path, content.value().data(), content.value().size(),
                              CreateCoffFileCpp(file_path), backend == ObjectBackend::kBoth);
#else
  return CreateCoffFileCpp(file_path);
#endif
}

ErrorMessageOr<std::unique_ptr<ElfFile>> CreateElfFileFromBuffer(
    const std::filesystem::path& file_path, const void* buf, size_t len) {
  const ObjectBackend backend = SelectedObjectBackend();
  if (backend == ObjectBackend::kCpp) {
    return CreateElfFileFromBufferCpp(file_path, buf, len);
  }

#ifdef __linux
  return CreateRustBacked(file_path, buf, len, CreateElfFileFromBufferCpp(file_path, buf, len),
                          backend == ObjectBackend::kBoth);
#else
  return CreateElfFileFromBufferCpp(file_path, buf, len);
#endif
}

}  // namespace orbit_object_utils
