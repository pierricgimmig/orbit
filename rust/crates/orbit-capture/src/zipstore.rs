// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A small zip writer and reader: store and deflate.
//!
//! A self-contained capture is several Arrow tables and a manifest, and it
//! has to travel as one file: a browser download, an attachment, a thing you
//! drop back onto the viewer. Zip is the container everyone can open.
//! Writing it by hand is a few dozen lines; deflate comes from
//! `miniz_oxide`, pure Rust, already in this crate's tree through parquet,
//! so no C reaches the static service binary. Deflate matters here: an
//! events table is mostly zero bytes (64-bit timestamps and durations that
//! rarely need more than four), and on a real capture it packs about five
//! to one, so a bundle that would be 127 MB stored is 25 MB.
//!
//! Reads accept methods 0 and 8, no data descriptors, sizes under 4 GiB
//! (no zip64). Anything else is an error, not a guess.

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;
/// 1980-01-01 in DOS date form; every entry carries it, so a bundle's bytes
/// depend only on its contents.
const DOS_DATE: u16 = 0x0021;
const VERSION: u16 = 20;

/// IEEE CRC-32, table driven, as every zip reader checks it.
pub fn crc32(bytes: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        t
    });
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// What went wrong reading a zip.
#[derive(Debug, PartialEq, Eq)]
pub enum ZipError {
    /// No end-of-central-directory record: not a zip, or truncated.
    NotAZip,
    Truncated,
    /// An entry uses compression or a feature this reader does not do.
    Unsupported(&'static str),
    /// An entry's bytes do not match its CRC.
    Corrupt(String),
    /// An entry or the archive is 4 GiB or more.
    TooLarge,
}

impl std::fmt::Display for ZipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZipError::NotAZip => write!(f, "not a zip file"),
            ZipError::Truncated => write!(f, "zip file is truncated"),
            ZipError::Unsupported(what) => write!(f, "unsupported zip feature: {what}"),
            ZipError::Corrupt(name) => write!(f, "zip entry {name:?} fails its CRC"),
            ZipError::TooLarge => write!(f, "zip entry too large (no zip64 support)"),
        }
    }
}

impl std::error::Error for ZipError {}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Deflate level for a bundle's tables. Level 6 is a third smaller than
/// level 1 on Arrow event tables for about four times the time; a 127 MB
/// capture takes a couple of seconds, once, at Save.
pub const BUNDLE_DEFLATE_LEVEL: u8 = 6;

/// Writes `entries` (name, bytes) as a store-only zip. Names are taken as
/// given; use forward slashes for directories.
pub fn write_store_zip(entries: &[(&str, &[u8])]) -> Result<Vec<u8>, ZipError> {
    write_zip(entries, None)
}

