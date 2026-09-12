#!/bin/sh
# Builds Orbit's Mojo test program. Needs the `max` package (the Mojo
# compiler plus the MAX GPU library): `uv pip install max`, or point MOJO at
# a `mojo` binary. Debug info on, so the symbols carry source lines.
set -eu
cd "$(dirname "$0")"
MOJO=${MOJO:-mojo}
exec "$MOJO" build -g orbit_test_mojo.mojo -o orbit_test_mojo
