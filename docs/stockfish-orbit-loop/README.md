# Stockfish AI-Driven Profiling & Optimization Loop

Poster project: an AI-agent-driven profiling and optimization loop on
[official Stockfish](https://github.com/official-stockfish/Stockfish)
that should eventually produce a meaningful upstream Stockfish PR.

This folder is **setup + the Phase 2 benchmarking/profiling suite**. Later
phases (Orbit MCP tools, the agent loop, and Stockfish source changes) are
out of scope. Nothing here is submitted to Stockfish upstream.

| Phase | Status | What |
| --- | --- | --- |
| 1 | done | Official Stockfish git submodule + Linux build |
| 2 | this PR | Repeatable speedtest / bench / perft / Orbit-or-perf capture |
| 3–5 | later | MCP tools, agent loop, engine changes |

## Pinned Stockfish revision

The engine lives as a git submodule at `stockfish/`, pointing at official
`master` (not a release tarball):

| Field | Value |
| --- | --- |
| Remote | https://github.com/official-stockfish/Stockfish.git |
| Commit | `031dfeb437fa6b06cdbdf4ef89dfb82f6b83c4d3` |
| Description | `Remove unused constexpr_lsb and lsb_index64` |
| Date | 2026-09-13 |
| Dev tag | `stockfish-dev-20260913-031dfeb4` |

This was current `master` HEAD when Phase 1 was set up. The latest official
release tag at that time was `sf_19`; master is pinned instead because later
phases aim at current upstream development.

## Clone and submodule init

From a fresh clone of this Orbit fork:

```sh
git clone --recurse-submodules https://github.com/pierricgimmig/orbit.git
cd orbit
git checkout rust   # or the poster branch
```

If the repo is already cloned without submodules:

```sh
git submodule update --init stockfish
```

That checks out the pinned SHA above. Confirm:

```sh
git submodule status stockfish
#  031dfeb437fa6b06cdbdf4ef89dfb82f6b83c4d3 stockfish (...)
```

To add the submodule from scratch on a branch that does not have it yet:

```sh
git submodule add https://github.com/official-stockfish/Stockfish.git stockfish
git -C stockfish checkout 031dfeb437fa6b06cdbdf4ef89dfb82f6b83c4d3
```

## Dependencies (Linux)

Verified on Ubuntu 24.04 with the packages already present in the cloud VM.
On a clean Debian/Ubuntu machine:

```sh
sudo apt update
sudo apt install -y build-essential git wget curl expect
```

| Tool | Role | Version that worked |
| --- | --- | --- |
| `g++` | C++17 compiler (Makefile default `COMP=gcc`) | 13.3.0 |
| `make` | Stockfish `src/Makefile` | GNU Make 4.3 |
| `git` | Submodule checkout | — |
| `wget` or `curl` | NNUE net download (`scripts/net.sh`) | — |
| `sha256sum` | NNUE checksum (optional; assumed valid if missing) | — |
| `expect` | Official `tests/perft.sh` only | 5.45.4 |
| `python3` | Phase 2 suite (`tools/stockfish-orbit-loop/run_suite.py`) | 3.12 |
| `addr2line` / `nm` | Optional symbol helpers | binutils |
| Rust 1.88 | Optional: build **this** repo's `orbit-service` for capture | 1.88.0 |

No extra libraries beyond the usual `pthread` / `librt` linkage on GNU/Linux.
The first build downloads `nn-134a887f4c8f.nnue` from
`https://tests.stockfishchess.org/api/nn/` and embeds it; the `.nnue` file is
gitignored inside the submodule.

The Linux `perf` CLI is **optional**. This cloud kernel (`6.12.94+`) had no
matching `linux-perf` package. The suite profiles through **this** fork's
`orbit-service` (`perf_event_open` in-process). If `perf` is on `PATH`,
`--capture auto` uses it when Orbit is unavailable.

Serve-mode sampling of another process needed
`sudo sysctl -w kernel.perf_event_paranoid=1` on this VM (default was 2).
The suite does that automatically when `sudo -n` works. System-wide
scheduling still needs paranoid ≤ 0. File-mode currently records samples
on the UCI thread-group leader; worker tids returned 0 samples even as root.

## Build

Upstream docs and `make help` recommend a profile-guided build from `src/`.
That path worked on this VM (native arch auto-detected as `x86-64-avx512icl`):

```sh
cd stockfish/src
make help
make -j profile-build
```

`profile-build` is four steps: instrumented compile, `./stockfish bench` for
PGO, optimized recompile, then profile cleanup. On this 4-core VM it finished
in about 40 seconds, including the NNUE download.

To skip PGO (faster, slightly less optimized):

```sh
cd stockfish/src
make -j build
```

The executable is `stockfish/src/stockfish`. It is produced locally and must
not be committed (the submodule already gitignores `src/stockfish*` and
`*.nnue`).

A wrapper that inits the submodule, runs the same `profile-build`, and smokes
the binary:

```sh
./docs/stockfish-orbit-loop/build.sh
```

## Smoke test

Quick fingerprint / nodes-per-second check (default: 16 MiB hash, 1 thread,
depth 13):

```sh
cd stockfish/src
./stockfish bench
```

Result recorded on this cloud VM after `make -j profile-build`
(`g++` 13.3.0, `x86-64-avx512icl`):

```
===========================
Total time (ms) : 1152
Nodes searched  : 1648567
Nodes/second    : 1431047
```

`compiler` on the same binary:

```
Stockfish dev-20260913-031dfeb4
Compiled by                : g++ (GNUC) 13.3.0 on Linux
Compilation architecture   : x86-64-avx512icl
Compilation settings       : 64bit AVX512ICL VNNI AVX512 BMI2 AVX2 SSE41 SSSE3 SSE2 POPCNT
```

## Phase 2: benchmarking and profiling suite

Harness: [`tools/stockfish-orbit-loop/run_suite.py`](../../tools/stockfish-orbit-loop/run_suite.py).
It writes `summary.json` + `summary.md` under
`docs/stockfish-orbit-loop/runs/<UTC>/` (gitignored).

### What it runs

1. **Primary — `speedtest`.** Official command is
   `./stockfish speedtest [threads] [hash_MiB] [runtime_s]`.
   Unspecified args become: all CPUs, `threads * 128` MiB hash, **150 s**.
   The suite default is the same thread/hash rule with **20 s**, so a full
   run finishes on a cloud VM. Pass `--speedtest-seconds 150` for the
   official hardware number.
2. **Secondary — repeated `bench`.** At least 20 iterations of
   `bench 16 1 13 default depth`. Reports mean ± sample stdev of
   nodes/second and of elapsed ms. Node count is the correctness fingerprint
   (must stay `1648567` on this SHA).
3. **Correctness — perft.**
   - Default `--perft smoke`: official `tests/perft.sh` positions at reduced
     depth (published node counts; a few seconds).
   - `--perft official` or
     `./tools/stockfish-orbit-loop/perft_official.sh` runs the real
     `stockfish/tests/perft.sh` from `src/` (needs `expect`; depth-7 cases
     search billions of nodes — budget a long time).
4. **Profile (pluggable).** `--capture auto` (default):
   1. If **this** repo's `orbit-service` is built, start it
      (`--host 127.0.0.1 --serve`), `POST /api/symbols/load`,
      `POST /api/capture/start` on the Stockfish PID, run a bench, stop,
      then `GET /api/sampling/report` for a symbolized hotspot table.
   2. Else if the `perf` CLI exists:
      `perf record --call-graph dwarf -- ./stockfish bench …` and
      `perf report --stdio --demangle`.
   3. Else skip capture and record the reason. File-mode
      `orbit-service --pid --duration-ms --out capture.pod` is a last-resort
      attempt (one tid only — the UCI thread, not search workers).

Build this Orbit's service (Rust 1.88):

```sh
rustup toolchain install 1.88.0 --profile minimal
cargo +1.88.0 build --release --manifest-path rust/crates/orbit-service/Cargo.toml
# optional, for file-mode dumps:
cargo +1.88.0 build --release --manifest-path rust/Cargo.toml -p orbit-pod-dump
```

Or `./rust.sh -- --pid <tid> --duration-ms 5000 --out /tmp/c.pod`.

### Exact suite commands

```sh
# Full default suite (speedtest 20s, 20× bench, smoke perft, Orbit-or-perf)
python3 tools/stockfish-orbit-loop/run_suite.py

# Official 150s speedtest + official perft.sh
python3 tools/stockfish-orbit-loop/run_suite.py \
  --speedtest-seconds 150 --perft official

# Force a capture backend
python3 tools/stockfish-orbit-loop/run_suite.py --capture orbit
python3 tools/stockfish-orbit-loop/run_suite.py --capture perf
python3 tools/stockfish-orbit-loop/run_suite.py --capture none

# Before/after: run A, then run B against A's summary
python3 tools/stockfish-orbit-loop/run_suite.py --out /tmp/sf-a
python3 tools/stockfish-orbit-loop/run_suite.py --out /tmp/sf-b \
  --baseline /tmp/sf-a/summary.json

# Compare two existing summaries without running
python3 tools/stockfish-orbit-loop/run_suite.py \
  --compare /tmp/sf-a/summary.json /tmp/sf-b/summary.json

# Official perft only
./tools/stockfish-orbit-loop/perft_official.sh
```

Each suite run prints the output directory. Open `summary.md` for the table;
`summary.json` is the machine-readable form (SHA, timestamps, command lines,
nps stats, perft pass/fail, hotspot rows).

### Recorded on this cloud VM

Full write-up: [`sample-run.md`](sample-run.md). Headline numbers from
`python3 tools/stockfish-orbit-loop/run_suite.py` on 2026-09-18:

```
speedtest 4 512 20          5,143,082 nps
bench 16 1 13 × 20          1,408,300 ± 20,734 nps
smoke perft                 6/6 passed
orbit-service file-mode     356 samples (HTTP report: 0 samples)
perf CLI                    not installed (kernel 6.12.94+)
```

`--baseline` / `--compare` print nps deltas. A 3-iteration re-bench against
the 20-iteration summary printed `+0.51%` (run-to-run noise).

## Out of scope (later phases)

- Orbit MCP tools against this binary
- The agent profiling and optimization loop
- Stockfish game-logic changes or an upstream PR
