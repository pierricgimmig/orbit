#!/usr/bin/env bash
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Host-side driver: materialize a Docker image for one distro version, run
# install-and-build.sh inside it, and write ci-results/<distro>-<version>.json.

set -u

DISTRO="${DISTRO:?DISTRO is required}"
VERSION="${VERSION:?VERSION is required}"
CODENAME="${CODENAME:-}"
IMAGE="${IMAGE:?IMAGE is required}"
IMAGE_ALT="${IMAGE_ALT:-}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TAG="orbit-ci:${DISTRO}-${VERSION}"
RESULT_DIR="${ROOT}/ci-results"
RESULT_FILE="${RESULT_DIR}/${DISTRO}-${VERSION}.json"
LOG_FILE="${RESULT_DIR}/${DISTRO}-${VERSION}.log"
CACHE_DIR="${HOME}/.cache/orbit-ci"
BAZELISK="${CACHE_DIR}/bazelisk-linux-amd64"

mkdir -p "${RESULT_DIR}" "${CACHE_DIR}/bazel-disk" "${CACHE_DIR}/bazel-repo" "${CACHE_DIR}/bazelisk"

write_result() {
  local status="$1"
  local error="$2"
  jq -n \
    --arg distro "${DISTRO}" \
    --arg version "${VERSION}" \
    --arg codename "${CODENAME}" \
    --arg status "${status}" \
    --arg error "${error}" \
    '{distro:$distro,version:$version,codename:$codename,status:$status,error:$error}' \
    > "${RESULT_FILE}"
}

# Pick a compact, human-readable line from a build/materialize log.
# $2 is "first" (guest compile) or "last" (docker/debootstrap retries).
useful_error() {
  local log="$1"
  local which="${2:-first}"
  local pick="head"
  if [[ "${which}" == "last" ]]; then
    pick="tail"
  fi
  local line=""
  if [[ -f "${log}" ]]; then
    line="$(
      grep -E -i \
        'no image:|ERROR: |error: |FATAL: |GLIBC_[0-9]|missing compiler|no Bazel|cannot execute|not found|Cannot connect|debootstrap|failed to solve|permission denied' \
        "${log}" \
        | grep -v -E '^(Get:|Hit:|Ign:|Fetched |Reading package|Selecting previously|Preparing to unpack|Unpacking |Setting up )' \
        | ${pick} -n 1 || true
    )"
    if [[ -z "${line}" ]]; then
      line="$(grep -E -i 'error|fail|fatal' "${log}" | tail -n 1 || true)"
    fi
    if [[ -z "${line}" ]]; then
      line="$(grep -E -v '^[[:space:]]*$' "${log}" | tail -n 1 || true)"
    fi
  fi
  if [[ -z "${line}" ]]; then
    line="unknown failure"
  fi
  printf '%s' "${line}" | tr '\n' ' ' | head -c 240
}

first_useful_error() {
  useful_error "$1" first
}

last_useful_error() {
  useful_error "$1" last
}

ensure_docker() {
  echo "=== docker version ==="
  if ! docker version; then
    write_result fail "docker not available: docker version failed"
    exit 1
  fi
  echo "=== docker info ==="
  if ! docker info; then
    echo "docker info failed; trying to start the daemon"
    sudo service docker start 2>/dev/null || sudo systemctl start docker 2>/dev/null || true
    if ! docker info; then
      write_result fail "docker not available: docker info failed (daemon not running?)"
      exit 1
    fi
  fi
  echo "====================="
}

trap 'if [[ ! -f "${RESULT_FILE}" ]]; then write_result fail "script aborted before writing a result"; fi' EXIT

ensure_docker

try_build_from() {
  local from_image="$1"
  echo "Building wrapper FROM ${from_image}"
  docker build --pull --platform linux/amd64 \
    -f "${ROOT}/ci/linux/Dockerfile" \
    --build-arg "IMAGE=${from_image}" \
    -t "${TAG}" \
    "${ROOT}/ci/linux"
}

