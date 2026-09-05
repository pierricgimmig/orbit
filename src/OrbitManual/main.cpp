// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Generates Orbit's manual by driving the real UI.
//
// The manual is a set of screenshots of Orbit doing its job, with the prose that explains each one
// living next to the code that produced it (see Chapters.cpp). Regenerating it is therefore also
// an end-to-end test: every chapter that comes out empty is a feature that did not work.
//
// One invocation starts the target application, starts OrbitService, connects to it, takes a
// capture, walks the UI and writes the pages out.

#include <absl/flags/flag.h>
#include <absl/flags/parse.h>
#include <absl/flags/usage.h>
#include <absl/flags/usage_config.h>
#include <absl/strings/str_cat.h>
#include <absl/strings/str_format.h>
#include <absl/time/time.h>

#include <QApplication>
#include <QCoreApplication>
#include <QProcess>
#include <QString>
#include <QStringList>
#include <Qt>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <memory>
#include <optional>
#include <string>
#include <utility>

#include "ClientFlags/ClientFlags.h"
#include "ClientServices/ProcessClient.h"
#include "GrpcProtos/process.pb.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Result.h"
#include "OrbitBase/ThreadPool.h"
#include "OrbitManual/Chapters.h"
#include "OrbitManual/Manual.h"
#include "OrbitManual/Screenshots.h"
#include "OrbitQt/orbitmainwindow.h"
#include "OrbitVersion/OrbitVersion.h"
#include "SessionSetup/Connections.h"
#include "SessionSetup/OrbitServiceInstance.h"
#include "SessionSetup/SessionSetupUtils.h"
#include "SessionSetup/TargetConfiguration.h"
#include "Style/Style.h"

ABSL_FLAG(std::string, output_directory, "docs/manual",
          "Directory the manual is written to. Created if it does not exist");
ABSL_FLAG(std::string, target_application, "",
          "Application to profile. Defaults to the OrbitTest next to this binary");
ABSL_FLAG(std::string, target_arguments, "5 10 1000",
          "Space-separated arguments for the target application");
ABSL_FLAG(std::string, orbit_service, "",
          "OrbitService binary to run. Defaults to the one next to this binary");
ABSL_FLAG(std::string, service_launcher, "pkexec",
          "How to obtain the root OrbitService needs: \"pkexec\" (a graphical prompt), \"sudo\", "
          "\"none\" to run it unprivileged, or \"external\" to use an OrbitService that is "
          "already running");
ABSL_FLAG(uint32_t, capture_seconds, 10, "How long the capture the manual is written around runs");
ABSL_FLAG(uint32_t, window_width, 2600, "Width of the Orbit window, in pixels");
ABSL_FLAG(uint32_t, window_height, 1400, "Height of the Orbit window, in pixels");

namespace {

using orbit_manual::PumpEventsFor;
using orbit_manual::PumpEventsUntil;

// A target application and OrbitService started by this program, stopped again when it ends.
class LaunchedProcesses {
 public:
  ~LaunchedProcesses() {
    if (target_.state() != QProcess::NotRunning) {
      target_.kill();
      target_.waitForFinished(5000);
    }
  }

  [[nodiscard]] ErrorMessageOr<void> StartTarget(const QString& program,
                                                 const QStringList& arguments) {
    ORBIT_LOG("Starting the target application: %s", program.toStdString());
    // OrbitTest reads from stdin to stay alive, so it needs a channel that stays open.
    target_.setProgram(program);
    target_.setArguments(arguments);
    target_.setProcessChannelMode(QProcess::ForwardedErrorChannel);
    target_.start(QIODevice::ReadWrite);
    if (!target_.waitForStarted(10000)) {
      return ErrorMessage{absl::StrCat("Unable to start ", program.toStdString(), ": ",
                                       target_.errorString().toStdString())};
    }
    return outcome::success();
  }

  [[nodiscard]] qint64 GetTargetPid() const { return target_.processId(); }

