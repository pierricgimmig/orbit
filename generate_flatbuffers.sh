#!/bin/bash
# Copyright (c) 2020 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

flatc --proto -o ./src/GrpcProtos/schemas/ ./src/GrpcProtos/services_ggp.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcProtos/schemas/ ./src/GrpcProtos/producer_side_services.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcProtos/schemas/ ./src/GrpcProtos/module.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcProtos/schemas/ ./src/GrpcProtos/services.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcProtos/schemas/ ./src/GrpcProtos/code_block.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcProtos/schemas/ ./src/GrpcProtos/capture.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcProtos/schemas/ ./src/GrpcProtos/symbol.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcProtos/schemas/ ./src/GrpcProtos/process.proto --proto-namespace-suffix fbs
flatc --proto -o ./src/GrpcProtos/schemas/ ./src/GrpcProtos/tracepoint.proto --proto-namespace-suffix fbs
