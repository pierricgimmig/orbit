// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use perfr_frida_build::build_frida;

fn main() {
    if std::env::var_os("CARGO_FEATURE_NATIVE").is_some() {
        build_frida("gum", "src/gum.c");
    }
}
