#!/usr/bin/env bash
# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Runs the "a probe actually fires" test with the capability it needs:
# cargo builds the test binary as you, and hands it to sudo to run.
set -e
cd "$(dirname "$0")/../.."
export PATH="$HOME/.cargo/bin:$PATH"
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER=sudo \
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER=sudo \
  cargo test -p perfr a_uprobe_fires -- --nocapture
