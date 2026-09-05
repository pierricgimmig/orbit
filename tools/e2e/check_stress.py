#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Checks a capture of `OrbitTestRust --stress-*` call for call.

The program's tree is known: each thread makes K `orbit_stress_outer` calls,
each outer two `orbit_stress_middle`, each middle three `orbit_stress_inner`.
Given the exported bundle, this counts what the capture holds, per function
and per thread, checks every scope's depth against its place in the tree and
its containment in its parent, and prints one JSON object the e2e scenario
asserts on. Needs pyarrow; run it with ORBIT_E2E_PYARROW_PYTHON.

    check_stress.py bundle.zip --pid N --threads T --calls K
    check_stress.py --self-test
"""
import argparse
import io
import json
import sys
import zipfile

API_SCOPE = 1
TREE = [("orbit_stress_outer", 0, 1), ("orbit_stress_middle", 1, 2), ("orbit_stress_inner", 2, 6)]
NAMES = {name: (depth, per_outer) for name, depth, per_outer in TREE}


def analyse(rows, threads, calls):
    """`rows`: (tid, name, depth, start_ns, duration_ns) of the hooked scopes.
    Returns the verdict as a dict."""
    per_name = {name: 0 for name, _, _ in TREE}
    per_thread = {}
    depth_wrong = {name: 0 for name, _, _ in TREE}
    not_contained = {name: 0 for name, _, _ in TREE}
    extras_in_parent = 0
    durations = {name: [] for name, _, _ in TREE}
    by_tid = {}
    for tid, name, depth, start, dur in rows:
        if name not in NAMES:
            continue
        by_tid.setdefault(tid, []).append((start, start + dur, name, depth))
    for tid, scopes in by_tid.items():
        scopes.sort(key=lambda s: (s[0], -s[1]))
        stack = []  # (end, name)
        counts = {name: 0 for name, _, _ in TREE}
        for start, end, name, depth in scopes:
            while stack and stack[-1][0] <= start:
                stack.pop()
            expected_depth, _ = NAMES[name]
            per_name[name] += 1
            counts[name] += 1
            durations[name].append(end - start)
            if depth != expected_depth:
                depth_wrong[name] += 1
            parent = {"orbit_stress_outer": None, "orbit_stress_middle": "orbit_stress_outer",
                      "orbit_stress_inner": "orbit_stress_middle"}[name]
            if parent is not None:
                if not stack or stack[-1][1] != parent or stack[-1][0] < end:
                    not_contained[name] += 1
            elif stack:
                extras_in_parent += 1
            stack.append((end, name))
        per_thread[tid] = counts
    expected = {name: threads * calls * per_outer for name, _, per_outer in TREE}
    missing = {name: expected[name] - per_name[name] for name in expected}
    extra = {name: max(0, per_name[name] - expected[name]) for name in expected}
    def stats(v):
        if not v:
            return {}
        v = sorted(v)
        return {"min_ns": v[0], "p50_ns": v[len(v) // 2], "max_ns": v[-1]}
    return {
        "expected": expected,
        "observed": per_name,
        "missing": missing,
        "extra": extra,
        "missing_total": sum(missing.values()),
        "extra_total": sum(extra.values()),
        "depth_wrong": depth_wrong,
        "depth_wrong_total": sum(depth_wrong.values()),
        "not_contained": not_contained,
        "not_contained_total": sum(not_contained.values()),
        "outer_inside_something": extras_in_parent,
        "threads_seen": len(per_thread),
        "threads_expected": threads,
        "per_thread": {str(t): c for t, c in sorted(per_thread.items())},
        "durations": {name: stats(v) for name, v in durations.items()},
    }


def rows_from_bundle(path, pid):
    import pyarrow.parquet as pq
    with zipfile.ZipFile(path) as z:
        table = pq.read_table(io.BytesIO(z.read("events.parquet")),
                              columns=["start_ns", "duration_ns", "pid", "tid", "kind", "depth", "name"])
    cols = {c: table.column(c).to_pylist() for c in table.column_names}
    rows = []
    for i in range(table.num_rows):
        if cols["kind"][i] != API_SCOPE or cols["pid"][i] != pid:
            continue
        rows.append((cols["tid"][i], cols["name"][i], cols["depth"][i], cols["start_ns"][i], cols["duration_ns"][i]))
    return rows


def self_test():
    # One thread, two outer calls, the second with a lost inner (missing)
    # and a middle at the wrong depth.
    o, m, i = "orbit_stress_outer", "orbit_stress_middle", "orbit_stress_inner"
    rows = []
    t = 0
    for call in range(2):
        rows.append((7, o, 0, t, 100))
        for mm in range(2):
            ms = t + 5 + mm * 45
            rows.append((7, m, 1 if not (call == 1 and mm == 1) else 2, ms, 40))
            for ii in range(3):
                if call == 1 and mm == 1 and ii == 2:
                    continue  # lost
                rows.append((7, i, 2, ms + 2 + ii * 12, 10))
        t += 200
    v = analyse(rows, threads=1, calls=2)
    assert v["expected"] == {o: 2, m: 4, i: 12}, v["expected"]
    assert v["observed"] == {o: 2, m: 4, i: 11}, v["observed"]
    assert v["missing_total"] == 1 and v["extra_total"] == 0, v
    assert v["depth_wrong"][m] == 1 and v["depth_wrong_total"] == 1, v
    assert v["not_contained_total"] == 0, v
    # An inner outside any middle is not contained.
    rows.append((7, i, 2, 5000, 10))
    v = analyse(rows, threads=1, calls=2)
    assert v["not_contained"][i] == 1 and v["extra"][i] == 0, v
    print("self-test ok")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("bundle", nargs="?")
    ap.add_argument("--pid", type=int)
    ap.add_argument("--threads", type=int)
    ap.add_argument("--calls", type=int)
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        self_test()
        return 0
    if not (args.bundle and args.pid and args.threads and args.calls):
        ap.error("bundle, --pid, --threads and --calls are required")
    verdict = analyse(rows_from_bundle(args.bundle, args.pid), args.threads, args.calls)
    print(json.dumps(verdict))
    return 0


if __name__ == "__main__":
    sys.exit(main())
