#!/bin/bash
# A multi-second Stockfish workload for Orbit to attach to (deeper bench, 4 threads).
cd /home/pierric/git/stockfish/src
exec ./stockfish bench 256 4 22
