//! Embed every file in viewer-dist/ as include_bytes! (no rust-embed / sha2).

use std::env;
use std::fs;
use std::path::PathBuf;

fn mime_for(name: &str) -> &'static str {
    if name.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if name.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if name.ends_with(".wasm") {
        "application/wasm"
    } else if name.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if name.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let dist = manifest.join("../../viewer-dist");
    println!("cargo:rerun-if-changed={}", dist.display());

    let mut code = String::from(
        "pub fn get(path: &str) -> Option<(&'static str, &'static [u8])> {\n    match path {\n",
    );
    if dist.is_dir() {
        let mut names: Vec<_> = fs::read_dir(&dist)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .collect();
        names.sort_by_key(|e| e.file_name());
        for entry in names {
            let name = entry.file_name().to_string_lossy().into_owned();
            println!("cargo:rerun-if-changed={}", entry.path().display());
            let abs = entry.path().canonicalize().unwrap();
            let mime = mime_for(&name);
            code.push_str(&format!(
                "        \"{}\" => Some((\"{}\", include_bytes!(\"{}\"))),\n",
                name,
                mime,
                abs.display()
            ));
        }
    }
    code.push_str("        _ => None,\n    }\n}\n");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("embedded_assets.rs");
    fs::write(out, code).unwrap();
}
