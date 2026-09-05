// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Turning a profiled process's memory map into framehop modules.

use framehop::{Module, ModuleSectionInfo};
use object::{Object, ObjectSection};
use std::ops::Range;

/// Feeds framehop the sections it asks for (`.eh_frame`, `.eh_frame_hdr`,
/// `.debug_frame`, `.text`, `.got`) out of an ELF file. Section data is
/// copied out (`uncompressed_data`), which also transparently handles
/// SHF_COMPRESSED debug sections.
struct ObjectSectionInfo<'data, 'file> {
    file: &'file object::File<'data>,
}

impl ModuleSectionInfo<Vec<u8>> for ObjectSectionInfo<'_, '_> {
    fn base_svma(&self) -> u64 {
        // For ELF objects the stated base is zero (framehop's convention);
        // relocation is expressed entirely through base_avma.
        0
    }

    fn section_svma_range(&mut self, name: &[u8]) -> Option<Range<u64>> {
        let section = self.file.section_by_name_bytes(name)?;
        Some(section.address()..section.address() + section.size())
    }

    fn section_data(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let section = self.file.section_by_name_bytes(name)?;
        Some(section.uncompressed_data().ok()?.into_owned())
    }
}

/// Loads one mapped file as a framehop module. `base_avma` is the load bias
/// (lowest `start - offset` over the file's mappings), `avma_range` the span
/// of its mappings. Returns None for files that don't parse as objects
/// (deleted files, special mappings).
pub fn load_module(path: &str, avma_range: Range<u64>, base_avma: u64) -> Option<Module<Vec<u8>>> {
    let contents = std::fs::read(path).ok()?;
    let file = object::File::parse(&contents[..]).ok()?;
    Some(Module::new(
        path.to_string(),
        avma_range,
        base_avma,
        ObjectSectionInfo { file: &file },
    ))
}
