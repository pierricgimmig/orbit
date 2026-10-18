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
    if target.contains("windows") {
        // The MSVC devkits are built against the static CRT (/MT) and lean
        // on these system libraries; the .lib carries no defaultlib
        // directives for them.
        for lib in [
            "ws2_32", "winmm", "shlwapi", "crypt32", "secur32", "iphlpapi", "psapi", "dnsapi", "setupapi",
            "ole32", "shell32", "user32", "gdi32", "advapi32", "bcrypt", "dbghelp", "ntdll", "userenv", "version",
        ] {
            println!("cargo:rustc-link-lib={lib}");
        }
        return;
    }
    if target.contains("apple") {
        // Do not make the target agent depend on unused dylibs/frameworks:
        // Darwin's injector resolves dependencies in the target process.
        println!("cargo:rustc-link-arg=-Wl,-dead_strip_dylibs");
        for lib in ["resolv", "m"] {
            println!("cargo:rustc-link-lib={lib}");
        }
        if component == "core" {
            for lib in ["bsm", "dl"] {
                println!("cargo:rustc-link-lib={lib}");
            }
            for framework in ["Foundation", "CoreFoundation", "AppKit"] {
                println!("cargo:rustc-link-lib=framework={framework}");
            }
        }
    } else {
        for lib in ["resolv", "dl", "m", "pthread", "rt"] {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
}
