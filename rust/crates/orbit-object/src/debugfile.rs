// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Finding the detached debug file of an ELF.
//!
//! A distribution's shared libraries are stripped to their dynamic symbol
//! table, so a sampled address inside libc resolves to nothing unless the
//! `-dbg` package's file is found: by build id under
//! `/usr/lib/debug/.build-id/xx/rest.debug`, else by `.gnu_debuglink`, next
//! to the file, in its `.debug/` directory, or under `/usr/lib/debug/<dir>/`,
//! the way gdb looks. A debuglink candidate must match the link's CRC; a
//! build-id path is its own proof.

use std::path::{Path, PathBuf};

use crate::debuglink::crc32_gnu_debuglink;
use crate::ElfMetadata;

/// Where distributions put detached debug files.
pub const DEFAULT_DEBUG_ROOT: &str = "/usr/lib/debug";

/// The detached debug file for `path`, whose metadata was already parsed.
pub fn detached_debug_file(path: &Path, metadata: &ElfMetadata) -> Option<PathBuf> {
    detached_debug_file_under(path, metadata, Path::new(DEFAULT_DEBUG_ROOT))
}

/// [`detached_debug_file`] with the debug root made explicit, for tests.
pub fn detached_debug_file_under(path: &Path, metadata: &ElfMetadata, root: &Path) -> Option<PathBuf> {
    if metadata.build_id.len() > 2 {
        let candidate = root
            .join(".build-id")
            .join(&metadata.build_id[..2])
            .join(format!("{}.debug", &metadata.build_id[2..]));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let link = metadata.gnu_debuglink.as_ref()?;
    let name = Path::new(&link.path);
    let dir = path.parent().unwrap_or(Path::new("."));
    let under_root = root.join(dir.strip_prefix("/").unwrap_or(dir)).join(name);
    for candidate in [dir.join(name), dir.join(".debug").join(name), under_root] {
        if let Ok(bytes) = std::fs::read(&candidate) {
            if crc32_gnu_debuglink(&bytes) == link.crc32_checksum {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_elf_metadata;

    fn testdata_dir() -> PathBuf {
        PathBuf::from(std::env::var("ORBIT_TESTDATA").unwrap_or_else(|_| {
            format!("{}/../../../src/ObjectUtils/testdata", env!("CARGO_MANIFEST_DIR"))
        }))
    }

    #[test]
    fn a_debuglink_next_to_the_file_is_found_when_its_crc_matches() {
        let path = testdata_dir().join("hello_world_elf_with_gnu_debuglink");
        let bytes = std::fs::read(&path).unwrap();
        let metadata = parse_elf_metadata(&bytes, path.to_str().unwrap()).unwrap();
        let empty_root = std::env::temp_dir().join("orbit-no-debug-root");
        let found = detached_debug_file_under(&path, &metadata, &empty_root).expect("debug file next to it");
        assert_eq!(found.file_name().unwrap(), "hello_world_elf.debug");
    }

    #[test]
    fn a_build_id_path_under_the_root_wins() {
        let path = testdata_dir().join("hello_world_elf");
        let bytes = std::fs::read(&path).unwrap();
        let metadata = parse_elf_metadata(&bytes, path.to_str().unwrap()).unwrap();
        assert!(metadata.gnu_debuglink.is_none(), "this file has no debuglink; only the build id can find it");
        let root = std::env::temp_dir().join(format!("orbit-debug-root-{}", std::process::id()));
        let dir = root.join(".build-id").join(&metadata.build_id[..2]);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{}.debug", &metadata.build_id[2..]));
        std::fs::write(&file, b"any bytes: the path is the proof").unwrap();
        assert_eq!(detached_debug_file_under(&path, &metadata, &root), Some(file));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn nothing_is_found_without_a_link_or_a_build_id_file() {
        let path = testdata_dir().join("hello_world_elf_no_build_id");
        let bytes = std::fs::read(&path).unwrap();
        let metadata = parse_elf_metadata(&bytes, path.to_str().unwrap()).unwrap();
        let empty_root = std::env::temp_dir().join("orbit-no-debug-root");
        assert_eq!(detached_debug_file_under(&path, &metadata, &empty_root), None);
    }
}
