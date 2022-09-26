// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "App.h"

#include <absl/container/flat_hash_map.h>
#include <absl/container/flat_hash_set.h>
#include <absl/flags/declare.h>
#include <absl/flags/flag.h>
#include <absl/flags/internal/flag.h>
#include <absl/strings/match.h>
#include <absl/strings/str_format.h>
#include <absl/strings/str_join.h>
#include <absl/strings/substitute.h>
#include <absl/synchronization/mutex.h>
#include <absl/time/time.h>
#include <imgui.h>
#include <stdlib.h>

#include <QProcess>
#include <algorithm>
#include <chrono>
#include <cinttypes>
#include <cstddef>
#include <iterator>
#include <limits>
#include <memory>
#include <optional>
#include <ratio>
#include <string>
#include <thread>
#include <tuple>
#include <utility>

#include "CaptureClient/CaptureListener.h"
#include "CaptureClient/LoadCapture.h"
#include "CaptureFile/CaptureFile.h"
#include "CaptureFile/CaptureFileHelpers.h"
#include "CaptureWindow.h"
#include "ClientData/CallstackData.h"
#include "ClientData/ModuleAndFunctionLookup.h"
#include "ClientData/ModuleData.h"
#include "ClientData/ModuleManager.h"
#include "ClientData/PostProcessedSamplingData.h"
#include "ClientData/ProcessData.h"
#include "ClientData/ScopeId.h"
#include "ClientData/ScopeStats.h"
#include "ClientData/TimerChain.h"
#include "ClientData/UserDefinedCaptureData.h"
#include "ClientFlags/ClientFlags.h"
#include "ClientModel/CaptureSerializer.h"
#include "ClientModel/SamplingDataPostProcessor.h"
#include "ClientProtos/capture_data.pb.h"
#include "ClientProtos/preset.pb.h"
#include "ClientSymbols/QSettingsBasedStorageManager.h"
#include "CodeReport/Disassembler.h"
#include "CodeReport/DisassemblyReport.h"
#include "CodeReport/SourceCodeReport.h"
#include "DataViews/AppInterface.h"
#include "DataViews/DataView.h"
#include "DataViews/DataViewType.h"
#include "DataViews/FunctionsDataView.h"
#include "DataViews/ModulesDataView.h"
#include "DataViews/PresetsDataView.h"
#include "DataViews/SymbolLoadingState.h"
#include "FrameTrackOnlineProcessor.h"
#include "GlCanvas.h"
#include "GrpcProtos/Constants.h"
#include "GrpcProtos/capture.pb.h"
#include "GrpcProtos/module.pb.h"
#include "GrpcProtos/symbol.pb.h"
#include "ImGuiOrbit.h"
#include "Introspection/Introspection.h"
#include "MainWindowInterface.h"
#include "MetricsUploader/CaptureMetric.h"
#include "MetricsUploader/MetricsUploader.h"
#include "MetricsUploader/ScopedMetric.h"
#include "MetricsUploader/orbit_log_event.pb.h"
#include "ModuleUtils/VirtualAndAbsoluteAddresses.h"
#include "ObjectUtils/ElfFile.h"
#include "OrbitBase/CanceledOr.h"
#include "OrbitBase/ExecutablePath.h"
#include "OrbitBase/File.h"
#include "OrbitBase/Future.h"
#include "OrbitBase/FutureHelpers.h"
#include "OrbitBase/ImmediateExecutor.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/MainThreadExecutor.h"
#include "OrbitBase/NotFoundOr.h"
#include "OrbitBase/Result.h"
#include "OrbitBase/StopSource.h"
#include "OrbitBase/StopToken.h"
#include "OrbitBase/ThreadConstants.h"
#include "OrbitBase/UniqueResource.h"
#include "OrbitBase/WhenAll.h"
#include "OrbitPaths/Paths.h"
#include "OrbitVersion/OrbitVersion.h"
#include "SamplingReport.h"
#include "Statistics/BinomialConfidenceInterval.h"
#include "SymbolProvider/ModuleIdentifier.h"
#include "SymbolProvider/SymbolLoadingOutcome.h"
#include "Symbols/SymbolHelper.h"
#include "Symbols/SymbolUtils.h"
#include "TimeGraph.h"

using orbit_base::CanceledOr;
using orbit_base::Future;
using orbit_base::NotFoundOr;
using orbit_base::StopToken;

using orbit_capture_client::CaptureClient;
using orbit_capture_client::CaptureEventProcessor;
using orbit_capture_client::CaptureListener;
using orbit_capture_client::ClientCaptureOptions;

using orbit_capture_file::CaptureFile;

using orbit_client_data::CallstackData;
using orbit_client_data::CallstackEvent;
using orbit_client_data::CallstackInfo;
using orbit_client_data::CaptureData;
using orbit_client_data::FunctionInfo;
using orbit_client_data::LinuxAddressInfo;
using orbit_client_data::ModuleData;
using orbit_client_data::PostProcessedSamplingData;
using orbit_client_data::ProcessData;
using orbit_client_data::SampledFunction;
using orbit_client_data::ScopeId;
using orbit_client_data::ScopeStats;
using orbit_client_data::ThreadID;
using orbit_client_data::ThreadStateSliceInfo;
using orbit_client_data::TimerBlock;
using orbit_client_data::TimerChain;
using orbit_client_data::TracepointInfoSet;
using orbit_client_data::UserDefinedCaptureData;

using orbit_client_protos::PresetInfo;
using orbit_client_protos::PresetModule;
using orbit_client_protos::TimerInfo;

using orbit_client_services::CrashManager;
using orbit_client_services::TracepointServiceClient;

using orbit_data_views::CallstackDataView;
using orbit_data_views::DataView;
using orbit_data_views::DataViewType;
using orbit_data_views::FunctionsDataView;
using orbit_data_views::ModulesDataView;
using orbit_data_views::PresetsDataView;
using orbit_data_views::SymbolLoadingState;
using orbit_data_views::TracepointsDataView;

using orbit_gl::MainWindowInterface;

using orbit_grpc_protos::CaptureFinished;
using orbit_grpc_protos::CaptureOptions;
using orbit_grpc_protos::CaptureStarted;
using orbit_grpc_protos::ClientCaptureEvent;
using orbit_grpc_protos::CrashOrbitServiceRequest_CrashType;
using orbit_grpc_protos::InstrumentedFunction;
using orbit_grpc_protos::ModuleInfo;
using orbit_grpc_protos::TracepointInfo;

using orbit_metrics_uploader::CaptureMetric;
using orbit_metrics_uploader::OrbitCaptureData;
using orbit_metrics_uploader::OrbitLogEvent;
using orbit_metrics_uploader::ScopedMetric;

using orbit_preset_file::PresetFile;

using orbit_symbol_provider::ModuleIdentifier;
using orbit_symbol_provider::SymbolLoadingOutcome;

using DynamicInstrumentationMethod =
    orbit_grpc_protos::CaptureOptions::DynamicInstrumentationMethod;
using UnwindingMethod = orbit_grpc_protos::CaptureOptions::UnwindingMethod;

namespace {

constexpr const char* kLibOrbitVulkanLayerSoFileName = "libOrbitVulkanLayer.so";
constexpr const char* kNtdllSoFileName = "ntdll.so";
constexpr const char* kWineSyscallDispatcherFunctionName = "__wine_syscall_dispatcher";
constexpr std::string_view kGgpVlkModulePathSubstring = "ggpvlk.so";

orbit_data_views::PresetLoadState GetPresetLoadStateForProcess(const PresetFile& preset,
                                                               const ProcessData* process) {
  if (process == nullptr) {
    return orbit_data_views::PresetLoadState::kNotLoadable;
  }

  size_t modules_not_found_count = 0;
  auto module_paths = preset.GetModulePaths();
  for (const auto& path : module_paths) {
    const std::string& module_path = path.string();
    if (!process->IsModuleLoadedByProcess(module_path)) {
      modules_not_found_count++;
    }
  }

  // Empty preset is also loadable
  if (modules_not_found_count == 0) {
    return orbit_data_views::PresetLoadState::kLoadable;
  }

  if (modules_not_found_count == module_paths.size()) {
    return orbit_data_views::PresetLoadState::kNotLoadable;
  }

  return orbit_data_views::PresetLoadState::kPartiallyLoadable;
}

orbit_metrics_uploader::CaptureStartData CreateCaptureStartData(
    const std::vector<FunctionInfo>& instrumented_functions, int64_t number_of_frame_tracks,
    bool thread_states, int64_t memory_information_sampling_period_ms,
    bool lib_orbit_vulkan_layer_loaded, uint64_t max_local_marker_depth_per_command_buffer,
    DynamicInstrumentationMethod dynamic_instrumentation_method,
    uint64_t callstack_samples_per_second, UnwindingMethod callstack_unwinding_method,
    bool auto_frame_track) {
  orbit_metrics_uploader::CaptureStartData capture_start_data{};
  capture_start_data.number_of_instrumented_functions = instrumented_functions.size();
  capture_start_data.number_of_frame_tracks = number_of_frame_tracks;
  capture_start_data.thread_states =
      thread_states ? orbit_metrics_uploader::OrbitCaptureData_ThreadStates_THREAD_STATES_ENABLED
                    : orbit_metrics_uploader::OrbitCaptureData_ThreadStates_THREAD_STATES_DISABLED;
  capture_start_data.memory_information_sampling_period_ms = memory_information_sampling_period_ms;
  capture_start_data.lib_orbit_vulkan_layer =
      lib_orbit_vulkan_layer_loaded
          ? orbit_metrics_uploader::OrbitCaptureData_LibOrbitVulkanLayer_LIB_LOADED
          : orbit_metrics_uploader::OrbitCaptureData_LibOrbitVulkanLayer_LIB_NOT_LOADED;
  if (max_local_marker_depth_per_command_buffer == std::numeric_limits<uint64_t>::max()) {
    capture_start_data.local_marker_depth_per_command_buffer =
        orbit_metrics_uploader::OrbitCaptureData_LocalMarkerDepthPerCommandBuffer_UNLIMITED;
  } else {
    capture_start_data.local_marker_depth_per_command_buffer =
        orbit_metrics_uploader::OrbitCaptureData_LocalMarkerDepthPerCommandBuffer_LIMITED;
    capture_start_data.max_local_marker_depth_per_command_buffer =
        max_local_marker_depth_per_command_buffer;
  }
  capture_start_data.dynamic_instrumentation_method =
      dynamic_instrumentation_method == orbit_grpc_protos::CaptureOptions::kKernelUprobes
          ? orbit_metrics_uploader::
                OrbitCaptureData_DynamicInstrumentationMethod_DYNAMIC_INSTRUMENTATION_METHOD_KERNEL
          : orbit_metrics_uploader::
                OrbitCaptureData_DynamicInstrumentationMethod_DYNAMIC_INSTRUMENTATION_METHOD_ORBIT;
  capture_start_data.callstack_samples_per_second = callstack_samples_per_second;
  capture_start_data.callstack_unwinding_method =
      callstack_unwinding_method == orbit_grpc_protos::CaptureOptions::kDwarf
          ? orbit_metrics_uploader::
                OrbitCaptureData_CallstackUnwindingMethod_CALLSTACK_UNWINDING_METHOD_DWARF
          : orbit_metrics_uploader::
                OrbitCaptureData_CallstackUnwindingMethod_CALLSTACK_UNWINDING_METHOD_FRAME_POINTER;
  capture_start_data.auto_frame_track =
      auto_frame_track
          ? orbit_metrics_uploader::OrbitCaptureData_AutoFrameTrack_AUTO_FRAME_TRACK_ENABLED
          : orbit_metrics_uploader::OrbitCaptureData_AutoFrameTrack_AUTO_FRAME_TRACK_DISABLED;
  return capture_start_data;
}

std::vector<std::filesystem::path> GetAllSymbolPaths() {
  orbit_client_symbols::QSettingsBasedStorageManager storage_manager;
  std::vector<std::filesystem::path> all_paths = storage_manager.LoadPaths();
  std::vector<std::string> temp_paths = absl::GetFlag(FLAGS_additional_symbol_paths);
  all_paths.insert(all_paths.end(), temp_paths.begin(), temp_paths.end());
  return all_paths;
}

ErrorMessageOr<std::optional<std::filesystem::path>> GetOverrideSymbolFileForModule(
    const ModuleData& module_data) {
  orbit_client_symbols::QSettingsBasedStorageManager storage_manager;
  absl::flat_hash_map<std::string, std::filesystem::path> mappings =
      storage_manager.LoadModuleSymbolFileMappings();
  if (!mappings.contains(module_data.file_path())) return std::nullopt;

  std::filesystem::path symbol_file_path = mappings[module_data.file_path()];

  OUTCOME_TRY(bool file_exists, orbit_base::FileOrDirectoryExists(symbol_file_path));

  if (!file_exists) {
    return ErrorMessage{absl::StrFormat(
        R"(A symbol file override is in place for module "%s", but the symbols file "%s" does not exist.)",
        module_data.file_path(), symbol_file_path.string())};
  }

  return symbol_file_path;
}

// Searches through an inout modules list for a module which paths contains path_substring. If one
// is found the module is erased from the modules list and returned. If not found, std::nullopt is
// returned.
[[nodiscard]] std::optional<const ModuleData*> FindAndEraseModuleByPathSubstringFromModuleList(
    std::vector<const ModuleData*>* modules, std::string_view path_substring) {
  auto module_it =
      std::find_if(modules->begin(), modules->end(), [path_substring](const ModuleData* module) {
        return absl::StrContains(module->file_path(), path_substring);
      });
  if (module_it == modules->end()) return std::nullopt;

  const ModuleData* module{*module_it};
  modules->erase(module_it);
  return module;
}

// Sorts a vector of modules with a prioritization list of module path substrings. This is done in a
// simple fashion by iterating through the prio_substrings list and searching for each substring in
// the the modules list (substring is contained in module path). If a module is found, it is amended
// to the result vector. After iterating through the prio_substring list, all remaining (not found)
// modules are added to the result vector.
[[nodiscard]] std::vector<const ModuleData*> SortModuleListWithPrioritizationList(
    std::vector<const ModuleData*> modules, absl::Span<std::string_view const> prio_substrings) {
  std::vector<const ModuleData*> prioritized_modules;
  prioritized_modules.reserve(modules.size());

  for (const auto& substring : prio_substrings) {
    std::optional<const ModuleData*> module_opt =
        FindAndEraseModuleByPathSubstringFromModuleList(&modules, substring);
    if (!module_opt.has_value()) continue;

    prioritized_modules.push_back(module_opt.value());
  }

  prioritized_modules.insert(prioritized_modules.end(), modules.begin(), modules.end());

  return prioritized_modules;
}

}  // namespace

bool DoZoom = false;

OrbitApp::OrbitApp(orbit_gl::MainWindowInterface* main_window,
                   orbit_base::MainThreadExecutor* main_thread_executor,
                   const orbit_base::CrashHandler* crash_handler,
                   orbit_metrics_uploader::MetricsUploader* metrics_uploader)
    : main_window_{main_window},
      main_thread_executor_(main_thread_executor),
      crash_handler_(crash_handler),
      metrics_uploader_(metrics_uploader) {
  ORBIT_CHECK(main_window_ != nullptr);

  thread_pool_ = orbit_base::ThreadPool::Create(
      /*thread_pool_min_size=*/4, /*thread_pool_max_size=*/256, /*thread_ttl=*/absl::Seconds(1),
      /*run_action=*/[](const std::unique_ptr<Action>& action) {
        ORBIT_START("Execute Action");
        action->Execute();
        ORBIT_STOP();
      });

  main_thread_id_ = std::this_thread::get_id();
  data_manager_ = std::make_unique<orbit_client_data::DataManager>(main_thread_id_);
  module_manager_ = std::make_unique<orbit_client_data::ModuleManager>();
  manual_instrumentation_manager_ = std::make_unique<ManualInstrumentationManager>();

  QObject::connect(
      &update_after_symbol_loading_throttle_, &orbit_qt_utils::Throttle::Triggered,
      &update_after_symbol_loading_throttle_,
      [this]() {
        UpdateAfterSymbolLoading();
        FireRefreshCallbacks();
      },
      Qt::QueuedConnection);

  if (absl::GetFlag(FLAGS_symbol_store_support)) InitRemoteSymbolProviders();
}

OrbitApp::~OrbitApp() {
  AbortCapture();
  RequestSymbolDownloadStop(module_manager_->GetAllModuleData(), false);
  thread_pool_->ShutdownAndWait();
}

void OrbitApp::OnCaptureFinished(const CaptureFinished& capture_finished) {
  ORBIT_LOG("CaptureFinished received: status=%s, error_message=\"%s\"",
            CaptureFinished::Status_Name(capture_finished.status()),
            capture_finished.error_message());
  main_thread_executor_->Schedule([this, capture_finished]() {
    switch (capture_finished.status()) {
      case orbit_grpc_protos::CaptureFinished::kSuccessful: {
        main_window_->AppendToCaptureLog(MainWindowInterface::CaptureLogSeverity::kInfo,
                                         GetCaptureTime(), "Capture finished.");
      } break;
      case orbit_grpc_protos::CaptureFinished::kInterruptedByService: {
        std::string full_message =
            absl::StrFormat("Capture interrupted prematurely by OrbitService: %s",
                            capture_finished.error_message());
        SendWarningToUi("Capture interrupted", full_message);
        main_window_->AppendToCaptureLog(MainWindowInterface::CaptureLogSeverity::kSevereWarning,
                                         GetCaptureTime(), full_message);
      } break;
      case orbit_grpc_protos::CaptureFinished::kFailed: {
        SendErrorToUi("Capture failed", capture_finished.error_message());
        main_window_->AppendToCaptureLog(
            MainWindowInterface::CaptureLogSeverity::kError, GetCaptureTime(),
            absl::StrFormat("Capture failed with error: %s.", capture_finished.error_message()));
      } break;
      case orbit_grpc_protos::
          CaptureFinished_Status_CaptureFinished_Status_INT_MIN_SENTINEL_DO_NOT_USE_:
        [[fallthrough]];
      case orbit_grpc_protos::
          CaptureFinished_Status_CaptureFinished_Status_INT_MAX_SENTINEL_DO_NOT_USE_:
        ORBIT_UNREACHABLE();
        break;
    }

    if (capture_finished.target_process_state_after_capture() == CaptureFinished::kCrashed) {
      main_window_->AppendToCaptureLog(
          MainWindowInterface::CaptureLogSeverity::kWarning, GetCaptureTime(),
          absl::StrFormat("The target process crashed during the capture with signal %d.",
                          capture_finished.target_process_termination_signal()));
    }
    metrics_capture_complete_data_.target_process_state_after_capture =
        static_cast<OrbitCaptureData::TargetProcessStateAfterCapture>(
            capture_finished.target_process_state_after_capture());
    metrics_capture_complete_data_.target_process_termination_signal =
        static_cast<OrbitCaptureData::TargetProcessTerminationSignal>(
            capture_finished.target_process_termination_signal());

    ORBIT_CHECK(HasCaptureData());
    if (GetCaptureData().file_path().has_value()) {
      capture_file_info_manager_.AddOrTouchCaptureFile(GetCaptureData().file_path().value(),
                                                       GetCaptureTime());
    }
  });
}

