#!/usr/bin/env bash
# usage: steps/20-vm.sh up|down|status
# QEMU **TCG only** (software emulation). Never add -enable-kvm / -accel kvm: nested KVM crashes
# the host kernel of the box this kit was written on. The guest is needed because the box kernel
# has no uprobe PMU; the guest (Debian 13 cloud kernel) has uprobes.
set -euo pipefail; . "$(dirname "$0")/../lib/config.sh"
RUN="$CACHE/run"; mkdir -p "$RUN"; PIDF="$RUN/qemu.pid"
alive() { [ -f "$PIDF" ] && kill -0 "$(cat "$PIDF")" 2>/dev/null; }
case "${1:-status}" in
up)
  if alive; then log "VM already running (pid $(cat "$PIDF"))"; else
    [ -f "$VM_DISK" ] || die "no VM disk at $VM_DISK (run steps/vm-create.sh, see README)"
    for p in "$SSH_PORT" "$VIEWER_PORT"; do ss -ltn "sport = :$p" | grep -q LISTEN && die "port $p already in use"; done
    log "booting VM (TCG, ${VM_SMP} vCPU, ${VM_MEM} MB)"
    nohup qemu-system-x86_64 -accel tcg,thread=multi -cpu max -smp "$VM_SMP" -m "$VM_MEM" \
      -drive "file=$VM_DISK,if=virtio,format=qcow2" \
      -netdev "user,id=n0,hostfwd=tcp:127.0.0.1:$SSH_PORT-:22,hostfwd=tcp:127.0.0.1:$VIEWER_PORT-:$GUEST_HTTP_PORT" \
      -device virtio-net-pci,netdev=n0 -display none -monitor none -serial "file:$RUN/vm-serial.log" \
      > "$RUN/qemu.out" 2>&1 < /dev/null &
    echo $! > "$PIDF"
  fi
  for i in $(seq 1 90); do vmssh true 2>/dev/null && { log "VM ssh up"; exit 0; }; alive || die "qemu exited: $(cat "$RUN/qemu.out")"; sleep 2; done
  die "VM ssh did not come up in 180 s (see $RUN/vm-serial.log)";;
down)
  if alive; then
    log "powering off VM"; vmssh 'sudo -n systemctl poweroff' 2>/dev/null || true
    for i in $(seq 1 30); do alive || break; sleep 2; done
    alive && { log "VM still up, killing qemu"; kill "$(cat "$PIDF")"; sleep 2; }
  fi
  rm -f "$PIDF"; log "VM down";;
status) alive && echo "running pid $(cat "$PIDF")" || echo "stopped";;
esac
