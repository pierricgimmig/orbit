#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""OrbitTestPython: every manual-instrumentation call, from Python. Same
scenario as OrbitTestRust, OrbitTestC and OrbitTestCpp.

    python3 OrbitTestPython.py [--seconds N]
"""

import math
import os
import queue
import sys
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import orbit  # noqa: E402


def busy(micros):
    until = time.perf_counter() + micros * 1e-6
    x = 1
    while time.perf_counter() < until:
        x = (x * 6364136223846793005 + 1442695040888963407) & 0xFFFFFFFFFFFFFFFF


def physics_worker(index, stop, jobs):
    name = f"physics-{index}"
    while not stop.is_set():
        with orbit.scope(name):
            with orbit.scope("solve contacts"):
                busy(700)
            with orbit.scope("integrate"):
                busy(300)
            try:
                job = jobs.get_nowait()
            except queue.Empty:
                job = None
            if job is not None:
                job_index, async_scope, enqueued_at = job
                with orbit.scope(f"run job {job_index}") as run:
                    orbit.link(enqueued_at, run.handle)
                    busy(1500)
                orbit.stop(async_scope)  # the async scope ends here, on this thread
        time.sleep(0.0005)


def main():
    seconds = 8
    if "--seconds" in sys.argv:
        seconds = int(sys.argv[sys.argv.index("--seconds") + 1])

    rc = orbit.init()
    if rc != 0:
        print(f"OrbitTestPython: orbit_init failed ({rc}); running uninstrumented", file=sys.stderr)
    print(f"OrbitTestPython pid={os.getpid()} seconds={seconds}", flush=True)

    stop = threading.Event()
    jobs = queue.Queue()
    workers = [threading.Thread(target=physics_worker, args=(i, stop, jobs), name=f"physics-{i}")
               for i in range(3)]
    for w in workers:
        w.start()

    started = time.perf_counter()
    last = started
    frame = 0
    while seconds == 0 or time.perf_counter() < started + seconds:
        with orbit.scope("frame"):
            orbit.instant("vsync")
            with orbit.scope("update"):
                busy(2000)
                detail = (f"update entities: pass={frame % 4} "
                          f"camera=({math.sin(frame * 0.7) * 100:.1f},{math.cos(frame * 0.3) * 100:.1f}) "
                          f"budget=16.6ms lod=adaptive")
                with orbit.scope(detail):
                    busy(1000)
            with orbit.scope("render"):
                busy(3000)
            if frame % 8 == 0:
                enqueued_at = orbit.instant("enqueue job")
                async_scope = orbit.start_async("background job")
                jobs.put((frame // 8, async_scope, enqueued_at))
            now = time.perf_counter()
            dt = now - last
            last = now
            orbit.value("fps", 1.0 / dt if dt > 0 else 0.0)
            orbit.value("entities", 1000.0 + 200.0 * math.sin(frame * 0.05))
        frame += 1
        time.sleep(0.008)

    stop.set()
    for w in workers:
        w.join()
    print(f"OrbitTestPython done: {frame} frames in {time.perf_counter() - started:.1f}s")
    orbit.shutdown()


if __name__ == "__main__":
    main()
