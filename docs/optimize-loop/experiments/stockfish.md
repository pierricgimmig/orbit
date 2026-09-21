# Experiment: optimizing Stockfish with the loop

Two runs of the optimize loop, with an agent (Claude) as the driver, against
Stockfish — profile, hook, analyse, change code, measure. Written as it
happened, noise, dead ends and one retraction included. Run 2's full
chronological trace, patches, scripts and data are in
[`stockfish-run2/`](stockfish-run2/); the narrative is blog post 23.

## Setup

- Target: `official-stockfish/Stockfish` (`17a6c8f`), cloned out of tree (not
  vendored), built `make -C src -j16 build ARCH=native COMP=gcc` →
  `x86-64-avxvnni`, **non-PGO** baseline.
- Metric: `Nodes/second` from `./stockfish bench 128 1 13`. Fingerprint:
  `Nodes searched` (1659671 for every binary that ever passed — a "speedup"
  that changes it is a bug).
- Driven through the Orbit optimize-loop MCP server (`optimize_run_suite`,
  `optimize_inspect_hotspots`, `optimize_apply_patch`, `optimize_rebuild`,
  `optimize_rerun_compare`).
- Box: i9-13900KF (8 P + 16 E cores), `powersave`, shared with the owner's
  other processes throughout. This matters below.

## 1. Gather data (Orbit sampling, via MCP)

Orbit serve-mode attached to a running `bench` and captured **12,796**
callstack samples at `perf_event_paranoid = -1` — no privilege needed:

| self % | function |
| --- | --- |
| 37.3 | `NNUE apply_combined` (incremental accumulator update) |
| 9.2 | `NNUE NetworkArchitecture::propagate` |
| 7.3 | `NNUE update_accumulator_refresh_cache` |
| 6.5 | `Search::Worker::search<NonPV>` |
| 5.8 | `MovePicker::next_move` |
| 3.4 | `NNUE update_accumulator_incremental` |

## 2. Hooking — the "don't hook too-often" rule, measured

Uprobe hooking needs `CAP_SYS_ADMIN`, granted by the passwordless
`orbit-service-sudo` wrapper. Two functions, one at a time, over ~7 s:

| hooked function | calls | outcome |
| --- | --- | --- |
| `apply_combined` (per-eval leaf) | **6,157,420** | **DROPPING EVENTS**; 22,813 hits refused — "hook fewer functions or lower the call rate" |
| `Search::Worker::iterative_deepening()` (coarse) | **24** | clean; brackets each position's search |

A leaf called a million times a second floods the scope rings and corrupts
its own measurement. Hook a coarse caller; let sampling say the leaf is hot.

## 3. The `__restrict` change — claimed in run 1, retracted in run 2

**Run 1 said:** adding `__restrict` to the source/destination pointers of
`apply_combined` gave +1.53% (gate ACCEPT) and +2.56% in 14/14 interleaved
rounds; "real and reproducible", "upstreamable".

**Run 2 found:** the same patch, a sibling patch for
`update_accumulator_refresh_cache`, and both together were all **rejected** by
the gate (−2.24%, +0.81%, +1.43%, none outside 1σ) and all lost a pinned
16-round interleaved run (6/16, 5/16, 7/16 wins). `objdump` of the binaries
explains it: the hint changed `apply_combined` by **5 bytes at the same 615
instructions** and removed **3 instructions** from `refresh_cache`. GCC with LTO
already assumed no aliasing. A change that does not change the code cannot own
a speedup; run 1's 14/14 was an unpinned interleave on a hybrid CPU measuring
something other than the patch. **Retracted.**

## 4. Noise, and what is deterministic

Unpinned, one binary read 1.385–1.495 M nps inside one batch of ten (8%).
Six runs of the same binary pinned to a P-core with an idle sibling:

| reading | CV |
| --- | --- |
| nodes/second | 0.79% |
| `cycles:u` | 0.49% |
| `instructions:u` | **0.004%** |

Controls adopted: pin the bench (`taskset`), read `instructions:u` next to
nps (deterministic — separates "less work" from "luck"), cool down 20 s
after a parallel build (the first bench after one is measurably slower).

## 5. Where the misses are (perf PEBS — a gap in Orbit)

`mem_load_retired.l3_miss:upp`, 50K samples: **49%** of all L3-miss loads are
in `apply_combined`, **35%** in `refresh_cache`. Inside `apply_combined`, ~55%
are the eight `vpaddw mem` loads of a piece-square (HalfKA) weight-column
chunk — every line missing equally, i.e. not hardware-prefetched. The psq
table is 22,528 columns × 2 KiB = 46 MB > 36 MB L3; the threat columns, which
Stockfish *does* prefetch, miss far less. Instruction-level and precise-event
attribution are things Orbit cannot do yet; `perf` was used for exactly that.