void OrbitApp::OnCaptureStarted(const orbit_grpc_protos::CaptureStarted& capture_started,
                                std::optional<std::filesystem::path> file_path,
                                absl::flat_hash_set<uint64_t> frame_track_function_ids) {
  // We need to block until initialization is complete to
  // avoid races when capture thread start processing data.
  absl::Mutex mutex;
  absl::MutexLock mutex_lock(&mutex);
  bool initialization_complete = false;

  if (file_path.has_value()) {
    metrics_capture_complete_data_.file_path = file_path.value();
  }

  main_thread_executor_->Schedule(
      [this, &initialization_complete, &mutex, &capture_started, file_path = std::move(file_path),
       frame_track_function_ids = std::move(frame_track_function_ids)]() mutable {
        absl::flat_hash_map<Track::Type, bool> track_type_visibility;
        bool had_capture = capture_window_->GetTimeGraph();
        if (had_capture) {
          track_type_visibility =
              capture_window_->GetTimeGraph()->GetTrackManager()->GetAllTrackTypesVisibility();
        }

        ClearCapture();

        // It is safe to do this write on the main thread, as the capture thread is suspended until
        // this task is completely executed.
        ConstructCaptureData(capture_started, file_path, std::move(frame_track_function_ids),
                             data_source_);
        GetMutableCaptureData().set_memory_warning_threshold_kb(
            data_manager_->memory_warning_threshold_kb());
        capture_window_->CreateTimeGraph(&GetMutableCaptureData());
        orbit_gl::TrackManager* track_manager = GetMutableTimeGraph()->GetTrackManager();
        track_manager->SetIsDataFromSavedCapture(data_source_ ==
                                                 CaptureData::DataSource::kLoadedCapture);
        if (had_capture) {
          track_manager->RestoreAllTrackTypesVisibility(track_type_visibility);
        }

        frame_track_online_processor_ =
            orbit_gl::FrameTrackOnlineProcessor(GetCaptureData(), GetMutableTimeGraph());

        ORBIT_CHECK(capture_started_callback_ != nullptr);
        capture_started_callback_(file_path);

        if (!GetCaptureData().instrumented_functions().empty()) {
          ORBIT_CHECK(select_live_tab_callback_);
          select_live_tab_callback_();
        }

        FireRefreshCallbacks();

        main_window_->AppendToCaptureLog(
            MainWindowInterface::CaptureLogSeverity::kInfo, absl::ZeroDuration(),
            absl::StrFormat(
                "Capture started on %s.",
                absl::FormatTime(absl::FromUnixNanos(capture_started.capture_start_unix_time_ns()),
                                 absl::LocalTimeZone())));

        orbit_version::Version capture_version{capture_started.orbit_version_major(),
                                               capture_started.orbit_version_minor()};
        orbit_version::Version current_version = orbit_version::GetVersion();
        if (capture_version > current_version) {
          std::string warning_message = absl::Substitute(
              "The capture was taken with Orbit version $0.$1, which is higher than the current "
              "version. Please use Orbit v$0.$1 or above to ensure all features are supported.",
              capture_version.major_version, capture_version.minor_version);
          main_window_->AppendToCaptureLog(MainWindowInterface::CaptureLogSeverity::kWarning,
                                           absl::ZeroDuration(), warning_message);
        }
        absl::MutexLock lock(&mutex);
        initialization_complete = true;
      });

  mutex.Await(absl::Condition(&initialization_complete));
}

Future<void> OrbitApp::OnCaptureComplete() {
  GetMutableCaptureData().OnCaptureComplete();

  GetMutableCaptureData().ComputeVirtualAddressOfInstrumentedFunctionsIfNecessary(*module_manager_);

  GetMutableCaptureData().FilterBrokenCallstacks();
  PostProcessedSamplingData post_processed_sampling_data =
      orbit_client_model::CreatePostProcessedSamplingData(GetCaptureData().GetCallstackData(),
                                                          GetCaptureData(), *module_manager_);

  ORBIT_LOG("The capture contains %u intervals with incomplete data",
            GetCaptureData().incomplete_data_intervals().size());

  return main_thread_executor_->Schedule(
      [this, post_processed_sampling_data = std::move(post_processed_sampling_data)]() mutable {
        ORBIT_SCOPE("OnCaptureComplete");
        TrySaveUserDefinedCaptureInfo();
        RefreshFrameTracks();
        GetMutableCaptureData().set_post_processed_sampling_data(
            std::move(post_processed_sampling_data));
        RefreshCaptureView();

        SetSamplingReport(&GetCaptureData().GetCallstackData(),
                          &GetCaptureData().post_processed_sampling_data());
        SetTopDownView(GetCaptureData());
        SetBottomUpView(GetCaptureData());

        ORBIT_CHECK(capture_stopped_callback_);
        capture_stopped_callback_();

        const orbit_client_data::ThreadSampleData* all_threads_sample_data =
            GetCaptureData().post_processed_sampling_data().GetSummary();
        if (all_threads_sample_data != nullptr) {
          metrics_capture_complete_data_.number_of_callstack_samples =
              all_threads_sample_data->samples_count;
          metrics_capture_complete_data_.number_of_unwinding_errors =
              all_threads_sample_data->unwinding_errors_count;
        }

        FireRefreshCallbacks();

        if (absl::GetFlag(FLAGS_auto_symbol_loading)) {
          std::ignore = LoadAllSymbols();
        }
      });
}

Future<void> OrbitApp::OnCaptureCancelled() {
  return main_thread_executor_->Schedule([this]() mutable {
    ORBIT_SCOPE("OnCaptureCancelled");
    ORBIT_CHECK(capture_failed_callback_);
    capture_failed_callback_();

    ClearCapture();
    if (absl::GetFlag(FLAGS_auto_symbol_loading)) {
      std::ignore = LoadAllSymbols();
    }
  });
}

Future<void> OrbitApp::OnCaptureFailed(ErrorMessage error_message) {
  return main_thread_executor_->Schedule(
      [this, error_message = std::move(error_message)]() mutable {
        ORBIT_SCOPE("OnCaptureFailed");
        ORBIT_CHECK(capture_failed_callback_);
        capture_failed_callback_();

        ClearCapture();
        SendErrorToUi("Error in capture", error_message.message());
        if (absl::GetFlag(FLAGS_auto_symbol_loading)) {
          std::ignore = LoadAllSymbols();
        }
      });
}

void OrbitApp::OnTimer(const TimerInfo& timer_info) {
  CaptureMetricProcessTimer(timer_info);

  CaptureData& capture_data = GetMutableCaptureData();
  capture_data.UpdateScopeStats(timer_info);

  if (timer_info.function_id() == 0) {
    GetMutableTimeGraph()->ProcessTimer(timer_info, nullptr);
    return;
  }

  const InstrumentedFunction& func =
      capture_data.instrumented_functions().at(timer_info.function_id());
  GetMutableTimeGraph()->ProcessTimer(timer_info, &func);
  frame_track_online_processor_.ProcessTimer(timer_info, func);
}

void OrbitApp::OnApiStringEvent(const orbit_client_data::ApiStringEvent& api_string_event) {
  GetMutableTimeGraph()->ProcessApiStringEvent(api_string_event);
}

void OrbitApp::OnApiTrackValue(const orbit_client_data::ApiTrackValue& api_track_value) {
  metrics_capture_complete_data_.number_of_manual_tracked_value_timers++;
  GetMutableTimeGraph()->ProcessApiTrackValueEvent(api_track_value);
}

void OrbitApp::OnKeyAndString(uint64_t key, std::string str) {
  string_manager_.AddIfNotPresent(key, std::move(str));
}

void OrbitApp::UpdateModulesAbortCaptureIfModuleWithoutBuildIdNeedsReload(
    absl::Span<const ModuleInfo> module_infos) {
  auto not_updated_modules = module_manager_->AddOrUpdateNotLoadedModules(module_infos);

  if (!not_updated_modules.empty()) {
    std::string module_paths;
    for (const auto* updated_module : not_updated_modules) {
      if (!module_paths.empty()) module_paths += ", ";
      module_paths += updated_module->file_path();
    }

    std::string error_message = absl::StrFormat(
        "Following modules have been updated during the capture: \"%s\", since they do not have "
        "build_id, this will likely result in undefined behaviour/incorrect data being produced, "
        "please recompile these modules with build_id support by adding \"-Wl,--build-id\" to "
        "compile flags (or removing \"-Wl,--build-id=none\" from them).",
        module_paths);
    SendErrorToUi("Capture Error", error_message);
    ORBIT_ERROR("%s", error_message);
    main_thread_executor_->Schedule([this] { AbortCapture(); });
  }
}

void OrbitApp::OnModuleUpdate(uint64_t /*timestamp_ns*/, ModuleInfo module_info) {
  UpdateModulesAbortCaptureIfModuleWithoutBuildIdNeedsReload({module_info});
  GetMutableCaptureData().mutable_process()->AddOrUpdateModuleInfo(module_info);
  main_thread_executor_->Schedule([this]() { FireRefreshCallbacks(DataViewType::kLiveFunctions); });
}

void OrbitApp::OnModulesSnapshot(uint64_t /*timestamp_ns*/, std::vector<ModuleInfo> module_infos) {
  UpdateModulesAbortCaptureIfModuleWithoutBuildIdNeedsReload(module_infos);
  GetMutableCaptureData().mutable_process()->UpdateModuleInfos(module_infos);
  main_thread_executor_->Schedule([this]() { FireRefreshCallbacks(DataViewType::kLiveFunctions); });
}

void OrbitApp::OnPresentEvent(const orbit_grpc_protos::PresentEvent& /*present_event*/) {}

void OrbitApp::OnWarningEvent(orbit_grpc_protos::WarningEvent warning_event) {
  main_thread_executor_->Schedule([this, warning_event = std::move(warning_event)]() {
    main_window_->AppendToCaptureLog(MainWindowInterface::CaptureLogSeverity::kWarning,
                                     GetCaptureTimeAt(warning_event.timestamp_ns()),
                                     warning_event.message());
  });
}

void OrbitApp::OnClockResolutionEvent(
    orbit_grpc_protos::ClockResolutionEvent clock_resolution_event) {
  main_thread_executor_->Schedule([this, clock_resolution_event]() {
    constexpr uint64_t kClockResolutionWarningThresholdNs = 10 * 1000;
    uint64_t timestamp_ns = clock_resolution_event.timestamp_ns();
    const uint64_t clock_resolution_ns = clock_resolution_event.clock_resolution_ns();

    if (clock_resolution_ns == 0) {
      main_window_->AppendToCaptureLog(MainWindowInterface::CaptureLogSeverity::kSevereWarning,
                                       GetCaptureTimeAt(timestamp_ns),
                                       "Failed to estimate clock resolution.");

    } else if (clock_resolution_ns < kClockResolutionWarningThresholdNs) {
      main_window_->AppendToCaptureLog(
          MainWindowInterface::CaptureLogSeverity::kInfo, GetCaptureTimeAt(timestamp_ns),
          absl::StrFormat("Clock resolution is %u ns.", clock_resolution_ns));

    } else {
      std::string message = absl::StrFormat(
          "Clock resolution is high (%u ns): some timings may be inaccurate.", clock_resolution_ns);
      main_window_->AppendToCaptureLog(MainWindowInterface::CaptureLogSeverity::kSevereWarning,
                                       GetCaptureTimeAt(timestamp_ns), message);

      if (!IsLoadingCapture()) {
        constexpr const char* kDontShowAgainHighClockResolutionWarningKey =
            "DontShowAgainHighClockResolutionWarning";
        main_window_->ShowWarningWithDontShowAgainCheckboxIfNeeded(
            "High clock resolution", message, kDontShowAgainHighClockResolutionWarningKey);
      }
    }
  });
}

void OrbitApp::OnErrorsWithPerfEventOpenEvent(
    orbit_grpc_protos::ErrorsWithPerfEventOpenEvent errors_with_perf_event_open_event) {
  main_thread_executor_->Schedule([this, errors_with_perf_event_open_event]() {
    std::string log_message =
        absl::StrFormat("There were errors with perf_event_open, in particular with: %s.",
                        absl::StrJoin(errors_with_perf_event_open_event.failed_to_open(), ", "));
    main_window_->AppendToCaptureLog(
        MainWindowInterface::CaptureLogSeverity::kSevereWarning,
        GetCaptureTimeAt(errors_with_perf_event_open_event.timestamp_ns()), log_message);

    if (!IsLoadingCapture()) {
      std::string box_message =
          log_message + "\n\nSome information will probably be missing from the capture.";
      constexpr const char* kDontShowAgainErrorsWithPerfEventOpenWarningKey =
          "DontShowAgainErrorsWithPerfEventOpenWarning";
      main_window_->ShowWarningWithDontShowAgainCheckboxIfNeeded(
          "Errors with perf_event_open", box_message,
          kDontShowAgainErrorsWithPerfEventOpenWarningKey);
    }
  });
}

void OrbitApp::OnWarningInstrumentingWithUprobesEvent(
    orbit_grpc_protos::WarningInstrumentingWithUprobesEvent
        warning_instrumenting_with_uprobes_event) {
  main_thread_executor_->Schedule([this, warning_instrumenting_with_uprobes_event = std::move(
                                             warning_instrumenting_with_uprobes_event)]() {
    std::string message = "Uprobes likely failed to instrument some functions:\n";
    for (const auto& function :
         warning_instrumenting_with_uprobes_event.functions_that_failed_to_instrument()) {
      absl::StrAppend(&message, "* ", function.error_message(), "\n");
    }
    absl::StrAppend(&message,
                    "\nConsider choosing the method \"Orbit\" for dynamic instrumentation in the "
                    "Capture Options dialog.\n");

    main_window_->AppendToCaptureLog(
        MainWindowInterface::CaptureLogSeverity::kWarning,
        GetCaptureTimeAt(warning_instrumenting_with_uprobes_event.timestamp_ns()), message);
  });
}

void OrbitApp::OnErrorEnablingOrbitApiEvent(
    orbit_grpc_protos::ErrorEnablingOrbitApiEvent error_enabling_orbit_api_event) {
  main_thread_executor_->Schedule(
      [this, error_enabling_orbit_api_event = std::move(error_enabling_orbit_api_event)]() {
        main_window_->AppendToCaptureLog(
            MainWindowInterface::CaptureLogSeverity::kSevereWarning,
            GetCaptureTimeAt(error_enabling_orbit_api_event.timestamp_ns()),
            error_enabling_orbit_api_event.message());

        if (!IsLoadingCapture()) {
          constexpr const char* kDontShowAgainErrorEnablingOrbitApiWarningKey =
              "DontShowAgainErrorEnablingOrbitApiWarning";
          main_window_->ShowWarningWithDontShowAgainCheckboxIfNeeded(
              "Could not enable Orbit API", error_enabling_orbit_api_event.message(),
              kDontShowAgainErrorEnablingOrbitApiWarningKey);
        }
      });
}

void OrbitApp::OnErrorEnablingUserSpaceInstrumentationEvent(
    orbit_grpc_protos::ErrorEnablingUserSpaceInstrumentationEvent error_event) {
  main_thread_executor_->Schedule([this, error_event = std::move(error_event)]() {
    const std::string message =
        absl::StrCat(error_event.message(),
                     "\nAll functions will be instrumented using the slower kernel (uprobes) "
                     "functionality.\n");
    main_window_->AppendToCaptureLog(MainWindowInterface::CaptureLogSeverity::kSevereWarning,
                                     GetCaptureTimeAt(error_event.timestamp_ns()), message);
    if (!IsLoadingCapture()) {
      // We use 'SendWarningToUi' here since we don't want the "don't show again" checkbox. The user
      // should always be notified.
      SendWarningToUi("Could not enable dynamic instrumentation", message);
    }
  });
}

void OrbitApp::OnWarningInstrumentingWithUserSpaceInstrumentationEvent(
    orbit_grpc_protos::WarningInstrumentingWithUserSpaceInstrumentationEvent warning_event) {
  main_thread_executor_->Schedule([this, warning_event = std::move(warning_event)]() {
    std::string message = "Failed to instrument some functions with the \"Orbit\" method:\n";
    for (const auto& function : warning_event.functions_that_failed_to_instrument()) {
      absl::StrAppend(&message, "* ", function.error_message(), "\n");
    }
    absl::StrAppend(&message,
                    "\nThe functions above will be instrumented using the slower kernel (uprobes) "
                    "functionality.\n");

    main_window_->AppendToCaptureLog(MainWindowInterface::CaptureLogSeverity::kWarning,
                                     GetCaptureTimeAt(warning_event.timestamp_ns()), message);
  });
}

static constexpr const char* kIncompleteDataLogMessage =
    "The capture contains one or more time ranges with incomplete data. Some information might "
    "be inaccurate.";

void OrbitApp::OnLostPerfRecordsEvent(
    orbit_grpc_protos::LostPerfRecordsEvent lost_perf_records_event) {
  main_thread_executor_->Schedule(
      [this, lost_perf_records_event = std::move(lost_perf_records_event)]() {
        uint64_t lost_end_timestamp_ns = lost_perf_records_event.end_timestamp_ns();
        uint64_t lost_start_timestamp_ns =
            lost_end_timestamp_ns - lost_perf_records_event.duration_ns();
        if (GetCaptureData().incomplete_data_intervals().empty()) {
          // This is only reported once in the Capture Log.
          main_window_->AppendToCaptureLog(MainWindowInterface::CaptureLogSeverity::kWarning,
                                           GetCaptureTimeAt(lost_start_timestamp_ns),
                                           kIncompleteDataLogMessage);
        }
        GetMutableCaptureData().AddIncompleteDataInterval(lost_start_timestamp_ns,
                                                          lost_end_timestamp_ns);
      });
}

void OrbitApp::OnOutOfOrderEventsDiscardedEvent(
    orbit_grpc_protos::OutOfOrderEventsDiscardedEvent out_of_order_events_discarded_event) {
  main_thread_executor_->Schedule([this, out_of_order_events_discarded_event =
                                             std::move(out_of_order_events_discarded_event)]() {
    uint64_t discarded_end_timestamp_ns = out_of_order_events_discarded_event.end_timestamp_ns();
    uint64_t discarded_start_timestamp_ns =
        discarded_end_timestamp_ns - out_of_order_events_discarded_event.duration_ns();
    if (GetCaptureData().incomplete_data_intervals().empty()) {
      main_window_->AppendToCaptureLog(MainWindowInterface::CaptureLogSeverity::kWarning,
                                       GetCaptureTimeAt(discarded_start_timestamp_ns),
                                       kIncompleteDataLogMessage);
    }
    GetMutableCaptureData().AddIncompleteDataInterval(discarded_start_timestamp_ns,
                                                      discarded_end_timestamp_ns);
  });
}

void OrbitApp::OnValidateFramePointers(std::vector<const ModuleData*> modules_to_validate) {
  thread_pool_->Schedule([modules_to_validate = std::move(modules_to_validate), this] {
    frame_pointer_validator_client_->AnalyzeModules(modules_to_validate);
  });
}

std::unique_ptr<OrbitApp> OrbitApp::Create(
    orbit_gl::MainWindowInterface* main_window,
    orbit_base::MainThreadExecutor* main_thread_executor,
    const orbit_base::CrashHandler* crash_handler,
    orbit_metrics_uploader::MetricsUploader* metrics_uploader) {
  return std::make_unique<OrbitApp>(main_window, main_thread_executor, crash_handler,
                                    metrics_uploader);
}

void OrbitApp::PostInit(bool is_connected) {
  if (is_connected) {
    ORBIT_CHECK(process_manager_ != nullptr);

    capture_client_ = std::make_unique<CaptureClient>(grpc_channel_);

    orbit_client_symbols::QSettingsBasedStorageManager storage_manager;
    download_disabled_modules_ = storage_manager.LoadDisabledModulePaths();

    if (GetTargetProcess() != nullptr) {
      std::ignore = UpdateProcessAndModuleList();
    }

    frame_pointer_validator_client_ =
        std::make_unique<FramePointerValidatorClient>(this, grpc_channel_);

    if (IsDevMode()) {
      crash_manager_ = CrashManager::Create(grpc_channel_);
    }
  }

  ListPresets();

  if (!absl::GetFlag(FLAGS_enable_tracepoint_feature)) {
    return;
  }

  thread_pool_->Schedule([this] {
    std::unique_ptr<TracepointServiceClient> tracepoint_manager =
        TracepointServiceClient::Create(grpc_channel_);

    ErrorMessageOr<std::vector<TracepointInfo>> result = tracepoint_manager->GetTracepointList();

    if (result.has_error()) {
      ORBIT_ERROR("Error retrieving tracepoints: %s", result.error().message());
      SendErrorToUi("Error retrieving tracepoints", result.error().message());
      return;
    }

    main_thread_executor_->Schedule([result, this]() {
      tracepoints_data_view_->SetTracepoints(result.value());

      FireRefreshCallbacks(DataViewType::kTracepoints);
    });
  });
}

