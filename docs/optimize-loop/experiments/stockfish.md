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

## 2. Hooking — the "don't hook too-often" lesson, measured

Orbit uprobe hooking needs `CAP_SYS_ADMIN`, granted here by the passwordless
`tools/sudo` wrapper (`(root) NOPASSWD: /usr/local/bin/orbit-service-sudo`;
`sudo -n` on general commands still needs a password, but the wrapper does not).
Two functions were hooked, one at a time, over a ~6 s `bench 512 4 20`:

| hooked function | calls in ~6 s | outcome |
| --- | --- | --- |
| `apply_combined` (per-eval leaf) | **6,499,833** | **DROPPING EVENTS: ~1.07M scope records lost** — "hook fewer functions or lower the call rate"; 23,197 hits refused |
| `Search::Worker::iterative_deepening()` (coarse) | **28** | clean; brackets each position's whole search |

The rule from the `perf-optimize-loop` skill, shown with real numbers: a leaf
called millions of times per second floods the scope rings and corrupts its own
measurement. Hook a coarse *caller* instead — `iterative_deepening` sees 28
calls with zero drops. Hooking attributes time and confirms a function matters;
it is not for timing the tight leaf (sampling already shows the leaf is hot).

## 2b. Before/after traces

Full Orbit sampling captures of each build, saved as openable `.orbit.zip`
bundles under `docs/optimize-loop/runs/traces/` (gitignored):

| trace | samples | top of profile |
| --- | --- | --- |
| `stockfish-before.orbit.zip` | 15,469 | apply_combined 36.8%, propagate 9.8%, search 7.2% |
| `stockfish-after.orbit.zip` | 15,543 | apply_combined 37.8%, evaluate 16%, search 12.5% |

Open either with `orbit-service --serve`, then **Open** in the viewer. The eval
share stays ~37% because `__restrict` sped the whole eval proportionally rather
than removing a function; the win shows in nodes/second, not as a shrinking bar.

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