 private:
  QProcess target_;
};

// Where a binary that Bazel put next to this one ends up: the runfiles tree mirrors the source
// tree, so OrbitService and OrbitTest sit in sibling directories rather than in this one. An
// installed Orbit has them in the same directory instead.
[[nodiscard]] std::optional<std::filesystem::path> FindSiblingBinary(
    const std::filesystem::path& package_relative_path) {
  const std::filesystem::path application_directory =
      std::filesystem::path{QCoreApplication::applicationDirPath().toStdString()};
  for (const std::filesystem::path& candidate :
       {application_directory / package_relative_path.filename(),
        application_directory / ".." / package_relative_path}) {
    std::error_code error{};
    if (std::filesystem::is_regular_file(candidate, error)) {
      return std::filesystem::weakly_canonical(candidate, error);
    }
  }
  return std::nullopt;
}

[[nodiscard]] ErrorMessageOr<std::filesystem::path> ResolveBinary(
    const std::string& flag_value, const std::filesystem::path& package_relative_path,
    std::string_view flag_name) {
  if (!flag_value.empty()) return std::filesystem::path{flag_value};

  std::optional<std::filesystem::path> found = FindSiblingBinary(package_relative_path);
  if (found.has_value()) return found.value();

  return ErrorMessage{absl::StrCat("Unable to find ", package_relative_path.filename().string(),
                                   " next to this binary; pass --", flag_name)};
}

// Starts OrbitService the way --service_launcher asks for, or returns nullptr when the manual is
// supposed to attach to one that is already running.
[[nodiscard]] ErrorMessageOr<std::unique_ptr<orbit_session_setup::OrbitServiceInstance>>
StartOrbitService(const std::filesystem::path& service_path, const std::string& launcher) {
  const QString service = QString::fromStdString(service_path.string());

  if (launcher == "external") {
    ORBIT_LOG("Using an OrbitService that is already running");
    return nullptr;
  }
  if (launcher == "none") {
    ORBIT_LOG("Starting %s without elevating; expect an incomplete capture",
              service_path.string());
    return orbit_session_setup::OrbitServiceInstance::Create(service, {});
  }
  if (launcher == "pkexec" || launcher == "sudo") {
    ORBIT_LOG("Starting %s via %s", service_path.string(), launcher);
    return orbit_session_setup::OrbitServiceInstance::Create(QString::fromStdString(launcher),
                                                             {service});
  }
  return ErrorMessage{absl::StrCat("Unknown --service_launcher \"", launcher,
                                   "\"; expected pkexec, sudo, none or external")};
}

// Polls OrbitService for the target until it shows up. The service has only just started, and it
// refreshes its process list on a timer, so the first few attempts are expected to fail.
[[nodiscard]] ErrorMessageOr<orbit_grpc_protos::ProcessInfo> WaitForTargetProcess(
    const std::shared_ptr<grpc::Channel>& channel, const std::filesystem::path& target_path,
    qint64 target_pid, absl::Duration timeout) {
  orbit_client_services::ProcessClient process_client{channel};
  std::optional<orbit_grpc_protos::ProcessInfo> process;

  const bool found = PumpEventsUntil(
      [&]() {
        ErrorMessageOr<std::vector<orbit_grpc_protos::ProcessInfo>> list_or_error =
            process_client.GetProcessList();
        if (list_or_error.has_error()) return false;
        for (const orbit_grpc_protos::ProcessInfo& candidate : list_or_error.value()) {
          if (candidate.pid() == target_pid) {
            process = candidate;
            return true;
          }
        }
        return false;
      },
      timeout);

  if (!found || !process.has_value()) {
    return ErrorMessage{absl::StrCat("OrbitService did not report ", target_path.string(), " (pid ",
                                     target_pid, ") within ", absl::FormatDuration(timeout),
                                     ". If it is running as an unprivileged user it cannot see it.")};
  }
  ORBIT_LOG("Profiling %s (pid %d)", process->full_path(), process->pid());
  return process.value();
}

// `bazel run` starts a binary in its runfiles tree rather than in the workspace, so a relative
// output directory would put the manual somewhere nobody will find it. Bazel names the workspace
// in the environment; use it when it is there.
[[nodiscard]] std::filesystem::path ResolveOutputDirectory(const std::string& output_directory) {
  const std::filesystem::path path{output_directory};
  if (path.is_absolute()) return path;

  const char* const workspace = std::getenv("BUILD_WORKSPACE_DIRECTORY");
  if (workspace == nullptr) return path;
  return std::filesystem::path{workspace} / path;
}

[[nodiscard]] ErrorMessageOr<void> GenerateManual() {
  OUTCOME_TRY(auto&& target_path,
              ResolveBinary(absl::GetFlag(FLAGS_target_application), "OrbitTest/OrbitTest",
                            "target_application"));
  OUTCOME_TRY(auto&& service_path,
              ResolveBinary(absl::GetFlag(FLAGS_orbit_service), "Service/OrbitService",
                            "orbit_service"));

  LaunchedProcesses processes;
  OUTCOME_TRY(processes.StartTarget(
      QString::fromStdString(target_path.string()),
      QString::fromStdString(absl::GetFlag(FLAGS_target_arguments)).split(' ', Qt::SkipEmptyParts)));

  OUTCOME_TRY(auto&& service_instance,
              StartOrbitService(service_path, absl::GetFlag(FLAGS_service_launcher)));

  const uint16_t grpc_port = absl::GetFlag(FLAGS_grpc_port);
  std::shared_ptr<grpc::Channel> channel = orbit_session_setup::CreateGrpcChannel(grpc_port);
  OUTCOME_TRY(auto&& process, WaitForTargetProcess(channel, target_path, processes.GetTargetPid(),
                                                   absl::Seconds(30)));

  orbit_session_setup::LocalTarget target{
      orbit_session_setup::LocalConnection{std::move(channel), std::move(service_instance)},
      std::move(process)};

  OrbitMainWindow window{std::move(target)};
  // A fixed size keeps a regenerated manual's screenshots comparable with the previous one's.
  window.resize(static_cast<int>(absl::GetFlag(FLAGS_window_width)),
                static_cast<int>(absl::GetFlag(FLAGS_window_height)));
  window.show();
  window.raise();
  window.activateWindow();
  PumpEventsFor(absl::Seconds(2));

  orbit_manual::Manual manual{ResolveOutputDirectory(absl::GetFlag(FLAGS_output_directory))};
  std::string subtitle = absl::StrFormat(
      "Generated from Orbit %s by capturing %s. Every screenshot below was taken by driving the "
      "real UI, so a chapter that looks wrong is a bug rather than a stale picture.",
      orbit_version::GetVersionString(), target_path.filename().string());
  if (absl::GetFlag(FLAGS_service_launcher) == "none") {
    // Worth saying plainly: without root there are no scheduling or thread-state tracks, and the
    // call trees are mostly unwind errors. A reader should not take that for how Orbit behaves.
    absl::StrAppend(&subtitle,
                    " This run had no root, so the capture is missing everything that comes from "
                    "kernel tracepoints.");
  }
  manual.SetSubtitle(std::move(subtitle));
  OUTCOME_TRY(orbit_base::CreateDirectories(manual.GetImageDirectory()));

  orbit_manual::RecordingOptions options;
  options.capture_duration = absl::Seconds(absl::GetFlag(FLAGS_capture_seconds));
  OUTCOME_TRY(orbit_manual::RecordManual(&window, &manual, options));

  OUTCOME_TRY(manual.Write());
  ORBIT_LOG("Wrote %d chapters to %s", manual.GetChapters().size(),
            ResolveOutputDirectory(absl::GetFlag(FLAGS_output_directory)).string());

  window.close();
  PumpEventsFor(absl::Seconds(1));
  return outcome::success();
}

}  // namespace

