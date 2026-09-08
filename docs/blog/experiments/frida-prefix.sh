#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license in LICENSE.
set -euo pipefail
cd "$(dirname "$0")/../../.."
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'This probe requires Linux x86-64.' >&2; exit 1; }
./tools/frida/devkits.sh x86_64-unknown-linux-gnu >&2
kit="${ORBIT_FRIDA_DEVKIT_ROOT:-$PWD/rust/target/frida-devkits}/x86_64-unknown-linux-gnu/gum"
temp=$(mktemp -d)
trap 'rm -rf "$temp"' EXIT
cc -O0 -g -ffunction-sections -fdata-sections -I "$kit" docs/blog/experiments/frida-prefix.c docs/blog/experiments/frida-prefix.S -L "$kit" -lfrida-gum -lrt -lresolv -ldl -lm -pthread -o "$temp/probe"
"$temp/probe"
