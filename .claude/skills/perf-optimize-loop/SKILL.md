---
name: perf-optimize-loop
description: >
  Profile a native program with Orbit and iterate on real, measured speedups —
  the profile→hook→analyze→change→measure→gate loop. Use when asked to optimize,
  speed up, or profile a native executable (a game, an engine, a benchmark) and
  make code changes that are proven faster, not guessed. Covers the Orbit MCP
  tools (tools/optimize-loop), safe hook selection (don't instrument functions
  called too often), the correctness fingerprint, and the noise-aware accept gate.
---

# Perf optimize loop

Be the optimization agent the loop is built for. Orbit gathers the data and the
gate measures honestly; you supply the hypotheses and the code changes. Never
claim a win the gate did not confirm.

## 0. Setup (once)

- Build the service: `cd rust/crates/orbit-service && cargo build --release`.
- Tooling lives in `tools/optimize-loop/`. Target = a config file (TOML). Set
  `project`, `build.command`, `bench.command`, `capture.command`. The bench
  command must print `ORBIT_METRIC <name>=<number>` (the thing to improve) and,
  for a deterministic target, `ORBIT_FINGERPRINT <value>` (a value that must NOT
  change — for a chess engine, the bench node count).
- Drive it over MCP: `python3 tools/optimize-loop/mcp/server.py` speaks JSON-RPC
  2.0 over stdio (NDJSON or Content-Length). Tools: `optimize_run_suite`,
  `optimize_get_summary`, `optimize_inspect_hotspots`, `optimize_apply_patch`
  (sandboxed to the project, dry-run by default), `optimize_rebuild`,
  `optimize_rerun_compare`, `optimize_run_loop`. A minimal stdio client is
  enough (initialize → tools/call).

## 1. Gather data — sampling first

Run `optimize_run_suite` (capture `orbit`, or `auto`), then
`optimize_inspect_hotspots`. Sampling needs no privilege at
`perf_event_paranoid <= 1` (`= -1` is best) and works on heavy native
workloads. Read the self-% table: that is where the time is.

## 2. Hooking — precise counts, but pick targets carefully

Sampling gives time-%, not call counts. To get per-function call count and
total/avg duration, hook the function with Orbit's uprobes (the service's
`/api/capture/start` with `instrumented_functions` + `kernel_uprobes`, or the
hook harness). **This needs privilege** (`CAP_SYS_ADMIN` via `sudo` /
`tools/sudo`); if `sudo -n` is unavailable, say so and select from sampling
instead — do not fake counts.

**The rule that matters: never hook a function called too often.** A leaf called
millions of times per second floods the scope rings, adds ruinous overhead, and
drops events — the numbers you get back are then wrong, and the target can even
crash. Prefer a coarse-grained *caller* one or two levels up, whose entry/exit
brackets the hot work but fires thousands, not millions, of times. Iterate: hook
a candidate, check the status line for "records lost" / dropped events; if it is
flooding, back off to a coarser function. Hooking is for *attributing* time and
*confirming* a function is worth changing, not for measuring the tight leaf.

## 3. Analyze and change — the correctness fingerprint constrains you

Pick a hotspot and read its source. The decisive constraint: **only a
behavior-preserving change can be accepted**, because a change that alters the
output changes the fingerprint. For an engine that means: you may make the same
computation *faster* (SIMD, memory layout, `__restrict` no-alias hints,
prefetch, branch hints, removing redundant work), but you may **not** change
what it computes (move ordering, eval values, search shape) — that changes the
node count and is correctly rejected. This is a feature: it stops you from
"optimizing" by quietly breaking the program.

Propose the change through `optimize_apply_patch` (or edit directly). Give it a
real mechanism, not a guess. Example that worked on Stockfish: the NNUE
`apply_combined` reads one accumulator and writes another that provably do not
alias — adding `__restrict` to the source/dest pointers let a non-PGO build keep
tiles in registers across the passes. Measured, behavior-preserving.

## 4. Measure — the gate, and confirm accepts

`optimize_rebuild` then `optimize_rerun_compare` (or `optimize_run_loop`). A
change is accepted only if **all** hold:

1. **fingerprint unchanged** (same output);
2. **mean gain > `min_gain_percent`** (default 0.5%);
3. **delta > 1σ**, σ = the larger of the within-run and between-run baseline
   spread;
4. **gain beats the measured between-run noise floor** (`--noise-batches`
   re-measures the baseline binary; a benchmark drifts more between runs than
   within one, so a no-op can otherwise drift past 1σ and read as a win).

Even after an accept, **confirm with an interleaved A/B**: alternate baseline
and candidate runs (A,B,A,B,…) and count paired wins. Drift cancels; if the
candidate is faster in nearly every paired round, the win is real. A single
back-to-back mean can be luck. (On Stockfish, a `+1.5%` single-shot accept held
up as `+2.56%`, 14/14 paired.)

## 5. Iterate and leave clean

Keep going on the next hotspot. **Leave the target tree clean** unless you are
deliberately keeping an accepted change — revert experiments (the loop does this
on reject). Save winning patches separately and offer them; do not silently
leave edits in someone else's checkout.

## Honest defaults

- If sampling records 0 callstacks, do not invent a win from an empty profile.
- Report rejects plainly; a well-tuned target resists naive changes, and "the
  gate rejected my change" is a valid, useful result.
- Build parity matters: compare like with like (same ARCH, same PGO state). A
  non-PGO baseline has real headroom (PGO alone was +5.2% on Stockfish,
  behavior-preserving) — note it, but attribute code-change wins to the code.
