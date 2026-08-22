//! Embed files under viewer-dist/ (including wasm-bindgen snippets/).
//!
//! Assets are copied into OUT_DIR and `include_bytes!`d relative to the
//! generated file. Absolute source paths would not survive Bazel's
//! sandbox, where the build script and the rustc action run separately.

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

/// `viewer-dist/` sits at the workspace root, but the build script's manifest
/// dir differs between Cargo (crates/orbit-live-server) and Bazel (the package
/// holding BUILD.bazel), so walk up instead of hard-coding `../..`.
fn find_viewer_dist() -> Option<PathBuf> {
    if let Ok(explicit) = env::var("ORBIT_VIEWER_DIST") {
        let path = PathBuf::from(explicit);
        return path.is_dir().then_some(path);
    }
    let mut dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").ok()?);
    loop {
        let candidate = dir.join("viewer-dist");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=ORBIT_VIEWER_DIST");
    let dist = find_viewer_dist();
    if let Some(dist) = &dist {
        println!("cargo:rerun-if-changed={}", dist.display());
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let assets_dir = out_dir.join("viewer_assets");

    let mut code = String::from(
        "pub fn get(path: &str) -> Option<(&'static str, &'static [u8])> {\n    match path {\n",
    );
    let mut files = Vec::new();
    if let Some(dist) = &dist {
        collect_files(dist, "", &mut files);
    }
    for (index, (rel, path)) in files.into_iter().enumerate() {
        println!("cargo:rerun-if-changed={}", path.display());
        // Flat, index-based names: asset paths carry subdirectories (wasm-bindgen
        // snippets/) and would otherwise need mkdir -p under OUT_DIR.
        let copied = format!("viewer_assets/{index}.bin");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::copy(&path, out_dir.join(&copied)).unwrap();
        let mime = mime_for(&rel);
        code.push_str(&format!(
            "        \"{}\" => Some((\"{}\", include_bytes!(\"{}\"))),\n",
            rel, mime, copied
        ));
    }
    code.push_str("        _ => None,\n    }\n}\n");

    fs::write(out_dir.join("embedded_assets.rs"), code).unwrap();
}
