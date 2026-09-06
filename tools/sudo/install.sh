#!/bin/bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Lets one user run any binary named orbit-service with CAP_SYS_ADMIN,
# CAP_PERFMON and CAP_DAC_READ_SEARCH (as themselves, uid-wise, but
# CAP_SYS_ADMIN is root in all but name) without a password, through the
# wrapper next to this script. Run once, as root:
#
#   sudo tools/sudo/install.sh            # for the user running sudo
#   sudo tools/sudo/install.sh alice      # for another user
#
# Afterwards, from anywhere:
#
#   sudo -n orbit-service-sudo "$PWD/rust/crates/orbit-service/target/release/orbit-service" --serve 44766
#
# and tools/e2e/orbit_e2e.py --sudo uses it by itself. Remove with
#   sudo rm /etc/sudoers.d/orbit-service /usr/local/bin/orbit-service-sudo
set -eu
if [ "$(id -u)" != 0 ]; then
  echo "run as root: sudo $0" >&2
  exit 1
fi
user=${1:-${SUDO_USER:-}}
if [ -z "$user" ] || ! id -u -- "$user" >/dev/null 2>&1; then
  echo "which user? sudo $0 <user>" >&2
  exit 1
fi
here=$(cd -- "$(dirname -- "$0")" && pwd)
wrapper=/usr/local/bin/orbit-service-sudo
install -o root -g root -m 0755 -- "$here/orbit-service-sudo" "$wrapper"
rule=/etc/sudoers.d/orbit-service
tmp=$(mktemp)
cat > "$tmp" <<RULE
# Orbit: $user may run any binary named orbit-service they own, as
# themselves with CAP_SYS_ADMIN, CAP_PERFMON and CAP_DAC_READ_SEARCH,
# through the wrapper, which checks the name and the owner. CAP_SYS_ADMIN
# is root in all but name. Written by $0.
Defaults!$wrapper env_keep += "ORBIT_SOURCE_ROOTS ORBIT_E2E_STRESS ORBIT_UPROBE_RING_KB ORBIT_DRAIN_MS"
$user ALL=(root) NOPASSWD: $wrapper
RULE
visudo -c -q -f "$tmp"
install -o root -g root -m 0440 -- "$tmp" "$rule"
rm -f -- "$tmp"
echo "installed $wrapper and $rule for $user"
echo "check: sudo -n $wrapper /bin/true   # must say 'refused: /bin/true is not named orbit-service'"