void OrbitApp::InitRemoteSymbolProviders() {
  download_manager_.emplace();

  auto client_result = orbit_ggp::CreateClient();
  if (client_result.has_error()) {
    ORBIT_ERROR("Error creating ggp client: %s\nStadia symbol provider won't be created.",
                client_result.error().message());
  } else {
    ggp_client_ = std::move(client_result.value());
    stadia_symbol_provider_.emplace(&symbol_helper_, &*download_manager_, ggp_client_.get());
  }

  microsoft_symbol_provider_.emplace(&symbol_helper_, &*download_manager_);
}

static std::vector<std::filesystem::path> ListRegularFilesWithExtension(
    const std::filesystem::path& directory, std::string_view extension) {
  std::vector<std::filesystem::path> files;

  std::error_code error;
  auto directory_iterator = std::filesystem::directory_iterator(directory, error);
  if (error) {
    ORBIT_ERROR("Unable to list files in directory \"%s\": %s", directory.string(),
                error.message());
    return {};
  }

  for (auto it = std::filesystem::begin(directory_iterator),
            end = std::filesystem::end(directory_iterator);
       it != end; it.increment(error)) {
    if (error) {
      ORBIT_ERROR("Iterating directory \"%s\": %s (increment failed, stopping)", directory.string(),
                  error.message());
      break;
    }

    const auto& path = it->path();
    bool is_regular_file = std::filesystem::is_regular_file(path, error);
    if (error) {
      ORBIT_ERROR("Unable to stat \"%s\": %s (ignoring)", path.string(), error.message());
      continue;
    }

    if (!is_regular_file) {
      continue;
    }

    if (path.extension().string() != extension) {
      continue;
    }

    files.push_back(path);
  }

  return files;
}

void OrbitApp::ListPresets() {
  std::vector<std::filesystem::path> preset_filenames =
      ListRegularFilesWithExtension(orbit_paths::CreateOrGetPresetDirUnsafe(), ".opr");
  std::vector<PresetFile> presets;
  for (const std::filesystem::path& filename : preset_filenames) {
    ErrorMessageOr<PresetFile> preset_result = ReadPresetFromFile(filename);
    if (preset_result.has_error()) {
      ORBIT_ERROR("Loading preset from \"%s\" failed: %s", filename.string(),
                  preset_result.error().message());
      continue;
    }

    presets.push_back(std::move(preset_result.value()));
  }

  presets_data_view_->SetPresets(std::move(presets));
}

void OrbitApp::RefreshCaptureView() {
  ORBIT_SCOPE_FUNCTION;
  RequestUpdatePrimitives();
  FireRefreshCallbacks();
  DoZoom = true;  // TODO: remove global, review logic
}

void OrbitApp::RenderImGuiDebugUI() {
  ORBIT_CHECK(debug_canvas_ != nullptr);
  ORBIT_CHECK(capture_window_ != nullptr);
  ScopeImguiContext context(debug_canvas_->GetImGuiContext());
  Orbit_ImGui_NewFrame(debug_canvas_);

  ImGuiWindowFlags window_flags = ImGuiWindowFlags_NoTitleBar | ImGuiWindowFlags_NoResize |
                                  ImGuiWindowFlags_NoMove | ImGuiWindowFlags_NoCollapse;

  ImGui::SetNextWindowSize(ImGui::GetIO().DisplaySize);
  ImGui::SetNextWindowPos(ImVec2(0, 0));
  ImGui::PushStyleColor(ImGuiCol_WindowBg, IM_COL32(25, 25, 25, 255));
  ImGui::PushStyleVar(ImGuiStyleVar_WindowRounding, 0.0f);
  ImGui::Begin("OrbitDebug", nullptr, window_flags);

  if (ImGui::BeginTabBar("DebugTabBar", ImGuiTabBarFlags_None)) {
    if (ImGui::BeginTabItem("CaptureWindow")) {
      capture_window_->RenderImGuiDebugUI();
      ImGui::EndTabItem();
    }

    if (introspection_window_) {
      if (ImGui::BeginTabItem("Introspection")) {
        introspection_window_->RenderImGuiDebugUI();
        ImGui::EndTabItem();
      }
    }

    if (ImGui::BeginTabItem("Misc")) {
      static bool show_imgui_demo = false;
      ImGui::Checkbox("Show ImGui Demo", &show_imgui_demo);
      if (show_imgui_demo) {
        ImGui::ShowDemoWindow();
      }
      ImGui::EndTabItem();
    }

    ImGui::EndTabBar();
  }

  ImGui::PopStyleVar();
  ImGui::PopStyleColor();
  ImGui::End();

  ImGui::Render();
  Orbit_ImGui_RenderDrawLists(ImGui::GetDrawData());
  debug_canvas_->RequestRedraw();
}

void OrbitApp::Disassemble(uint32_t pid, const FunctionInfo& function) {
  ScopedMetric metric{metrics_uploader_, OrbitLogEvent::ORBIT_DISASSEMBLY_SHOW};

  ORBIT_CHECK(process_ != nullptr);
  const ModuleData* module = GetModuleByModuleIdentifier(function.module_id());
  ORBIT_CHECK(module != nullptr);
  const bool is_64_bit = process_->is_64_bit();
  std::optional<uint64_t> absolute_address = function.GetAbsoluteAddress(*process_, *module);
  if (!absolute_address.has_value()) {
    SendErrorToUi(
        "Error reading memory",
        absl::StrFormat(
            R"(Unable to calculate function "%s" address, likely because the module "%s" is not loaded.)",
            function.pretty_name(), module->file_path()),
        std::move(metric));
    return;
  }
  thread_pool_->Schedule([this, metric = std::move(metric),
                          absolute_address = absolute_address.value(), is_64_bit, pid,
                          function]() mutable {
    auto result = GetProcessManager()->LoadProcessMemory(pid, absolute_address, function.size());
    if (!result.has_value()) {
      SendErrorToUi("Error reading memory",
                    absl::StrFormat("Could not read process memory: %s.", result.error().message()),
                    std::move(metric));
      return;
    }

    const std::string& memory = result.value();
    orbit_code_report::Disassembler disasm;
    disasm.AddLine(absl::StrFormat("asm: /* %s */", function.pretty_name()));
    disasm.Disassemble(*process_, *module_manager_, memory.data(), memory.size(), absolute_address,
                       is_64_bit);
    if (!HasCaptureData() || !GetCaptureData().has_post_processed_sampling_data()) {
      orbit_code_report::DisassemblyReport empty_report(disasm, absolute_address);
      SendDisassemblyToUi(function, disasm.GetResult(), std::move(empty_report));
      return;
    }
    const CaptureData& capture_data = GetCaptureData();
    const PostProcessedSamplingData& post_processed_sampling_data =
        capture_data.post_processed_sampling_data();
    const orbit_client_data::ThreadSampleData* const thread_sample_data =
        post_processed_sampling_data.GetSummary();

    if (thread_sample_data == nullptr) {
      orbit_code_report::DisassemblyReport empty_report(disasm, absolute_address);
      SendDisassemblyToUi(function, disasm.GetResult(), std::move(empty_report));
      return;
    }

    orbit_code_report::DisassemblyReport report(
        disasm, absolute_address, *thread_sample_data,
        post_processed_sampling_data.GetCountOfFunction(absolute_address),
        capture_data.GetCallstackData().GetCallstackEventsCount());
    SendDisassemblyToUi(function, disasm.GetResult(), std::move(report));
  });
}

void OrbitApp::ShowSourceCode(const orbit_client_data::FunctionInfo& function) {
  ScopedMetric metric{metrics_uploader_, OrbitLogEvent::ORBIT_SOURCE_CODE_SHOW};

  const ModuleData* module = GetModuleByModuleIdentifier(function.module_id());

  auto loaded_module = RetrieveModuleWithDebugInfo(module);

  (void)loaded_module.Then(main_thread_executor_, [this, module, function,
                                                   metric = std::move(metric)](
                                                      const ErrorMessageOr<std::filesystem::path>&
                                                          local_file_path_or_error) mutable {
    const std::string error_title = "Error showing source code";
    if (local_file_path_or_error.has_error()) {
      SendErrorToUi(error_title, local_file_path_or_error.error().message(), std::move(metric));
      return;
    }

    const auto elf_file = orbit_object_utils::CreateElfFile(local_file_path_or_error.value());
    const auto decl_line_info_or_error =
        elf_file.value()->GetLocationOfFunction(function.address());
    if (decl_line_info_or_error.has_error()) {
      SendErrorToUi(
          error_title,
          absl::StrFormat(
              R"(Could not find source code location of function "%s" in module "%s": %s)",
              function.pretty_name(), module->file_path(),
              decl_line_info_or_error.error().message()),
          std::move(metric));
      return;
    }

    const auto& line_info = decl_line_info_or_error.value();
    auto source_file_path = std::filesystem::path{line_info.source_file()}.lexically_normal();

    std::optional<std::unique_ptr<orbit_code_report::CodeReport>> code_report;

    if (HasCaptureData() && GetCaptureData().has_post_processed_sampling_data()) {
      const auto& sampling_data = GetCaptureData().post_processed_sampling_data();
      const auto absolute_address = function.GetAbsoluteAddress(*process_, *module);

      if (!absolute_address.has_value()) {
        SendErrorToUi(
            error_title,
            absl::StrFormat(
                R"(Unable calculate function "%s" address in memory, likely because the module "%s" is not loaded)",
                function.pretty_name(), module->file_path()),
            std::move(metric));
        return;
      }

      const orbit_client_data::ThreadSampleData* summary = sampling_data.GetSummary();
      if (summary != nullptr) {
        code_report = std::make_unique<orbit_code_report::SourceCodeReport>(
            line_info.source_file(), function, absolute_address.value(), elf_file.value().get(),
            *summary, GetCaptureData().GetCallstackData().GetCallstackEventsCount());
      }
    }

    main_window_->ShowSourceCode(source_file_path, line_info.source_line(), std::move(code_report),
                                 &metric);
  });
}

void OrbitApp::MainTick() {
  ORBIT_SCOPE("OrbitApp::MainTick");

  if (DoZoom && HasCaptureData()) {
    capture_window_->ZoomAll();
    RequestUpdatePrimitives();
    DoZoom = false;
  }
}

void OrbitApp::SetCaptureWindow(CaptureWindow* capture) {
  ORBIT_CHECK(capture_window_ == nullptr);
  capture_window_ = capture;
  capture_window_->set_draw_help(false);
}

void OrbitApp::SetDebugCanvas(GlCanvas* debug_canvas) {
  ORBIT_CHECK(debug_canvas_ == nullptr);
  debug_canvas_ = debug_canvas;
  debug_canvas_->EnableImGui();
  debug_canvas_->AddRenderCallback([this]() { RenderImGuiDebugUI(); });
}

void OrbitApp::SetIntrospectionWindow(IntrospectionWindow* introspection_window) {
  ORBIT_CHECK(introspection_window_ == nullptr);
  introspection_window_ = introspection_window;
}

void OrbitApp::StopIntrospection() {
  if (introspection_window_) {
    introspection_window_->StopIntrospection();
  }
}

void OrbitApp::RequestUpdatePrimitives() {
  if (capture_window_ != nullptr) {
    capture_window_->RequestUpdatePrimitives();
  }
}

void OrbitApp::SetSamplingReport(
    const CallstackData* callstack_data,
    const orbit_client_data::PostProcessedSamplingData* post_processed_sampling_data) {
  ORBIT_SCOPE_FUNCTION;
  // clear old sampling report
  if (sampling_report_ != nullptr) {
    sampling_report_->ClearReport();
  }

  auto report = std::make_shared<SamplingReport>(this, callstack_data, post_processed_sampling_data,
                                                 metrics_uploader_);
  orbit_data_views::DataView* callstack_data_view = GetOrCreateDataView(DataViewType::kCallstack);
  ORBIT_CHECK(sampling_reports_callback_);
  sampling_reports_callback_(callstack_data_view, report);

  sampling_report_ = report;
}

void OrbitApp::ClearSamplingReport() {
  if (sampling_report_ == nullptr) return;
  sampling_report_->ClearReport();
  sampling_report_.reset();
  ORBIT_CHECK(sampling_reports_callback_);
  sampling_reports_callback_(GetOrCreateDataView(DataViewType::kCallstack), nullptr);
}

void OrbitApp::SetSelectionReport(
    const CallstackData* selection_callstack_data,
    const orbit_client_data::PostProcessedSamplingData* selection_post_processed_sampling_data,
    bool has_summary) {
  // clear old selection report
  if (selection_report_ != nullptr) {
    selection_report_->ClearReport();
  }

  auto report = std::make_shared<SamplingReport>(this, selection_callstack_data,
                                                 selection_post_processed_sampling_data,
                                                 metrics_uploader_, has_summary);
  orbit_data_views::DataView* callstack_data_view = GetOrCreateSelectionCallstackDataView();

  selection_report_ = report;
  ORBIT_CHECK(selection_report_callback_);
  selection_report_callback_(callstack_data_view, report);
  FireRefreshCallbacks();
}

void OrbitApp::ClearSelectionReport() {
  if (selection_report_ == nullptr) return;
  selection_report_->ClearReport();
  selection_report_.reset();
  ORBIT_CHECK(sampling_reports_callback_);
  selection_report_callback_(GetOrCreateDataView(DataViewType::kCallstack), nullptr);
}

void OrbitApp::SetTopDownView(const CaptureData& capture_data) {
  ORBIT_SCOPE_FUNCTION;
  ORBIT_CHECK(top_down_view_callback_);
  std::unique_ptr<CallTreeView> top_down_view =
      CallTreeView::CreateTopDownViewFromPostProcessedSamplingData(
          capture_data.post_processed_sampling_data(), *module_manager_, capture_data);
  top_down_view_callback_(std::move(top_down_view));
}

void OrbitApp::ClearTopDownView() {
  ORBIT_CHECK(top_down_view_callback_);
  top_down_view_callback_(std::make_unique<CallTreeView>());
}

void OrbitApp::SetSelectionTopDownView(
    const PostProcessedSamplingData& selection_post_processed_data,
    const CaptureData& capture_data) {
  ORBIT_CHECK(selection_top_down_view_callback_);
  std::unique_ptr<CallTreeView> selection_top_down_view =
      CallTreeView::CreateTopDownViewFromPostProcessedSamplingData(selection_post_processed_data,
                                                                   *module_manager_, capture_data);
  selection_top_down_view_callback_(std::move(selection_top_down_view));
}

void OrbitApp::ClearSelectionTopDownView() {
  ORBIT_CHECK(selection_top_down_view_callback_);
  selection_top_down_view_callback_(std::make_unique<CallTreeView>());
}

void OrbitApp::SetBottomUpView(const CaptureData& capture_data) {
  ORBIT_SCOPE_FUNCTION;
  ORBIT_CHECK(bottom_up_view_callback_);
  std::unique_ptr<CallTreeView> bottom_up_view =
      CallTreeView::CreateBottomUpViewFromPostProcessedSamplingData(
          capture_data.post_processed_sampling_data(), *module_manager_, capture_data);
  bottom_up_view_callback_(std::move(bottom_up_view));
}

void OrbitApp::ClearBottomUpView() {
  ORBIT_CHECK(bottom_up_view_callback_);
  bottom_up_view_callback_(std::make_unique<CallTreeView>());
}

void OrbitApp::SetSelectionBottomUpView(
    const PostProcessedSamplingData& selection_post_processed_data,
    const CaptureData& capture_data) {
  ORBIT_CHECK(selection_bottom_up_view_callback_);
  std::unique_ptr<CallTreeView> selection_bottom_up_view =
      CallTreeView::CreateBottomUpViewFromPostProcessedSamplingData(selection_post_processed_data,
                                                                    *module_manager_, capture_data);
  selection_bottom_up_view_callback_(std::move(selection_bottom_up_view));
}

void OrbitApp::ClearSelectionBottomUpView() {
  ORBIT_CHECK(selection_bottom_up_view_callback_);
  selection_bottom_up_view_callback_(std::make_unique<CallTreeView>());
}

absl::Duration OrbitApp::GetCaptureTime() const {
  const TimeGraph* time_graph = GetTimeGraph();
  return absl::Nanoseconds((time_graph == nullptr) ? 0 : time_graph->GetCaptureTimeSpanNs());
}

absl::Duration OrbitApp::GetCaptureTimeAt(uint64_t timestamp_ns) const {
  const TimeGraph* time_graph = GetTimeGraph();
  if (time_graph == nullptr) {
    return absl::ZeroDuration();
  }
  const uint64_t capture_min_timestamp_ns = time_graph->GetCaptureMin();
  if (timestamp_ns < capture_min_timestamp_ns) {
    return absl::ZeroDuration();
  }
  return absl::Nanoseconds(timestamp_ns - capture_min_timestamp_ns);
}

std::string OrbitApp::GetSaveFile(const std::string& extension) const {
  ORBIT_CHECK(save_file_callback_);
  return save_file_callback_(extension);
}

void OrbitApp::SetClipboard(const std::string& text) {
  ORBIT_CHECK(clipboard_callback_);
  clipboard_callback_(text);
}

ErrorMessageOr<void> OrbitApp::OnSavePreset(const std::string& filename) {
  ScopedMetric metric{metrics_uploader_, OrbitLogEvent::ORBIT_PRESET_SAVE};
  auto save_result = SavePreset(filename);
  if (save_result.has_error()) {
    metric.SetStatusCode(OrbitLogEvent::INTERNAL_ERROR);
    return save_result.error();
  }
  ListPresets();
  Refresh(DataViewType::kPresets);
  return outcome::success();
}

ErrorMessageOr<void> OrbitApp::SavePreset(const std::string& filename) {
  PresetInfo preset;

  for (const auto& function : data_manager_->GetSelectedFunctions()) {
    (*preset.mutable_modules())[function.module_path()].add_function_names(function.pretty_name());
  }

  for (const auto& function : data_manager_->user_defined_capture_data().frame_track_functions()) {
    (*preset.mutable_modules())[function.module_path()].add_frame_track_function_names(
        function.pretty_name());
  }

  std::string filename_with_ext = filename;
  if (!absl::EndsWith(filename, ".opr")) {
    filename_with_ext += ".opr";
  }

  PresetFile preset_file{filename_with_ext, preset};
  OUTCOME_TRY(preset_file.SaveToFile());

  return outcome::success();
}

ErrorMessageOr<PresetFile> OrbitApp::ReadPresetFromFile(const std::filesystem::path& filename) {
  std::filesystem::path file_path =
      filename.is_absolute() ? filename : orbit_paths::CreateOrGetPresetDirUnsafe() / filename;

  return orbit_preset_file::ReadPresetFromFile(file_path);
}

ErrorMessageOr<void> OrbitApp::OnLoadPreset(const std::string& filename) {
  OUTCOME_TRY(auto&& preset_file, ReadPresetFromFile(filename));
  (void)LoadPreset(preset_file)
      .ThenIfSuccess(main_thread_executor_,
                     [this, preset_file_path = std::move(preset_file.file_path())]() {
                       ORBIT_CHECK(presets_data_view_ != nullptr);
                       presets_data_view_->OnLoadPresetSuccessful(preset_file_path);
                     });
  return outcome::success();
}

orbit_data_views::PresetLoadState OrbitApp::GetPresetLoadState(const PresetFile& preset) const {
  return GetPresetLoadStateForProcess(preset, GetTargetProcess());
}

