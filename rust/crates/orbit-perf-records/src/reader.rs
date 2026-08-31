// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Parsing of whole ring-buffer records, dynamic tails included, ported from
//! `src/LinuxTracing/PerfEventReaders.cpp`.
//!
//! `parse_record_sample` is the Rust twin of the C++ `ConsumeRecordSample`:
//! the same flag-driven cursor walk over the same field order, including its
//! quirks -- the stack copy takes only `dyn_size` of the `stack_size` bytes,
//! `dyn_size` itself is read only when stack data is copied while the cursor
//! still advances past it, and the callchain ips advance the cursor even
//! when not copied. The one deliberate difference: where the C++ trusts the
//! kernel and reads unchecked, this returns `None` on a truncated record
//! instead of reading out of bounds.
//!
//! Byte-for-byte agreement with the C++ consumers on kernel-produced records
//! is checked by `rust/tools/differential/perf_reader_differential.cpp`.

use crate::{
    FromLe, MmapUpToPgoff, PerfEventHeader, SampleIdTidTimeStreamidCpu,
};

/// The `PERF_SAMPLE_*` bits of `perf_event_attr::sample_type`, from
/// `linux/perf_event.h`.
pub mod sample_bits {
    pub const IP: u64 = 1 << 0;
    pub const TID: u64 = 1 << 1;
    pub const TIME: u64 = 1 << 2;
    pub const ADDR: u64 = 1 << 3;
    pub const CALLCHAIN: u64 = 1 << 5;
    pub const ID: u64 = 1 << 6;
    pub const CPU: u64 = 1 << 7;
    pub const PERIOD: u64 = 1 << 8;
    pub const STREAM_ID: u64 = 1 << 9;
    pub const RAW: u64 = 1 << 10;
    pub const REGS_USER: u64 = 1 << 12;
    pub const STACK_USER: u64 = 1 << 13;
    pub const IDENTIFIER: u64 = 1 << 16;

    /// `kSampleTypeTidTimeStreamidCpu` in PerfEventOpen.h.
    pub const TID_TIME_STREAMID_CPU: u64 = TID | TIME | STREAM_ID | CPU;
}

/// `PERF_RECORD_MISC_MMAP_DATA` in `perf_event_header::misc`.
pub const MISC_MMAP_DATA: u16 = 1 << 13;

/// Number of registers dumped for `kSampleRegsUserAll`.
#[cfg(target_arch = "x86_64")]
pub const REGS_USER_ALL_COUNT: usize = 20;
#[cfg(target_arch = "aarch64")]
pub const REGS_USER_ALL_COUNT: usize = 33;

/// What the C++ side passes as `perf_event_attr flags`: the sample-type bits
/// and how many user registers a `PERF_SAMPLE_REGS_USER` block carries
/// (`popcount(sample_regs_user)` in the C++).
#[derive(Clone, Copy, Debug)]
pub struct SampleFlags {
    pub sample_type: u64,
    pub regs_user_count: usize,
}

impl SampleFlags {
    /// The flags of `stack_sample_event_open` / `ConsumeStackSamplePerfEvent`
    /// and `uprobes_with_stack_and_sp` reuse this shape with other counts.
    pub fn stack_sample() -> Self {
        Self {
            sample_type: sample_bits::REGS_USER
                | sample_bits::STACK_USER
                | sample_bits::TID_TIME_STREAMID_CPU,
            regs_user_count: REGS_USER_ALL_COUNT,
        }
    }

    /// The flags of `callchain_sample_event_open` /
    /// `ConsumeCallchainSamplePerfEvent`.
    pub fn callchain_sample() -> Self {
        Self {
            sample_type: sample_bits::REGS_USER
                | sample_bits::STACK_USER
                | sample_bits::CALLCHAIN
                | sample_bits::TID_TIME_STREAMID_CPU,
            regs_user_count: REGS_USER_ALL_COUNT,
        }
    }
}

