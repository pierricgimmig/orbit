// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "PdbFileTest.h"

// Must be included after PdbFileTest.h
#include "PdbFileRaw.h"

using orbit_object_utils::PdbFileRaw;

INSTANTIATE_TYPED_TEST_SUITE_P(PdbFileRawTest, PdbFileTest, ::testing::Types<PdbFileRaw>);
