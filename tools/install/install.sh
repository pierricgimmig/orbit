#!/bin/sh
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Orbit installer:
#
#     curl -fsSL https://raw.githubusercontent.com/pierricgimmig/orbit/main/tools/install/install.sh | sh
#
# It downloads the Rust orbit-service release for this machine (the static
# capture service, which also serves the web viewer), checks SHA-256, and
# installs it plus the Frida files the service looks for in the same
# directory. No compiler and no package manager.
#
# Download host. This one line is the switch to orbitprofiler.com later:
# point it at a prefix that serves the same file names
# (orbit-linux-x86_64.tar.gz, orbit-macos-universal.tar.gz, and the
# matching .sha256 files).
DEFAULT_BASE="https://github.com/pierricgimmig/orbit/releases/latest/download"
#
# Overridable:
#   ORBIT_INSTALL_BASE   full prefix of the asset URLs (overrides DEFAULT_BASE
#                        and ORBIT_VERSION)
#   ORBIT_VERSION        tag to download, e.g. orbit-v0.1.0 (default: latest)
#   ORBIT_INSTALL_DIR    where to install (default: ~/.local/bin)

set -eu

BASE="${ORBIT_INSTALL_BASE:-$DEFAULT_BASE}"
BASE=${BASE%/}
if [ -z "${ORBIT_INSTALL_BASE:-}" ] && [ -n "${ORBIT_VERSION:-}" ]; then
    case "${ORBIT_VERSION}" in
        */*) printf 'orbit: error: ORBIT_VERSION must be a tag such as orbit-v0.1.0\n' >&2; exit 1 ;;
    esac
    BASE="https://github.com/pierricgimmig/orbit/releases/download/${ORBIT_VERSION}"
fi
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
    *) die "unsupported OS '$os'. Linux and macOS have release binaries. Windows is not supported." ;;
esac
case "$arch" in
    x86_64 | amd64)        arch=x86_64 ;;
    aarch64 | arm64)       arch=aarch64 ;;
    *) die "unsupported architecture '$arch'" ;;
esac

case "${os}-${arch}" in
    linux-x86_64)
        asset="orbit-linux-x86_64.tar.gz"
        ;;
    macos-x86_64 | macos-aarch64)
        asset="orbit-macos-universal.tar.gz"
        ;;
    linux-aarch64)
        die "no published Linux arm64 build. The release is a static Linux x86-64 binary and a macOS universal binary. Arm64 Linux compiles from source and is not in CI."
        ;;
    *)
        die "no release asset for ${os}/${arch}"
        ;;
esac

url="${BASE}/${asset}"
say "installing orbit-service ${VERSION} for ${os}/${arch}"
say "from ${url}"

# --- pick a downloader -------------------------------------------------------
if have curl; then
    fetch() { curl -fsSL "$1" -o "$2"; }
elif have wget; then
    fetch() { wget -qO "$2" "$1"; }
else
    die "need curl or wget to download"
fi

tmp=$(mktemp -d "${TMPDIR:-/tmp}/orbit-install.XXXXXX") || die "cannot make a temp dir"
trap 'rm -rf "$tmp"' EXIT INT TERM

archive="$tmp/$asset"
if ! fetch "$url" "$archive"; then
    die "download failed: ${url}

No Rust release is published at that URL yet, or this platform's archive is missing.
Releases are tagged orbit-vX.Y.Z (see docs/releasing.md). A prerelease suffix
(orbit-vX.Y.Z-rc.1) is not Latest, so this default URL will not see it.
The v1.* and nightly-* releases are the legacy C++ build and do not contain orbit-service.

Build from source:
  Linux x86-64:  ./rust/build-service-musl.sh
  macOS:         ./build-service-macos.sh

Or set ORBIT_INSTALL_BASE to a URL prefix that already serves the files.
To use orbitprofiler.com later, change DEFAULT_BASE at the top of this script."
fi

# --- checksum is required ----------------------------------------------------
if ! fetch "${url}.sha256" "$tmp/sum"; then
    die "no checksum at ${url}.sha256
Refusing to install an unverified binary."
fi
want=$(awk '{print $1}' "$tmp/sum")
case "$want" in
    [0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]*)
        ;;
    *) die "checksum file did not contain a SHA-256 hash" ;;
esac
if have sha256sum; then
    got=$(sha256sum "$archive" | awk '{print $1}')
elif have shasum; then
    got=$(shasum -a 256 "$archive" | awk '{print $1}')
else
    die "need sha256sum or shasum to verify the download"
fi
if [ "$got" != "$want" ]; then
    die "checksum mismatch
  expected $want
  got      $got"
fi
say "checksum ok"

# --- unpack and install ------------------------------------------------------
tar -xzf "$archive" -C "$tmp" || die "could not unpack ${asset}"
root="$tmp/orbit"
[ -f "$root/orbit-service" ] || die "archive has no orbit/orbit-service"
for need in orbit-frida-helper; do
    [ -f "$root/$need" ] || die "archive is missing $need. Function hooks need it beside orbit-service."
done
agent=""
for candidate in liborbit_frida_agent.so liborbit_frida_agent.dylib; do
    if [ -f "$root/$candidate" ]; then
        agent=$candidate
        break
    fi
done
[ -n "$agent" ] || die "archive is missing liborbit_frida_agent. Function hooks need it beside orbit-service."

mkdir -p "$INSTALL_DIR" || die "cannot create $INSTALL_DIR"
install_bin() {
    chmod 755 "$1"
    mv -f "$1" "$2" || die "cannot install to $2"
    say "installed $2"
}
install_file() {
    chmod 644 "$1"
    mv -f "$1" "$2" || die "cannot install to $2"
    say "installed $2"
}

install_bin "$root/orbit-service" "$INSTALL_DIR/orbit-service"
install_bin "$root/orbit-frida-helper" "$INSTALL_DIR/orbit-frida-helper"
install_file "$root/$agent" "$INSTALL_DIR/$agent"

case "$os" in
    linux) lib="liborbit_api.so" ;;
    macos) lib="liborbit_api.dylib" ;;
esac
if [ -f "$root/$lib" ]; then
    install_file "$root/$lib" "$INSTALL_DIR/$lib"
else
    say "no $lib in this archive; orbit.h and Python orbit-api calls run as no-ops"
fi
if [ -f "$root/orbit.h" ]; then
    install_file "$root/orbit.h" "$INSTALL_DIR/orbit.h"
fi
if [ -f "$root/orbit-gpu-helper" ]; then
    install_bin "$root/orbit-gpu-helper" "$INSTALL_DIR/orbit-gpu-helper"
fi
if [ -d "$root/licenses" ]; then
    mkdir -p "$INSTALL_DIR/licenses"
    cp -R "$root/licenses/." "$INSTALL_DIR/licenses/"
    say "installed Frida license notices in $INSTALL_DIR/licenses"
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

  orbit-service
  # then open http://127.0.0.1:44766/

Function hooks use liborbit_frida_agent and orbit-frida-helper in the same
directory as orbit-service. They do not need root. On Linux, sampling needs
perf_event_paranoid at 2 or below, and the scheduler needs 0 or below.
Root, CAP_PERFMON, or CAP_SYS_ADMIN bypass that sysctl. Auto-profiling uses
the kernel hook engine and needs root or CAP_SYS_ADMIN.

To instrument your own code: orbit.h is beside orbit-service (nothing to
link; it finds liborbit_api there), or use the orbit-api crate or package.
EOF
