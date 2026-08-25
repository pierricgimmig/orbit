# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Thin wrapper around an official (or fallback) distro image. Packages and
# Bazel are installed at run time so a missing compiler / too-old glibc / no
# Bazel shows up as a failed job with a log, not a silent matrix skip.

ARG IMAGE=ubuntu:24.04
FROM ${IMAGE}

LABEL org.opencontainers.image.source="https://github.com/pierricgimmig/orbit"
LABEL orbit.ci="linux-distro-matrix"
