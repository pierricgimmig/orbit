#ifndef ORBIT_LINUX_TRACING_UPROBES_FUNCTION_CALL_MANAGER_H_
#define ORBIT_LINUX_TRACING_UPROBES_FUNCTION_CALL_MANAGER_H_

#include <OrbitBase/Logging.h>
#include <OrbitLinuxTracing/Events.h>
#include "PerfEventRecords.h"

#include <stack>
#include <vector>

#include "absl/container/flat_hash_map.h"

namespace LinuxTracing {

static inline perf_event_uprobe_regs dummy_regs;

// Keeps a stack, for every thread, of the open uprobes and matches them with
// the uretprobes to produce FunctionCall objects.
class UprobesFunctionCallManager {
 public:
  UprobesFunctionCallManager() = default;

  UprobesFunctionCallManager(const UprobesFunctionCallManager&) = delete;
  UprobesFunctionCallManager& operator=(const UprobesFunctionCallManager&) =
      delete;

  UprobesFunctionCallManager(UprobesFunctionCallManager&&) = default;
  UprobesFunctionCallManager& operator=(UprobesFunctionCallManager&&) = default;

  void ProcessUprobes(pid_t tid, uint64_t function_address,
                      uint64_t begin_timestamp,
                      const perf_event_uprobe_regs& regs = dummy_regs) {
    auto& tid_uprobes_stack = tid_uprobes_stacks_[tid];
    tid_uprobes_stack.emplace(function_address, begin_timestamp, regs);
    LOG("ProcessUprobes");
  }

  std::optional<FunctionCall> ProcessUretprobes(pid_t tid,
                                                uint64_t end_timestamp,
                                                uint64_t return_value) {
    if (!tid_uprobes_stacks_.contains(tid)) {
      LOG("early out tid_uprobes_stacks_.size() = %lu",
          tid_uprobes_stacks_.size());
      return std::optional<FunctionCall>{};
    }

    auto& tid_uprobes_stack = tid_uprobes_stacks_.at(tid);

    // As we erase the stack for this thread as soon as it becomes empty.
    CHECK(!tid_uprobes_stack.empty());
    auto& tid_uprobe = tid_uprobes_stack.top();
    std::vector<uint64_t> regs{
        tid_uprobe.registers.di, tid_uprobe.registers.si,
        tid_uprobe.registers.dx, tid_uprobe.registers.cx,
        tid_uprobe.registers.r8, tid_uprobe.registers.r9};
    auto function_call = std::make_optional<FunctionCall>(
        tid, tid_uprobe.function_address,
        tid_uprobe.begin_timestamp, end_timestamp,
        tid_uprobes_stack.size() - 1, return_value, regs);
    tid_uprobes_stack.pop();
    if (tid_uprobes_stack.empty()) {
      tid_uprobes_stacks_.erase(tid);
    }
    return function_call;
  }

 private:
  struct OpenUprobes {
    OpenUprobes(uint64_t function_address, uint64_t begin_timestamp, const perf_event_uprobe_regs& regs = dummy_regs)
        : function_address{function_address},
          begin_timestamp{begin_timestamp},
          registers(regs)
          {}
    uint64_t function_address;
    uint64_t begin_timestamp;
    perf_event_uprobe_regs registers;
  };

  // This map keeps the stack of the dynamically-instrumented functions entered.
  absl::flat_hash_map<pid_t, std::stack<OpenUprobes, std::vector<OpenUprobes>>>
      tid_uprobes_stacks_{};
};

}  // namespace LinuxTracing

#endif  // ORBIT_LINUX_TRACING_UPROBES_FUNCTION_CALL_MANAGER_H_
