#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Build OrbitService and run it as root so it serves the live WASM viewer.
# Root is required for real context-switch / thread-state tracks. Record
# attaches to the process you pick in the Capture strip (not the demo).
#
#   ./wasm.sh                            # http://127.0.0.1:44766/
#   ./wasm.sh --http-port 44768          # when 44766 is already taken
#   ./wasm.sh -- --devmode --spill_path /tmp/orbit-spill
#
# This builds the service, not the pack. Run src/OrbitLiveViewer/build_wasm.sh
# first if you changed the front end; the script warns when it looks stale.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

HTTP_PORT=44766
GRPC_PORT=44765
PASSTHROUGH=()

usage() {
  sed -n '6,14p' "${BASH_SOURCE[0]}" | sed 's/^# \?//'
  cat <<'USAGE'

Options:
  --http-port N   Port for the viewer page (default 44766; 0 disables it)
  --grpc-port N   Port for the gRPC capture service (default 44765)
  -h, --help      This message

Anything after -- goes to OrbitService verbatim.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --http-port|--http_port) HTTP_PORT="${2:?--http-port needs a value}"; shift 2 ;;
    --grpc-port|--grpc_port) GRPC_PORT="${2:?--grpc-port needs a value}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    --) shift; PASSTHROUGH+=("$@"); break ;;
    *) PASSTHROUGH+=("$1"); shift ;;
  esac
done

die() { echo "wasm.sh: $*" >&2; exit 1; }
warn() { echo "wasm.sh: $*" >&2; }

port_in_use() {
  command -v ss >/dev/null || return 1
  ss -ltn 2>/dev/null | awk '{print $4}' | grep -qE "[:.]$1$"
}

for port in "$HTTP_PORT" "$GRPC_PORT"; do
  [[ "$port" == "0" ]] && continue
  if port_in_use "$port"; then
    die "port $port is already in use (another OrbitService or bazel run //:live?)"
  fi
done

# Build as the invoking user. Bazel under sudo writes a root-owned output base
# and every later non-root build then fails on it.
if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
  warn "already root: bazel will build as root, which can leave a root-owned"
  warn "output base behind. Prefer running this as your normal user."
  SUDO=()
else
  command -v sudo >/dev/null || die "sudo not found; run as root instead"
  SUDO=(sudo --)
fi

echo "wasm.sh: building //src/Service:OrbitService"
bazel build //src/Service:OrbitService

BIN="$ROOT/bazel-bin/src/Service/OrbitService"
[[ -x "$BIN" ]] || die "built, but $BIN is missing"

# Bazel re-embeds viewer-dist/ when it changes, so the served pack matches the
# checkout. It does not rebuild the pack from the Rust front end.
PACK="$ROOT/src/OrbitLiveViewer/viewer-dist/orbit_live_viewer_bg.wasm"
VIEWER_SRC="$ROOT/src/OrbitLiveViewer/crates/orbit-live-viewer/src"
if [[ -f "$PACK" && -d "$VIEWER_SRC" ]] &&
   [[ -n "$(find "$VIEWER_SRC" -name '*.rs' -newer "$PACK" -print -quit)" ]]; then
  warn "viewer-dist/ is older than crates/orbit-live-viewer/src — the page will"
  warn "serve the previous pack. Run src/OrbitLiveViewer/build_wasm.sh to refresh it."
fi

cat <<BANNER

  Orbit live viewer   http://127.0.0.1:${HTTP_PORT}/
  gRPC capture        127.0.0.1:${GRPC_PORT}

  Ctrl-C to stop. Root is what makes scheduling and thread-state tracks work.
  Pick a process, wait for symbols, then Record. Demo is the dummy path.

BANNER

# OrbitService quits the moment stdin reaches EOF, because the Qt UI drives it
# over a pipe. A pipe opened read-write never reports EOF, so hand it one and
# the service survives whether or not this script has a terminal.
fifo_dir="$(mktemp -d)"
mkfifo "$fifo_dir/stdin"
exec 3<>"$fifo_dir/stdin"
rm -rf "$fifo_dir"

exec "${SUDO[@]}" "$BIN" \
  --grpc_port "$GRPC_PORT" \
  --http_port "$HTTP_PORT" \
  "${PASSTHROUGH[@]}" <&3
