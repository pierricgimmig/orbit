// Copyright (c) 2020 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_LIB_ORBIT_LIB_H_
#define ORBIT_LIB_ORBIT_LIB_H_

#include <stdint.h>

#ifdef ORBIT_LIB_API_EXPORTS
#define ORBIT_LIB_API __declspec(dllexport)
#else
#define ORBIT_LIB_API __declspec(dllimport)
#endif

namespace orbit_lib {

    struct ProcessListener {
        void (*on_process)(const char* process_path, uint32_t pid, bool is_64_bit, float cpu_usage);
        void (*on_error)(const char* error_message);
    };

    struct ModuleListener {
        void (*on_module)(const char* module_path, uint64_t start_address, uint64_t end_address,
            uint64_t debug_info_size);
        void (*on_error)(const char* error_message);
    };

    struct DebugInfoListener {
        void (*on_function)(const char* module_path, const char* function_name, uint64_t relative_address,
            const char* file_name, int line);
        void (*on_error)(const char* error_message);
    };

    struct CaptureListener {
        void (*on_timer)(uint64_t absolute_address, uint64_t start, uint64_t end, uint32_t tid,
            uint32_t pid);
        void (*on_error)(const char* error_message);
    };

    ORBIT_LIB_API void Initialize();

    ORBIT_LIB_API int ListProcesses(const ProcessListener& listener);
    ORBIT_LIB_API int ListModules(uint32_t pid, const ModuleListener& listener);
    ORBIT_LIB_API int ListFunctions(const char* symbols_path, const DebugInfoListener& listener);

    ORBIT_LIB_API int StartCapture(uint32_t pid, uint64_t* addresses, size_t num_addresses,
        const CaptureListener& listener);
    ORBIT_LIB_API int StopCapture();

}  // namespace orbit_lib

#endif  // ORBIT_LIB_ORBIT_LIB_H_