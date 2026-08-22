#!/bin/sh
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Expands @PLACEHOLDER@ in a version template from Bazel's stable-status file.
#
# Usage: expand_version.sh <stable-status.txt> <template> <output>

set -eu

status=$1
template=$2
output=$3

# placeholder=workspace-status-key
mapping='
VERSION_STRING=STABLE_ORBIT_VERSION_STRING
MAJOR_VERSION=STABLE_ORBIT_MAJOR_VERSION
MINOR_VERSION=STABLE_ORBIT_MINOR_VERSION
COMMIT_HASH=STABLE_ORBIT_COMMIT_HASH
COMPILER_STRING=STABLE_ORBIT_COMPILER
BUILD_TIMESTAMP_STRING=STABLE_ORBIT_BUILD_TIMESTAMP
BUILD_MACHINE_STRING=STABLE_ORBIT_BUILD_MACHINE
BUILD_OS_NAME=STABLE_ORBIT_BUILD_OS_NAME
BUILD_OS_RELEASE=STABLE_ORBIT_BUILD_OS_RELEASE
BUILD_OS_VERSION=STABLE_ORBIT_BUILD_OS_VERSION
BUILD_OS_PLATFORM=STABLE_ORBIT_BUILD_OS_PLATFORM
'

program=''
for pair in $mapping; do
  placeholder=${pair%%=*}
  key=${pair#*=}
  # Everything after the key on its line is the value; keys are unique.
  value=$(awk -v key="$key" '$1 == key { $1 = ""; sub(/^ /, ""); print }' "$status")
  # Escape the characters sed treats specially in a replacement.
  value=$(printf '%s' "$value" | sed -e 's/[&\\|]/\\&/g')
  program="${program}
s|@${placeholder}@|${value}|g"
done

sed -e "$program" "$template" > "$output"
