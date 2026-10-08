#!/usr/bin/env bash
# Recreate the Debian 13 test VM from the official cloud image (only needed if $VM_DISK is missing).
# Produces $VM_DIR/{debian13.qcow2 (base), disk.qcow2 (overlay, 8G), seed.iso, vmkey, vmkey.pub}.
set -euo pipefail; . "$(dirname "$0")/../lib/config.sh"
mkdir -p "$VM_DIR"; cd "$VM_DIR"
[ -f debian13.qcow2 ] || curl -L -o debian13.qcow2 https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2
[ -f vmkey ] || ssh-keygen -q -t ed25519 -N '' -C autoinstr-vm -f vmkey
cat > user-data <<UD
#cloud-config
users:
  - name: $VM_USER
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - $(cat vmkey.pub)
ssh_pwauth: false
UD
printf 'instance-id: autoinstr\nlocal-hostname: autoinstr-vm\n' > meta-data
genisoimage -quiet -output seed.iso -volid cidata -joliet -rock user-data meta-data
[ -f disk.qcow2 ] && die "disk.qcow2 exists; move it away first"
qemu-img create -q -f qcow2 -F qcow2 -b debian13.qcow2 disk.qcow2 8G
log "first boot with the cloud-init seed (TCG; takes a few minutes)"
RUN="$CACHE/run"; mkdir -p "$RUN"
qemu-system-x86_64 -accel tcg,thread=multi -cpu max -smp "$VM_SMP" -m "$VM_MEM" \
  -drive file=disk.qcow2,if=virtio,format=qcow2 -drive file=seed.iso,media=cdrom \
  -netdev "user,id=n0,hostfwd=tcp:127.0.0.1:$SSH_PORT-:22" -device virtio-net-pci,netdev=n0 \
  -display none -monitor none -serial "file:$RUN/vm-create-serial.log" &
Q=$!
for i in $(seq 1 300); do vmssh true 2>/dev/null && break; sleep 3; done
vmssh 'cloud-init status --wait >/dev/null; uname -r; ls /sys/bus/event_source/devices/ | tr "\n" " "' || die "VM did not come up"
vmssh 'sudo -n systemctl poweroff' || true; wait $Q || true
log "VM created: $VM_DIR/disk.qcow2 (the seed is only needed on first boot)"
