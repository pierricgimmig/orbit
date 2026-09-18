#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""In-repo demo target for the optimize-loop. Not a real product benchmark.

Prints ORBIT_METRIC / ORBIT_FINGERPRINT lines the suite knows how to parse.
No external engine is required.
"""

from __future__ import annotations

import sys
import time

ITERS = 250_000


def work(n: int = ITERS) -> int:
    acc = 0
    for i in range(n):
        acc = (acc + i * 1103515245 + 12345) & 0xFFFFFFFF
    return acc


def main(argv: list[str]) -> int:
    mode = argv[1] if len(argv) > 1 else "--bench"
    if mode == "--self-test":
        left, right = work(10_000), work(10_000)
        if left != right:
            print("mismatch", left, right)
            return 1
        print(f"ORBIT_FINGERPRINT {left}")
        print("OK")
        return 0
    if mode == "--work":
        seconds = float(argv[2]) if len(argv) > 2 else 1.5
        deadline = time.perf_counter() + seconds
        while time.perf_counter() < deadline:
            work()
        return 0
    t0 = time.perf_counter()
    result = work()
    elapsed = time.perf_counter() - t0
    rate = ITERS / elapsed if elapsed else 0.0
    print(f"ORBIT_METRIC throughput={rate:.4f}")
    print(f"ORBIT_FINGERPRINT {result}")
    print(f"elapsed_s={elapsed:.6f}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
