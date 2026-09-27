# Orbit in Unreal Engine

Three levels, each on top of the previous one. Written against Unreal 5.8;
the hooks it uses (`FExternalProfiler`, the named-event macros) have been in
the engine since 4.x.

## 0. Nothing to add

Sampling and the Functions tab's dynamic hooks work on any Linux build of an
Unreal game, Shipping included, with symbols from the `.debug` file next to
the executable. Point `orbit-service` at the process and press Record. This is
how the ThirdPerson demo in the e2e suite is profiled.

## 1. The plugin: Unreal's own macros reach Orbit

Copy [`OrbitProfiler/`](OrbitProfiler) into `<Project>/Plugins/` (a project
plugin is enabled by default) or `Engine/Plugins/Developer/` (then enable it
in the `.uproject`), build, and run with the profiler selected:

```
UnrealEditor MyGame.uproject -game -Orbit -statnamedevents
```

`-Orbit` is how Unreal selects an external profiler; `UE_EXTERNAL_PROFILER=Orbit`
in the environment or `[Core.ProfilingDebugging] ExternalProfiler=Orbit` in
`DefaultEngine.ini` do the same. From then on:

| Unreal | Orbit |
|---|---|
| `SCOPED_NAMED_EVENT(Name, Color)`, `_TEXT`, `_TCHAR`, `_F`, `_FSTRING` | a scope on the calling thread |
| `SCOPE_CYCLE_COUNTER(STAT_x)`, `QUICK_SCOPE_CYCLE_COUNTER(x)` | a scope, when stat named events are on: `-statnamedevents` or `stats.NamedEvents on` |
| the engine's own named events (`FEngineLoop::Tick`, render and RHI thread work, task graph, …) | scopes, same as above |
| each game-thread frame (`FExternalProfiler::FrameSync`) | a `frame` instant |
| thread names | come from the OS; Unreal names its threads |

The plugin is one Core-only module: at startup it calls `orbit_init()`, which
finds `liborbit_api` (`$ORBIT_API_LIB`, then beside an `orbit-service` on
`PATH`, `~/.local/bin`, `~/.orbit/bin`, beside the executable, then the system
loader) and logs one line either way. Without a library every macro is one
predictable branch. It then registers `FOrbitExternalProfiler`, which maps
Unreal's `StartScopedEvent` / `EndScopedEvent` onto `orbit_start` /
`orbit_stop` with a per-thread stack of handles, since Unreal ends an event
without naming it.

Configurations: Development and Test. Unreal compiles external profiling out
of Shipping (`UE_EXTERNAL_PROFILING_ENABLED` is `!UE_BUILD_SHIPPING`, and the
named-event macros need `ENABLE_NAMED_EVENTS`). To keep it in a Shipping
build add to the target:

```cs
GlobalDefinitions.Add("UE_EXTERNAL_PROFILING_ENABLED=1");
GlobalDefinitions.Add("ALLOW_NAMED_EVENTS_IN_TEST=1"); // Test builds only need this one
```

`orbit.h` is copied into the plugin so it can be dropped into a project as is;
[`sync-orbit-header.sh`](sync-orbit-header.sh) refreshes the copy from
`rust/crates/orbit-api/include/orbit.h`.

## 2. Your code: the Orbit API directly

A module that depends on `OrbitProfiler` gets the whole API from one include:

```cpp
#include "OrbitProfiler.h"

void AMyCharacter::Tick(float DeltaSeconds)
{
    ORBIT_SCOPE("AMyCharacter::Tick");       // RAII, nested on this thread
    ORBIT_VALUE("speed", GetVelocity().Size()); // a lane to graph
    ...
}

// A job handed to another thread: the handle is the identity, link joins them.
orbit_scope Enqueued = orbit_instant(ORBIT_LIT("enqueue"));
Async(EAsyncExecution::TaskGraph, [Enqueued] {
    ORBIT_SCOPE("job");
    orbit_link(Enqueued, orbit_start_async(ORBIT_LIT("job result")));
});

// GPU work whose timestamps come back later: a span at the times you supply.
orbit_span_async(ORBIT_LIT("shadow pass"), StartNs, EndNs);
```

Names are pointer and length (`ORBIT_LIT` for a literal); TCHAR text wants
`TCHAR_TO_ANSI` or the `FString`'s `Len()`. The header documents every call.

## 3. `TRACE_CPUPROFILER_EVENT_SCOPE` too (engine patch, optional)

The Unreal Insights macros never reach an external profiler: they write to
the trace channel or nothing. [`engine-cpu-trace-scopes.patch`](engine-cpu-trace-scopes.patch)
adds an Orbit scope to `TRACE_CPUPROFILER_EVENT_SCOPE_USE`, the one macro
every `TRACE_CPUPROFILER_EVENT_SCOPE*` expands through, so all of them show
in Orbit at the same cost as the Insights channel check when no library is
loaded. Apply from the engine root after copying the header:

```
cp <orbit>/rust/crates/orbit-api/include/orbit.h Engine/Source/Runtime/Core/Public/ProfilingDebugging/orbit.h
patch -p1 < <orbit>/integrations/unreal/engine-cpu-trace-scopes.patch
```

Two things to know. It touches a header that every module includes, so it is
a full engine rebuild. And volume: an editor frame emits tens of thousands of
these scopes, more than the scope ring drains (it drops past about a million
records a second and the viewer shows the gap), so start with the plugin's
named events and add the patch when you want the finer Insights-level
picture of one system. With the patch applied, `SCOPED_NAMED_EVENT` scopes
arrive twice if `-Orbit` is also passed; pick one route.

## Checking it works

![Unreal's named events as Orbit scopes: the ThirdPerson demo, Development Editor, `-game -nullrhi -Orbit -statnamedevents`](../../docs/screenshots/unreal-named-events.png)


The game log says `Orbit: liborbit_api loaded` at module startup. The
service log says `opened segment of pid <n>` when Record starts, and
`manual instrumentation: <n> events` when it stops. In the viewer the game's
threads carry scope bands named as the macros named them, and the game
thread has a `frame` tick per frame.