Future<ErrorMessageOr<CaptureListener::CaptureOutcome>> OrbitApp::LoadCaptureFromFile(
    const std::filesystem::path& file_path) {
  if (capture_window_ != nullptr) {
    capture_window_->set_draw_help(false);
  }
  ClearCapture();
  auto load_future = thread_pool_->Schedule([this, file_path]() {
    capture_loading_cancellation_requested_ = false;

    auto capture_file_or_error = CaptureFile::OpenForReadWrite(file_path);

    ErrorMessageOr<CaptureListener::CaptureOutcome> load_result{CaptureOutcome::kComplete};

    // Set is_loading_capture_ to true for the duration of this scope.
    data_source_ = CaptureData::DataSource::kLoadedCapture;
    orbit_base::unique_resource scope_exit{&data_source_,
                                           [](std::atomic<CaptureData::DataSource>* value) {
                                             *value = CaptureData::DataSource::kLiveCapture;
                                           }};

    ScopedMetric metric{metrics_uploader_, capture_file_or_error.has_value()
                                               ? OrbitLogEvent::ORBIT_CAPTURE_LOAD_V2
                                               : OrbitLogEvent::ORBIT_CAPTURE_LOAD};
    if (capture_file_or_error.has_value()) {
      load_result = LoadCapture(this, capture_file_or_error.value().get(),
                                &capture_loading_cancellation_requested_);
    } else {
      load_result = capture_file_or_error.error();
    }

    if (load_result.has_error()) {
      metric.SetStatusCode(OrbitLogEvent::INTERNAL_ERROR);
      return load_result;
    }

    switch (load_result.value()) {
      case CaptureOutcome::kCancelled:
        metric.SetStatusCode(OrbitLogEvent::CANCELLED);
        break;
      case CaptureOutcome::kComplete:
        OnCaptureComplete();
        break;
    }

    return load_result;
  });

  DoZoom = true;  // TODO: remove global, review logic

  (void)load_future.ThenIfSuccess(
      main_thread_executor_, [this, file_path](CaptureListener::CaptureOutcome outcome) {
        if (outcome != CaptureOutcome::kComplete) return;
        capture_file_info_manager_.AddOrTouchCaptureFile(file_path, GetCaptureTime());
      });

  return load_future;
}

Future<ErrorMessageOr<void>> OrbitApp::MoveCaptureFile(const std::filesystem::path& src,
                                                       const std::filesystem::path& dest) {
  std::optional<absl::Duration> capture_length =
      capture_file_info_manager_.GetCaptureLengthByPath(src);
  return thread_pool_->Schedule([src, dest]() { return orbit_base::MoveOrRenameFile(src, dest); })
      .ThenIfSuccess(main_thread_executor_, [this, dest, capture_length] {
        capture_file_info_manager_.AddOrTouchCaptureFile(dest, capture_length);
      });
}

void OrbitApp::OnLoadCaptureCancelRequested() { capture_loading_cancellation_requested_ = true; }

void OrbitApp::FireRefreshCallbacks(DataViewType type) {
  for (orbit_data_views::DataView* panel : panels_) {
    if (type == orbit_data_views::DataViewType::kAll || type == panel->GetType()) {
      panel->OnDataChanged();
    }
  }

  ORBIT_CHECK(refresh_callback_);
  refresh_callback_(type);
}

static std::unique_ptr<CaptureEventProcessor> CreateCaptureEventProcessor(
    CaptureListener* listener, const std::string& process_name,
    absl::flat_hash_set<uint64_t> frame_track_function_ids,
    const std::function<void(const ErrorMessage&)>& error_handler) {
  std::filesystem::path file_path = orbit_paths::CreateOrGetCaptureDirUnsafe() /
                                    orbit_client_model::capture_serializer::GenerateCaptureFileName(
                                        process_name, absl::Now(), "_autosave");

  uint64_t suffix_number = 0;
  while (true) {
    auto file_exists_or_error = orbit_base::FileOrDirectoryExists(file_path);
    if (file_exists_or_error.has_error()) {
      ORBIT_ERROR("Unable to check for existence of \"%s\": %s", file_path.string(),
                  file_exists_or_error.error().message());
      break;
    }

    if (!file_exists_or_error.value()) {
      break;
    }

    std::string suffix = absl::StrFormat("_autosave(%d)", ++suffix_number);
    file_path = orbit_paths::CreateOrGetCaptureDirUnsafe() /
                orbit_client_model::capture_serializer::GenerateCaptureFileName(
                    process_name, absl::Now(), suffix);
  }

  auto save_to_file_processor_or_error =
      CaptureEventProcessor::CreateSaveToFileProcessor(file_path, error_handler);

  if (save_to_file_processor_or_error.has_error()) {
    error_handler(ErrorMessage{
        absl::StrFormat("Unable to set up automatic capture saving to \"%s\": %s",
                        file_path.string(), save_to_file_processor_or_error.error().message())});
    return CaptureEventProcessor::CreateForCaptureListener(listener, std::nullopt,
                                                           std::move(frame_track_function_ids));
  }

  std::vector<std::unique_ptr<CaptureEventProcessor>> event_processors;
  event_processors.push_back(CaptureEventProcessor::CreateForCaptureListener(
      listener, std::move(file_path), std::move(frame_track_function_ids)));
  event_processors.push_back(std::move(save_to_file_processor_or_error.value()));
  return CaptureEventProcessor::CreateCompositeProcessor(std::move(event_processors));
}

static void FindAndAddFunctionToStopUnwindingAt(
    const std::string& function_name, const std::string& module_name,
    const orbit_client_data::ModuleManager& module_manager, const ProcessData& process,
    std::map<uint64_t, uint64_t>* absolute_address_to_size_of_functions_to_stop_unwinding_at) {
  std::vector<orbit_client_data::ModuleInMemory> modules =
      process.FindModulesByFilename(module_name);
  for (const auto& module_in_memory : modules) {
    const ModuleData* module_data =
        module_manager.GetModuleByModuleIdentifier(module_in_memory.module_id());

    const FunctionInfo* function_to_stop_unwinding_at =
        module_data->FindFunctionFromPrettyName(function_name);
    if (function_to_stop_unwinding_at == nullptr) {
      continue;
    }
    uint64_t function_absolute_start_address =
        orbit_module_utils::SymbolVirtualAddressToAbsoluteAddress(
            function_to_stop_unwinding_at->address(), module_in_memory.start(),
            module_data->load_bias(), module_data->executable_segment_offset());

    auto [unused_it, inserted] =
        absolute_address_to_size_of_functions_to_stop_unwinding_at->insert_or_assign(
            function_absolute_start_address, function_to_stop_unwinding_at->size());
    ORBIT_CHECK(inserted);
  }
}

void OrbitApp::StartCapture() {
  const ProcessData* process = GetTargetProcess();
  if (process == nullptr) {
    SendErrorToUi("Error starting capture",
                  "No process selected. Please select a target process for the capture.");
    return;
  }

  if (absl::GetFlag(FLAGS_auto_symbol_loading)) {
    RequestSymbolDownloadStop(module_manager_->GetAllModuleData(), false);
  }

  if (capture_window_ != nullptr) {
    capture_window_->set_draw_help(false);
  }

  std::vector<FunctionInfo> selected_functions = data_manager_->GetSelectedFunctions();

  UserDefinedCaptureData user_defined_capture_data = data_manager_->user_defined_capture_data();

  absl::flat_hash_map<uint64_t, FunctionInfo> selected_functions_map;
  absl::flat_hash_set<uint64_t> frame_track_function_ids;

  // non-zero since 0 is reserved for invalid ids.
  uint64_t function_id = 1;
  for (const auto& function : selected_functions) {
    if (user_defined_capture_data.ContainsFrameTrack(function)) {
      frame_track_function_ids.insert(function_id);
    }
    selected_functions_map.insert_or_assign(function_id++, function);
  }

  orbit_client_data::WineSyscallHandlingMethod wine_syscall_handling_method =
      data_manager_->wine_syscall_handling_method();

  // With newer Wine versions, unwinding will fail after `__wine_syscall_dispatcher`
  // (see go/unwinding_wine_syscall_dispatcher). The main reason for failing is that the "syscall"
  // implementation of Wine operates on a different stack than the "Windows user-space" stack. Our
  // unwinder will only have offline memory for the syscall stack. We can mitigate this by
  // collecting the stack data on every call to `__wine_syscall_dispatcher` and keeping the most
  // recent stack copy per thread in memory for unwinding.
  // Note: This requires symbols being loaded. We prioritize loading of the `ntdll.so` and rely on
  // auto-symbol loading.
  absl::flat_hash_map<uint64_t, FunctionInfo> functions_to_record_additional_stack_on;
  if (wine_syscall_handling_method ==
          orbit_client_data::WineSyscallHandlingMethod::kRecordUserStack &&
      data_manager_->unwinding_method() == CaptureOptions::kDwarf) {
    for (const auto& module_data : module_manager_->GetModulesByFilename(kNtdllSoFileName)) {
      const FunctionInfo* function_to_record_stack =
          module_data->FindFunctionFromPrettyName(kWineSyscallDispatcherFunctionName);
      if (function_to_record_stack == nullptr) {
        continue;
      }
      functions_to_record_additional_stack_on.insert_or_assign(function_id++,
                                                               *function_to_record_stack);
    }
  }

  // With newer Wine versions, unwinding will fail after `__wine_syscall_dispatcher`
  // (see go/unwinding_wine_syscall_dispatcher). Unless we mitigate this situation as above, we at
  // least want to report "complete" callstacks for the "Windows kernel" part (until
  // `__wine_syscall_dispatcher`). To do so, we look for the absolute address of this function and
  // send it to the service as a function to stop unwinding at. The unwinder will stop on those
  // functions and report the callstacks as "complete".
  // Note: This requires symbols being loaded. We prioritize loading of the `ntdll.so` and rely on
  // auto-symbol loading.
  std::map<uint64_t, uint64_t> absolute_address_to_size_of_functions_to_stop_unwinding_at;
  if (wine_syscall_handling_method ==
          orbit_client_data::WineSyscallHandlingMethod::kStopUnwinding &&
      data_manager_->unwinding_method() == CaptureOptions::kDwarf) {
    FindAndAddFunctionToStopUnwindingAt(
        kWineSyscallDispatcherFunctionName, kNtdllSoFileName, *module_manager_, *process_,
        &absolute_address_to_size_of_functions_to_stop_unwinding_at);
  }

  ClientCaptureOptions options;
  options.selected_tracepoints = data_manager_->selected_tracepoints();
  options.collect_scheduling_info = !IsDevMode() || data_manager_->collect_scheduler_info();
  options.collect_thread_states = data_manager_->collect_thread_states();
  options.collect_gpu_jobs = !IsDevMode() || data_manager_->trace_gpu_submissions();
  options.enable_api = data_manager_->enable_api();
  options.enable_introspection = IsDevMode() && data_manager_->enable_introspection();
  options.dynamic_instrumentation_method = data_manager_->dynamic_instrumentation_method();
  options.samples_per_second = data_manager_->samples_per_second();
  options.stack_dump_size = data_manager_->stack_dump_size();
  options.thread_state_change_callstack_stack_dump_size =
      data_manager_->thread_state_change_callstack_stack_dump_size();
  options.unwinding_method = data_manager_->unwinding_method();
  options.max_local_marker_depth_per_command_buffer =
      data_manager_->max_local_marker_depth_per_command_buffer();

  options.collect_memory_info = data_manager_->collect_memory_info();
  options.memory_sampling_period_ms = data_manager_->memory_sampling_period_ms();
  options.selected_functions = std::move(selected_functions_map);
  options.functions_to_record_additional_stack_on =
      std::move(functions_to_record_additional_stack_on);
  options.absolute_address_to_size_of_functions_to_stop_unwinding_at =
      std::move(absolute_address_to_size_of_functions_to_stop_unwinding_at);
  options.process_id = process->pid();
  options.record_return_values = absl::GetFlag(FLAGS_show_return_values);
  options.record_arguments = false;
  options.enable_auto_frame_track = data_manager_->enable_auto_frame_track();
  options.thread_state_change_callstack_collection =
      data_manager_->thread_state_change_callstack_collection();

  // In metrics, -1 indicates memory collection was turned off. See also the comment in
  // orbit_log_event.proto
  constexpr int64_t kMemoryCollectionDisabledMetricsValue = -1;
  int64_t memory_information_sampling_period_ms_for_metrics = kMemoryCollectionDisabledMetricsValue;
  if (options.collect_memory_info) {
    memory_information_sampling_period_ms_for_metrics =
        static_cast<int64_t>(options.memory_sampling_period_ms);
  }

  // Whether the Orbit custom vulkan layer is used by the process (game), is determined via the
  // module list of the process. If "libOrbitVulkanLayer.so" is in the module list, it means the
  // process loaded it and it is in use.
  bool orbit_vulkan_layer_loaded_by_process = false;
  std::vector<const ModuleData*> vulkan_layer_modules =
      module_manager_->GetModulesByFilename(kLibOrbitVulkanLayerSoFileName);
  for (const auto& module : vulkan_layer_modules) {
    if (process->IsModuleLoadedByProcess(module)) {
      orbit_vulkan_layer_loaded_by_process = true;
    }
  }

  CaptureMetric capture_metric{
      metrics_uploader_,
      CreateCaptureStartData(
          selected_functions, user_defined_capture_data.frame_track_functions().size(),
          data_manager_->collect_thread_states(), memory_information_sampling_period_ms_for_metrics,
          orbit_vulkan_layer_loaded_by_process, options.max_local_marker_depth_per_command_buffer,
          options.dynamic_instrumentation_method, static_cast<uint64_t>(options.samples_per_second),
          options.unwinding_method, data_manager_->enable_auto_frame_track())};

  metrics_capture_complete_data_ = orbit_metrics_uploader::CaptureCompleteData{};

  ORBIT_CHECK(capture_client_ != nullptr);

  std::unique_ptr<CaptureEventProcessor> capture_event_processor = CreateCaptureEventProcessor(
      this, process->name(), frame_track_function_ids, [this](const ErrorMessage& error) {
        GetMutableCaptureData().reset_file_path();
        SendErrorToUi("Error saving capture", error.message());
        ORBIT_ERROR("%s", error.message());
      });

  Future<ErrorMessageOr<CaptureOutcome>> capture_result = capture_client_->Capture(
      thread_pool_.get(), std::move(capture_event_processor), *module_manager_, *process_, options);

  // TODO(b/187250643): Refactor this to be more readable and maybe remove parts that are not needed
  // here (capture cancelled)
  capture_result.Then(
      main_thread_executor_, [this, capture_metric = std::move(capture_metric)](
                                 ErrorMessageOr<CaptureOutcome> capture_result) mutable {
        if (capture_result.has_error()) {
          OnCaptureFailed(capture_result.error());
          capture_metric.SetCaptureCompleteData(metrics_capture_complete_data_);
          capture_metric.SendCaptureFailed();
          return;
        }
        switch (capture_result.value()) {
          case CaptureListener::CaptureOutcome::kCancelled:
            OnCaptureCancelled();
            capture_metric.SetCaptureCompleteData(metrics_capture_complete_data_);
            capture_metric.SendCaptureCancelled();
            return;
          case CaptureListener::CaptureOutcome::kComplete:
            OnCaptureComplete().Then(
                main_thread_executor_,
                [this, capture_metric = std::move(capture_metric)]() mutable {
                  auto capture_time = absl::Nanoseconds(GetTimeGraph()->GetCaptureTimeSpanNs());
                  auto capture_time_ms = ToChronoMilliseconds(capture_time);
                  capture_metric.SetCaptureCompleteData(metrics_capture_complete_data_);
                  capture_metric.SendCaptureSucceeded(capture_time_ms);
                });

            return;
        }
      });
}

void OrbitApp::StopCapture() {
  if (!capture_client_->StopCapture()) {
    return;
  }

  auto capture_time = absl::Nanoseconds(GetTimeGraph()->GetCaptureTimeSpanUs());
  auto capture_time_ms = ToChronoMilliseconds(capture_time);
  metrics_uploader_->SendLogEvent(OrbitLogEvent::ORBIT_CAPTURE_DURATION, capture_time_ms);

  ORBIT_CHECK(capture_stop_requested_callback_);
  capture_stop_requested_callback_();
}

void OrbitApp::AbortCapture() {
  if (capture_client_ == nullptr) return;

  static constexpr int64_t kMaxWaitForAbortCaptureMs = 2000;
  if (!capture_client_->AbortCaptureAndWait(kMaxWaitForAbortCaptureMs)) {
    return;
  }

  ORBIT_CHECK(capture_stop_requested_callback_);
  capture_stop_requested_callback_();
}

void OrbitApp::ClearCapture() {
  ORBIT_SCOPE_FUNCTION;
  if (capture_window_ != nullptr) {
    capture_window_->ClearTimeGraph();
  }
  ResetCaptureData();

  string_manager_.Clear();

  set_selected_thread_id(orbit_base::kAllProcessThreadsTid);
  SelectTimer(nullptr);

  UpdateAfterCaptureCleared();

  ORBIT_CHECK(capture_cleared_callback_);
  capture_cleared_callback_();

  FireRefreshCallbacks();
}

void OrbitApp::ToggleCapture() {
  if (IsCapturing()) {
    StopCapture();
  } else {
    StartCapture();
  }
}

bool OrbitApp::IsCaptureConnected(const CaptureData& capture) const {
  // This function is used to determine if a capture is in a connected state. Lets imagine a user
  // selects a process and takes a capture. Then the process of the capture is the same as the
  // selected one and that means they are connected. If the user than selects a different process,
  // the capture is not connected anymore. Orbit can be in a similar "capture connected" state, when
  // the user connects to an instance, selects a process and then loads an instance from file that
  // was taken shortly before of the same process.
  // TODO(b/163303287): It might be the case in the future that captures loaded from file are always
  // opened in a new window (compare b/163303287). Then this function is probably not necessary
  // anymore. Otherwise, this function should probably be more sophisticated and also compare the
  // build-id of the selected process (main module) and the process of the capture.

  const ProcessData* selected_process = GetTargetProcess();
  if (selected_process == nullptr) return false;

  const ProcessData* capture_process = capture.process();
  ORBIT_CHECK(capture_process != nullptr);

  return selected_process->pid() == capture_process->pid() &&
         selected_process->full_path() == capture_process->full_path();
}

bool OrbitApp::IsDevMode() const { return absl::GetFlag(FLAGS_devmode); }

void OrbitApp::SendDisassemblyToUi(const orbit_client_data::FunctionInfo& function_info,
                                   std::string disassembly,
                                   orbit_code_report::DisassemblyReport report) {
  main_thread_executor_->Schedule([this, function_info, disassembly = std::move(disassembly),
                                   report = std::move(report)]() mutable {
    main_window_->ShowDisassembly(function_info, disassembly, std::move(report));
  });
}

void OrbitApp::SendTooltipToUi(const std::string& tooltip) {
  main_thread_executor_->Schedule([this, tooltip] { main_window_->ShowTooltip(tooltip); });
}

void OrbitApp::SendInfoToUi(const std::string& title, const std::string& text) {
  main_thread_executor_->Schedule([this, title, text] {
    ORBIT_CHECK(info_message_callback_);
    info_message_callback_(title, text);
  });
}

void OrbitApp::SendWarningToUi(const std::string& title, const std::string& text) {
  main_thread_executor_->Schedule([this, title, text] {
    ORBIT_CHECK(warning_message_callback_);
    warning_message_callback_(title, text);
  });
}

void OrbitApp::SendErrorToUi(const std::string& title, const std::string& text) {
  main_thread_executor_->Schedule([this, title, text] {
    ORBIT_CHECK(error_message_callback_);
    error_message_callback_(title, text);
  });
}

void OrbitApp::SendErrorToUi(const std::string& title, const std::string& text,
                             ScopedMetric metric) {
  SendErrorToUi(title, text);
  metric.SetStatusCode(OrbitLogEvent::INTERNAL_ERROR);
}

