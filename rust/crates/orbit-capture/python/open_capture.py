#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Open an Orbit capture saved as an Arrow IPC file.

Orbit's live viewer can Save the current capture as ``capture.arrow`` (the Save
pill, or ``GET /api/capture/export``). The file is a plain Arrow IPC file, so
there is nothing Orbit-specific to install: pyarrow reads it, and it drops
straight into pandas.

    python open_capture.py capture.arrow

One table, one row per event, these columns:

    start_ns      u64   event start, nanoseconds on the capture clock
    duration_ns   u64   span length -- but see the two kinds below where it is
                        NOT a duration
    pid           u32   process id
    tid           u32   thread id
    kind          u8    what the row is; see KINDS
    depth         u8    nesting depth of a scope within its thread
    extra         u8    core id (scheduling slices) or thread-state code
    name_id       u32   interned name id (join key, kept for fidelity)
    name          str   the name already resolved -- use this, no join needed

Two kinds reuse ``duration_ns`` for something other than a duration, which is
the one thing worth knowing before you sum it:

  * VALUE (6): a timestamped scalar. ``duration_ns`` holds the float's bits,
    ``f32::from_bits``. ``value_of()`` below unpacks it.
  * SAMPLE (7): a sampled callstack tick. ``duration_ns`` is the sampling
    period, not a measured duration.

So filter to API_SCOPE / FUNCTION_CALL before you add durations up.
"""

import struct
import sys

# kind codes, from orbit-live-event's `kind` module (the source of truth).
KINDS = {
    1: "API_SCOPE",
    2: "FUNCTION_CALL",
    3: "SCHEDULING_SLICE",
    4: "THREAD_STATE",
    5: "API_TRACK",
    6: "VALUE",
    7: "SAMPLE",
}

# Kinds whose duration_ns really is a span you can sum.
SPAN_KINDS = {1, 2, 3}  # API_SCOPE, FUNCTION_CALL, SCHEDULING_SLICE


def value_of(duration_ns: int) -> float:
    """Unpack a VALUE row's float from the bits stashed in ``duration_ns``."""
    return struct.unpack("<f", struct.pack("<I", duration_ns & 0xFFFFFFFF))[0]


def main(path: str) -> int:
    try:
        import pyarrow as pa
        import pyarrow.ipc as ipc
    except ImportError:
        print(
            "This example needs pyarrow:  pip install pyarrow  (pandas is optional).",
            file=sys.stderr,
        )
        return 1

    # Read the whole file into one Arrow table. `open_file` is the reader for
    # the IPC *file* format Orbit writes (as opposed to the IPC *stream*).
    with pa.memory_map(path, "r") as source:
        table = ipc.open_file(source).read_all()

    print(f"{path}: {table.num_rows:,} events, {table.num_columns} columns")
    print("schema:")
    for field in table.schema:
        print(f"  {field.name:<12} {field.type}")

    # --- Plain Arrow: a couple of answers without pandas -------------------
    kinds = table.column("kind").to_pylist()
    names = table.column("name").to_pylist()
    durations = table.column("duration_ns").to_pylist()
    tids = table.column("tid").to_pylist()

    by_kind: dict[str, int] = {}
    for k in kinds:
        by_kind[KINDS.get(k, f"?{k}")] = by_kind.get(KINDS.get(k, f"?{k}"), 0) + 1
    print("\nrows by kind:")
    for name, count in sorted(by_kind.items(), key=lambda kv: -kv[1]):
        print(f"  {name:<18} {count:,}")

    # Hottest scopes by total time, summing only the kinds where duration_ns
    # is actually a duration.
    totals: dict[str, int] = {}
    for k, name, dur in zip(kinds, names, durations):
        if k in SPAN_KINDS and name:
            totals[name] = totals.get(name, 0) + dur
    if totals:
        print("\ntop scopes by total duration:")
        for name, total in sorted(totals.items(), key=lambda kv: -kv[1])[:10]:
            print(f"  {total / 1e6:10.3f} ms   {name}")

    # A VALUE row, decoded, if the capture has any.
    for k, name, dur in zip(kinds, names, durations):
        if k == 6:  # VALUE
            print(f"\nexample VALUE row: {name} = {value_of(dur):g}")
            break

    print(f"\ndistinct threads: {len(set(tids))}")

    # --- pandas, if it is installed ---------------------------------------
    try:
        df = table.to_pandas()
    except ImportError:
        print("\n(install pandas to get the DataFrame view)")
        return 0

    df["kind_name"] = df["kind"].map(KINDS).fillna("?")
    spans = df[df["kind"].isin(SPAN_KINDS)].copy()
    spans["ms"] = spans["duration_ns"] / 1e6
    print("\npandas: mean scope length per thread (top 5 threads by count)")
    top_threads = spans["tid"].value_counts().head(5).index
    view = spans[spans["tid"].isin(top_threads)]
    print(view.groupby("tid")["ms"].agg(["count", "mean", "max"]).round(3))
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(__doc__)
        print("usage: python open_capture.py <capture.arrow>", file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1]))
