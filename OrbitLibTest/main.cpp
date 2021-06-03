// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "OrbitLibTest.h"
#include "OrbitLib.h"

#include <windows.h>
#include <debugapi.h>
#include <iostream>
#include <string>
#include <vector>

#define PRINT_VAR(x) do { std::cout << #x << " = " << x << std::endl; } while(0)

void OutputError(const char* message) {
	OutputDebugStringA(message);
}

std::vector<uint32_t> pids;

int main(int argc, char* argv[]) {

	orbit_lib::Initialize();

	// Processes
	orbit_lib::ProcessListener process_listener;
	process_listener.on_error = &OutputError;
	process_listener.on_process = [](const char* process_path, uint32_t pid, bool is_64_bit, float cpu_usage) {
		PRINT_VAR(process_path);
		PRINT_VAR(pid);
		PRINT_VAR(is_64_bit);
		PRINT_VAR(cpu_usage);
		pids.push_back(pid);
	};
	orbit_lib::ListProcesses(process_listener);

	// Modules
	orbit_lib::ModuleListener module_listener;
	module_listener.on_error = &OutputError;
	module_listener.on_module = [](const char* module_path, uint64_t start_address, uint64_t end_address, uint64_t debug_info_size) {
		PRINT_VAR(module_path);
		PRINT_VAR(start_address);
		PRINT_VAR(end_address);
		PRINT_VAR(debug_info_size);
	};

	for (uint32_t pid : pids) {
		orbit_lib::ListModules(pid, module_listener);
	}

	// Functions
	orbit_lib::DebugInfoListener debug_info_listener;
	debug_info_listener.on_error = [](const char* message) {OutputDebugStringA(message);  };
	debug_info_listener.on_function = [](const char* module_path, const char* function_name, uint64_t relative_address, const char* file_name, int line) {
		PRINT_VAR(module_path);
		PRINT_VAR(function_name);
		PRINT_VAR(relative_address);
		PRINT_VAR(file_name);
		PRINT_VAR(line);
	};

	std::string pdb_name = "C:/git/orbitprofiler/build/OrbitLib/RelWithDebInfo/OrbitLib.pdb";
	orbit_lib::ListFunctions(pdb_name.c_str(), debug_info_listener);

}