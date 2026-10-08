#!/usr/bin/env bash
# One-command media generation for the Orbit live viewer.
#   ./generate.sh --ref <git-ref> --out <dir> [options]
# Options:
#   --ref REF          git ref/sha/branch of pierricgimmig/orbit to build (from $ORBIT_REPO, else fetched read-only)
#   --binary PATH      use this prebuilt orbit-service (viewer embedded) instead of building --ref
#   --out DIR          output directory (default /workspace/orbit-media)
#   --only IDS         comma-separated clip ids to (re)record/convert (default: all, see clips.mjs)
#   --skip-existing    resume: keep clips already recorded without error in DIR/raw, record the rest, convert all
#   --steps LIST       subset of: prereqs,build,vm,target,record,convert,manifest,down (default: all)
#   --keep-vm          leave the VM running at the end (default: power it off)
#   --no-clean         keep existing outputs in DIR (default: a full run wipes DIR's media, raw/ and stills/)
# Publish the web-sized MP4s and posters into the landing page (do not point
# --out at tools/site/media; that directory is the publish step's output):
#   python3 publish_site_media.py --from <dir>
# Long runs: launch detached, e.g.
#   setsid nohup ./generate.sh --ref main --out /workspace/orbit-media > /dev/null 2>&1 &
#   tail -f /workspace/orbit-media/raw/generate.log
set -euo pipefail
cd "$(dirname "$0")"; . lib/config.sh
REF=""; BIN=""; OUT=/workspace/orbit-media; ONLY=""; STEPS="prereqs,build,vm,target,record,convert,manifest,down"; KEEP_VM=0; CLEAN=1; SKIP=""
CMDLINE="./generate.sh $*"
while [ $# -gt 0 ]; do case "$1" in
  --ref) REF="$2"; shift 2;; --binary) BIN="$2"; shift 2;; --out) OUT="$2"; shift 2;; --only) ONLY="$2"; shift 2;;
  --skip-existing) SKIP="--skip-existing"; CLEAN=0; shift;;
  --steps) STEPS="$2"; shift 2;; --keep-vm) KEEP_VM=1; shift;; --no-clean) CLEAN=0; shift;;
  -h|--help) sed -n 2,16p "$0"; exit 0;; *) die "unknown option $1";; esac; done
has() { [[ ",$STEPS," == *",$1,"* ]]; }
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd); mkdir -p "$OUT/raw" "$CACHE/run"
exec > >(tee -a "$OUT/raw/generate.log") 2>&1
log "generate: $CMDLINE (pid $$)"
# one run at a time
if [ -f "$CACHE/run/generate.pid" ] && kill -0 "$(cat "$CACHE/run/generate.pid")" 2>/dev/null; then die "another generate.sh is running (pid $(cat "$CACHE/run/generate.pid"))"; fi
echo $$ > "$CACHE/run/generate.pid"
SHA=""; T0=$(date '+%F %T')
# raw/commands.txt: every run that touched this output directory (MANIFEST.md lists them)
cleanup() { rc=$?; trap - EXIT
  if [ $rc -eq 0 ]; then echo "$T0 $CMDLINE" >> "$OUT/raw/commands.txt"; else echo "$T0 $CMDLINE  (aborted, rc $rc)" >> "$OUT/raw/commands.txt"; fi
  if [ $rc -ne 0 ]; then log "failed or interrupted (rc $rc): cleaning up"
    steps/05-stale.sh --keep-vm || true; steps/30-target.sh stop "$OUT/raw/guest-logs" 2>/dev/null || true
    [ $KEEP_VM = 1 ] || steps/20-vm.sh down || true; fi
  rm -f "$CACHE/run/generate.pid"; exit $rc; }
trap cleanup EXIT
trap 'exit 143' TERM INT HUP

# leftovers of an aborted previous run (recorder / Chrome / Xvfb / VM, via pidfiles)
steps/05-stale.sh $( [ $KEEP_VM = 1 ] && echo --keep-vm )
has prereqs && steps/00-prereqs.sh
if has build || has target; then
  if [ -z "$BIN" ]; then [ -n "$REF" ] || die "--ref or --binary is required"; BIN=$(steps/10-build.sh "$REF" | tail -1); fi
  [ -x "$BIN" ] || die "no service binary: $BIN"
fi
[ -n "$REF" ] && SHA=$(git -C "$CACHE/src" rev-parse "$REF^{commit}" 2>/dev/null || git -C "$ORBIT_REPO" rev-parse "$REF" 2>/dev/null || echo "")
if [ $CLEAN = 1 ] && [ -z "$ONLY" ] && has record; then
  log "cleaning previous outputs in $OUT"; rm -rf "$OUT/raw"/*.mkv "$OUT/raw/commands.txt" "$OUT/raw/clips.json" "$OUT/raw/convert.json" "$OUT/stills" "$OUT"/[0-9][0-9]-*.{mp4,webm,gif,png} "$OUT/MANIFEST.md"
fi
has vm && steps/20-vm.sh up
has target && steps/30-target.sh start "$BIN"
if has record; then
  log "recording clips (one viewer page; see $OUT/raw/record.log)"
  rc=0; node steps/40-record.mjs --out "$OUT" ${ONLY:+--only "$ONLY"} $SKIP || rc=$?
  curl -s -m 10 "127.0.0.1:$VIEWER_PORT/api/status" > "$OUT/raw/status-final.json" || true
  [ $rc = 0 ] || log "WARNING: recorder exit $rc (scene errors are listed in MANIFEST.md)"
fi
if has target; then steps/30-target.sh stop "$OUT/raw/guest-logs"; fi
if has down && [ $KEEP_VM = 0 ]; then steps/20-vm.sh down; fi
has convert && python3 steps/50-convert.py --out "$OUT" ${ONLY:+--only "$ONLY"}
has manifest && python3 steps/60-manifest.py --out "$OUT" --ref "$REF" --sha "$SHA" --cmd "$CMDLINE"
log "done: $OUT"
