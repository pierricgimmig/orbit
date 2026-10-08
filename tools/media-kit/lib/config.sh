# Shared configuration. Every value can be overridden from the environment.
KIT="${KIT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
# The Orbit checkout that contains this kit (tools/media-kit). Override to
# point at another clone; the build step only fetches, and never edits it.
ORBIT_REPO="${ORBIT_REPO:-$(cd "$KIT/../.." && pwd)}"
CACHE="${CACHE:-$KIT/.cache}"                                    # build worktrees, binaries
VM_DIR="${VM_DIR:-/workspace/autoinstr-test/vm}"                 # disk.qcow2 + vmkey (see README / steps/vm-create.sh)
VM_DISK="${VM_DISK:-$VM_DIR/disk.qcow2}"
VM_KEY="${VM_KEY:-$VM_DIR/vmkey}"
VM_USER="${VM_USER:-tester}"
VM_SMP="${VM_SMP:-6}"
VM_MEM="${VM_MEM:-2048}"
SSH_PORT="${SSH_PORT:-2222}"
VIEWER_PORT="${VIEWER_PORT:-44900}"                              # box 127.0.0.1:VIEWER_PORT -> guest :44766
GUEST_HTTP_PORT=44766
GAME_LOOP_PHASE2_S="${GAME_LOOP_PHASE2_S:-40}"                   # game_loop starts phase2_pathfind after N s
XDISPLAY="${XDISPLAY:-:77}"
REC_W="${REC_W:-1600}"; REC_H="${REC_H:-1000}"; REC_FPS="${REC_FPS:-30}"
GIF_W="${GIF_W:-960}"; GIF_FPS="${GIF_FPS:-15}"; GIF_QUALITY="${GIF_QUALITY:-90}"
NODE_DIR="$KIT/node"                                             # playwright-core lives here (npm ci)
CHROME="${CHROME:-/usr/bin/google-chrome}"

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() { log "ERROR: $*"; exit 1; }
vmssh() {
  ssh -q -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=10 \
      -o ServerAliveInterval=15 -o LogLevel=ERROR -i "$VM_KEY" -p "$SSH_PORT" "$VM_USER@127.0.0.1" "$@"
}
vmscp() {  # vmscp <local> <remote-path>
  scp -q -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -i "$VM_KEY" -P "$SSH_PORT" "$1" "$VM_USER@127.0.0.1:$2"
}
REC_DPR="${REC_DPR:-1.25}"
export VIEWER_PORT XDISPLAY REC_W REC_H REC_FPS REC_DPR CHROME GIF_W GIF_FPS
