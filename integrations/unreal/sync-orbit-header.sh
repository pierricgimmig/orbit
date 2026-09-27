#!/usr/bin/env bash
# The plugin carries its own copy of orbit.h so it can be dropped into a
# project as is. Run this after changing rust/crates/orbit-api/include/orbit.h.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cp "$here/../../rust/crates/orbit-api/include/orbit.h" "$here/OrbitProfiler/Source/OrbitProfiler/Public/orbit.h"
echo "synced $(sha256sum "$here/OrbitProfiler/Source/OrbitProfiler/Public/orbit.h" | cut -c1-12)"
