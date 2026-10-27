#!/usr/bin/env bash
# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Build perfr (static musl) with the WASM viewer embedded, from a git ref.
# usage: steps/10-build.sh <git-ref> [--rebuild]      prints the binary path on stdout
# Works in a private clone under $CACHE/src (never touches $PERFR_REPO's working tree, never pushes).
set -euo pipefail; . "$(dirname "$0")/../lib/config.sh"
REF="${1:?git ref}"; REBUILD="${2:-}"
SRC="$CACHE/src"; mkdir -p "$CACHE/bin" "$CACHE/logs"
UPSTREAM="${PERFR_UPSTREAM:-https://github.com/perfr/perfr.git}"
if [ ! -d "$SRC/.git" ]; then
  log "cloning $PERFR_REPO -> $SRC (shared objects)"
  git clone -q --shared --no-checkout "$PERFR_REPO" "$SRC"
  git -C "$SRC" remote add upstream "$UPSTREAM"
fi
git -C "$SRC" fetch -q origin 2>/dev/null || true
SHA=$(git -C "$SRC" rev-parse -q --verify "$REF^{commit}" 2>/dev/null || git -C "$SRC" rev-parse -q --verify "origin/$REF^{commit}" 2>/dev/null || true)
if [ -z "$SHA" ]; then
  log "ref $REF not in the local clone; fetching it from $UPSTREAM (read-only)"
  git -C "$SRC" fetch -q upstream "$REF" && SHA=$(git -C "$SRC" rev-parse FETCH_HEAD)
fi
[ -n "$SHA" ] || die "cannot resolve ref $REF"
SHORT=${SHA:0:8}; OUT="$CACHE/bin/perfr-$SHORT"
if [ -x "$OUT" ] && [ "$REBUILD" != "--rebuild" ]; then log "using cached $OUT"; echo "$OUT"; exit 0; fi
log "building $REF = $SHA"
git -C "$SRC" checkout -q --force --detach "$SHA"
git -C "$SRC" clean -q -fd -e target   # keep cargo target dirs for incremental builds
command -v rustup >/dev/null || export PATH="$HOME/.cargo/bin:$PATH"
L="$CACHE/logs/build-$SHORT"
log "wasm viewer (crates/perfr-live-viewer/build_wasm.sh; log $L-wasm.log)"
( cd "$SRC/crates/perfr-live-viewer" && ./build_wasm.sh ) > "$L-wasm.log" 2>&1 || { tail -30 "$L-wasm.log" >&2; die "wasm build failed"; }
log "perfr musl (log $L-service.log)"
( cd "$SRC" && cargo build --release --target x86_64-unknown-linux-musl -p perfr ) \
  > "$L-service.log" 2>&1 || { tail -30 "$L-service.log" >&2; die "service build failed"; }
cp "$SRC/target/x86_64-unknown-linux-musl/release/perfr" "$OUT"
# The wasm build regenerates tracked viewer-dist files; restore them so the clone stays clean.
git -C "$SRC" checkout -q -- . ; git -C "$SRC" status --short | head -5 >&2 || true
log "built $OUT ($(du -h "$OUT" | cut -f1))"
echo "$OUT"
