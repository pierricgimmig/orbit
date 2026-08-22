// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <absl/container/flat_hash_map.h>
#include <absl/container/flat_hash_set.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <linux/seccomp.h>
#include <signal.h>
#include <sys/prctl.h>
#include <sys/ptrace.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

#include <array>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <memory>
#include <random>
#include <string>
#include <string_view>
#include <thread>
#include <vector>

#include "GrpcProtos/capture.pb.h"
#include "ObjectUtils/ElfFile.h"
#include "OrbitBase/ExecutablePath.h"
#include "OrbitBase/GetProcessIds.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Result.h"
#include "TestUtils.h"
#include "TestUtils/TestUtils.h"
#include "Trampoline.h"
#include "UserSpaceInstrumentation/AddressRange.h"
#include "UserSpaceInstrumentation/InstrumentProcess.h"

namespace orbit_user_space_instrumentation {

namespace {

using orbit_test_utils::HasErrorWithMessage;
using orbit_test_utils::HasNoError;
using ::testing::AnyOf;
using ::testing::HasSubstr;
using ::testing::IsSupersetOf;
using ::testing::Not;

constexpr int kFunctionId1 = 42;
constexpr int kFunctionId2 = 43;

void AddFunctionToCaptureOptions(orbit_grpc_protos::CaptureOptions* capture_options,
                                 std::string_view function_name, int function_id) {
  const auto [module_file_path, range] = FindFunctionOrDie(function_name);
  orbit_grpc_protos::InstrumentedFunction* my_function =
      capture_options->add_instrumented_functions();
  my_function->set_function_id(function_id);
  my_function->set_function_virtual_address(range.start);
  my_function->set_function_size(range.end - range.start);
  my_function->set_function_name(std::string{function_name});
  my_function->set_file_path(module_file_path);
}

orbit_grpc_protos::CaptureOptions BuildCaptureOptions() {
  orbit_grpc_protos::CaptureOptions capture_options;

  AddFunctionToCaptureOptions(&capture_options, "SomethingToInstrument", kFunctionId1);
  AddFunctionToCaptureOptions(&capture_options, "ReturnImmediately", kFunctionId2);

  return capture_options;
}

// Fails if the target is not running anymore, naming the signal that took it down: the heap errors
// glibc reports end in abort(), and the message it prints goes to the target's stderr.
void ExpectTargetIsRunning(pid_t pid, std::string_view when) {
  int status = 0;
  const pid_t waited = waitpid(pid, &status, WNOHANG);
  if (waited == 0) return;
  if (waited == pid && WIFSIGNALED(status)) {
    ADD_FAILURE() << "The target died from signal " << WTERMSIG(status) << " " << when
                  << "; if that is SIGABRT, look for the message glibc printed on stderr.";
    return;
  }
  ADD_FAILURE() << "The target is gone " << when << " (waitpid returned " << waited << ", status 0x"
                << std::hex << status << ").";
}

[[nodiscard]] InstrumentationManager* GetInstrumentationManager() {
  static std::unique_ptr<InstrumentationManager> m = InstrumentationManager::Create();
  return m.get();
}

}  // namespace

extern "C" int SomethingToInstrument() {
  std::random_device rd;
  std::mt19937 gen(rd());
  std::uniform_int_distribution<int> dis(1, 6);
  return dis(gen);
}

// We will not be able to instrument this - the function is just one byte long and we need five
// bytes to write a jump.
extern "C" __attribute__((naked)) int ReturnImmediately() {
  __asm__ __volatile__("ret \n\t" : : :);
}

TEST(InstrumentProcessTest, FailToInstrumentAlreadyAttached) {
  InstrumentationManager* instrumentation_manager = GetInstrumentationManager();

  // Skip if not running as root. We need to trace a child process.
  if (geteuid() != 0) {
    GTEST_SKIP();
  }

  const pid_t pid = fork();
  ORBIT_CHECK(pid != -1);
  if (pid == 0) {
    prctl(PR_SET_PDEATHSIG, SIGTERM);

    volatile uint64_t counter = 0;
    while (true) {
      // Endless loops without side effects are UB and recent versions of clang optimize it away.
      ++counter;
    }
  }

  // We spawn another child and wait for it to trace `pid`. Then we can't attach.
  const pid_t pid_tracer = fork();
  ORBIT_CHECK(pid_tracer != -1);
  if (pid_tracer == 0) {
    prctl(PR_SET_PDEATHSIG, SIGTERM);

    ptrace(PTRACE_ATTACH, pid, nullptr, nullptr);
    volatile uint64_t counter = 0;
    while (true) {
      // Endless loops without side effects are UB and recent versions of clang optimize it away.
      ++counter;
    }
  }
  bool already_tracing = false;
  while (!already_tracing) {
    auto tracer_pid_or_error = orbit_base::GetTracerPidOfProcess(pid);
    ORBIT_CHECK(!tracer_pid_or_error.has_error());
    already_tracing = tracer_pid_or_error.value() != 0;
  }

  orbit_grpc_protos::CaptureOptions capture_options;
  capture_options.set_pid(pid);
  auto result_or_error = instrumentation_manager->InstrumentProcess(capture_options);
  ASSERT_THAT(result_or_error, HasErrorWithMessage("is already being traced by"));

  // End tracer process, end child process.
  kill(pid_tracer, SIGKILL);
  waitpid(pid_tracer, nullptr, 0);
  kill(pid, SIGKILL);
  waitpid(pid, nullptr, 0);
}

TEST(InstrumentProcessTest, FailToInstrumentInvalidPid) {
  InstrumentationManager* instrumentation_manager = GetInstrumentationManager();

  orbit_grpc_protos::CaptureOptions capture_options;
  capture_options.set_pid(-1);
  auto result_or_error = instrumentation_manager->InstrumentProcess(capture_options);
  ASSERT_THAT(result_or_error, HasErrorWithMessage("There is no process with pid"));
}

TEST(InstrumentProcessTest, FailToInstrumentThisProcess) {
  InstrumentationManager* instrumentation_manager = GetInstrumentationManager();

  orbit_grpc_protos::CaptureOptions capture_options;
  capture_options.set_pid(getpid());
  auto result_or_error = instrumentation_manager->InstrumentProcess(capture_options);
  ASSERT_THAT(result_or_error, HasErrorWithMessage("The target process is OrbitService itself."));
}

static void VerifyTrampolineAddressRangesAndLibraryPath(
    const InstrumentationManager::InstrumentationResult& instrumentation_result) {
  EXPECT_EQ(instrumentation_result.entry_trampoline_address_ranges.size(), 1);
  if (!instrumentation_result.entry_trampoline_address_ranges.empty()) {
    EXPECT_EQ(instrumentation_result.entry_trampoline_address_ranges.at(0).end -
                  instrumentation_result.entry_trampoline_address_ranges.at(0).start,
              4096 * GetMaxTrampolineSize());
  }

  EXPECT_EQ(instrumentation_result.return_trampoline_address_range.end -
                instrumentation_result.return_trampoline_address_range.start,
            GetReturnTrampolineSize());
  /* copybara:strip_begin(In the internal build the library name can be different.) */

  EXPECT_EQ(instrumentation_result.injected_library_path.filename().string(),
            "liborbituserspaceinstrumentation.so");
  /* copybara:strip_end */
}

TEST(InstrumentProcessTest, Instrument) {
  /* copybara:insert(b/237251106 injecting the library into the target process triggers some
                     initilization code that check fails.)
  GTEST_SKIP();
  */
  InstrumentationManager* instrumentation_manager = GetInstrumentationManager();

  const pid_t pid_process_1 = fork();
  ORBIT_CHECK(pid_process_1 != -1);
  if (pid_process_1 == 0) {
    prctl(PR_SET_PDEATHSIG, SIGTERM);

    // Endless loops without side effects are UB and recent versions of clang optimize
    // it away. Making `sum` volatile avoids that problem.
    [[maybe_unused]] volatile int sum = 0;
    while (true) {
      sum += SomethingToInstrument();
    }
  }

  orbit_grpc_protos::CaptureOptions capture_options = BuildCaptureOptions();
  capture_options.set_pid(pid_process_1);
  auto result_or_error = instrumentation_manager->InstrumentProcess(capture_options);
  ASSERT_THAT(result_or_error, HasNoError());
  EXPECT_TRUE(result_or_error.value().instrumented_function_ids.contains(kFunctionId1));
  VerifyTrampolineAddressRangesAndLibraryPath(result_or_error.value());
  auto result = instrumentation_manager->UninstrumentProcess(pid_process_1);
  ASSERT_THAT(result, HasNoError());

  // End child pid_process_1.
  kill(pid_process_1, SIGKILL);
  waitpid(pid_process_1, nullptr, 0);

  // Just do the same thing with another process to trigger the code path deleting the data for the
  // first. Also Instrument / Uninstrument repeatedly.
  const pid_t pid_process_2 = fork();
  ORBIT_CHECK(pid_process_2 != -1);
  if (pid_process_2 == 0) {
    prctl(PR_SET_PDEATHSIG, SIGTERM);

    // Endless loops without side effects are UB and recent versions of clang optimize
    // it away. Making `sum` volatile avoids that problem.
    [[maybe_unused]] volatile int sum = 0;
    while (true) {
      sum += SomethingToInstrument();
    }
  }

  capture_options.set_pid(pid_process_2);
  for (int i = 0; i < 5; i++) {
    result_or_error = instrumentation_manager->InstrumentProcess(capture_options);
    ASSERT_THAT(result_or_error, HasNoError());
    EXPECT_TRUE(result_or_error.value().instrumented_function_ids.contains(kFunctionId1));
    VerifyTrampolineAddressRangesAndLibraryPath(result_or_error.value());
    result = instrumentation_manager->UninstrumentProcess(pid_process_2);
    ASSERT_THAT(result, HasNoError());
  }

  // End child pid_process_2.
  kill(pid_process_2, SIGKILL);
  waitpid(pid_process_2, nullptr, 0);
}

// The instrumentation library is loaded into the target with dlopen, and everything it brings with
// it -- gRPC, protobuf, abseil -- has to stay inside it. In particular it must not bring an
// allocator of its own: loading it into a linker namespace of its own used to do exactly that,
// because such a namespace comes with a second copy of libc, and the two main arenas then grew the
// same brk heap and handed out overlapping memory. The target died of that, with
// "malloc(): unsorted double linked list corrupted" or the sysmalloc assertion about the program
// break, right while the injected library was starting up.
//
// So: a target that uses its heap the way any real one does has to come out of being instrumented
// alive.
TEST(InstrumentProcessTest, TargetHeapSurvivesInstrumentation) {
  InstrumentationManager* instrumentation_manager = GetInstrumentationManager();

  const pid_t pid = fork();
  ORBIT_CHECK(pid != -1);
  if (pid == 0) {
    prctl(PR_SET_PDEATHSIG, SIGTERM);

    // Hold on to a couple of thousand blocks and keep replacing them, which makes the allocator
    // grow and shrink the heap the whole time we are being instrumented.
    constexpr size_t kLiveBlockCount = 2048;
    static std::array<void*, kLiveBlockCount> live_blocks{};
    uint32_t random_state = 12345;
    [[maybe_unused]] volatile int sum = 0;
    while (true) {
      for (void*& block : live_blocks) {
        random_state = random_state * 1103515245 + 12345;
        free(block);
        block = malloc(64 + (random_state >> 16) % 32768);
        memset(block, 0x5a, 32);
        sum += SomethingToInstrument();
      }
    }
  }

  orbit_grpc_protos::CaptureOptions capture_options = BuildCaptureOptions();
  capture_options.set_pid(pid);
  auto result_or_error = instrumentation_manager->InstrumentProcess(capture_options);
  ASSERT_THAT(result_or_error, HasNoError());
  EXPECT_TRUE(result_or_error.value().instrumented_function_ids.contains(kFunctionId1));

  // Let the target run instrumented for a while: a corrupted heap is only noticed the next time the
  // target allocates.
  std::this_thread::sleep_for(std::chrono::milliseconds{500});
  ExpectTargetIsRunning(pid, "while instrumented");

  auto result = instrumentation_manager->UninstrumentProcess(pid);
  ASSERT_THAT(result, HasNoError());

  std::this_thread::sleep_for(std::chrono::milliseconds{500});
  ExpectTargetIsRunning(pid, "after uninstrumenting");

  kill(pid, SIGKILL);
  waitpid(pid, nullptr, 0);
}

// The library injected into the target statically links gRPC, protobuf and abseil, and is loaded
// into the linker namespace the target already has. That is only safe as long as none of it is
// visible there: a symbol this library exports can be bound to by its own code in place of the
// target's copy, or the other way around, and two copies of protobuf that find each other abort
// over the same descriptors being registered twice. The CMake build hid these symbols; the port to
// Bazel dropped that, which is what this test is here to catch.
TEST(InstrumentProcessTest, InstrumentationLibraryExportsNothingButItsPayloads) {
  const std::filesystem::path library_path =
      orbit_base::GetExecutableDir() / "liborbituserspaceinstrumentation.so";
  ASSERT_TRUE(std::filesystem::exists(library_path)) << library_path.string();

  auto elf_file_or_error = orbit_object_utils::CreateElfFile(library_path);
  ASSERT_THAT(elf_file_or_error, HasNoError());
  auto symbols_or_error = elf_file_or_error.value()->LoadSymbolsFromDynsym();
  ASSERT_THAT(symbols_or_error, HasNoError());

  std::vector<std::string> exported_names;
  for (const orbit_grpc_protos::SymbolInfo& symbol : symbols_or_error.value().symbol_infos()) {
    exported_names.emplace_back(symbol.demangled_name());
  }

  // Everything OrbitService calls into the library is here; nothing else has to be.
  EXPECT_THAT(exported_names,
              IsSupersetOf({"InitializeInstrumentationInNewThread", "StartNewCapture",
                            "AddOrbitThreads", "EntryPayload", "ExitPayload"}));

  for (const std::string& name : exported_names) {
    EXPECT_THAT(name, Not(AnyOf(HasSubstr("grpc"), HasSubstr("protobuf"), HasSubstr("absl"),
                                HasSubstr("upb"), HasSubstr("google"))))
        << "The library exports the code it brings along, which must stay private to it.";
  }
}

TEST(InstrumentProcessTest, GetErrorMessage) {
  // The function "ReturnImmediately" compiles to something unexpected in gcc. So we only run this
  // test with the release build of clang.
#if defined(ORBIT_COVERAGE_BUILD) || !defined(__clang__) || !defined(NDEBUG)
  GTEST_SKIP();
#endif
  /* copybara:insert(b/237251106 injecting the library into the target process triggers some
                     initilization code that check fails.)
  GTEST_SKIP();
  */

  InstrumentationManager* instrumentation_manager = GetInstrumentationManager();

  const pid_t pid = fork();
  ORBIT_CHECK(pid != -1);
  if (pid == 0) {
    prctl(PR_SET_PDEATHSIG, SIGTERM);

    // Endless loops without side effects are UB and recent versions of clang optimize
    // it away. Making `sum` volatile avoids that problem.
    [[maybe_unused]] volatile int sum = 0;
    while (true) {
      sum += SomethingToInstrument();
    }
  }

  orbit_grpc_protos::CaptureOptions capture_options = BuildCaptureOptions();
  capture_options.set_pid(pid);
  auto result_or_error = instrumentation_manager->InstrumentProcess(capture_options);
  ASSERT_THAT(result_or_error, HasNoError());
  EXPECT_FALSE(result_or_error.value().instrumented_function_ids.contains(kFunctionId2));
  ASSERT_EQ(result_or_error.value().function_ids_to_error_messages.size(), 1);
  EXPECT_THAT(result_or_error.value().function_ids_to_error_messages[kFunctionId2],
              HasSubstr("Failed to create trampoline: Unable to disassemble enough of the function "
                        "to instrument it. Code: c3"));
  VerifyTrampolineAddressRangesAndLibraryPath(result_or_error.value());
  auto result = instrumentation_manager->UninstrumentProcess(pid);
  ASSERT_THAT(result, HasNoError());
  kill(pid, SIGKILL);
  waitpid(pid, nullptr, 0);
}

// Don't #include <complex.h>, it defines the macro I which can break compilation of other headers.
extern "C" long double creall(_Complex long double z);
extern "C" long double cimagl(_Complex long double z);

// Sets st(0) and st(1). Disabling optimizations for this function -- clang spells that "optnone",
// gcc spells it optimize("O0") -- prevents constant folding, which noinline alone would not.
#if defined(__clang__)
#define ORBIT_NO_OPTIMIZE __attribute__((optnone))
#else
#define ORBIT_NO_OPTIMIZE __attribute__((optimize("O0")))
#endif
extern "C" ORBIT_NO_OPTIMIZE _Complex long double ReturnComplexLongDouble() {
  return {42.0L, 43.0L};
}

// The top two elements of the x87 FPU register stack are used in the System V calling convention to
// return (complex) long double values. We do not back them up in the return trampoline, because we
// can't do it in a way that is correct and also has minimal overhead. But we assume that the
// ExitPayload doesn't change the content. This test verifies it.
TEST(InstrumentProcessTest, ExitPayloadDoesNotUseX87Fpu) {
  /* copybara:insert(b/237251106 injecting the library into the target process triggers some
                     initilization code that check fails.)
  GTEST_SKIP();
  */
  InstrumentationManager* instrumentation_manager = GetInstrumentationManager();

  const pid_t pid = fork();
  ORBIT_CHECK(pid != -1);
  if (pid == 0) {
    prctl(PR_SET_PDEATHSIG, SIGTERM);

    while (true) {
      volatile _Complex long double value = ReturnComplexLongDouble();
      ORBIT_CHECK(creall(value) == 42.0L && cimagl(value) == 43.0L);
    }
  }

  orbit_grpc_protos::CaptureOptions capture_options;
  capture_options.set_pid(pid);
  AddFunctionToCaptureOptions(&capture_options, "ReturnComplexLongDouble", kFunctionId1);
  auto result_or_error = instrumentation_manager->InstrumentProcess(capture_options);
  ASSERT_THAT(result_or_error, HasNoError());
  EXPECT_TRUE(result_or_error.value().instrumented_function_ids.contains(kFunctionId1));
  VerifyTrampolineAddressRangesAndLibraryPath(result_or_error.value());

  std::this_thread::sleep_for(std::chrono::milliseconds(10));
  // This will fail or hang if the child crashed.
  auto result = instrumentation_manager->UninstrumentProcess(pid);
  ASSERT_THAT(result, HasNoError());

  kill(pid, SIGKILL);
  waitpid(pid, nullptr, 0);
}

TEST(InstrumentProcessTest, AnyTargetThreadInStrictSeccompMode) {
  InstrumentationManager* instrumentation_manager = GetInstrumentationManager();

  std::array<int, 2> child_to_parent_pipe{};
  ORBIT_CHECK(pipe(child_to_parent_pipe.data()) == 0);

  const pid_t pid = fork();
  ORBIT_CHECK(pid != -1);
  if (pid == 0) {
    prctl(PR_SET_PDEATHSIG, SIGTERM);

    // Close the read end of the pipe.
    ORBIT_CHECK(close(child_to_parent_pipe[0]) == 0);

    std::thread t{[&child_to_parent_pipe] {
      // Transition to strict seccomp mode.
      ORBIT_CHECK(syscall(SYS_seccomp, SECCOMP_SET_MODE_STRICT, 0, nullptr) == 0);

      // Send one byte to the parent to notify that the child has called seccomp. Note that the
      // strict seccomp mode still allows write.
      ORBIT_CHECK(write(child_to_parent_pipe[1], "a", 1) == 1);

      [[maybe_unused]] volatile uint64_t counter = 0;
      while (true) {
        ++counter;
      }
    }};

    // Endless loops without side effects are UB and recent versions of clang optimize
    // it away. Making `sum` volatile avoids that problem.
    [[maybe_unused]] volatile int sum = 0;
    while (true) {
      sum += SomethingToInstrument();
    }
  }

  // Close the write end of the pipe.
  ORBIT_CHECK(close(child_to_parent_pipe[1]) == 0);

  // Wait for the child to execute the seccomp syscall.
  char buf[1];
  ORBIT_CHECK(read(child_to_parent_pipe[0], buf, 1) == 1);

  orbit_grpc_protos::CaptureOptions capture_options = BuildCaptureOptions();
  capture_options.set_pid(pid);
  auto result_or_error = instrumentation_manager->InstrumentProcess(capture_options);
  ASSERT_THAT(
      result_or_error,
      HasErrorWithMessage("At least one thread of the target process is in strict seccomp mode."));

  kill(pid, SIGKILL);
  waitpid(pid, nullptr, 0);
}

}  // namespace orbit_user_space_instrumentation