#!/usr/bin/env python3
"""Write a small, deterministic Chrome Trace Event Format file (a fake game engine: main, render and
worker threads with nested X slices, a counter track, instants and a flow) for the import clip."""
import json, random, sys
random.seed(7)
PID = 4242   # not 1: the viewer names a trace pid after a live process with the same pid (see README)
US = 1000.0  # times below are written in ms and emitted in µs (the format's default unit)
out = sys.argv[1] if len(sys.argv) > 1 else "demo-trace.json"
ev = [{"ph": "M", "pid": PID, "name": "process_name", "args": {"name": "demo-engine"}}]
threads = {1: "MainThread", 2: "RenderThread", 3: "Worker 1", 4: "Worker 2", 5: "Worker 3"}
for tid, n in threads.items():
    ev.append({"ph": "M", "pid": PID, "tid": tid, "name": "thread_name", "args": {"name": n}})
ts = 1000.0
for frame in range(120):
    f0 = ts
    def X(tid, name, t, d, cat="engine", **args):
        ev.append({"ph": "X", "pid": PID, "tid": tid, "name": name, "cat": cat, "ts": round(t * US, 1), "dur": round(d * US, 1), "args": args})
    X(1, "Frame", f0, 15.5, frame=frame)
    t = f0 + 0.2
    for name, d in [("Input", 0.6), ("Update", 6.0 + random.random()), ("Physics", 3.0 + random.random()), ("Submit", 1.2)]:
        X(1, name, t, d)
        if name == "Update":
            u = t + 0.1
            for sub, sd in [("AI", 2.0), ("Animation", 1.8), ("Scripts", 1.5)]:
                X(1, sub, u, sd * (0.9 + 0.2 * random.random())); u += sd + 0.05
        t += d + 0.1
    r = f0 + 7.0
    X(2, "RenderFrame", r, 12.0)
    for sub, sd in [("Culling", 2.0), ("Shadows", 3.5), ("Opaque", 4.0), ("Post", 1.8)]:
        X(2, sub, r + 0.1, sd); r += sd + 0.12
    for w in (3, 4, 5):
        s = f0 + random.random() * 3
        for j in range(3):
            d = 2 + random.random() * 2.5
            X(w, random.choice(["Job: Decompress", "Job: Pathfind", "Job: Skinning", "Job: Audio"]), s, d, cat="jobs"); s += d + 0.4
    ev.append({"ph": "C", "pid": PID, "name": "memory MB", "ts": f0 * US, "args": {"heap": 300 + 40 * random.random() + frame * 0.5}})
    if frame % 30 == 0:
        ev.append({"ph": "i", "pid": PID, "tid": 1, "name": "GC", "ts": (f0 + 14.0) * US, "s": "t"})
    ev.append({"ph": "s", "pid": PID, "tid": 1, "name": "submit", "cat": "flow", "id": frame, "ts": (f0 + 14.5) * US})
    ev.append({"ph": "f", "pid": PID, "tid": 2, "name": "submit", "cat": "flow", "id": frame, "ts": (f0 + 16.0) * US, "bp": "e"})
    ts += 16.667
json.dump({"traceEvents": ev}, open(out, "w"))
print(out, len(ev), "events")
