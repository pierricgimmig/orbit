// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <absl/container/flat_hash_set.h>
#include <absl/flags/declare.h>
#include <absl/flags/flag.h>
#include <absl/flags/parse.h>
#include <absl/flags/usage.h>
#include <absl/flags/usage_config.h>

#include <QApplication>
#include <QCoreApplication>
#include <QDir>
#include <QMessageBox>
#include <QMetaType>
#include <QObject>
#include <QProcessEnvironment>
#include <QString>
#include <Qt>
#include <cstdint>
#include <filesystem>
#include <optional>
#include <string>
#include <system_error>
#include <utility>
#include <vector>

#include "MetricsUploader/ScopedMetric.h"
#include "OrbitBase/File.h"
#include "OrbitQt/MoveFilesDialog.h"
#include "OrbitQt/MoveFilesProcess.h"

#ifdef _WIN32
#include <process.h>
#else
#endif

#include "AccessibilityAdapter.h"
#include "Connections.h"
#include "DeploymentConfigurations.h"
#include "ImGuiOrbit.h"
#include "MetricsUploader/MetricsUploader.h"
#include "OrbitBase/CrashHandler.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/Profiling.h"
#include "OrbitSsh/Context.h"
#include "OrbitSshQt/ScopedConnection.h"
#include "OrbitVersion/OrbitVersion.h"
#include "Path.h"
#include "ProfilingTargetDialog.h"
#include "SourcePathsMapping/MappingManager.h"
#include "Style/Style.h"
#include "TargetConfiguration.h"
#include "opengldetect.h"
#include "orbitmainwindow.h"
#include "servicedeploymanager.h"

#ifdef ORBIT_CRASH_HANDLING
#include "CrashHandler/CrashHandler.h"
#include "CrashHandler/CrashOptions.h"
#endif

ABSL_DECLARE_FLAG(uint16_t, grpc_port);
ABSL_DECLARE_FLAG(std::string, collector_root_password);
ABSL_DECLARE_FLAG(std::string, collector);
ABSL_DECLARE_FLAG(bool, local);
ABSL_DECLARE_FLAG(bool, devmode);
ABSL_DECLARE_FLAG(bool, nodeploy);

// This flag is needed by the E2E tests to ensure a clean state before running.
ABSL_FLAG(bool, clear_source_paths_mappings, false, "Clear all the stored source paths mappings");

using ServiceDeployManager = orbit_qt::ServiceDeployManager;
using DeploymentConfiguration = orbit_qt::DeploymentConfiguration;
using ScopedConnection = orbit_ssh_qt::ScopedConnection;
using GrpcPort = ServiceDeployManager::GrpcPort;
using Context = orbit_ssh::Context;

Q_DECLARE_METATYPE(std::error_code);

void RunUiInstance(const DeploymentConfiguration& deployment_configuration,
                   const Context* ssh_context, const QStringList& command_line_flags,
                   const orbit_base::CrashHandler* crash_handler,
                   const std::filesystem::path& capture_file_path) {
  qRegisterMetaType<std::error_code>();

  const GrpcPort grpc_port{/*.grpc_port =*/absl::GetFlag(FLAGS_grpc_port)};

  orbit_qt::SshConnectionArtifacts ssh_connection_artifacts{ssh_context, grpc_port,
                                                            &deployment_configuration};

  std::optional<orbit_qt::TargetConfiguration> target_config;

  std::unique_ptr<orbit_metrics_uploader::MetricsUploader> metrics_uploader =
      orbit_metrics_uploader::MetricsUploader::CreateMetricsUploader();
  metrics_uploader->SendLogEvent(
      orbit_metrics_uploader::OrbitLogEvent_LogEventType_ORBIT_METRICS_UPLOADER_START);

  orbit_metrics_uploader::ScopedMetric metric{
      metrics_uploader.get(), orbit_metrics_uploader::OrbitLogEvent_LogEventType_ORBIT_EXIT};

  // If Orbit starts with loading a capture file, we skip ProfilingTargetDialog and create a
  // FileTarget from capture_file_path. After creating the FileTarget, we reset
  // skip_profiling_target_dialog as false such that if a user ends the previous session, Orbit
  // will return to a ProfilingTargetDialog.
  bool skip_profiling_target_dialog = !capture_file_path.empty();
  while (true) {
    {
      if (skip_profiling_target_dialog) {
        target_config = orbit_qt::FileTarget(capture_file_path);
        skip_profiling_target_dialog = false;
      } else {
        orbit_qt::ProfilingTargetDialog target_dialog{
            &ssh_connection_artifacts, std::move(target_config), metrics_uploader.get()};
        target_config = target_dialog.Exec();

        if (!target_config.has_value()) {
          // User closed dialog
          break;
        }
      }
    }

    int application_return_code = 0;
    orbit_qt::InstallAccessibilityFactories();

    {  // Scoping of QT UI Resources

      OrbitMainWindow w(std::move(target_config.value()), crash_handler, metrics_uploader.get(),
                        command_line_flags);

      // "resize" is required to make "showMaximized" work properly.
      w.resize(1280, 720);
      w.showMaximized();

      application_return_code = QApplication::exec();

      target_config = w.ClearTargetConfiguration();
    }

    if (application_return_code == OrbitMainWindow::kQuitOrbitReturnCode) {
      // User closed window
      break;
    }

    if (application_return_code == OrbitMainWindow::kEndSessionReturnCode) {
      // User clicked end session, or socket error occurred.
      continue;
    }

    UNREACHABLE();
  }
}

