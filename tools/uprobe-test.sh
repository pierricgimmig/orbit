#!/usr/bin/env bash
# Runs the "a probe actually fires" test with the capability it needs:
# cargo builds the test binary as you, and hands it to sudo to run.
set -e
cd "$(dirname "$0")/../rust/crates/orbit-service"
export PATH="$HOME/.cargo/bin:$PATH"
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER=sudo \
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER=sudo \
  cargo test a_uprobe_fires -- --nocapture
