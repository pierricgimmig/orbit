// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "UserSpaceInstrumentation/InstrumentProcess.h"

#include <absl/algorithm/container.h>
#include <absl/base/casts.h>
#include <absl/container/flat_hash_set.h>
#include <absl/meta/type_traits.h>
#include <absl/strings/str_format.h>
#include <absl/types/span.h>
#include <capstone/capstone.h>
#include <dlfcn.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <mutex>
#include <set>
#include <string>
#include <string_view>
#include <thread>
#include <type_traits>
#include <utility>
#include <vector>

#include "AccessTraceesMemory.h"
#include "AllocateInTracee.h"
#include "ExecuteMachineCode.h"
#include "GrpcProtos/module.pb.h"
#include "MachineCode.h"
#include "ModuleUtils/ReadLinuxModules.h"
#include "ModuleUtils/VirtualAndAbsoluteAddresses.h"
#include "OrbitBase/ExecutablePath.h"
#include "OrbitBase/File.h"
#include "OrbitBase/GetProcessIds.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Profiling.h"
#include "OrbitBase/Result.h"
#include "OrbitBase/ThreadUtils.h"
#include "OrbitBase/UniqueResource.h"
#include "Trampoline.h"
#include "UserSpaceInstrumentation/AddressRange.h"
#include "UserSpaceInstrumentation/AnyThreadIsInStrictSeccompMode.h"
#include "UserSpaceInstrumentation/Attach.h"
#include "UserSpaceInstrumentation/ExecuteInProcess.h"
#include "UserSpaceInstrumentation/InjectLibraryInTracee.h"

