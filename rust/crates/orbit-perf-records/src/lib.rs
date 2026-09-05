// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Packed layouts of the perf_event_open ring-buffer records Orbit consumes,
//! ported from `src/LinuxTracing/PerfEventRecords.h`.
//!
//! The kernel's perf ABI is the spec both languages answer to, so every
//! struct here must match the C++ layout bit for bit. Two things enforce
//! that: the `record!` macro below generates the struct, its parser, and its
//! field-offset table from one field list, so they cannot drift from each
//! other; and the layout parity test in
//! `src/LinuxTracing/PerfEventRecordsLayoutParityTest.cpp` compares `sizeof`
//! and `offsetof` of every C++ struct against the values this crate exports,
//! so the two languages cannot drift from one another.

#![deny(unsafe_code)]

pub mod reader;
pub mod tracepoints;

/// Little-endian decoding of one field out of a byte slice. The slice handed
/// to `from_le_slice` is exactly `SIZE` bytes.
pub trait FromLe: Sized + Copy {
    const SIZE: usize;
    fn from_le_slice(bytes: &[u8]) -> Self;
}

macro_rules! impl_from_le_int {
    ($($ty:ty),+) => {
        $(impl FromLe for $ty {
            const SIZE: usize = core::mem::size_of::<$ty>();
            fn from_le_slice(bytes: &[u8]) -> Self {
                Self::from_le_bytes(bytes.try_into().expect("caller sliced SIZE bytes"))
            }
        })+
    };
}
impl_from_le_int!(u16, u32, u64);

impl<T: FromLe, const N: usize> FromLe for [T; N] {
    const SIZE: usize = T::SIZE * N;
    fn from_le_slice(bytes: &[u8]) -> Self {
        core::array::from_fn(|i| T::from_le_slice(&bytes[i * T::SIZE..(i + 1) * T::SIZE]))
    }
}

/// Defines a packed record struct plus its parser and field-offset table
/// from a single field list. Fields decode in declaration order; because the
/// struct is `repr(C, packed)`, declaration order and byte order coincide,
/// which `parse` debug-asserts by walking exactly `SIZE` bytes.
macro_rules! record {
    ($(#[$meta:meta])* $name:ident { $($field:ident: $ty:ty),+ $(,)? }) => {
        $(#[$meta])*
        #[repr(C, packed)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name {
            $(pub $field: $ty,)+
        }

        impl $name {
            pub const SIZE: usize = core::mem::size_of::<Self>();

            /// Byte offset of each field, in declaration order. The layout
            /// parity test compares these against C++ `offsetof`.
            pub const FIELD_OFFSETS: &'static [usize] = &[
                $(core::mem::offset_of!($name, $field),)+
            ];

            pub fn parse(bytes: &[u8]) -> Option<Self> {
                if bytes.len() < Self::SIZE {
                    return None;
                }
                let mut offset = 0usize;
                $(
                    let $field =
                        <$ty as FromLe>::from_le_slice(&bytes[offset..offset + <$ty as FromLe>::SIZE]);
                    offset += <$ty as FromLe>::SIZE;
                )+
                debug_assert_eq!(offset, Self::SIZE);
                Some(Self { $($field,)+ })
            }
        }

        impl FromLe for $name {
            const SIZE: usize = core::mem::size_of::<Self>();
            fn from_le_slice(bytes: &[u8]) -> Self {
                Self::parse(bytes).expect("caller sliced SIZE bytes")
            }
        }
    };
}

/// The record type ids of `perf_event_header::type` Orbit consumes, from
/// `linux/perf_event.h`.
pub mod record_type {
    pub const MMAP: u32 = 1;
    pub const LOST: u32 = 2;
    pub const EXIT: u32 = 4;
    pub const THROTTLE: u32 = 5;
    pub const UNTHROTTLE: u32 = 6;
    pub const FORK: u32 = 7;
    pub const SAMPLE: u32 = 9;
    pub const SWITCH: u32 = 14;
    pub const SWITCH_CPU_WIDE: u32 = 15;
}

