#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# A/B benchmark for the ParseMaps backends. Same binary, two environments.
#
#   scripts/bench_parse_maps.sh [iterations] [rounds]
#
# Alternates backends across rounds so a warming CPU cannot favour whichever
# one runs first, and reports the median of each.

set -euo pipefail
cd "$(dirname "$0")/.."

ITERATIONS="${1:-30000}"
ROUNDS="${2:-5}"
BIN=bazel-bin/rust/tools/ab_bench/parse_maps_bench

[[ -x "$BIN" ]] || bazel build -c opt //rust/tools/ab_bench:parse_maps_bench

field() { grep -P "^$2\t" | cut -f2; }
median() { sort -g | awk '{a[NR]=$1} END {print (NR%2) ? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2}'; }

declare -A samples=([cpp]="" [rust]="")
for ((r = 0; r < ROUNDS; r++)); do
  for backend in cpp rust; do
    us=$(ORBIT_MAPS_BACKEND=$backend "$BIN" "$ITERATIONS" | field . us_per_parse)
    samples[$backend]+="$us"$'\n'
  done
done

cpp_us=$(printf '%s' "${samples[cpp]}" | median)
rust_us=$(printf '%s' "${samples[rust]}" | median)

printf 'iterations   %s, %s rounds\n' "$ITERATIONS" "$ROUNDS"
printf 'cpp          %s us/parse (median)\n' "$cpp_us"
printf 'rust         %s us/parse (median)\n' "$rust_us"
awk -v c="$cpp_us" -v r="$rust_us" 'BEGIN {printf "speedup      %.2fx\n", c / r}'
