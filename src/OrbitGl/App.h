// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_GL_APP_H_
#define ORBIT_GL_APP_H_

#include <absl/container/flat_hash_map.h>
#include <absl/container/flat_hash_set.h>
#include <absl/types/span.h>
#include <grpc/impl/codegen/connectivity_state.h>
#include <grpcpp/channel.h>

#include <atomic>
#include <chrono>
#include <cstdint>
#include <filesystem>
#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <thread>
#include <utility>
#include <vector>

#include "CallTreeView.h"
#include "CaptureClient/AbstractCaptureListener.h"
#include "CaptureClient/AppInterface.h"
#include "CaptureClient/CaptureClient.h"
#include "CaptureClient/CaptureListener.h"
#include "CaptureClient/ClientCaptureOptions.h"
#include "CaptureFile/CaptureFile.h"
#include "CaptureFileInfo/Manager.h"
#include "CaptureWindow.h"
#include "ClientData/CallstackType.h"
#include "ClientData/CaptureData.h"
#include "ClientData/DataManager.h"
#include "ClientData/FunctionInfo.h"
#include "ClientData/LinuxAddressInfo.h"
#include "ClientData/ModuleData.h"
#include "ClientData/ModuleManager.h"
#include "ClientData/PostProcessedSamplingData.h"
#include "ClientData/ProcessData.h"
#include "ClientData/ThreadStateSliceInfo.h"
#include "ClientData/TracepointCustom.h"
#include "ClientData/UserDefinedCaptureData.h"
#include "ClientData/WineSyscallHandlingMethod.h"
#include "ClientProtos/capture_data.pb.h"
#include "ClientProtos/preset.pb.h"
#include "ClientServices/CrashManager.h"
#include "ClientServices/ProcessManager.h"
#include "ClientServices/TracepointServiceClient.h"
#include "CodeReport/DisassemblyReport.h"
#include "DataViewFactory.h"
#include "DataViews/AppInterface.h"
#include "DataViews/CallstackDataView.h"
#include "DataViews/DataViewType.h"
#include "DataViews/FunctionsDataView.h"
#include "DataViews/LiveFunctionsDataView.h"
#include "DataViews/ModulesDataView.h"
#include "DataViews/PresetLoadState.h"
#include "DataViews/PresetsDataView.h"
#include "DataViews/SymbolLoadingState.h"
#include "DataViews/TracepointsDataView.h"
#include "FramePointerValidatorClient.h"
#include "FrameTrackOnlineProcessor.h"
#include "GlCanvas.h"
#include "GrpcProtos/capture.pb.h"
#include "GrpcProtos/services.pb.h"
#include "GrpcProtos/symbol.pb.h"
#include "GrpcProtos/tracepoint.pb.h"
#include "Http/HttpDownloadManager.h"
#include "IntrospectionWindow.h"
#include "MainWindowInterface.h"
#include "ManualInstrumentationManager.h"
#include "MetricsUploader/CaptureMetric.h"
#include "MetricsUploader/MetricsUploader.h"
#include "OrbitBase/CanceledOr.h"
#include "OrbitBase/CrashHandler.h"
#include "OrbitBase/Future.h"
#include "OrbitBase/Logging.h"
#include "OrbitBase/MainThreadExecutor.h"
#include "OrbitBase/Result.h"
#include "OrbitBase/StopSource.h"
#include "OrbitBase/StopToken.h"
#include "OrbitBase/ThreadPool.h"
#include "OrbitGgp/Client.h"
#include "OrbitPaths/Paths.h"
#include "QtUtils/Throttle.h"
#include "RemoteSymbolProvider/MicrosoftSymbolServerSymbolProvider.h"
#include "RemoteSymbolProvider/StadiaSymbolStoreSymbolProvider.h"
#include "SamplingReport.h"
#include "Statistics/BinomialConfidenceInterval.h"
#include "StringManager/StringManager.h"
#include "SymbolProvider/ModuleIdentifier.h"
#include "Symbols/SymbolHelper.h"

