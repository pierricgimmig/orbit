// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Parsing of `/proc/[pid]/maps`.
//!
//! A byte-for-byte port of `orbit_module_utils::ParseMaps` from
//! `src/ModuleUtils/ReadLinuxMaps.cpp`. The C++ remains in the tree and is
//! still the default; `ORBIT_MAPS_BACKEND` selects between them, and `both`
//! runs the two and compares. See `docs/rust-port-plan.html`.
//!
//! Everything here operates on bytes rather than `str`. Paths in
//! `/proc/[pid]/maps` are kernel-supplied and are not guaranteed to be UTF-8,
//! and the C++ this replaces used `std::string_view`, so decoding would both
//! change behaviour and add a failure mode the original does not have.

#![deny(unsafe_code)]

/// `PROT_READ` from `<sys/mman.h>`. The C++ shim `static_assert`s these
/// against the real values rather than trusting this comment.
pub const PROT_READ: u64 = 1;
/// `PROT_WRITE` from `<sys/mman.h>`.
pub const PROT_WRITE: u64 = 2;
/// `PROT_EXEC` from `<sys/mman.h>`.
pub const PROT_EXEC: u64 = 4;

/// One entry of `/proc/[pid]/maps`.
///
/// Mirrors `orbit_module_utils::LinuxMemoryMapping`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryMapping {
    pub start_address: u64,
    pub end_address: u64,
    pub perms: u64,
    pub offset: u64,
    pub inode: u64,
    /// Raw bytes as the kernel reported them; not necessarily UTF-8.
    pub pathname: Vec<u8>,
}

/// Splits `input` on `sep` at most `max_splits` times, so the result holds at
/// most `max_splits + 1` fields and the last one keeps every remaining
/// separator. Equivalent to `absl::StrSplit(s, absl::MaxSplits(sep, n))`.
fn split_max(input: &[u8], sep: u8, max_splits: usize) -> Vec<&[u8]> {
    // saturating_add, and capped: callers pass small bounds, but a reservation
    // derived from an argument should not be able to overflow or to ask for a
    // gigabyte because someone passed usize::MAX.
    let mut fields = Vec::with_capacity(max_splits.saturating_add(1).min(16));
    let mut rest = input;
    for _ in 0..max_splits {
        match rest.iter().position(|&b| b == sep) {
            Some(i) => {
                fields.push(&rest[..i]);
                rest = &rest[i + 1..];
            }
            None => break,
        }
    }
    fields.push(rest);
    fields
}

/// `absl::StripLeadingAsciiWhitespace`.
fn strip_leading_ascii_whitespace(bytes: &[u8]) -> &[u8] {
    let first = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    &bytes[first..]
}

/// Parses a hexadecimal integer, rejecting an empty string or any trailing
/// byte that is not a hex digit.
///
/// The C++ uses `std::stoull(s, nullptr, 16)`, which *throws* on a string
/// with no leading digits and silently ignores trailing garbage. Returning
/// `None` and skipping the line is a deliberate, documented divergence: no
/// existing test covers malformed hex, and a parser that reads
/// kernel-supplied text should not be able to terminate the process. See
/// docs/blog/.
fn parse_hex_u64(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    let mut value: u64 = 0;
    for &b in bytes {
        let digit = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return None,
        };
        value = value.checked_mul(16)?.checked_add(u64::from(digit))?;
    }
    Some(value)
}

/// `absl::SimpleAtoi<uint64_t>`: a full-string decimal parse that accepts an
/// optional leading `+` and rejects any trailing byte.
fn parse_dec_u64(bytes: &[u8]) -> Option<u64> {
    let digits = match bytes.first() {
        Some(b'+') => &bytes[1..],
        _ => bytes,
    };
    if digits.is_empty() {
        return None;
    }
    let mut value: u64 = 0;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u64::from(b - b'0'))?;
    }
    Some(value)
}

