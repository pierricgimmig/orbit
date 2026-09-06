// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Shared dynamic hook request model; backend availability is checked at Start.

/// Ceiling on armed probes. Each hook costs two file descriptors and two
/// mappings per thread, so a careless selection over a 200-thread process is
/// a resource problem, not just a slow one.
pub const MAX_HOOKS: usize = 16;

/// One function to hook: where the probe goes, in file terms.
#[derive(Clone, Debug)]
pub struct HookSpec {
    pub function_id: u64,
    pub module_path: String,
    pub file_offset: u64,
    pub name: String,
}
