#!/bin/bash
# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# authoring helper: ./tools-dev.sh -e 'js'  |  ./tools-dev.sh clip <id>  (needs 40-record.mjs --serve running)
if [ "$1" = "clip" ]; then curl -s -m 900 -XPOST "127.0.0.1:9333/clip?id=$2"; else curl -s -m 900 --data-binary "$2" 127.0.0.1:9333/run; fi
