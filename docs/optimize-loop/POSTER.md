# Poster: Orbit-driven optimize loop for external projects

**Orbit** ([pierricgimmig/orbit](https://github.com/pierricgimmig/orbit))
can drive a measure → propose → rebuild → compare loop against **any
user-pointed executable**. The target is configured, not vendored. Orbit
does not ship another project’s source as a git submodule.

| | |
| --- | --- |
| PR | https://github.com/pierricgimmig/orbit/pull/65 (draft) |
| Date | 2026-09-18 |

## What we built

| Piece | What it does |
| --- | --- |
| Config (TOML/JSON/YAML) | `project`, `build`, `bench`, `correctness`, `capture`, `env` |
| `suite.py` | Runs those commands; parses a primary throughput metric; JSON+MD summary; `--compare` |
| Capture | Attaches this repo’s `orbit-service` to the **child PID** |
| MCP | Stdio tools `optimize_*` so an external model can drive the loop |
| `loop.py` | Baseline → hotspots → patch (project only) → rebuild → gate → accept or revert |

The in-repo fixture (`tools/optimize-loop/fixture/workload.py`) is a
plumbing demo. It is not a product benchmark.

## How to reproduce

```sh
git clone https://github.com/pierricgimmig/orbit.git
cd orbit
git checkout rust   # or cursor/stockfish-orbit-loop-659b

python3 tools/optimize-loop/suite.py --config docs/optimize-loop/examples/fixture.toml
python3 tools/optimize-loop/loop.py --mode mock
python3 tools/optimize-loop/mcp/smoke.py
```

Point at some other tree (clone it yourself, out of Orbit):

```sh
cp docs/optimize-loop/examples/external-project.toml /tmp/my-target.toml
# edit project= and commands
python3 tools/optimize-loop/suite.py --config /tmp/my-target.toml
```

## What the loop found

The gate is generic: mean metric gain **> 0.5%** and **outside 1σ**, plus
a stable fingerprint. Mock fixtures (0001–0004 style) reject +0.20%,
reject +0.60% inside 1σ, accept +2.00% above 1σ, reject fingerprint
mismatch.

A live no-op on a real engine previously moved **+1.68% inside 1σ** and
was **rejected**. That was noise, not a win — do not cite it as an
optimization. New live runs should use the fixture or an external
checkout you own.

Capture quality is still the limiter: HTTP sampling may report 0
callstacks; file-mode is one tid. Until the hotspot table is the real
hot path, `--proposal auto` stays on a labeled no-op.

## Status / next steps

1. Use the fixture to prove suite + MCP + gate on a clean Orbit checkout.
2. Improve Orbit attach so serve-mode fills callstacks on an arbitrary PID
   after the workload is running.
3. Point `--config` at a real external project and only then propose a
   tiny mapped patch. Keep the gate; do not loosen it to manufacture a win.
4. If the gate accepts: leave the target dirty, log the attempt, fill
   [`UPSTREAM-PR-TEMPLATE.md`](UPSTREAM-PR-TEMPLATE.md) for *that*
   project. A human opens any upstream PR.

## Doc map

| File | What |
| --- | --- |
| [`README.md`](README.md) | Commands and config schema |
| [`examples/`](examples/) | Fixture + external-project templates |
| [`loop-log/`](loop-log/) | Attempt JSON |
| [`UPSTREAM-PR-TEMPLATE.md`](UPSTREAM-PR-TEMPLATE.md) | Generic “propose to project X” outline |
