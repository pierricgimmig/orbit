# Stockfish AI-Driven Profiling & Optimization Loop

Poster project: an AI-agent-driven profiling and optimization loop on
[official Stockfish](https://github.com/official-stockfish/Stockfish)
that should eventually produce a meaningful upstream Stockfish PR.

This folder is **setup + the Phase 2 suite + Phase 3 MCP tools + Phase 4
loop**. Phase 5 (poster writeup / upstream PR text) is still later.
Nothing here is submitted to Stockfish upstream.

| Phase | Status | What |
| --- | --- | --- |
| 1 | done | Official Stockfish git submodule + Linux build |
| 2 | done | Repeatable speedtest / bench / perft / Orbit-or-perf capture |
| 3 | done | Stdio MCP so an external model can drive the suite |
| 4 | this PR | Closed loop: propose → rebuild → gate → accept or revert |
| 5 | later | Poster writeup; upstream PR text only if a patch ever accepts |

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

## Phase 3: MCP tools (agent control)

This Orbit repo has **no** `orbit mcp` / `orbit mcp init` CLI.
[`docs/TODO.md`](../TODO.md) item 12 is explicit: the agent-native surface
today is `orbit-scope` plus HTTP `POST /api/scope`. The MCP layer over that
CLI is **not done**, and it would be about instrumenting an agent, not
driving Stockfish.

Phase 3 therefore adds a **stockfish-specific stdio MCP server** in-tree:

[`tools/stockfish-orbit-loop/mcp/server.py`](../../tools/stockfish-orbit-loop/mcp/server.py)

JSON-RPC 2.0, MCP `2024-11-05` (also accepts later protocol versions).
Newline-delimited JSON **and** LSP-style `Content-Length` frames (Cursor /
the official MCP SDKs use the latter). Tool results are structured JSON
inside the MCP `content[0].text` field.

The server never talks to Claude / OpenAI / Grok and never reads API keys.

### Start / connect

From the repo root (stdio; leave it running for a host to attach):

```sh
python3 -u tools/stockfish-orbit-loop/mcp/server.py
```

Helpers that do **not** stay up as a server:

```sh
python3 tools/stockfish-orbit-loop/mcp/server.py --list-tools
python3 tools/stockfish-orbit-loop/mcp/server.py --call stockfish_get_summary
python3 tools/stockfish-orbit-loop/mcp/server.py --call stockfish_inspect_hotspots '{"limit":10}'
python3 tools/stockfish-orbit-loop/mcp/smoke.py
```

**Cursor:** copy
[`tools/stockfish-orbit-loop/mcp/cursor-mcp.example.json`](../../tools/stockfish-orbit-loop/mcp/cursor-mcp.example.json)
into `.cursor/mcp.json` (project) or `~/.cursor/mcp.json` (user). Restart
MCP / reload the window. The example is:

```json
{
  "mcpServers": {
    "stockfish-orbit-loop": {
      "command": "python3",
      "args": ["-u", "tools/stockfish-orbit-loop/mcp/server.py"],
      "cwd": "${workspaceFolder}"
    }
  }
}
```

**Claude Desktop:** same `mcpServers` object in
`~/Library/Application Support/Claude/claude_desktop_config.json` (macOS)
or `%APPDATA%\Claude\claude_desktop_config.json` (Windows). Set `cwd` to
the orbit checkout.

Do not invent `orbit mcp init` — it is not in this tree.

### LLM API keys (host, not this server)

Keys stay on the **agent host**. Do not commit them, do not put them in
the MCP `env` block unless you are deliberately forwarding them to some
*other* process.

| Provider | Typical env var | Also |
| --- | --- | --- |
| Claude | `ANTHROPIC_API_KEY` | Cursor login / dashboard |
| OpenAI | `OPENAI_API_KEY` | Cursor login / dashboard |
| Grok | `XAI_API_KEY` | Cursor login / dashboard |

This MCP process is a local toolchain. The model that *calls* the tools
is whatever the host already configured.

### Tools

| Name | What |
| --- | --- |
| `stockfish_run_suite` | Wrap `run_suite.py` (speedtest, bench, perft, capture) |
| `stockfish_get_summary` | Latest `summary.json` (`runs/latest`, or `path=`) |
| `stockfish_inspect_hotspots` | Top symbols from that summary's profile |
| `stockfish_apply_patch` | Preview or write a file **only** under `stockfish/` |
| `stockfish_rebuild` | `make -j profile-build` or `build` in `stockfish/src` |
| `stockfish_rerun_compare` | Re-run vs a baseline summary; return nps deltas |

`stockfish_apply_patch` defaults to **dry-run** (`apply=false`): it returns
the unified diff and does not write. `apply=true` writes the working tree
only. It refuses paths that escape `stockfish/` or touch `.git`. It never
`git commit`, `git push`, or force-pushes.

Each suite run updates `docs/stockfish-orbit-loop/runs/latest` (symlink, or
`latest.path` if the filesystem cannot symlink) so `get_summary` /
`inspect_hotspots` / `rerun_compare` can find the newest capture without
an explicit path.

### Sample tool I/O

`--call` prints the same JSON a `tools/call` result wraps in
`content[0].text`.

List:

```sh
python3 tools/stockfish-orbit-loop/mcp/server.py --list-tools
```

```json
{
  "tools": [
    {"name": "stockfish_run_suite", "description": "Run the Stockfish Orbit profiling+benchmark suite ..."},
    {"name": "stockfish_get_summary", "description": "Fetch the latest structured suite summary JSON ..."},
    {"name": "stockfish_inspect_hotspots", "description": "Return the top symbols / hotspot table ..."},
    {"name": "stockfish_apply_patch", "description": "Propose or apply a file edit inside the stockfish/ submodule only. ..."},
    {"name": "stockfish_rebuild", "description": "Rebuild Stockfish from stockfish/src ..."},
    {"name": "stockfish_rerun_compare", "description": "Re-run the bench suite against a baseline summary ..."}
  ]
}
```

Stdio handshake (NDJSON; Content-Length is the same JSON with headers):

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"example"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"stockfish_inspect_hotspots","arguments":{"limit":5,"path":"/tmp/sf-suite-5/summary.json"}}}
```

`stockfish_inspect_hotspots` (file-mode capture on this VM; serve-mode
still records 0 callstack samples):

```json
{
  "ok": true,
  "backend": "orbit-file",
  "samples": 137,
  "hotspots": [
    {"symbol": "__nss_database_lookup", "self": 57, "self_percent": 46.72},
    {"symbol": "__madvise", "self": 36, "self_percent": 29.51},
    {"symbol": "Stockfish::hash_bytes(char const*, unsigned long)", "self": 14, "self_percent": 11.48}
  ]
}
```

`stockfish_apply_patch` dry-run (nothing written):

```json
{
  "ok": true,
  "applied": false,
  "dry_run": true,
  "pushed": false,
  "path": "src/orbit_loop_note.txt",
  "diff": "--- a/src/orbit_loop_note.txt\n+++ b/src/orbit_loop_note.txt\n@@ -0,0 +1 @@\n+Phase 3 MCP dry-run only. Do not apply.\n"
}
```

`stockfish_rerun_compare` / `stockfish_run_suite` return `ok`, `summary_path`,
`speedtest_nps`, `bench_nps`, and a `compare` object with
`bench_nps_mean.delta_percent` when a baseline is set. Full recorded suite
numbers stay in [`sample-run.md`](sample-run.md).

### Recorded smoke (this VM)

`python3 tools/stockfish-orbit-loop/mcp/smoke.py` on 2026-09-18:

```
tools: stockfish_run_suite, stockfish_get_summary, stockfish_inspect_hotspots,
       stockfish_apply_patch, stockfish_rebuild, stockfish_rerun_compare
