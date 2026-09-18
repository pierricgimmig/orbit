# Loop attempt 0005 — REJECT

- mode: `live`
- proposal: `noop_experiment`
- decision: **reject**
- reverted: True

- bench nps: 1365859.1 → 1388785.8 (+1.679%, +22927)
- delta 22,927 nps is not outside 1σ (threshold 59,898 nps from baseline stdev)
- hotspot quality: usable (classifier v1 false positive: `ThreadPool::set` demangles with `Stockfish::Search::SharedState`; later classifier treats libc/startup first)
- top leaves were still `__nss_database_lookup` / `__madvise` / NNUE load — not a search hot path
- proposal was a two-line comment in `src/misc.cpp`; fingerprint stayed 1,648,567
- +1.68% is run-to-run noise (baseline stdev 59,898 nps after one slow iter); gate rejected; tree reverted

