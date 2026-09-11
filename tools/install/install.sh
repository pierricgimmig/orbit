#!/bin/sh
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Orbit installer, in the shape the LLM CLIs use:
#
#     curl -fsSL https://<orbit-site>/install.sh | sh
#
# It downloads the orbit-service binary (the static capture service, which
# also serves the web viewer) for this OS and architecture, verifies its
# SHA-256 when a checksum is published beside it, and drops it in a bin
# directory on PATH. No compiler, no package manager.
#
# Overridable with environment variables:
#   ORBIT_INSTALL_BASE   where the binaries live (default: the Orbit site)
#   ORBIT_INSTALL_DIR    where to install (default: ~/.local/bin)
#   ORBIT_VERSION        which release (default: latest)
#
# Until the public site is live, point it at a dev server that serves the
# dist/ tree, e.g.:
#   ORBIT_INSTALL_BASE=http://192.168.1.10:44766 sh install.sh

set -eu

# The public site is not up yet (TODO item 25); this is the address it will
# serve from. Override with ORBIT_INSTALL_BASE against a dev server today.
DEFAULT_BASE="https://orbitprofiler.dev"
BASE="${ORBIT_INSTALL_BASE:-$DEFAULT_BASE}"
VERSION="${ORBIT_VERSION:-latest}"
INSTALL_DIR="${ORBIT_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf 'orbit: %s\n' "$1" >&2; }
die() { printf 'orbit: error: %s\n' "$1" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# --- detect platform ---------------------------------------------------------
os=$(uname -s 2>/dev/null || echo unknown)
arch=$(uname -m 2>/dev/null || echo unknown)
case "$os" in
    Linux)  os=linux ;;
    Darwin) os=macos ;;
    *) die "unsupported OS '$os'. Linux and macOS have binaries; Windows: build from source." ;;
esac
case "$arch" in
    x86_64 | amd64)        arch=x86_64 ;;
    aarch64 | arm64)       arch=aarch64 ;;
    *) die "unsupported architecture '$arch'" ;;
esac

target="orbit-service-${os}-${arch}"
url="${BASE}/dist/${VERSION}/${target}"
say "installing orbit-service ${VERSION} for ${os}/${arch}"
say "from ${url}"

# --- pick a downloader -------------------------------------------------------
if have curl; then
    fetch() { curl -fsSL "$1" -o "$2"; }
    fetch_ok() { curl -fsSL -o /dev/null "$1" 2>/dev/null; }
elif have wget; then
    fetch() { wget -qO "$2" "$1"; }
    fetch_ok() { wget -q -O /dev/null "$1" 2>/dev/null; }
else
    die "need curl or wget to download"
fi

tmp=$(mktemp -d "${TMPDIR:-/tmp}/orbit-install.XXXXXX") || die "cannot make a temp dir"
trap 'rm -rf "$tmp"' EXIT INT TERM

bin="$tmp/orbit-service"
fetch "$url" "$bin" || die "download failed: $url
The public site may not be up yet; set ORBIT_INSTALL_BASE to a server that
serves the dist/ tree (see the comment at the top of this script)."

# --- verify checksum if one is published -------------------------------------
if fetch_ok "${url}.sha256"; then
    fetch "${url}.sha256" "$tmp/sum" || die "checksum download failed"
    want=$(awk '{print $1}' "$tmp/sum")
    if have sha256sum; then
        got=$(sha256sum "$bin" | awk '{print $1}')
    elif have shasum; then
        got=$(shasum -a 256 "$bin" | awk '{print $1}')
    else
        got=""
        say "no sha256 tool found; skipping verification"
    fi
    if [ -n "$got" ] && [ "$got" != "$want" ]; then
        die "checksum mismatch
  expected $want
  got      $got"
    fi
    [ -n "$got" ] && say "checksum ok"
else
    say "no published checksum; skipping verification"
fi

# --- install -----------------------------------------------------------------
mkdir -p "$INSTALL_DIR" || die "cannot create $INSTALL_DIR"
dest="$INSTALL_DIR/orbit-service"
chmod +x "$bin"
mv -f "$bin" "$dest" || die "cannot install to $dest"
say "installed $dest"

# --- manual instrumentation library (optional) --------------------------------
# orbit.h and the orbit-api Python package load liborbit_api from the
# directory that holds orbit-service, so it goes beside the binary when the
# release publishes one. Missing is not an error: programs run uninstrumented.
case "$os" in
    linux) lib="liborbit_api.so" ;;
    macos) lib="liborbit_api.dylib" ;;
esac
liburl="${BASE}/dist/${VERSION}/${lib%.*}-${os}-${arch}.${lib##*.}"
if fetch_ok "$liburl" && fetch "$liburl" "$tmp/$lib"; then
    mv -f "$tmp/$lib" "$INSTALL_DIR/$lib" && say "installed $INSTALL_DIR/$lib (manual instrumentation)"
else
    say "no instrumentation library published for this release; orbit.h and pip's orbit-api run as no-ops"
fi

# --- PATH hint ---------------------------------------------------------------
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        say "note: $INSTALL_DIR is not on your PATH. Add it, e.g.:"
        printf '  export PATH="%s:$PATH"\n' "$INSTALL_DIR" >&2
        ;;
esac

cat >&2 <<EOF

orbit-service is installed. Start it and open the viewer:

  orbit-service --serve 44766
  # then open http://127.0.0.1:44766/ in a browser

Sampling needs perf_event_paranoid <= -1 or CAP_PERFMON; dynamic
instrumentation needs CAP_SYS_ADMIN. See the manual for details.

To instrument your own code: drop orbit.h into a C or C++ project (nothing
to link; it finds liborbit_api beside orbit-service), or pip install orbit-api.
EOF
