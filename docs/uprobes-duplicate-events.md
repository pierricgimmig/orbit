# The uprobes duplicate-event workaround, and what is underneath it

Investigated 2026-08-31. Two questions: what is the workaround in Orbit's C++,
and is there a Linux kernel bug behind it. The first is answered definitively;
the second is answered partially, and the honest limits are marked.

## The workaround

`src/LinuxTracing/UprobesUnwindingVisitor.cpp`, `OnUprobes()`:

```cpp
// We are seeing that, on thread migration, uprobe events can sometimes be
// duplicated: the duplicate uprobe event will have the same stack pointer and
// instruction pointer as the previous uprobe, but different cpu. In that
// situation, we discard the second uprobe event.
// We also discard a uprobe event in the general case of strictly-increasing
// stack pointers, ...
```

It keeps, per thread, a stack of `(sp, ip, cpu)` for open uprobes and drops an
incoming uprobe when either:

1. `sp > last_uprobe_sp` — logged as `MISSING URETPROBE OR DUPLICATE UPROBE`.
   The stack grows downward, so two consecutive *entries* on one thread cannot
   have an increasing stack pointer. Either a uretprobe went missing (the
   previous frame was never closed) or this entry is a duplicate.
2. `sp == last_uprobe_sp && ip == last_uprobe_ip && cpu != last_uprobe_cpu` —
   logged as `Duplicate uprobe on thread migration`. Same probe, same stack
   frame, reported from two different CPUs.

`UprobesFunctionCallManager::ProcessFunctionExit` is the other half: an exit
for a thread with no open entries returns `nullopt` rather than asserting, so
an unmatched *end* is tolerated silently.

So the symptom Orbit compensated for is duplicated **entry** events correlated
with **thread migration**, plus the resulting entry/exit accounting drift.

## Why the shape of the duplicate is a strong clue

Orbit opens these probes **per CPU**: `TracerImpl` comments that sample-delivery
fds are "two (uprobe + uretprobe) per CPU per instrumented function", and
`uprobes_retaddr_event_open(module, offset, pid, cpu)` is called for each CPU in
the cpuset. Every CPU therefore has its own ring buffer for the same probe.

A duplicate with **identical `sp` and `ip` but a different `cpu`** is the same
logical probe hit surfacing in two different per-CPU buffers. The stack pointer
being identical rules out two genuine nested calls; only a re-report or a
re-execution of the same instruction produces that.

## What is underneath: the XOL preemption window

The most credible upstream mechanism of this shape is the **execute-out-of-line
(XOL) preemption window**.

When a uprobe fires, the kernel does not execute the replaced instruction in
place: it copies it to a per-task XOL slot and single-steps it there, then
returns control. Probe bookkeeping around that window is per-CPU state. If the
task is **preempted or migrated between entering the XOL window and the
single-step completing**, the completion is handled on a different CPU from the
one that set the state up.

This is not speculation about the class of failure — it is the subject of
upstream work. The arm64 kprobes series *"arm64: kprobes: fix XOL preemption
window"* describes it precisely: XOL executes in normal kernel context while
probe state is kept per-CPU, so preemption or migration during the window means
the following single-step exception is handled on another CPU, corrupting that
state and preventing correct recovery. The fix disables preemption across the
XOL instruction so the pair is guaranteed to run on one CPU. It was found
through long stability runs where the effect appeared rarely.

That is the same failure shape Orbit sees: probe state that assumes one CPU,
broken by a migration inside the probe's window, surfacing as a duplicated or
unmatched event.

## Limits of this finding — read before citing it

- The upstream patch above is **arm64 kprobes**, not **x86-64 uprobes**. It
  establishes that "per-CPU probe state + migration inside the XOL window" is a
  real, acknowledged kernel failure mode. It is *not* proof that Orbit's x86-64
  uprobe duplicates come from the same code path.
- I did **not** find an upstream commit, bugzilla entry or LKML thread reporting
  duplicated *perf samples* from x86-64 uprobes on task migration. It may exist;
  I could not locate it. Anyone continuing this should search
  `linux-trace-kernel@vger.kernel.org` around `kernel/events/uprobes.c` and
  `arch/x86/kernel/uprobes.c`, and try to reproduce with a minimal per-CPU
  `perf_event_open` uprobe plus a migration-heavy workload.
- The **multiple-uretprobe** half has a by-design explanation rather than a bug.
  The kernel keeps a `return_instances` list per task and, on hitting the return
  trampoline, walks it and runs the handler for every instance whose frame the
  stack pointer shows has been left — which is how `longjmp` and similar
  non-local exits are handled. One trampoline hit can legitimately produce
  several return-probe callbacks. A consumer assuming 1:1 entry/exit sees that
  as duplicated ends. Orbit's tolerant `ProcessFunctionExit` handles it either
  way.

## What this means for the Rust port

`orbit-tracing-state`'s `function_calls` and `return_addresses` modules were
ported quirk-for-quirk, including the tolerant unmatched-exit behaviour, so the
port inherits the same resilience. The `(sp, ip, cpu)` duplicate detection lives
in `UprobesUnwindingVisitor`, which is **not yet ported** — when it is, this
logic must come with it, and this document is the reason it exists.

A cheaper option is available to the Rust collector specifically: because it can
open probes **per task instead of per CPU**, the two-buffer duplicate cannot
arise in the first place. That trades a larger number of file descriptors for
not needing the workaround. Worth measuring before choosing.

## Follow-up: the Rust service took the per-task route (2026-09-01)

`rust/crates/orbit-service/src/uprobes.rs` arms probes with `pid = <thread>,
cpu = -1` plus `PERF_ATTR.inherit`, not per CPU. Every thread therefore has
exactly one buffer for a given probe wherever it is scheduled, so the
`(sp, ip, cpu)` duplicate this document describes cannot be constructed, and
the filter it would need was never written. The cost is the one predicted
above: two file descriptors and two mappings per thread per hook, which is why
`MAX_HOOKS` is 16.

What replaced the filter is a reordering buffer. Entry and exit arrive on
separate rings, so a drain can hand back an exit before the entry that opened
it; hits are held for 100 ms and paired in timestamp order. That is a
different problem from the duplicate, and it is the one per-task probes
actually create.
