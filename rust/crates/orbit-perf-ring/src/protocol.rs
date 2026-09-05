// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The pure arithmetic of reading from a power-of-two ring, kept free of
//! unsafe so the wrap handling -- the part of `PerfEventRingBuffer.cpp` that
//! actually has edge cases -- is unit-testable without a kernel.

/// Where in the ring `count` bytes starting at absolute position `index`
/// live: a first segment, and a second one when the read wraps. Mirrors the
/// two-copy split in `PerfEventRingBuffer::ReadAtOffsetFromTail`; like the
/// C++, a `count` larger than the ring is a caller bug.
#[derive(Debug, PartialEq, Eq)]
pub struct Split {
    pub first_start: usize,
    pub first_len: usize,
    pub second_len: usize,
}

pub fn split_for_read(index: u64, count: usize, ring_size: u64) -> Split {
    debug_assert!(ring_size.is_power_of_two());
    debug_assert!(count as u64 <= ring_size);
    let start = (index & (ring_size - 1)) as usize;
    let to_end = (ring_size as usize) - start;
    if count <= to_end {
        Split { first_start: start, first_len: count, second_len: 0 }
    } else {
        Split { first_start: start, first_len: to_end, second_len: count - to_end }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwrapped_read_is_one_segment() {
        assert_eq!(
            split_for_read(24, 16, 4096),
            Split { first_start: 24, first_len: 16, second_len: 0 }
        );
    }

    #[test]
    fn read_ending_at_the_boundary_does_not_wrap() {
        assert_eq!(
            split_for_read(4096 - 16, 16, 4096),
            Split { first_start: 4080, first_len: 16, second_len: 0 }
        );
    }

    #[test]
    fn read_crossing_the_boundary_wraps() {
        assert_eq!(
            split_for_read(4096 - 8, 24, 4096),
            Split { first_start: 4088, first_len: 8, second_len: 16 }
        );
    }

    #[test]
    fn index_far_beyond_the_ring_still_masks() {
        assert_eq!(
            split_for_read(3 * 4096 + 100, 8, 4096),
            Split { first_start: 100, first_len: 8, second_len: 0 }
        );
    }
}
