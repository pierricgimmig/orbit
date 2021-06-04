// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "OrbitLib.h"

#include <windows.h>
#include <debugapi.h>
#include <iostream>
#include <string>
#include <vector>

#define PRINT_VAR(x) do { std::cout << #x << " = " << x << std::endl; } while(0)

struct TestProcessListener : public orbit_lib::ProcessListener {
	void OnError(const char* message) override {
		OutputDebugStringA(message);
	}

	void OnProcess(const char* process_path, uint32_t pid, bool is_64_bit, float cpu_usage) override {
		PRINT_VAR(process_path);
		PRINT_VAR(pid);
		PRINT_VAR(is_64_bit);
		PRINT_VAR(cpu_usage);
		pids.push_back(pid);
	}

	std::vector<uint32_t> pids;
};

struct TestModuleListener : public orbit_lib::ModuleListener {
	void OnError(const char* message) override {
		OutputDebugStringA(message);
	}

	void OnModule(const char* module_path, uint64_t start_address, uint64_t end_address, uint64_t debug_info_size) override {
		PRINT_VAR(module_path);
		PRINT_VAR(start_address);
		PRINT_VAR(end_address);
		PRINT_VAR(debug_info_size);
	}
};

struct TestDebugInfoListener : public orbit_lib::DebugInfoListener {
	void OnError(const char* message) override {
		OutputDebugStringA(message);
	}

	void OnFunction(const char* module_path, const char* function_name, uint64_t relative_address, uint64_t size, const char* file_name, int line) {
		PRINT_VAR(module_path);
		PRINT_VAR(function_name);
		PRINT_VAR(relative_address);
    PRINT_VAR(size);
		PRINT_VAR(file_name);
		PRINT_VAR(line);
	}
};

std::vector<uint32_t> pids;

int main(int argc, char* argv[]) {
	// Processes
	TestProcessListener process_listener;
	orbit_lib::ListProcesses(&process_listener);

	// Modules
	TestModuleListener module_listener;
	for (uint32_t pid : pids) {
		orbit_lib::ListModules(pid, &module_listener);
	}

	// Functions
	TestDebugInfoListener debug_info_listener;

	std::string pdb_name = "C:/git/orbitprofiler/build/OrbitLib/RelWithDebInfo/OrbitLib.pdb";
	orbit_lib::ListFunctions(pdb_name.c_str(), &debug_info_listener);

}
