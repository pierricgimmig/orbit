# Poster: AI-driven profiling loop on official Stockfish

**Orbit** (this repo — [pierricgimmig/orbit](https://github.com/pierricgimmig/orbit), not google/orbit)
meets **official Stockfish**. The goal is a closed loop an external model can
drive: build the engine, capture a profile, propose a tiny change, rebuild,
compare nodes/second, and keep the patch only when the gain clears a
statistical gate.

This is a case study of the **harness**, not a Stockfish speedup. After four
implementation phases, the loop ran one live experiment on a cloud VM and
**rejected +1.68% bench nps as noise**. Nothing was pushed to
[official-stockfish/Stockfish](https://github.com/official-stockfish/Stockfish).

| | |
| --- | --- |
| Stockfish pin | `031dfeb437fa6b06cdbdf4ef89dfb82f6b83c4d3` (`stockfish-dev-20260913-031dfeb4`) |
| Host | Ubuntu 24.04 cloud VM, 4 CPUs, `g++` 13.3.0, `x86-64-avx512icl` |
| Orbit PR | https://github.com/pierricgimmig/orbit/pull/65 (draft, branch `rust`) |
| Date | 2026-09-18 |

How-to, commands, and phase notes live in [`README.md`](README.md). This
page is the story.

## What we built

| Phase | Artifact | Role |
| --- | --- | --- |
| 1 | `stockfish/` submodule + [`build.sh`](build.sh) | Official tree, pinned SHA, `make -j profile-build` |
| 2 | [`run_suite.py`](../../tools/stockfish-orbit-loop/run_suite.py) | speedtest, 20× bench, perft, Orbit/perf capture, JSON+MD summaries, `--compare` |
| 3 | [`mcp/server.py`](../../tools/stockfish-orbit-loop/mcp/server.py) | Stdio MCP (no `orbit mcp` CLI exists). Tools to run, read, patch, rebuild, compare |
| 4 | [`loop.py`](../../tools/stockfish-orbit-loop/loop.py) | Baseline → hotspots → propose → apply → rebuild → gate → accept or revert |
| 5 | This poster + [`UPSTREAM-PR-TEMPLATE.md`](UPSTREAM-PR-TEMPLATE.md) | Story and a **template** for a future official PR — not a submission |

The loop calls the same Python APIs as the MCP tools. It does not call
Claude / OpenAI / Grok. API keys stay on the agent host
(`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `XAI_API_KEY`, or Cursor login).

Safety, by design:

- Patches are confined to `stockfish/`; dry-run is the default on the MCP tool
- The loop never `git commit`s or `git push`es the submodule
- Accept requires mean bench nps gain **> 0.5%** and, when stdev exists,
  **outside 1σ**. A fingerprint or perft break rejects even a huge nps jump

## How to reproduce end-to-end

From a clone of this Orbit fork, on Linux:

```sh
git clone --recurse-submodules https://github.com/pierricgimmig/orbit.git
cd orbit
git checkout rust   # or cursor/stockfish-orbit-loop-659b
git submodule update --init stockfish

# Phase 1 — build
./docs/stockfish-orbit-loop/build.sh
# or: cd stockfish/src && make -j profile-build && ./stockfish bench

# Phase 2 — suite (writes gitignored docs/stockfish-orbit-loop/runs/<UTC>/)
python3 tools/stockfish-orbit-loop/run_suite.py
# optional: cargo +1.88.0 build --release --manifest-path rust/crates/orbit-service/Cargo.toml

# Phase 3 — MCP smoke (list tools + one call; no secrets)
python3 tools/stockfish-orbit-loop/mcp/server.py --list-tools
python3 tools/stockfish-orbit-loop/mcp/smoke.py
# Cursor: copy tools/stockfish-orbit-loop/mcp/cursor-mcp.example.json → .cursor/mcp.json

# Phase 4 — gate self-test, then one live iteration
python3 tools/stockfish-orbit-loop/loop.py --mode mock
python3 tools/stockfish-orbit-loop/loop.py --mode live --proposal auto \
  --bench-iters 10 --perft off --capture none
```

Expect the live iteration to **reject** unless Orbit later produces a
search-dominated profile and a real patch clears the gate. Recorded
numbers from this VM are in [`sample-run.md`](sample-run.md) and
[`loop-log/`](loop-log/).

## What the loop found (no Stockfish win)

Phase 2 baseline on this VM ([`sample-run.md`](sample-run.md)):

| Metric | Result |
| --- | --- |
| `speedtest 4 512 20` | **5,143,082 nps** |
| 20× `bench 16 1 13 default depth` | **1,408,300 ± 20,734 nps** (~1.5% stdev) |
| Fingerprint | 1,648,567 nodes |
| Smoke perft | 6/6 |
| Orbit HTTP capture | 7740 symbols loaded, **0 callstack samples** |
| Orbit file-mode | 356 samples; leaves often libc / NNUE load |
| `perf` CLI | not installed (kernel `6.12.94+`) |

A 3-iteration re-bench against that 20-iteration summary moved **+0.51%**
— already at the percent floor, and inside noise.

Phase 4 live attempt
[`0005`](loop-log/0005-live-auto/) applied a **two-line comment** in
`stockfish/src/misc.cpp` because the file-mode table was
`__nss_database_lookup` / `__madvise` / NNUE startup, not `Search::` or
`evaluate`. That comment is not an optimization.

| | Baseline (10×) | After no-op | Δ |
| --- | --- | --- | --- |
| Bench nps mean | 1,365,859 | 1,388,786 | **+1.68%** (+22,927) |
| Bench nps stdev | 59,898 | 28,278 | one slow baseline iter |
| Fingerprint | 1,648,567 | 1,648,567 | unchanged |

**Gate: reject.** +1.68% is greater than 0.5% but **not** outside 1σ
(threshold 59,898 nps). The submodule was reverted; `stockfish/` stayed
clean; nothing was pushed.

Mock fixtures 0001–0004 (no `make`, tree untouched) show the same gate:
+0.20% reject, +0.60% inside 1σ reject, +2.00% above 1σ accept,
fingerprint mismatch reject.

Do not cite +1.68% or +0.51% as a Stockfish improvement.

## What Orbit still needs for a real upstream opt

A plausible official-Stockfish patch needs a profile of **search workers
during `go`**, not the UCI thread during startup. Today:

1. **Serve-mode samples are empty.** `POST /api/symbols/load` reaches
   `ready` (thousands of functions) and capture starts, but
   `/api/sampling/report` returns **0 callstack samples** with rings
   open at `kernel.perf_event_paranoid=1`.
2. **File-mode is one tid.** `orbit-service --pid` samples the UCI
   leader. Worker tids returned 0 samples even as root. Leaves are
   often `__madvise`, `__nss_database_lookup`, `Network::load`.
3. **Attach timing.** Sampling must start after `go`, on
   `ThreadPool` workers in `search` / `qsearch`, not while the leader
   sits in `getline`.
4. **Permissions.** `paranoid` ≤ 1 for attach (≤ 0 system-wide);
   `setcap cap_perfmon,cap_sys_ptrace+ep` on `orbit-service` so workers
   can be followed without root.
5. **Cross-check.** No `linux-perf` package on this kernel. Dwarf
   `perf record --call-graph` would validate Orbit once the package
   exists.

Until the top of the table is search/eval, `--proposal auto` must keep
taking the labeled no-op path. Auto-authoring a game-logic change from
libc leaves would be a fake win.

## Status and next steps

**Status (2026-09-18):** Phases 1–5 of the poster project are in
[PR 65](https://github.com/pierricgimmig/orbit/pull/65) (draft). The
loop works. Capture quality is not yet good enough to hunt a Stockfish
micro-opt. There is no candidate for official-stockfish.

**Continue here (human or agent), in order:**

1. **Fix Orbit search-worker sampling** (this repo, not Stockfish).
   Make serve-mode produce callstack samples on the target PID; make
   file-mode attach to worker tids *after* `go`. Re-run
   `run_suite.py --capture orbit` until `summary.json` `profile.hotspots`
   are dominated by `Stockfish::Search` / `evaluate` / `MovePicker`.
2. **Tighten the bench.** Use ≥20 iterations (Phase 2 stdev was ~1.5%,
   not the 4% from a 10-iter run with one outlier) so the 1σ gate is
   meaningful.
3. **Only then propose a real patch** mapped to a hotspot file, via
   `stockfish_apply_patch` dry-run first. Keep it small. Run
   `loop.py --mode live` (or MCP `stockfish_run_loop`) with the same
   binary flavor (`profile-build` vs `build`) on both sides.
4. **If the gate accepts:** leave the change in the `stockfish/`
   working tree, copy the diff into a new `loop-log/` entry, fill
   [`UPSTREAM-PR-TEMPLATE.md`](UPSTREAM-PR-TEMPLATE.md) with *that*
   iteration’s numbers, and have a human review. Still do not push to
   official-stockfish from this automation.
5. **If the gate rejects:** keep the log, revert, try the next
   hypothesis. Do not loosen the gate to “create” a win.

**Recommended next experiment:** do not edit Stockfish. Reproduce HTTP
capture with `perf_event_paranoid=1`, then attach file-mode to a worker
tid *after* `go movetime` has started, and check whether
`/api/sampling/report` or the `.pod` dump finally shows `Search::`
frames. Log that capture as a Phase 2 suite run. Only if those symbols
dominate, start a Phase 4 live iteration with a real (still tiny)
patch.

## Doc map

| File | What |
| --- | --- |
| [`README.md`](README.md) | Setup, commands, phase notes |
| [`sample-run.md`](sample-run.md) | Phase 2 VM numbers |
| [`loop-log/`](loop-log/) | Phase 4 attempts (JSON + decision) |
| [`UPSTREAM-PR-TEMPLATE.md`](UPSTREAM-PR-TEMPLATE.md) | Future official-stockfish PR text (template only) |
| [`build.sh`](build.sh) | Submodule init + profile-build + smoke bench |
