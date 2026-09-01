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

/// The uprobe PMU's dynamic type number, from
/// `/sys/bus/event_source/devices/uprobe/type`.
///
/// Unlike `PERF_TYPE_SOFTWARE`, uprobes have no fixed type: the kernel
/// registers the PMU at boot and assigns it a number, so it has to be read
/// rather than hard-coded. A kernel built without `CONFIG_UPROBE_EVENTS` has
/// no such directory, which is the honest answer "this machine cannot do
/// dynamic instrumentation".
pub fn uprobe_pmu_type() -> Option<u32> {
    read_sysfs_u32("/sys/bus/event_source/devices/uprobe/type")
}

/// The bit of `config` that turns a uprobe into a uretprobe, from the PMU's
/// own format description (`config:0` on every kernel so far, but the file is
/// there precisely so it need not be assumed).
pub fn uprobe_retprobe_bit() -> Option<u32> {
    let text = std::fs::read_to_string("/sys/bus/event_source/devices/uprobe/format/retprobe").ok()?;
    parse_config_bit(&text)
}

fn read_sysfs_u32(path: &str) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// `config:0` -> 0. The format file may also name a range (`config:0-3`); the
/// low bit is the one that matters here.
fn parse_config_bit(text: &str) -> Option<u32> {
    let rest = text.trim().strip_prefix("config:")?;
    rest.split('-').next()?.parse().ok()
}

/// A uprobe's attributes together with the path buffer they point at.
///
/// `perf_event_attr.uprobe_path` (config1) is a pointer into *our* address
/// space that the kernel reads during the syscall, so the string has to
/// outlive the attr. Bundling them makes that impossible to get wrong: there
/// is no way to obtain the attr without also holding the path alive.
pub struct UprobeAttr {
    // Read by the kernel through the pointer stored in attr.config1.
    _path: std::ffi::CString,
    attr: PerfEventAttr,
}

impl UprobeAttr {
    /// A probe on `file_offset` bytes into the binary at `path`.
    ///
    /// The offset is a *file* offset, not a virtual address: the kernel finds
    /// the inode and places the breakpoint at that offset, so every process
    /// mapping the file can be probed by the same pair.
    pub fn new(path: &str, file_offset: u64, retprobe: bool) -> Result<UprobeAttr, String> {
        let kind = uprobe_pmu_type()
            .ok_or_else(|| "this kernel has no uprobe PMU (CONFIG_UPROBE_EVENTS)".to_string())?;
        let bit = uprobe_retprobe_bit()
            .ok_or_else(|| "the uprobe PMU does not describe its retprobe bit".to_string())?;
        let path = std::ffi::CString::new(path)
            .map_err(|_| "module path contains a NUL byte".to_string())?;
        let mut attr = generic_event_attr();
        attr.kind = kind;
        attr.config = if retprobe { 1u64 << bit } else { 0 };
        // config1 is uprobe_path, config2 is uprobe_offset.
        attr.config1 = path.as_ptr() as u64;
        attr.config2 = file_offset;
        // New threads of the target inherit the probe. Existing threads are
        // opened explicitly by the caller; between the two, every thread of
        // the process is covered.
        attr.flags |= flag::INHERIT;
        Ok(UprobeAttr { _path: path, attr })
    }

    pub fn attr(&self) -> &PerfEventAttr {
        &self.attr
    }
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

    #[test]
    fn a_format_bit_is_read_from_its_description() {
        assert_eq!(parse_config_bit("config:0\n"), Some(0));
        assert_eq!(parse_config_bit("config:5"), Some(5));
        // A range names its low bit.
        assert_eq!(parse_config_bit("config:2-4"), Some(2));
        assert_eq!(parse_config_bit("nonsense"), None);
    }

    #[test]
    fn a_uprobe_attr_points_at_a_path_it_keeps_alive() {
        if uprobe_pmu_type().is_none() {
            eprintln!("skipping: no uprobe PMU on this kernel");
            return;
        }
        let uprobe = UprobeAttr::new("/bin/true", 0x1234, false).unwrap();
        let attr = *uprobe.attr();
        assert_eq!(attr.config2, 0x1234);
        assert_ne!(attr.config1, 0, "config1 must point at the path");
        // The pointer must still name the path after the attr was copied out.
        // SAFETY: config1 is the CString owned by `uprobe`, still alive.
        let seen = unsafe { std::ffi::CStr::from_ptr(attr.config1 as *const libc::c_char) };
        assert_eq!(seen.to_str().unwrap(), "/bin/true");

        let ret = UprobeAttr::new("/bin/true", 0x1234, true).unwrap();
        assert_ne!(ret.attr().config, 0, "the retprobe bit distinguishes exit from entry");
    }
}