ndjson initialize + tools/list: ok (6 tools)
content-length initialize + tools/list: ok (6 tools)
stockfish_get_summary: ok sha=031dfeb437fa6b06cdbdf4ef89dfb82f6b83c4d3
stockfish_inspect_hotspots: backend=orbit-file rows=5
stockfish_apply_patch dry-run: ok (not written)
stockfish_apply_patch escape: refused
stockfish_run_suite: ok bench_nps mean=1,369,241 (1× bench, no capture)
```

`runs/latest` then pointed at `/tmp/sf-mcp-smoke`. The `stockfish/` submodule
working tree stayed clean.

### Blockers (unchanged from Phase 2)

- HTTP `orbit-service` capture: rings open, **0 callstack samples**.
- File-mode on search **worker** tids: 0 samples even as root. UCI leader works.
- No `linux-perf` package on kernel `6.12.94+`.
- File-mode leaf PCs on this VM are often startup / libc (`__madvise`), not
  search hot paths — useful as a plumbing check, not yet an optimization map.

## Phase 4: the agent loop

Driver: [`tools/stockfish-orbit-loop/loop.py`](../../tools/stockfish-orbit-loop/loop.py).
It calls the same Python APIs as the Phase 3 MCP tools (`ops.py`). There is
also `stockfish_run_loop` on the MCP server. **No LLM is invoked.** The
loop does not invent a fake nps win when the profile is libc/startup.

```sh
# CI-safe gate self-test (no make, stockfish/ untouched)
python3 tools/stockfish-orbit-loop/loop.py --mode mock

