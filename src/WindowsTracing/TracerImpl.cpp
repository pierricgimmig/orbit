// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "TracerImpl.h"

#include "GrpcProtos/module.pb.h"
#include "ModuleUtils/VirtualAndAbsoluteAddresses.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Profiling.h"
#include "WindowsTracing/ListModulesETW.h"
#include "WindowsUtils/ListModules.h"
#include "WindowsUtils/ListThreads.h"

using orbit_grpc_protos::ModuleInfo;
using orbit_grpc_protos::ModulesSnapshot;
using orbit_grpc_protos::ThreadName;
using orbit_grpc_protos::ThreadNamesSnapshot;
using orbit_windows_utils::Module;
using orbit_windows_utils::Thread;

#define PRINT_STR(x) ORBIT_LOG(#x " : %s", x)
#define PRINT_VAR(x) ORBIT_LOG(#x " : %s", std::to_string(x))
#define PRINT_HEX(x) ORBIT_LOG(#x " : 0x%x", x)

namespace orbit_windows_tracing {

namespace {

std::vector<Module> GetModules(uint32_t pid) {
  std::vector<Module> modules = orbit_windows_utils::ListModules(pid);
  if (modules.empty()) {
    // Fallback on etw module enumeration which involves more work.
    modules = ListModulesEtw(pid);
  }

  if (modules.empty()) {
    ORBIT_ERROR("Unable to load modules for %u", pid);
  }
  return modules;
}

[[nodiscard]] absl::flat_hash_map<std::string, std::vector<Module>> GetModulesByPath(
    uint32_t pid) {
  absl::flat_hash_map<std::string, std::vector<Module>> result;
  std::vector<Module> modules = GetModules(pid);
  for (const Module& module : modules) {
    result[module.full_path].emplace_back(module);
  }
  return result;
}

}  // namespace

TracerImpl::TracerImpl(orbit_grpc_protos::CaptureOptions capture_options, TracerListener* listener)
    : capture_options_(std::move(capture_options)), listener_(listener) {}

void TracerImpl::Start() {
  ORBIT_CHECK(krabs_tracer_ == nullptr);
  SendModulesSnapshot();
  SendThreadNamesSnapshot();
  krabs_tracer_ = std::make_unique<KrabsTracer>(capture_options_.pid(),
                                                capture_options_.samples_per_second(), listener_);
  krabs_tracer_->Start();

  absl::flat_hash_map<std::string, std::vector<Module>> modules_by_path =
      GetModulesByPath(capture_options_.pid());
  
  std::vector<FunctionToInstrument> functions_to_instrument;
  for (const auto& function : capture_options_.instrumented_functions()) {
    for (const Module& module : modules_by_path[function.file_path()]) {
      const uint64_t function_address = orbit_module_utils::SymbolVirtualAddressToAbsoluteAddress(
          function.function_virtual_address(), module.address_start, module.load_bias,
          /*module_executable_section_offset=*/0);

      FunctionToInstrument& function_to_instrument = functions_to_instrument.emplace_back();
      function_to_instrument.absolute_virtual_address = function_address;
      function_to_instrument.function_name = function.function_name();
      function_to_instrument.function_size = function.function_size();
      function_to_instrument.module_full_path = function.file_path();

      ORBIT_LOG("Instrumented function: %s", function.function_name());
      PRINT_STR(function.file_path());
      PRINT_STR(function.file_build_id());
      PRINT_HEX(function.file_offset());
      PRINT_HEX(function.function_id());
      PRINT_HEX(function.function_virtual_address());
      PRINT_VAR(function.function_size());
      PRINT_HEX(function_address);
    }
  }
  
  dynamic_instrumentation_ = std::make_unique<DynamicInstrumentation>(capture_options_.pid());
  dynamic_instrumentation_->Start();
}

void TracerImpl::Stop() {
  ORBIT_CHECK(dynamic_instrumentation_ != nullptr);
  dynamic_instrumentation_->Stop();
  dynamic_instrumentation_ = nullptr;
  ORBIT_CHECK(krabs_tracer_ != nullptr);
  krabs_tracer_->Stop();
  krabs_tracer_ = nullptr;
}

void TracerImpl::SendModulesSnapshot() {
  uint32_t pid = capture_options_.pid();
  ModulesSnapshot modules_snapshot;
  modules_snapshot.set_pid(pid);
  modules_snapshot.set_timestamp_ns(orbit_base::CaptureTimestampNs());

  std::vector<Module> modules = GetModules(pid);
  if (modules.empty()) {
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

}  // namespace orbit_windows_tracing
