#!/usr/bin/env python3
"""Interleaved round-robin benchmark of several *uninstrumented* Stockfish
binaries: no profiler attached, no build in between, order rotated per round
so drift cancels. Reports per-binary means, and paired per-round deltas and
win counts against the first binary (the baseline).

usage: bench_rr.py [--rounds N] [--cpu C] label=path label=path ...
"""
import argparse, json, re, statistics, subprocess, sys, time

ap = argparse.ArgumentParser()
ap.add_argument("--rounds", type=int, default=16)
ap.add_argument("--cpu", type=int, default=-1, help="pin every run to this core (taskset)")
ap.add_argument("--bench", default="128 1 13")
ap.add_argument("--out", default=None)
ap.add_argument("bins", nargs="+")
a = ap.parse_args()
bins = [b.split("=", 1) for b in a.bins]
labels = [l for l, _ in bins]
nps = {l: [] for l in labels}
fps = {l: set() for l in labels}
rounds = []
t0 = time.time()
for r in range(a.rounds):
    order = bins[r % len(bins):] + bins[:r % len(bins)]  # rotate start each round
    row = {}
    for label, path in order:
        cmd = ([ "taskset", "-c", str(a.cpu)] if a.cpu >= 0 else []) + [path, "bench", *a.bench.split()]
        p = subprocess.run(cmd, capture_output=True, text=True)
        o = p.stdout + p.stderr
        m = re.search(r"Nodes/second\s*:\s*(\d+)", o); n = re.search(r"Nodes searched\s*:\s*(\d+)", o)
        if not (m and n):
            print(f"round {r} {label}: no result rc={p.returncode}: {o[-200:]}", file=sys.stderr); continue
        row[label] = int(m.group(1)); nps[label].append(int(m.group(1))); fps[label].add(int(n.group(1)))
    rounds.append(row)
    base = row.get(labels[0])
    deltas = " ".join(f"{l}={100*(row[l]-base)/base:+.2f}%" for l in labels[1:] if l in row and base)
    print(f"round {r+1:2d}/{a.rounds}: base={base} {deltas}", flush=True)

summary = {"rounds": a.rounds, "cpu": a.cpu, "bench": a.bench, "wall_s": round(time.time() - t0, 1), "binaries": {}}
b0 = labels[0]
for l in labels:
    v = nps[l]; mean = statistics.fmean(v); sd = statistics.pstdev(v)
    entry = {"n": len(v), "mean": mean, "stdev": sd, "cv_pct": 100 * sd / mean, "min": min(v), "max": max(v),
             "fingerprint": sorted(fps[l])}
    if l != b0:
        paired = [100 * (row[l] - row[b0]) / row[b0] for row in rounds if l in row and b0 in row]
        wins = sum(1 for d in paired if d > 0)
        entry.update({"delta_pct_of_means": 100 * (mean - statistics.fmean(nps[b0])) / statistics.fmean(nps[b0]),
                      "paired_mean_pct": statistics.fmean(paired), "paired_min_pct": min(paired),
                      "paired_max_pct": max(paired), "paired_wins": f"{wins}/{len(paired)}",
                      "paired_stdev_pct": statistics.pstdev(paired),
                      "fingerprint_same_as_baseline": fps[l] == fps[b0]})
    summary["binaries"][l] = entry
for l, e in summary["binaries"].items():
    line = f"{l:18s} mean={e['mean']:9.0f} ±{e['stdev']:6.0f} (cv {e['cv_pct']:.2f}%)  fp={e['fingerprint']}"
    if "paired_wins" in e:
        line += f"  Δ={e['delta_pct_of_means']:+.2f}%  paired {e['paired_mean_pct']:+.2f}% [{e['paired_min_pct']:+.2f},{e['paired_max_pct']:+.2f}] wins {e['paired_wins']}"
    print(line)
if a.out:
    json.dump({"summary": summary, "rounds": rounds}, open(a.out, "w"), indent=1)
