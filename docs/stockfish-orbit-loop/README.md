# Stockfish AI-Driven Profiling & Optimization Loop

Phase 1 of the Orbit poster project: an AI-agent-driven profiling and
optimization loop on [official Stockfish](https://github.com/official-stockfish/Stockfish)
that should eventually produce a meaningful upstream Stockfish PR.

This folder is **setup only**. Later phases (Orbit MCP tools, the agent loop,
and Stockfish source changes) are out of scope here. Nothing in this project
is submitted to Stockfish upstream.

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
git checkout rust
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
sudo apt install -y build-essential git wget curl
```

| Tool | Role | Version that worked |
| --- | --- | --- |
| `g++` | C++17 compiler (Makefile default `COMP=gcc`) | 13.3.0 |
| `make` | Stockfish `src/Makefile` | GNU Make 4.3 |
| `git` | Submodule checkout | — |
| `wget` or `curl` | NNUE net download (`scripts/net.sh`) | — |
| `sha256sum` | NNUE checksum (optional; assumed valid if missing) | — |

No extra libraries beyond the usual `pthread` / `librt` linkage on GNU/Linux.
The first build downloads `nn-134a887f4c8f.nnue` from
`https://tests.stockfishchess.org/api/nn/` and embeds it; the `.nnue` file is
gitignored inside the submodule.

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

For a longer hardware-oriented run, Stockfish also ships `speedtest` (defaults
are all threads, `threads * 128` MiB hash, 150 seconds). That is too slow for
a smoke check; use a short invocation if needed:

```sh
./stockfish speedtest 1 16 5
```

## Out of scope (later phases)

- Orbit capture / MCP tools against this binary
- The agent profiling and optimization loop
- Stockfish source changes or an upstream PR
