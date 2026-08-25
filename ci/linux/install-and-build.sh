#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Guest-side: install the minimum tools this userspace can offer, then
# `bazel build //src/Service:OrbitService`. Failures must be loud.

set -u

export DEBIAN_FRONTEND=noninteractive
export TZ=UTC
export PATH="/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:${PATH:-}"

if [[ -f /etc/os-release ]]; then
  echo "=== /etc/os-release ==="
  cat /etc/os-release
  echo "======================="
fi

if command -v git >/dev/null 2>&1; then
  git config --global --add safe.directory /workspace || true
fi

try_install() {
  local mgr="$1"
  shift
  echo "Trying ${mgr}: $*"
  if "$@"; then
    echo "OK: ${mgr}"
    return 0
  fi
  echo "WARN: ${mgr} failed"
  return 1
}

rewrite_urls() {
  local file="$1"
  local mirror="$2"
  local pattern="$3"
  if [[ -f "${file}" ]]; then
    sed -i "s|${pattern}|${mirror}|g" "${file}" || true
  fi
}

fix_ubuntu_mirrors() {
  local mirror="$1"
  local files=(/etc/apt/sources.list /etc/apt/sources.list.d/ubuntu.sources)
  local f
  for f in "${files[@]}"; do
    rewrite_urls "${f}" "${mirror}" 'http://[^ ]*ubuntu.com/ubuntu'
    rewrite_urls "${f}" "${mirror}" 'https://[^ ]*ubuntu.com/ubuntu'
  done
}

fix_debian_archive() {
  local code=""
  if [[ -f /etc/os-release ]]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    code="${VERSION_CODENAME:-}"
  fi
  case "${code}" in
    stretch|buster)
      echo "deb http://archive.debian.org/debian ${code} main" > /etc/apt/sources.list
      echo 'Acquire::Check-Valid-Until "false";' > /etc/apt/apt.conf.d/99archive
      rm -f /etc/apt/sources.list.d/* || true
      ;;
  esac
}

install_apt() {
  fix_debian_archive
  if ! apt-get update; then
    echo "apt-get update failed; rewriting Ubuntu mirrors to old-releases"
    fix_ubuntu_mirrors "http://old-releases.ubuntu.com/ubuntu"
    if ! apt-get update; then
      echo "old-releases failed; trying archive.ubuntu.com"
      fix_ubuntu_mirrors "http://archive.ubuntu.com/ubuntu"
      apt-get update || echo "WARN: apt-get update still failing"
    fi
  fi
  local pkg
  for pkg in ca-certificates curl wget git gcc g++ python3 unzip zip findutils; do
    try_install "apt ${pkg}" apt-get install -y --no-install-recommends "${pkg}" || true
  done
}

install_dnf_yum() {
  if command -v dnf >/dev/null 2>&1; then
    dnf -y update --refresh || true
    try_install dnf dnf install -y gcc gcc-c++ python3 git curl ca-certificates unzip zip findutils || \
      try_install dnf-minimal dnf install -y gcc gcc-c++ python3 git curl ca-certificates unzip zip || true
  elif command -v yum >/dev/null 2>&1; then
    yum -y update || true
    try_install yum yum install -y gcc gcc-c++ python3 git curl ca-certificates unzip zip findutils || true
  else
    echo "WARN: no dnf/yum"
  fi
}

install_zypper() {
  zypper --non-interactive refresh || true
  try_install zypper zypper --non-interactive install -y \
    gcc gcc-c++ python3 git curl ca-certificates unzip zip findutils || true
}

install_apk() {
  apk update || true
  try_install apk apk add --no-cache \
    bash gcc g++ python3 git curl ca-certificates unzip zip || true
}

install_pacman() {
  pacman -Syu --noconfirm || true
  try_install pacman pacman -S --noconfirm \
    gcc python git curl unzip zip ca-certificates || true
}

if command -v apt-get >/dev/null 2>&1; then
  install_apt
elif command -v dnf >/dev/null 2>&1 || command -v yum >/dev/null 2>&1; then
  install_dnf_yum
elif command -v zypper >/dev/null 2>&1; then
  install_zypper
elif command -v apk >/dev/null 2>&1; then
  install_apk
elif command -v pacman >/dev/null 2>&1; then
  install_pacman
else
  echo "WARN: no known package manager"
fi

if ! command -v gcc >/dev/null 2>&1 && ! command -v cc >/dev/null 2>&1 && ! command -v g++ >/dev/null 2>&1; then
  echo "ERROR: missing compiler: gcc/g++/cc not installed"
fi

if [[ -f /workspace/.bazelversion ]]; then
  export USE_BAZEL_VERSION
  USE_BAZEL_VERSION="$(tr -d '[:space:]' < /workspace/.bazelversion)"
fi

if ! command -v bazel >/dev/null 2>&1; then
  echo "ERROR: no Bazel: bazelisk is not on PATH"
  exit 1
fi

if ! bazel version; then
  echo "ERROR: no Bazel: bazelisk/bazel cannot run on this userspace (too-old glibc or incompatible binary)"
  exit 1
fi

cat > /tmp/ci.bazelrc <<'EOF'
common --announce_rc
common --color=no
common --curses=no
build --spawn_strategy=standalone
build --genrule_strategy=standalone
build --disk_cache=/cache/bazel-disk
build --repository_cache=/cache/bazel-repo
build --keep_going
EOF

echo "=== bazel build //src/Service:OrbitService ==="
bazel --bazelrc=/tmp/ci.bazelrc build //src/Service:OrbitService