// Extract command line flags by filtering the positional arguments out from the command line
// arguments.
static QStringList ExtractCommandLineFlags(const std::vector<std::string>& command_line_args,
                                           const std::vector<char*>& positional_args) {
  QStringList command_line_flags;
  absl::flat_hash_set<std::string> positional_arg_set(positional_args.begin(),
                                                      positional_args.end());
  for (const std::string& command_line_arg : command_line_args) {
    if (!positional_arg_set.contains(command_line_arg)) {
      command_line_flags << QString::fromStdString(command_line_arg);
    }
  }
  return command_line_flags;
}

static std::optional<std::string> GetCollectorRootPassword(
    const QProcessEnvironment& process_environment) {
  constexpr const char* kEnvRootPassword = "ORBIT_COLLECTOR_ROOT_PASSWORD";
  if (FLAGS_collector_root_password.IsSpecifiedOnCommandLine()) {
    return absl::GetFlag(FLAGS_collector_root_password);
  }

  if (process_environment.contains(kEnvRootPassword)) {
    return process_environment.value(kEnvRootPassword).toStdString();
  }

  return std::nullopt;
}

static std::optional<std::string> GetCollectorPath(const QProcessEnvironment& process_environment) {
  constexpr const char* kEnvExecutablePath = "ORBIT_COLLECTOR_EXECUTABLE_PATH";
  if (FLAGS_collector.IsSpecifiedOnCommandLine()) {
    return absl::GetFlag(FLAGS_collector);
  }

  if (process_environment.contains(kEnvExecutablePath)) {
    return process_environment.value(kEnvExecutablePath).toStdString();
  }
  return std::nullopt;
}

static orbit_qt::DeploymentConfiguration FigureOutDeploymentConfiguration() {
  if (absl::GetFlag(FLAGS_nodeploy)) {
    return orbit_qt::NoDeployment{};
  }

  constexpr const char* kEnvPackagePath = "ORBIT_COLLECTOR_PACKAGE_PATH";
  constexpr const char* kEnvSignaturePath = "ORBIT_COLLECTOR_SIGNATURE_PATH";
  constexpr const char* kEnvNoDeployment = "ORBIT_COLLECTOR_NO_DEPLOYMENT";

  QProcessEnvironment env = QProcessEnvironment::systemEnvironment();
  std::optional<std::string> collector_path = GetCollectorPath(env);
  std::optional<std::string> collector_password = GetCollectorRootPassword(env);

  if (collector_path.has_value() && collector_password.has_value()) {
    return orbit_qt::BareExecutableAndRootPasswordDeployment{collector_path.value(),
                                                             collector_password.value()};
  }

  if (env.contains(kEnvPackagePath) && env.contains(kEnvSignaturePath)) {
    return orbit_qt::SignedDebianPackageDeployment{env.value(kEnvPackagePath).toStdString(),
                                                   env.value(kEnvSignaturePath).toStdString()};
  }

  if (env.contains(kEnvNoDeployment)) {
    return orbit_qt::NoDeployment{};
  }

  return orbit_qt::SignedDebianPackageDeployment{};
}

static void DisplayErrorToUser(const QString& message) {
  QMessageBox::critical(nullptr, QApplication::applicationName(), message);
}

static bool DevModeEnabledViaEnvironmentVariable() {
  const auto env = QProcessEnvironment::systemEnvironment();
  return env.contains("ORBIT_DEV_MODE") || env.contains("ORBIT_DEVELOPER_MODE");
}

