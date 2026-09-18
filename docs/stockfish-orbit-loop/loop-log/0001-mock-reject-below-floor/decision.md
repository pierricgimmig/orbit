# Loop attempt 0001 — REJECT

- mode: `mock`
- proposal: `mock`
- decision: **reject**
- reverted: True

- bench nps: 1000000.0 → 1002000.0 (+0.200%, +2000)
- mean nps gain 0.200% is not > 0.5%
- delta 2,000 nps is not outside 1σ (threshold 15,000 nps from baseline stdev)
- hotspot quality: poor
- mock fixture; no capture