/// The Rust twin of the C++ `PerfRecordSample` intermediate: every field the
/// flag walk can produce, zeroed when its bit is absent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecordSample {
    pub identifier: u64,
    pub ip: u64,
    pub pid: u32,
    pub tid: u32,
    pub time: u64,
    pub addr: u64,
    pub id: u64,
    pub stream_id: u64,
    pub cpu: u32,
    pub res: u32,
    pub period: u64,
    pub ips: Option<Vec<u64>>,
    pub ips_size: u64,
    pub raw_data: Option<Vec<u8>>,
    pub abi: u64,
    pub regs: Option<Vec<u64>>,
    pub stack_size: u64,
    pub stack_data: Option<Vec<u8>>,
    pub dyn_size: u64,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn read<T: FromLe>(&mut self) -> Option<T> {
        let end = self.offset.checked_add(T::SIZE)?;
        let slice = self.bytes.get(self.offset..end)?;
        self.offset = end;
        Some(T::from_le_slice(slice))
    }

    fn read_bytes(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(count)?;
        let slice = self.bytes.get(self.offset..end)?;
        self.offset = end;
        Some(slice)
    }

    fn skip(&mut self, count: usize) -> Option<()> {
        let end = self.offset.checked_add(count)?;
        if end > self.bytes.len() {
            return None;
        }
        self.offset = end;
        Some(())
    }

    fn peek_u64_at(&self, offset: usize) -> Option<u64> {
        let slice = self.bytes.get(offset..offset.checked_add(8)?)?;
        Some(u64::from_le_slice(slice))
    }
}

/// Twin of `ConsumeRecordSample`. `bytes` is the whole record,
/// `header.size` long, as copied out of the ring buffer.
pub fn parse_record_sample(
    bytes: &[u8],
    flags: SampleFlags,
    copy_stack_related_data: bool,
) -> Option<RecordSample> {
    let mut cursor = Cursor { bytes, offset: 0 };
    let mut sample = RecordSample::default();

    let _header: PerfEventHeader = cursor.read()?;
    let st = flags.sample_type;

    if st & sample_bits::IDENTIFIER != 0 {
        sample.identifier = cursor.read()?;
    }
    if st & sample_bits::IP != 0 {
        sample.ip = cursor.read()?;
    }
    if st & sample_bits::TID != 0 {
        sample.pid = cursor.read()?;
        sample.tid = cursor.read()?;
    }
    if st & sample_bits::TIME != 0 {
        sample.time = cursor.read()?;
    }
    if st & sample_bits::ADDR != 0 {
        sample.addr = cursor.read()?;
    }
    if st & sample_bits::ID != 0 {
        sample.id = cursor.read()?;
    }
    if st & sample_bits::STREAM_ID != 0 {
        sample.stream_id = cursor.read()?;
    }
    if st & sample_bits::CPU != 0 {
        sample.cpu = cursor.read()?;
        sample.res = cursor.read()?;
    }
    if st & sample_bits::PERIOD != 0 {
        sample.period = cursor.read()?;
    }
    if st & sample_bits::CALLCHAIN != 0 {
        sample.ips_size = cursor.read()?;
        let byte_count = (sample.ips_size as usize).checked_mul(8)?;
        if copy_stack_related_data {
            let ip_bytes = cursor.read_bytes(byte_count)?;
            sample.ips = Some(
                ip_bytes
                    .chunks_exact(8)
                    .map(u64::from_le_slice)
                    .collect(),
            );
        } else {
            cursor.skip(byte_count)?;
        }
    }
    if st & sample_bits::RAW != 0 {
        let raw_size: u32 = cursor.read()?;
        sample.raw_data = Some(cursor.read_bytes(raw_size as usize)?.to_vec());
    }
    if st & sample_bits::REGS_USER != 0 {
        sample.abi = cursor.read()?;
        // PERF_SAMPLE_REGS_ABI_NONE == 0: no register block follows.
        if sample.abi != 0 {
            let byte_count = flags.regs_user_count.checked_mul(8)?;
            if copy_stack_related_data {
                let reg_bytes = cursor.read_bytes(byte_count)?;
                sample.regs = Some(
                    reg_bytes
                        .chunks_exact(8)
                        .map(u64::from_le_slice)
                        .collect(),
                );
            } else {
                cursor.skip(byte_count)?;
            }
        }
    }
    if st & sample_bits::STACK_USER != 0 {
        sample.stack_size = cursor.read()?;
        let stack_size = sample.stack_size as usize;
        if sample.stack_size != 0 && copy_stack_related_data {
            // dyn_size sits after the stack bytes; the C++ reads it first so
            // it can copy only the used part of the stack. dyn_size stays 0
            // when stack data is not copied -- the C++ does the same.
            sample.dyn_size = cursor.peek_u64_at(cursor.offset.checked_add(stack_size)?)?;
            let used = sample.dyn_size as usize;
            if used > stack_size {
                return None;
            }
            sample.stack_data =
                Some(cursor.bytes.get(cursor.offset..cursor.offset + used)?.to_vec());
        }
        cursor.skip(stack_size)?;
        if sample.stack_size != 0 {
            cursor.skip(8)?;
        }
    }

    Some(sample)
}