static void LogAndMaybeWarnAboutClockResolution() {
  uint64_t estimated_clock_resolution = orbit_base::EstimateClockResolution();
  LOG("%s", absl::StrFormat("Clock resolution on client: %d (ns)", estimated_clock_resolution));

  // Since a low clock resolution on the client only affects our own introspection and logging
  // timings, we only show a warning dialogue when running in devmode.
  constexpr uint64_t kWarnThresholdClockResolutionNs = 10 * 1000;  // 10 us
  if (absl::GetFlag(FLAGS_devmode) &&
      estimated_clock_resolution > kWarnThresholdClockResolutionNs) {
    DisplayErrorToUser(
        QString("Warning, clock resolution is low (estimated as %1 ns)! Introspection "
                "timings may be inaccurate.")
            .arg(estimated_clock_resolution));
  }
  // An estimated clock resolution of 0 means that estimating the resolution failed. This can
  // happen for really low resolutions and is likely an error case that we should warn about
  // in devmode.
  if (absl::GetFlag(FLAGS_devmode) && estimated_clock_resolution == 0) {
    DisplayErrorToUser(QString(
        "Warning, failed to estimate clock resolution! Introspection timings may be inaccurate."));
  }
}

static bool IsDirectoryEmpty(const std::filesystem::path& directory) {
  auto exists_or_error = orbit_base::FileExists(directory);
  if (exists_or_error.has_error()) {
    ERROR("Unable to stat \"%s\": %s", directory.string(), exists_or_error.error().message());
    return false;
  }

  if (!exists_or_error.value()) {
    return true;
  }

  auto file_list_or_error = orbit_base::ListFilesInDirectory(directory);
  if (file_list_or_error.has_error()) {
    ERROR("Unable to list directory \"%s\": %s", directory.string(),
          file_list_or_error.error().message());
    return false;
  }

  return file_list_or_error.value().empty();
}

static void TryMoveSavedDataLocationIfNeeded() {
  return;
  
  /*if (IsDirectoryEmpty(orbit_core::GetPresetDirPriorTo1_65()) &&
      IsDirectoryEmpty(orbit_core::GetCaptureDirPriorTo1_65())) {
    return;
  }

  orbit_qt::MoveFilesDialog dialog;
  orbit_qt::MoveFilesProcess process;

  QObject::connect(&process, &orbit_qt::MoveFilesProcess::generalError,
                   [&dialog](const std::string& error_message) {
                     dialog.AddText(absl::StrFormat("Error: %s", error_message));
                   });

  QObject::connect(
      &process, &orbit_qt::MoveFilesProcess::moveStarted,
      [&dialog](const std::filesystem::path& from_dir, const std::filesystem::path& to_dir,
                size_t number_of_files) {
        dialog.AddText(absl::StrFormat(R"(Moving %d files from "%s" to "%s" ...)", number_of_files,
                                       from_dir.string(), to_dir.string()));
      });

  QObject::connect(&process, &orbit_qt::MoveFilesProcess::moveDone,
                   [&dialog]() { dialog.AddText("... done"); });

  QObject::connect(&process, &orbit_qt::MoveFilesProcess::processFinished,
                   [&dialog]() { dialog.EnableCloseButton(); });

  process.Start();
  dialog.exec();*/
}

// Removes all source paths mappings from the persistent settings storage.
static void ClearSourcePathsMappings() {
  orbit_source_paths_mapping::MappingManager mapping_manager{};
  mapping_manager.SetMappings({});
  LOG("Cleared the saved source paths mappings.");
}

// Put the command line into the log.
static void LogCommandLine(int argc, char* argv[]) {
  LOG("Command line invoking Orbit:");
  LOG("%s", argv[0]);
  for (int i = 1; i < argc; i++) {
    LOG("  %s", argv[i]);
  }
  LOG("");
}

