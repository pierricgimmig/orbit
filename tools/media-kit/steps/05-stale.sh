#!/usr/bin/env bash
# Kill whatever a previous (aborted) run left behind, using the pidfiles in $CACHE/run:
#   recorder.pid (node 40-record.mjs), chrome.pid (Playwright's browser), xvfb.pid, qemu.pid (VM: ssh poweroff first).
# usage: steps/05-stale.sh [--keep-vm]
set -uo pipefail; . "$(dirname "$0")/../lib/config.sh"
RUN="$CACHE/run"; mkdir -p "$RUN"
alive() { [ -f "$1" ] && kill -0 "$(cat "$1")" 2>/dev/null; }
for f in recorder chrome xvfb; do
  if alive "$RUN/$f.pid"; then p=$(cat "$RUN/$f.pid"); log "stale $f (pid $p): killing"
    pkill -TERM -P "$p" 2>/dev/null; kill -TERM "$p" 2>/dev/null; sleep 1; kill -KILL "$p" 2>/dev/null; fi
  rm -f "$RUN/$f.pid"
done
# Playwright Chrome children that outlived their parent (temp profile dir is Playwright's)
pkill -f 'playwright_chromiumdev_profile' 2>/dev/null && log "killed leftover Playwright Chrome processes"
if [ "${1:-}" != "--keep-vm" ] && alive "$RUN/qemu.pid"; then "$(dirname "$0")/20-vm.sh" down; fi
for p in "$SSH_PORT" "$VIEWER_PORT"; do
  if ss -ltn "sport = :$p" | grep -q LISTEN && ! alive "$RUN/qemu.pid"; then
    log "WARNING: port $p is held by something that is not this kit's VM: $(ss -ltnp "sport = :$p" | tail -1)"; fi
done
true
