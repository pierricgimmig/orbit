#!/usr/bin/env bash
# Run the uprobe attach/detach bench (10/20/50 + tracer e2e) on a real uprobe machine.
#
#   ./src/LinuxTracingIntegrationTests/run_uprobe_attach_detach_bench.sh
#   ./src/LinuxTracingIntegrationTests/run_uprobe_attach_detach_bench.sh /path/to/LinuxTracingIntegrationTests
#   # or: sudo ORBIT_UPROBE_BENCH=1 ./bin/LinuxTracingIntegrationTests --gtest_filter='*UprobeAttachDetachBench*'
# Writes uprobe_attach_detach_bench.txt and uprobe_attach_detach_bench.html in cwd.
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "${SCRIPT_DIR}/../.." && pwd)

find_binary() {
  local candidates=(
    "${1:-}"
    "${PWD}/LinuxTracingIntegrationTests"
    "${PWD}/bin/LinuxTracingIntegrationTests"
    "${REPO_ROOT}/bin/LinuxTracingIntegrationTests"
    "${REPO_ROOT}/build/bin/LinuxTracingIntegrationTests"
    "${REPO_ROOT}/build/src/LinuxTracingIntegrationTests/LinuxTracingIntegrationTests"
    "${REPO_ROOT}/cmake-build-release/bin/LinuxTracingIntegrationTests"
    "${REPO_ROOT}/cmake-build-debug/bin/LinuxTracingIntegrationTests"
    "${REPO_ROOT}/build_gcc17_release/bin/LinuxTracingIntegrationTests"
    "${REPO_ROOT}/build_default/bin/LinuxTracingIntegrationTests"
    "${REPO_ROOT}/out/bin/LinuxTracingIntegrationTests"
  )
  local candidate
  for candidate in "${candidates[@]}"; do
    if [[ -n "${candidate}" && -f "${candidate}" && -x "${candidate}" ]]; then
      echo "${candidate}"
      return 0
    fi
  done
  return 1
}

EXTRA_ARGS=()
if [[ $# -ge 1 && -e "$1" ]]; then
  BIN=$1
  shift
  EXTRA_ARGS=("$@")
else
  EXTRA_ARGS=("$@")
  if ! BIN=$(find_binary ""); then
    echo "Could not find LinuxTracingIntegrationTests. Pass the binary path:" >&2
    echo "  $0 /path/to/LinuxTracingIntegrationTests" >&2
    exit 1
  fi
fi

if [[ ! -x "${BIN}" ]]; then
  echo "Not an executable: ${BIN}" >&2
  exit 1
fi

BIN=$(cd "$(dirname "${BIN}")" && pwd)/$(basename "${BIN}")
FILTER='*UprobeAttachDetachBench*'
export ORBIT_UPROBE_BENCH=1

if [[ "$(id -u)" -ne 0 ]]; then
  exec sudo ORBIT_UPROBE_BENCH=1 "${BIN}" --gtest_filter="${FILTER}" "${EXTRA_ARGS[@]}"
fi

exec "${BIN}" --gtest_filter="${FILTER}" "${EXTRA_ARGS[@]}"
