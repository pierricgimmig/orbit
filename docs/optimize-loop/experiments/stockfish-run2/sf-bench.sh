#!/bin/bash
# Run Stockfish bench and emit ORBIT_METRIC (nps) + ORBIT_FINGERPRINT (nodes).
set -euo pipefail
cd /home/pierric/git/stockfish/src
out=$(taskset -c 2 ./stockfish bench "${1:-128}" "${2:-1}" "${3:-13}" 2>&1)
nps=$(echo "$out" | grep 'Nodes/second' | grep -oE '[0-9]+' | tail -1)
nodes=$(echo "$out" | grep 'Nodes searched' | grep -oE '[0-9]+' | tail -1)
echo "ORBIT_METRIC throughput=${nps}"
echo "ORBIT_FINGERPRINT ${nodes}"
