#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Checks the hooked (dynamically instrumented) scopes of a capture bundle.

Reads an exported `.zip` bundle (manifest.json + events.parquet), finds every
point where a hooked function's depth changes on a thread -- the signature of
a lost or doubled probe hit -- and lines each up with the thread's scheduler
slices, so a change that follows a migration says so.

    python3 tools/check_hooked_scopes.py capture.zip [--pid N] [--show 12]

Needs pyarrow and pandas.
"""
import argparse
import io
import json
import zipfile

import pyarrow.parquet as pq

API_SCOPE, SCHEDULING_SLICE = 1, 3


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("bundle")
    ap.add_argument("--pid", type=int, help="the process to check (default: the bundle's target)")
    ap.add_argument("--show", type=int, default=12, help="depth changes to print per thread")
    args = ap.parse_args()
    with zipfile.ZipFile(args.bundle) as z:
        manifest = json.loads(z.read("manifest.json"))
        events = pq.read_table(io.BytesIO(z.read("events.parquet"))).to_pandas()
    pid = args.pid or manifest["bundle"].get("target_pid")
    api = events[(events.kind == API_SCOPE) & (events.pid == pid)].copy()
    api["end"] = api.start_ns + api.duration_ns
    sched = events[(events.kind == SCHEDULING_SLICE) & (events.pid == pid)].copy()
    sched["end"] = sched.start_ns + sched.duration_ns
    print(f"pid {pid}: {len(api)} hooked scopes on {api.tid.nunique()} threads")
    print(api.groupby(["name", "depth"]).size().to_string())
    print()
    total = 0
    for tid, g in api.groupby("tid"):
        g = g.sort_values("start_ns")
        last, changes = {}, []
        for r in g.itertuples():
            d = last.get(r.name)
            if d is not None and d != r.depth:
                changes.append((r.start_ns, r.name, d, r.depth))
            last[r.name] = r.depth
        s = sched[sched.tid == tid].sort_values("start_ns")
        slices = list(zip(s.start_ns.tolist(), s.end.tolist(), s.extra.tolist()))
        migrations = sum(1 for i in range(1, len(slices)) if slices[i][2] != slices[i - 1][2])
        print(f"tid {tid}: {len(changes)} depth changes, {len(slices)} slices, {migrations} migrations")
        total += len(changes)
        for t, name, before, after in changes[: args.show]:
            # The slice the thread was in when the depth changed, and the one before it.
            i = next((k for k, (a, b, _) in enumerate(slices) if a <= t < b), None)
            where = ""
            if i is not None and i > 0:
                a, b, core = slices[i]
                pa, pb, pcore = slices[i - 1]
                if core != pcore:
                    where = f"  <- migrated core {pcore} -> {core}, resumed {t - a} ns before, off-cpu {a - pb} ns"
                else:
                    where = f"  (on core {core}, resumed {t - a} ns before)"
            kind = "an entry was lost (or a return doubled)" if after < before else "a return was lost (or an entry doubled)"
            print(f"   {t}  {name} depth {before} -> {after}: {kind}{where}")
        if len(changes) > args.show:
            print(f"   ... {len(changes) - args.show} more")
    print(f"\n{total} depth changes in all; 0 is what a clean capture shows.")


if __name__ == "__main__":
    main()