int main(int argc, char* argv[]) {
  absl::SetProgramUsageMessage("CPU Profiler");
  absl::SetFlagsUsageConfig(absl::FlagsUsageConfig{{}, {}, {}, &orbit_core::GetBuildReport, {}});
  std::vector<char*> positional_args = absl::ParseCommandLine(argc, argv);

  QString orbit_executable = QString(argv[0]);
  std::vector<std::string> command_line_args;
  if (argc > 1) {
    command_line_args.assign(argv + 1, argv + argc);
  }
  QStringList command_line_flags = ExtractCommandLineFlags(command_line_args, positional_args);
  // Skip program name in positional_args[0].
  std::vector<std::string> capture_file_paths(positional_args.begin() + 1, positional_args.end());

  orbit_base::InitLogFile(orbit_core::GetLogFilePath());
  LOG("You are running Orbit Profiler version %s", orbit_core::GetVersion());
  LogCommandLine(argc, argv);
  ErrorMessageOr<void> remove_old_log_result =
      orbit_base::TryRemoveOldLogFiles(orbit_core::CreateOrGetLogDir());
  if (remove_old_log_result.has_error()) {
    LOG("Warning: Unable to remove some old log files:\n%s",
        remove_old_log_result.error().message());
  }

#if __linux__
  QCoreApplication::setAttribute(Qt::AA_DontUseNativeDialogs);
#endif

  QApplication app(argc, argv);
  QApplication::setOrganizationName("The Orbit Authors");
  QApplication::setApplicationName("orbitprofiler");

  if (DevModeEnabledViaEnvironmentVariable()) {
    absl::SetFlag(&FLAGS_devmode, true);
  }

  // The application display name is automatically appended to all window titles when shown in the
  // title bar: <specific window title> - <application display name>
  const auto version_string = QString::fromStdString(orbit_core::GetVersion());
  auto display_name = QString{"Orbit Profiler %1 [BETA]"}.arg(version_string);

  if (absl::GetFlag(FLAGS_devmode)) {
    display_name.append(" [DEVELOPER MODE]");
  }

  QApplication::setApplicationDisplayName(display_name);
  QApplication::setApplicationVersion(version_string);

  auto crash_handler = std::make_unique<orbit_base::CrashHandler>();
#ifdef ORBIT_CRASH_HANDLING
  const std::string dump_path = orbit_core::CreateOrGetDumpDir().string();
#ifdef _WIN32
  const char* handler_name = "crashpad_handler.exe";
#else
  const char* handler_name = "crashpad_handler";
#endif
  const std::string handler_path =
      QDir(QCoreApplication::applicationDirPath()).absoluteFilePath(handler_name).toStdString();
  const std::string crash_server_url = orbit_crash_handler::GetServerUrl();
  const std::vector<std::string> attachments = {orbit_core::GetLogFilePath().string()};

  crash_handler = std::make_unique<orbit_crash_handler::CrashHandler>(
      dump_path, handler_path, crash_server_url, attachments);
#endif  // ORBIT_CRASH_HANDLING

  if (absl::GetFlag(FLAGS_clear_source_paths_mappings)) {
    ClearSourcePathsMappings();
    return 0;
  }

  orbit_style::ApplyStyle(&app);

  const auto open_gl_version = orbit_qt::DetectOpenGlVersion();

  if (!open_gl_version) {
    DisplayErrorToUser(
        "OpenGL support was not found. This usually indicates some DLLs are missing. "
        "Please try to reinstall Orbit!");
    return -1;
  }

  LOG("Detected OpenGL version: %i.%i %s", open_gl_version->major, open_gl_version->minor,
      open_gl_version->is_opengl_es ? "OpenGL ES" : "OpenGL");

  if (open_gl_version->is_opengl_es) {
    DisplayErrorToUser(
        "Orbit was only able to load OpenGL ES while Desktop OpenGL is required. Try to force "
        "software rendering by starting Orbit with the environment variable QT_OPENGL=software "
        "set.");
    return -1;
  }

  if (open_gl_version->major < 2) {
    DisplayErrorToUser(QString("The minimum required version of OpenGL is 2.0. But this machine "
                               "only supports up to version %1.%2. Please make sure you're not "
                               "trying to start Orbit in a remote session and make sure you "
                               "have a recent graphics driver installed. Then try again!")
                           .arg(open_gl_version->major)
                           .arg(open_gl_version->minor));
    return -1;
  }

  LogAndMaybeWarnAboutClockResolution();

  TryMoveSavedDataLocationIfNeeded();

  const DeploymentConfiguration deployment_configuration = FigureOutDeploymentConfiguration();

  auto context = Context::Create();
  if (!context) {
    DisplayErrorToUser(QString("An error occurred while initializing ssh: %1")
                           .arg(QString::fromStdString(context.error().message())));
    return -1;
  }

  // If more than one capture files are provided, start multiple Orbit instances.
  for (size_t i = 1; i < capture_file_paths.size(); ++i) {
    QStringList arguments;
    arguments << QString::fromStdString(capture_file_paths[i]) << command_line_flags;
    QProcess::startDetached(orbit_executable, arguments);
  }

  RunUiInstance(deployment_configuration, &context.value(), command_line_flags, crash_handler.get(),
                capture_file_paths.empty() ? "" : capture_file_paths[0]);
  return 0;
}