/// Parses the contents of `/proc/[pid]/maps`.
///
/// Lines that do not match the expected shape are skipped, exactly as the C++
/// does. Never fails: a completely malformed input yields an empty vector.
pub fn parse_maps(content: &[u8]) -> Vec<MemoryMapping> {
    let mut result = Vec::new();

    for line in content.split(|&b| b == b'\n') {
        // The number of spaces between the inode and the path is variable and
        // the path itself can contain spaces, so cap the split and strip the
        // path's leading whitespace separately.
        let tokens = split_max(line, b' ', 5);
        if tokens.len() < 5 {
            continue;
        }
        debug_assert!(tokens.len() == 5 || tokens.len() == 6);

        // absl::StrSplit(tokens[0], '-') splits on every dash, so "1-2-3"
        // yields three fields and is rejected below.
        let mut start_and_end = tokens[0].split(|&b| b == b'-');
        let (Some(start_field), Some(end_field), None) = (
            start_and_end.next(),
            start_and_end.next(),
            start_and_end.next(),
        ) else {
            continue;
        };
        let Some(start_address) = parse_hex_u64(start_field) else {
            continue;
        };
        let Some(end_address) = parse_hex_u64(end_field) else {
            continue;
        };
        let Some(offset) = parse_hex_u64(tokens[2]) else {
            continue;
        };

        if tokens[1].len() < 4 {
            continue;
        }
        let mut perms = 0;
        if tokens[1][0] == b'r' {
            perms |= PROT_READ;
        }
        if tokens[1][1] == b'w' {
            perms |= PROT_WRITE;
        }
        if tokens[1][2] == b'x' {
            perms |= PROT_EXEC;
        }

        let Some(inode) = parse_dec_u64(tokens[4]) else {
            continue;
        };

        let pathname = match tokens.get(5) {
            Some(rest) => strip_leading_ascii_whitespace(rest).to_vec(),
            None => Vec::new(),
        };

        result.push(MemoryMapping {
            start_address,
            end_address,
            perms,
            offset,
            inode,
            pathname,
        });
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(m: &MemoryMapping) -> &str {
        std::str::from_utf8(&m.pathname).unwrap()
    }

    /// Mirrors `TEST(ReadLinuxMaps, ParseMaps)` in
    /// `src/ModuleUtils/ReadLinuxMapsTest.cpp`, assertion for assertion.
    #[test]
    fn parse_maps_matches_cpp_test() {
        const CONTENT: &[u8] = b"\
00400000-00452000 r-xp 00000000 08:02 173521      /usr/bin/dbus-daemon
00e03000-00e24000 rw-p 00000000 00:00 0           [heap]
35b1800000-35b1820000 r-xp 00000000 08:02 135522  /path with spaces
35b1a21000-35b1a22000 rw-p 00000000 00:00 0       
";
        let maps = parse_maps(CONTENT);
        assert_eq!(maps.len(), 4);

        assert_eq!(maps[0].start_address, 0x400000);
        assert_eq!(maps[0].end_address, 0x452000);
        assert_eq!(maps[0].perms, PROT_READ | PROT_EXEC);
        assert_eq!(maps[0].inode, 173521);
        assert_eq!(path(&maps[0]), "/usr/bin/dbus-daemon");

        assert_eq!(maps[1].start_address, 0xe03000);
        assert_eq!(maps[1].end_address, 0xe24000);
        assert_eq!(maps[1].perms, PROT_READ | PROT_WRITE);
        assert_eq!(maps[1].inode, 0);
        assert_eq!(path(&maps[1]), "[heap]");

        assert_eq!(maps[2].start_address, 0x35b1800000);
        assert_eq!(maps[2].end_address, 0x35b1820000);
        assert_eq!(maps[2].perms, PROT_READ | PROT_EXEC);
        assert_eq!(maps[2].inode, 135522);
        assert_eq!(path(&maps[2]), "/path with spaces");

        assert_eq!(maps[3].start_address, 0x35b1a21000);
        assert_eq!(maps[3].end_address, 0x35b1a22000);
        assert_eq!(maps[3].perms, PROT_READ | PROT_WRITE);
        assert_eq!(maps[3].inode, 0);
        assert_eq!(path(&maps[3]), "");
    }

    /// Mirrors `TEST(ReadLinuxMaps, ParseMapsFromInvalidProcPidMapsContent)`.
    #[test]
    fn parse_maps_from_invalid_content() {
        assert_eq!(parse_maps(b"").len(), 0);
        assert_eq!(parse_maps(b"\n\n").len(), 0);
        // Missing inode.
        assert_eq!(parse_maps(b"00400000-00452000 r-xp 00000000 08:02").len(), 0);
        // Unexpected protection format.
        assert_eq!(
            parse_maps(b"00400000-00452000 r-x 00000000 08:02 173521      /usr/bin/dbus-daemon")
                .len(),
            0
        );
        // Non-numeric inode.
        assert_eq!(
            parse_maps(b"00400000-00452000 r-xp 00000000 08:02 173521a      /usr/bin/dbus-daemon\n")
                .len(),
            0
        );
    }

    /// Not covered by the C++ suite: `std::stoull` would throw here.
    #[test]
    fn malformed_hex_is_skipped_not_fatal() {
        assert_eq!(parse_maps(b"zzzz-00452000 r-xp 0 08:02 0 /x").len(), 0);
        assert_eq!(parse_maps(b"00400000-zzzz r-xp 0 08:02 0 /x").len(), 0);
        assert_eq!(parse_maps(b"00400000-00452000 r-xp zz 08:02 0 /x").len(), 0);
        assert_eq!(parse_maps(b"-00452000 r-xp 0 08:02 0 /x").len(), 0);
    }

    /// Not covered by the C++ suite either: the kernel does not promise UTF-8.
    #[test]
    fn non_utf8_pathname_survives() {
        let mut content = b"00400000-00452000 r-xp 00000000 08:02 1 /tmp/".to_vec();
        content.extend_from_slice(&[0xff, 0xfe]);
        let maps = parse_maps(&content);
        assert_eq!(maps.len(), 1);
        assert_eq!(maps[0].pathname, b"/tmp/\xff\xfe");
    }

    #[test]
    fn address_range_must_have_exactly_two_parts() {
        assert_eq!(parse_maps(b"00400000 r-xp 0 08:02 0 /x").len(), 0);
        assert_eq!(parse_maps(b"1-2-3 r-xp 0 08:02 0 /x").len(), 0);
    }

    #[test]
    fn overflowing_values_are_skipped_not_wrapped() {
        // u64::MAX is 16 hex digits; 17 significant ones cannot fit.
        assert_eq!(
            parse_maps(b"1ffffffffffffffff-1 r-xp 0 08:02 0 /x").len(),
            0
        );
        // Leading zeros are not significant, so this one is fine.
        assert_eq!(parse_maps(b"00000000000000001-2 r-xp 0 08:02 0 /x").len(), 1);
        assert_eq!(
            parse_maps(b"1-2 r-xp 0 08:02 99999999999999999999999 /x").len(),
            0
        );
    }

    #[test]
    fn split_max_keeps_separators_in_the_tail() {
        assert_eq!(split_max(b"a b c d", b' ', 2), vec![&b"a"[..], b"b", b"c d"]);
        assert_eq!(split_max(b"a", b' ', 5), vec![&b"a"[..]]);
        assert_eq!(split_max(b"", b' ', 5), vec![&b""[..]]);
        assert_eq!(split_max(b"a  b", b' ', 5), vec![&b"a"[..], b"", b"b"]);
    }
}
