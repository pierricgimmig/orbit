# Orbit optimize-loop (external projects)

A **generic** profiling and optimization loop. You point it at **any
external project / executable** via a config file. Orbit does **not**
vendor targets — there is no engine submodule, and Orbit’s build does
not fetch one.

The in-repo [`examples/fixture.toml`](examples/fixture.toml) drives a
tiny Python workload so the suite, MCP tools, and accept/reject gate
can be demoed without cloning anything else.

Optional: clone some other tree *out of band* and point `project =` at
it. That checkout stays yours; never add it as an Orbit submodule.

| Piece | Path |
| --- | --- |
| Suite | [`tools/optimize-loop/suite.py`](../../tools/optimize-loop/suite.py) |
| Loop + gate | [`tools/optimize-loop/loop.py`](../../tools/optimize-loop/loop.py) |
| MCP | [`tools/optimize-loop/mcp/server.py`](../../tools/optimize-loop/mcp/server.py) |
| Fixture | [`tools/optimize-loop/fixture/workload.py`](../../tools/optimize-loop/fixture/workload.py) |
| Poster | [`POSTER.md`](POSTER.md) |

## Point the loop at a project

1. Copy [`examples/external-project.toml`](examples/external-project.toml).
2. Set `project` to the checkout path (absolute, or relative to this
   Orbit repo root).
3. Set `build.command`, `bench.command`, optional `correctness.command`,
   and `capture.command` (the child Orbit attaches to — a PID, not a
   protocol).
4. Make the bench command print a primary metric, one of:
   - a line `ORBIT_METRIC throughput=123456`
   - `bench.metric.regex` with a named group `value`
   - `bench.metric.json_path` if stdout is JSON
5. Run:

```sh
python3 tools/optimize-loop/suite.py --config path/to/your.toml
python3 tools/optimize-loop/loop.py --mode live --config path/to/your.toml
```

`--config` defaults to the in-repo fixture.

## Demo (no external clone)

```sh
python3 tools/optimize-loop/suite.py --config docs/optimize-loop/examples/fixture.toml
python3 tools/optimize-loop/suite.py --capture none --bench-iters 4
python3 tools/optimize-loop/loop.py --mode mock
python3 tools/optimize-loop/mcp/server.py --list-tools
python3 tools/optimize-loop/mcp/smoke.py
```

Cursor: copy [`tools/optimize-loop/mcp/cursor-mcp.example.json`](../../tools/optimize-loop/mcp/cursor-mcp.example.json)
to `.cursor/mcp.json`. There is no `orbit mcp` CLI. LLM keys
(`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `XAI_API_KEY`, or Cursor login)
stay on the host.

## MCP tools

| Name | Role |
| --- | --- |
| `optimize_run_suite` | Wrap `suite.py` |
| `optimize_get_summary` | Latest `summary.json` |
| `optimize_inspect_hotspots` | Profile table |
| `optimize_apply_patch` | Dry-run (default) or write under the configured project |
| `optimize_rebuild` | `build.command` |
| `optimize_rerun_compare` | Re-run vs baseline |
| `optimize_run_loop` | Closed loop (`mode=mock` is CI-safe) |

Patches cannot escape `project` / `patch_root` or touch `.git`. Nothing
is committed or force-pushed.

## Gate

Accept only if **all** hold:

| Parameter | Default | Rule |
| --- | --- | --- |
| `min_gain_percent` | **0.5** | after mean must beat baseline by more than 0.5% |
| `sigma` | **1.0** | when baseline stdev exists, delta must be **> 1σ** |
| fingerprint / correctness | required | `ORBIT_FINGERPRINT` must match; correctness must not fail |

Reject restores the experiment files in the target tree.

## Capture quality (honesty)

Orbit serve-mode on Linux VMs has been seen to load symbols and still
record **0 callstack samples**. File-mode samples one tid (the launched
leader). Leaves are often libc/runtime until attach happens on the
threads that actually do the work, after the workload starts. The loop
will not invent a performance win from that. `--proposal auto` applies a
labeled **no-op** when hotspots look poor.

Better sampling: `kernel.perf_event_paranoid` ≤ 1 (≤ 0 system-wide);
`setcap cap_perfmon,cap_sys_ptrace+ep` on `orbit-service`; attach after
the child is in its hot path; a matching `linux-perf` package when the
kernel has one.

## API keys

This toolchain does not read secrets. Do not put keys in the MCP `env`
block.

## Logs

Suite outputs: `docs/optimize-loop/runs/` (gitignored).
Attempt log: [`loop-log/`](loop-log/) (small JSON/MD, committed).