record!(
    /// `perf_event_header`. The C++ side calls the first field `type`, a
    /// Rust keyword, so it is `kind` here; the layout is what matters.
    PerfEventHeader {
        kind: u32,
        misc: u16,
        size: u16,
    }
);

record!(
    /// Must stay in sync with `kSampleTypeTidTimeStreamidCpu`: the bits set
    /// in `perf_event_attr::sample_type` determine these fields.
    SampleIdTidTimeStreamidCpu {
        pid: u32,
        tid: u32,
        time: u64,
        stream_id: u64,
        cpu: u32,
        res: u32,
    }
);

record!(
    ForkExit {
        header: PerfEventHeader,
        pid: u32,
        ppid: u32,
        tid: u32,
        ptid: u32,
        time: u64,
        sample_id: SampleIdTidTimeStreamidCpu,
    }
);

#[cfg(target_arch = "x86_64")]
record!(
    /// Must stay in sync with `kSampleRegsUserAll`.
    SampleRegsUserAll {
        ax: u64,
        bx: u64,
        cx: u64,
        dx: u64,
        si: u64,
        di: u64,
        bp: u64,
        sp: u64,
        ip: u64,
        flags: u64,
        cs: u64,
        ss: u64,
        r8: u64,
        r9: u64,
        r10: u64,
        r11: u64,
        r12: u64,
        r13: u64,
        r14: u64,
        r15: u64,
    }
);

#[cfg(target_arch = "aarch64")]
record!(
    /// Must stay in sync with `kSampleRegsUserAll`. x0-x30; x30 is LR.
    SampleRegsUserAll {
        x: [u64; 31],
        sp: u64,
        pc: u64,
    }
);

impl SampleRegsUserAll {
    #[cfg(target_arch = "x86_64")]
    pub fn instruction_pointer(&self) -> u64 {
        self.ip
    }
    #[cfg(target_arch = "x86_64")]
    pub fn stack_pointer(&self) -> u64 {
        self.sp
    }
    #[cfg(target_arch = "x86_64")]
    pub fn frame_pointer(&self) -> u64 {
        self.bp
    }

    #[cfg(target_arch = "aarch64")]
    pub fn instruction_pointer(&self) -> u64 {
        self.pc
    }
    #[cfg(target_arch = "aarch64")]
    pub fn stack_pointer(&self) -> u64 {
        self.sp
    }
    #[cfg(target_arch = "aarch64")]
    pub fn frame_pointer(&self) -> u64 {
        self.x[29]
    }
}

#[cfg(target_arch = "x86_64")]
record!(
    /// Must stay in sync with `kSampleRegsUserAx`.
    SampleRegsUserAx {
        abi: u64,
        ax: u64,
    }
);

#[cfg(target_arch = "aarch64")]
record!(
    /// Must stay in sync with `kSampleRegsUserAx`; x0 holds return values.
    SampleRegsUserAx {
        abi: u64,
        x0: u64,
    }
);

impl SampleRegsUserAx {
    #[cfg(target_arch = "x86_64")]
    pub fn return_value(&self) -> u64 {
        self.ax
    }
    #[cfg(target_arch = "aarch64")]
    pub fn return_value(&self) -> u64 {
        self.x0
    }
}

#[cfg(target_arch = "x86_64")]
record!(
    /// Must stay in sync with `kSampleRegsUserSpIp`.
    SampleRegsUserSpIp {
        abi: u64,
        sp: u64,
        ip: u64,
    }
);

#[cfg(target_arch = "aarch64")]
record!(
    /// Must stay in sync with `kSampleRegsUserSpIp`.
    SampleRegsUserSpIp {
        abi: u64,
        sp: u64,
        pc: u64,
    }
);

impl SampleRegsUserSpIp {
    #[cfg(target_arch = "x86_64")]
    pub fn instruction_pointer(&self) -> u64 {
        self.ip
    }
    #[cfg(target_arch = "aarch64")]
    pub fn instruction_pointer(&self) -> u64 {
        self.pc
    }
}

record!(
    /// Must stay in sync with `kSampleRegsUserSp`.
    SampleRegsUserSp {
        sp: u64,
    }
);

