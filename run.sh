#!/usr/bin/env bash
# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Build the all-Rust capture service and run it so it serves the live WASM
# viewer. It does not need root: without privileges the
# service drops what it cannot read and tells you how to enable it, rather
# than refusing to start. Record attaches to the process you pick in the
# Capture strip.
#
#   ./run.sh                            # http://127.0.0.1:44766/
#   ./run.sh --http-port 44768          # when 44766 is already taken
#   ./run.sh --static                   # the shippable static musl binary
#   ./run.sh --sudo                     # root, for system-wide scheduling
#   ./run.sh -- --pid 1234 --duration-ms 5000 --out /tmp/c.pod   # file mode
#
# This builds the service and native Frida components, not the viewer pack. Run
# crates/perfr-live-viewer/build_wasm.sh if you changed the front end; the script
# warns when it looks stale.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

HTTP_PORT=44766
STATIC=0
USE_SUDO=0
WITH_GPU=0
PASSTHROUGH=()

usage() {
  sed -n '6,20p' "${BASH_SOURCE[0]}" | sed 's/^# \?//'
  cat <<'USAGE'

Options:
  --http-port N   Port for the viewer page (default 44766)
  --static        Build and run the fully static musl binary
  --sudo          Run as root (system-wide scheduling needs it, or
                  perf_event_paranoid <= 0)
  --gpu-helper    Also build the NVIDIA telemetry helper and pass it through
  -h, --help      This message

Anything after -- goes to perfr verbatim, which is how you get the
file-capture mode (--pid / --duration-ms / --out) instead of the UI.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --http-port|--http_port) HTTP_PORT="${2:?--http-port needs a value}"; shift 2 ;;
    --static) STATIC=1; shift ;;
    --sudo) USE_SUDO=1; shift ;;
    --gpu-helper) WITH_GPU=1; shift ;;
    -h|--help) usage; exit 0 ;;
    --) shift; PASSTHROUGH+=("$@"); break ;;
    *) PASSTHROUGH+=("$1"); shift ;;
  esac
done

die() { echo "run.sh: $*" >&2; exit 1; }
warn() { echo "run.sh: $*" >&2; }

port_in_use() {
  command -v ss >/dev/null || return 1
  ss -ltn 2>/dev/null | awk '{print $4}' | grep -qE "[:.]$1$"
}

# Only the UI mode binds a port; file mode does not.
SERVE=1
for arg in "${PASSTHROUGH[@]:-}"; do
  [[ "$arg" == "--pid" || "$arg" == "--out" || "$arg" == "--duration-ms" ]] && SERVE=0
done

if [[ "$SERVE" -eq 1 && "$HTTP_PORT" != "0" ]] && port_in_use "$HTTP_PORT"; then
  die "port $HTTP_PORT is already in use (another perfr?)"
fi

command -v cargo >/dev/null 2>&1 || export PATH="$HOME/.cargo/bin:$PATH"
command -v cargo >/dev/null 2>&1 || die "cargo not found (expected in ~/.cargo/bin)"

# Build as the invoking user: building under sudo leaves root-owned artifacts
# in target/ that every later non-root build then trips over.
if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
  warn "already root: cargo will build as root, which can leave root-owned"
  warn "artifacts in target/. Prefer running this as your normal user."
  SUDO=()
elif [[ "$USE_SUDO" -eq 1 ]]; then
  command -v sudo >/dev/null || die "sudo not found; run as root instead"
  SUDO=(sudo --)
else
  SUDO=()
fi

if [[ "$STATIC" -eq 1 ]]; then
  echo "run.sh: building the static musl service"
  tools/build/build-service-musl.sh >/dev/null
  BIN="$ROOT/target/x86_64-unknown-linux-musl/release/perfr"
else
  echo "run.sh: building perfr (release)"
  cargo build --release -p perfr
  BIN="$ROOT/target/release/perfr"
fi
[[ -x "$BIN" ]] || die "built, but $BIN is missing"

# Dynamic instrumentation defaults to Frida. Keep its native components beside
# the development service; the musl builder already packages them there.
if [[ "$SERVE" -eq 1 && "$STATIC" -eq 0 ]]; then
  ./tools/frida/build.sh "$(dirname "$BIN")" --agent-only
fi

if [[ "$WITH_GPU" -eq 1 ]]; then
  echo "run.sh: building perfr-gpu-helper"
  cargo build --release -p perfr-gpu-helper
  HELPER="$ROOT/target/release/perfr-gpu-helper"
  [[ -x "$HELPER" ]] || die "built, but $HELPER is missing"
  PASSTHROUGH+=(--gpu-helper "$HELPER")
fi

# The service embeds nothing: it serves whatever pack is in viewer-dist, so a
# stale pack shows a stale page.
PACK="$ROOT/crates/perfr-live-viewer/viewer-dist/perfr_live_viewer_bg.wasm"
VIEWER_SRC="$ROOT/crates/perfr-live-viewer/src"
if [[ "$SERVE" -eq 1 && -f "$PACK" && -d "$VIEWER_SRC" ]] &&
   [[ -n "$(find "$VIEWER_SRC" -name '*.rs' -newer "$PACK" -print -quit)" ]]; then
  warn "viewer-dist/ is older than crates/perfr-live-viewer/src — the page will"
  warn "serve the previous pack. Run crates/perfr-live-viewer/build_wasm.sh to refresh it."
fi

# Scheduling is the one capture feature that needs privilege. Say so up front
# instead of letting the timeline come up mysteriously empty.
if [[ "$SERVE" -eq 1 && "${#SUDO[@]}" -eq 0 ]]; then
  PARANOID="$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo unknown)"
  if [[ "$PARANOID" =~ ^-?[0-9]+$ ]] && (( PARANOID > 0 )); then
    warn "perf_event_paranoid is $PARANOID, so system-wide scheduling will be"
    warn "skipped. Use ./run.sh --sudo, or:"
    warn "  sudo sysctl -w kernel.perf_event_paranoid=0"
    warn "The service still runs and reports what it could not capture."
  fi
fi

if [[ "$SERVE" -eq 0 ]]; then
  exec "${SUDO[@]}" "$BIN" "${PASSTHROUGH[@]}"
fi

cat <<BANNER

  perfr live viewer   http://127.0.0.1:${HTTP_PORT}/

  Ctrl-C to stop. No root required: without it the service captures what it
  can and prints how to enable the rest.
  Pick a process in the Capture strip, then Record.

BANNER

exec "${SUDO[@]}" "$BIN" --serve "$HTTP_PORT" "${PASSTHROUGH[@]}"