Future<ErrorMessageOr<CanceledOr<std::filesystem::path>>> OrbitApp::RetrieveModuleFromRemote(
    const std::string& module_file_path, StopToken stop_token) {
  ORBIT_SCOPE_FUNCTION;

  Future<ErrorMessageOr<NotFoundOr<std::filesystem::path>>> check_file_on_remote =
      thread_pool_->Schedule(
          [process_manager = GetProcessManager(),
           module_file_path]() -> ErrorMessageOr<NotFoundOr<std::filesystem::path>> {
            std::vector<std::string> additional_instance_folder;
            if (!absl::GetFlag(FLAGS_instance_symbols_folder).empty()) {
              additional_instance_folder.emplace_back(absl::GetFlag(FLAGS_instance_symbols_folder));
            }
            return process_manager->FindDebugInfoFile(module_file_path, additional_instance_folder);
          });

  auto download_file = [this, module_file_path, stop_token = std::move(stop_token)](
                           const NotFoundOr<std::filesystem::path>& remote_search_outcome) mutable
      -> Future<ErrorMessageOr<CanceledOr<std::filesystem::path>>> {
    // TODO(b/231455031) As a first step, we treat the not found message as an error
    if (orbit_base::IsNotFound(remote_search_outcome)) {
      return {ErrorMessage{orbit_base::GetNotFoundMessage(remote_search_outcome)}};
    }
    const std::filesystem::path& remote_debug_file_path =
        orbit_base::GetFound(remote_search_outcome);
    ORBIT_LOG("Found symbols file on the remote: \"%s\" - loading it using scp...",
              remote_debug_file_path.string());

    const std::filesystem::path local_debug_file_path =
        symbol_helper_.GenerateCachedFilePath(module_file_path);

    const std::chrono::time_point<std::chrono::steady_clock> copy_begin =
        std::chrono::steady_clock::now();
    ORBIT_LOG("Copying \"%s\" started", remote_debug_file_path.string());
    Future<ErrorMessageOr<CanceledOr<void>>> copy_result = main_window_->DownloadFileFromInstance(
        remote_debug_file_path, local_debug_file_path, std::move(stop_token));

    orbit_base::ImmediateExecutor immediate_executor{};
    return copy_result.Then(&immediate_executor,
                            [remote_debug_file_path, local_debug_file_path,
                             copy_begin](ErrorMessageOr<CanceledOr<void>> sftp_result)
                                -> ErrorMessageOr<CanceledOr<std::filesystem::path>> {
                              if (sftp_result.has_error()) {
                                return ErrorMessage{absl::StrFormat(
                                    "Could not copy debug info file from the remote: %s",
                                    sftp_result.error().message())};
                              }
                              if (orbit_base::IsCanceled(sftp_result.value())) {
                                return orbit_base::Canceled{};
                              }
                              const auto duration =
                                  std::chrono::duration_cast<std::chrono::milliseconds>(
                                      std::chrono::steady_clock::now() - copy_begin);
                              ORBIT_LOG("Copying \"%s\" took %.3f ms",
                                        remote_debug_file_path.string(), duration.count());
                              return local_debug_file_path;
                            });
  };

  Future<ErrorMessageOr<CanceledOr<std::filesystem::path>>> chained_result_future = UnwrapFuture(
      check_file_on_remote.ThenIfSuccess(main_thread_executor_, std::move(download_file)));

  return chained_result_future;
}

Future<void> OrbitApp::LoadSymbolsManually(absl::Span<const ModuleData* const> modules) {
  // Use a set, to filter out duplicates
  absl::flat_hash_set<const ModuleData*> modules_set(modules.begin(), modules.end());

  std::vector<Future<void>> futures;
  futures.reserve(modules_set.size());

  orbit_base::ImmediateExecutor immediate_executor;
  for (const auto& module : modules_set) {
    download_disabled_modules_.erase(module->file_path());

    // Explicitly do not handle the result.
    Future<void> future = RetrieveModuleAndLoadSymbolsAndHandleError(module).Then(
        &immediate_executor, [](const SymbolLoadingAndErrorHandlingResult& /*result*/) -> void {});
    futures.emplace_back(std::move(future));
  }
  orbit_client_symbols::QSettingsBasedStorageManager storage_manager;
  storage_manager.SaveDisabledModulePaths(download_disabled_modules_);

  return orbit_base::WhenAll(futures);
}

Future<OrbitApp::SymbolLoadingAndErrorHandlingResult>
OrbitApp::RetrieveModuleAndLoadSymbolsAndHandleError(const ModuleData* module) {
  Future<ErrorMessageOr<CanceledOr<void>>> load_future = RetrieveModuleAndLoadSymbols(module);

  Future<Future<SymbolLoadingAndErrorHandlingResult>> chained_load_future = load_future.Then(
      main_thread_executor_,
      [module, this](ErrorMessageOr<CanceledOr<void>> load_result)
          -> Future<SymbolLoadingAndErrorHandlingResult> {
        if (!load_result.has_error()) {
          if (orbit_base::IsCanceled(load_result.value())) {
            return {SymbolLoadingAndErrorHandlingResult::kCanceled};
          }
          return {SymbolLoadingAndErrorHandlingResult::kSymbolsLoadedSuccessfully};
        }

        MainWindowInterface::SymbolErrorHandlingResult error_handling_result =
            main_window_->HandleSymbolError(load_result.error(), module);

        switch (error_handling_result) {
          case MainWindowInterface::SymbolErrorHandlingResult::kSymbolLoadingCancelled: {
            metrics_uploader_->SendLogEvent(OrbitLogEvent::ORBIT_SYMBOL_LOADING_ERROR_CANCELLED);
            return {SymbolLoadingAndErrorHandlingResult::kCanceled};
          }
          case MainWindowInterface::SymbolErrorHandlingResult::kReloadRequired: {
            return RetrieveModuleAndLoadSymbolsAndHandleError(module).Then(
                main_thread_executor_,
                [this](SymbolLoadingAndErrorHandlingResult result)
                    -> SymbolLoadingAndErrorHandlingResult {
                  if (result == SymbolLoadingAndErrorHandlingResult::kSymbolsLoadedSuccessfully) {
                    metrics_uploader_->SendLogEvent(
                        OrbitLogEvent::ORBIT_SYMBOL_LOADING_ERROR_RESOLVED);
                  }
                  return result;
                });
          }
        }
        ORBIT_UNREACHABLE();
      });

  return orbit_base::UnwrapFuture(chained_load_future);
}

Future<ErrorMessageOr<CanceledOr<void>>> OrbitApp::RetrieveModuleAndLoadSymbols(
    const ModuleData* module) {
  return RetrieveModuleAndLoadSymbols(module->module_id());
}

Future<ErrorMessageOr<CanceledOr<void>>> OrbitApp::RetrieveModuleAndLoadSymbols(
    const ModuleIdentifier& module_id) {
  // TODO: make async
  auto result = GetPlatformApiInfo(module).Get();
  if (result.has_error()) {
    return result.error();
  }

  const orbit_grpc_protos::GetPlatformApiInfoResponse& response = result.value();
  ORBIT_LOG("Platform description: %s", response.platform_description());
  orbit_grpc_protos::ModuleSymbols module_symbols;
  uint32_t counter = 0;
  for (const auto& api_function : response.functions()) {
    if (api_function.key().empty()) continue;
    if (!api_function.is_hookable()) {
      ORBIT_LOG("Api function %s is not hookable, skipping.", api_function.key());
      continue;
    }
    orbit_grpc_protos::SymbolInfo* symbol_info = module_symbols.add_symbol_infos();
    symbol_info->set_demangled_name(api_function.key());
    symbol_info->set_address(++counter);
    ORBIT_LOG("api_function.key(): %s", api_function.key());
  }

  ModuleData* module_data =
      GetMutableModuleByPathAndBuildId(orbit_grpc_protos::kWindowsApiFakeModuleName, "");
  ORBIT_CHECK(module_data == module);
  module_data->AddSymbols(module_symbols);

  const ProcessData* selected_process = GetTargetProcess();

  functions_data_view_->AddFunctions(module_data->GetFunctions());
  ORBIT_LOG("Added loaded function symbols for module \"%s\" to the functions tab",
      module_data->file_path());

  UpdateAfterSymbolLoading();
  FireRefreshCallbacks();

  return outcome::success();
}

orbit_base::Future<ErrorMessageOr<void>> OrbitApp::RetrieveModuleAndLoadSymbols(
    const ModuleData* module) {
  if (module->name() == orbit_grpc_protos::kWindowsApiFakeModuleName) {
    return RetrieveAndLoadPlatformApiInfo(module);
  }
  return RetrieveModuleAndLoadSymbols(module->file_path(), module->build_id());
}

orbit_base::Future<ErrorMessageOr<orbit_grpc_protos::GetPlatformApiInfoResponse>>
OrbitApp::GetPlatformApiInfo(const orbit_client_data::ModuleData* module) {
  ScopedStatus scoped_status =
      CreateScopedStatus("Retrieving platform api info on remote instance...");

  orbit_base::Future<ErrorMessageOr<orbit_grpc_protos::GetPlatformApiInfoResponse>>
      get_platform_api_info_on_remote =
          thread_pool_->Schedule([process_manager = GetProcessManager()]() {
            return process_manager->GetPlatformApiInfo();
          });

  return std::move(get_platform_api_info_on_remote);
}

orbit_base::Future<ErrorMessageOr<void>> OrbitApp::RetrieveModuleAndLoadSymbols(
    const std::string& module_path, const std::string& build_id) {
  ORBIT_SCOPE_FUNCTION;
  ORBIT_CHECK(main_thread_id_ == std::this_thread::get_id());

  // TODO(b/231455031): Make sure this scoped metric gets the state CANCELLED when the user stops
  // the download.
  ScopedMetric metric(metrics_uploader_, OrbitLogEvent::ORBIT_SYMBOL_LOAD);

  const ModuleData* const module_data = GetModuleByModuleIdentifier(module_id);
  if (module_data == nullptr) {
    metric.SetStatusCode(OrbitLogEvent::INTERNAL_ERROR);
    return {ErrorMessage{absl::StrFormat("Module \"%s\" was not found.", module_id.file_path)}};
  }

  modules_with_symbol_loading_error_.erase(module_id);

  if (module_data->AreDebugSymbolsLoaded()) return {outcome::success()};

  const auto it = symbols_currently_loading_.find(module_id);
  if (it != symbols_currently_loading_.end()) {
    return it->second;
  }

  Future<ErrorMessageOr<CanceledOr<void>>> load_result =
      orbit_base::UnwrapFuture(RetrieveModule(module_id).ThenIfSuccess(
          main_thread_executor_,
          [this, module_id](const CanceledOr<std::filesystem::path>& load_result_or_canceled)
              -> Future<ErrorMessageOr<CanceledOr<void>>> {
            if (orbit_base::IsCanceled(load_result_or_canceled)) {
              return {orbit_base::Canceled{}};
            }
            const std::filesystem::path& local_file_path{
                orbit_base::GetNotCanceled(load_result_or_canceled)};
            orbit_base::ImmediateExecutor executor;
            return LoadSymbols(local_file_path, module_id)
                .ThenIfSuccess(&executor, []() -> CanceledOr<void> { return CanceledOr<void>{}; });
          }));

  load_result.Then(main_thread_executor_,
                   [this, metric = std::move(metric),
                    module_id](const ErrorMessageOr<CanceledOr<void>>& result) mutable {
                     if (result.has_error()) {
                       modules_with_symbol_loading_error_.emplace(module_id);
                       metric.SetStatusCode(OrbitLogEvent::INTERNAL_ERROR);
                     }
                     symbols_currently_loading_.erase(module_id);
                     FireRefreshCallbacks(orbit_data_views::DataViewType::kModules);
                   });

  symbols_currently_loading_.emplace(module_id, load_result);
  FireRefreshCallbacks(orbit_data_views::DataViewType::kModules);

  return load_result;
}

[[nodiscard]] static Future<ErrorMessageOr<CanceledOr<std::filesystem::path>>>
ConvertSymbolProviderRetrieveFuture(const Future<SymbolLoadingOutcome>& future,
                                    orbit_base::Executor* executor,
                                    std::string symbol_provider_name, std::string error_msg) {
  return future.Then(
      executor,
      [symbol_provider_name = std::move(symbol_provider_name),
       error_msg = std::move(error_msg)](const SymbolLoadingOutcome& retrieve_result) mutable
      -> ErrorMessageOr<CanceledOr<std::filesystem::path>> {
        if (orbit_symbol_provider::IsSuccessResult(retrieve_result)) {
          return orbit_symbol_provider::GetSuccessResult(retrieve_result).path;
        }

        if (orbit_symbol_provider::IsCanceled(retrieve_result)) return orbit_base::Canceled{};

        error_msg.append(
            absl::StrFormat("\n- Did not find symbols from %s: %s", symbol_provider_name,
                            orbit_symbol_provider::IsNotFound(retrieve_result)
                                ? orbit_symbol_provider::GetNotFoundMessage(retrieve_result)
                                : retrieve_result.error().message()));
        return ErrorMessage{error_msg};
      });
};

Future<ErrorMessageOr<CanceledOr<std::filesystem::path>>> OrbitApp::RetrieveModuleViaDownload(
    const ModuleIdentifier& module_id) {
  ORBIT_CHECK(std::this_thread::get_id() == main_thread_id_);

  if (const auto it = symbol_files_currently_downloading_.find(module_id.file_path);
      it != symbol_files_currently_downloading_.end()) {
    return it->second.future;
  }

  orbit_base::StopSource stop_source;

  using SymbolRetrieveResult = ErrorMessageOr<CanceledOr<std::filesystem::path>>;
  Future<SymbolRetrieveResult> retrieve_from_instance_future =
      orbit_base::UnwrapFuture(main_thread_executor_->Schedule(
          [this, module_id,
           stop_token = stop_source.GetStopToken()]() mutable -> Future<SymbolRetrieveResult> {
            // If --local, Orbit cannot download files from the instance, because no ssh channel
            // exists. We still return an ErrorMessage to enable continuing searching for symbols
            // from other symbol sources.
            if (absl::GetFlag(FLAGS_local) || !main_window_->IsConnected() ||
                absl::GetFlag(FLAGS_disable_instance_symbols)) {
              return {ErrorMessage{"\n- Not able to search for symbols from instance"}};
            }

            return RetrieveModuleFromRemote(module_id.file_path, std::move(stop_token))
                .Then(main_thread_executor_,
                      [module_id](const SymbolRetrieveResult& retrieve_result) mutable
                      -> SymbolRetrieveResult {
                        if (retrieve_result.has_value()) return retrieve_result;

                        return ErrorMessage{
                            absl::StrFormat("\n- Did not find symbols from instance: %s",
                                            retrieve_result.error().message())};
                      });
          }));

  Future<SymbolRetrieveResult> retrieve_from_stadia_future =
      orbit_base::UnwrapFuture(retrieve_from_instance_future.Then(
          main_thread_executor_,
          [this, module_id, stop_token = stop_source.GetStopToken()](
              const SymbolRetrieveResult& previous_result) mutable -> Future<SymbolRetrieveResult> {
            if (orbit_client_symbols::QSettingsBasedStorageManager storage_manager;
                stadia_symbol_provider_ == std::nullopt ||
                !storage_manager.LoadEnableStadiaSymbolStore()) {
              return {previous_result};
            }

            if (previous_result.has_value()) return {previous_result.value()};

            return ConvertSymbolProviderRetrieveFuture(
                stadia_symbol_provider_->RetrieveSymbols(module_id, std::move(stop_token)),
                main_thread_executor_, "Stadia symbol store", previous_result.error().message());
          }));

  Future<SymbolRetrieveResult> retrieve_from_microsoft_future =
      orbit_base::UnwrapFuture(retrieve_from_stadia_future.Then(
          main_thread_executor_,
          [this, module_id, stop_token = stop_source.GetStopToken()](
              const SymbolRetrieveResult& previous_result) mutable -> Future<SymbolRetrieveResult> {
            if (orbit_client_symbols::QSettingsBasedStorageManager storage_manager;
                microsoft_symbol_provider_ == std::nullopt ||
                !storage_manager.LoadEnableMicrosoftSymbolServer()) {
              return {previous_result};
            }

            if (previous_result.has_value()) return {previous_result.value()};

            return ConvertSymbolProviderRetrieveFuture(
                microsoft_symbol_provider_->RetrieveSymbols(module_id, std::move(stop_token)),
                main_thread_executor_, "Microsoft symbol server",
                previous_result.error().message());
          }));

  symbol_files_currently_downloading_.emplace(
      module_id.file_path,
      OrbitApp::ModuleDownloadOperation{std::move(stop_source), retrieve_from_microsoft_future});
  FireRefreshCallbacks(orbit_data_views::DataViewType::kModules);
  retrieve_from_microsoft_future.Then(
      main_thread_executor_,
      [this, module_file_path = module_id.file_path](const SymbolRetrieveResult& /*result*/) {
        symbol_files_currently_downloading_.erase(module_file_path);
        FireRefreshCallbacks(orbit_data_views::DataViewType::kModules);
      });

  return retrieve_from_microsoft_future;
}

Future<ErrorMessageOr<CanceledOr<std::filesystem::path>>> OrbitApp::RetrieveModule(
    const ModuleIdentifier& module_id) {
  ORBIT_SCOPE_FUNCTION;
  const ModuleData* module_data = GetModuleByModuleIdentifier(module_id);

  if (module_data == nullptr) {
    return {ErrorMessage{absl::StrFormat("Module \"%s\" was not found.", module_id.file_path)}};
  }

  const auto it = symbol_files_currently_being_retrieved_.find(module_id);
  if (it != symbol_files_currently_being_retrieved_.end()) {
    return it->second;
  }

  Future<ErrorMessageOr<CanceledOr<std::filesystem::path>>> retrieve_from_local_future =
      FindModuleLocally(module_data)
          .Then(main_thread_executor_,
                [module_id](const ErrorMessageOr<std::filesystem::path>& retrieve_result)
                    -> ErrorMessageOr<CanceledOr<std::filesystem::path>> {
                  if (retrieve_result.has_value()) return {retrieve_result.value()};

                  return {ErrorMessage{absl::StrFormat(
                      "Failed to find symbols for module \"%s\" with build_id=\"%s\"\n"
                      "- Did not find symbols locally: %s",
                      module_id.file_path, module_id.build_id, retrieve_result.error().message())}};
                });

  if (download_disabled_modules_.contains(module_id.file_path)) return retrieve_from_local_future;

  Future<ErrorMessageOr<CanceledOr<std::filesystem::path>>> retrieve_via_download_future =
      orbit_base::UnwrapFuture(retrieve_from_local_future.Then(
          main_thread_executor_,
          [this, module_id](
              const ErrorMessageOr<CanceledOr<std::filesystem::path>>& previous_result) mutable
          -> Future<ErrorMessageOr<CanceledOr<std::filesystem::path>>> {
            if (previous_result.has_value()) return {previous_result.value()};

            return RetrieveModuleViaDownload(module_id).Then(
                main_thread_executor_,
                [module_id, error_msg = previous_result.error().message()](
                    const ErrorMessageOr<CanceledOr<std::filesystem::path>>&
                        retrieve_result) mutable
                -> ErrorMessageOr<CanceledOr<std::filesystem::path>> {
                  if (retrieve_result.has_value()) return retrieve_result;

                  error_msg.append(retrieve_result.error().message());
                  return ErrorMessage{error_msg};
                });
          }));

  symbol_files_currently_being_retrieved_.emplace(module_id, retrieve_via_download_future);
  retrieve_via_download_future.Then(
      main_thread_executor_,
      [this, module_id](const ErrorMessageOr<CanceledOr<std::filesystem::path>>& /*result*/) {
        symbol_files_currently_being_retrieved_.erase(module_id);
      });

  return retrieve_via_download_future;
}

Future<ErrorMessageOr<std::filesystem::path>> OrbitApp::RetrieveModuleWithDebugInfo(
    const ModuleData* module_data) {
  return RetrieveModuleWithDebugInfo(module_data->module_id());
}

Future<ErrorMessageOr<std::filesystem::path>> OrbitApp::RetrieveModuleWithDebugInfo(
    const ModuleIdentifier& module_id) {
  auto loaded_module = RetrieveModule(module_id);
  return loaded_module.ThenIfSuccess(
      main_thread_executor_,
      [this, module_path = module_id.file_path](
          const CanceledOr<std::filesystem::path>& local_file_path_or_canceled)
          -> ErrorMessageOr<std::filesystem::path> {
        if (orbit_base::IsCanceled(local_file_path_or_canceled)) {
          return ErrorMessage{"User canceled loading."};
        }
        const std::filesystem::path& local_file_path =
            orbit_base::GetNotCanceled(local_file_path_or_canceled);

        auto elf_file = orbit_object_utils::CreateElfFile(local_file_path);

        if (elf_file.has_error()) return elf_file.error();

        if (elf_file.value()->HasDebugInfo()) return local_file_path;

        if (!elf_file.value()->HasGnuDebuglink()) {
          return ErrorMessage{
              absl::StrFormat("Module \"%s\" neither includes debug info, nor does it contain "
                              "a .gnu_debuglink section which could refer to a separate debug "
                              "info file.",
                              module_path)};
        }

        const auto debuglink = elf_file.value()->GetGnuDebugLinkInfo().value();
        ErrorMessageOr<std::filesystem::path> local_debuginfo_path =
            symbol_helper_.FindDebugInfoFileLocally(debuglink.path.filename().string(),
                                                    debuglink.crc32_checksum, GetAllSymbolPaths());
        if (local_debuginfo_path.has_error()) {
          return ErrorMessage{absl::StrFormat(
              "Module \"%s\" doesn't include debug info, and a separate debuginfo file wasn't "
              "found on this machine, when searching the folders from the Symbol Locations. Please "
              "make sure that the debuginfo file can be found in one of the added folders. "
              "According to the .gnu_debuglink section, the debuginfo file must be called \"%s\".",
              module_path, debuglink.path.string())};
        }

        elf_file = orbit_object_utils::CreateElfFile(local_debuginfo_path.value());
        if (elf_file.has_error()) return elf_file.error();
        return local_debuginfo_path;
      });
}

