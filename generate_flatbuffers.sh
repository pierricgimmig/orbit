#!/bin/bash
# Copyright (c) 2020 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

flatc --proto -o ./src/GrpcFlatBuffers/ ./src/GrpcProtos/services_ggp.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcFlatBuffers/ ./src/GrpcProtos/producer_side_services.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcFlatBuffers/ ./src/GrpcProtos/module.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcFlatBuffers/ ./src/GrpcProtos/services.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcFlatBuffers/ ./src/GrpcProtos/code_block.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcFlatBuffers/ ./src/GrpcProtos/capture.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcFlatBuffers/ ./src/GrpcProtos/symbol.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcFlatBuffers/ ./src/GrpcProtos/process.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcFlatBuffers/ ./src/GrpcProtos/tracepoint.proto --proto-namespace-suffix fbs
