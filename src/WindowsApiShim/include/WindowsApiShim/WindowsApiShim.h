// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

extern "C" {

__declspec(dllexport) bool __cdecl OrbitHookApi(const char* function_name, const char* module,
                                                const char* name_space);

}