#[cfg(target_arch = "x86_64")]
record!(
    /// Must stay in sync with `kSampleRegsUserSpIpArguments`.
    SampleRegsUserSpIpArguments {
        abi: u64,
        cx: u64,
        dx: u64,
        si: u64,
        di: u64,
        sp: u64,
        ip: u64,
        r8: u64,
        r9: u64,
    }
);

#[cfg(target_arch = "aarch64")]
record!(
    /// Must stay in sync with `kSampleRegsUserSpIpArguments`; x0-x7 carry
    /// arguments in the AArch64 AAPCS.
    SampleRegsUserSpIpArguments {
        abi: u64,
        x0: u64,
        x1: u64,
        x2: u64,
        x3: u64,
        x4: u64,
        x5: u64,
        x6: u64,
        x7: u64,
        sp: u64,
        pc: u64,
    }
);

impl SampleRegsUserSpIpArguments {
    #[cfg(target_arch = "x86_64")]
    pub fn instruction_pointer(&self) -> u64 {
        self.ip
    }
    /// Arguments in x86_64 SysV order: rdi, rsi, rdx, rcx, r8, r9.
    #[cfg(target_arch = "x86_64")]
    pub fn args(&self) -> [u64; 6] {
        [self.di, self.si, self.dx, self.cx, self.r8, self.r9]
    }

    #[cfg(target_arch = "aarch64")]
    pub fn instruction_pointer(&self) -> u64 {
        self.pc
    }
    #[cfg(target_arch = "aarch64")]
    pub fn args(&self) -> [u64; 6] {
        [self.x0, self.x1, self.x2, self.x3, self.x4, self.x5]
    }
}

record!(
    SampleStackUser8bytes {
        size: u64,
        top8bytes: u64,
        dyn_size: u64,
    }
);

record!(
    /// After `regs` the record continues with `u64 size`, `char data[size]`
    /// and `u64 dyn_size` (the last only when `size != 0`), which are read
    /// dynamically.
    StackSampleFixed {
        header: PerfEventHeader,
        sample_id: SampleIdTidTimeStreamidCpu,
        abi: u64,
        regs: SampleRegsUserAll,
    }
);

record!(
    SpIpArguments8bytesSample {
        header: PerfEventHeader,
        sample_id: SampleIdTidTimeStreamidCpu,
        regs: SampleRegsUserSpIpArguments,
        stack: SampleStackUser8bytes,
    }
);

record!(
    SpIp8bytesSample {
        header: PerfEventHeader,
        sample_id: SampleIdTidTimeStreamidCpu,
        regs: SampleRegsUserSpIp,
        stack: SampleStackUser8bytes,
    }
);

record!(
    /// Like `StackSampleFixed`, continues with a dynamically-read stack.
    SpStackUserSampleFixed {
        header: PerfEventHeader,
        sample_id: SampleIdTidTimeStreamidCpu,
        abi: u64,
        regs: SampleRegsUserSp,
    }
);

record!(
    EmptySample {
        header: PerfEventHeader,
        sample_id: SampleIdTidTimeStreamidCpu,
    }
);

record!(
    AxSample {
        header: PerfEventHeader,
        sample_id: SampleIdTidTimeStreamidCpu,
        regs: SampleRegsUserAx,
    }
);

record!(
    /// The rest of the sample is a `char[size]` read dynamically.
    RawSampleFixed {
        header: PerfEventHeader,
        sample_id: SampleIdTidTimeStreamidCpu,
        size: u32,
    }
);

record!(
    /// The record continues with `char filename[]` and the sample id, both
    /// read dynamically because `filename` is variable-length.
    MmapUpToPgoff {
        header: PerfEventHeader,
        pid: u32,
        tid: u32,
        address: u64,
        length: u64,
        page_offset: u64,
    }
);

record!(
    Lost {
        header: PerfEventHeader,
        id: u64,
        lost: u64,
        sample_id: SampleIdTidTimeStreamidCpu,
    }
);

