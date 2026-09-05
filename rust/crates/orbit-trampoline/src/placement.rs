// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Address-range bookkeeping for trampoline placement.

use orbit_maps::parse_maps;

/// A half-open address range [start, end). Mirrors the C++ `AddressRange`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PlacementError {
    /// `code_range` was not found inside `unavailable_ranges`.
    CodeRangeNotUnavailable,
    /// No free slot of the requested size within +/-2GB of `code_range`.
    NoRoomNearCodeRange,
    /// The 64-bit address difference does not fit in a signed 32-bit offset.
    DifferenceTooLarge,
}

/// True if the ranges overlap; touching ranges (a.end == b.start) do not.
pub fn ranges_overlap(a: AddressRange, b: AddressRange) -> bool {
    !(b.end <= a.start || b.start >= a.end)
}

/// Index of the lowest range in `sorted` overlapping `range`.
pub fn lowest_intersecting(sorted: &[AddressRange], range: AddressRange) -> Option<usize> {
    sorted.iter().position(|candidate| ranges_overlap(*candidate, range))
}

/// Index of the highest range in `sorted` overlapping `range`.
pub fn highest_intersecting(sorted: &[AddressRange], range: AddressRange) -> Option<usize> {
    sorted.iter().rposition(|candidate| ranges_overlap(*candidate, range))
}

/// The taken address ranges of `pid`: a `[0, mmap_min_addr)` guard followed
/// by the mappings from /proc/<pid>/maps, directly-neighboring ones joined.
/// Twin of `GetUnavailableAddressRanges`. `mmap_min_addr` is read from
/// /proc/sys/vm/mmap_min_addr.
pub fn get_unavailable_address_ranges(pid: i32) -> std::io::Result<Vec<AddressRange>> {
    let mmap_min_addr: u64 = std::fs::read_to_string("/proc/sys/vm/mmap_min_addr")?
        .trim()
        .parse()
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "bad mmap_min_addr")
        })?;
    let maps = std::fs::read(format!("/proc/{pid}/maps"))?;
    Ok(unavailable_ranges_from_parts(mmap_min_addr, &maps))
}

/// The pure core, factored out so the differential can feed identical bytes
/// to both languages.
pub fn unavailable_ranges_from_parts(mmap_min_addr: u64, maps: &[u8]) -> Vec<AddressRange> {
    let mut result = vec![AddressRange { start: 0, end: mmap_min_addr }];
    for mapping in parse_maps(maps) {
        let (begin, end) = (mapping.start_address, mapping.end_address);
        // parse_maps yields well-formed ranges; join or append.
        let last = result.last_mut().expect("seeded with the guard range");
        if last.end == begin {
            last.end = end;
        } else {
            result.push(AddressRange { start: begin, end });
        }
    }
    result
}

const MAX_32BIT_OFFSET: u64 = i32::MAX as u64;

/// Finds a free range of `size` bytes within a 32-bit relative jump of
/// `code_range`, preferring just below it, then just above. Twin of
/// `FindAddressRangeForTrampoline`. `unavailable_ranges` must be the sorted,
/// zero-based output of `get_unavailable_address_ranges`.
pub fn find_address_range_for_trampoline(
    unavailable_ranges: &[AddressRange],
    code_range: AddressRange,
    size: u64,
    page_size: u64,
) -> Result<AddressRange, PlacementError> {
    assert!(
        !unavailable_ranges.is_empty() && unavailable_ranges[0].start == 0,
        "unavailable_ranges must start at zero"
    );

    // Below code_range.
    let mut index =
        lowest_intersecting(unavailable_ranges, code_range).ok_or(PlacementError::CodeRangeNotUnavailable)?;
    while index > 0 {
        if unavailable_ranges[index].start < size {
            break;
        }
        let mut address = unavailable_ranges[index].start - size;
        address = (address / page_size) * page_size; // round down to a page
        let candidate = AddressRange { start: address, end: address + size };
        match lowest_intersecting(unavailable_ranges, candidate) {
            None => {
                if code_range.end - candidate.start <= MAX_32BIT_OFFSET {
                    return Ok(candidate);
                }
                break;
            }
            Some(next) => index = next,
        }
    }

    // Above code_range.
    let mut index =
        highest_intersecting(unavailable_ranges, code_range).ok_or(PlacementError::CodeRangeNotUnavailable)?;
    loop {
        let end = unavailable_ranges[index].end;
        if end > u64::MAX - (page_size - 1) {
            break;
        }
        let address = ((end + (page_size - 1)) / page_size) * page_size; // round up to a page
        if address >= u64::MAX - size {
            break;
        }
        let candidate = AddressRange { start: address, end: address + size };
        match highest_intersecting(unavailable_ranges, candidate) {
            None => {
                if candidate.end - code_range.start <= MAX_32BIT_OFFSET {
                    return Ok(candidate);
                }
                break;
            }
            Some(next) => index = next,
        }
    }

    Err(PlacementError::NoRoomNearCodeRange)
}

