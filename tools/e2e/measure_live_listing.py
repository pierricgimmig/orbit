#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Measures the viewer's primitive listing during a live capture, with the
window still (Follow off: the per-lane cache reuses rows) and with the
window moving (Follow on: every lane is re-listed every frame). TODO item 21.

Runs the same harness the e2e suite uses -- headless Chrome, a fresh service
on its own port, the Box3D target -- opens the Self pane and reads
`window.__orbit_self` after a few seconds of capture in each mode. Prints a
table; nothing is asserted.

    python3 tools/e2e/measure_live_listing.py [--seconds 8]
"""

import argparse
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from orbit_e2e import Chrome, Run, Service, Target, build_target, DEFAULT_BOX3D  # noqa: E402


def phase(phases, name):
    return next((p for p in phases.get("phases", []) if p["name"] == name), None)


def sample(run, seconds):
    # Two readings a window apart: the phase totals are cumulative, so the
    # difference is the cost over exactly `seconds` of live capture.
    before = run.self_phases()
    time.sleep(seconds)
    after = run.self_phases()
    frames = after["frames"] - before["frames"]
    out = {"frames": frames, "fps": after.get("fps"), "lanes": after.get("lanes"), "reused": after.get("reused")}
    for name in ("PrimitiveListing", "Frame"):
        a, b = phase(after, name), phase(before, name)
        if a and b and frames > 0:
            out[name] = ((a["total_ms"] - b["total_ms"]) / frames, a["max_us"])
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=44820)
    parser.add_argument("--box3d", default=DEFAULT_BOX3D)
    parser.add_argument("--seconds", type=float, default=8.0)
    args = parser.parse_args()
    target_bin = "/tmp/orbit-e2e-box3d-target"
    build_target(args.box3d, target_bin)
    target = service = chrome = None
    try:
        target = Target([target_bin, "--threads", "3"])
        service = Service(args.port)
        chrome = Chrome(port=args.port + 5000)
        run = Run(service, target, chrome, "/tmp/orbit-e2e-measure")
        run.open_viewer("?collapse=scheduler")
        run.click("Self")
        run.wait_for(lambda: run.self_phases() or None, "the self readout", timeout=20)
        service.post("/api/capture/start", {"pid": target.pid})
        time.sleep(3.0)
        results = {}
        # Follow is on during a capture: the window moves every frame.
        results["follow on (window moves)"] = sample(run, args.seconds)
        run.click("Follow")
        time.sleep(1.0)
        results["follow off (window still)"] = sample(run, args.seconds)
        service.post("/api/capture/stop")
        print(f"\nlive capture of Box3D ({args.seconds:.0f} s per mode), headless Chrome / SwiftShader")
        print(f"{'mode':<28} {'frames':>6} {'fps':>5} {'lanes':>5} {'reused':>6} {'listing ms/frame':>17} {'frame ms/frame':>15}")
        for mode, r in results.items():
            listing = r.get("PrimitiveListing", (float('nan'), 0))[0]
            frame = r.get("Frame", (float('nan'), 0))[0]
            print(f"{mode:<28} {r['frames']:>6} {r['fps'] or 0:>5.0f} {r['lanes'] or 0:>5} {r['reused'] or 0:>6} {listing:>17.3f} {frame:>15.2f}")
        names = sorted(p["name"] for p in run.self_phases().get("phases", []))
        print("phases:", ", ".join(names))
    finally:
        for item in (chrome, service, target):
            if item is not None:
                item.stop() if hasattr(item, "stop") else item.close()


if __name__ == "__main__":
    main()