record!(
    ThrottleUnthrottle {
        header: PerfEventHeader,
        time: u64,
        id: u64,
        lost: u64,
        sample_id: SampleIdTidTimeStreamidCpu,
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_match_the_perf_abi() {
        assert_eq!(PerfEventHeader::SIZE, 8);
        assert_eq!(SampleIdTidTimeStreamidCpu::SIZE, 32);
        assert_eq!(ForkExit::SIZE, 64);
        assert_eq!(SampleRegsUserSp::SIZE, 8);
        assert_eq!(SampleStackUser8bytes::SIZE, 24);
        assert_eq!(EmptySample::SIZE, 40);
        assert_eq!(RawSampleFixed::SIZE, 44);
        assert_eq!(MmapUpToPgoff::SIZE, 40);
        assert_eq!(Lost::SIZE, 56);
        assert_eq!(ThrottleUnthrottle::SIZE, 64);
        #[cfg(target_arch = "x86_64")]
        {
            assert_eq!(SampleRegsUserAll::SIZE, 160);
            assert_eq!(SampleRegsUserAx::SIZE, 16);
            assert_eq!(SampleRegsUserSpIp::SIZE, 24);
            assert_eq!(SampleRegsUserSpIpArguments::SIZE, 72);
            assert_eq!(StackSampleFixed::SIZE, 208);
            assert_eq!(SpIpArguments8bytesSample::SIZE, 136);
            assert_eq!(SpIp8bytesSample::SIZE, 88);
            assert_eq!(SpStackUserSampleFixed::SIZE, 56);
            assert_eq!(AxSample::SIZE, 56);
        }
    }

    #[test]
    fn offsets_are_dense_and_end_at_size() {
        fn check(offsets: &[usize], sizes: &[usize], total: usize) {
            let mut expected = 0usize;
            for (offset, size) in offsets.iter().zip(sizes) {
                assert_eq!(*offset, expected);
                expected += size;
            }
            assert_eq!(expected, total);
        }
        check(
            ForkExit::FIELD_OFFSETS,
            &[8, 4, 4, 4, 4, 8, 32],
            ForkExit::SIZE,
        );
        check(
            ThrottleUnthrottle::FIELD_OFFSETS,
            &[8, 8, 8, 8, 32],
            ThrottleUnthrottle::SIZE,
        );
        check(
            RawSampleFixed::FIELD_OFFSETS,
            &[8, 32, 4],
            RawSampleFixed::SIZE,
        );
    }

    #[test]
    fn fork_exit_parses_field_by_field() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&record_type::FORK.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&(ForkExit::SIZE as u16).to_le_bytes());
        for value in [11u32, 10, 12, 10] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&700u64.to_le_bytes());
        bytes.extend_from_slice(&11u32.to_le_bytes());
        bytes.extend_from_slice(&12u32.to_le_bytes());
        bytes.extend_from_slice(&700u64.to_le_bytes());
        bytes.extend_from_slice(&3u64.to_le_bytes());
        bytes.extend_from_slice(&5u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());

        let record = ForkExit::parse(&bytes).unwrap();
        assert_eq!({ record.header.kind }, record_type::FORK);
        assert_eq!({ record.header.size } as usize, ForkExit::SIZE);
        assert_eq!({ record.pid }, 11);
        assert_eq!({ record.ppid }, 10);
        assert_eq!({ record.tid }, 12);
        assert_eq!({ record.ptid }, 10);
        assert_eq!({ record.time }, 700);
        assert_eq!({ record.sample_id.stream_id }, 3);
        assert_eq!({ record.sample_id.cpu }, 5);

        assert!(ForkExit::parse(&bytes[..ForkExit::SIZE - 1]).is_none());
    }

    #[test]
    fn nested_parse_round_trips() {
        let mut bytes = vec![0u8; Lost::SIZE];
        bytes[0..4].copy_from_slice(&record_type::LOST.to_le_bytes());
        bytes[8..16].copy_from_slice(&77u64.to_le_bytes());
        bytes[16..24].copy_from_slice(&1234u64.to_le_bytes());
        let record = Lost::parse(&bytes).unwrap();
        assert_eq!({ record.header.kind }, record_type::LOST);
        assert_eq!({ record.id }, 77);
        assert_eq!({ record.lost }, 1234);
    }
}