namespace orbit_user_space_instrumentation {

namespace {

using orbit_grpc_protos::CaptureOptions;
using orbit_grpc_protos::ModuleInfo;

/* copybara:insert(In internal tests the library path depends on the current path)
// Since some part could potentially change the current working directory we store
// the initial value here.
const std::filesystem::path initial_current_path = std::filesystem::current_path();
*/
constexpr const char* kLibName = "liborbituserspaceinstrumentation.so";

ErrorMessageOr<std::filesystem::path> GetLibraryPath() {
  // When packaged, liborbituserspaceinstrumentation.so is found alongside OrbitService. In
  // development, it is found in "../lib", relative to OrbitService.
  const std::filesystem::path exe_dir = orbit_base::GetExecutableDir();
  std::vector<std::filesystem::path> potential_paths = {exe_dir / kLibName,
                                                        exe_dir / ".." / "lib" / kLibName};
  /* copybara:insert(In internal tests the library is in a different place)
  potential_paths.emplace_back(initial_current_path /
  "@@LIB_ORBIT_USER_SPACE_INSTRUMENTATION_PATH@@");
  */
  for (const auto& path : potential_paths) {
    if (std::filesystem::exists(path)) {
      return path;
    }
  }
  return ErrorMessage(absl::StrFormat("%s not found on system.", kLibName));
}

bool ProcessWithPidExists(pid_t pid) {
  const std::string pid_dirname = absl::StrFormat("/proc/%d", pid);
  auto result = orbit_base::FileOrDirectoryExists(pid_dirname);
  ORBIT_FAIL_IF(result.has_error(), "Accessing \"%s\" failed: %s", pid_dirname,
                result.error().message());
  return result.value();
}

// Returns true if liborbituserspaceinstrumentation.so is present in target process.
ErrorMessageOr<bool> AlreadyInjected(absl::Span<const ModuleInfo> modules) {
  for (const ModuleInfo& module : modules) {
    if (module.name() == kLibName) {
      return true;
    }
  }
  return false;
}

// We need to initialize some thread local memory when entering the payload functions. This leads to
// a situation where instrumenting the functions below would lead to a recursive call into the
// instrumentation. We just skip these and leave instrumenting them to the kernel/uprobe fallback.
bool IsBlocklisted(std::string_view function_name) {
  static const absl::flat_hash_set<std::string> kBlocklist{
      "__GI___libc_malloc",
      "__GI___libc_free",
      "get_free_list",
      "malloc_consolidate",
      "sysmalloc",
      "_int_malloc",
      "__libc_enable_asynccancel",
      "__GI___ctype_init",
      "__GI___mprotect",
      "__munmap",
      "new_heap",
      "__get_nprocs",
      "__get_nprocs_conf",
      "__strtoul",
      "arena_get2.part.3",
      "next_line",
      "__GI___libc_alloca_cutoff",
      "start_thread",
      "__pthread_enable_asynccancel",
      "__errno_location",
      "__memalign",
      "_mid_memalign",
      // There is some code in libc that that jumps to _GI_memcpy+0x3. If __GI_memcpy is
      // instrumented this location gets overwritten and we end up jumping to the middle of an
      // instruction.
      "__GI_memcpy",
  };
  return kBlocklist.contains(function_name);
}

ErrorMessageOr<void> WaitForThreadToExit(pid_t pid, pid_t tid) {
  // In all tests the thread exited in one to three rounds of waiting for one millisecond. To make
  // sure that we never stall OrbitService here we return an error when the thread requires an
  // excessive amount of time to exit.
  constexpr int kNumberOfRetries = 3000;
  int count = 0;
  auto tids = orbit_base::GetTidsOfProcess(pid);
  while (std::find(tids.begin(), tids.end(), tid) != tids.end()) {
    if (count++ > kNumberOfRetries) {
      return ErrorMessage("Initilization thread injected into target process failed to exit.");
    }
    std::this_thread::sleep_for(std::chrono::milliseconds{1});
    tids = orbit_base::GetTidsOfProcess(pid);
  }
  return outcome::success();
}

// The threads Orbit itself starts when liborbituserspaceinstrumentation.so is
// injected into the target process. Injection is complete once both are up.
constexpr std::array<std::string_view, 2> kOrbitThreadNames{"ConnectRcvCmds", "ForwarderThread"};

// Threads gRPC starts underneath them. Which of these exist, and how many of
// each, depends on the gRPC release: up to 1.72 it ran an executor, since 1.73
// it runs an event engine whose pool is sized by the number of cores. They are
// recognised so that the payload can skip them, but none is required to be
// present.
constexpr std::array<std::string_view, 6> kGrpcThreadNames{
    "grpc_global_tim",                     // Timer manager, present throughout.
    "event_engine",    "lifeguard",        // gRPC >= 1.73.
    "default-executo", "resolver-execut",  // gRPC <= 1.72.
    "grpcpp_sync_ser",
};

[[nodiscard]] bool IsOrbitThreadName(std::string_view name) {
  return absl::c_linear_search(kOrbitThreadNames, name) ||
         absl::c_linear_search(kGrpcThreadNames, name);
}

ErrorMessageOr<std::vector<pid_t>> GetNewOrbitThreads(
    pid_t pid, const absl::flat_hash_set<pid_t>& tids_before_injection) {
  // Waiting for one second was enough to have all the threads being spawned every single time
  // when running in the unit tests. Reducing the wait time to 900 ms lead to multiple rounds in the
  // loop.
  // However, tests with real target processes show that the threads usually spawn in ~90 ms with
  // very little variance. Presumably this is due to some initilization in grpc that already had
  // happened in the real target processes.
  // We choose a three second (300 x 10 ms) timeout and query the existing threads every 10 ms.
  constexpr int kNumberOfRetries = 300;
  constexpr std::chrono::milliseconds kWaitingPeriod{10};
  std::vector<pid_t> orbit_threads;
  int count = 0;
  while (true) {
    std::this_thread::sleep_for(kWaitingPeriod);
    orbit_threads.clear();
    absl::flat_hash_set<std::string_view> found_orbit_thread_names;
    for (const pid_t tid : orbit_base::GetTidsOfProcess(pid)) {
      if (tids_before_injection.contains(tid)) {
        continue;
      }
      const std::string tid_name = orbit_base::GetThreadName(tid);
      if (!IsOrbitThreadName(tid_name)) {
        continue;
      }
      orbit_threads.push_back(tid);
      const auto it = absl::c_find(kOrbitThreadNames, tid_name);
      if (it != kOrbitThreadNames.end()) found_orbit_thread_names.insert(*it);
    }
    if (found_orbit_thread_names.size() == kOrbitThreadNames.size()) {
      return orbit_threads;
    }
    if (count++ >= kNumberOfRetries) {
      return ErrorMessage(
          "Unable to find threads spawned by library injected for user space instrumentation.");
    }
  }
}

ErrorMessageOr<void> SetOrbitThreadsInTarget(pid_t pid, absl::Span<const ModuleInfo> modules,
                                             void* library_handle,
                                             const std::vector<pid_t>& orbit_threads) {
  ORBIT_CHECK(!orbit_threads.empty());
  constexpr const char* kAddOrbitThreadsFunctionName = "AddOrbitThreads";
  OUTCOME_TRY(void* add_orbit_threads_function_address,
              DlsymInTracee(pid, modules, library_handle, kAddOrbitThreadsFunctionName));

  // ExecuteInProcess passes six arguments in registers, so the ids go over in
  // batches of six; -1 pads the last, partially filled one.
  constexpr size_t kBatchSize = 6;
  for (size_t begin = 0; begin < orbit_threads.size(); begin += kBatchSize) {
    std::array<pid_t, kBatchSize> batch;
    batch.fill(-1);
    for (size_t i = 0; i < kBatchSize && begin + i < orbit_threads.size(); ++i) {
      batch[i] = orbit_threads[begin + i];
    }
    OUTCOME_TRY(ExecuteInProcess(pid, add_orbit_threads_function_address, batch[0], batch[1],
                                 batch[2], batch[3], batch[4], batch[5]));
  }
  return outcome::success();
}

// Given the path of a module in the process, get all loaded instances of that module (usually there
// will only be one, but a module can be loaded more than once).
ErrorMessageOr<std::vector<ModuleInfo>> ModulesFromModulePath(
    absl::Span<const ModuleInfo> modules, std::string_view path,
    absl::flat_hash_map<std::string, std::vector<ModuleInfo>>* cache_of_modules_from_path) {
  ORBIT_CHECK(cache_of_modules_from_path != nullptr);
  auto cached_modules_from_path_it = cache_of_modules_from_path->find(path);
  if (cached_modules_from_path_it == cache_of_modules_from_path->end()) {
    std::vector<ModuleInfo> result;
    for (const ModuleInfo& module : modules) {
      if (module.file_path() == path) {
        result.push_back(module);
      }
    }
    if (result.empty()) {
      return ErrorMessage(absl::StrFormat("Unable to find module for path \"%s\"", path));
    }
    cached_modules_from_path_it = cache_of_modules_from_path->emplace(path, result).first;
  }
  return cached_modules_from_path_it->second;
}

}  // namespace

// Holds all the data necessary to keep track of a process we instrument.
// Needs to be created via the static factory function `Create`. This will inject the shared library
// with our instrumentation code into the target process and create the return trampoline. Once
// created we can instrument functions in the target process and deactivate the instrumentation
// again (see `InstrumentFunctions`, `UninstrumentFunctions` below).
class InstrumentedProcess {
 public:
  InstrumentedProcess(const InstrumentedProcess&) = delete;
  InstrumentedProcess(InstrumentedProcess&&) = delete;
  InstrumentedProcess& operator=(const InstrumentedProcess&) = delete;
  InstrumentedProcess& operator=(InstrumentedProcess&&) = delete;
  ~InstrumentedProcess() = default;

