// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "OrbitProfiler.h"

#include "Features/IModularFeatures.h"
#include "Logging/LogMacros.h"
#include "Modules/ModuleManager.h"
#include "ProfilingDebugging/ExternalProfiler.h"
#include "Templates/UniquePtr.h"

DEFINE_LOG_CATEGORY_STATIC(LogOrbit, Log, All);

#if UE_EXTERNAL_PROFILING_ENABLED

/**
 * Unreal's external-profiler hook, backed by Orbit.
 *
 * FGenericPlatformMisc::BeginNamedEvent / EndNamedEvent call
 * StartScopedEvent / EndScopedEvent on the active profiler, and those are
 * what SCOPED_NAMED_EVENT*, FScopeCycleCounter (stat named events) and the
 * engine's own instrumentation expand to. Unreal ends an event without
 * naming it, so each thread keeps a small stack of the handles Orbit
 * returned; past its depth the count still balances and the inner scopes
 * are simply not recorded.
 */
class FOrbitExternalProfiler final : public FExternalProfiler
{
public:
	FOrbitExternalProfiler()
	{
		IModularFeatures::Get().RegisterModularFeature(FExternalProfiler::GetFeatureName(), this);
	}

	virtual ~FOrbitExternalProfiler() override
	{
		IModularFeatures::Get().UnregisterModularFeature(FExternalProfiler::GetFeatureName(), this);
	}

	/** What `-Orbit`, UE_EXTERNAL_PROFILER and the ini key match. */
	virtual const TCHAR* GetProfilerName() const override { return TEXT("Orbit"); }

	/** Once per game-thread frame, from FEngineLoop::Tick: a tick on the timeline. */
	virtual void FrameSync() override { orbit_instant(ORBIT_LIT("frame")); }

	/** Orbit records from the outside (Record in the viewer, or the API); nothing to pause here. */
	virtual void ProfilerPauseFunction() override {}
	virtual void ProfilerResumeFunction() override {}

	virtual void StartScopedEvent(const FColor& Color, const ANSICHAR* Text) override
	{
		Push(orbit_start(Text, Text ? strlen(Text) : 0));
	}

	virtual void StartScopedEvent(const FColor& Color, const TCHAR* Text) override
	{
		// Names are ASCII in practice (stat names, "FEngineLoop::Tick");
		// anything wider becomes '?' rather than costing a conversion per
		// scope on a hot thread.
		ANSICHAR Narrow[256];
		int32 Len = 0;
		if (Text)
		{
			for (; Len < 255 && Text[Len]; ++Len)
			{
				Narrow[Len] = (Text[Len] < 128) ? static_cast<ANSICHAR>(Text[Len]) : '?';
			}
		}
		Push(orbit_start(Narrow, Len));
	}

	virtual void EndScopedEvent() override
	{
		FHandleStack& Stack = ThreadStack();
		if (Stack.Depth > 0)
		{
			--Stack.Depth;
			if (Stack.Depth < FHandleStack::Capacity)
			{
				orbit_stop(Stack.Handles[Stack.Depth]);
			}
		}
	}

	/** Thread names reach Orbit from the OS (Unreal sets them with pthread_setname_np / SetThreadDescription). */
	virtual void SetThreadName(const TCHAR* Name) override {}

private:
	struct FHandleStack
	{
		static constexpr int32 Capacity = 256;
		orbit_scope Handles[Capacity];
		int32 Depth = 0;
	};

	static FHandleStack& ThreadStack()
	{
		static thread_local FHandleStack Stack;
		return Stack;
	}

	static void Push(orbit_scope Handle)
	{
		FHandleStack& Stack = ThreadStack();
		if (Stack.Depth < FHandleStack::Capacity)
		{
			Stack.Handles[Stack.Depth] = Handle;
		}
		++Stack.Depth;
	}
};

static TUniquePtr<FOrbitExternalProfiler> GOrbitExternalProfiler;

#endif // UE_EXTERNAL_PROFILING_ENABLED

void FOrbitProfilerModule::StartupModule()
{
	// Finds liborbit_api ($ORBIT_API_LIB, beside orbit-service on PATH,
	// beside the executable, then the system loader) or stays quiet: with
	// no library every instrumentation call is one predictable branch.
	const int Status = orbit_init();
	if (Status == 0)
	{
		UE_LOG(LogOrbit, Log, TEXT("Orbit: liborbit_api loaded; scopes reach a capturing orbit-service"));
	}
	else if (Status == ORBIT_E_NOLIB)
	{
		UE_LOG(LogOrbit, Log, TEXT("Orbit: no liborbit_api found (set ORBIT_API_LIB or put orbit-service on PATH); instrumentation is off"));
	}
	else
	{
		UE_LOG(LogOrbit, Warning, TEXT("Orbit: orbit_init failed with %d; instrumentation is off"), Status);
	}

#if UE_EXTERNAL_PROFILING_ENABLED
	GOrbitExternalProfiler = MakeUnique<FOrbitExternalProfiler>();
#else
	UE_LOG(LogOrbit, Log, TEXT("Orbit: external profiling is compiled out of this configuration (UE_EXTERNAL_PROFILING_ENABLED=0); named events will not be forwarded, ORBIT_* calls still work"));
#endif
}

void FOrbitProfilerModule::ShutdownModule()
{
#if UE_EXTERNAL_PROFILING_ENABLED
	GOrbitExternalProfiler.Reset();
#endif
	orbit_shutdown();
}

IMPLEMENT_MODULE(FOrbitProfilerModule, OrbitProfiler)
