#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Records the evidence for the C++ -> Rust port as a JSON snapshot.
#
#   scripts/port_metrics.sh                  # cheap metrics only, to stdout
#   scripts/port_metrics.sh --bazel          # also query the build graph (slow)
#   scripts/port_metrics.sh --bazel > docs/blog/metrics/phase-1.json
#
# Every number a blog post claims should come from a committed snapshot, so a
# reader can re-run this and get the same figures.

set -uo pipefail
cd "$(dirname "$0")/.."

WITH_BAZEL=0
[[ "${1:-}" == "--bazel" ]] && WITH_BAZEL=1

# ------------------------------------------------------------------ helpers

# Lines in the given paths, excluding test, mock and fuzz sources.
loc_impl() {
  find "$@" \( -name '*.cpp' -o -name '*.h' \) \
    ! -name '*Test*' ! -name 'Mock*' ! -name '*Fuzzer*' 2>/dev/null \
    | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1+0}'
}

loc_test() {
  find "$@" -name '*Test*.cpp' 2>/dev/null \
    | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1+0}'
}

loc_rust() {
  find "$@" -name '*.rs' ! -path '*/target/*' 2>/dev/null \
    | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1+0}'
}

count_occurrences() { grep -rho "$1" src rust 2>/dev/null | wc -l; }

gtest_cases() {
  grep -rhoE '^(TEST|TEST_F|TEST_P)\(' "$@" 2>/dev/null | wc -l
}

# deps(target) as a target count, and how many distinct external repos it pulls.
bazel_deps() {
  [[ $WITH_BAZEL -eq 0 ]] && { echo "null"; return; }
  bazel query "deps($1)" 2>/dev/null | wc -l
}

bazel_ext_repos() {
  [[ $WITH_BAZEL -eq 0 ]] && { echo "null"; return; }
  bazel query "deps($1)" 2>/dev/null \
    | grep -oP '^@+\K[^/]+' | sort -u | wc -l
}

json_str() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }

# --------------------------------------------------------------- the payload

cat <<JSON
{
  "generated_utc": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "commit": "$(git rev-parse --short HEAD 2>/dev/null || echo unknown)",
  "branch": "$(json_str "$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo unknown)")",
  "bazel_queried": $([[ $WITH_BAZEL -eq 1 ]] && echo true || echo false),

  "loc": {
    "cpp_impl_total": $(loc_impl src),
    "cpp_test_total": $(loc_test src),
    "rust_total": $(loc_rust src rust third_party),
    "rust_first_party": $(loc_rust rust src/OrbitLiveViewer),
    "by_module": {
      "ModuleUtils":              { "impl": $(loc_impl src/ModuleUtils),   "test": $(loc_test src/ModuleUtils) },
      "ObjectUtils":              { "impl": $(loc_impl src/ObjectUtils),   "test": $(loc_test src/ObjectUtils) },
      "Symbols":                  { "impl": $(loc_impl src/Symbols),       "test": $(loc_test src/Symbols) },
      "LinuxTracing":             { "impl": $(loc_impl src/LinuxTracing),  "test": $(loc_test src/LinuxTracing) },
      "CaptureFile":              { "impl": $(loc_impl src/CaptureFile),   "test": $(loc_test src/CaptureFile) },
      "OrbitBase":                { "impl": $(loc_impl src/OrbitBase),     "test": $(loc_test src/OrbitBase) },
      "UserSpaceInstrumentation": { "impl": $(loc_impl src/UserSpaceInstrumentation), "test": $(loc_test src/UserSpaceInstrumentation) }
    }
  },

  "reimplemented_std": {
    "comment": "C++ constructs Rust's std or a mainstream crate provides directly",
    "error_message_or_uses": $(count_occurrences 'ErrorMessageOr'),
    "orbit_check_uses": $(count_occurrences 'ORBIT_CHECK'),
    "std_optional_uses": $(count_occurrences 'std::optional'),
    "absl_mutex_uses": $(count_occurrences 'absl::Mutex'),
    "guarded_by_annotations": $(count_occurrences 'GUARDED_BY'),
    "orbit_base_std_equivalent_headers_loc": $(
      cd src/OrbitBase/include/OrbitBase 2>/dev/null && wc -l \
        Result.h Future.h FutureHelpers.h Promise.h PromiseHelpers.h Executor.h \
        ImmediateExecutor.h SimpleExecutor.h ThreadPool.h TaskGroup.h StopSource.h \
        StopToken.h SharedState.h WhenAll.h WhenAny.h AnyInvocable.h AnyMovable.h \
        Typedef.h TypedefUtils.h UniqueResource.h Overloaded.h NotFoundOr.h \
        CanceledOr.h Chunk.h Sort.h Align.h Append.h VoidToMonostate.h \
        MakeUniqueForOverwrite.h ParameterPackTrait.h Action.h 2>/dev/null \
        | tail -1 | awk '{print $1+0}')
  },

  "strangler": {
    "comment": "Methods in the Rust shims still forwarding to C++. Only ever goes down; zero means the C++ implementation can be deleted.",
    "rust_elf_file_delegating_methods": $(grep -c 'return cpp_->' rust/shims/ObjectUtilsRust/RustElfFile.cpp 2>/dev/null || echo 0),
    "rust_elf_file_ported_methods": $(grep -c 'override { return \(facts_\|build_id_\|soname_\|segments_\|gnu_debuglink_\|file_path_\|true\|false\)' rust/shims/ObjectUtilsRust/RustElfFile.cpp 2>/dev/null || echo 0)
  },

  "tests": {
    "cpp_test_files": $(find src -name '*Test*.cpp' | wc -l),
    "cases_module_utils": $(gtest_cases src/ModuleUtils/*Test.cpp),
    "cases_object_utils": $(gtest_cases src/ObjectUtils/*Test.cpp),
    "cases_linux_tracing": $(gtest_cases src/LinuxTracing/*Test.cpp)
  },

  "dependencies": {
    "bazel_dep_count": $(grep -c '^bazel_dep(' MODULE.bazel),
    "third_party_dirs": $(find third_party -mindepth 1 -maxdepth 1 -type d | wc -l),
    "llvm_dependents": $(grep -rl 'deps:llvm' src/*/BUILD.bazel 2>/dev/null | wc -l),
    "llvm_include_sites": $(grep -rl 'llvm/' src --include=*.cpp --include=*.h 2>/dev/null | wc -l),
    "bazel_patches": $(ls bazel/patches/*.patch 2>/dev/null | wc -l),
    "deps_object_utils": $(bazel_deps //src/ObjectUtils:ObjectUtils),
    "deps_module_utils": $(bazel_deps //src/ModuleUtils:ModuleUtils),
    "deps_orbit_service": $(bazel_deps //src/Service:OrbitService),
    "ext_repos_object_utils": $(bazel_ext_repos //src/ObjectUtils:ObjectUtils),
    "ext_repos_orbit_service": $(bazel_ext_repos //src/Service:OrbitService)
  }
}
JSON
