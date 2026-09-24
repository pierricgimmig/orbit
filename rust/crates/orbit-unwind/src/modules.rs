// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Turning a profiled process's memory map into framehop modules.

use framehop::{Module, ModuleSectionInfo};
use object::ObjectSegment as _;
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

/// Loads one mapped file as a framehop module. `lowest_bias` is the lowest
/// `start - offset` over the file's mappings, `avma_range` the span of its
/// mappings. Returns None for files that don't parse as objects (deleted
/// files, special mappings).
///
/// framehop's `base_avma` is where the file's virtual address 0 landed, and
/// `start - offset` only equals that when the first PT_LOAD puts file offset
/// 0 at virtual address 0 -- which the usual linker layout does. Unreal's
/// binaries (and anything linked with an image base) put it at 0x200000,
/// so every `.eh_frame` lookup was off by that much and the unwinder walked
/// no frame of the game itself; the difference is read from the program
/// headers instead.
pub fn load_module(path: &str, avma_range: Range<u64>, lowest_bias: u64) -> Option<Module<Vec<u8>>> {
    let contents = std::fs::read(path).ok()?;
    let file = object::File::parse(&contents[..]).ok()?;
    let segments: Vec<(u64, u64)> =
        file.segments().map(|segment| (segment.file_range().0, segment.address())).collect();
    let base_avma = lowest_bias.wrapping_sub(first_segment_vaddr_offset(&segments));
    Some(Module::new(
        path.to_string(),
        avma_range,
        base_avma,
        ObjectSectionInfo { file: &file },
    ))
}

/// `vaddr - file offset` of the PT_LOAD with the lowest file offset: what
/// separates "start - offset" of the first mapping from the address of the
/// file's virtual address 0. Zero for the usual layout.
pub fn first_segment_vaddr_offset(segments: &[(u64, u64)]) -> u64 {
    segments
        .iter()
        .min_by_key(|(offset, _)| *offset)
        .map(|(offset, vaddr)| vaddr.wrapping_sub(*offset))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::first_segment_vaddr_offset;

    #[test]
    fn image_base_is_read_from_the_first_load_segment() {
        // The usual layout: file offset 0 at virtual address 0.
        assert_eq!(first_segment_vaddr_offset(&[(0, 0), (0x1000, 0x1000), (0x5000, 0x6000)]), 0);
        // Unreal's Shipping binary: first PT_LOAD at 0x200000, text at
        // offset 0x3081000 / vaddr 0x3282000. The correction is the first
        // segment's, not the text segment's (alignment padding differs).
        assert_eq!(
            first_segment_vaddr_offset(&[(0, 0x200000), (0x3081000, 0x3282000), (0xb029ec0, 0xb22bec0)]),
            0x200000
        );
        assert_eq!(first_segment_vaddr_offset(&[]), 0);
    }
}
