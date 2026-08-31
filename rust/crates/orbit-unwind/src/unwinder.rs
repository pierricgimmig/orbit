// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The per-process unwinder: modules from the process's maps, frames from a
//! copied stack slice.

use crate::modules::load_module;
use framehop::{UnwindIterator, Unwinder};
use orbit_maps::{parse_maps, MemoryMapping, PROT_EXEC};
use std::collections::HashMap;

#[cfg(target_arch = "x86_64")]
type ArchUnwinder = framehop::x86_64::UnwinderX86_64<Vec<u8>>;
#[cfg(target_arch = "x86_64")]
type ArchCache = framehop::x86_64::CacheX86_64;
#[cfg(target_arch = "aarch64")]
type ArchUnwinder = framehop::aarch64::UnwinderAarch64<Vec<u8>>;
#[cfg(target_arch = "aarch64")]
type ArchCache = framehop::aarch64::CacheAarch64;

/// The registers unwinding starts from, in capture terms. On x86_64 `link`
/// is unused; on aarch64 it is LR.
#[derive(Clone, Copy, Debug)]
pub struct StartRegs {
    pub ip: u64,
    pub sp: u64,
    pub frame_pointer: u64,
    pub link: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnwindOutcome {
    /// Frame addresses, outermost last: `frames[0]` is the sampled ip, the
    /// rest are return addresses minus one -- the address INSIDE the call
    /// instruction, which is libunwindstack's `FrameData::pc` convention and
    /// what Orbit's symbolization consumes. (The differential caught this:
    /// raw return addresses diverged from the C++ by exactly one on every
    /// frame past the first.)
    pub frames: Vec<u64>,
    /// None on a clean walk to the root, the framehop error otherwise.
    pub error: Option<String>,
}

impl UnwindOutcome {
    pub fn is_success(&self) -> bool {
        self.error.is_none()
    }
}

pub struct ProcessUnwinder {
    unwinder: ArchUnwinder,
    cache: ArchCache,
    modules_loaded: usize,
}

/// One mapped file's aggregate: bias and address span.
#[derive(Debug)]
struct FileSpan {
    base_avma: u64,
    start: u64,
    end: u64,
    executable: bool,
}

impl ProcessUnwinder {
    /// Builds an unwinder from the content of a `/proc/<pid>/maps`.
    pub fn from_maps_content(maps_content: &[u8]) -> ProcessUnwinder {
        let mappings = parse_maps(maps_content);
        let mut files: HashMap<Vec<u8>, FileSpan> = HashMap::new();
        for mapping in &mappings {
            if !is_file_backed(mapping) {
                continue;
            }
            let bias = mapping.start_address.wrapping_sub(mapping.offset);
            let entry = files.entry(mapping.pathname.clone()).or_insert(FileSpan {
                base_avma: bias,
                start: mapping.start_address,
                end: mapping.end_address,
                executable: false,
            });
            entry.base_avma = entry.base_avma.min(bias);
            entry.start = entry.start.min(mapping.start_address);
            entry.end = entry.end.max(mapping.end_address);
            entry.executable |= mapping.perms & PROT_EXEC != 0;
        }

        let mut unwinder = ArchUnwinder::new();
        let mut modules_loaded = 0;
        for (path, span) in &files {
            if !span.executable {
                continue;
            }
            let Ok(path) = std::str::from_utf8(path) else { continue };
            if let Some(module) = load_module(path, span.start..span.end, span.base_avma) {
                unwinder.add_module(module);
                modules_loaded += 1;
            }
        }
        ProcessUnwinder { unwinder, cache: ArchCache::new(), modules_loaded }
    }

    pub fn for_pid(pid: i32) -> std::io::Result<ProcessUnwinder> {
        let content = std::fs::read(format!("/proc/{pid}/maps"))?;
        Ok(Self::from_maps_content(&content))
    }

    pub fn modules_loaded(&self) -> usize {
        self.modules_loaded
    }

