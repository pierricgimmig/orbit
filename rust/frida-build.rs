// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Shared native devkit linking for the small Core and Gum ABI adapters.
fn build_frida(component: &str, source: &str) {
    let target = std::env::var("TARGET").unwrap();
    let root = std::env::var_os("ORBIT_FRIDA_DEVKIT_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/frida-devkits")
        });
    let kit = root.join(&target).join(component);
    assert!(
        kit.join(format!("frida-{component}.h")).is_file(),
        "missing Frida {component} devkit at {}; run tools/frida/devkits.sh {target}",
        kit.display()
    );
    println!("cargo:rerun-if-env-changed=ORBIT_FRIDA_DEVKIT_ROOT");
    println!("cargo:rerun-if-changed={source}");
    cc::Build::new()
        .file(source)
        .include(&kit)
        .flag_if_supported("-fvisibility=hidden")
        .compile(&format!("orbit-frida-{component}-adapter"));
    println!("cargo:rustc-link-search=native={}", kit.display());
    println!("cargo:rustc-link-lib=static=frida-{component}");
    if target.contains("apple") {
        for lib in ["resolv", "pthread", "bsm", "dl", "m"] {
            println!("cargo:rustc-link-lib={lib}");
        }
        for framework in [
            "Foundation",
            "Security",
            "CoreFoundation",
            "SystemConfiguration",
            "AppKit",
            "IOKit",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    } else {
        for lib in ["resolv", "dl", "m", "pthread", "rt"] {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
}