# One live iteration: baseline benches → classify hotspots → no-op experiment
# → incremental rebuild → re-bench → gate → revert if reject
python3 tools/stockfish-orbit-loop/loop.py --mode live --proposal auto \
  --hotspots-from /path/to/summary.json \
  --bench-iters 10 --skip-speedtest --perft off --capture none
```

MCP:

```sh
python3 tools/stockfish-orbit-loop/mcp/server.py --call stockfish_run_loop \
  '{"mode":"mock","log_dir":"/tmp/sf-loop-mock"}'
```

### Gate (accept only if all hold)

| Parameter | Default | Rule |
| --- | --- | --- |
| `min_gain_percent` | **0.5** | `after_mean` must exceed `baseline_mean` by more than 0.5% |
| `sigma` | **1.0** | When baseline bench stdev exists, `delta_nps` must be **> 1σ** |
| fingerprint | required | `bench.nodes_searched` must match; perft must not fail |

A +0.6% move inside a 1.5% stdev is **rejected** (Phase 2 noise was ~1.5% /
+0.51% on a 3-iter re-bench). Missing stdev falls back to the percent floor
only. `--min-gain-percent` / `--sigma` override the defaults.

On reject the loop `git checkout --` the experiment paths under `stockfish/`
and incrementally rebuilds so the binary matches HEAD. It never commits or
pushes the submodule. On accept the working tree is left dirty and the
patch is logged; still no upstream push.

### Proposal policy (honesty)

Orbit file-mode on this VM usually reports `__madvise` / `__nss_database_lookup`
/ NNUE load — not `Search::` / `evaluate`. `--proposal auto` then applies a
**labeled no-op** (a two-line comment in `stockfish/src/misc.cpp`) so the
machinery can be proven. That is not an optimization. Search-like symbols
do **not** auto-author a game-logic patch (Phase 4 will not fake a win).

### Attempt log

Written under [`docs/stockfish-orbit-loop/loop-log/`](loop-log/) (committed
JSON/MD/diff only; heavy suite outputs stay in gitignored `runs/`):

```
docs/stockfish-orbit-loop/loop-log/index.md
docs/stockfish-orbit-loop/loop-log/NNNN-<slug>/attempt.json
docs/stockfish-orbit-loop/loop-log/NNNN-<slug>/proposal.diff
docs/stockfish-orbit-loop/loop-log/NNNN-<slug>/decision.md
```

### What better Orbit search-thread sampling needs

1. `kernel.perf_event_paranoid` ≤ 1 (≤ 0 for system-wide). The suite already
   tries `sudo -n sysctl` to 1.
2. `sudo setcap cap_perfmon,cap_sys_ptrace+ep rust/.../orbit-service` so
   file-mode can follow workers without root.
3. **Attach after search starts**, to **worker tids** (`ThreadPool` threads
   that run `idle_loop` / `search`), not the UCI leader while it is blocked
   in `getline`. Today worker-tid file-mode returns 0 samples even as root.
4. Serve-mode `/api/capture/start` must actually fill callstack samples
   (currently 0 on this VM even with rings open and 7k+ symbols loaded).
5. A matching `linux-perf` package (absent on `6.12.94+`) for dwarf
   `--call-graph` as a cross-check.

Until those land, the loop’s honest live path is **no-op → gate reject**.

### Recorded on this VM (2026-09-18)

`python3 tools/stockfish-orbit-loop/loop.py --mode mock` wrote attempts
0001–0004: reject +0.20%, reject +0.60% inside 1σ, accept +2.00% above 1σ,
reject fingerprint mismatch.

Live attempt **0005** (`--proposal auto --bench-iters 10 --hotspots-from`
the Phase 2 file-mode summary):

| | |
| --- | --- |
| Hotspots | `__nss_database_lookup` 47%, `__madvise` 30%, NNUE load / `hash_bytes` |
| Proposal | Labeled no-op comment in `stockfish/src/misc.cpp` |
| Baseline bench | 1,365,859 ± 59,898 nps (10×; one slow iter) |
| After bench | 1,388,786 ± 28,278 nps |
| Δ | **+1.68%** (+22,927 nps) |
| Fingerprint | 1,648,567 unchanged |
| Gate | **reject** — delta < 1σ (threshold 59,898 nps) |
| Revert | `src/misc.cpp` restored; `stockfish/` clean; not pushed |

This is not an optimization win. The +1.68% is noise; the gate did its job.

Log: [`loop-log/0005-live-auto/`](loop-log/0005-live-auto/).

## Phase 5 (stub)

Poster writeup and candidate official-Stockfish PR text belong in Phase 5,
and only if a later iteration accepts a real, reviewable patch. This PR
does not push to `official-stockfish`.

## Out of scope (later)

- Full Phase 5 poster / upstream PR
- Stockfish game-logic “optimizations” invented from libc profiles