  [[nodiscard]] static ErrorMessageOr<std::unique_ptr<InstrumentedProcess>> Create(
      pid_t pid, absl::Span<const ModuleInfo> modules);

  // Instruments the functions capture_options.instrumented_functions. Returns a set of
  // function_id's of successfully instrumented functions, a map of function_id's to errors for
  // functions that couldn't be instrumented, the address ranges dedicated to trampolines, and the
  // map name of the injected library.
  [[nodiscard]] ErrorMessageOr<InstrumentationManager::InstrumentationResult> InstrumentFunctions(
      const CaptureOptions& capture_options, absl::Span<const ModuleInfo> modules);

  // Removes the instrumentation for all functions in capture_options.instrumented_functions that
  // have been instrumented previously.
  [[nodiscard]] ErrorMessageOr<void> UninstrumentFunctions();

  // Returns the pid of the process.
  [[nodiscard]] pid_t GetPid() const { return pid_; }

 private:
  InstrumentedProcess() = default;

  // Returns an address where we can construct a new trampoline for some function in the module
  // identified by `address_range`. Handles the allocation in the tracee and the tracks the
  // allocated memory in `trampolines_for_modules_` below.
  [[nodiscard]] ErrorMessageOr<uint64_t> GetTrampolineMemory(AddressRange address_range);
  // Releases the address previously obtained by `GetTrampolineMemory` such that it can be reused.
  // Note that this must only be called once for each call to `GetTrampolineMemory`.
  [[nodiscard]] ErrorMessageOr<void> ReleaseMostRecentlyAllocatedTrampolineMemory(
      AddressRange address_range);

  [[nodiscard]] ErrorMessageOr<void> EnsureTrampolinesWritable();
  [[nodiscard]] ErrorMessageOr<void> EnsureTrampolinesExecutable();

  // Returns a vector of the address ranges dedicated to all entry trampolines for this process. The
  // number of address ranges is usually very small as kTrampolinesPerChunk is high.
  [[nodiscard]] std::vector<AddressRange> GetEntryTrampolineAddressRanges();

  pid_t pid_ = -1;

  uint64_t start_new_capture_function_address_ = 0;
  uint64_t entry_payload_function_address_ = 0;
  uint64_t exit_payload_function_address_ = 0;

  uint64_t return_trampoline_address_ = 0;

  // Keep track of each relocated instruction that has been moved into a trampoline. Used to move
  // the instruction pointers out of overwritten memory areas after the instrumentation has been
  // done.
  absl::flat_hash_map<uint64_t, uint64_t> relocation_map_;