int main(int argc, char* argv[]) {
  absl::SetProgramUsageMessage(
      "Generates Orbit's manual by starting a target application, capturing it and screenshotting "
      "the UI");
  absl::SetFlagsUsageConfig(absl::FlagsUsageConfig{{}, {}, {}, &orbit_version::GetBuildReport, {}});
  absl::ParseCommandLine(argc, argv);

#if __linux__
  QCoreApplication::setAttribute(Qt::AA_DontUseNativeDialogs);
#endif

  QApplication app(argc, argv);
  QApplication::setOrganizationName("The Orbit Authors");
  // Deliberately not "orbitprofiler": Orbit keeps window geometry, symbol locations and the rest
  // in QSettings under the application name, and the manual has to be reproducible rather than a
  // picture of whatever state the machine's own Orbit was last left in. This also keeps a
  // generation run from writing over those settings.
  QApplication::setApplicationName("orbitprofiler-manual");
  QApplication::setApplicationDisplayName(
      QString{"Orbit Profiler %1"}.arg(QString::fromStdString(orbit_version::GetVersionString())));
  QApplication::setApplicationVersion(QString::fromStdString(orbit_version::GetVersionString()));

  orbit_base::ThreadPool::InitializeDefaultThreadPool();
  orbit_style::ApplyStyle(&app);

  ErrorMessageOr<void> result = GenerateManual();
  if (result.has_error()) {
    ORBIT_ERROR("%s", result.error().message());
    return 1;
  }
  return 0;
}
