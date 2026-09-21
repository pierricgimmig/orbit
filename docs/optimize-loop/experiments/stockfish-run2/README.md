# Stockfish, run 2 — the record

Files from the second optimize-loop run on Stockfish (see
`../stockfish.md` for the write-up and `docs/blog/23-*.html` for the story).
They are a **record**, not a runnable kit: paths that pointed into the
session's scratch directory are written as `<scratch>`.

| file | what |
| --- | --- |
| `trace.md` | chronological log, appended as each step ran; every number in the blog post comes from here |
| `stockfish.toml`, `stockfish-pgo.toml` | the two target configs (plain build / `profile-build`); build commands clean first |
| `sf-bench.sh`, `sf-capture.sh` | bench harness (emits `ORBIT_METRIC` nps + `ORBIT_FINGERPRINT` nodes, pinned to one P-core) and the capture workload |
| `experiment_driver.py` | drives patches or build variants through the MCP tools: baseline + noise batches → apply_patch → rebuild → rerun_compare → gate → revert |
| `bench_rr.py` | interleaved, uninstrumented round-robin benchmark with paired deltas and win counts |
| `apply-combined-restrict.patch`, `refresh-restrict.patch` | the retracted `__restrict` changes (5 bytes / 3 instructions of codegen difference) |
| `psq-prefetch.patch`, `psq-chunk-prefetch.patch`, `domove-prefetch.patch` | the three prefetch experiments, all rejected |
| `experiments.json` | gate results of the last driver run (PGO accept) |
| `rr-final-16.json` | the 16-round e2e confirmation: baseline / -mtune=native / PGO |
| `rr-restrict-pinned-16.json` | the 16-round pinned run that failed to reproduce the `__restrict` win |
