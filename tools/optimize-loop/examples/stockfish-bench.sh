#!/bin/bash
# Run Stockfish bench and emit ORBIT_METRIC (nps) + ORBIT_FINGERPRINT.
#
# The fingerprint is the sha256 of the *entire* bench output with only the
# timing-dependent tokens removed (time, nps, Total time, Nodes/second), so it
# covers every position's per-depth node count, score, PV and bestmove: an
# optimization is accepted only if the engine's output is bit-exact before
# and after. The node total alone is printed too, for humans.
set -euo pipefail
cd "${STOCKFISH_SRC:-$HOME/git/stockfish/src}"
out=$(taskset -c 2 ./stockfish bench "${1:-128}" "${2:-1}" "${3:-13}" 2>&1)
nps=$(echo "$out" | grep 'Nodes/second' | grep -oE '[0-9]+' | tail -1)
nodes=$(echo "$out" | grep 'Nodes searched' | grep -oE '[0-9]+' | tail -1)
fp=$(echo "$out" | grep -vE 'Total time|Nodes/second|^Stockfish|^info string' \
      | sed -E 's/ time [0-9]+//; s/ nps [0-9]+//' | sha256sum | cut -c1-16)
echo "ORBIT_METRIC throughput=${nps}"
echo "ORBIT_FINGERPRINT ${fp}-${nodes}"