static ErrorMessageOr<std::filesystem::path> FindModuleLocallyImpl(
    const orbit_symbols::SymbolHelper& symbol_helper, const ModuleData& module_data) {
  ORBIT_SCOPE_FUNCTION;
  if (absl::GetFlag(FLAGS_enable_unsafe_symbols)) {
    // First checkout if a symbol file override exists and if it does, use it
    OUTCOME_TRY(std::optional<std::filesystem::path> overriden_symbols_file,
                GetOverrideSymbolFileForModule(module_data));
    if (overriden_symbols_file.has_value()) return overriden_symbols_file.value();
  }

  if (module_data.build_id().empty()) {
    return ErrorMessage(
        absl::StrFormat("Unable to find local symbols for module \"%s\", build id is empty",
                        module_data.file_path()));
  }

  #ifdef _WIN32
  ErrorMessageOr<std::filesystem::path> symbols_path = orbit_windows_utils::FindDebugSymbols(

            module_data.file_path(), /*additional_search_directories=*/{});

  if (symbols_path.has_value()) {
    ORBIT_LOG("Found symbols for module \"%s\" locally. Symbols filename: \"%s\"",
              module_data.file_path(), symbols_path.value().string());
    return symbols_path.value();
  }
#endif

  std::string error_message;
  {
    std::vector<std::filesystem::path> search_paths = GetAllSymbolPaths();
    std::filesystem::path module_path(module_data.file_path());
    search_paths.emplace_back(module_path.parent_path());

    const auto symbols_path =
        symbol_helper.FindSymbolsFileLocally(module_data.file_path(), module_data.build_id(),
                                             module_data.object_file_type(), search_paths);
    if (symbols_path.has_value()) {
      ORBIT_LOG(
          "Found symbols for module \"%s\" in user provided symbol folder. Symbols filename: "
          "\"%s\"",
          module_data.file_path(), symbols_path.value().string());
      return symbols_path.value();
    }
    error_message += "\n* " + symbols_path.error().message();
  }
  {
    const auto symbols_path =
        symbol_helper.FindSymbolsInCache(module_data.file_path(), module_data.build_id());
    if (symbols_path.has_value()) {
      ORBIT_LOG("Found symbols for module \"%s\" in cache. Symbols filename: \"%s\"",
                module_data.file_path(), symbols_path.value().string());
      return symbols_path.value();
    }
    error_message += "\n* " + symbols_path.error().message();
  }
  if (absl::GetFlag(FLAGS_local)) {
    const auto symbols_included_in_module =
        orbit_symbols::VerifySymbolFile(module_data.file_path(), module_data.build_id());
    if (symbols_included_in_module.has_value()) {
      ORBIT_LOG("Found symbols included in module: \"%s\"", module_data.file_path());
      return module_data.file_path();
    }
    error_message += "\n* Symbols are not included in module file: " +
                     symbols_included_in_module.error().message();
  }

  error_message = absl::StrFormat("Did not find local symbols for module \"%s\": %s",
                                  module_data.file_path(), error_message);
  ORBIT_LOG("%s", error_message);
  return ErrorMessage(error_message);
}

Future<ErrorMessageOr<std::filesystem::path>> OrbitApp::FindModuleLocally(
    const ModuleData* module_data) {
  ORBIT_SCOPE_FUNCTION;
  return thread_pool_->Schedule([this, module_data]() -> ErrorMessageOr<std::filesystem::path> {
    return FindModuleLocallyImpl(symbol_helper_, *module_data);
  });
}

void OrbitApp::AddSymbols(const ModuleIdentifier& module_id,
                          const orbit_grpc_protos::ModuleSymbols& module_symbols) {
  ORBIT_SCOPE_FUNCTION;
  ModuleData* module_data = GetMutableModuleByModuleIdentifier(module_id);
  module_data->AddSymbols(module_symbols);

  const ProcessData* selected_process = GetTargetProcess();
  if (selected_process != nullptr &&
      selected_process->IsModuleLoadedByProcess(module_data->file_path())) {
    functions_data_view_->AddFunctions(module_data->GetFunctions());
    ORBIT_LOG("Added loaded function symbols for module \"%s\" to the functions tab",
              module_data->file_path());
  }

  FireRefreshCallbacks(DataViewType::kModules);
  UpdateAfterSymbolLoadingThrottled();
}

Future<ErrorMessageOr<void>> OrbitApp::LoadSymbols(const std::filesystem::path& symbols_path,
                                                   const ModuleIdentifier& module_id) {
  ORBIT_SCOPE_FUNCTION;

  auto load_symbols_from_file = thread_pool_->Schedule([this, symbols_path, module_id]() {
    const ModuleData* module_data = GetModuleByModuleIdentifier(module_id);
    orbit_object_utils::ObjectFileInfo object_file_info{module_data->load_bias()};
    return orbit_symbols::SymbolHelper::LoadSymbolsFromFile(symbols_path, object_file_info);
  });

  auto add_symbols = [this, module_id](const ErrorMessageOr<orbit_grpc_protos::ModuleSymbols>&
                                           symbols_result) mutable -> ErrorMessageOr<void> {
    if (symbols_result.has_error()) return symbols_result.error();

    AddSymbols(module_id, symbols_result.value());

    ORBIT_LOG("Successfully loaded %d symbols for \"%s\"",
              symbols_result.value().symbol_infos_size(), module_id.file_path);
    return outcome::success();
  };

  return load_symbols_from_file.Then(main_thread_executor_, std::move(add_symbols));
}

ErrorMessageOr<std::vector<const ModuleData*>> OrbitApp::GetLoadedModulesByPath(
    const std::filesystem::path& module_path) {
  std::vector<std::string> build_ids =
      GetTargetProcess()->FindModuleBuildIdsByPath(module_path.string());

  std::vector<const ModuleData*> result;
  for (const auto& build_id : build_ids) {
    const ModuleData* module_data =
        GetModuleByModuleIdentifier(ModuleIdentifier{module_path.string(), build_id});
    if (module_data == nullptr) {
      ORBIT_ERROR("Module \"%s\" was loaded by the process, but is not part of module manager",
                  module_path.string());
      crash_handler_->DumpWithoutCrash();
      return ErrorMessage{"Unexpected error while loading preset."};
    }

    result.emplace_back(module_data);
  }

  return result;
}

Future<ErrorMessageOr<void>> OrbitApp::LoadPresetModule(const std::filesystem::path& module_path,
                                                        const PresetFile& preset_file) {
  auto modules_data_or_error = GetLoadedModulesByPath(module_path);

  if (modules_data_or_error.has_error()) {
    return modules_data_or_error.error();
  }

  std::vector<const ModuleData*>& modules_data = modules_data_or_error.value();

  if (modules_data.empty()) {
    return ErrorMessage{
        absl::StrFormat("Module \"%s\" is not loaded by process.", module_path.string())};
  }

  // TODO(b/191240539): Ask the user which build_id they prefer.
  if (modules_data.size() > 1) {
    ORBIT_ERROR("Found multiple build_ids (%s) for module \"%s\", will choose the first one",
                absl::StrJoin(modules_data, ", ",
                              [](std::string* out, const ModuleData* module_data) {
                                out->append(module_data->build_id());
                              }),
                module_path.string());
  }

  ORBIT_CHECK(!modules_data.empty());
  const ModuleData* module_data = modules_data.at(0);

  auto handle_hooks_and_frame_tracks =
      [this, module_data,
       preset_file](const ErrorMessageOr<CanceledOr<void>>& result) -> ErrorMessageOr<void> {
    if (result.has_error()) return result.error();
    if (orbit_base::IsCanceled(result.value())) {
      return ErrorMessage{"User canceled symbol loading"};
    }
    const std::string& module_path = module_data->file_path();
    if (preset_file.IsLegacyFileFormat()) {
      SelectFunctionsFromHashes(module_data,
                                preset_file.GetSelectedFunctionHashesForModuleLegacy(module_path));
      EnableFrameTracksFromHashes(
          module_data, preset_file.GetFrameTrackFunctionHashesForModuleLegacy(module_path));

      return outcome::success();
    }

    SelectFunctionsByName(module_data, preset_file.GetSelectedFunctionNamesForModule(module_path));
    EnableFrameTracksByName(module_data,
                            preset_file.GetFrameTrackFunctionNamesForModule(module_path));
    return outcome::success();
  };

  return RetrieveModuleAndLoadSymbols(module_data)
      .Then(main_thread_executor_, std::move(handle_hooks_and_frame_tracks));
}

void OrbitApp::SelectFunctionsFromHashes(const ModuleData* module,
                                         absl::Span<const uint64_t> function_hashes) {
  for (const auto function_hash : function_hashes) {
    const FunctionInfo* const function_info = module->FindFunctionFromHash(function_hash);
    if (function_info == nullptr) {
      ORBIT_ERROR("Could not find function hash %#x in module \"%s\"", function_hash,
                  module->file_path());
      continue;
    }
    SelectFunction(*function_info);
  }
}

void OrbitApp::SelectFunctionsByName(const ModuleData* module,
                                     absl::Span<const std::string> function_names) {
  for (const auto& function_name : function_names) {
    const orbit_client_data::FunctionInfo* const function_info =
        module->FindFunctionFromPrettyName(function_name);
    if (function_info == nullptr) {
      ORBIT_ERROR("Could not find function \"%s\" in module \"%s\"", function_name,
                  module->file_path());
      continue;
    }
    SelectFunction(*function_info);
  }
}

void OrbitApp::EnableFrameTracksFromHashes(const ModuleData* module,
                                           absl::Span<const uint64_t> function_hashes) {
  for (const auto function_hash : function_hashes) {
    const orbit_client_data::FunctionInfo* const function_info =
        module->FindFunctionFromHash(function_hash);
    if (function_info == nullptr) {
      ORBIT_ERROR("Could not find function hash %#x in module \"%s\"", function_hash,
                  module->file_path());
      continue;
    }
    EnableFrameTrack(*function_info);
  }
}

void OrbitApp::EnableFrameTracksByName(const ModuleData* module,
                                       absl::Span<const std::string> function_names) {
  for (const auto& function_name : function_names) {
    const orbit_client_data::FunctionInfo* const function_info =
        module->FindFunctionFromPrettyName(function_name);
    if (function_info == nullptr) {
      ORBIT_ERROR("Could not find function \"%s\" in module \"%s\"", function_name,
                  module->file_path());
      continue;
    }
    EnableFrameTrack(*function_info);
  }
}

Future<ErrorMessageOr<void>> OrbitApp::LoadPreset(const PresetFile& preset_file) {
  ScopedMetric metric{metrics_uploader_, OrbitLogEvent::ORBIT_PRESET_LOAD};
  std::vector<Future<std::string>> load_module_results{};
  auto module_paths = preset_file.GetModulePaths();
  load_module_results.reserve(module_paths.size());

  // First we try to load all preset modules in parallel
  for (const auto& module_path : module_paths) {
    auto load_preset_result = LoadPresetModule(module_path, preset_file);

    orbit_base::ImmediateExecutor immediate_executor{};
    auto future = load_preset_result.Then(
        &immediate_executor,
        [module_path = module_path](const ErrorMessageOr<void>& result) -> std::string {
          if (!result.has_error()) return {};
          // We will return the module_path plus error message in case loading fails.
          return absl::StrFormat("%s, error: \"%s\"", module_path.string(),
                                 result.error().message());
        });

    load_module_results.emplace_back(std::move(future));
  }

  // Then - when all modules are loaded or failed to load - we update the UI and potentially show an
  // error message.
  auto results = orbit_base::WhenAll(absl::MakeConstSpan(load_module_results));
  return results.Then(
      main_thread_executor_,
      [this, metric = std::move(metric), preset_file](
          std::vector<std::string> module_paths_not_found) mutable -> ErrorMessageOr<void> {
        size_t tried_to_load_amount = module_paths_not_found.size();
        module_paths_not_found.erase(
            std::remove_if(module_paths_not_found.begin(), module_paths_not_found.end(),
                           [](const std::string& path) { return path.empty(); }),
            module_paths_not_found.end());

        if (tried_to_load_amount == module_paths_not_found.size()) {
          metric.SetStatusCode(OrbitLogEvent::INTERNAL_ERROR);
          std::string error_message =
              absl::StrFormat("None of the modules of the preset were loaded:\n* %s",
                              absl::StrJoin(module_paths_not_found, "\n* "));
          SendErrorToUi("Preset loading failed", error_message);
          return ErrorMessage{error_message};
        }

        if (!module_paths_not_found.empty()) {
          SendWarningToUi("Preset only partially loaded",
                          absl::StrFormat("The following modules were not loaded:\n* %s",
                                          absl::StrJoin(module_paths_not_found, "\n* ")));
        } else {
          // Then if load was successful and the preset is in old format - convert it to new one.
          auto convertion_result = ConvertPresetToNewFormatIfNecessary(preset_file);
          if (convertion_result.has_error()) {
            ORBIT_ERROR("Unable to convert preset file \"%s\" to new file format: %s",
                        preset_file.file_path().string(), convertion_result.error().message());
          }
        }
        // Then - when all modules are loaded or failed to load - we update the UI and potentially
        // show an error message.
        /*auto results = orbit_base::JoinFutures(absl::MakeConstSpan(load_module_results));
        results.Then(main_thread_executor_, [this, metric = std::move(metric), preset_file](
                                                std::vector<std::string> module_paths_not_found)
        mutable { size_t tried_to_load_amount = module_paths_not_found.size();
          module_paths_not_found.erase(
              std::remove_if(module_paths_not_found.begin(), module_paths_not_found.end(),
                             [](const std::string& path) { return path.empty(); }),
              module_paths_not_found.end());

          if (tried_to_load_amount == module_paths_not_found.size()) {
            metric.SetStatusCode(orbit_metrics_uploader::OrbitLogEvent::INTERNAL_ERROR);
            SendErrorToUi("Preset loading failed",
                          absl::StrFormat("None of the modules of the preset were loaded:\n* %s",
                                          absl::StrJoin(module_paths_not_found, "\n* ")));
            return;
          }

          if (!module_paths_not_found.empty()) {
            SendWarningToUi("Preset only partially loaded",
                            absl::StrFormat("The following modules were not loaded:\n* %s",
                                            absl::StrJoin(module_paths_not_found, "\n* ")));
          } else {
            // Then if load was successful and the preset is in old format - convert it to new
            // one.
            auto convertion_result = ConvertPresetToNewFormatIfNecessary(preset_file);
            if (convertion_result.has_error()) {
              ORBIT_ERROR("Unable to convert preset file \"%s\" to new file format: %s",
                          preset_file.file_path().string(), convertion_result.error().message());
            }
          }
          */

        FireRefreshCallbacks();
        return outcome::success();
      });
}

void OrbitApp::ShowPresetInExplorer(const PresetFile& preset) {
  const auto file_exists = orbit_base::FileOrDirectoryExists(preset.file_path());
  if (file_exists.has_error()) {
    SendErrorToUi("Unable to find preset file: %s", file_exists.error().message());
    return;
  }

#if defined(__linux)
  const QString program{"dbus-send"};
  const QStringList arguments = {"--session",
                                 "--print-reply",
                                 "--dest=org.freedesktop.FileManager1",
                                 "--type=method_call",
                                 "/org/freedesktop/FileManager1",
                                 "org.freedesktop.FileManager1.ShowItems",
                                 QString::fromStdString(absl::StrFormat(
                                     "array:string:file:////%s", preset.file_path().string())),
                                 "string:"};
#elif defined(_WIN32)
  const QString program{"explorer.exe"};
  const QStringList arguments = {
      QString::fromStdString(absl::StrFormat("/select,%s", preset.file_path().string()))};
#endif  // defined(__linux)
  // QProcess::startDetached starts the program `program` with the arguments `arguments` in a
  // new process, and detaches from it. Returns true on success; otherwise returns false.
  if (QProcess::startDetached(program, arguments)) return;

  SendErrorToUi("%s", "Unable to show preset file in explorer.");
}
Future<ErrorMessageOr<void>> OrbitApp::UpdateProcessAndModuleList() {
  ORBIT_SCOPE_FUNCTION;
  functions_data_view_->ClearFunctions();

  auto module_infos = thread_pool_->Schedule(
      [this] { return GetProcessManager()->LoadModuleList(GetTargetProcess()->pid()); });

  auto all_reloaded_modules = module_infos.ThenIfSuccess(
      main_thread_executor_,
      [this](const std::vector<orbit_grpc_protos::ModuleInfo>& module_infos) {
        return ReloadModules(module_infos);
      });

  // `all_modules_reloaded` is a future in a future. So we have to unwrap here.
  return orbit_base::UnwrapFuture(all_reloaded_modules)
      .ThenIfSuccess(main_thread_executor_,
                     [this](const std::vector<ErrorMessageOr<void>>& reload_results) {
                       // We ignore whether reloading a particular module failed to preserve the
                       // behaviour from before refactoring this. This can be changed in the future.
                       std::ignore = reload_results;

                       RefreshUIAfterModuleReload();
                     })
      .ThenIfSuccess(main_thread_executor_,
                     [this]() {
                       if (absl::GetFlag(FLAGS_auto_symbol_loading)) {
                         std::ignore = LoadAllSymbols();
                       }
                     })
      .Then(main_thread_executor_, [this](const ErrorMessageOr<void>& result) {
        if (result.has_error()) {
          std::string error_message =
              absl::StrFormat("Error retrieving modules: %s", result.error().message());
          ORBIT_ERROR("%s", error_message);
          SendErrorToUi("%s", error_message);
        }
        return result;
      });
}

Future<std::vector<ErrorMessageOr<CanceledOr<void>>>> OrbitApp::LoadAllSymbols() {
  const ProcessData& process = GetConnectedOrLoadedProcess();

  std::vector<const ModuleData*> sorted_module_list = SortModuleListWithPrioritizationList(
      module_manager_->GetAllModuleData(),
      {kGgpVlkModulePathSubstring, kNtdllSoFileName, process.full_path()});

  std::vector<Future<ErrorMessageOr<CanceledOr<void>>>> loading_futures;

  for (const ModuleData* module : sorted_module_list) {
    if (module->AreAtLeastFallbackSymbolsLoaded()) continue;

    loading_futures.push_back(RetrieveModuleAndLoadSymbols(module));
  }
  if (data_manager_->enable_auto_frame_track()) {
    // Orbit will try to add the default frame track while loading all symbols.
    AddDefaultFrameTrackOrLogError();
  }

  return orbit_base::WhenAll(absl::MakeConstSpan(loading_futures));
}

