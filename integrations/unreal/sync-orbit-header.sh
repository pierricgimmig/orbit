#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# The plugin carries its own copy of orbit.h so it can be dropped into a
# project as is. Run this after changing crates/orbit-api/include/orbit.h.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cp "$here/../../crates/orbit-api/include/orbit.h" "$here/OrbitProfiler/Source/OrbitProfiler/Public/orbit.h"
echo "synced $(sha256sum "$here/OrbitProfiler/Source/OrbitProfiler/Public/orbit.h" | cut -c1-12)"
