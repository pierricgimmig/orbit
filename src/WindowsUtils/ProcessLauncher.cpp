// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsUtils/ProcessLauncher.h"

#include <absl/base/casts.h>

#include "OrbitBase/GetLastError.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/ThreadUtils.h"
#include "WindowsUtils/ReadProcessMemory.h"
#include "WindowsUtils/SafeHandle.h"
#include "WindowsUtils/WriteProcessMemory.h"

namespace orbit_windows_utils {

namespace {

ErrorMessage GetLastErrorWithPrefix(std::string_view prefix) {
  return ErrorMessage(absl::StrFormat("%s: %s", prefix, orbit_base::GetLastErrorAsString()));
}

ErrorMessageOr<void> FlushInstructionCache(HANDLE process_handle, void* address, size_t size) {
  if (::FlushInstructionCache(process_handle, address, size) == 0) {
    return GetLastErrorWithPrefix("Calling \"FlushInstructionCache\"");
  }
  return outcome::success();
}

ErrorMessageOr<BusyLoopInfo> InstallBusyLoopAtEntryPoint(HANDLE process_handle,
                                                         HANDLE entry_point_thread_handle,
                                                         void* entry_point_address) {
  constexpr const char kBusyLoopCode[] = {0xEB, 0xFE};
  BusyLoopInfo busy_loop;
  busy_loop.address = absl::bit_cast<uint64_t>(entry_point_address);
  busy_loop.process_id = GetProcessId(process_handle);
  busy_loop.thread_id = GetThreadId(entry_point_thread_handle);

  // Copy original bytes before installing busy loop.
  OUTCOME_TRY(
      busy_loop.original_bytes,
      ReadProcessMemory(GetProcessId(process_handle), busy_loop.address, sizeof(kBusyLoopCode)));

  // Install busy loop.
  void* entry_point = absl::bit_cast<void*>(entry_point_address);
  OUTCOME_TRY(WriteProcessMemory(process_handle, entry_point, kBusyLoopCode));

  // Flush instruction cache.
  OUTCOME_TRY(FlushInstructionCache(process_handle, entry_point, sizeof(kBusyLoopCode)));

  return busy_loop;
}

ErrorMessageOr<void> SetThreadInstructionPointer(HANDLE thread_handle,
                                                 uint64_t instruction_pointer) {
  CONTEXT c = {0};
  c.ContextFlags = CONTEXT_CONTROL;
  if (!GetThreadContext(thread_handle, &c)) {
    return GetLastErrorWithPrefix("Calling \"GetThreadContext\"");
  }

  c.Rip = instruction_pointer;

  if (SetThreadContext(thread_handle, &c) == 0) {
    return GetLastErrorWithPrefix("Calling \"SetThreadContext\"");
  }

  return outcome::success();
}

ErrorMessageOr<void> WriteInstructionsAtAddress(const std::string& byte_buffer, void* address) {
  // Mark memory page for writing.
  DWORD old_protect = 0;
  if (!VirtualProtect(address, byte_buffer.size(), PAGE_EXECUTE_READWRITE, &old_protect)) {
    return GetLastErrorWithPrefix("Calling \"VirtualProtect\"");
  }

  // Write original bytes.
  memcpy(address, byte_buffer.data(), byte_buffer.size());

  // Restore original page protection.
  if (!VirtualProtect(address, byte_buffer.size(), old_protect, &old_protect)) {
    return GetLastErrorWithPrefix("Calling \"VirtualProtect\"");
  }

  OUTCOME_TRY(FlushInstructionCache(GetCurrentProcess(), address, byte_buffer.size()));
  return outcome::success();
}

}  // namespace

ProcessLauncher::ProcessLauncher() : debugger_({this}) {}

ErrorMessageOr<BusyLoopInfo> ProcessLauncher::StartWithBusyLoopAtEntryPoint(
    const std::filesystem::path& executable, const std::filesystem::path& working_directory,
    const std::string_view arguments) {
  OUTCOME_TRY(debugger_.Start(executable, working_directory, arguments));

  // The "busy_loop_info_or_error_promise_"'s result is set when the debugger is notified of the
  // process creation in "OnCreateProcessDebugEvent".
  return busy_loop_info_or_error_promise_.GetFuture().Get();
}

void ProcessLauncher::OnCreateProcessDebugEvent(const DEBUG_EVENT& event) {
  // Try installing a busy loop at entry point.
  busy_loop_info_or_error_promise_.SetResult(InstallBusyLoopAtEntryPoint(
      event.u.CreateProcessInfo.hProcess, event.u.CreateProcessInfo.hThread,
      event.u.CreateProcessInfo.lpStartAddress));

  // At this point, we don't need to continue debugging the process.
  debugger_.Detach();
}

// From within the target process, restore original bytes in the main thread that is busy-looping at
// the entry point.
ErrorMessageOr<void> RemoveBusyLoopAndSuspendThread(const BusyLoopInfo& busy_loop_info) {
  // Get the busy-loop thread handle.
  SafeHandle thread_handle(OpenThread(THREAD_ALL_ACCESS, FALSE, busy_loop_info.thread_id));
  if (*thread_handle == nullptr) {
    return GetLastErrorWithPrefix("Calling \"OpenThread\"");
  }

  // Suspend the busy-loop thread.
  if (SuspendThread(*thread_handle) == (DWORD)-1) {
    return GetLastErrorWithPrefix("Calling \"SuspendThread\"");
  }

  // Remove the busy loop and restore the original bytes.
  void* busy_loop_address = absl::bit_cast<void*>(busy_loop_info.address);
  OUTCOME_TRY(WriteInstructionsAtAddress(busy_loop_info.original_bytes, busy_loop_address));

  // Make sure the instruction pointer is back to the entry point.
  OUTCOME_TRY(SetThreadInstructionPointer(*thread_handle, busy_loop_info.address));

  return outcome::success();
}

ErrorMessageOr<void> ResumeThread(uint32_t thread_id) {
  SafeHandle thread_handle(OpenThread(THREAD_ALL_ACCESS, FALSE, thread_id));
  if (*thread_handle == nullptr) {
    return GetLastErrorWithPrefix("Calling \"OpenThread\"");
  }

  if (::ResumeThread(*thread_handle) == (DWORD)-1) {
    return GetLastErrorWithPrefix("Calling \"OpenThread\"");
  }

  return outcome::success();
}

}  // namespace orbit_windows_utils
