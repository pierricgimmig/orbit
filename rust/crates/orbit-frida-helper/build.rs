// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

include!("../../frida-build.rs");
fn main() {
    build_frida("core", "src/core.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/loader.c");
        let output =
            std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("loader.dylib");
        let status = cc::Build::new()
            .get_compiler()
            .to_command()
            .args(["-dynamiclib", "-O2", "src/loader.c", "-o"])
            .arg(&output)
            .status()
            .expect("compile Darwin loader");
        assert!(status.success(), "Darwin loader compilation failed");
    }
}
