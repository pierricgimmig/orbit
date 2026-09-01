// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Same-origin Chrome demo file (catapult `theverge_trace.json`).
//!
//! Not embedded: first miss downloads the public fixture into a cache file.
//! `ORBIT_LIVE_THEVERGE_PATH` overrides with a local path.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;

/// Viewer and `curl` path. Must stay same-origin under COEP `require-corp`.
pub const THEVERGE_HTTP_PATH: &str = "/traces/theverge_trace.json";

pub const THEVERGE_UPSTREAM: &str = "https://raw.githubusercontent.com/catapult-project/catapult/main/tracing/test_data/theverge_trace.json";

/// Published size of the catapult fixture (bytes). Used to reject a truncated
/// download; an override path may differ.
pub const THEVERGE_BYTES: u64 = 54_370_856;

pub const THEVERGE_CONTENT_TYPE: &str = "application/json";

const OVERRIDE_ENV: &str = "ORBIT_LIVE_THEVERGE_PATH";

static FILL: Mutex<()> = Mutex::new(());

pub fn cache_path() -> PathBuf {
    std::env::temp_dir()
        .join("orbit-live-traces")
        .join("theverge_trace.json")
}

fn override_path() -> Option<PathBuf> {
    std::env::var_os(OVERRIDE_ENV)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn file_usable(path: &Path, require_published_size: bool) -> bool {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() > 0 => !require_published_size || m.len() == THEVERGE_BYTES,
        _ => false,
    }
}

/// Local file to stream. Override wins; otherwise the cache, filled from
/// GitHub on the first miss.
pub fn ensure_theverge_file() -> Result<PathBuf, String> {
    if let Some(p) = override_path() {
        if file_usable(&p, false) {
            return Ok(p);
        }
        return Err(format!(
            "{OVERRIDE_ENV} is not a readable file: {}",
            p.display()
        ));
    }
    let cache = cache_path();
    if file_usable(&cache, true) {
        return Ok(cache);
    }
    // A previous complete copy may have a different size if catapult
    // republishes; still serve a large existing cache.
    if file_usable(&cache, false)
        && cache
            .metadata()
            .map(|m| m.len() > 1_000_000)
            .unwrap_or(false)
    {
        return Ok(cache);
    }
    fill_cache(&cache)?;
    Ok(cache)
}

fn fill_cache(dest: &Path) -> Result<(), String> {
    let _guard = FILL.lock();
    if file_usable(dest, true) {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cache dir: {e}"))?;
    }
    let tmp = dest.with_extension("json.part");
    let status = std::process::Command::new("curl")
        .args(["-fsSL", "--retry", "3", "--retry-delay", "2", "-o"])
        .arg(&tmp)
        .arg(THEVERGE_UPSTREAM)
        .status()
        .map_err(|e| format!("download theverge (curl): {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "download theverge failed: curl {status} ({THEVERGE_UPSTREAM})"
        ));
    }
    let len = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    if len < 1_000_000 {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("theverge download too small: {len} B"));
    }
    std::fs::rename(&tmp, dest).map_err(|e| format!("cache rename: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_path_is_same_origin_trace() {
        assert_eq!(THEVERGE_HTTP_PATH, "/traces/theverge_trace.json");
        assert!(THEVERGE_HTTP_PATH.starts_with('/'));
        assert!(!THEVERGE_HTTP_PATH.contains("://"));
    }

    #[test]
    fn override_env_wins_without_download() {
        let path = "/tmp/chrome-traces/theverge_trace.json";
        if !Path::new(path).is_file() {
            return;
        }
        let prev = std::env::var_os(OVERRIDE_ENV);
        std::env::set_var(OVERRIDE_ENV, path);
        let got = ensure_theverge_file().expect("override");
        match prev {
            Some(v) => std::env::set_var(OVERRIDE_ENV, v),
            None => std::env::remove_var(OVERRIDE_ENV),
        }
        assert_eq!(got, PathBuf::from(path));
        assert_eq!(std::fs::metadata(&got).unwrap().len(), THEVERGE_BYTES);
    }
}