  // Keep track of all trampolines we created for this process.
  struct TrampolineData {
    uint64_t trampoline_address = 0;
    uint64_t address_after_prologue = 0;
    // The first few bytes of the function. Guaranteed to contain everything that was overwritten.
    std::vector<uint8_t> function_data;
  };
  // Maps function addresses to TrampolineData.
  absl::flat_hash_map<uint64_t, TrampolineData> trampoline_map_;

  // Trampolines are allocated in chunks of kTrampolinesPerChunk. Trampolines are fixed size
  // (compare `GetMaxTrampolineSize`) and are never freed; we just allocate new chunks when that
  // last one is filled up. Each module (identified by its address range) gets it own sequence of
  // chunks (`trampolines_for_modules_`).
  static constexpr int kTrampolinesPerChunk = 4096;
  struct TrampolineMemoryChunk {
    TrampolineMemoryChunk() = default;
    TrampolineMemoryChunk(std::unique_ptr<MemoryInTracee> m, int first_available)
        : memory(std::move(m)), first_available(first_available) {}
    std::unique_ptr<MemoryInTracee> memory;
    int first_available = 0;
  };
  using TrampolineMemoryChunks = std::vector<TrampolineMemoryChunk>;
  absl::flat_hash_map<AddressRange, TrampolineMemoryChunks> trampolines_for_modules_;

  // When instrumenting a function we record the address here. This is used when we uninstrument: we
  // look up the original bytes in `trampoline_map_` above.
  absl::flat_hash_set<uint64_t> addresses_of_instrumented_functions_;

