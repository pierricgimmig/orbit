#include "OrbitLib.h"

#include <atomic>
#include <chrono>
#include <filesystem>
#include <thread>

#include "windows.h"
#include "Pdb.h"
#include "ProcessUtils.h"
#include "Capture.h"
#include "CoreApp.h"
#include "OrbitModule.h"
#include "TimerManager.h"
#include "TcpServer.h"

std::atomic<bool> g_is_capturing = false;

namespace orbit_lib{ 

void LazyInit() {
    static bool init = false;
    if (init) return;

    static CoreApp app;
    GCoreApp = &app;
    InitProfiling();
    Capture::Init();
    oqpi_tk::start_default_scheduler();
    GTimerManager = new TimerManager();
    GTcpServer = new TcpServer();
    GTcpServer->Start(Capture::GCapturePort);
    init = true;
}

int ListProcesses(ProcessListener* listener) {
    if (listener == nullptr) return -1;
    LazyInit();

    ProcessList process_list;
    process_list.UpdateCpuTimes();
    std::this_thread::sleep_for(std::chrono::milliseconds(10));
    process_list.UpdateCpuTimes();
    process_list.SortByCPU();
    
    for (std::shared_ptr<Process> process : process_list.m_Processes) {
        std::string process_name = ws2s(process->GetFullName());
        uint32_t pid = process->GetID();
        bool is_64_bit = process->GetIs64Bit();
        float cpu_usage = process->GetCpuUsage();
        listener->OnProcess(process_name.c_str(), pid, is_64_bit, cpu_usage);
    }

    return 0;
}

int ListModules(uint32_t pid, ModuleListener* listener) {
    if (listener == nullptr) return -1;
    LazyInit();

    ProcessList process_list;
    std::shared_ptr<Process> process = process_list.m_ProcessesMap[pid];
    if(process == nullptr) {
        listener->OnError("Error listing modules: Process not found");
        return -1;
    }

    process->Init();
    process->ListModules();
    for (auto [base_address, module] : process->GetModules()) {
        std::string module_name = ws2s(module->m_FullName);
        uint64_t size = 0;
        if (std::filesystem::exists(module_name)) {
            size = std::filesystem::file_size(module_name);
        }
        listener->OnModule(module_name.c_str(), module->m_AddressStart, module->m_AddressEnd, size);
    }

    return 0;
}

int ListFunctions(const char* symbols_path, DebugInfoListener* listener) {
    if (listener == nullptr) return -1;
    LazyInit();

    std::wstring wide_symbols_path = s2ws(symbols_path);
    auto pdb = std::make_shared<Pdb>(wide_symbols_path.c_str());
    GPdbDbg = pdb;
    pdb->LoadFunctions();

    for (Function& function : pdb->GetFunctions()) {
        std::string module_name = ws2s(function.GetModuleName());
        std::string function_name = ws2s(function.m_PrettyName);
        std::string file_name = ws2s(function.m_File);
        uint64_t function_size = function.m_Size;
        listener->OnFunction(module_name.c_str(), function_name.c_str(), function.m_Address,
                             function_size, file_name.c_str(), function.m_Line);
    }

    GPdbDbg = nullptr;
    return 0;
}

int StartCapture(uint32_t pid, FunctionHook* function_hooks, size_t num_hooks,
                 CaptureListener* listener) {
  ProcessList process_list;
  std::shared_ptr<Process> target_process = process_list.m_ProcessesMap[pid];
  if (target_process == nullptr) {
    listener->OnError("Error starting capture, could not find process");
    return -1;
  }

  GTimerManager->m_TimerAddedCallbacks.push_back(
      [pid, listener](Timer& timer) { 
         if (!g_is_capturing) return;
      // TODO-PG: we have the callstack on instrumented functions, we should use it.
      listener->OnTimer(timer.m_FunctionAddress, timer.m_Start, timer.m_End, timer.m_TID, pid);
      });

  std::vector<FunctionHook> hooks;
  hooks.assign(function_hooks, function_hooks + num_hooks);
  g_is_capturing = true;

  std::unordered_map < FunctionHook::Type, std::vector<uint64_t>> addresses_by_type;
  for (int i = 0; i < num_hooks; ++i) {
    FunctionHook& hook = function_hooks[i];
    addresses_by_type[hook.type].push_back(hook.address);
  }
  
  // Start capturing, then enable hooks.
  Capture::StartCapture(target_process);
  
  // Hook functions.
  for (auto& [type, addresses] : addresses_by_type) {
      switch (type) { 
      case FunctionHook::Type::kRegular:
          GTcpServer->Send(Msg_FunctionHook, addresses);
          break;
       case FunctionHook::Type::kFileIO:
        GTcpServer->Send(Msg_FunctionHookFileIo, addresses);
        break;
       case FunctionHook::Type::kInvalid:
         listener->OnError("Not hooking functions for unsupported type.");
         break;
    }
  }

  return 0;
}

int StopCapture() {
  g_is_capturing = false;
  Capture::StopCapture();
  GTimerManager->m_TimerAddedCallbacks.clear();
  return 0;
}

}