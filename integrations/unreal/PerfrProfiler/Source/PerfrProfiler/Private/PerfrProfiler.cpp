// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "PerfrProfiler.h"

#include "Features/IModularFeatures.h"
#include "Logging/LogMacros.h"
#include "Modules/ModuleManager.h"
#include "ProfilingDebugging/ExternalProfiler.h"
#include "Templates/UniquePtr.h"

DEFINE_LOG_CATEGORY_STATIC(LogPerfr, Log, All);

#if UE_EXTERNAL_PROFILING_ENABLED

/**
 * Unreal's external-profiler hook, backed by perfr.
 *
 * FGenericPlatformMisc::BeginNamedEvent / EndNamedEvent call
 * StartScopedEvent / EndScopedEvent on the active profiler, and those are
 * what SCOPED_NAMED_EVENT*, FScopeCycleCounter (stat named events) and the
 * engine's own instrumentation expand to. Unreal ends an event without
 * naming it, so each thread keeps a small stack of the handles perfr
 * returned; past its depth the count still balances and the inner scopes
 * are simply not recorded.
 */
class FPerfrExternalProfiler final : public FExternalProfiler
{
public:
	FPerfrExternalProfiler()
	{
		IModularFeatures::Get().RegisterModularFeature(FExternalProfiler::GetFeatureName(), this);
	}

	virtual ~FPerfrExternalProfiler() override
	{
		IModularFeatures::Get().UnregisterModularFeature(FExternalProfiler::GetFeatureName(), this);
	}

	/** What `-perfr`, UE_EXTERNAL_PROFILER and the ini key match. */
	virtual const TCHAR* GetProfilerName() const override { return TEXT("perfr"); }

	/** Once per game-thread frame, from FEngineLoop::Tick: a tick on the timeline. */
	virtual void FrameSync() override { perfr_instant(PERFR_LIT("frame")); }

	/** perfr records from the outside (Record in the viewer, or the API); nothing to pause here. */
	virtual void ProfilerPauseFunction() override {}
	virtual void ProfilerResumeFunction() override {}

	virtual void StartScopedEvent(const FColor& Color, const ANSICHAR* Text) override
	{
		Push(perfr_start(Text, Text ? strlen(Text) : 0));
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
		Push(perfr_start(Narrow, Len));
	}

	virtual void EndScopedEvent() override
	{
		FHandleStack& Stack = ThreadStack();
		if (Stack.Depth > 0)
		{
			--Stack.Depth;
			if (Stack.Depth < FHandleStack::Capacity)
			{
				perfr_stop(Stack.Handles[Stack.Depth]);
			}
		}
	}

	/** Thread names reach perfr from the OS (Unreal sets them with pthread_setname_np / SetThreadDescription). */
	virtual void SetThreadName(const TCHAR* Name) override {}

private:
	struct FHandleStack
	{
		static constexpr int32 Capacity = 256;
		perfr_scope Handles[Capacity];
		int32 Depth = 0;
	};

	static FHandleStack& ThreadStack()
	{
		static thread_local FHandleStack Stack;
		return Stack;
	}

	static void Push(perfr_scope Handle)
	{
		FHandleStack& Stack = ThreadStack();
		if (Stack.Depth < FHandleStack::Capacity)
		{
			Stack.Handles[Stack.Depth] = Handle;
		}
		++Stack.Depth;
	}
};

static TUniquePtr<FPerfrExternalProfiler> GPerfrExternalProfiler;

#endif // UE_EXTERNAL_PROFILING_ENABLED

void FPerfrProfilerModule::StartupModule()
{
	// Finds libperfr_api ($PERFR_API_LIB, beside perfr on PATH,
	// beside the executable, then the system loader) or stays quiet: with
	// no library every instrumentation call is one predictable branch.
	const int Status = perfr_init();
	if (Status == 0)
	{
		UE_LOG(LogPerfr, Log, TEXT("perfr: libperfr_api loaded; scopes reach a capturing perfr"));
	}
	else if (Status == PERFR_E_NOLIB)
	{
		UE_LOG(LogPerfr, Log, TEXT("perfr: no libperfr_api found (set PERFR_API_LIB or put perfr on PATH); instrumentation is off"));
	}
	else
	{
		UE_LOG(LogPerfr, Warning, TEXT("perfr: perfr_init failed with %d; instrumentation is off"), Status);
	}

#if UE_EXTERNAL_PROFILING_ENABLED
	GPerfrExternalProfiler = MakeUnique<FPerfrExternalProfiler>();
#else
	UE_LOG(LogPerfr, Log, TEXT("perfr: external profiling is compiled out of this configuration (UE_EXTERNAL_PROFILING_ENABLED=0); named events will not be forwarded, PERFR_* calls still work"));
#endif
}

void FPerfrProfilerModule::ShutdownModule()
{
#if UE_EXTERNAL_PROFILING_ENABLED
	GPerfrExternalProfiler.Reset();
#endif
	perfr_shutdown();
}

IMPLEMENT_MODULE(FPerfrProfilerModule, PerfrProfiler)