class OrbitApp final : public DataViewFactory,
                       public orbit_capture_client::AbstractCaptureListener<OrbitApp>,
                       public orbit_data_views::AppInterface,
                       public orbit_capture_client::CaptureControlInterface {
  using ScopeId = orbit_client_data::ScopeId;

 public:
  explicit OrbitApp(orbit_gl::MainWindowInterface* main_window,
                    orbit_base::MainThreadExecutor* main_thread_executor,
                    const orbit_base::CrashHandler* crash_handler,
                    orbit_metrics_uploader::MetricsUploader* metrics_uploader = nullptr);
  ~OrbitApp() override;

  static std::unique_ptr<OrbitApp> Create(
      orbit_gl::MainWindowInterface* main_window,
      orbit_base::MainThreadExecutor* main_thread_executor,
      const orbit_base::CrashHandler* crash_handler,
      orbit_metrics_uploader::MetricsUploader* metrics_uploader = nullptr);

  void PostInit(bool is_connected);
  void MainTick();

  [[nodiscard]] absl::Duration GetCaptureTime() const;
  [[nodiscard]] absl::Duration GetCaptureTimeAt(uint64_t timestamp_ns) const;

  [[nodiscard]] std::string GetSaveFile(const std::string& extension) const override;
  void SetClipboard(const std::string& text) override;

  ErrorMessageOr<void> OnSavePreset(const std::string& file_name);
  ErrorMessageOr<void> OnLoadPreset(const std::string& file_name);
  orbit_base::Future<ErrorMessageOr<CaptureOutcome>> LoadCaptureFromFile(
      const std::filesystem::path& file_path);

  orbit_base::Future<ErrorMessageOr<void>> MoveCaptureFile(const std::filesystem::path& src,
                                                           const std::filesystem::path& dest);
  void OnLoadCaptureCancelRequested();

  // --------- orbit_capture_client::CaptureControlInterface  ----------
  [[nodiscard]] orbit_capture_client::CaptureClient::State GetCaptureState() const override;
  [[nodiscard]] bool IsCapturing() const override;

  void StartCapture() override;
  void StopCapture() override;
  void AbortCapture() override;
  void ToggleCapture() override;
  // ---------------------------------------------------------

  void ClearCapture();

  [[nodiscard]] bool IsLoadingCapture() const;

  [[nodiscard]] const orbit_client_data::ModuleManager* GetModuleManager() const override {
    ORBIT_CHECK(module_manager_ != nullptr);
    return module_manager_.get();
  }

  [[nodiscard]] orbit_client_data::ModuleManager* GetMutableModuleManager() override {
    ORBIT_CHECK(module_manager_ != nullptr);
    return module_manager_.get();
  }

  [[nodiscard]] bool HasSampleSelection() const {
    return selection_report_ != nullptr && selection_report_->HasSamples();
  }

  void ListPresets();
  void RefreshCaptureView();
  void Disassemble(uint32_t pid, const orbit_client_data::FunctionInfo& function) override;
  void ShowSourceCode(const orbit_client_data::FunctionInfo& function) override;

  void OnCaptureStarted(const orbit_grpc_protos::CaptureStarted& capture_started,
                        std::optional<std::filesystem::path> file_path,
                        absl::flat_hash_set<uint64_t> frame_track_function_ids) override;
  void OnCaptureFinished(const orbit_grpc_protos::CaptureFinished& capture_finished) override;
  void OnTimer(const orbit_client_protos::TimerInfo& timer_info) override;
  void OnKeyAndString(uint64_t key, std::string str) override;

  void OnModuleUpdate(uint64_t timestamp_ns, orbit_grpc_protos::ModuleInfo module_info) override;
  void OnModulesSnapshot(uint64_t timestamp_ns,
                         std::vector<orbit_grpc_protos::ModuleInfo> module_infos) override;
  void OnPresentEvent(const orbit_grpc_protos::PresentEvent& present_event) override;
  void OnApiStringEvent(const orbit_client_data::ApiStringEvent& api_string_event) override;
  void OnApiTrackValue(const orbit_client_data::ApiTrackValue& api_track_value) override;
  void OnWarningEvent(orbit_grpc_protos::WarningEvent warning_event) override;
  void OnClockResolutionEvent(
      orbit_grpc_protos::ClockResolutionEvent clock_resolution_event) override;
  void OnErrorsWithPerfEventOpenEvent(
      orbit_grpc_protos::ErrorsWithPerfEventOpenEvent errors_with_perf_event_open_event) override;
  void OnWarningInstrumentingWithUprobesEvent(
      orbit_grpc_protos::WarningInstrumentingWithUprobesEvent
          warning_instrumenting_with_uprobes_event) override;
  void OnErrorEnablingOrbitApiEvent(
      orbit_grpc_protos::ErrorEnablingOrbitApiEvent error_enabling_orbit_api_event) override;
  void OnErrorEnablingUserSpaceInstrumentationEvent(
      orbit_grpc_protos::ErrorEnablingUserSpaceInstrumentationEvent error_event) override;
  void OnWarningInstrumentingWithUserSpaceInstrumentationEvent(
      orbit_grpc_protos::WarningInstrumentingWithUserSpaceInstrumentationEvent warning_event)
      override;
  void OnLostPerfRecordsEvent(
      orbit_grpc_protos::LostPerfRecordsEvent lost_perf_records_event) override;
  void OnOutOfOrderEventsDiscardedEvent(orbit_grpc_protos::OutOfOrderEventsDiscardedEvent
                                            out_of_order_events_discarded_event) override;

  void OnValidateFramePointers(
      std::vector<const orbit_client_data::ModuleData*> modules_to_validate) override;

  void SetCaptureWindow(CaptureWindow* capture);
  [[nodiscard]] const TimeGraph* GetTimeGraph() const {
    ORBIT_CHECK(capture_window_ != nullptr);
    return capture_window_->GetTimeGraph();
  }
  [[nodiscard]] TimeGraph* GetMutableTimeGraph() {
    ORBIT_CHECK(capture_window_ != nullptr);
    return capture_window_->GetTimeGraph();
  }
  void SetDebugCanvas(GlCanvas* debug_canvas);
  void SetIntrospectionWindow(IntrospectionWindow* canvas);
  void StopIntrospection();

  void SetSamplingReport(
      const orbit_client_data::CallstackData* callstack_data,
      const orbit_client_data::PostProcessedSamplingData* post_processed_sampling_data);
  void ClearSamplingReport();
  void SetSelectionReport(
      const orbit_client_data::CallstackData* selection_callstack_data,
      const orbit_client_data::PostProcessedSamplingData* selection_post_processed_sampling_data,
      bool has_summary);
  void ClearSelectionReport();
  void SetTopDownView(const orbit_client_data::CaptureData& capture_data);
  void ClearTopDownView();
  void SetSelectionTopDownView(
      const orbit_client_data::PostProcessedSamplingData& selection_post_processed_data,
      const orbit_client_data::CaptureData& capture_data);
  void ClearSelectionTopDownView();

  void SetBottomUpView(const orbit_client_data::CaptureData& capture_data);
  void ClearBottomUpView();
  void SetSelectionBottomUpView(
      const orbit_client_data::PostProcessedSamplingData& selection_post_processed_data,
      const orbit_client_data::CaptureData& capture_data);
  void ClearSelectionBottomUpView();

  // This needs to be called from the main thread.
  [[nodiscard]] bool IsCaptureConnected(
      const orbit_client_data::CaptureData& capture) const override;

  [[nodiscard]] bool IsDevMode() const;

  // Callbacks
  using CaptureStartedCallback = std::function<void(const std::optional<std::filesystem::path>&)>;
  void SetCaptureStartedCallback(CaptureStartedCallback callback) {
    capture_started_callback_ = std::move(callback);
  }
  using CaptureStopRequestedCallback = std::function<void()>;
  void SetCaptureStopRequestedCallback(CaptureStopRequestedCallback callback) {
    capture_stop_requested_callback_ = std::move(callback);
  }
  using CaptureStoppedCallback = std::function<void()>;
  void SetCaptureStoppedCallback(CaptureStoppedCallback callback) {
    capture_stopped_callback_ = std::move(callback);
  }
  using CaptureFailedCallback = std::function<void()>;
  void SetCaptureFailedCallback(CaptureFailedCallback callback) {
    capture_failed_callback_ = std::move(callback);
  }

  using CaptureClearedCallback = std::function<void()>;
  void SetCaptureClearedCallback(CaptureClearedCallback callback) {
    capture_cleared_callback_ = std::move(callback);
  }

  using SelectLiveTabCallback = std::function<void()>;
  void SetSelectLiveTabCallback(SelectLiveTabCallback callback) {
    select_live_tab_callback_ = std::move(callback);
  }
  using ErrorMessageCallback = std::function<void(const std::string&, const std::string&)>;
  void SetErrorMessageCallback(ErrorMessageCallback callback) {
    error_message_callback_ = std::move(callback);
  }
  using WarningMessageCallback = std::function<void(const std::string&, const std::string&)>;
  void SetWarningMessageCallback(WarningMessageCallback callback) {
    warning_message_callback_ = std::move(callback);
  }
  using InfoMessageCallback = std::function<void(const std::string&, const std::string&)>;
  void SetInfoMessageCallback(InfoMessageCallback callback) {
    info_message_callback_ = std::move(callback);
  }
  using RefreshCallback = std::function<void(orbit_data_views::DataViewType type)>;
  void SetRefreshCallback(RefreshCallback callback) { refresh_callback_ = std::move(callback); }

  using SamplingReportCallback =
      std::function<void(orbit_data_views::DataView*, const std::shared_ptr<SamplingReport>&)>;
  void SetSamplingReportCallback(SamplingReportCallback callback) {
    sampling_reports_callback_ = std::move(callback);
  }
  void SetSelectionReportCallback(SamplingReportCallback callback) {
    selection_report_callback_ = std::move(callback);
  }

  using CallTreeViewCallback = std::function<void(std::unique_ptr<CallTreeView>)>;
  void SetTopDownViewCallback(CallTreeViewCallback callback) {
    top_down_view_callback_ = std::move(callback);
  }
  void SetSelectionTopDownViewCallback(CallTreeViewCallback callback) {
    selection_top_down_view_callback_ = std::move(callback);
  }
  void SetBottomUpViewCallback(CallTreeViewCallback callback) {
    bottom_up_view_callback_ = std::move(callback);
  }
  void SetSelectionBottomUpViewCallback(CallTreeViewCallback callback) {
    selection_bottom_up_view_callback_ = std::move(callback);
  }
  using TimerSelectedCallback = std::function<void(const orbit_client_protos::TimerInfo*)>;
  void SetTimerSelectedCallback(TimerSelectedCallback callback) {
    timer_selected_callback_ = std::move(callback);
  }

  using SaveFileCallback = std::function<std::string(const std::string& extension)>;
  void SetSaveFileCallback(SaveFileCallback callback) { save_file_callback_ = std::move(callback); }
  void FireRefreshCallbacks(
      orbit_data_views::DataViewType type = orbit_data_views::DataViewType::kAll);
  void Refresh(orbit_data_views::DataViewType type = orbit_data_views::DataViewType::kAll) {
    FireRefreshCallbacks(type);
  }
  using ClipboardCallback = std::function<void(const std::string&)>;
  void SetClipboardCallback(ClipboardCallback callback) {
    clipboard_callback_ = std::move(callback);
  }

  void SendDisassemblyToUi(const orbit_client_data::FunctionInfo& function_info,
                           std::string disassembly, orbit_code_report::DisassemblyReport report);
  void SendTooltipToUi(const std::string& tooltip);
  void SendInfoToUi(const std::string& title, const std::string& text);
  void SendWarningToUi(const std::string& title, const std::string& text);
  void SendErrorToUi(const std::string& title, const std::string& text) override;
  void SendErrorToUi(const std::string& title, const std::string& text,
                     orbit_metrics_uploader::ScopedMetric metric);
  void RenderImGuiDebugUI();

  // RetrieveModule retrieves a module file and returns the local file path (potentially from the
  // local cache). Only modules with a .symtab section will be considered.
  orbit_base::Future<ErrorMessageOr<orbit_base::CanceledOr<std::filesystem::path>>> RetrieveModule(
      const orbit_symbol_provider::ModuleIdentifier& module_id);
  orbit_base::Future<void> LoadSymbolsManually(
      absl::Span<const orbit_client_data::ModuleData* const> modules) override;

  enum class SymbolLoadingAndErrorHandlingResult {
    kSymbolsLoadedSuccessfully,
    kCanceled,
  };
  // RetrieveModuleAndLoadSymbolsAndHandleError attempts to retrieve the module and loads the
  // symbols via RetrieveModuleAndLoadSymbols and when that fails handles the error with
  // symbol_loading_error_callback_. This might result in another loading attempt (another call to
  // RetrieveModuleAndLoadSymbolsAndHandleError).
  orbit_base::Future<SymbolLoadingAndErrorHandlingResult>
  RetrieveModuleAndLoadSymbolsAndHandleError(const orbit_client_data::ModuleData* module);

  // RetrieveModuleAndSymbols is a helper function which first retrieves the module by calling
  // `RetrieveModule` and afterwards load the symbols by calling `LoadSymbols`.
  orbit_base::Future<ErrorMessageOr<orbit_base::CanceledOr<void>>> RetrieveModuleAndLoadSymbols(
      const orbit_client_data::ModuleData* module);
  orbit_base::Future<ErrorMessageOr<orbit_base::CanceledOr<void>>> RetrieveModuleAndLoadSymbols(
      const orbit_symbol_provider::ModuleIdentifier& module_id);

  orbit_base::Future<ErrorMessageOr<orbit_base::CanceledOr<void>>>
  OrbitApp::RetrieveAndLoadPlatformApiInfo(
      const orbit_client_data::ModuleData* module);
  orbit_base::Future<ErrorMessageOr<orbit_grpc_protos::GetPlatformApiInfoResponse>>
  GetPlatformApiInfo();

  // This method is pretty similar to `RetrieveModule`, but it also requires debug information to be
  // present.
  orbit_base::Future<ErrorMessageOr<std::filesystem::path>> RetrieveModuleWithDebugInfo(
      const orbit_client_data::ModuleData* module);
  orbit_base::Future<ErrorMessageOr<std::filesystem::path>> RetrieveModuleWithDebugInfo(
      const orbit_symbol_provider::ModuleIdentifier& module_id);

  orbit_base::Future<ErrorMessageOr<void>> UpdateProcessAndModuleList() override;
  orbit_base::Future<std::vector<ErrorMessageOr<void>>> ReloadModules(
      absl::Span<const orbit_grpc_protos::ModuleInfo> module_infos);
  void RefreshUIAfterModuleReload();

  void UpdateAfterSymbolLoading();
  void UpdateAfterSymbolLoadingThrottled();
  void UpdateAfterCaptureCleared();

  // Load the functions and add frame tracks from a particular module of a preset file.
  orbit_base::Future<ErrorMessageOr<void>> LoadPresetModule(
      const std::filesystem::path& module_path, const orbit_preset_file::PresetFile& preset_file);
  orbit_base::Future<ErrorMessageOr<void>> LoadPreset(
      const orbit_preset_file::PresetFile& preset) override;
  [[nodiscard]] orbit_data_views::PresetLoadState GetPresetLoadState(
      const orbit_preset_file::PresetFile& preset) const override;
  void ShowPresetInExplorer(const orbit_preset_file::PresetFile& preset) override;
  void FilterTracks(const std::string& filter);

  void CrashOrbitService(orbit_grpc_protos::CrashOrbitServiceRequest_CrashType crash_type);

  void SetGrpcChannel(std::shared_ptr<grpc::Channel> grpc_channel) {
    ORBIT_CHECK(grpc_channel_ == nullptr);
    ORBIT_CHECK(grpc_channel != nullptr);
    grpc_channel_ = std::move(grpc_channel);
  }
  void SetProcessManager(orbit_client_services::ProcessManager* process_manager) {
    ORBIT_CHECK(process_manager_ == nullptr);
    ORBIT_CHECK(process_manager != nullptr);
    process_manager_ = process_manager;
  }
  void SetTargetProcess(orbit_client_data::ProcessData* process);
  [[nodiscard]] orbit_data_views::DataView* GetOrCreateDataView(
      orbit_data_views::DataViewType type) override;
  [[nodiscard]] orbit_data_views::DataView* GetOrCreateSelectionCallstackDataView();

  [[nodiscard]] orbit_string_manager::StringManager* GetStringManager() { return &string_manager_; }
  [[nodiscard]] orbit_client_services::ProcessManager* GetProcessManager() {
    return process_manager_;
  }
  [[nodiscard]] orbit_base::MainThreadExecutor* GetMainThreadExecutor() {
    return main_thread_executor_;
  }
  [[nodiscard]] orbit_client_data::ProcessData* GetMutableTargetProcess() const { return process_; }
  [[nodiscard]] const orbit_client_data::ProcessData* GetTargetProcess() const override {
    return process_;
  }
  [[nodiscard]] ManualInstrumentationManager* GetManualInstrumentationManager() {
    return manual_instrumentation_manager_.get();
  }
  [[nodiscard]] orbit_client_data::ModuleData* GetMutableModuleByModuleIdentifier(
      const orbit_symbol_provider::ModuleIdentifier& module_id) override {
    return module_manager_->GetMutableModuleByModuleIdentifier(module_id);
  }
  [[nodiscard]] const orbit_client_data::ModuleData* GetModuleByModuleIdentifier(
      const orbit_symbol_provider::ModuleIdentifier& module_id) const override {
    return module_manager_->GetModuleByModuleIdentifier(module_id);
  }
  [[nodiscard]] const orbit_client_data::ProcessData& GetConnectedOrLoadedProcess() const;

  void SetCollectSchedulerInfo(bool collect_scheduler_info);
  void SetCollectThreadStates(bool collect_thread_states);
  void SetTraceGpuSubmissions(bool trace_gpu_submissions);
  void SetEnableApi(bool enable_api);
  void SetEnableIntrospection(bool enable_introspection);
  void SetDynamicInstrumentationMethod(
      orbit_grpc_protos::CaptureOptions::DynamicInstrumentationMethod method);
  void SetWineSyscallHandlingMethod(orbit_client_data::WineSyscallHandlingMethod method);
  void SetSamplesPerSecond(double samples_per_second);
  void SetStackDumpSize(uint16_t stack_dump_size);
  void SetThreadStateChangeCallstackStackDumpSize(uint16_t stack_dump_size);
  void SetUnwindingMethod(orbit_grpc_protos::CaptureOptions::UnwindingMethod unwinding_method);
  void SetMaxLocalMarkerDepthPerCommandBuffer(uint64_t max_local_marker_depth_per_command_buffer);
  void SetEnableAutoFrameTrack(bool enable_auto_frame_track);
  void SetCollectMemoryInfo(bool collect_memory_info) {
    data_manager_->set_collect_memory_info(collect_memory_info);
  }
  void SetMemorySamplingPeriodMs(uint64_t memory_sampling_period_ms) {
    data_manager_->set_memory_sampling_period_ms(memory_sampling_period_ms);
  }
  [[nodiscard]] uint64_t GetMemorySamplingPeriodMs() const {
    return GetCaptureData().GetMemorySamplingPeriodNs() / 1'000'000;
  }
  void SetMemoryWarningThresholdKb(uint64_t memory_warning_threshold_kb) {
    data_manager_->set_memory_warning_threshold_kb(memory_warning_threshold_kb);
  }
  [[nodiscard]] uint64_t GetMemoryWarningThresholdKb() const {
    return GetCaptureData().memory_warning_threshold_kb();
  }
  [[nodiscard]] orbit_grpc_protos::CaptureOptions::ThreadStateChangeCallStackCollection
  GetThreadStateChangeCallstackCollection() const {
    return data_manager_->thread_state_change_callstack_collection();
  }
  void SetThreadStateChangeCallstackCollection(
      orbit_grpc_protos::CaptureOptions::ThreadStateChangeCallStackCollection
          thread_state_change_callstack_collection) {
    data_manager_->set_thread_state_change_callstack_collection(
        thread_state_change_callstack_collection);
  }

  // TODO(kuebler): Move them to a separate controller at some point
  void SelectFunction(const orbit_client_data::FunctionInfo& func) override;
  void DeselectFunction(const orbit_client_data::FunctionInfo& func) override;
  [[nodiscard]] bool IsFunctionSelected(const orbit_client_data::FunctionInfo& func) const override;
  [[nodiscard]] bool IsFunctionSelected(
      const orbit_client_data::SampledFunction& func) const override;
  [[nodiscard]] bool IsFunctionSelected(uint64_t absolute_address) const;
  [[nodiscard]] const orbit_grpc_protos::InstrumentedFunction* GetInstrumentedFunction(
      uint64_t function_id) const;

  void SetVisibleScopeIds(absl::flat_hash_set<ScopeId> visible_scope_ids) override;
  [[nodiscard]] bool IsScopeVisible(ScopeId scope_id) const;

  [[nodiscard]] std::optional<ScopeId> GetHighlightedScopeId() const override;
  void SetHighlightedScopeId(std::optional<ScopeId> highlighted_scope_id) override;

  [[nodiscard]] orbit_client_data::ThreadID selected_thread_id() const;
  void set_selected_thread_id(orbit_client_data::ThreadID thread_id);

  [[nodiscard]] std::optional<orbit_client_data::ThreadStateSliceInfo> selected_thread_state_slice()
      const;
  void set_selected_thread_state_slice(
      std::optional<orbit_client_data::ThreadStateSliceInfo> thread_state_slice);

  [[nodiscard]] std::optional<orbit_client_data::ThreadStateSliceInfo> hovered_thread_state_slice()
      const;
  void set_hovered_thread_state_slice(
      std::optional<orbit_client_data::ThreadStateSliceInfo> thread_state_slice);

  [[nodiscard]] const orbit_client_protos::TimerInfo* selected_timer() const;
  void SelectTimer(const orbit_client_protos::TimerInfo* timer_info);
  void DeselectTimer() override;

  [[nodiscard]] std::optional<ScopeId> GetScopeIdToHighlight() const;
  [[nodiscard]] uint64_t GetGroupIdToHighlight() const;

  // origin_is_multiple_threads defines if the selection is specific to a single thread,
  // or spans across multiple threads.
  void SelectCallstackEvents(
      const std::vector<orbit_client_data::CallstackEvent>& selected_callstack_events,
      bool origin_is_multiple_threads);
  void InspectCallstackEvents(
      const std::vector<orbit_client_data::CallstackEvent>& selected_callstack_events,
      bool origin_is_multiple_threads);
  void ClearInspection();

  void SelectTracepoint(const orbit_grpc_protos::TracepointInfo& info) override;
  void DeselectTracepoint(const orbit_grpc_protos::TracepointInfo& tracepoint) override;

  [[nodiscard]] bool IsTracepointSelected(
      const orbit_grpc_protos::TracepointInfo& info) const override;

  // Only enables the frame track in the capture settings (in DataManager) and does not
  // add a frame track to the current capture data.
  void EnableFrameTrack(const orbit_client_data::FunctionInfo& function) override;

  // Only disables the frame track in the capture settings (in DataManager) and does
  // not remove the frame track from the capture data.
  void DisableFrameTrack(const orbit_client_data::FunctionInfo& function) override;

  [[nodiscard]] bool IsFrameTrackEnabled(
      const orbit_client_data::FunctionInfo& function) const override;

  // Adds a frame track to the current capture data if the captures contains function calls to
  // the function and the function was instrumented.
  void AddFrameTrack(const orbit_client_data::FunctionInfo& function) override;

  // Removes the frame track (if it exists) from the capture data.
  void RemoveFrameTrack(const orbit_client_data::FunctionInfo& function) override;

  [[nodiscard]] bool HasFrameTrackInCaptureData(uint64_t instrumented_function_id) const override;

  void JumpToTimerAndZoom(ScopeId scope_id, JumpToTimerMode selection_mode) override;
  [[nodiscard]] std::vector<const orbit_client_data::TimerChain*> GetAllThreadTimerChains()
      const override;

  [[nodiscard]] const orbit_statistics::BinomialConfidenceIntervalEstimator&
  GetConfidenceIntervalEstimator() const override;

  void SetHistogramSelectionRange(std::optional<orbit_statistics::HistogramSelectionRange> range) {
    histogram_selection_range_ = range;
    RequestUpdatePrimitives();
  }

  [[nodiscard]] std::optional<orbit_statistics::HistogramSelectionRange>
  GetHistogramSelectionRange() const {
    return histogram_selection_range_;
  }

  [[nodiscard]] bool IsModuleDownloading(
      const orbit_client_data::ModuleData* module) const override;
  [[nodiscard]] orbit_data_views::SymbolLoadingState GetSymbolLoadingStateForModule(
      const orbit_client_data::ModuleData* module) const override;

  [[nodiscard]] bool IsSymbolLoadingInProgressForModule(
      const orbit_client_data::ModuleData* module) const override;
  void RequestSymbolDownloadStop(
      absl::Span<const orbit_client_data::ModuleData* const> modules) override;

  // Triggers symbol loading for all modules in ModuleManager that are not loaded yet. This is done
  // with a simple prioritization. The module `ggpvlk.so` is queued to be loaded first, the "main
  // module" (binary of the process) is queued to be loaded second. All other modules are queued in
  // no particular order.
  orbit_base::Future<std::vector<ErrorMessageOr<orbit_base::CanceledOr<void>>>> LoadAllSymbols();

  // Automatically add a default Frame Track. It will choose only one frame track from an internal
  // list of auto-loadable presets.
  void AddDefaultFrameTrackOrLogError();

 private:
  void InitializeProcessSpinningAtEntryPoint(uint32_t pid);
  void UpdateModulesAbortCaptureIfModuleWithoutBuildIdNeedsReload(
      absl::Span<const orbit_grpc_protos::ModuleInfo> module_infos);
  void AddSymbols(const orbit_symbol_provider::ModuleIdentifier& module_id,
                  const orbit_grpc_protos::ModuleSymbols& module_symbols);
  ErrorMessageOr<std::vector<const orbit_client_data::ModuleData*>> GetLoadedModulesByPath(
      const std::filesystem::path& module_path);
  ErrorMessageOr<void> ConvertPresetToNewFormatIfNecessary(
      const orbit_preset_file::PresetFile& preset_file);

  [[nodiscard]] orbit_base::Future<ErrorMessageOr<orbit_base::CanceledOr<std::filesystem::path>>>
  RetrieveModuleFromRemote(const std::string& module_file_path, orbit_base::StopToken stop_token);

  void SelectFunctionsFromHashes(const orbit_client_data::ModuleData* module,
                                 absl::Span<const uint64_t> function_hashes);
  void SelectFunctionsByName(const orbit_client_data::ModuleData* module,
                             absl::Span<const std::string> function_names);

  orbit_base::Future<ErrorMessageOr<std::filesystem::path>> FindModuleLocally(
      const orbit_client_data::ModuleData* module_data);
  [[nodiscard]] orbit_base::Future<ErrorMessageOr<void>> LoadSymbols(
      const std::filesystem::path& symbols_path,
      const orbit_symbol_provider::ModuleIdentifier& module_id);

  static ErrorMessageOr<orbit_preset_file::PresetFile> ReadPresetFromFile(
      const std::filesystem::path& filename);

  ErrorMessageOr<void> SavePreset(const std::string& filename);

  void AddFrameTrack(uint64_t instrumented_function_id);
  void RemoveFrameTrack(uint64_t instrumented_function_id);
  void EnableFrameTracksFromHashes(const orbit_client_data::ModuleData* module,
                                   absl::Span<const uint64_t> function_hashes);
  void EnableFrameTracksByName(const orbit_client_data::ModuleData* module,
                               absl::Span<const std::string> function_names);
  void AddFrameTrackTimers(uint64_t instrumented_function_id);
  void RefreshFrameTracks();
  void TrySaveUserDefinedCaptureInfo();

  orbit_base::Future<void> OnCaptureFailed(ErrorMessage error_message);
  orbit_base::Future<void> OnCaptureCancelled();
  orbit_base::Future<void> OnCaptureComplete();

  void RequestUpdatePrimitives();

  // Only call from the capture thread
  void CaptureMetricProcessTimer(const orbit_client_protos::TimerInfo& timer);

  void ShowHistogram(const std::vector<uint64_t>* data, const std::string& scope_name,
                     std::optional<ScopeId> scope_id) override;

  void RequestSymbolDownloadStop(absl::Span<const orbit_client_data::ModuleData* const> modules,
                                 bool show_dialog);
  // Sets CaptureData's selection_callstack_data and selection_post_processed_sampling_data.
  void SetCaptureDataSelectionFields(
      const std::vector<orbit_client_data::CallstackEvent>& selected_callstack_events,
      bool origin_is_multiple_threads);

  // TODO(b/243520787) The following is temporary. The SymbolProvider related logic SHOULD be moved
  // to the ProxySymbolProvider as planned in our symbol refactoring discussion in b/243520787.
  void InitRemoteSymbolProviders();
  // TODO(b/243520787) Rename existing `RetrieveModuleFromRemote` as `RetrieveModuleFromInstance`
  // and rename this `RetrieveModuleViaDownload` to `RetrieveModuleFromRemote`.
  [[nodiscard]] orbit_base::Future<ErrorMessageOr<orbit_base::CanceledOr<std::filesystem::path>>>
  RetrieveModuleViaDownload(const orbit_symbol_provider::ModuleIdentifier& module_id);

  std::atomic<bool> capture_loading_cancellation_requested_ = false;
  std::atomic<orbit_client_data::CaptureData::DataSource> data_source_{
      orbit_client_data::CaptureData::DataSource::kLiveCapture};

  CaptureStartedCallback capture_started_callback_;
  CaptureStopRequestedCallback capture_stop_requested_callback_;
  CaptureStoppedCallback capture_stopped_callback_;
  CaptureFailedCallback capture_failed_callback_;
  CaptureClearedCallback capture_cleared_callback_;
  SelectLiveTabCallback select_live_tab_callback_;
  ErrorMessageCallback error_message_callback_;
  WarningMessageCallback warning_message_callback_;
  InfoMessageCallback info_message_callback_;
  RefreshCallback refresh_callback_;
  SamplingReportCallback sampling_reports_callback_;
  SamplingReportCallback selection_report_callback_;
  CallTreeViewCallback top_down_view_callback_;
  CallTreeViewCallback selection_top_down_view_callback_;
  CallTreeViewCallback bottom_up_view_callback_;
  CallTreeViewCallback selection_bottom_up_view_callback_;
  SaveFileCallback save_file_callback_;
  ClipboardCallback clipboard_callback_;
  TimerSelectedCallback timer_selected_callback_;

  std::vector<orbit_data_views::DataView*> panels_;

  std::unique_ptr<orbit_data_views::ModulesDataView> modules_data_view_;
  std::unique_ptr<orbit_data_views::FunctionsDataView> functions_data_view_;
  std::unique_ptr<orbit_data_views::CallstackDataView> callstack_data_view_;
  std::unique_ptr<orbit_data_views::CallstackDataView> selection_callstack_data_view_;
  std::unique_ptr<orbit_data_views::PresetsDataView> presets_data_view_;
  std::unique_ptr<orbit_data_views::TracepointsDataView> tracepoints_data_view_;

  CaptureWindow* capture_window_ = nullptr;
  IntrospectionWindow* introspection_window_ = nullptr;
  GlCanvas* debug_canvas_ = nullptr;

  std::shared_ptr<SamplingReport> sampling_report_;
  std::shared_ptr<SamplingReport> selection_report_ = nullptr;

  struct ModuleDownloadOperation {
    orbit_base::StopSource stop_source;
    orbit_base::Future<ErrorMessageOr<orbit_base::CanceledOr<std::filesystem::path>>> future;
  };
  // Map of module file path to download operation future, that holds all symbol downloads that
  // are currently in progress.
  // ONLY access this from the main thread.
  absl::flat_hash_map<std::string, ModuleDownloadOperation> symbol_files_currently_downloading_;

  // Map of "module ID" (file path and build ID) to symbol file retrieving future, that holds all
  // symbol retrieving operations currently in progress. (Retrieving here means finding locally or
  // downloading from the instance). Since downloading a symbols file can be part of the retrieval,
  // if a module ID is contained in symbol_files_currently_downloading_, it is also contained in
  // symbol_files_currently_being_retrieved_.
  // ONLY access this from the main thread.
  absl::flat_hash_map<
      orbit_symbol_provider::ModuleIdentifier,
      orbit_base::Future<ErrorMessageOr<orbit_base::CanceledOr<std::filesystem::path>>>>
      symbol_files_currently_being_retrieved_;

  // Map of "module ID" (file path and build ID) to symbol loading future, that holds all symbol
  // loading operations that are currently in progress. Since retrieving and downloading a file can
  // be part of the overall symbol loading process, if a module ID is contained in
  // symbol_files_currently_being_retrieved_ or symbol_files_currently_downloading_, it is also
  // contained in symbols_currently_loading_.
  // ONLY access this from the main thread.
  absl::flat_hash_map<orbit_symbol_provider::ModuleIdentifier,
                      orbit_base::Future<ErrorMessageOr<orbit_base::CanceledOr<void>>>>
      symbols_currently_loading_;

  // Set of modules where a symbol loading error has occurred. The module identifier consists of
  // file path and build ID.
  // ONLY access this from the main thread.
  absl::flat_hash_set<orbit_symbol_provider::ModuleIdentifier> modules_with_symbol_loading_error_;

  // Set of modules for which the download is disabled.
  // ONLY access this from the main thread.
  absl::flat_hash_set<std::string> download_disabled_modules_;

  // A boolean information about if the default Frame Track was added in the current session.
  bool default_frame_track_was_added_ = false;

  orbit_string_manager::StringManager string_manager_;
  std::shared_ptr<grpc::Channel> grpc_channel_;

  orbit_gl::MainWindowInterface* main_window_ = nullptr;
  orbit_base::MainThreadExecutor* main_thread_executor_;
  std::thread::id main_thread_id_;
  std::shared_ptr<orbit_base::ThreadPool> thread_pool_;
  std::unique_ptr<orbit_capture_client::CaptureClient> capture_client_;
  orbit_client_services::ProcessManager* process_manager_ = nullptr;
  std::unique_ptr<orbit_client_data::ModuleManager> module_manager_;
  std::unique_ptr<orbit_client_data::DataManager> data_manager_;
  std::unique_ptr<orbit_client_services::CrashManager> crash_manager_;
  std::unique_ptr<ManualInstrumentationManager> manual_instrumentation_manager_;

  const orbit_symbols::SymbolHelper symbol_helper_{orbit_paths::CreateOrGetCacheDirUnsafe()};

  orbit_client_data::ProcessData* process_ = nullptr;

  std::unique_ptr<FramePointerValidatorClient> frame_pointer_validator_client_;

  orbit_gl::FrameTrackOnlineProcessor frame_track_online_processor_;

  const orbit_base::CrashHandler* crash_handler_;
  orbit_metrics_uploader::MetricsUploader* metrics_uploader_;
  // TODO(b/166767590) Synchronize. Probably in the same way as capture_data
  // Created by the main thread right before a capture is started. While capturing its modified by
  // the capture thread. After the capture is finished its read by the main thread again. This is
  // similar to how capture_data is used.
  orbit_metrics_uploader::CaptureCompleteData metrics_capture_complete_data_;

  orbit_capture_file_info::Manager capture_file_info_manager_{};

  const orbit_statistics::WilsonBinomialConfidenceIntervalEstimator confidence_interval_estimator_;

  std::optional<orbit_statistics::HistogramSelectionRange> histogram_selection_range_;

  static constexpr std::chrono::milliseconds kMaxPostProcessingInterval{1000};
  orbit_qt_utils::Throttle update_after_symbol_loading_throttle_{kMaxPostProcessingInterval};

  // TODO(b/243520787) The following is temporary. The SymbolProvider related logic SHOULD be moved
  // to the ProxySymbolProvider as planned in our symbol refactoring discussion in b/243520787.
  std::optional<orbit_http::HttpDownloadManager> download_manager_;
  std::unique_ptr<orbit_ggp::Client> ggp_client_;
  std::optional<orbit_remote_symbol_provider::StadiaSymbolStoreSymbolProvider>
      stadia_symbol_provider_ = std::nullopt;
  std::optional<orbit_remote_symbol_provider::MicrosoftSymbolServerSymbolProvider>
      microsoft_symbol_provider_ = std::nullopt;
};

#endif  // ORBIT_GL_APP_H_
