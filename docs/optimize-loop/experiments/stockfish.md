# Experiment: optimizing Stockfish with the loop

A real run of the optimize loop, with an agent (Claude) as the driver, against
Stockfish — profile, analyse, change code, measure. Written as it happened,
noise and dead ends included.

## Setup

- Target: `official-stockfish/Stockfish`, cloned out of tree (not vendored),
  built `make -C src -j build ARCH=native COMP=gcc` → `x86-64-avxvnni`,
  **non-PGO**.
- Metric: `Nodes/second` from `./stockfish bench`. Fingerprint: `Nodes
  searched` (deterministic per binary — a "speedup" that changes it is a bug).
- Driven through the Orbit optimize-loop MCP server (`optimize_run_suite`,
  `optimize_inspect_hotspots`, …).

## 1. Gather data (Orbit sampling, via MCP)

Orbit serve-mode attached to a running `bench` and captured **~12,900
callstack samples** at `perf_event_paranoid = -1` — no privilege needed. The
hotspots are the engine's own shape:

| self % | function |
| --- | --- |
| ~36% | `NNUE ...apply_combined` |
| ~9.5% | `NNUE NetworkArchitecture::propagate` |
| ~7.5% | `Search::Worker::search<PV>` |
| ~6.3% | `MovePicker::next_move` |
| ~6% | `NNUE update_accumulator_refresh` |
| ~3.4% | `partial_insertion_sort` |

Eval (NNUE) dominates: `apply_combined` + `propagate` + accumulator updates are
over half the time.

## 2. Hooking — not done here, and why

The plan was to hook a few functions for exact call counts and durations. Orbit
uprobe hooking needs `CAP_SYS_ADMIN` (via `tools/sudo`), and `sudo -n` was not
available in this environment, so hooking was skipped rather than faked. The
methodology and its key caution are in the `perf-optimize-loop` skill: **never
hook a function called too often** — a per-eval leaf like `apply_combined` fires
millions of times and would flood the rings, distort timing, and drop events.
Hook a coarser caller instead. Sampling was enough to pick a target here.

## 3. Analyse and change

The fingerprint (node count) rules out anything that changes what the engine
computes — move ordering, eval math, search shape all change the node count and
would be (correctly) rejected. So the target is a **behavior-preserving**
speedup of the hot code.

`apply_combined` reads one accumulator state and writes another. `from` and
`to` are distinct `AccumulatorState`s and the transformer weights are a separate
`const` object, so the source, destination and weight arrays **provably do not
alias**. On a non-PGO build the compiler cannot know that and reloads across the
store passes. Hypothesis: mark the pointers `__restrict`. Behavior-preserving
(no output change), real mechanism. The patch is in
[`stockfish-apply-combined-restrict.patch`](stockfish-apply-combined-restrict.patch).

Dead end worth recording: a global `sed` to rename one store also hit a second
`apply_combined` variant behind an `#ifdef`, breaking the build — a reminder to
scope edits to the function, not the file.

## 4. Measure — the gate, then confirm

Baseline (12 runs, idle box): **1,443,448 nps ± 8,278 (CV 0.57%)**, fingerprint
constant. The between-run noise floor is ~0.6% (it climbs to ~2× under
concurrent load — measure it, do not assume it).

| build | nps mean | Δ vs baseline | fingerprint | gate |
| --- | --- | --- | --- | --- |
| baseline (plain) | 1,443,448 | — | 1659671 | — |
| **+ `__restrict` (code change)** | **1,465,586** | **+1.53%** | 1659671 | **ACCEPT** |
| PGO (`profile-build`, build change) | 1,518,650 | +5.21% | 1659671 | ACCEPT |

The `__restrict` change cleared every gate condition: fingerprint unchanged,
gain > 0.5%, delta > 1σ, and above the 0.57% noise floor.

**Confirmation (interleaved A/B, 14 paired rounds).** A single before/after mean
can be luck, so baseline and candidate were run alternately (drift cancels):
restrict was faster in **14/14** rounds, paired mean **+2.56%** (min +0.35%, max
+4.16%). The win is real and reproducible.

## What this showed

- Orbit's own sampling profiled a large, real, multi-threaded C++ program
  cleanly (~12,900 samples), contradicting the pessimistic "0 samples on VMs"
  caveat for this class of target.
- A profiler-guided, one-function, behavior-preserving source change —
  `__restrict` on the NNUE hot path — gave a confirmed **~2.5%** on the non-PGO
  build.
- The gate did its job both ways: it **accepted** genuine wins (the code change
  and the +5.2% PGO build) and, in earlier testing, **rejected** a no-op that
  benchmark drift had dressed up as a +3.9% "win" (this is why the noise-floor
  and interleaved-A/B checks exist).

The biggest single lever on this build is PGO (+5.2%, no source change). The
code change stacks a further ~2.5% and is upstreamable on its own.