  // The absolute canonical path to the library injected into the target process. This path should
  // appear in the maps of the target process.
  std::filesystem::path injected_library_path_;
};

ErrorMessageOr<std::unique_ptr<InstrumentedProcess>> InstrumentedProcess::Create(
    pid_t pid, absl::Span<const ModuleInfo> modules) {
  std::unique_ptr<InstrumentedProcess> process(new InstrumentedProcess());
  process->pid_ = pid;
  ORBIT_LOG("Starting to instrument process with pid %d", pid);
  OUTCOME_TRY(AttachAndStopProcess(pid));
  orbit_base::unique_resource detach_on_exit_1{pid, [](int32_t pid) {
                                                 if (DetachAndContinueProcess(pid).has_error()) {
                                                   ORBIT_ERROR("Detaching from %i", pid);
                                                 }
                                               }};

  if (AnyThreadIsInStrictSeccompMode(pid)) {
    return ErrorMessage("At least one thread of the target process is in strict seccomp mode.");
  }

  // Inject library into target process.
  auto library_path_or_error = GetLibraryPath();
  if (library_path_or_error.has_error()) {
    return ErrorMessage(absl::StrFormat("Unable to get path to library: %s",
                                        library_path_or_error.error().message()));
  }
  const std::filesystem::path library_path = library_path_or_error.value();
  ORBIT_CHECK(library_path.is_absolute());
  process->injected_library_path_ = std::filesystem::canonical(library_path);
  ORBIT_LOG("Injecting library \"%s\" into process %d", process->injected_library_path_, pid);

  // If we already injected the library in a previous run of OrbitService we need to skip some of
  // the initialization below. However, we need to call dlopen again on the library. This will not
  // load the library again but merely return the handle to the existing one. We also need to
  // retrieve some function pointers from that library and create a new return trampoline for this
  // run of OrbitService.
  // The initialization part that we will skip is responsible for setting up the communication with
  // OrbitService and identifying the threads created in that process. All of that already happened
  // in the previous run.
  OUTCOME_TRY(const bool already_injected, AlreadyInjected(modules));

  auto library_handle_or_error =
      DlmopenInTracee(pid, modules, library_path, RTLD_NOW, LinkerNamespace::kCreateNewNamespace);
  if (library_handle_or_error.has_error()) {
    return ErrorMessage(absl::StrFormat("Unable to open library in tracee: %s",
                                        library_handle_or_error.error().message()));
  }
  void* library_handle = library_handle_or_error.value();

  // Get function pointers into the injected library.
  ORBIT_LOG("Resolving function pointers in injected library");
  constexpr const char* kStartNewCaptureFunctionName = "StartNewCapture";
  constexpr const char* kEntryPayloadFunctionName = "EntryPayload";
  constexpr const char* kExitPayloadFunctionName = "ExitPayload";
  OUTCOME_TRY(void* start_new_capture_function_address,
              DlsymInTracee(pid, modules, library_handle, kStartNewCaptureFunctionName));
  process->start_new_capture_function_address_ =
      absl::bit_cast<uint64_t>(start_new_capture_function_address);
  OUTCOME_TRY(void* entry_payload_function_address,
              DlsymInTracee(pid, modules, library_handle, kEntryPayloadFunctionName));
  process->entry_payload_function_address_ =
      absl::bit_cast<uint64_t>(entry_payload_function_address);
  OUTCOME_TRY(void* exit_payload_function_address,
              DlsymInTracee(pid, modules, library_handle, kExitPayloadFunctionName));
  process->exit_payload_function_address_ = absl::bit_cast<uint64_t>(exit_payload_function_address);

  // Get memory, create the return trampoline and make it executable.
  OUTCOME_TRY(auto&& return_trampoline_memory,
              MemoryInTracee::Create(pid, 0, GetReturnTrampolineSize()));
  process->return_trampoline_address_ = return_trampoline_memory->GetAddress();
  auto result = CreateReturnTrampoline(pid, process->exit_payload_function_address_,
                                       process->return_trampoline_address_);
  OUTCOME_TRY(return_trampoline_memory->EnsureMemoryExecutable());

  if (already_injected) {
    ORBIT_LOG(
        "Skipping initialization of instrumentation library since it was already present in the "
        "target process");
    return process;
  }

  // Keep track of the threads in the target process before we initialize the user space
  // instrumentation library.
  const auto tids_as_vector = orbit_base::GetTidsOfProcess(pid);
  const absl::flat_hash_set<pid_t> tids_before_injection(tids_as_vector.begin(),
                                                         tids_as_vector.end());

  // Let the injected library start a thread of its own and run the initialization code on it. The
  // thread is created by the target's own libc, with pthread_create, so that it has thread-local
  // storage of its own: a thread fabricated here with a raw clone syscall shares the storage of the
  // thread we halted, and the initialization -- gRPC allocating dynamic TLS, starting threads --
  // then works through the bookkeeping of a thread that runs at the same time.
  ORBIT_LOG("Initializing instrumentation library and setting up communication to OrbitService");
  constexpr const char* kInitializeInstrumentationFunctionName =
      "InitializeInstrumentationInNewThread";
  OUTCOME_TRY(void* initialize_instrumentation_function_address,
              DlsymInTracee(pid, modules, library_handle,
                            kInitializeInstrumentationFunctionName));
  // The code below is what ExecuteInProcess would generate, but the memory holding it is ours to
  // free: ExecuteInProcess frees its scratch memory as soon as the call returns, and the munmap
  // that takes needs a syscall instruction placed into a page of the target -- which is not safe to
  // do once the initialization thread this call starts is running. The memory is freed further
  // down, after we stopped the target again.
  // movabsq rax, initialize_instrumentation    48 b8 initialize_instrumentation
  // call rax                                   ff d0
  // int3                                       cc
  MachineCode code;
  code.AppendBytes({0x48, 0xb8})
      .AppendImmediate64(absl::bit_cast<uint64_t>(initialize_instrumentation_function_address))
      .AppendBytes({0xff, 0xd0})
      .AppendBytes({0xcc});
  OUTCOME_TRY(auto&& code_memory, MemoryInTracee::Create(pid, 0, code.GetResultAsVector().size()));
  OUTCOME_TRY(const uint64_t init_thread_tid_as_uint64, ExecuteMachineCode(*code_memory, code));
  const auto init_thread_tid = static_cast<pid_t>(init_thread_tid_as_uint64);
  if (init_thread_tid == -1) {
    return ErrorMessage(
        "Unable to start the initialization thread of the user space instrumentation library in "
        "the target process.");
  }

  // Manually detach such that we can wait for the initilization to finish and detect the newly
  // spawned threads.
  detach_on_exit_1.release();
  if (DetachAndContinueProcess(pid).has_error()) {
    return ErrorMessage(absl::StrFormat("Unable to detach from process %i", pid));
  }
  ORBIT_LOG("Waiting for initialization to complete");
  OUTCOME_TRY(WaitForThreadToExit(pid, init_thread_tid));
  OUTCOME_TRY(auto&& orbit_threads, GetNewOrbitThreads(pid, tids_before_injection));

  // Attach again in order to set the newly created thread ids and get rid of the allocated memory.
  OUTCOME_TRY(AttachAndStopProcess(pid));
  orbit_base::unique_resource detach_on_exit_2{pid, [](int32_t pid) {
                                                 if (DetachAndContinueProcess(pid).has_error()) {
                                                   ORBIT_ERROR("Detaching from %i", pid);
                                                 }
                                               }};
  OUTCOME_TRY(SetOrbitThreadsInTarget(pid, modules, library_handle, orbit_threads));
  OUTCOME_TRY(code_memory->Free());

  ORBIT_LOG("Initialization of instrumentation library done");

  return process;
}

ErrorMessageOr<InstrumentationManager::InstrumentationResult>
InstrumentedProcess::InstrumentFunctions(const CaptureOptions& capture_options,
                                         absl::Span<const ModuleInfo> modules) {
  ORBIT_LOG("Instrumenting functions in process %d", pid_);
  OUTCOME_TRY(AttachAndStopProcess(pid_));
  orbit_base::unique_resource detach_on_exit{pid_, [](int32_t pid) {
                                               if (DetachAndContinueProcess(pid).has_error()) {
                                                 ORBIT_ERROR("Detaching from %i", pid);
                                               }
                                             }};

  if (AnyThreadIsInStrictSeccompMode(pid_)) {
    return ErrorMessage("At least one thread of the target process is in strict seccomp mode.");
  }

  // Init Capstone disassembler.
  csh capstone_handle = 0;
  cs_err error_code = cs_open(CS_ARCH_X86, CS_MODE_64, &capstone_handle);
  if (error_code != CS_ERR_OK) {
    return ErrorMessage("Failed to open Capstone disassembler.");
  }
  error_code = cs_option(capstone_handle, CS_OPT_DETAIL, CS_OPT_ON);
  if (error_code != CS_ERR_OK) {
    return ErrorMessage("Failed to configure Capstone disassembler.");
  }
  orbit_base::unique_resource close_on_exit{
      &capstone_handle, [](csh* capstone_handle) { cs_close(capstone_handle); }};

  const uint64_t now = orbit_base::CaptureTimestampNs();
  ORBIT_LOG("Calling StartNewCapture at timestamp %d", now);
  OUTCOME_TRY(
      ExecuteInProcess(pid_, absl::bit_cast<void*>(start_new_capture_function_address_), now));

  OUTCOME_TRY(EnsureTrampolinesWritable());

  ORBIT_LOG("Trying to instrument %d functions", capture_options.instrumented_functions().size());
  InstrumentationManager::InstrumentationResult result;
  absl::flat_hash_map<std::string, std::vector<ModuleInfo>> cache_of_modules_from_path;
  for (const auto& function : capture_options.instrumented_functions()) {
    const uint64_t function_id = function.function_id();
    if (IsBlocklisted(function.function_name())) {
      const std::string message =
          absl::StrFormat("Can't instrument function \"%s\" since it is used internally by Orbit.",
                          function.function_name());
      ORBIT_ERROR("%s", message);
      result.function_ids_to_error_messages[function_id] = message;
      continue;
    }
    if (function.function_size() == 0) {
      const std::string message = absl::StrFormat(
          "Can't instrument function \"%s\" since it has size zero.", function.function_name());
      ORBIT_ERROR("%s", message);
      result.function_ids_to_error_messages[function_id] = message;
      continue;
    }
    // Get all modules with the right path (usually one, but might be more) and get a function
    // address to instrument for each of them.
    OUTCOME_TRY(auto&& function_modules,
                ModulesFromModulePath(modules, function.file_path(), &cache_of_modules_from_path));
    for (const auto& module : function_modules) {
      const uint64_t function_address = orbit_module_utils::SymbolVirtualAddressToAbsoluteAddress(
          function.function_virtual_address(), module.address_start(), module.load_bias(),
          module.executable_segment_offset());
      if (!trampoline_map_.contains(function_address)) {
        const AddressRange module_address_range(module.address_start(), module.address_end());
        auto trampoline_address_or_error = GetTrampolineMemory(module_address_range);
        if (trampoline_address_or_error.has_error()) {
          ORBIT_ERROR("Failed to allocate memory for trampoline: %s",
                      trampoline_address_or_error.error().message());
          continue;
        }
        const uint64_t trampoline_address = trampoline_address_or_error.value();
        // We need the machine code of the function for two purposes: We need to relocate the
        // instructions that get overwritten into the trampoline and we also need to check if the
        // function contains a jump back into the first five bytes (which would prohibit
        // instrumentation). For the first reason 20 bytes would be enough; the 200 is chosen
        // somewhat arbitrarily to cover all cases of jumps into the first five bytes we encountered
        // in the wild. Specifically this covers all relative jumps to a signed 8 bit offset.
        // Compare the comment of CheckForRelativeJumpIntoFirstFiveBytes in Trampoline.cpp.
        constexpr uint64_t kMaxFunctionReadSize = 200;
        const uint64_t function_read_size =
            std::min(kMaxFunctionReadSize, function.function_size());
        OUTCOME_TRY(auto&& function_data,
                    ReadTraceesMemory(pid_, function_address, function_read_size));
        auto address_after_prologue_or_error =
            CreateTrampoline(pid_, function_address, function_data, trampoline_address,
                             entry_payload_function_address_, return_trampoline_address_,
                             capstone_handle, relocation_map_);
        if (address_after_prologue_or_error.has_error()) {
          const std::string message = absl::StrFormat(
              "Can't instrument function \"%s\". Failed to create trampoline: %s",
              function.function_name(), address_after_prologue_or_error.error().message());
          ORBIT_ERROR("%s", message);
          result.function_ids_to_error_messages[function_id] = message;
          OUTCOME_TRY(ReleaseMostRecentlyAllocatedTrampolineMemory(module_address_range));
          continue;
        }
        TrampolineData trampoline_data;
        trampoline_data.trampoline_address = trampoline_address;
        // We'll overwrite the first five bytes of the function and the rest of the instruction that
        // we clobbered. Since we'll need to restore that when we remove the instrumentation we need
        // a backup.
        constexpr uint64_t kMaxFunctionBackupSize = 20;
        const uint64_t function_backup_size =
            std::min(kMaxFunctionBackupSize, function.function_size());
        OUTCOME_TRY(auto&& function_backup_data,
                    ReadTraceesMemory(pid_, function_address, function_backup_size));
        trampoline_data.function_data = function_backup_data;
        trampoline_data.address_after_prologue = address_after_prologue_or_error.value();
        trampoline_map_.emplace(function_address, trampoline_data);
      }
      auto it = trampoline_map_.find(function_address);
      if (it == trampoline_map_.end()) {
        continue;
      }
      const TrampolineData& trampoline_data = it->second;

      auto result_or_error = InstrumentFunction(pid_, function_address, function_id,
                                                trampoline_data.address_after_prologue,
                                                trampoline_data.trampoline_address);
      if (result_or_error.has_error()) {
        const std::string message =
            absl::StrFormat("Can't instrument function \"%s\": %s", function.function_name(),
                            result_or_error.error().message());
        ORBIT_ERROR("%s", message);
        result.function_ids_to_error_messages[function_id] = message;
      } else {
        addresses_of_instrumented_functions_.insert(function_address);
        result.instrumented_function_ids.insert(function_id);
      }
    }
  }
  ORBIT_LOG("Successfully instrumented %d functions", result.instrumented_function_ids.size());

  result.entry_trampoline_address_ranges = GetEntryTrampolineAddressRanges();
  result.return_trampoline_address_range = AddressRange{
      return_trampoline_address_, return_trampoline_address_ + GetReturnTrampolineSize()};
  result.injected_library_path = injected_library_path_;

  MoveInstructionPointersOutOfOverwrittenCode(pid_, relocation_map_);

  OUTCOME_TRY(EnsureTrampolinesExecutable());

  return result;
}

ErrorMessageOr<void> InstrumentedProcess::UninstrumentFunctions() {
  OUTCOME_TRY(AttachAndStopProcess(pid_));
  orbit_base::unique_resource detach_on_exit{pid_, [](int32_t pid) {
                                               if (DetachAndContinueProcess(pid).has_error()) {
                                                 ORBIT_ERROR("Detaching from %i", pid);
                                               }
                                             }};
  for (uint64_t function_address : addresses_of_instrumented_functions_) {
    auto it = trampoline_map_.find(function_address);
    // Skip if this function was not instrumented.
    if (it == trampoline_map_.end()) continue;
    const TrampolineData& trampoline_data = it->second;
    std::vector<uint8_t> code(trampoline_data.function_data.begin(),
                              trampoline_data.function_data.begin() +
                                  (trampoline_data.address_after_prologue - function_address));
    auto write_result_or_error = WriteTraceesMemory(pid_, function_address, code);
    ORBIT_FAIL_IF(write_result_or_error.has_error(), "%s", write_result_or_error.error().message());
  }
  return outcome::success();
}

ErrorMessageOr<uint64_t> InstrumentedProcess::GetTrampolineMemory(AddressRange address_range) {
  if (!trampolines_for_modules_.contains(address_range)) {
    trampolines_for_modules_.emplace(address_range, TrampolineMemoryChunks());
  }
  auto it = trampolines_for_modules_.find(address_range);
  TrampolineMemoryChunks& trampoline_memory_chunks = it->second;
  if (trampoline_memory_chunks.empty() ||
      trampoline_memory_chunks.back().first_available == kTrampolinesPerChunk) {
    OUTCOME_TRY(auto&& trampoline_memory,
                AllocateMemoryForTrampolines(pid_, address_range,
                                             kTrampolinesPerChunk * GetMaxTrampolineSize()));

    trampoline_memory_chunks.emplace_back(std::move(trampoline_memory), 0);
  }
  const uint64_t result = trampoline_memory_chunks.back().memory->GetAddress() +
                          trampoline_memory_chunks.back().first_available * GetMaxTrampolineSize();
  trampoline_memory_chunks.back().first_available++;
  return result;
}

ErrorMessageOr<void> InstrumentedProcess::ReleaseMostRecentlyAllocatedTrampolineMemory(
    AddressRange address_range) {
  if (!trampolines_for_modules_.contains(address_range)) {
    return ErrorMessage("Tried to release trampoline memory for a non existent address range");
  }
  trampolines_for_modules_.find(address_range)->second.back().first_available--;
  return outcome::success();
}

ErrorMessageOr<void> InstrumentedProcess::EnsureTrampolinesWritable() {
  for (auto& trampoline_for_module : trampolines_for_modules_) {
    for (auto& memory_chunk : trampoline_for_module.second) {
      OUTCOME_TRY(memory_chunk.memory->EnsureMemoryWritable());
    }
  }
  return outcome::success();
}

ErrorMessageOr<void> InstrumentedProcess::EnsureTrampolinesExecutable() {
  for (auto& trampoline_for_module : trampolines_for_modules_) {
    for (auto& memory_chunk : trampoline_for_module.second) {
      OUTCOME_TRY(memory_chunk.memory->EnsureMemoryExecutable());
    }
  }
  return outcome::success();
}

std::vector<AddressRange> InstrumentedProcess::GetEntryTrampolineAddressRanges() {
  std::vector<AddressRange> address_ranges;
  for (const auto& [unused_module_address_range, trampoline_memory_chunks] :
       trampolines_for_modules_) {
    for (const TrampolineMemoryChunk& trampoline_memory_chunk : trampoline_memory_chunks) {
      address_ranges.emplace_back(
          trampoline_memory_chunk.memory->GetAddress(),
          trampoline_memory_chunk.memory->GetAddress() + trampoline_memory_chunk.memory->GetSize());
    }
  }
  return address_ranges;
}

static std::mutex already_exists_mutex;
static bool already_exists = false;

std::unique_ptr<InstrumentationManager> InstrumentationManager::Create() {
  std::unique_lock<std::mutex> lock(already_exists_mutex);
  ORBIT_FAIL_IF(already_exists, "InstrumentationManager should be globally unique.");
  already_exists = true;
  return std::unique_ptr<InstrumentationManager>(new InstrumentationManager());
}

InstrumentationManager::~InstrumentationManager() {
  std::unique_lock<std::mutex> lock(already_exists_mutex);
  already_exists = false;
}

ErrorMessageOr<InstrumentationManager::InstrumentationResult>
InstrumentationManager::InstrumentProcess(const CaptureOptions& capture_options) {
  const pid_t pid = orbit_base::ToNativeProcessId(capture_options.pid());

  OUTCOME_TRY(auto&& exists, orbit_base::FileOrDirectoryExists(absl::StrFormat("/proc/%d", pid)));
  if (!exists) {
    return ErrorMessage(absl::StrFormat("There is no process with pid %d.", pid));
  }

  // If the user tries to instrument this instance of OrbitService we can't use user space
  // instrumentation: We would need to attach to / stop our own process.
  if (pid == getpid()) {
    return ErrorMessage("The target process is OrbitService itself.");
  }

  OUTCOME_TRY(auto&& modules, orbit_module_utils::ReadModules(pid));
  if (!process_map_.contains(pid)) {
    // Delete entries belonging to processes that are not running anymore.
    auto it = process_map_.begin();
    while (it != process_map_.end()) {
      if (!ProcessWithPidExists(it->second->GetPid())) {
        process_map_.erase(it++);
      } else {
        it++;
      }
    }

    auto process_or_error = InstrumentedProcess::Create(pid, modules);
    if (process_or_error.has_error()) {
      return ErrorMessage(absl::StrFormat("Unable to initialize process %d: %s", pid,
                                          process_or_error.error().message()));
    }
    process_map_.emplace(pid, std::move(process_or_error.value()));
  }
  OUTCOME_TRY(auto&& instrumentation_result,
              process_map_[pid]->InstrumentFunctions(capture_options, modules));

  return std::move(instrumentation_result);
}

ErrorMessageOr<void> InstrumentationManager::UninstrumentProcess(pid_t pid) {
  // If the user tries to instrument this instance of OrbitService we can't use user space
  // instrumentation: We would need to attach to / stop our own process. Therefore nothing was
  // instrumented in the first place and we can just return here.
  if (pid == getpid()) {
    return outcome::success();
  }

  if (process_map_.contains(pid)) {
    OUTCOME_TRY(process_map_[pid]->UninstrumentFunctions());
  }

  return outcome::success();
}

}  // namespace orbit_user_space_instrumentation