## 6. Three prefetch experiments — all rejected

Behavior-preserving (integer adds, same order; fingerprint unchanged), each
measured pinned over six alternating pairs:

| variant | where | instr | cycles | nps | why it lost |
| --- | --- | --- | --- | --- | --- |
| P1 | all 32 lines of each psq column, right before `apply_combined` | +2.1% | +1.7% | −1.3% | up to 128 prefetches in a burst, no lead time — queueing |
| P2 | next tile's chunk, inside the tile loop | +7.9% | +2.0% | −2.1% | the chunk's eight loads already issue back to back (MLP); prefetch only added work |
| P3 | at `Worker::do_move` (real lead time), from/to/captured × both perspectives | +3.4% | +2.6% | −3.9% | 12 KiB per node for updates that often never happen — pollution |

A precise miss profile locates misses; it does not say they are on the
critical path. Patches kept in `stockfish-run2/` as the record.

## 7. Build-level levers — the gate, three times

`-mtune=native`: −0.27%, 6/16 interleaved — nothing. `profile-build` (PGO):

| gate run | baseline | noise batches | floor | PGO | verdict |
| --- | --- | --- | --- | --- | --- |
| 1 | 1,474,007 | 1,261,038 · 1,487,658 | 16.1% | +9.0% | **refused** — inside the floor (something took the core for ~20 s) |
| 2 | 1,489,739 | 1,538,748 · 1,552,906 | 4.1% | +0.3% | **invalid** — incremental `make build` linked the previous profile-build's objects: PGO vs PGO |
| 3 | 1,445,521 ± 0.72% | 1,438,885 · 1,433,271 | 0.85% | **1,500,970, +4.29%** | **ACCEPT** (>0.5%, >1σ, above floor, fingerprint 1659671) |

## 8. End-to-end confirmation — no profiler attached

16 rounds, rotated order, pinned, plain `./stockfish bench`:

| binary | mean nps | Δ | paired | wins |
| --- | --- | --- | --- | --- |
| baseline | 1,438,291 | — | — | — |
| `-mtune=native` | 1,433,989 | −0.30% | −0.27% | 6/16 |
| **PGO** | **1,524,148** | **+5.97%** | **+5.99%** [+0.17, +12.02] | **16/16** |

Node count 1,659,671 in all 48 runs.

## 9. Before/after Orbit captures

Sampling captures of both binaries over a 6 s `bench 256 4 22`, saved as
`.orbit.zip` bundles under `docs/optimize-loop/runs/traces/` (gitignored):

| trace | samples | top of profile (self %) |
| --- | --- | --- |
| `run2-before-baseline.orbit.zip` | 15,554 | apply_combined 37.6 · propagate 9.3 · search 7.3 · next_move 6.7 · refresh 4.9 |
| `run2-after-pgo.orbit.zip` | 15,570 | apply_combined 37.1 · Network::evaluate 14.9 · search 12.8 · next_move 9.1 · refresh 6.5 |

The after-profile is a different shape, not a smaller one: PGO inlined
`propagate` into `evaluate` and reshaped the search hot paths; the 37% leaf is
untouched, consistent with it being memory-bound and already tight. A coarse
hook (`iterative_deepening`, 28 calls) on the PGO binary was clean.

## 10. What the loop got wrong about itself

- **Incremental rebuild across build variants** (gate run 2): build-level
  variants must clean first; an incremental build does not see a flag change.
- **`optimize_rerun_compare` rebuilt implicitly** with no way to skip; it now
  takes `skip_build`, and re-measures pass it.
- **Stale "1071584 scope records lost"** on every hooked run, including a
  24-call coarse hook in a fresh service: the service discovers every scope
  ring in `/dev/shm`, including the owner's five-hour-old `samples` process,
  and counts its unread backlog as a lap loss. Follow-up: start a discovered
  segment at its current write cursor; attribute losses per producer.

## What this showed

- The gate rejected five code changes and one build flag, refused a real
  gain when the box could not prove it, and accepted it when the box could.
  Every reject was correct; the one accept held up 16/16 uninstrumented.
- The undeniable gain is Stockfish's own PGO build (+4.3% gated, +6.0%
  interleaved). Not a discovery — the discovery is a method that tells a 4%
  real gain from a 2.5% imaginary one on a shared machine.
- The previous write-up's `__restrict` claim is withdrawn.
