// Copyright (c) 2022 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "WindowsUtils/Debugger.h"

#include "OrbitBase/GetLastError.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/StringConversion.h"
#include "OrbitBase/ThreadUtils.h"
#include "WindowsUtils/CreateProcess.h"
#include "WindowsUtils/SafeHandle.h"

//#pragma optimize("", off)

namespace orbit_windows_utils {

Debugger::~Debugger() { Wait(); }

ErrorMessageOr<ProcessInfo> Debugger::Start(std::filesystem::path executable,
                                            std::filesystem::path working_directory,
                                            std::string_view arguments) {
  // Launch process and start debugging loop on the same separate thread.
  thread_ = std::thread(&Debugger::DebuggerThread, this, executable, working_directory,
                        std::string(arguments));

  // Return result from process creation which happens in another thread. This will wait until the
  // thread has been created and has called CreateProcess.
  return std::move(process_info_promise_.GetFuture().Get());
}

void Debugger::Detach() {
  detach_requested_ = true;
  ;
}

void Debugger::Wait() {
  if (thread_.joinable()) thread_.join();
}

void Debugger::DebuggerThread(std::filesystem::path executable,
                              std::filesystem::path working_directory, std::string arguments) {
  orbit_base::SetCurrentThreadName("OrbitDebugger");

  // Only the thread that created the process being debugged can call WaitForDebugEvent.
  auto process_info_or_error = CreateProcessToDebug(executable, working_directory, arguments);

  bool has_error = process_info_or_error.has_error();
  uint32_t process_id = !has_error ? process_info_or_error.value().process_id : 0;
  process_info_promise_.SetResult(std::move(process_info_or_error));
  if (has_error) {
    return;
  }

  DEBUG_EVENT debug_event = {0};
  bool continue_debugging = true;
  DWORD continue_status = DBG_CONTINUE;
  constexpr uint32_t kWaitForDebugEventMs = 500;
  // "The semaphore timeout period has expired".
  constexpr uint32_t kSemaphoreExpiredErrorCode = 121;

  while (continue_debugging) {
    if (!WaitForDebugEvent(&debug_event, kWaitForDebugEventMs)) {
      if (::GetLastError() != kSemaphoreExpiredErrorCode) {
        ORBIT_ERROR("Calling \"WaitForDebugEvent\": %s", orbit_base::GetLastErrorAsString());
        detach_requested_ = true;
      }

      if (detach_requested_) {
        DebugActiveProcessStop(process_id);
        break;
      }
      continue;
    }

    switch (debug_event.dwDebugEventCode) {
      case CREATE_PROCESS_DEBUG_EVENT:
        OnCreateProcessDebugEvent(debug_event);
        break;
      case EXIT_PROCESS_DEBUG_EVENT:
        OnExitProcessDebugEvent(debug_event);
        Detach();
        break;
      case CREATE_THREAD_DEBUG_EVENT:
        OnCreateThreadDebugEvent(debug_event);
        break;
      case EXIT_THREAD_DEBUG_EVENT:
        OnExitThreadDebugEvent(debug_event);
        break;
      case LOAD_DLL_DEBUG_EVENT:
        OnLoadDllDebugEvent(debug_event);
        break;
      case UNLOAD_DLL_DEBUG_EVENT:
        OnUnLoadDllDebugEvent(debug_event);
        break;
      case OUTPUT_DEBUG_STRING_EVENT:
        OnOutputStringDebugEvent(debug_event);
        break;
      case RIP_EVENT:
        OnRipEvent(debug_event);
        break;
      case EXCEPTION_DEBUG_EVENT: {
        EXCEPTION_DEBUG_INFO& exception = debug_event.u.Exception;
        if (exception.ExceptionRecord.ExceptionCode == STATUS_BREAKPOINT) {
          OnBreakpointDebugEvent(debug_event);
        } else {
          OnExceptionDebugEvent(debug_event);
          if (exception.dwFirstChance == 1) {
            ORBIT_LOG("First chance exception at %x, exception-code: 0x%08x",
                      exception.ExceptionRecord.ExceptionAddress,
                      exception.ExceptionRecord.ExceptionCode);
          }
          continue_status = DBG_EXCEPTION_NOT_HANDLED;
        }
        break;
      }
      default:
        ORBIT_ERROR("Unhandled debugger event code: %u", debug_event.dwDebugEventCode);
    }

    ContinueDebugEvent(debug_event.dwProcessId, debug_event.dwThreadId, continue_status);
    continue_status = DBG_CONTINUE;
  }
}

}  // namespace orbit_windows_utils
