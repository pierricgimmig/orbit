## What the capture shows

The capture is ten seconds of `stockfish bench 256 4 22` — the standard bench
positions searched to depth 22 with four threads and a 256 MB hash — on a
plain (non-PGO) `ARCH=native` build, sampled on every thread with four
functions hooked. Read the two tables above together:

- **Five threads, four working.** The main thread parses the bench, hands each
  position to the pool and waits; it has no samples. The four search workers
  have 6,510–6,520 samples each: the work is perfectly balanced, as Lazy SMP
  intends (every thread searches the whole tree; they share only the hash).
  The scheduler track records 38.4 s of CPU in 10.1 s of wall time.
- **The hooks give the skeleton.** `Engine::go` fires once per position and
  returns in 0.16 ms — it only wakes the pool. `ThreadPool::start_thinking`'s
  per-thread lambda fires 28 times (7 positions × 4 threads). Each thread then
  runs one `Worker::iterative_deepening` per position: 24 completed inside the
  window, 1.5 s on average, 0.19–3.9 s. Inside each, `search<Root>` runs once
  per aspiration-window attempt per depth: 2,111 calls, 18 ms on average, the
  longest 1.49 s (a depth-22 root search on a wide position). Everything below
  root is too frequent to hook and is what the samples describe.

## How Stockfish works, as the profile sees it

```
bench / UCI "go"
 └─ Engine::go ──────────────── 7 calls, async: wakes the pool and returns
     └─ ThreadPool::start_thinking ── one wake-up per worker (28)
         └─ Worker::iterative_deepening ── per thread per position (24 in window)
             └─ search<Root> ─────────── per depth × aspiration attempt (2,111)
                 └─ search<PV> / search<NonPV> ── the recursion: 98.7 % inclusive
                     ├─ TranspositionTable::probe ── 2.3 % self, one random cache line per node
                     ├─ static eval: Eval::evaluate → NNUE::Network::evaluate
                     │    ├─ AccumulatorStack::evaluate ── 38.3 % inclusive
                     │    │    ├─ apply_combined ──────── 37.4 % self  (incremental update: add/sub weight columns)
                     │    │    ├─ update_accumulator_refresh_cache ── 4.6 % (king moved: rebuild from the finny cache)
                     │    │    └─ update_accumulator_hybrid ──── 2.7 % (king moved a little: partial rebuild)
                     │    └─ NetworkArchitecture::propagate ── 10.0 % self (sparse affine + clipped-ReLU layers)
                     ├─ MovePicker::next_move ── 12.6 % inclusive (staged generation, SEE, history scoring)
                     │    └─ partial_insertion_sort ── 3.4 % self
                     ├─ Position::do_move ── 7.3 % inclusive
                     │    └─ update_piece_threats ── 2.7 % (threat bookkeeping for the net's threat features)
                     ├─ correction_value ── 1.8 % (eval correction histories)
                     └─ qsearch ── captures-only tail of the tree (inside the recursion above)
```

Three things stand out, and they set the agenda.

**Evaluation is 60 % of the time, and one function is 37 % of it.**
`NNUE::Network::evaluate` is 60.5 % inclusive; the network is evaluated at
almost every node (`qsearch` alone is 27.5 % inclusive). Its first layer, the *feature
transformer*, is an accumulator of `i16` sums that is maintained
incrementally: a move adds and removes a handful of piece-square features
(2 KiB `i16` columns) and a few dozen threat features (1 KiB `i8` columns),
and `apply_combined` does exactly that — load a tile of the parent's
accumulator, add and subtract columns, store the child's. The 46 MB
piece-square table and the 61 MB threat table are both larger than the 36 MB
L3, so much of that 37 % is memory latency: a precise-event profile (`perf
mem_load_retired.l3_miss`) put 49 % of the program's L3 misses in this one
function and 35 % in the refresh path. The rest of eval is `propagate`, the
small dense layers on top: 10 %, compute-bound, already VNNI.

**Search and move ordering are 22 %.** `search` itself (7.5 % self) is the
alpha-beta recursion with its pruning heuristics; `MovePicker` (6.6 % self,
12.6 % inclusive) generates moves in stages and scores them from history
tables; `partial_insertion_sort` (3.4 %) orders the quiet moves; `do_move`
(7.3 % inclusive) updates the board, the Zobrist key and — new in this
generation of nets — the list of attacked pieces the threat features need.

**The hash table is 2.3 % and already prefetched.** `probe` is one random
64-byte line per node; the search issues a prefetch for the child's key as
soon as the move is known. There is little left there.

## The constraint: bit-exact output

Only a change that leaves the engine's output **bit-exact** can be accepted.
The bench fingerprint is the SHA-256 of the entire `bench` output with the
timing tokens removed: every position's per-depth node count, score,
principal variation and best move. Anything that alters the search — a
pruning margin, a history bonus, an ordering tie-break, an eval rounding —
changes the node count somewhere in 46 positions and is rejected by
construction. This is a feature: it rules out "optimizing" by playing
differently, and it means the candidates below are all *the same
computation, done faster*.