    /// Unwinds from `regs` over the copied stack slice starting at
    /// `stack_base` (the captured stack pointer). Reads outside the slice
    /// fail, like `offline_memory_only` unwinding in the C++.
    pub fn unwind(
        &mut self,
        regs: StartRegs,
        stack_base: u64,
        stack: &[u8],
        max_frames: usize,
    ) -> UnwindOutcome {
        let mut read_stack = |address: u64| -> Result<u64, ()> {
            let offset = address.checked_sub(stack_base).ok_or(())? as usize;
            let bytes = stack.get(offset..offset.checked_add(8).ok_or(())?).ok_or(())?;
            Ok(u64::from_le_bytes(bytes.try_into().expect("8 bytes")))
        };

        #[cfg(target_arch = "x86_64")]
        let arch_regs =
            framehop::x86_64::UnwindRegsX86_64::new(regs.ip, regs.sp, regs.frame_pointer);
        #[cfg(target_arch = "aarch64")]
        let arch_regs =
            framehop::aarch64::UnwindRegsAarch64::new(regs.link, regs.sp, regs.frame_pointer);

        let mut iterator =
            UnwindIterator::new(&self.unwinder, regs.ip, arch_regs, &mut self.cache, &mut read_stack);
        let mut outcome = UnwindOutcome { frames: Vec::new(), error: None };
        loop {
            match iterator.next() {
                Ok(Some(frame)) => {
                    outcome.frames.push(frame.address_for_lookup());
                    if outcome.frames.len() >= max_frames {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    outcome.error = Some(error.to_string());
                    break;
                }
            }
        }
        outcome
    }
}

fn is_file_backed(mapping: &MemoryMapping) -> bool {
    mapping.inode != 0 && mapping.pathname.first() == Some(&b'/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_perf_records::reader::{parse_record_sample, SampleFlags};
    use orbit_perf_records::{record_type, PerfEventHeader};

    // Deep, non-inlinable recursion so samples land under a real call chain.
    #[inline(never)]
    fn burn(depth: u32, spin: u64) -> u64 {
        let mut acc = std::hint::black_box(spin);
        if depth > 0 {
            for j in 0..(1u64 << 12) {
                acc = acc.wrapping_add(j * j);
            }
            acc = acc.wrapping_add(burn(depth - 1, acc));
        } else {
            for j in 0..(1u64 << 18) {
                acc = acc.wrapping_add(j * j);
            }
        }
        std::hint::black_box(acc)
    }

    // Live end-to-end: sample this thread with full regs + stack, unwind
    // each sample with framehop over the stack copy, and require deep,
    // mostly-successful unwinds. Skips where perf_event_open is denied.
    #[test]
    fn unwinds_own_samples_deeply() {
        #[allow(unsafe_code)]
        let tid = unsafe { libc::gettid() };
        let mut ring = match orbit_perf_ring::ring::open_stack_sample(200_000, 64000, tid, -1, 8192)
        {
            Ok(ring) => ring,
            Err(error) => {
                eprintln!("skipping: perf_event_open not permitted here ({error})");
                return;
            }
        };
        ring.enable().unwrap();
        // Interleave work and draining so the ring never overflows while
        // still accumulating a few hundred milliseconds of samples.
        let mut records = Vec::new();
        for round in 0..200u64 {
            burn(24, round);
            while let Some(record) = ring.read_record().unwrap() {
                records.push(record);
            }
        }

        let mut unwinder = ProcessUnwinder::for_pid(std::process::id() as i32).unwrap();
        assert!(unwinder.modules_loaded() > 0, "no modules loaded from own maps");

        let mut errors: std::collections::HashMap<(String, usize), u64> = Default::default();
        let mut samples = 0u64;
        let mut successes = 0u64;
        let mut deep = 0u64;
        for record in records {
            let header = PerfEventHeader::parse(&record).unwrap();
            if { header.kind } != record_type::SAMPLE {
                continue;
            }
            let sample = parse_record_sample(&record, SampleFlags::stack_sample(), true).unwrap();
            let Some(regs) = sample.regs.as_deref() else { continue };
            let Some(stack) = sample.stack_data.as_deref() else { continue };
            // kSampleRegsUserAll order on x86_64: ax,bx,cx,dx,si,di,bp,sp,ip,..
            #[cfg(target_arch = "x86_64")]
            let start = StartRegs { ip: regs[8], sp: regs[7], frame_pointer: regs[6], link: 0 };
            #[cfg(target_arch = "aarch64")]
            let start = StartRegs { ip: regs[32], sp: regs[31], frame_pointer: regs[29], link: regs[30] };

            samples += 1;
            let outcome = unwinder.unwind(start, start.sp, stack, 256);
            if outcome.is_success() {
                successes += 1;
            } else if let Some(error) = &outcome.error {
                *errors.entry((error.clone(), outcome.frames.len().min(3))).or_insert(0u64) += 1;
            }
            if outcome.frames.len() >= 10 {
                deep += 1;
            }
        }

        let mut error_list: Vec<_> = errors.into_iter().collect();
        error_list.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        for ((error, frames), count) in error_list.iter().take(6) {
            eprintln!("error {count}x at >={frames} frames: {error}");
        }
        assert!(samples > 10, "only {samples} samples");
        assert!(
            successes * 10 >= samples * 5,
            "success rate too low: {successes}/{samples}"
        );
        assert!(deep > 0, "no sample unwound 10+ frames ({samples} samples)");
        eprintln!("samples={samples} successes={successes} deep={deep}");
    }
}
