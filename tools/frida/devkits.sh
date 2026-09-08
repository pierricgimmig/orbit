#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Download pinned, checksum-verified native Frida devkits. No language runtime.
set -euo pipefail
cd "$(dirname "$0")/../.."
target=${1:-$(rustc +1.88.0 -vV | sed -n 's/^host: //p')}
case "$target" in
  x86_64-unknown-linux-gnu) platform=linux-x86_64 ;;
  aarch64-unknown-linux-gnu) platform=linux-arm64 ;;
  x86_64-apple-darwin) platform=macos-x86_64 ;;
  aarch64-apple-darwin) platform=macos-arm64 ;;
  *) echo "Unsupported native Frida target: $target" >&2; exit 1 ;;
esac
root=${ORBIT_FRIDA_DEVKIT_ROOT:-"$PWD/rust/target/frida-devkits"}
for component in core gum; do
  name="frida-$component-devkit-17.17.0-$platform.tar.xz"
  kit="$root/$target/$component"
  [[ -f "$kit/.verified-17.17.0" ]] && continue
  mkdir -p "$root/$target"
  temp=$(mktemp -d "$root/$target/download.XXXXXX")
  trap 'rm -rf "$temp"' EXIT
  curl --fail --location --retry 3 "https://github.com/frida/frida/releases/download/17.17.0/$name" -o "$temp/$name"
  expected=$(awk -v name="$name" '$2 == name {print $1}' tools/frida/devkits.sha256)
  actual=$(shasum -a 256 "$temp/$name" | awk '{print $1}')
  [[ -n "$expected" && "$actual" == "$expected" ]] || { echo "Invalid checksum: $name" >&2; exit 1; }
  mkdir -p "$kit"
  tar -xJf "$temp/$name" -C "$kit"
  touch "$kit/.verified-17.17.0"
  rm -rf "$temp"
  trap - EXIT
done