/// The signed 32-bit difference a - b, or an error when it does not fit.
/// Twin of `AddressDifferenceAsInt32`.
pub fn address_difference_as_i32(a: u64, b: u64) -> Result<i32, PlacementError> {
    const MAX: u64 = i32::MAX as u64;
    // -(i32::MIN) as u64.
    const MIN_MAGNITUDE: u64 = 0x8000_0000;
    if a > b && a - b > MAX {
        return Err(PlacementError::DifferenceTooLarge);
    }
    if b > a && b - a > MIN_MAGNITUDE {
        return Err(PlacementError::DifferenceTooLarge);
    }
    Ok(a.wrapping_sub(b) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: u64, end: u64) -> AddressRange {
        AddressRange { start, end }
    }

    #[test]
    fn overlap_excludes_touching() {
        assert!(ranges_overlap(r(0, 10), r(5, 15)));
        assert!(!ranges_overlap(r(0, 10), r(10, 20)));
        assert!(!ranges_overlap(r(10, 20), r(0, 10)));
    }

    #[test]
    fn intersecting_indices() {
        let sorted = [r(0, 10), r(20, 30), r(40, 50)];
        assert_eq!(lowest_intersecting(&sorted, r(25, 45)), Some(1));
        assert_eq!(highest_intersecting(&sorted, r(25, 45)), Some(2));
        assert_eq!(lowest_intersecting(&sorted, r(11, 19)), None);
    }

    #[test]
    fn joins_neighboring_mappings() {
        let maps = b"00001000-00002000 r-xp 00000000 00:00 0 /a\n\
                     00002000-00003000 r--p 00000000 00:00 0 /b\n\
                     00005000-00006000 rw-p 00000000 00:00 0 /c\n";
        let ranges = unavailable_ranges_from_parts(0x1000, maps);
        // The guard [0,0x1000) joins the first mapping (0x1000 == 0x1000),
        // which joins the second, giving [0,0x3000); then [0x5000,0x6000).
        assert_eq!(ranges, vec![r(0, 0x3000), r(0x5000, 0x6000)]);
    }

    #[test]
    fn places_trampoline_just_below_code() {
        // Guard, then a gap, then the code mapping. size fits below it.
        let unavailable = vec![r(0, 0x1000), r(0x100000, 0x101000)];
        let code = r(0x100000, 0x101000);
        let got = find_address_range_for_trampoline(&unavailable, code, 0x1000, 0x1000).unwrap();
        assert_eq!(got, r(0xff000, 0x100000));
        assert!(!ranges_overlap(got, code));
    }

    #[test]
    fn falls_back_to_above_when_below_is_blocked() {
        // The code mapping starts right after the guard, so there is no room
        // below; placement must go above.
        let unavailable = vec![r(0, 0x1000), r(0x1000, 0x2000)];
        let code = r(0x1000, 0x2000);
        let got = find_address_range_for_trampoline(&unavailable, code, 0x1000, 0x1000).unwrap();
        assert_eq!(got, r(0x2000, 0x3000));
    }

    #[test]
    fn errors_when_code_range_is_not_taken() {
        let unavailable = vec![r(0, 0x1000)];
        assert_eq!(
            find_address_range_for_trampoline(&unavailable, r(0x8000, 0x9000), 0x1000, 0x1000),
            Err(PlacementError::CodeRangeNotUnavailable)
        );
    }

    #[test]
    fn difference_fits_or_errors() {
        assert_eq!(address_difference_as_i32(0x100, 0x080), Ok(0x80));
        assert_eq!(address_difference_as_i32(0x080, 0x100), Ok(-0x80));
        assert_eq!(address_difference_as_i32(0, 0x8000_0000), Ok(i32::MIN));
        assert_eq!(
            address_difference_as_i32(0x1_0000_0000, 0),
            Err(PlacementError::DifferenceTooLarge)
        );
        assert_eq!(
            address_difference_as_i32(0, 0x8000_0001),
            Err(PlacementError::DifferenceTooLarge)
        );
    }
}