void OrbitApp::AddDefaultFrameTrackOrLogError() {
  // The default frame track should be only added once (to give the possibility to the users of
  // manually removing an undesired default FrameTrack in the current session). As the FrameTrack
  // was already added before, we won't log an error in this case.
  if (default_frame_track_was_added_) return;

  const std::filesystem::path default_auto_preset_folder_path =
      orbit_base::GetExecutableDir() / "autopresets";
  const std::filesystem::path stadia_default_preset_path =
      default_auto_preset_folder_path / "stadia-default-frame-track.opr";

  std::vector<std::filesystem::path> auto_preset_paths = {stadia_default_preset_path};

  // Each preset in auto_preset_paths contains a FrameTrack that users might be interested in
  // loading by default. Orbit will try to load automatically just the first loadable preset from
  // the list as we don't want Orbit to automatically add more than one FrameTrack. If no presets
  // could be loaded, Orbit will log an error.
  for (std::filesystem::path preset_path : auto_preset_paths) {
    const ErrorMessageOr<PresetFile> preset = ReadPresetFromFile(preset_path);
    // Errors on reading a preset from a file won't be shown, Orbit simply will try the next preset
    // from the list until one of them is loadable.
    if (preset.has_value() &&
        GetPresetLoadState(preset.value()).state == orbit_data_views::PresetLoadState::kLoadable) {
      std::vector<std::filesystem::path> preset_module_paths = preset.value().GetModulePaths();
      orbit_base::ImmediateExecutor immediate_executor{};
      // Shipped preset files will have only one module. We are not officially supporting users to
      // change the files, but if the user modify internally the preset files, we will load all the
      // modules. In this case multiple messages might appear in the log file.
      for (std::filesystem::path module_path : preset_module_paths) {
        LoadPresetModule(module_path, preset.value())
            .Then(&immediate_executor, [this](ErrorMessageOr<void> result) -> void {
              if (result.has_error()) {
                ORBIT_ERROR(
                    "It was not possible to add a frame track automatically. The desired preset "
                    "couldn't be loaded: %s",
                    result.error().message());
              } else {
                ORBIT_LOG("The default frame track was automatically added.");
                default_frame_track_was_added_ = true;
              }
            });
      }
      return;
    }
  }
  std::string error_message =
      "It was not possible to add a frame track automatically, because none of the presets "
      "available for auto-loading could be loaded. The reason might be that you are not profiling "
      "a Stadia-game running with Vulkan.";
  ORBIT_ERROR("%s", error_message);
}

void OrbitApp::RefreshUIAfterModuleReload() {
  modules_data_view_->UpdateModules(GetTargetProcess());

  // TODO(b/247069854): Avoid FunctionsDataView::ClearFunctions: use
  //  FunctionsDataView::RemoveFunctionsOfModule for the modules that changed or are no longer
  //  loaded, followed by FunctionsDataView::AddFunctions only for the modules that changed.
  functions_data_view_->ClearFunctions();
  auto module_ids = GetTargetProcess()->GetUniqueModuleIdentifiers();
  for (const ModuleIdentifier& module_id : module_ids) {
    ModuleData* module = GetMutableModuleByModuleIdentifier(module_id);
    if (module->AreAtLeastFallbackSymbolsLoaded()) {
      functions_data_view_->AddFunctions(module->GetFunctions());
    }
  }

  FireRefreshCallbacks();
}

Future<std::vector<ErrorMessageOr<void>>> OrbitApp::ReloadModules(
    absl::Span<const ModuleInfo> module_infos) {
  ProcessData* process = GetMutableTargetProcess();

  ORBIT_CHECK(process != nullptr);
  process->UpdateModuleInfos(module_infos);

  // Updating the list of loaded modules (in memory) of a process, can mean that a process has
  // now less loaded modules than before. If the user hooked (selected) functions of a module
  // that is now not used anymore by the process, these functions need to be deselected (A)

  // Updating a module can result in not having symbols(functions) anymore. In that case all
  // functions from this module need to be deselected (B), because they are not valid
  // anymore. These functions are saved (C), so the module can be loaded again and the functions
  // are then selected (hooked) again (D).

  // This all applies similarly to frame tracks that are based on selected functions.

  // Update modules and get the ones to reload.
  std::vector<ModuleData*> modules_to_reload = module_manager_->AddOrUpdateModules(module_infos);

  absl::flat_hash_map<std::string, std::vector<uint64_t>> function_hashes_to_hook_map;
  for (const FunctionInfo& func : data_manager_->GetSelectedFunctions()) {
    const ModuleData* module = GetModuleByModuleIdentifier(func.module_id());
    if (!process->IsModuleLoadedByProcess(module->file_path())) {
      // (A) deselect functions when the module is not loaded by the process anymore
      data_manager_->DeselectFunction(func);
    } else if (!module->AreAtLeastFallbackSymbolsLoaded()) {
      // (B) deselect when module does not have functions anymore
      data_manager_->DeselectFunction(func);
      // (C) Save function hashes, so they can be hooked again after reload
      function_hashes_to_hook_map[module->file_path()].push_back(func.GetPrettyNameHash());
    }
  }
  absl::flat_hash_map<std::string, std::vector<uint64_t>> frame_track_function_hashes_map;
  for (const FunctionInfo& func :
       data_manager_->user_defined_capture_data().frame_track_functions()) {
    const ModuleData* module = GetModuleByModuleIdentifier(func.module_id());
    // Frame tracks are only meaningful if the module for the underlying function is actually
    // loaded by the process.
    if (!process->IsModuleLoadedByProcess(module->file_path())) {
      RemoveFrameTrack(func);
    } else if (!module->AreAtLeastFallbackSymbolsLoaded()) {
      RemoveFrameTrack(func);
      frame_track_function_hashes_map[module->file_path()].push_back(func.GetPrettyNameHash());
    }
  }

  std::vector<Future<ErrorMessageOr<void>>> reloaded_modules;
  reloaded_modules.reserve(modules_to_reload.size());

  for (const auto& module_to_reload : modules_to_reload) {
    std::vector<uint64_t> hooked_functions =
        std::move(function_hashes_to_hook_map[module_to_reload->file_path()]);
    std::vector<uint64_t> frame_tracks =
        std::move(frame_track_function_hashes_map[module_to_reload->file_path()]);

    auto reloaded_module =
        RetrieveModuleAndLoadSymbols(module_to_reload)
            .ThenIfSuccess(main_thread_executor_, [this, module_to_reload,
                                                   hooked_functions = std::move(hooked_functions),
                                                   frame_tracks = std::move(frame_tracks)](
                                                      const CanceledOr<void>& load_result) {
              if (orbit_base::IsCanceled(load_result)) return;

              // (D) Re-hook functions which had been hooked before.
              SelectFunctionsFromHashes(module_to_reload, hooked_functions);
              ORBIT_LOG("Auto hooked functions in module \"%s\"", module_to_reload->file_path());

              EnableFrameTracksFromHashes(module_to_reload, frame_tracks);
              ORBIT_LOG("Added frame tracks in module \"%s\"", module_to_reload->file_path());
            });
    reloaded_modules.emplace_back(std::move(reloaded_module));
  }

  return orbit_base::WhenAll(absl::MakeConstSpan(reloaded_modules));
}

void OrbitApp::SetCollectSchedulerInfo(bool collect_scheduler_info) {
  data_manager_->set_collect_scheduler_info(collect_scheduler_info);
}

void OrbitApp::SetCollectThreadStates(bool collect_thread_states) {
  data_manager_->set_collect_thread_states(collect_thread_states);
}

void OrbitApp::SetTraceGpuSubmissions(bool trace_gpu_submissions) {
  data_manager_->set_trace_gpu_submissions(trace_gpu_submissions);
}

void OrbitApp::SetEnableApi(bool enable_api) { data_manager_->set_enable_api(enable_api); }

void OrbitApp::SetEnableIntrospection(bool enable_introspection) {
  data_manager_->set_enable_introspection(enable_introspection);
}

void OrbitApp::SetDynamicInstrumentationMethod(DynamicInstrumentationMethod method) {
  data_manager_->set_dynamic_instrumentation_method(method);
}

void OrbitApp::SetWineSyscallHandlingMethod(orbit_client_data::WineSyscallHandlingMethod method) {
  data_manager_->set_wine_syscall_handling_method(method);
}

void OrbitApp::SetSamplesPerSecond(double samples_per_second) {
  data_manager_->set_samples_per_second(samples_per_second);
}

void OrbitApp::SetStackDumpSize(uint16_t stack_dump_size) {
  data_manager_->set_stack_dump_size(stack_dump_size);
}

void OrbitApp::SetUnwindingMethod(UnwindingMethod unwinding_method) {
  data_manager_->set_unwinding_method(unwinding_method);
}

void OrbitApp::SetThreadStateChangeCallstackStackDumpSize(uint16_t stack_dump_size) {
  data_manager_->set_thread_state_change_callstack_stack_dump_size(stack_dump_size);
}

void OrbitApp::SetMaxLocalMarkerDepthPerCommandBuffer(
    uint64_t max_local_marker_depth_per_command_buffer) {
  data_manager_->set_max_local_marker_depth_per_command_buffer(
      max_local_marker_depth_per_command_buffer);
}

void OrbitApp::SetEnableAutoFrameTrack(bool enable_auto_frame_track) {
  // If the option is true, Orbit will try to add the default frame track as soon as possible. This
  // might fail because a user can start a capture before the needed symbols are downloaded, so we
  // are additionaly saving the state for the future.
  if (enable_auto_frame_track) {
    AddDefaultFrameTrackOrLogError();
  }
  data_manager_->set_enable_auto_frame_track(enable_auto_frame_track);
}

void OrbitApp::SelectFunction(const orbit_client_data::FunctionInfo& func) {
  ORBIT_LOG("Selected %s (address_=%#x, loaded_module_path_=%s)", func.pretty_name(),
            func.address(), func.module_path());
  data_manager_->SelectFunction(func);
}

void OrbitApp::DeselectFunction(const orbit_client_data::FunctionInfo& func) {
  data_manager_->DeselectFunction(func);
}

[[nodiscard]] bool OrbitApp::IsFunctionSelected(const orbit_client_data::FunctionInfo& func) const {
  return data_manager_->IsFunctionSelected(func);
}

[[nodiscard]] bool OrbitApp::IsFunctionSelected(const SampledFunction& func) const {
  return IsFunctionSelected(func.absolute_address);
}

[[nodiscard]] bool OrbitApp::IsFunctionSelected(uint64_t absolute_address) const {
  const ProcessData* process = GetTargetProcess();
  if (process == nullptr) return false;

  const auto result = process->FindModuleByAddress(absolute_address);
  if (result.has_error()) return false;

  const orbit_client_data::ModuleInMemory& module_in_memory = result.value();

  const ModuleData* module = module_manager_->GetModuleByModuleInMemoryAndAbsoluteAddress(
      module_in_memory, absolute_address);
  if (module == nullptr) return false;

  const uint64_t virtual_address = orbit_module_utils::SymbolAbsoluteAddressToVirtualAddress(
      absolute_address, module_in_memory.start(), module->load_bias(),
      module->executable_segment_offset());
  const FunctionInfo* function = module->FindFunctionByVirtualAddress(virtual_address, false);
  if (function == nullptr) return false;

  return data_manager_->IsFunctionSelected(*function);
}

const InstrumentedFunction* OrbitApp::GetInstrumentedFunction(uint64_t function_id) const {
  return HasCaptureData() ? GetCaptureData().GetInstrumentedFunctionById(function_id) : nullptr;
}

void OrbitApp::SetVisibleScopeIds(absl::flat_hash_set<ScopeId> visible_scope_ids) {
  data_manager_->set_visible_scope_ids(std::move(visible_scope_ids));
  RequestUpdatePrimitives();
}

bool OrbitApp::IsScopeVisible(ScopeId scope_id) const {
  return data_manager_->IsScopeVisible(scope_id);
}

std::optional<ScopeId> OrbitApp::GetHighlightedScopeId() const {
  return data_manager_->highlighted_scope_id();
}

void OrbitApp::SetHighlightedScopeId(std::optional<ScopeId> highlighted_scope_id) {
  data_manager_->set_highlighted_scope_id(highlighted_scope_id);
  RequestUpdatePrimitives();
}

ThreadID OrbitApp::selected_thread_id() const { return data_manager_->selected_thread_id(); }

void OrbitApp::set_selected_thread_id(ThreadID thread_id) {
  RequestUpdatePrimitives();
  return data_manager_->set_selected_thread_id(thread_id);
}

std::optional<ThreadStateSliceInfo> OrbitApp::selected_thread_state_slice() const {
  return data_manager_->selected_thread_state_slice();
}

void OrbitApp::set_selected_thread_state_slice(
    std::optional<ThreadStateSliceInfo> thread_state_slice) {
  RequestUpdatePrimitives();
  data_manager_->set_selected_thread_state_slice(thread_state_slice);
}

std::optional<ThreadStateSliceInfo> OrbitApp::hovered_thread_state_slice() const {
  return data_manager_->hovered_thread_state_slice();
}

void OrbitApp::set_hovered_thread_state_slice(
    std::optional<ThreadStateSliceInfo> thread_state_slice) {
  RequestUpdatePrimitives();
  data_manager_->set_hovered_thread_state_slice(thread_state_slice);
}

const orbit_client_protos::TimerInfo* OrbitApp::selected_timer() const {
  return data_manager_->selected_timer();
}

void OrbitApp::SelectTimer(const orbit_client_protos::TimerInfo* timer_info) {
  data_manager_->set_selected_timer(timer_info);
  const std::optional<ScopeId> scope_id =
      timer_info != nullptr ? GetCaptureData().ProvideScopeId(*timer_info) : std::nullopt;
  data_manager_->set_highlighted_scope_id(scope_id);

  const uint64_t group_id = timer_info != nullptr ? timer_info->group_id() : kOrbitDefaultGroupId;
  data_manager_->set_highlighted_group_id(group_id);

  ORBIT_CHECK(timer_selected_callback_);
  timer_selected_callback_(timer_info);
  RequestUpdatePrimitives();
}

void OrbitApp::DeselectTimer() {
  data_manager_->set_selected_timer(nullptr);
  RequestUpdatePrimitives();
}

std::optional<ScopeId> OrbitApp::GetScopeIdToHighlight() const {
  const orbit_client_protos::TimerInfo* timer_info = selected_timer();

  if (timer_info == nullptr) return GetHighlightedScopeId();
  return GetCaptureData().ProvideScopeId(*timer_info);
}

uint64_t OrbitApp::GetGroupIdToHighlight() const {
  const orbit_client_protos::TimerInfo* timer_info = selected_timer();

  uint64_t selected_group_id =
      timer_info != nullptr ? timer_info->group_id() : data_manager_->highlighted_group_id();

  return selected_group_id;
}

void OrbitApp::SetCaptureDataSelectionFields(
    const std::vector<CallstackEvent>& selected_callstack_events, bool origin_is_multiple_threads) {
  const CallstackData& callstack_data = GetCaptureData().GetCallstackData();
  std::unique_ptr<CallstackData> selection_callstack_data = std::make_unique<CallstackData>();
  for (const CallstackEvent& event : selected_callstack_events) {
    selection_callstack_data->AddCallstackFromKnownCallstackData(event, callstack_data);
  }
  GetMutableCaptureData().set_selection_callstack_data(std::move(selection_callstack_data));

  // Generate selection report.
  PostProcessedSamplingData selection_post_processed_sampling_data =
      orbit_client_model::CreatePostProcessedSamplingData(
          GetCaptureData().selection_callstack_data(), GetCaptureData(), *module_manager_,
          /*generate_summary*/ origin_is_multiple_threads);
  GetMutableCaptureData().set_selection_post_processed_sampling_data(
      std::move(selection_post_processed_sampling_data));
}

void OrbitApp::SelectCallstackEvents(const std::vector<CallstackEvent>& selected_callstack_events,
                                     bool origin_is_multiple_threads) {
  main_window_->ClearCallstackInspection();
  SetCaptureDataSelectionFields(selected_callstack_events, origin_is_multiple_threads);
  SetSelectionTopDownView(GetCaptureData().selection_post_processed_sampling_data(),
                          GetCaptureData());
  SetSelectionBottomUpView(GetCaptureData().selection_post_processed_sampling_data(),
                           GetCaptureData());
  SetSelectionReport(&GetCaptureData().selection_callstack_data(),
                     &GetCaptureData().selection_post_processed_sampling_data(),
                     /*has_summary*/ origin_is_multiple_threads);
}

void OrbitApp::InspectCallstackEvents(const std::vector<CallstackEvent>& selected_callstack_events,
                                      bool origin_is_multiple_threads) {
  SetCaptureDataSelectionFields(selected_callstack_events, origin_is_multiple_threads);
  std::unique_ptr<SamplingReport> report = std::make_unique<SamplingReport>(
      this, &GetCaptureData().selection_callstack_data(),
      &GetCaptureData().selection_post_processed_sampling_data(), metrics_uploader_,
      /*generate_summary*/ origin_is_multiple_threads);
  main_window_->SetCallstackInspection(
      CallTreeView::CreateTopDownViewFromPostProcessedSamplingData(
          GetCaptureData().selection_post_processed_sampling_data(), *module_manager_,
          GetCaptureData()),
      CallTreeView::CreateBottomUpViewFromPostProcessedSamplingData(
          GetCaptureData().selection_post_processed_sampling_data(), *module_manager_,
          GetCaptureData()),
      GetOrCreateSelectionCallstackDataView(), std::move(report));
  FireRefreshCallbacks();
}

void OrbitApp::ClearInspection() {
  SetCaptureDataSelectionFields(std::vector<CallstackEvent>(),
                                /*origin_is_multiple_threads*/ false);
  main_window_->ClearCallstackInspection();
  FireRefreshCallbacks();
}

void OrbitApp::UpdateAfterSymbolLoading() {
  ORBIT_SCOPE_FUNCTION;
  if (!HasCaptureData()) {
    return;
  }
  const CaptureData& capture_data = GetCaptureData();

  if (sampling_report_ != nullptr) {
    PostProcessedSamplingData post_processed_sampling_data =
        orbit_client_model::CreatePostProcessedSamplingData(capture_data.GetCallstackData(),
                                                            capture_data, *module_manager_);
    GetMutableCaptureData().set_post_processed_sampling_data(post_processed_sampling_data);
    sampling_report_->UpdateReport(&capture_data.GetCallstackData(),
                                   &capture_data.post_processed_sampling_data());
    SetTopDownView(capture_data);
    SetBottomUpView(capture_data);
  }

  if (selection_report_ == nullptr) {
    return;
  }

  PostProcessedSamplingData selection_post_processed_sampling_data =
      orbit_client_model::CreatePostProcessedSamplingData(capture_data.selection_callstack_data(),
                                                          capture_data, *module_manager_,
                                                          selection_report_->has_summary());
  GetMutableCaptureData().set_selection_post_processed_sampling_data(
      std::move(selection_post_processed_sampling_data));

  SetSelectionTopDownView(capture_data.selection_post_processed_sampling_data(), capture_data);
  SetSelectionBottomUpView(capture_data.selection_post_processed_sampling_data(), capture_data);
  selection_report_->UpdateReport(&capture_data.selection_callstack_data(),
                                  &capture_data.selection_post_processed_sampling_data());
}

void OrbitApp::UpdateAfterSymbolLoadingThrottled() { update_after_symbol_loading_throttle_.Fire(); }

void OrbitApp::UpdateAfterCaptureCleared() {
  PostProcessedSamplingData empty_post_processed_sampling_data;
  absl::flat_hash_map<uint64_t, std::shared_ptr<CallstackInfo>> empty_unique_callstacks;

  ClearSamplingReport();
  ClearSelectionReport();
  ClearTopDownView();
  ClearSelectionTopDownView();
  ClearBottomUpView();
  ClearSelectionBottomUpView();
}

orbit_data_views::DataView* OrbitApp::GetOrCreateDataView(DataViewType type) {
  switch (type) {
    case DataViewType::kFunctions:
      if (!functions_data_view_) {
        functions_data_view_ = DataView::CreateAndInit<FunctionsDataView>(this, metrics_uploader_);
        panels_.push_back(functions_data_view_.get());
      }
      return functions_data_view_.get();

    case DataViewType::kCallstack:
      if (!callstack_data_view_) {
        callstack_data_view_ = DataView::CreateAndInit<CallstackDataView>(this, metrics_uploader_);
        panels_.push_back(callstack_data_view_.get());
      }
      return callstack_data_view_.get();

    case DataViewType::kModules:
      if (!modules_data_view_) {
        modules_data_view_ = DataView::CreateAndInit<ModulesDataView>(this, metrics_uploader_);
        panels_.push_back(modules_data_view_.get());
      }
      return modules_data_view_.get();

    case DataViewType::kPresets:
      if (!presets_data_view_) {
        presets_data_view_ = DataView::CreateAndInit<PresetsDataView>(this, metrics_uploader_);
        panels_.push_back(presets_data_view_.get());
      }
      return presets_data_view_.get();

    case DataViewType::kSampling:
      ORBIT_FATAL(
          "DataViewType::kSampling Data View construction is not supported by"
          "the factory.");
    case DataViewType::kLiveFunctions:
      ORBIT_FATAL("DataViewType::kLiveFunctions should not be used with the factory.");

    case DataViewType::kAll:
      ORBIT_FATAL("DataViewType::kAll should not be used with the factory.");

    case DataViewType::kTracepoints:
      if (!tracepoints_data_view_) {
        tracepoints_data_view_ =
            DataView::CreateAndInit<TracepointsDataView>(this, metrics_uploader_);
        panels_.push_back(tracepoints_data_view_.get());
      }
      return tracepoints_data_view_.get();

    case DataViewType::kInvalid:
      ORBIT_FATAL("DataViewType::kInvalid should not be used with the factory.");
  }
  ORBIT_FATAL("Unreachable");
}