The measurement is the loop's gate (mean gain above the measured
run-to-run noise floor and above 1σ, fingerprint identical), confirmed by an
interleaved uninstrumented A/B, bench pinned to one performance core.
Retired instructions (`perf stat -e instructions:u`) are read alongside:
they are deterministic to five digits and say whether a change did less
work before anyone argues about whether it was faster.

## Candidate wins, design level

Ordered by the size of the domain they act on and how safely they preserve
the output. The percentages are the share of samples the candidate can
touch, i.e. upper bounds, not predictions.

### 1. The accumulator update policy — bit-exact by construction (domain 45 %)

The accumulator is an exact integer sum of feature columns. Whether a
position's accumulator is built incrementally from its parent, from a
grandparent (skipping a ply), from the finny cache entry for its king
square, or by the hybrid path, **the resulting bits are identical** — that is
why Stockfish can already mix these paths freely. Every knob in that policy
is therefore free to retune for speed with no fingerprint risk:

- `find_last_usable_accumulator` walks back to the nearest computed state;
  the choice between one big update from a far ancestor and a chain of small
  ones is a cost model, not a correctness rule.
- The refresh path (4.6 % self, 35 % of L3 misses) rebuilds from a cache
  keyed by king square. A finer key (king square × a second dimension, or
  more entries per square) trades memory for fewer columns per refresh.
- `MIN_PC_COUNT_HYBRID` and the hybrid condition decide when a king move gets
  a partial rebuild instead of a full one.

Cheap to test, gated in minutes each. Expect low single digits.

### 2. Cancel add/remove pairs in the feature lists (domain 37 %)

`apply_combined` receives *added* and *removed* index lists and applies both
in full. A feature that appears in both lists costs two column passes
(2 × 4 tiles) and contributes nothing: integer add then subtract of the same
column is the identity, exactly, including on wrap-around. Whether such
pairs occur — the threat diff for a capture, a piece stepping through a
square it already attacked — is a five-line counter to find out. If they are
a few percent of list entries, removing them before the tile loop is a
proportional, bit-exact saving in the hottest function.

### 3. Locality of the piece-square weight table (domain: the 49 % of misses)

Software prefetch of the piece-square columns was tried three ways (a burst
before the call, next-chunk inside the tile loop, and at `do_move` with real
lead time) and all three lost: the eight loads of a chunk already issue
together, so the latency is overlapped, and the prefetches only added
instructions and pollution. The remaining lever is **layout**: the table is
`[feature][1024] i16`, indexed by (king bucket, piece, square). A search
rarely leaves one king bucket for long, so ordering the table bucket-major
keeps the working set of one bucket contiguous; laying the two perspectives'
columns for the same (piece, square) side by side turns two misses into one
line pair. Same bits in a different order — the output is unchanged. The
gain is bounded by the miss share and needs measuring, not guessing.

### 4. `partial_insertion_sort` and move ordering (domain 3.4 % + 6.6 %)

The sort must produce the identical permutation, ties included — and it
does so today with a specific insertion order. Any implementation that
reproduces that order exactly (a branch-free insertion, or sorting only as
many moves as the node actually consumes — most nodes cut off after one or
two) is accepted by the fingerprint and rejected the instant it differs by
one swap. Lazy selection is the interesting one: the cut-off statistics say
how many ordered moves a node needs on average, and that number is small.

### 5. Lazy threat bookkeeping in `do_move` (domain 2.7 %)

`update_piece_threats` runs for every move made, eagerly, to feed the threat
features. A node whose static eval comes from the hash table never touches
its accumulator, so its threat diff was computed for nothing. Deferring the
computation to the accumulator update (it depends only on the position)
saves exactly that fraction; the bench's TT-eval rate says how much. The
diff itself is a pure function of the board — bit-exact.

### 6. Startup: decoding the embedded network (8 % of a short run's cycles)

`read_leb_128` — inflating the LEB128-encoded weights embedded in the
binary — is 8 % of the cycles of a one-second bench and 0 % of the reported
nodes/second, because it happens before the clock starts. It matters to
tools that launch the engine often (test harnesses, this loop) and to
nothing else. A SIMD decoder or a raw-weights section is bit-exact and
easy; it is listed so it is not mistaken for a hot spot in a short capture.

### Not on the table

Anything that changes what is computed: move ordering heuristics, pruning,
eval values, the net architecture, the number of threat features per
update. Those are Elo questions with a different gate (fishtest), not
speed questions with this one.

## The loop from here

Each candidate is one iteration: read the source, write the patch, put it
through `optimize_apply_patch → optimize_rebuild → optimize_rerun_compare`,
gate it, confirm an accept with an interleaved uninstrumented A/B, and
record accept or reject with the numbers. Candidates 1 and 2 first: they are
cheapest to test and safest by construction.
