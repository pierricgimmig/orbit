#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Open an Orbit capture saved as Arrow.

Orbit's live viewer saves a capture as ``capture.orbit.zip`` (the Save pill;
Save slice does the same for the selected time range). That is a store-only
zip of a dataset directory -- events, samples and frames tables plus a
manifest that also names every thread and process -- so ``unzip`` it and
point this script at the directory. ``GET /api/capture/export`` still hands
out the events table alone as Arrow IPC (``?format=ipc``) or Parquet
(``?format=parquet``), and ``orbit-service --out-arrow <dir>`` writes a
dataset directory straight to disk. All are plain Arrow / Parquet, so there
is nothing Orbit-specific to install: pyarrow reads them, and they drop
straight into pandas.

    unzip capture.orbit.zip -d my-capture
    python open_capture.py my-capture/         # dataset directory (or an unzipped bundle)
    python open_capture.py capture.arrow       # Arrow IPC file
    python open_capture.py capture.parquet     # Parquet file

The events table -- one row per event, these columns:

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

    import os

    def read_ipc(file_path: str) -> "pa.Table":
        # `open_file` is the reader for the IPC *file* format Orbit writes (as
        # opposed to the IPC *stream*). The file holds several record batches
        # of 65,536 rows; `read_all` concatenates them, or use `get_batch(i)`
        # to stream a big capture one batch at a time.
        with pa.memory_map(file_path, "r") as source:
            return ipc.open_file(source).read_all()

    # Three shapes of capture: one Parquet file, one Arrow IPC file, or a
    # dataset directory with a manifest naming the tables inside it.
    dataset_dir = path if os.path.isdir(path) else None
    if dataset_dir:
        import json

        with open(os.path.join(dataset_dir, "manifest.json")) as fh:
            manifest = json.load(fh)
        print(f"{path}: dataset {manifest['format']}, rows {manifest['rows']}")
        table = read_ipc(os.path.join(dataset_dir, manifest["files"]["events"]))
    elif path.endswith(".parquet"):
        import pyarrow.parquet as pq

        table = pq.read_table(path)  # same columns, other container
    else:
        table = read_ipc(path)

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

    # --- A dataset also carries the sampled callstacks ---------------------
    if dataset_dir:
        samples = read_ipc(os.path.join(dataset_dir, manifest["files"]["samples"]))
        frames = read_ipc(os.path.join(dataset_dir, manifest["files"]["frames"]))
        # frames: id -> (name, module, address). The command-line capture path
        # fills addresses but not names; a name is present when the service
        # symbolized the frame.
        by_id = {
            fid: (name, module, addr)
            for fid, name, module, addr in zip(
                frames.column("id").to_pylist(),
                frames.column("name").to_pylist(),
                frames.column("module").to_pylist(),
                frames.column("address").to_pylist(),
            )
        }
        leaves: dict = {}
        stacks = samples.column("frames").to_pylist()
        for stack in stacks:
            if stack:  # frames are innermost first, so [0] is the leaf
                leaves[stack[0]] = leaves.get(stack[0], 0) + 1
        print(f"\nsamples: {samples.num_rows:,}  frames: {frames.num_rows:,}")
        print("hottest leaf frames (self samples):")
        for fid, count in sorted(leaves.items(), key=lambda kv: -kv[1])[:8]:
            name, module, addr = by_id.get(fid, ("?", "?", 0))
            label = name or f"0x{addr:x}"
            print(f"  {count:6d}   {label}  {module}".rstrip())

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
