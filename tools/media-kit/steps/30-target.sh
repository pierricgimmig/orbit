#!/usr/bin/env bash
# usage: steps/30-target.sh start <orbit-service-binary> | stop [<logs-out-dir>] | status
# start: compile dummy/game_loop.c (box and guest are both Debian 13 / glibc 2.41), copy it and the
#        service into the guest, start orbit-service as root (--serve 44766) and game_loop as the user.
set -euo pipefail; . "$(dirname "$0")/../lib/config.sh"
G=/home/$VM_USER/media
case "${1:-status}" in
start)
  BIN="${2:?orbit-service binary}"; [ -x "$BIN" ] || die "not executable: $BIN"
  B="$CACHE/run"; mkdir -p "$B"
  # Built in $B with a relative source name and the comp dir mapped to the guest dir, so the service
  # (whose source root is its cwd) can show game_loop.c in the Code tab.
  cp "$KIT/dummy/game_loop.c" "$B/game_loop.c"
  ( cd "$B" && gcc -O0 -g -fno-omit-frame-pointer -fno-inline -fdebug-prefix-map="$B"=$G -o game_loop game_loop.c )
  vmssh "sudo -n pkill -INT -f '^[.]/orbit-service' ; pkill -x game_loop ; sleep 2; sudo -n pkill -KILL -f '^[.]/orbit-service' ; rm -rf $G; mkdir -p $G/logs" || true
  log "copying service ($(du -h "$BIN" | cut -f1)) and game_loop into the guest"
  vmscp "$BIN" "$G/orbit-service"; vmscp "$B/game_loop" "$G/game_loop"; vmscp "$B/game_loop.c" "$G/game_loop.c"
  vmssh -n "cd $G && (sudo -n setsid nohup ./orbit-service --log-dir $G/logs --serve $GUEST_HTTP_PORT > logs/service-stdout.log 2>&1 < /dev/null &); sleep 1"
  for i in $(seq 1 60); do curl -s -m 3 "127.0.0.1:$VIEWER_PORT/api/status" | grep -q '"capturing"' && break; sleep 1; done
  curl -s -m 3 "127.0.0.1:$VIEWER_PORT/api/status" | grep -q '"capturing"' || die "orbit-service did not answer on 127.0.0.1:$VIEWER_PORT"
  vmssh -n "cd $G && (setsid nohup ./game_loop $GAME_LOOP_PHASE2_S > logs/game_loop.out 2>&1 < /dev/null &); sleep 1; pgrep -x game_loop" > "$B/game_loop.pid"
  log "orbit-service up; game_loop pid $(cat "$B/game_loop.pid") (phase2 at ${GAME_LOOP_PHASE2_S} s)";;
stop)
  OUTL="${2:-}"
  vmssh "pkill -x game_loop; sudo -n pkill -INT -f '^[.]/orbit-service'; sleep 4; sudo -n pkill -KILL -f '^[.]/orbit-service'; true" 2>/dev/null || true
  if [ -n "$OUTL" ]; then mkdir -p "$OUTL"; vmssh "cd $G/logs && tar cz ." 2>/dev/null | tar xz -C "$OUTL" || true; fi
  log "game_loop and orbit-service stopped";;
status) vmssh "pgrep -af 'game_loop|orbit-service'" || true;;
esac
