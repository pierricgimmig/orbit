#!/usr/bin/env bash
# Install / check the box prerequisites. Idempotent.
set -euo pipefail; . "$(dirname "$0")/../lib/config.sh"
PKGS="qemu-system-x86 qemu-utils openssh-client xvfb x11-utils ffmpeg xz-utils curl python3 gcc iproute2"   # + genisoimage for steps/vm-create.sh
missing=""
for p in $PKGS; do dpkg -s "$p" >/dev/null 2>&1 || missing="$missing $p"; done
if [ -n "$missing" ]; then
  log "apt install:$missing"
  # deb.debian.org sometimes answers 500: retry, and never fail when nothing is missing (this block is skipped then)
  for i in 1 2 3; do sudo apt-get update -qq && break; log "WARNING: apt-get update failed (try $i/3)"; sleep $((i*5)); done
  ok=0; for i in 1 2 3; do sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq $missing >/dev/null && { ok=1; break; }; log "WARNING: apt-get install failed (try $i/3)"; sleep $((i*5)); done
  [ $ok = 1 ] || die "apt-get install failed:$missing"
fi
if ! command -v gifski >/dev/null; then
  log "installing gifski 1.34.0 (prebuilt release binary)"
  t=$(mktemp -d); curl -sL -o "$t/g.tar.xz" https://github.com/ImageOptim/gifski/releases/download/1.34.0/gifski-1.34.0.tar.xz
  tar -xJf "$t/g.tar.xz" -C "$t" 2>/dev/null; sudo install -m755 "$t/linux/gifski" /usr/local/bin/gifski; rm -rf "$t"
fi
[ -x "$CHROME" ] || die "Chrome not found at $CHROME (set CHROME=...)"
command -v node >/dev/null || die "node (>=18) is required"
if [ ! -d "$NODE_DIR/node_modules/playwright-core" ]; then
  log "npm ci (playwright-core) in $NODE_DIR"; (cd "$NODE_DIR" && npm ci --silent)
fi
log "prereqs ok: $(qemu-system-x86_64 --version | head -1); $(gifski --version); ffmpeg $(ffmpeg -version | head -1 | cut -d' ' -f3)"
