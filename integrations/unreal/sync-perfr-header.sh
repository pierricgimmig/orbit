#!/usr/bin/env bash
# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# The plugin carries its own copy of perfr.h so it can be dropped into a
# project as is. Run this after changing crates/perfr-api/include/perfr.h.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cp "$here/../../crates/perfr-api/include/perfr.h" "$here/PerfrProfiler/Source/PerfrProfiler/Public/perfr.h"
echo "synced $(sha256sum "$here/PerfrProfiler/Source/PerfrProfiler/Public/perfr.h" | cut -c1-12)"
