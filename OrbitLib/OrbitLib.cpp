#include "OrbitLib.h"

#include <chrono>
#include <thread>

#include "windows.h"
#include "Pdb.h"
#include "ProcessUtils.h"
#include "Capture.h"
#include "CoreApp.h"
#include "OrbitModule.h"

#pragma optimize("", off)

namespace orbit_lib{ 

void Initialize() {
    static CoreApp app;
    GCoreApp = &app;
    InitProfiling();
    Capture::Init();
    oqpi_tk::start_default_scheduler();
}

int ListProcesses(ProcessListener* listener) {
    if (listener == nullptr) return -1;

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
    ProcessList process_list;
    std::shared_ptr<Process> process = process_list.m_ProcessesMap[pid];
    if(process == nullptr) {
        listener->OnError("Error in ListModules:Process not found");
        return -1;
    }

    process->Init();
    process->ListModules();
    for (auto [base_address, module] : process->GetModules()) {
        std::string module_name = ws2s(module->m_FullName);
        listener->OnModule(module_name.c_str(), module->m_AddressStart, module->m_AddressEnd, module->m_PdbSize);
    }

    return 0;
}

int ListFunctions(const char* symbols_path, DebugInfoListener* listener) {
    if (listener == nullptr) return -1;

    std::wstring wide_symbols_path = s2ws(symbols_path);
    auto pdb = std::make_shared<Pdb>(wide_symbols_path.c_str());
    GPdbDbg = pdb;
    pdb->LoadDataFromPdb();
    pdb->LoadPdb(wide_symbols_path.c_str());

    for (Function& function : pdb->GetFunctions()) {
        std::string module_name = ws2s(function.GetModuleName());
        std::string function_name = ws2s(function.m_PrettyName);
        std::string file_name = ws2s(function.m_File);
		listener->OnFunction(module_name.c_str(), function_name.c_str(), function.m_Address, file_name.c_str(), function.m_Line);
    }

    GPdbDbg = nullptr;
    return 0;
}

int StartCapture(uint32_t pid, uint64_t* addresses, size_t num_addresses, const CaptureListener& listener) {
    return 0;
}

int StopCapture() {
    return 0;
}

}