//! Embed files under viewer-dist/ (including wasm-bindgen snippets/).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

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

fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if path.is_dir() {
            collect_files(&path, &rel, out);
        } else if path.is_file() {
            out.push((rel, path));
        }
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let dist = manifest.join("../../viewer-dist");
    println!("cargo:rerun-if-changed={}", dist.display());

    let mut code = String::from(
        "pub fn get(path: &str) -> Option<(&'static str, &'static [u8])> {\n    match path {\n",
    );
    let mut files = Vec::new();
    if dist.is_dir() {
        collect_files(&dist, "", &mut files);
    }
    for (rel, path) in files {
        println!("cargo:rerun-if-changed={}", path.display());
        let abs = path.canonicalize().unwrap();
        let mime = mime_for(&rel);
        code.push_str(&format!(
            "        \"{}\" => Some((\"{}\", include_bytes!(\"{}\"))),\n",
            rel,
            mime,
            abs.display()
        ));
    }
    code.push_str("        _ => None,\n    }\n}\n");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("embedded_assets.rs");
    fs::write(out, code).unwrap();
}
