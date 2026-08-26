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
echo "Matrix DISTRO=${DISTRO:-} VERSION=${VERSION:-} CODENAME=${CODENAME:-}"

# Read one field from /etc/os-release without leaking assignments into this
# shell. Ubuntu's file sets VERSION="20.04.6 LTS (Focal Fossa)", which would
# clobber the matrix VERSION=20.04 used to pin compilers.
os_release_field() {
  local field="$1"
  [[ -f /etc/os-release ]] || return 0
  # shellcheck disable=SC1091
  (
    . /etc/os-release
    case "${field}" in
      ID) printf '%s' "${ID:-}" ;;
      VERSION_ID) printf '%s' "${VERSION_ID:-}" ;;
      VERSION_CODENAME) printf '%s' "${VERSION_CODENAME:-}" ;;
      *) return 1 ;;
    esac
  )
}

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
  code="$(os_release_field VERSION_CODENAME)"
  case "${code}" in
    stretch|buster)
      echo "deb http://archive.debian.org/debian ${code} main" > /etc/apt/sources.list
      echo 'Acquire::Check-Valid-Until "false";' > /etc/apt/apt.conf.d/99archive
      rm -f /etc/apt/sources.list.d/* || true
      ;;
  esac
}

# Minimal EL/Amazon images ship curl-minimal. Installing the full `curl`
# package conflicts with it and dnf aborts the whole transaction — gcc
# never gets installed. Those images already provide /usr/bin/curl.
dnf_skip_full_curl() {
  case "${DISTRO:-}:${VERSION:-}" in
    almalinux:9|rocky:9|amazonlinux:2023) return 0 ;;
    *) return 1 ;;
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
  # Ubuntu 20.04's default GCC is 9; Abseil needs GCC 10+. Detect from
  # the matrix env *or* os-release VERSION_ID so a leaked VERSION= from
  # /etc/os-release cannot skip this pin (that is what failed run #4).
  if [[ "${DISTRO:-}" == "ubuntu" && "${VERSION:-}" == "20.04" ]] \
      || { [[ "$(os_release_field ID)" == "ubuntu" ]] \
           && [[ "$(os_release_field VERSION_ID)" == "20.04" ]]; }; then
    # gcc-10/g++-10 are in universe on Focal (main only has gcc-9).
    # Official ubuntu:20.04 already enables universe; this covers slim/debootstrap.
    echo "Ensuring Ubuntu 20.04 universe (needed for gcc-10)"
    if [[ -f /etc/apt/sources.list ]]; then
      sed -i -E '/ubuntu\.com\/ubuntu/ { /[[:space:]]universe([[:space:]]|$)/! s/[[:space:]]*$/ universe/ }' \
        /etc/apt/sources.list
    fi
    apt-get update || echo "WARN: apt-get update after enabling universe failed"
    if ! try_install "apt gcc-10" apt-get install -y --no-install-recommends gcc-10 g++-10; then
      echo "ERROR: Ubuntu 20.04 needs gcc-10/g++-10 (Abseil requires GCC 10+)"
      exit 1
    fi
    if ! command -v gcc-10 >/dev/null 2>&1 || ! command -v g++-10 >/dev/null 2>&1; then
      echo "ERROR: gcc-10/g++-10 did not land on PATH"
      exit 1
    fi
    # /usr/bin/gcc is a metapackage symlink to gcc-9, not an alternatives
    # slave. Bazel 9 + --incompatible_strict_action_env execs /usr/bin/gcc
    # for both target and [for tool] (exec/host) compiles; --action_env=CC
    # does not change that. Make the names Bazel actually runs be GCC 10.
    pin_cc_name() {
      local name="$1"
      local bin="$2"
      local dest="/usr/bin/${name}"
      if command -v update-alternatives >/dev/null 2>&1; then
        rm -f "${dest}"
        if update-alternatives --install "${dest}" "${name}" "${bin}" 100 \
            && update-alternatives --set "${name}" "${bin}"; then
          return 0
        fi
      fi
      ln -sfn "${bin}" "${dest}"
    }
    pin_cc_name gcc /usr/bin/gcc-10
    pin_cc_name g++ /usr/bin/g++-10
    pin_cc_name cc /usr/bin/gcc-10
    pin_cc_name c++ /usr/bin/g++-10
    export CC=/usr/bin/gcc-10
    export CXX=/usr/bin/g++-10
    local gcc_major=""
    gcc_major="$(gcc -dumpversion | cut -d. -f1)"
    echo "Using CC=${CC} CXX=${CXX} (Ubuntu 20.04); /usr/bin/gcc major=${gcc_major}"
    gcc -v || true
    if ! [[ "${gcc_major}" =~ ^[0-9]+$ ]] || [[ "${gcc_major}" -lt 10 ]]; then
      echo "ERROR: /usr/bin/gcc is still GCC ${gcc_major:-unknown}; need 10+"
      exit 1
    fi
  fi
}

install_dnf_yum() {
  if command -v dnf >/dev/null 2>&1; then
    dnf -y update --refresh || true
    if dnf_skip_full_curl; then
      echo "Skipping full curl package (${DISTRO} ${VERSION} ships curl-minimal)"
      try_install dnf dnf install -y gcc gcc-c++ python3 git ca-certificates unzip zip findutils || \
        try_install dnf-minimal dnf install -y gcc gcc-c++ python3 git ca-certificates unzip zip || true
    else
      try_install dnf dnf install -y gcc gcc-c++ python3 git curl ca-certificates unzip zip findutils || \
        try_install dnf-minimal dnf install -y gcc gcc-c++ python3 git curl ca-certificates unzip zip || true
    fi
  elif command -v yum >/dev/null 2>&1; then
    yum -y update || true
    try_install yum yum install -y gcc gcc-c++ python3 git curl ca-certificates unzip zip findutils || true
  else
    echo "WARN: no dnf/yum"
  fi
}

install_zypper() {
  if [[ "${DISTRO:-}" == "opensuse" && "${VERSION:-}" == "15.6" ]]; then
    # Leap 15.6 default gcc is gcc7 (too old for Abseil). gcc13 is in-tree.
    # Force-refresh so SLE-update metadata is not stale (that repo 404'd
    # versioned RPMs such as libatomic1 during the first matrix run).
    zypper --non-interactive refresh --force || zypper --non-interactive refresh || true
    try_install zypper zypper --non-interactive install -y \
      gcc13 gcc13-c++ python3 git curl ca-certificates unzip zip findutils || true
    if command -v gcc-13 >/dev/null 2>&1 && command -v g++-13 >/dev/null 2>&1; then
      export CC=gcc-13
      export CXX=g++-13
      echo "Using CC=${CC} CXX=${CXX} (openSUSE Leap 15.6)"
    fi
  else
    zypper --non-interactive refresh || true
    try_install zypper zypper --non-interactive install -y \
      gcc gcc-c++ python3 git curl ca-certificates unzip zip findutils || true
  fi
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

if ! command -v gcc >/dev/null 2>&1 \
    && ! command -v cc >/dev/null 2>&1 \
    && ! command -v g++ >/dev/null 2>&1 \
    && ! { [[ -n "${CC:-}" ]] && command -v "${CC}" >/dev/null 2>&1; }; then
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

# .bazelrc uses --incompatible_strict_action_env. Pin the compiler for
# repository configuration, target actions, and exec/host ([for tool])
# actions. --action_env alone does not reach the exec configuration.
if [[ -n "${CC:-}" ]]; then
  {
    echo "build --repo_env=CC=${CC}"
    echo "build --action_env=CC=${CC}"
    echo "build --host_action_env=CC=${CC}"
  } >> /tmp/ci.bazelrc
fi
if [[ -n "${CXX:-}" ]]; then
  {
    echo "build --repo_env=CXX=${CXX}"
    echo "build --action_env=CXX=${CXX}"
    echo "build --host_action_env=CXX=${CXX}"
  } >> /tmp/ci.bazelrc
fi

echo "=== bazel build //src/Service:OrbitService ==="
bazel --bazelrc=/tmp/ci.bazelrc build //src/Service:OrbitService
