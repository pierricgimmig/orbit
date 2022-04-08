// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "TracerImpl.h"

#include "GrpcProtos/module.pb.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Profiling.h"
#include "WindowsTracing/ListModulesETW.h"
#include "WindowsApiTracing.h"
#include "WindowsUtils/CreateProcess.h"
#include "WindowsUtils/ListModules.h"
#include "WindowsUtils/ListThreads.h"
#include "win32/manifest.h"

using orbit_grpc_protos::ModuleInfo;
using orbit_grpc_protos::ModulesSnapshot;
using orbit_grpc_protos::ProcessToLaunch;
using orbit_grpc_protos::ThreadName;
using orbit_grpc_protos::ThreadNamesSnapshot;
using orbit_windows_utils::BusyLoopInfo;
using orbit_windows_utils::BusyLoopLauncher;
using orbit_windows_utils::Module;
using orbit_windows_utils::Thread;

namespace orbit_windows_tracing {

namespace {

template <typename T>
void LogIfError(const ErrorMessageOr<T>& result, std::string_view message) {
  if (result.has_error()) {
    ORBIT_LOG("%s %s", message, result.error().message());
  }
}

}  // namespace

TracerImpl::TracerImpl(orbit_grpc_protos::CaptureOptions capture_options, TracerListener* listener)
    : capture_options_(std::move(capture_options)), listener_(listener) {
  LogIfError(LaunchProcessIfNeeded(), "Error launching process");
  InitializeWindowsApiTracing();
}

ErrorMessageOr<void> TracerImpl::ResumeProcessIfNeeded() {
  if (busy_loop_launcher_ && busy_loop_launcher_->IsProcessSuspended()) {
    OUTCOME_TRY(busy_loop_launcher_->ResumeMainThread());
  }
  return outcome::success();
}

ErrorMessageOr<void> TracerImpl::LaunchProcessIfNeeded() {
  const ProcessToLaunch& process_to_launch = capture_options_.proces_to_launch();
  if (process_to_launch.pause_on_entry_point()) {
    busy_loop_launcher_ = std::make_unique<orbit_windows_utils::BusyLoopLauncher>();
    OUTCOME_TRY(BusyLoopInfo busy_loop_info,
                busy_loop_launcher_->StartWithBusyLoopAtEntryPoint(
                    process_to_launch.executable_path(), process_to_launch.working_directory(),
                    process_to_launch.arguments()));
    launched_process_id_ = busy_loop_info.process_id;
  } else {
  }
  return outcome::success();
}

void TracerImpl::Start() {
  ORBIT_CHECK(krabs_tracer_ == nullptr);
  SendModulesSnapshot();
  SendThreadNamesSnapshot();
  krabs_tracer_ = std::make_unique<KrabsTracer>(capture_options_.pid(),
                                                capture_options_.samples_per_second(), listener_);
  krabs_tracer_->Start();
  LogIfError(ResumeProcessIfNeeded(), "Error resuming process");
}

void TracerImpl::Stop() {
  ORBIT_CHECK(krabs_tracer_ != nullptr);
  krabs_tracer_->Stop();
  krabs_tracer_ = nullptr;
}

void TracerImpl::SendModulesSnapshot() {
  uint32_t pid = capture_options_.pid();
  ModulesSnapshot modules_snapshot;
  modules_snapshot.set_pid(pid);
  modules_snapshot.set_timestamp_ns(orbit_base::CaptureTimestampNs());

  std::vector<Module> modules = orbit_windows_utils::ListModules(pid);
  if (modules.empty()) {
    // Fallback on etw module enumeration which involves more work.
    modules = ListModulesEtw(pid);
  }

  if (modules.empty()) {
    ORBIT_ERROR("Unable to load modules for %u", pid);
    return;
  }

  for (const Module& module : modules) {
    ModuleInfo* module_info = modules_snapshot.add_modules();
    module_info->set_name(module.name);
    module_info->set_file_path(module.full_path);
    module_info->set_address_start(module.address_start);
    module_info->set_address_end(module.address_end);
    module_info->set_build_id(module.build_id);
    module_info->set_load_bias(module.load_bias);
    module_info->set_object_file_type(ModuleInfo::kCoffFile);
    for (const ModuleInfo::ObjectSegment& section : module.sections) {
      *module_info->add_object_segments() = section;
    }
  }

  listener_->OnModulesSnapshot(std::move(modules_snapshot));
}

void TracerImpl::SendThreadNamesSnapshot() {
  uint64_t timestamp = orbit_base::CaptureTimestampNs();
  ThreadNamesSnapshot thread_names_snapshot;
  thread_names_snapshot.set_timestamp_ns(timestamp);

  std::vector<Thread> threads = orbit_windows_utils::ListAllThreads();
  if (threads.empty()) {
    ORBIT_ERROR("Unable to list threads");
    return;
  }

  for (const Thread& thread : threads) {
    ThreadName* thread_name = thread_names_snapshot.add_thread_names();
    thread_name->set_pid(thread.pid);
    thread_name->set_tid(thread.tid);
    thread_name->set_name(thread.name);
    thread_name->set_timestamp_ns(timestamp);
  }

  listener_->OnThreadNamesSnapshot(std::move(thread_names_snapshot));
}

void TracerImpl::InitializeWindowsApiTracing() {
  // TODO: properly initialize "enable_platform_api" bool.
  bool platform_api_active = capture_options_.enable_platform_api() || true;
  if (platform_api_active) {
    auto result = InitializeWindowsApiTracingInTarget(capture_options_.pid());
    if (result.has_error()) {
      ERROR("Initializing Windows Api tracing: %s", result.error().message());
    }
  }
}

}  // namespace orbit_windows_tracing
