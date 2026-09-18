# Phase 2 suite — recorded run

Cloud VM, 2026-09-18. Stockfish
`031dfeb437fa6b06cdbdf4ef89dfb82f6b83c4d3` (`stockfish-dev-20260913-031dfeb4`),
`g++` 13.3.0, native `x86-64-avx512icl`, 4 CPUs.

```sh
python3 tools/stockfish-orbit-loop/run_suite.py
```

## Speedtest (primary)

`./stockfish speedtest 4 512 20` (official default is 150 s)

| | |
| --- | --- |
| Nodes | 102,054,177 |
| Time | 19.843 s |
| **Nodes/second** | **5,143,082** |

## Repeated bench (secondary)

`./stockfish bench 16 1 13 default depth` × 20

| | |
| --- | --- |
| Nodes searched (fingerprint) | 1,648,567 |
| **nps mean ± stdev** | **1,408,300 ± 20,734** |
| nps min / max | 1,365,838 / 1,437,285 |
| time ms mean ± stdev | 1170.8 ± 17.3 |

## Perft (correctness)

`--perft smoke` (official `tests/perft.sh` positions, reduced depth): **all 6 passed**.

Official `./tools/stockfish-orbit-loop/perft_official.sh` is wired (`expect` installed) but not run here — depth-7 cases search billions of nodes.

## Profile (this Orbit)

`--capture auto` built and launched **this** repo's `orbit-service` (Rust 1.88 release).

| Path | Result on this VM |
| --- | --- |
| HTTP `POST /api/symbols/load` | 7740 functions / 6 modules, status `ready` |
| HTTP `POST /api/capture/start` + `GET /api/sampling/report` | Capture starts; **0 callstack samples** (rings open at `perf_event_paranoid=1`; scheduling needs 0) |
| File mode `orbit-service --pid --duration-ms --out capture.pod` | **356 samples**, 58 interned callstacks; leaf PCs symbolized via `/proc/pid/maps` + `addr2line` |
| `perf` CLI | Not installed (no `linux-perf` package for kernel 6.12.94+) |

Symbolized file-mode leaf table from a follow-up capture (same binary):

| % self | symbol |
| ---: | --- |
| 48.15 | `Stockfish::Eval::NNUE::read_leb_128_detail<…>` |
| 37.37 | `__nss_database_lookup` |
| 12.46 | `__madvise` |
| 1.01 | `__munmap` |
| 1.01 | `Stockfish::hash_bytes` |

File-mode samples the UCI thread-group leader. Attaching to a search worker tid returned 0 samples even under `sudo`. Serve-mode loaded symbols but did not record callstacks. Those are the blockers for a search-dominated Orbit flame graph on this VM; the harness keeps both backends pluggable.

## Before / after

```sh
python3 tools/stockfish-orbit-loop/run_suite.py --baseline path/to/summary.json
```

A 3-iteration re-bench against the 20-iteration baseline printed
`bench mean nps: +0.51% (+7,248)` (noise, as expected).