try_ubuntu_archive() {
  local mirror="$1"
  echo "Debootstrapping Ubuntu ${CODENAME} from ${mirror}"
  docker build --platform linux/amd64 \
    -f "${ROOT}/ci/linux/Dockerfile.ubuntu-archive" \
    --build-arg "SUITE=${CODENAME}" \
    --build-arg "MIRROR=${mirror}" \
    -t "${TAG}" \
    "${ROOT}/ci/linux"
}

materialize() {
  if try_build_from "${IMAGE}"; then
    return 0
  fi
  echo "Primary image ${IMAGE} failed"
  if [[ -n "${IMAGE_ALT}" ]]; then
    if try_build_from "${IMAGE_ALT}"; then
      return 0
    fi
    echo "Fallback image ${IMAGE_ALT} failed"
  fi
  if [[ "${DISTRO}" == "ubuntu" && -n "${CODENAME}" ]]; then
    if try_ubuntu_archive "http://old-releases.ubuntu.com/ubuntu"; then
      return 0
    fi
    if try_ubuntu_archive "http://archive.ubuntu.com/ubuntu"; then
      return 0
    fi
  fi
  return 1
}

if ! materialize >"${LOG_FILE}" 2>&1; then
  cat "${LOG_FILE}"
  detail="$(last_useful_error "${LOG_FILE}")"
  err="no image: cannot materialize ${DISTRO} ${VERSION} (${IMAGE}): ${detail}"
  echo "${err}"
  write_result fail "${err}"
  exit 1
fi
cat "${LOG_FILE}"

# v1.26.1 is not a bazelisk release (404). Pin a real tag; bazelisk then
# downloads the Bazel in .bazelversion.
BAZELISK_VERSION="v1.26.0"
BAZELISK_URL="https://github.com/bazelbuild/bazelisk/releases/download/${BAZELISK_VERSION}/bazelisk-linux-amd64"

if [[ ! -x "${BAZELISK}" ]]; then
  echo "Downloading bazelisk ${BAZELISK_VERSION}"
  curl_err=""
  if ! curl_err="$(curl -fsSL -o "${BAZELISK}" "${BAZELISK_URL}" 2>&1)"; then
    err="no Bazel: failed to download bazelisk ${BAZELISK_VERSION} (${BAZELISK_URL}): ${curl_err}"
    echo "${err}"
    write_result fail "${err}"
    exit 1
  fi
  if [[ "$(head -c 4 "${BAZELISK}")" != $'\x7fELF' ]]; then
    err="no Bazel: downloaded bazelisk ${BAZELISK_VERSION} is not an ELF binary"
    echo "${err}"
    write_result fail "${err}"
    exit 1
  fi
  chmod +x "${BAZELISK}"
fi

echo "Running ${DISTRO} ${VERSION} (${CODENAME}) in ${TAG}"
set +e
docker run --rm --privileged \
  --network host \
  -v "${ROOT}:/workspace:rw" \
  -v "${CACHE_DIR}/bazel-disk:/cache/bazel-disk:rw" \
  -v "${CACHE_DIR}/bazel-repo:/cache/bazel-repo:rw" \
  -v "${CACHE_DIR}/bazelisk:/root/.cache/bazelisk:rw" \
  -v "${BAZELISK}:/usr/local/bin/bazel:ro" \
  -e "DISTRO=${DISTRO}" \
  -e "VERSION=${VERSION}" \
  -e "CODENAME=${CODENAME}" \
  -w /workspace \
  "${TAG}" \
  /bin/sh -c '
    if ! command -v bash >/dev/null 2>&1; then
      if command -v apk >/dev/null 2>&1; then
        apk add --no-cache bash >/tmp/apk-bash.log 2>&1 || true
      fi
    fi
    if command -v bash >/dev/null 2>&1; then
      exec bash /workspace/ci/linux/install-and-build.sh
    fi
    exec /bin/sh /workspace/ci/linux/install-and-build.sh
  ' >"${LOG_FILE}" 2>&1
rc=$?
set -e
cat "${LOG_FILE}"

if [[ ${rc} -eq 0 ]]; then
  write_result pass ""
  exit 0
fi

write_result fail "$(first_useful_error "${LOG_FILE}")"
exit "${rc}"
