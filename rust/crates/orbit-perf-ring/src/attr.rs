// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `perf_event_attr` construction, mirroring `PerfEventOpen.cpp`.

use std::sync::OnceLock;

/// `perf_event_attr` through `PERF_ATTR_SIZE_VER8` (136 bytes). The kernel
/// accepts any size it knows; `size` says how much of this it should read.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PerfEventAttr {
    pub kind: u32,
    pub size: u32,
    pub config: u64,
    pub sample_period: u64,
    pub sample_type: u64,
    pub read_format: u64,
    pub flags: u64,
    pub wakeup_events: u32,
    pub bp_type: u32,
    pub config1: u64,
    pub config2: u64,
    pub branch_sample_type: u64,
    pub sample_regs_user: u64,
    pub sample_stack_user: u32,
    pub clockid: i32,
    pub sample_regs_intr: u64,
    pub aux_watermark: u32,
    pub sample_max_stack: u16,
    pub reserved_2: u16,
    pub aux_sample_size: u32,
    pub reserved_3: u32,
    pub sig_data: u64,
    pub config3: u64,
}

/// The bit positions of the `perf_event_attr` flag bitfield.
pub mod flag {
    pub const DISABLED: u64 = 1 << 0;
    pub const INHERIT: u64 = 1 << 1;
    pub const MMAP: u64 = 1 << 8;
    pub const TASK: u64 = 1 << 13;
    pub const MMAP_DATA: u64 = 1 << 17;
    pub const SAMPLE_ID_ALL: u64 = 1 << 18;
    pub const EXCLUDE_CALLCHAIN_KERNEL: u64 = 1 << 21;
    pub const USE_CLOCKID: u64 = 1 << 25;
    pub const CONTEXT_SWITCH: u64 = 1 << 26;
}

pub const PERF_TYPE_SOFTWARE: u32 = 1;
pub const PERF_COUNT_SW_CPU_CLOCK: u64 = 0;
pub const PERF_COUNT_SW_DUMMY: u64 = 9;
const CLOCK_MONOTONIC: i32 = 1;

/// `kSampleRegsUserAll`: every user register, for DWARF unwinding.
#[cfg(target_arch = "x86_64")]
pub const SAMPLE_REGS_USER_ALL: u64 = 0x00FF0FFF;
#[cfg(target_arch = "aarch64")]
pub const SAMPLE_REGS_USER_ALL: u64 = 0x1_FFFF_FFFF;

use orbit_perf_records::reader::sample_bits;

fn max_stack() -> u16 {
    static MAX_STACK: OnceLock<u16> = OnceLock::new();
    *MAX_STACK.get_or_init(|| {
        std::fs::read_to_string("/proc/sys/kernel/perf_event_max_stack")
            .ok()
            .and_then(|contents| contents.trim().parse().ok())
            .unwrap_or(127)
    })
}

/// Twin of `generic_event_attr`.
fn generic_event_attr() -> PerfEventAttr {
    PerfEventAttr {
        size: std::mem::size_of::<PerfEventAttr>() as u32,
        sample_period: 1,
        flags: flag::USE_CLOCKID | flag::SAMPLE_ID_ALL | flag::DISABLED,
        clockid: CLOCK_MONOTONIC,
        sample_type: sample_bits::TID_TIME_STREAMID_CPU,
        ..PerfEventAttr::default()
    }
}

/// Twin of `context_switch_event_open`'s attr.
pub fn context_switch() -> PerfEventAttr {
    let mut attr = generic_event_attr();
    attr.kind = PERF_TYPE_SOFTWARE;
    attr.config = PERF_COUNT_SW_DUMMY;
    attr.flags |= flag::CONTEXT_SWITCH;
    attr
}

/// Twin of `mmap_task_event_open`'s attr.
pub fn mmap_task() -> PerfEventAttr {
    let mut attr = generic_event_attr();
    attr.kind = PERF_TYPE_SOFTWARE;
    attr.config = PERF_COUNT_SW_DUMMY;
    attr.flags |= flag::MMAP | flag::MMAP_DATA | flag::TASK;
    attr
}

/// Twin of `stack_sample_event_open`'s attr.
pub fn stack_sample(period_ns: u64, stack_dump_size: u16) -> PerfEventAttr {
    let mut attr = generic_event_attr();
    attr.kind = PERF_TYPE_SOFTWARE;
    attr.config = PERF_COUNT_SW_CPU_CLOCK;
    attr.sample_period = period_ns;
    attr.sample_type |= sample_bits::REGS_USER | sample_bits::STACK_USER;
    attr.sample_regs_user = SAMPLE_REGS_USER_ALL;
    attr.sample_stack_user = u32::from(stack_dump_size);
    attr
}

/// Twin of `callchain_sample_event_open`'s attr.
pub fn callchain_sample(period_ns: u64, stack_dump_size: u16) -> PerfEventAttr {
    let mut attr = stack_sample(period_ns, stack_dump_size);
    attr.sample_type |= sample_bits::CALLCHAIN;
    attr.sample_max_stack = max_stack();
    attr.flags |= flag::EXCLUDE_CALLCHAIN_KERNEL;
    attr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attr_is_ver8_sized() {
        assert_eq!(std::mem::size_of::<PerfEventAttr>(), 136);
    }

    #[test]
    fn regs_mask_popcount_matches_the_record_layout() {
        assert_eq!(
            SAMPLE_REGS_USER_ALL.count_ones() as usize,
            orbit_perf_records::reader::REGS_USER_ALL_COUNT
        );
    }
}
