// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "UprobeEvents.h"

#include <absl/strings/str_format.h>
#include <fcntl.h>
#include <unistd.h>

#include <cerrno>
#include <string>

#include "LinuxTracingUtils.h"
#include "OrbitBase/File.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/SafeStrerror.h"

namespace orbit_linux_tracing {
namespace {

ErrorMessageOr<void> WriteTracefsCommand(const std::filesystem::path& path, std::string_view line) {
  // Must not O_TRUNC: uprobe_events is a command interface, and truncating it
  // would delete every probe on the system.
  int raw_fd = TEMP_FAILURE_RETRY(open(path.c_str(), O_WRONLY | O_CLOEXEC));
  if (raw_fd < 0) {
    return ErrorMessage{absl::StrFormat("Opening \"%s\": %s", path.string(), SafeStrerror(errno))};
  }
  orbit_base::UniqueFd fd{raw_fd};

  std::string command{line};
  if (command.empty() || command.back() != '\n') {
    command.push_back('\n');
  }
  ErrorMessageOr<void> write_result = orbit_base::WriteFully(fd, command);
  if (write_result.has_error()) {
    return ErrorMessage{
        absl::StrFormat("Writing \"%s\": %s", path.string(), write_result.error().message())};
  }
  return outcome::success();
}

ErrorMessageOr<std::filesystem::path> UprobeEventsPath() {
  std::optional<std::filesystem::path> tracing_dir = FindTracingDirectory();
  if (!tracing_dir.has_value()) {
    return ErrorMessage{"No tracefs directory found"};
  }
  std::filesystem::path path = tracing_dir.value() / "uprobe_events";
  if (access(path.c_str(), F_OK) != 0) {
    return ErrorMessage{absl::StrFormat("\"%s\" is not available", path.string())};
  }
  return path;
}

}  // namespace

std::string MakeOrbitUprobeEventName(uint64_t unique_id, bool is_return) {
  return absl::StrFormat("%c%llu", is_return ? 'r' : 'u',
                         static_cast<unsigned long long>(unique_id));
}

ErrorMessageOr<TracefsUprobe> DefineTracefsUprobe(std::string_view module_path,
                                                  uint64_t function_offset, bool is_return,
                                                  std::string_view event_name) {
  ErrorMessageOr<std::filesystem::path> events_path_or_error = UprobeEventsPath();
  if (events_path_or_error.has_error()) {
    return events_path_or_error.error();
  }

  TracefsUprobe probe{.group = kOrbitUprobeEventGroup, .event = std::string{event_name}};
  // Remove a leftover definition from a previous crashed capture with the same name.
  UndefineTracefsUprobe(probe);

  const char type = is_return ? 'r' : 'p';
  std::string command =
      absl::StrFormat("%c:%s/%s %s:0x%llx", type, probe.group, probe.event, module_path,
                      static_cast<unsigned long long>(function_offset));
  ErrorMessageOr<void> write_result = WriteTracefsCommand(events_path_or_error.value(), command);
  if (write_result.has_error()) {
    return ErrorMessage{absl::StrFormat(
        "Defining %s/%s at %s+0x%llx: %s", probe.group, probe.event, module_path,
        static_cast<unsigned long long>(function_offset), write_result.error().message())};
  }
  return probe;
}

void UndefineTracefsUprobe(const TracefsUprobe& probe) {
  if (probe.group.empty() || probe.event.empty()) {
    return;
  }
  ErrorMessageOr<std::filesystem::path> events_path_or_error = UprobeEventsPath();
  if (events_path_or_error.has_error()) {
    return;
  }
  std::string command = absl::StrFormat("-:%s/%s", probe.group, probe.event);
  ErrorMessageOr<void> write_result = WriteTracefsCommand(events_path_or_error.value(), command);
  if (write_result.has_error()) {
    ORBIT_ERROR("Undefining %s/%s: %s", probe.group, probe.event, write_result.error().message());
  }
}

bool OpenTracefsUprobeFdsPerCpu(const TracefsUprobe& probe, absl::Span<const int32_t> cpus,
                                pid_t pid, UprobeSampleLayout layout, uint16_t stack_dump_size,
                                absl::flat_hash_map<int32_t, int>* fds_per_cpu) {
  for (int32_t cpu : cpus) {
    int fd = OpenTracefsUprobeFd(probe, pid, cpu, layout, stack_dump_size);
    if (fd < 0) {
      ORBIT_ERROR("Opening tracefs %s/%s on cpu %d", probe.group, probe.event, cpu);
      return false;
    }
    (*fds_per_cpu)[cpu] = fd;
  }
  return true;
}

}  // namespace orbit_linux_tracing
