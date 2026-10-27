// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#pragma once

#include "CoreMinimal.h"
#include "Modules/ModuleInterface.h"

// The perfr manual-instrumentation API, for any module that depends on
// "PerfrProfiler": PERFR_SCOPE("name"), PERFR_VALUE("fps", 59.9),
// perfr_start_async / perfr_stop across threads, perfr_link, perfr_span for
// timestamps read back later (GPU work). One header, nothing to link; every
// call is a single predictable branch until a perfr is capturing
// this process. See perfr.h for the full API and where the library is
// looked for.
#if PLATFORM_WINDOWS
#include "Windows/AllowWindowsPlatformTypes.h"
#endif
#include "perfr.h"
#if PLATFORM_WINDOWS
#include "Windows/HideWindowsPlatformTypes.h"
#endif

/**
 * Loads libperfr_api once at startup and registers "perfr" as an external
 * profiler, so that every SCOPED_NAMED_EVENT*, SCOPE_CYCLE_COUNTER (with
 * -statnamedevents), QUICK_SCOPE_CYCLE_COUNTER and engine named event reaches
 * perfr as a scope on its thread, with a "frame" tick per game-thread frame.
 *
 * Select it as Unreal selects any external profiler: `-perfr` on the
 * command line, `UE_EXTERNAL_PROFILER=perfr` in the environment, or
 * `[Core.ProfilingDebugging] ExternalProfiler=perfr` in DefaultEngine.ini.
 */
class FPerfrProfilerModule : public IModuleInterface
{
public:
	virtual void StartupModule() override;
	virtual void ShutdownModule() override;

	/** Whether perfr_init() found a libperfr_api. False means every call is a no-op. */
	static bool IsLibraryLoaded() { return perfr_available() != 0; }
};
