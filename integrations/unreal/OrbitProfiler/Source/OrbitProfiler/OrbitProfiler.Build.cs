// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

using UnrealBuildTool;

public class OrbitProfiler : ModuleRules
{
	public OrbitProfiler(ReadOnlyTargetRules Target) : base(Target)
	{
		PCHUsage = PCHUsageMode.UseExplicitOrSharedPCHs;

		// Only Core: the external-profiler interface, modular features and
		// the module system all live there, so the plugin loads at
		// PostConfigInit, well before the engine picks its profiler at the
		// end of FEngineLoop::Init.
		PublicDependencyModuleNames.AddRange(new string[] { "Core" });

		// orbit.h links nothing and loads liborbit_api at run time. Its
		// loader uses dlopen, which glibc older than 2.34 keeps in libdl.
		if (Target.Platform == UnrealTargetPlatform.Linux || Target.Platform == UnrealTargetPlatform.LinuxArm64)
		{
			PublicSystemLibraries.Add("dl");
		}
	}
}
