# Plan log — Stockfish, iterating from the baseline (post 25)

Each entry: what was measured, what it changed in the plan. Newest last.

## 1. Counters before code (instrumented build, bench 128 1 13, fingerprint 1659671 unchanged)

```
apply_combined calls=1,427,087  psq/call=2.34  thr/call=6.06  psq_pairs=193  thr_pairs=48,076
thr per call histogram (<8,<16,<24,<32,<48,<64): 915k 420k 67k 21k 3.9k 16
paths: both=411,959 incr=192,285 hybrid=13,604 refresh=217,131  incremental steps=842,427
refresh: 5.3 psq + 5.6 threat columns each
```

- **Candidate 2 (cancel add/remove pairs) → dropped without coding.** Pairs are
  ~0.5 % of threat entries (48k of 8.6M) and 193 of 3.3M psq entries. Upper
  bound ≈ 0.2 % of `apply_combined`. Not worth a patch.
- **Threat lists are short** (6 per update, two thirds under 8), so the bytes
  per update are dominated by the 2.3 psq columns (2 KiB each) — consistent
  with PEBS putting the misses on the psq loads.
- **Refresh is 35 % of per-side evaluations** (217k vs 192k+2×412k), hybrid
  takes only 13.6k. A refresh costs about as many columns as an incremental
  update (5.3 psq + 5.6 threat vs 2.3 + 6), so it is not a cost outlier per
  call; it is the *count* that is high. Candidate 1 becomes: can more of
  those 217k go through the hybrid path (bit-exact by construction)?
- **Open question raised:** the psq working set for one king bucket is
  ~1.4 MB (fits L2), yet its loads miss L3. Something evicts it. The prime
  suspect is the hash table: one random 64-byte line per node, 128 MB, all of
  it streamed through L2/L3 at temporal priority. A non-temporal TT prefetch
  is a one-line, bit-exact experiment. Added as candidate 7.

## 2. Reading the code the counters pointed at

- `KingBuckets` gives every king square its own bucket (mirror pairs share):
  **every king move changes every psq index**, so a "same-bucket king move →
  cheap incremental update" idea is impossible by construction; the refresh
  count is inherent to the feature set. The overview's candidate 3 ("lay the
  table out bucket-major") is also moot: `make_index` is already bucket-major
  (`KingBuckets[ksq] + PieceSquareIndex[pc] + oriented square`). Struck.
- `update_accumulator_hybrid` is exact by construction (parent accumulator +
  old/new cache entries + psq diffs) and gated by `MIN_PC_COUNT_HYBRID = 15`
  and same-half. Candidate 1 reduces to moving that threshold.
- Threat indices depend on the king only through orientation (`OrientTBL`),
  as the source comment says; nothing to gain there.

## 3. Three quick A/Bs (6 pinned alternating pairs each; fingerprint bit-exact in all three)

| experiment | instructions | cycles | nps | read |
| --- | --- | --- | --- | --- |
| E1a `MIN_PC_COUNT_HYBRID` 15 → 0 | +0.29 % | +1.02 % (1/6) | −0.94 % | hybrid is dearer than refresh when pieces are few — the 15 is right-ish |
| E1b hybrid never (33) | +0.71 % | −0.55 % (4/6) | +0.69 % (4/6) | more instructions, slightly fewer cycles: hybrid touches two cache entries + the parent; refresh streams less. Borderline — sent to the gate |
| E2 TT prefetch non-temporal (both sites) | −0.01 % | +0.46 % (2/6) | −0.54 % | the "TT evicts the weights" theory gets no support from a hint change |

**Plan update.** Candidates 1 (as widening), 2 and 3 are closed by data; E2 is
closed. What is left with domain: the psq columns still miss, and the cause
is now more likely the *threat* table (61 MB, 6 sparse columns per update)
and the hash table together streaming through a 36 MB L3 shared with 16
E-cores — not something a hint fixes. The remaining bit-exact lever with
real domain is structural: **merging consecutive incremental steps** (842k
steps for 604k updates — 17 % of `apply_combined` calls are chain steps
that materialize an intermediate accumulator nobody may read). Upper bound
≈ 1–2 %, with a real risk of paying it back when a sibling subtree needs
the skipped state. Deferred: it is a day's work for a bounded ~1 %.

## 4. Gate: hybrid-never

Clean builds, 10 runs, 3 noise batches, pinned. Baseline 1,389,938 ± 0.60 %,
floor 0.58 %; candidate 1,385,985 → **+0.06 %, reject**. The quick A/B's
+0.69 % (4/6) was noise, as its own numbers half-said. Fingerprint
`af278ac38b7025a4-1659671` unchanged throughout.

## Where this leaves the plan

Closed by data this round: 1 (both directions), 2, 3, 7 (TT hint). Open, in
order: step-merging in the accumulator chain (bounded ~1–2 %, a day), lazy
threat bookkeeping (≤ 2.7 %, needs the previous position's attack maps kept
alive), and everything else is below the box's 0.6–0.8 % floor. The plain
build's one large, bit-exact lever remains the profile-guided build
(+4.3 % gated, post 24). For code-level wins above 1 % on this engine the
honest reading of the profile is: change what is computed (Elo work with the
fishtest gate), not how.
