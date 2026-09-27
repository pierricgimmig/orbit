// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#pragma once

#include "CoreMinimal.h"
#include "Modules/ModuleInterface.h"

// The Orbit manual-instrumentation API, for any module that depends on
// "OrbitProfiler": ORBIT_SCOPE("name"), ORBIT_VALUE("fps", 59.9),
// orbit_start_async / orbit_stop across threads, orbit_link, orbit_span for
// timestamps read back later (GPU work). One header, nothing to link; every
// call is a single predictable branch until an orbit-service is capturing
// this process. See orbit.h for the full API and where the library is
// looked for.
#if PLATFORM_WINDOWS
#include "Windows/AllowWindowsPlatformTypes.h"
#endif
#include "orbit.h"
#if PLATFORM_WINDOWS
#include "Windows/HideWindowsPlatformTypes.h"
#endif

/**
 * Loads liborbit_api once at startup and registers "Orbit" as an external
 * profiler, so that every SCOPED_NAMED_EVENT*, SCOPE_CYCLE_COUNTER (with
 * -statnamedevents), QUICK_SCOPE_CYCLE_COUNTER and engine named event reaches
 * Orbit as a scope on its thread, with a "frame" tick per game-thread frame.
 *
 * Select it as Unreal selects any external profiler: `-Orbit` on the
 * command line, `UE_EXTERNAL_PROFILER=Orbit` in the environment, or
 * `[Core.ProfilingDebugging] ExternalProfiler=Orbit` in DefaultEngine.ini.
 */
class FOrbitProfilerModule : public IModuleInterface
{
public:
	virtual void StartupModule() override;
	virtual void ShutdownModule() override;

	/** Whether orbit_init() found a liborbit_api. False means every call is a no-op. */
	static bool IsLibraryLoaded() { return orbit_available() != 0; }
};