/// Where the event's capture timestamp lives, mirroring how TracerImpl
/// reads it before deferring a record: a PERF_RECORD_SAMPLE opens with the
/// sample id right after the header (`ReadSampleRecordTime`), every other
/// record carries it in the `sample_id_all` tail.
pub fn record_timestamp(bytes: &[u8]) -> Option<u64> {
    let header = PerfEventHeader::parse(bytes)?;
    if { header.kind } == crate::record_type::SAMPLE {
        let start = PerfEventHeader::SIZE;
        return SampleIdTidTimeStreamidCpu::parse(bytes.get(start..)?).map(|id| id.time);
    }
    parse_sample_id_all(bytes).map(|id| id.time)
}

/// Twin of `ReadPerfSampleIdAll`: `sample_id_all` puts the sample id at the
/// very end of every non-SAMPLE record.
pub fn parse_sample_id_all(bytes: &[u8]) -> Option<SampleIdTidTimeStreamidCpu> {
    let start = bytes.len().checked_sub(SampleIdTidTimeStreamidCpu::SIZE)?;
    if start <= PerfEventHeader::SIZE {
        return None;
    }
    SampleIdTidTimeStreamidCpu::parse(&bytes[start..])
}

/// The projection `ConsumeMmapPerfEvent` builds, quirks included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmapRecord {
    pub pid: i32,
    pub timestamp: u64,
    pub address: u64,
    pub length: u64,
    pub page_offset: u64,
    pub filename: Vec<u8>,
    pub executable: bool,
}