orbit_data_views::DataView* OrbitApp::GetOrCreateSelectionCallstackDataView() {
  if (selection_callstack_data_view_ == nullptr) {
    selection_callstack_data_view_ =
        DataView::CreateAndInit<CallstackDataView>(this, metrics_uploader_);
    panels_.push_back(selection_callstack_data_view_.get());
  }
  return selection_callstack_data_view_.get();
}

void OrbitApp::FilterTracks(const std::string& filter) {
  GetMutableTimeGraph()->GetTrackContainer()->SetThreadFilter(filter);
}

void OrbitApp::CrashOrbitService(CrashOrbitServiceRequest_CrashType crash_type) {
  if (IsDevMode()) {
    thread_pool_->Schedule([crash_type, this] { crash_manager_->CrashOrbitService(crash_type); });
  }
}

CaptureClient::State OrbitApp::GetCaptureState() const {
  return capture_client_ != nullptr ? capture_client_->state() : CaptureClient::State::kStopped;
}

bool OrbitApp::IsCapturing() const {
  return capture_client_ != nullptr && capture_client_->IsCapturing();
}

bool OrbitApp::IsLoadingCapture() const {
  return data_source_ == orbit_client_data::CaptureData::DataSource::kLoadedCapture;
}

void OrbitApp::SelectTracepoint(const TracepointInfo& tracepoint) {
  data_manager_->SelectTracepoint(tracepoint);
}

void OrbitApp::DeselectTracepoint(const TracepointInfo& tracepoint) {
  data_manager_->DeselectTracepoint(tracepoint);
}

[[nodiscard]] bool OrbitApp::IsTracepointSelected(const TracepointInfo& info) const {
  return data_manager_->IsTracepointSelected(info);
}

void OrbitApp::EnableFrameTrack(const FunctionInfo& function) {
  data_manager_->EnableFrameTrack(function);
  metrics_uploader_->SendLogEvent(OrbitLogEvent::ORBIT_FRAME_TRACK_ENABLED);
}

void OrbitApp::DisableFrameTrack(const FunctionInfo& function) {
  if (data_manager_->IsFrameTrackEnabled(function)) {
    data_manager_->DisableFrameTrack(function);
    metrics_uploader_->SendLogEvent(OrbitLogEvent::ORBIT_FRAME_TRACK_DISABLED);
  }
}

void OrbitApp::AddFrameTrack(const FunctionInfo& function) {
  if (!HasCaptureData()) return;

  std::optional<uint64_t> instrumented_function_id =
      orbit_client_data::FindInstrumentedFunctionIdSlow(GetCaptureData(), function);
  // If the function is not instrumented - ignore it. This happens when user
  // enables frame tracks for a not instrumented function from the function list.
  if (!instrumented_function_id) return;

  AddFrameTrack(instrumented_function_id.value());
}

void OrbitApp::AddFrameTrack(uint64_t instrumented_function_id) {
  ORBIT_CHECK(instrumented_function_id != orbit_grpc_protos::kInvalidFunctionId);
  ORBIT_CHECK(std::this_thread::get_id() == main_thread_id_);
  if (!HasCaptureData()) return;

  const CaptureData& capture_data = GetCaptureData();
  const std::optional<ScopeId> scope_id =
      capture_data.FunctionIdToScopeId(instrumented_function_id);
  ORBIT_CHECK(scope_id.has_value());

  // We only add a frame track to the actual capture data if the function for the frame
  // track actually has hits in the capture data. Otherwise we can end up in inconsistent
  // states where "empty" frame tracks exist in the capture data (which would also be
  // serialized).
  const ScopeStats& stats = capture_data.GetScopeStatsOrDefault(scope_id.value());
  if (stats.count() > 1) {
    frame_track_online_processor_.AddFrameTrack(instrumented_function_id);
    GetMutableCaptureData().EnableFrameTrack(instrumented_function_id);
    if (!IsCapturing()) {
      AddFrameTrackTimers(instrumented_function_id);
    }
    TrySaveUserDefinedCaptureInfo();
    metrics_uploader_->SendLogEvent(OrbitLogEvent::ORBIT_FRAME_TRACK_ADDED_VISIBLE);
    return;
  }

  const InstrumentedFunction* function =
      GetCaptureData().GetInstrumentedFunctionById(instrumented_function_id);
  constexpr const char* kDontShowAgainEmptyFrameTrackWarningKey = "EmptyFrameTrackWarning";
  const std::string title = "Frame track not added";
  const std::string message = absl::StrFormat(
      "Frame track enabled for function \"%s\", but since the function does not have any hits in "
      "the current capture, a frame track was not added to the capture.",
      function->function_name());
  main_window_->ShowWarningWithDontShowAgainCheckboxIfNeeded(
      title, message, kDontShowAgainEmptyFrameTrackWarningKey);
  metrics_uploader_->SendLogEvent(OrbitLogEvent::ORBIT_FRAME_TRACK_ADDED_INVISIBLE);
}

void OrbitApp::RemoveFrameTrack(const FunctionInfo& function) {
  // Ignore this call if there is no capture data
  if (!HasCaptureData()) return;

  std::optional<uint64_t> instrumented_function_id =
      orbit_client_data::FindInstrumentedFunctionIdSlow(GetCaptureData(), function);
  // If the function is not instrumented - ignore it. This happens when user
  // enables frame tracks for a not instrumented function from the function list.
  if (!instrumented_function_id) return;

  RemoveFrameTrack(instrumented_function_id.value());
}

void OrbitApp::RemoveFrameTrack(uint64_t instrumented_function_id) {
  ORBIT_CHECK(std::this_thread::get_id() == main_thread_id_);
  if (!HasCaptureData()) return;

  // We can only remove the frame track from the capture data if we have capture data and
  // the frame track is actually enabled in the capture data.
  if (GetCaptureData().IsFrameTrackEnabled(instrumented_function_id)) {
    frame_track_online_processor_.RemoveFrameTrack(instrumented_function_id);
    GetMutableCaptureData().DisableFrameTrack(instrumented_function_id);
    GetMutableTimeGraph()->GetTrackContainer()->RemoveFrameTrack(instrumented_function_id);
    TrySaveUserDefinedCaptureInfo();
    metrics_uploader_->SendLogEvent(OrbitLogEvent::ORBIT_FRAME_TRACK_REMOVED);
  }
}

bool OrbitApp::IsFrameTrackEnabled(const FunctionInfo& function) const {
  return data_manager_->IsFrameTrackEnabled(function);
}

bool OrbitApp::HasFrameTrackInCaptureData(uint64_t instrumented_function_id) const {
  return GetTimeGraph()->GetTrackContainer()->HasFrameTrack(instrumented_function_id);
}

void OrbitApp::JumpToTimerAndZoom(ScopeId scope_id, JumpToTimerMode selection_mode) {
  switch (selection_mode) {
    case JumpToTimerMode::kFirst: {
      const auto* first_timer = GetMutableTimeGraph()->FindNextScopeTimer(
          scope_id, std::numeric_limits<uint64_t>::lowest());
      if (first_timer != nullptr) GetMutableTimeGraph()->SelectAndZoom(first_timer);
      break;
    }
    case JumpToTimerMode::kLast: {
      const auto* last_timer = GetMutableTimeGraph()->FindPreviousScopeTimer(
          scope_id, std::numeric_limits<uint64_t>::max());
      if (last_timer != nullptr) GetMutableTimeGraph()->SelectAndZoom(last_timer);
      break;
    }
    case JumpToTimerMode::kMin: {
      auto [min_timer, unused_max_timer] = GetMutableTimeGraph()->GetMinMaxTimerForScope(scope_id);
      if (min_timer != nullptr) GetMutableTimeGraph()->SelectAndZoom(min_timer);
      break;
    }
    case JumpToTimerMode::kMax: {
      auto [unused_min_timer, max_timer] = GetMutableTimeGraph()->GetMinMaxTimerForScope(scope_id);
      if (max_timer != nullptr) GetMutableTimeGraph()->SelectAndZoom(max_timer);
      break;
    }
  }
}

[[nodiscard]] std::vector<const orbit_client_data::TimerChain*> OrbitApp::GetAllThreadTimerChains()
    const {
  return GetTimeGraph()->GetAllThreadTrackTimerChains();
}

void OrbitApp::RefreshFrameTracks() {
  ORBIT_CHECK(HasCaptureData());
  ORBIT_CHECK(std::this_thread::get_id() == main_thread_id_);
  for (const auto& function_id : GetCaptureData().frame_track_function_ids()) {
    GetMutableTimeGraph()->GetTrackContainer()->RemoveFrameTrack(function_id);
    AddFrameTrackTimers(function_id);
  }
  GetMutableTimeGraph()->GetTrackManager()->RequestTrackSorting();
}

void OrbitApp::AddFrameTrackTimers(uint64_t instrumented_function_id) {
  ORBIT_CHECK(HasCaptureData());
  const CaptureData& capture_data = GetCaptureData();
  const std::optional<ScopeId> scope_id =
      capture_data.FunctionIdToScopeId(instrumented_function_id);
  ORBIT_CHECK(scope_id.has_value());

  const ScopeStats& stats = capture_data.GetScopeStatsOrDefault(scope_id.value());
  if (stats.count() == 0) {
    return;
  }

  std::vector<const TimerChain*> chains = GetMutableTimeGraph()->GetAllThreadTrackTimerChains();

  std::vector<uint64_t> all_start_times;

  for (const TimerChain* chain : chains) {
    for (const TimerBlock& block : *chain) {
      for (uint64_t i = 0; i < block.size(); ++i) {
        const TimerInfo& timer_info = block[i];
        if (timer_info.function_id() == instrumented_function_id) {
          all_start_times.push_back(timer_info.start());
        }
      }
    }
  }
  std::sort(all_start_times.begin(), all_start_times.end());

  const InstrumentedFunction* function =
      GetCaptureData().GetInstrumentedFunctionById(instrumented_function_id);
  ORBIT_CHECK(function != nullptr);

  for (size_t k = 0; k < all_start_times.size() - 1; ++k) {
    TimerInfo frame_timer;
    orbit_gl::CreateFrameTrackTimer(instrumented_function_id, all_start_times[k],
                                    all_start_times[k + 1], k, &frame_timer);
    GetMutableTimeGraph()->ProcessTimer(frame_timer, function);
  }
}

void OrbitApp::SetTargetProcess(ProcessData* process) {
  ORBIT_CHECK(process != nullptr);

  if (process != process_) {
    data_manager_->ClearSelectedFunctions();
    data_manager_->ClearUserDefinedCaptureData();
    process_ = process;
  }
}

ErrorMessageOr<void> OrbitApp::ConvertPresetToNewFormatIfNecessary(const PresetFile& preset_file) {
  if (!preset_file.IsLegacyFileFormat()) {
    return outcome::success();
  }

  ORBIT_LOG("Converting preset file \"%s\" to new format.", preset_file.file_path().string());

  // Convert first
  PresetInfo new_info;
  for (const auto& module_path : preset_file.GetModulePaths()) {
    OUTCOME_TRY(auto&& modules_data, GetLoadedModulesByPath(module_path.string()));
    if (modules_data.empty()) {
      return ErrorMessage{
          absl::StrFormat("Module \"%s\" is not loaded by process.", module_path.string())};
    }
    const ModuleData* module_data = modules_data.at(0);

    PresetModule module_info;

    for (uint64_t function_hash :
         preset_file.GetSelectedFunctionHashesForModuleLegacy(module_path)) {
      const auto* function_info = module_data->FindFunctionFromHash(function_hash);
      if (function_info == nullptr) {
        ORBIT_ERROR("Could not find function hash %#x in module \"%s\"", function_hash,
                    module_path.string());
        continue;
      }
      module_info.add_function_names(function_info->pretty_name());
    }

    for (uint64_t function_hash :
         preset_file.GetFrameTrackFunctionHashesForModuleLegacy(module_path)) {
      const auto* function_info = module_data->FindFunctionFromHash(function_hash);
      if (function_info == nullptr) {
        ORBIT_ERROR("Could not find function hash %#x in module \"%s\"", function_hash,
                    module_path.string());
        continue;
      }
      module_info.add_frame_track_function_names(function_info->pretty_name());
    }

    (*new_info.mutable_modules())[module_path.string()] = module_info;
  }

  // Backup the old file
  const std::string file_path = preset_file.file_path().string();
  const std::string backup_file_path = file_path + ".backup";
  if (rename(file_path.c_str(), backup_file_path.c_str()) == -1) {
    return ErrorMessage{absl::StrFormat(R"(Unable to rename "%s" to "%s": %s)", file_path,
                                        backup_file_path, SafeStrerror(errno))};
  }

  PresetFile new_preset_file{file_path, new_info};

  auto save_to_file_result = new_preset_file.SaveToFile();
  if (save_to_file_result.has_error()) {
    // restore the backup
    if (rename(backup_file_path.c_str(), file_path.c_str()) == -1) {
      ORBIT_ERROR(R"(Unable to rename "%s" to "%s": %s)", file_path, backup_file_path,
                  SafeStrerror(errno));
    }

    return save_to_file_result.error();
  }

  return outcome::success();
}

void OrbitApp::CaptureMetricProcessTimer(const orbit_client_protos::TimerInfo& timer) {
  if (timer.function_id() != 0) {
    metrics_capture_complete_data_.number_of_instrumented_function_timers++;
    return;
  }
  switch (timer.type()) {
    case orbit_client_protos::TimerInfo_Type_kGpuActivity:
      metrics_capture_complete_data_.number_of_gpu_activity_timers++;
      break;
    case orbit_client_protos::TimerInfo_Type_kGpuCommandBuffer:
      metrics_capture_complete_data_.number_of_vulkan_layer_gpu_command_buffer_timers++;
      break;
    case orbit_client_protos::TimerInfo_Type_kGpuDebugMarker:
      metrics_capture_complete_data_.number_of_vulkan_layer_gpu_debug_marker_timers++;
      break;
    case orbit_client_protos::TimerInfo_Type_kApiScope:
      metrics_capture_complete_data_.number_of_manual_start_timers++;
      break;
    case orbit_client_protos::TimerInfo_Type_kApiScopeAsync:
      metrics_capture_complete_data_.number_of_manual_start_async_timers++;
      break;
    default:
      break;
  }
}

void OrbitApp::TrySaveUserDefinedCaptureInfo() {
  ORBIT_CHECK(std::this_thread::get_id() == main_thread_id_);
  ORBIT_CHECK(HasCaptureData());
  if (IsCapturing()) {
    // We are going to save it at the end of capture anyways.
    return;
  }

  const auto& file_path = GetCaptureData().file_path();
  if (!file_path.has_value()) {
    ORBIT_LOG(
        "Warning: capture is not backed by a file, skipping the save of UserDefinedCaptureInfo");
    return;
  }

  orbit_client_protos::UserDefinedCaptureInfo capture_info;
  const auto& frame_track_function_ids = GetCaptureData().frame_track_function_ids();
  *capture_info.mutable_frame_tracks_info()->mutable_frame_track_function_ids() = {
      frame_track_function_ids.begin(), frame_track_function_ids.end()};
  thread_pool_->Schedule([this, capture_info = std::move(capture_info),
                          file_path = file_path.value()] {
    ORBIT_LOG("Saving user defined capture info to \"%s\"", file_path.string());
    auto write_result = orbit_capture_file::WriteUserData(file_path, capture_info);
    if (write_result.has_error()) {
      SendErrorToUi("Save failed", absl::StrFormat("Save to \"%s\" failed: %s", file_path.string(),
                                                   write_result.error().message()));
    }
    capture_file_info_manager_.AddOrTouchCaptureFile(file_path, GetCaptureTime());
  });
}

[[nodiscard]] const orbit_statistics::BinomialConfidenceIntervalEstimator&
OrbitApp::GetConfidenceIntervalEstimator() const {
  return confidence_interval_estimator_;
}

void OrbitApp::ShowHistogram(const std::vector<uint64_t>* data, const std::string& scope_name,
                             std::optional<ScopeId> scope_id) {
  main_window_->ShowHistogram(data, scope_name, scope_id);
}

[[nodiscard]] bool OrbitApp::IsModuleDownloading(const ModuleData* module) const {
  ORBIT_CHECK(module != nullptr);
  ORBIT_CHECK(main_thread_id_ == std::this_thread::get_id());
  return symbol_files_currently_downloading_.contains(module->file_path());
}

SymbolLoadingState OrbitApp::GetSymbolLoadingStateForModule(const ModuleData* module) const {
  ORBIT_CHECK(module != nullptr);
  ORBIT_CHECK(main_thread_id_ == std::this_thread::get_id());

  if (IsModuleDownloading(module)) return SymbolLoadingState::kDownloading;

  ModuleIdentifier module_id = module->module_id();
  if (symbols_currently_loading_.contains(module_id)) {
    return SymbolLoadingState::kLoading;
  }

  switch (module->GetLoadedSymbolsCompleteness()) {
    case ModuleData::SymbolCompleteness::kNoSymbols:
      break;
    case ModuleData::SymbolCompleteness::kDynamicLinkingAndUnwindInfo:
      return SymbolLoadingState::kFallback;
    case ModuleData::SymbolCompleteness::kDebugSymbols:
      return SymbolLoadingState::kLoaded;
  }

  if (download_disabled_modules_.contains(module->file_path())) {
    return SymbolLoadingState::kDisabled;
  }

  if (modules_with_symbol_loading_error_.contains(module_id)) {
    return SymbolLoadingState::kError;
  }

  return SymbolLoadingState::kUnknown;
}

bool OrbitApp::IsSymbolLoadingInProgressForModule(
    const orbit_client_data::ModuleData* module) const {
  ORBIT_CHECK(module != nullptr);
  ORBIT_CHECK(main_thread_id_ == std::this_thread::get_id());
  return symbols_currently_loading_.contains(module->module_id());
}

void OrbitApp::RequestSymbolDownloadStop(absl::Span<const ModuleData* const> modules,
                                         bool show_dialog) {
  ORBIT_CHECK(main_thread_id_ == std::this_thread::get_id());

  for (const auto* module : modules) {
    if (!symbol_files_currently_downloading_.contains(module->file_path())) {
      // Download already ended
      continue;
    }
    if (show_dialog) {
      CanceledOr<void> canceled_or = main_window_->DisplayStopDownloadDialog(module);
      if (orbit_base::IsCanceled(canceled_or)) continue;
    }

    if (!symbol_files_currently_downloading_.contains(module->file_path())) {
      // Download already ended (while user was looking at the dialog)
      continue;
    }
    symbol_files_currently_downloading_.at(module->file_path()).stop_source.RequestStop();
  }
}

void OrbitApp::RequestSymbolDownloadStop(
    absl::Span<const orbit_client_data::ModuleData* const> modules) {
  RequestSymbolDownloadStop(modules, true);
}

const ProcessData& OrbitApp::GetConnectedOrLoadedProcess() const {
  const ProcessData* process_ptr = GetTargetProcess();  // This is the connected Process
  if (process_ptr == nullptr) {
    // Orbit is not currently connected, so this uses the process from capture data, which then is
    // from the capture that was loaded from file.
    process_ptr = GetCaptureData().process();
  }
  ORBIT_CHECK(process_ptr != nullptr);
  return *process_ptr;
}