/// Writes `entries` deflated at `level` (1 fastest, 9 smallest); `None`
/// stores them. An entry that deflate cannot shrink is stored as is, which
/// is what every zip tool does.
pub fn write_zip(entries: &[(&str, &[u8])], level: Option<u8>) -> Result<Vec<u8>, ZipError> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        if data.len() >= u32::MAX as usize || name.len() > u16::MAX as usize {
            return Err(ZipError::TooLarge);
        }
        let offset = out.len();
        if offset >= u32::MAX as usize {
            return Err(ZipError::TooLarge);
        }
        let crc = crc32(data);
        let size = data.len() as u32;
        let name_bytes = name.as_bytes();
        let deflated = level.map(|l| miniz_oxide::deflate::compress_to_vec(data, l.clamp(1, 10)));
        let (method, body): (u16, &[u8]) = match &deflated {
            Some(d) if d.len() < data.len() => (8, d.as_slice()),
            _ => (0, data),
        };
        let csize = body.len() as u32;

        // Local file header.
        out.extend_from_slice(&LOCAL_SIG.to_le_bytes());
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&DOS_DATE.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&csize.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(body);

        // Central directory entry.
        central.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
        central.extend_from_slice(&VERSION.to_le_bytes()); // made by
        central.extend_from_slice(&VERSION.to_le_bytes()); // needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // time
        central.extend_from_slice(&DOS_DATE.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&csize.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&(offset as u32).to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let cd_offset = out.len();
    if cd_offset >= u32::MAX as usize || central.len() >= u32::MAX as usize {
        return Err(ZipError::TooLarge);
    }
    out.extend_from_slice(&central);
    // End of central directory.
    out.extend_from_slice(&EOCD_SIG.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&(cd_offset as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment
    Ok(out)
}

/// The byte range of every entry of a stored zip, in central directory
/// order, so a reader can work on the file's bytes in place. A deflated
/// entry is `Unsupported`: it has no in-place bytes to give. Nothing is
/// checked against its CRC here; that is the reader's call when it
/// decodes the entry.
pub fn stored_entry_ranges(bytes: &[u8]) -> Result<Vec<(String, std::ops::Range<usize>)>, ZipError> {
    if bytes.len() < 22 {
        return Err(ZipError::NotAZip);
    }
    let floor = bytes.len().saturating_sub(22 + u16::MAX as usize);
    let eocd = (floor..=bytes.len() - 22)
        .rev()
        .find(|&i| u32_at(bytes, i) == EOCD_SIG)
        .ok_or(ZipError::NotAZip)?;
    let entries = u16_at(bytes, eocd + 10) as usize;
    let cd_offset = u32_at(bytes, eocd + 16) as usize;
    let mut out = Vec::with_capacity(entries);
    let mut pos = cd_offset;
    for _ in 0..entries {
        if pos + 46 > bytes.len() || u32_at(bytes, pos) != CENTRAL_SIG {
            return Err(ZipError::Truncated);
        }
        let method = u16_at(bytes, pos + 10);
        let csize = u32_at(bytes, pos + 20) as usize;
        let name_len = u16_at(bytes, pos + 28) as usize;
        let extra_len = u16_at(bytes, pos + 30) as usize;
        let comment_len = u16_at(bytes, pos + 32) as usize;
        let local = u32_at(bytes, pos + 42) as usize;
        if pos + 46 + name_len > bytes.len() {
            return Err(ZipError::Truncated);
        }
        let name = String::from_utf8_lossy(&bytes[pos + 46..pos + 46 + name_len]).into_owned();
        pos += 46 + name_len + extra_len + comment_len;
        if method != 0 {
            return Err(ZipError::Unsupported("compressed entry cannot be read in place"));
        }
        if local + 30 > bytes.len() || u32_at(bytes, local) != LOCAL_SIG {
            return Err(ZipError::Truncated);
        }
        let l_name = u16_at(bytes, local + 26) as usize;
        let l_extra = u16_at(bytes, local + 28) as usize;
        let start = local + 30 + l_name + l_extra;
        let end = start.checked_add(csize).ok_or(ZipError::Truncated)?;
        if end > bytes.len() {
            return Err(ZipError::Truncated);
        }
        out.push((name, start..end));
    }
    Ok(out)
}

/// Reads a zip of stored and deflated entries back into (name, bytes)
/// pairs, in central directory order. Every entry's CRC is checked.
pub fn read_store_zip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, ZipError> {
    // The end record is the last 22 bytes when there is no archive comment,
    // and we write none; but tolerate a comment by scanning back for the
    // signature within the largest comment a zip allows.
    if bytes.len() < 22 {
        return Err(ZipError::NotAZip);
    }
    let floor = bytes.len().saturating_sub(22 + u16::MAX as usize);
    let eocd = (floor..=bytes.len() - 22)
        .rev()
        .find(|&i| u32_at(bytes, i) == EOCD_SIG)
        .ok_or(ZipError::NotAZip)?;
    let entries = u16_at(bytes, eocd + 10) as usize;
    let cd_size = u32_at(bytes, eocd + 12) as usize;
    let cd_offset = u32_at(bytes, eocd + 16) as usize;
    if cd_offset.checked_add(cd_size).is_none_or(|end| end > bytes.len()) {
        return Err(ZipError::Truncated);
    }

    let mut out = Vec::with_capacity(entries);
    let mut pos = cd_offset;
    for _ in 0..entries {
        if pos + 46 > bytes.len() || u32_at(bytes, pos) != CENTRAL_SIG {
            return Err(ZipError::Truncated);
        }
        let flags = u16_at(bytes, pos + 8);
        let method = u16_at(bytes, pos + 10);
        let crc = u32_at(bytes, pos + 16);
        let csize = u32_at(bytes, pos + 20) as usize;
        let usize_ = u32_at(bytes, pos + 24) as usize;
        let name_len = u16_at(bytes, pos + 28) as usize;
        let extra_len = u16_at(bytes, pos + 30) as usize;
        let comment_len = u16_at(bytes, pos + 32) as usize;
        let local = u32_at(bytes, pos + 42) as usize;
        if pos + 46 + name_len > bytes.len() {
            return Err(ZipError::Truncated);
        }
        let name = String::from_utf8_lossy(&bytes[pos + 46..pos + 46 + name_len]).into_owned();
        pos += 46 + name_len + extra_len + comment_len;

        if method != 0 && method != 8 {
            return Err(ZipError::Unsupported("compression method other than store or deflate"));
        }
        if flags & 0x0008 != 0 {
            return Err(ZipError::Unsupported("data descriptor"));
        }
        if csize == u32::MAX as usize || usize_ == u32::MAX as usize {
            return Err(ZipError::Unsupported("zip64 sizes"));
        }
        // The local header has its own name/extra lengths; the data follows.
        if local + 30 > bytes.len() || u32_at(bytes, local) != LOCAL_SIG {
            return Err(ZipError::Truncated);
        }
        let l_name = u16_at(bytes, local + 26) as usize;
        let l_extra = u16_at(bytes, local + 28) as usize;
        let start = local + 30 + l_name + l_extra;
        let end = start.checked_add(csize).ok_or(ZipError::Truncated)?;
        if end > bytes.len() {
            return Err(ZipError::Truncated);
        }
        let data = if method == 8 {
            miniz_oxide::inflate::decompress_to_vec_with_limit(&bytes[start..end], usize_)
                .map_err(|_| ZipError::Corrupt(name.clone()))?
        } else {
            bytes[start..end].to_vec()
        };
        if data.len() != usize_ || crc32(&data) != crc {
            return Err(ZipError::Corrupt(name));
        }
        out.push((name, data));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_reference_check_value() {
        // The standard CRC-32 check: "123456789" -> 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn entries_round_trip_in_order_with_their_bytes() {
        let big: Vec<u8> = (0..100_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let zip = write_store_zip(&[
            ("manifest.json", b"{\"a\":1}"),
            ("dir/events.arrow", &big),
            ("empty", b""),
        ])
        .unwrap();
        let back = read_store_zip(&zip).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[0].0, "manifest.json");
        assert_eq!(back[0].1, b"{\"a\":1}");
        assert_eq!(back[1].0, "dir/events.arrow");
        assert_eq!(back[1].1, big);
        assert_eq!(back[2].0, "empty");
        assert!(back[2].1.is_empty());
    }

    #[test]
    fn stored_entries_can_be_read_in_place_and_deflated_ones_cannot() {
        let zip = write_store_zip(&[("a", b"hello"), ("dir/b", b"world!")]).unwrap();
        let ranges = stored_entry_ranges(&zip).unwrap();
        assert_eq!(ranges.len(), 2);
        assert_eq!(&zip[ranges[0].1.clone()], b"hello");
        assert_eq!(ranges[1].0, "dir/b");
        assert_eq!(&zip[ranges[1].1.clone()], b"world!");
        let big: Vec<u8> = vec![0; 100_000];
        let deflated = write_zip(&[("a", &big)], Some(6)).unwrap();
        assert!(matches!(stored_entry_ranges(&deflated), Err(ZipError::Unsupported(_))));
    }

    #[test]
    fn the_layout_is_what_other_tools_expect() {
        let zip = write_store_zip(&[("a.txt", b"hello")]).unwrap();
        // Local header signature first, end record signature last.
        assert_eq!(&zip[0..4], &LOCAL_SIG.to_le_bytes());
        assert_eq!(&zip[zip.len() - 22..zip.len() - 18], &EOCD_SIG.to_le_bytes());
        // Store method, and the name right after the 30-byte local header.
        assert_eq!(u16_at(&zip, 8), 0);
        assert_eq!(&zip[30..35], b"a.txt");
        assert_eq!(&zip[35..40], b"hello");
    }

    #[test]
    fn a_flipped_byte_is_reported_as_corrupt_not_returned() {
        let mut zip = write_store_zip(&[("a.txt", b"hello world")]).unwrap();
        zip[37] ^= 0x01; // inside "hello world" (30-byte header + 5-byte name)
        assert_eq!(read_store_zip(&zip), Err(ZipError::Corrupt("a.txt".into())));
    }

    #[test]
    fn garbage_and_truncation_are_errors() {
        assert_eq!(read_store_zip(b"PK"), Err(ZipError::NotAZip));
        assert_eq!(read_store_zip(&[0u8; 64]), Err(ZipError::NotAZip));
        let zip = write_store_zip(&[("a.txt", b"hello")]).unwrap();
        // Keep the end record but lose the front: the local header is gone.
        let mut cut = zip[10..].to_vec();
        // Fix nothing up: offsets now point past what exists or at wrong bytes.
        cut.truncate(cut.len());
        assert!(read_store_zip(&cut).is_err());
    }

    #[test]
    fn an_unknown_method_is_refused_rather_than_misread() {
        let mut zip = write_store_zip(&[("a.txt", b"hello")]).unwrap();
        // Central directory method field: locate the central signature.
        let cd = (0..zip.len() - 4).find(|&i| u32_at(&zip, i) == CENTRAL_SIG).unwrap();
        zip[cd + 10] = 12; // bzip2
        assert!(matches!(read_store_zip(&zip), Err(ZipError::Unsupported(_))));
    }

    #[test]
    fn deflated_entries_round_trip_and_shrink_redundant_data() {
        // Something like an events table: mostly zero bytes.
        let big: Vec<u8> = (0..200_000u32).flat_map(|i| [(i % 7) as u8, 0, 0, 0, (i % 3) as u8, 0, 0, 0]).collect();
        // xorshift64: deflate cannot shrink this, so it must be stored.
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let random: Vec<u8> = (0..4096)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 56) as u8
            })
            .collect();
        let zip = write_zip(&[("events.arrow", &big), ("noise.bin", &random), ("empty", b"")], Some(6)).unwrap();
        assert!(zip.len() < big.len() / 4, "deflate should pack this 4:1 at least, got {}", zip.len());
        let back = read_store_zip(&zip).unwrap();
        assert_eq!(back[0].1, big);
        assert_eq!(back[1].1, random);
        assert!(back[2].1.is_empty());
        // The incompressible entry was stored, not grown: method 0 in its header.
        let cd = (0..zip.len() - 4).find(|&i| u32_at(&zip, i) == CENTRAL_SIG).unwrap();
        let second = (cd + 46..zip.len() - 4).find(|&i| u32_at(&zip, i) == CENTRAL_SIG).unwrap();
        assert_eq!(u16_at(&zip, cd + 10), 8, "events deflated");
        assert_eq!(u16_at(&zip, second + 10), 0, "noise stored");
        // A corrupted deflate stream is reported, not returned.
        let mut bad = zip.clone();
        bad[60] ^= 0xFF; // inside the deflate stream (30-byte header + 12-byte name)
        assert!(matches!(read_store_zip(&bad), Err(ZipError::Corrupt(_))));
    }
}