/// Twin of `ConsumeMmapPerfEvent`.
pub fn parse_mmap(bytes: &[u8]) -> Option<MmapRecord> {
    let fixed = MmapUpToPgoff::parse(bytes)?;
    let sample_id = parse_sample_id_all(bytes)?;

    let filename_end = bytes.len().checked_sub(SampleIdTidTimeStreamidCpu::SIZE)?;
    if filename_end <= MmapUpToPgoff::SIZE {
        return None;
    }
    let mut filename_bytes = bytes[MmapUpToPgoff::SIZE..filename_end].to_vec();
    // "This is a bit paranoid but you never know."
    *filename_bytes.last_mut()? = 0;
    let nul = filename_bytes.iter().position(|&b| b == 0)?;
    filename_bytes.truncate(nul);

    // Anonymous maps arrive as "//anon"; the C++ clears the name and, for
    // anonymous or bracketed maps whose page_offset equals the address,
    // zeroes the page_offset.
    if filename_bytes == b"//anon" {
        filename_bytes.clear();
    }
    let mut page_offset = fixed.page_offset;
    if (filename_bytes.is_empty() || filename_bytes[0] == b'[')
        && page_offset == fixed.address
    {
        page_offset = 0;
    }

    let executable = fixed.header.misc & MISC_MMAP_DATA == 0;

    Some(MmapRecord {
        pid: sample_id.pid as i32,
        timestamp: sample_id.time,
        address: fixed.address,
        length: fixed.length,
        page_offset,
        filename: filename_bytes,
        executable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record_type;

    fn push_u64(bytes: &mut Vec<u8>, value: u64) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn header(kind: u32, misc: u16, size: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_u32(&mut bytes, kind);
        bytes.extend_from_slice(&misc.to_le_bytes());
        bytes.extend_from_slice(&(size as u16).to_le_bytes());
        bytes
    }

    fn stack_sample_bytes(stack_size: u64, dyn_size: u64, abi: u64) -> Vec<u8> {
        let mut bytes = header(record_type::SAMPLE, 0, 0);
        push_u32(&mut bytes, 100); // pid
        push_u32(&mut bytes, 101); // tid
        push_u64(&mut bytes, 5000); // time
        push_u64(&mut bytes, 7); // stream_id
        push_u32(&mut bytes, 3); // cpu
        push_u32(&mut bytes, 0); // res
        push_u64(&mut bytes, abi);
        if abi != 0 {
            for reg in 0..REGS_USER_ALL_COUNT as u64 {
                push_u64(&mut bytes, 0x1000 + reg);
            }
        }
        push_u64(&mut bytes, stack_size);
        for i in 0..stack_size {
            bytes.push(i as u8);
        }
        if stack_size != 0 {
            push_u64(&mut bytes, dyn_size);
        }
        let size = bytes.len();
        bytes[6..8].copy_from_slice(&(size as u16).to_le_bytes());
        bytes
    }

    #[test]
    fn stack_sample_copies_only_dyn_size_bytes() {
        let bytes = stack_sample_bytes(64, 24, 1);
        let sample =
            parse_record_sample(&bytes, SampleFlags::stack_sample(), true).unwrap();
        assert_eq!(sample.pid, 100);
        assert_eq!(sample.tid, 101);
        assert_eq!(sample.time, 5000);
        assert_eq!(sample.stream_id, 7);
        assert_eq!(sample.cpu, 3);
        assert_eq!(sample.regs.as_ref().unwrap().len(), REGS_USER_ALL_COUNT);
        assert_eq!(sample.regs.as_ref().unwrap()[2], 0x1002);
        assert_eq!(sample.stack_size, 64);
        assert_eq!(sample.dyn_size, 24);
        let stack = sample.stack_data.unwrap();
        assert_eq!(stack.len(), 24);
        assert_eq!(stack[23], 23);
    }

    #[test]
    fn without_copy_dyn_size_stays_zero_like_the_cpp() {
        let bytes = stack_sample_bytes(64, 24, 1);
        let sample =
            parse_record_sample(&bytes, SampleFlags::stack_sample(), false).unwrap();
        assert_eq!(sample.dyn_size, 0);
        assert!(sample.stack_data.is_none());
        assert!(sample.regs.is_none());
        assert_eq!(sample.stack_size, 64);
    }

    #[test]
    fn abi_none_means_no_register_block() {
        let bytes = stack_sample_bytes(0, 0, 0);
        let sample =
            parse_record_sample(&bytes, SampleFlags::stack_sample(), true).unwrap();
        assert_eq!(sample.abi, 0);
        assert!(sample.regs.is_none());
        assert!(sample.stack_data.is_none());
    }

    #[test]
    fn truncated_record_returns_none() {
        let bytes = stack_sample_bytes(64, 24, 1);
        assert!(
            parse_record_sample(&bytes[..bytes.len() - 9], SampleFlags::stack_sample(), true)
                .is_none()
        );
    }

    #[test]
    fn callchain_walks_ips() {
        let mut bytes = header(record_type::SAMPLE, 0, 0);
        push_u32(&mut bytes, 1);
        push_u32(&mut bytes, 2);
        push_u64(&mut bytes, 3);
        push_u64(&mut bytes, 4);
        push_u32(&mut bytes, 5);
        push_u32(&mut bytes, 0);
        push_u64(&mut bytes, 3); // ips_size
        for ip in [0xa0u64, 0xb0, 0xc0] {
            push_u64(&mut bytes, ip);
        }
        push_u64(&mut bytes, 0); // abi = none
        push_u64(&mut bytes, 0); // stack size = 0
        let size = bytes.len();
        bytes[6..8].copy_from_slice(&(size as u16).to_le_bytes());

        let sample =
            parse_record_sample(&bytes, SampleFlags::callchain_sample(), true).unwrap();
        assert_eq!(sample.ips_size, 3);
        assert_eq!(sample.ips.unwrap(), vec![0xa0, 0xb0, 0xc0]);
    }

    #[test]
    fn mmap_quirks_match_the_cpp() {
        fn mmap_bytes(filename: &[u8], address: u64, page_offset: u64, misc: u16) -> Vec<u8> {
            let mut bytes = header(record_type::MMAP, misc, 0);
            push_u32(&mut bytes, 42); // pid
            push_u32(&mut bytes, 43); // tid
            push_u64(&mut bytes, address);
            push_u64(&mut bytes, 0x2000); // length
            push_u64(&mut bytes, page_offset);
            bytes.extend_from_slice(filename);
            bytes.push(0);
            // sample_id tail
            push_u32(&mut bytes, 42);
            push_u32(&mut bytes, 43);
            push_u64(&mut bytes, 9999); // time
            push_u64(&mut bytes, 1); // stream
            push_u32(&mut bytes, 0); // cpu
            push_u32(&mut bytes, 0);
            let size = bytes.len();
            bytes[6..8].copy_from_slice(&(size as u16).to_le_bytes());
            bytes
        }

        let named = parse_mmap(&mmap_bytes(b"/usr/lib/libc.so", 0x7000, 0x1000, 0)).unwrap();
        assert_eq!(named.filename, b"/usr/lib/libc.so");
        assert_eq!(named.page_offset, 0x1000);
        assert_eq!(named.pid, 42);
        assert_eq!(named.timestamp, 9999);
        assert!(named.executable);

        let anon = parse_mmap(&mmap_bytes(b"//anon", 0x7000, 0x7000, MISC_MMAP_DATA)).unwrap();
        assert!(anon.filename.is_empty());
        assert_eq!(anon.page_offset, 0);
        assert!(!anon.executable);

        let bracket = parse_mmap(&mmap_bytes(b"[heap]", 0x8000, 0x8000, 0)).unwrap();
        assert_eq!(bracket.filename, b"[heap]");
        assert_eq!(bracket.page_offset, 0);
    }
}
