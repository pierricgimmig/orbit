# TEMPLATE ONLY — not a Stockfish submission

**Do not open this against [official-stockfish/Stockfish](https://github.com/official-stockfish/Stockfish).**

This file is a fill-in outline for a *future* official PR, to be used only
after the Orbit loop **accepts** a real, reviewable engine change (mean
bench nps gain > 0.5% **and** outside ~1σ, fingerprint unchanged, perft
green). As of 2026-09-18 no such change exists. Live attempt 0005 was a
no-op comment that moved **+1.68% inside 1σ** and was **rejected**.

Copy this markdown, replace every `{{placeholder}}`, attach the accepted
`loop-log/NNNN-*/` files, and have a human review before anyone opens an
upstream PR by hand.

---

## Title

`{{one-line description of the accepted change}}`

## Motivation

`{{Why this code path is hot. Cite Orbit / perf symbols, not libc leaves.}}`

Search workers spend `{{X%}}` of samples in `{{Symbol::name}}`
(`{{file.cpp}}:{{line}}`) according to
`{{path to summary.json or loop-log attempt}}`.

This is **not** a fishing expedition. The profile was taken **during
`go`**, on **worker tids**, after Orbit serve-mode or file-mode produced
a search-dominated hotspot table.

## Orbit workflow

Harness: https://github.com/pierricgimmig/orbit (poster loop under
`docs/stockfish-orbit-loop/`, PR `{{orbit PR URL}}`).

1. Pin Stockfish `{{sha}}` (`{{dev tag}}`) as a submodule.
2. `make -j profile-build` in `src/` (or `{{build command}}`).
3. `python3 tools/stockfish-orbit-loop/run_suite.py` — speedtest, N×
   `bench 16 1 13 default depth`, perft, Orbit capture.
4. Inspect `summary.json` `profile.hotspots`.
5. Apply a patch **only** under `stockfish/` (MCP `stockfish_apply_patch`
   or `loop.py`). Never force-push.
6. Rebuild, re-run the suite with `--baseline`, run the nps gate
   (`min_gain_percent=0.5`, `sigma=1.0`).
7. Accept only if the gate passes. Log: `docs/stockfish-orbit-loop/loop-log/{{id}}/`.

Orbit version / service: `{{orbit-service SHA or tag}}`.
Capture backend: `{{orbit-http | orbit-file | perf}}`.
`kernel.perf_event_paranoid`: `{{value}}`.

## Before / after numbers

Hardware: `{{CPU, cores, OS, compiler, ARCH}}`.
Stockfish: `{{sha}}` → this patch (working tree / commit `{{new sha}}`).
Same binary flavor on both sides: `{{profile-build | build}}`.

| Metric | Baseline | After | Δ |
| --- | --- | --- | --- |
| Bench nps mean (`bench 16 1 13 default depth` × `{{N}}`) | `{{mean}}` ± `{{stdev}}` | `{{mean}}` ± `{{stdev}}` | `{{pct}}` (`{{delta}}` nps) |
| Bench nodes (fingerprint) | `{{nodes}}` | `{{nodes}}` | must match |
| `speedtest {{threads}} {{hash}} {{seconds}}` nps | `{{nps}}` | `{{nps}}` | `{{pct}}` |
| Smoke / official perft | `{{pass}}` | `{{pass}}` | must pass |

Gate: gain `{{pct}}` > 0.5% and `{{delta_nps}}` > `{{sigma}}` × baseline
stdev `{{stdev}}`. Decision: **accept**. Attempt id: `{{NNNN}}`.

Do not fill this table from rejected runs. Rejected +1.68% (attempt 0005)
and +0.51% (Phase 2 re-bench) are noise and must not appear here as wins.

## Patch summary

`{{2–8 sentences: what changed, why it is correct, why it is faster.}}`

```
{{unified diff or link to loop-log/NNNN/proposal.diff}}
```

Touched files (all under Stockfish):

- `{{src/foo.cpp}}`
- `{{src/foo.h}}`

No NNUE net changes unless that is the actual patch. No drive-by
reformatting.

## Test plan

Run from `stockfish/src` after `make -j profile-build`:

```sh
# Fingerprint + nps (repeat N ≥ 20 for a stdev)
./stockfish bench 16 1 13 default depth
# expect nodes searched == {{fingerprint}}

# Primary throughput (official default is 150 s; note if shortened)
./stockfish speedtest    # or: ./stockfish speedtest {{threads}} {{hash}} {{seconds}}

# Correctness — official suite (needs expect; cwd is src/)
../tests/perft.sh
```

Poster-harness equivalent (from the Orbit repo):

```sh
python3 tools/stockfish-orbit-loop/run_suite.py \
  --speedtest-seconds 150 --perft official --bench-iters 20 \
  --baseline {{path/to/baseline/summary.json}}
```

Also:

- [ ] Bench node count unchanged vs unpatched pin
- [ ] Smoke perft 6/6 (or official `perft.sh` green)
- [ ] No new compiler warnings on `{{compiler}}`
- [ ] Fishtest / SPRT only if Stockfish maintainers want a Elo check
      (nps ≠ Elo; do not claim rating from this table)

## Non-goals / honesty

- This PR is **not** opened by the Orbit cloud agent.
- Numbers are from `{{host}}` and may not match other machines.
- Capture quality: `{{search-dominated | still libc — STOP and do not submit}}`.
