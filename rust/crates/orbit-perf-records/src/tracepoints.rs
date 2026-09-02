// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The scheduling tracepoint payloads, ported from
//! `src/LinuxTracing/KernelTracepoints.h`.
//!
//! A tracepoint sample's `RAW` payload is the tracepoint's own fields, laid
//! out exactly as its `format` file in tracefs describes. The C++ hard-codes
//! those layouts with `static_assert`s on the sizes rather than parsing the
//! format file at runtime, and this does the same, offset for offset -- the
//! offsets are ABI in practice, and a mismatch is the sort of thing the size
//! assertions catch at compile time rather than in the field.
//!
//! Reading is by explicit offset rather than by casting a packed struct, so
//! nothing here needs `unsafe` and a short payload returns `None` instead of
//! reading past the end.

/// Every tracepoint payload starts with these 8 bytes.
pub const COMMON_LEN: usize = 8;

fn i32_at(bytes: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
}

fn i64_at(bytes: &[u8], offset: usize) -> Option<i64> {
    Some(i64::from_le_bytes(bytes.get(offset..offset + 8)?.try_into().ok()?))
}

/// A fixed-width `char[16]` comm field, trimmed at the first NUL.
fn comm_at(bytes: &[u8], offset: usize) -> Option<String> {
    let raw = bytes.get(offset..offset + 16)?;
    let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
    Some(String::from_utf8_lossy(&raw[..end]).into_owned())
}

/// `sched:sched_switch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedSwitch {
    pub prev_comm: String,
    pub prev_tid: i32,
    /// The task state bits; see [`thread_state_from_bits`].
    pub prev_state: i64,
    pub next_comm: String,
    pub next_tid: i32,
}

impl SchedSwitch {
    /// Layout: common(8), prev_comm[16], prev_pid(4), prev_prio(4),
    /// prev_state(8), next_comm[16], next_pid(4), next_prio(4), pad(4) = 68.
    pub const LEN: usize = 68;

    pub fn parse(payload: &[u8]) -> Option<SchedSwitch> {
        if payload.len() < 60 {
            // The trailing 4 bytes are padding the format file does not
            // document, so they are not required to be present.
            return None;
        }
        Some(SchedSwitch {
            prev_comm: comm_at(payload, 8)?,
            prev_tid: i32_at(payload, 24)?,
            prev_state: i64_at(payload, 32)?,
            next_comm: comm_at(payload, 40)?,
            next_tid: i32_at(payload, 56)?,
        })
    }
}

/// `sched:sched_wakeup`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedWakeup {
    pub comm: String,
    pub tid: i32,
}

impl SchedWakeup {
    /// Only the fixed head is read. Kernel v5.14 removed the `success` field
    /// (torvalds/linux 58b9987de86c) and the padding with it, so everything
    /// after `prio` is version-dependent and none of it is needed here.
    pub const FIXED_LEN: usize = 32;

    pub fn parse(payload: &[u8]) -> Option<SchedWakeup> {
        if payload.len() < 28 {
            return None;
        }
        Some(SchedWakeup { comm: comm_at(payload, 8)?, tid: i32_at(payload, 24)? })
    }
}

/// `task:task_newtask`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskNewtask {
    pub tid: i32,
    pub comm: String,
}

impl TaskNewtask {
    /// Layout: common(8), pid(4), comm[16], clone_flags(8), oom_score_adj(2),
    /// pad(14) = 52.
    pub const LEN: usize = 52;

    pub fn parse(payload: &[u8]) -> Option<TaskNewtask> {
        if payload.len() < 28 {
            return None;
        }
        Some(TaskNewtask { tid: i32_at(payload, 8)?, comm: comm_at(payload, 12)? })
    }
}

/// `ThreadStateSlice::ThreadState`, matching the proto's numeric values and
/// `orbit_thread_states::state`.
pub mod thread_state {
    pub const RUNNING: i32 = 0;
    pub const RUNNABLE: i32 = 1;
    pub const INTERRUPTIBLE_SLEEP: i32 = 2;
    pub const UNINTERRUPTIBLE_SLEEP: i32 = 3;
    pub const STOPPED: i32 = 4;
    pub const TRACED: i32 = 5;
    pub const DEAD: i32 = 6;
    pub const ZOMBIE: i32 = 7;
    pub const PARKED: i32 = 8;
    pub const IDLE: i32 = 9;
}

/// The state a `prev_state` mask means, twin of
/// `SwitchesStatesNamesVisitor::GetThreadStateFromBits`.
///
/// The order is the C++'s and the kernel's: the mask can in principle have
/// more than one bit set, and the first match wins. Zero means the thread was
/// preempted while still runnable, which is the common case and the reason
/// this is not simply a table lookup.
pub fn thread_state_from_bits(bits: i64) -> i32 {
    let bits = bits as u64;
    if bits & 0x01 != 0 {
        return thread_state::INTERRUPTIBLE_SLEEP;
    }
    if bits & 0x02 != 0 {
        return thread_state::UNINTERRUPTIBLE_SLEEP;
    }
    if bits & 0x04 != 0 {
        return thread_state::STOPPED;
    }
    if bits & 0x08 != 0 {
        return thread_state::TRACED;
    }
    if bits & 0x10 != 0 {
        return thread_state::DEAD;
    }
    if bits & 0x20 != 0 {
        return thread_state::ZOMBIE;
    }
    if bits & 0x40 != 0 {
        return thread_state::PARKED;
    }
    if bits & 0x80 != 0 {
        return thread_state::IDLE;
    }
    thread_state::RUNNABLE
}

/// True when the mask names more than one state, which the C++ logs as an
/// error before reporting only the first.
pub fn is_combined_state(bits: i64) -> bool {
    (bits as u64 & 0xFF).count_ones() > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched_switch_payload(prev_tid: i32, prev_state: i64, next_tid: i32) -> Vec<u8> {
        let mut payload = vec![0u8; SchedSwitch::LEN];
        payload[8..12].copy_from_slice(b"prev");
        payload[24..28].copy_from_slice(&prev_tid.to_le_bytes());
        payload[32..40].copy_from_slice(&prev_state.to_le_bytes());
        payload[40..44].copy_from_slice(b"next");
        payload[56..60].copy_from_slice(&next_tid.to_le_bytes());
        payload
    }

    #[test]
    fn a_sched_switch_payload_yields_both_threads_and_the_state() {
        let parsed = SchedSwitch::parse(&sched_switch_payload(1234, 0x01, 5678)).unwrap();
        assert_eq!(parsed.prev_tid, 1234);
        assert_eq!(parsed.next_tid, 5678);
        assert_eq!(parsed.prev_state, 0x01);
        assert_eq!(parsed.prev_comm, "prev");
        assert_eq!(parsed.next_comm, "next");
    }

    #[test]
    fn a_payload_missing_its_documented_tail_still_parses() {
        // The last 4 bytes are undocumented padding; a kernel that omits them
        // must not cost us the record.
        let mut payload = sched_switch_payload(7, 0, 9);
        payload.truncate(60);
        assert_eq!(SchedSwitch::parse(&payload).unwrap().next_tid, 9);
    }

    #[test]
    fn a_truncated_payload_is_none_not_a_panic() {
        assert!(SchedSwitch::parse(&[0u8; 8]).is_none());
        assert!(SchedWakeup::parse(&[]).is_none());
        assert!(TaskNewtask::parse(&[0u8; 4]).is_none());
    }

    #[test]
    fn a_comm_is_trimmed_at_its_nul() {
        let mut payload = vec![0u8; SchedSwitch::LEN];
        payload[8..16].copy_from_slice(b"physics\0");
        assert_eq!(SchedSwitch::parse(&payload).unwrap().prev_comm, "physics");
    }

    #[test]
    fn a_comm_filling_all_sixteen_bytes_is_not_truncated() {
        let mut payload = vec![0u8; SchedSwitch::LEN];
        payload[8..24].copy_from_slice(b"0123456789abcdef");
        assert_eq!(SchedSwitch::parse(&payload).unwrap().prev_comm, "0123456789abcdef");
    }

    #[test]
    fn state_bits_map_the_way_the_kernel_prints_them() {
        assert_eq!(thread_state_from_bits(0x00), thread_state::RUNNABLE);
        assert_eq!(thread_state_from_bits(0x01), thread_state::INTERRUPTIBLE_SLEEP);
        assert_eq!(thread_state_from_bits(0x02), thread_state::UNINTERRUPTIBLE_SLEEP);
        assert_eq!(thread_state_from_bits(0x04), thread_state::STOPPED);
        assert_eq!(thread_state_from_bits(0x08), thread_state::TRACED);
        assert_eq!(thread_state_from_bits(0x10), thread_state::DEAD);
        assert_eq!(thread_state_from_bits(0x20), thread_state::ZOMBIE);
        assert_eq!(thread_state_from_bits(0x40), thread_state::PARKED);
        assert_eq!(thread_state_from_bits(0x80), thread_state::IDLE);
    }

    #[test]
    fn a_combined_mask_reports_the_first_state_and_is_flagged() {
        // The C++ logs an error and reports only the first; same order here.
        let bits = 0x01 | 0x02;
        assert!(is_combined_state(bits));
        assert_eq!(thread_state_from_bits(bits), thread_state::INTERRUPTIBLE_SLEEP);
        assert!(!is_combined_state(0x01));
        assert!(!is_combined_state(0x00));
    }

    #[test]
    fn high_bits_above_the_state_mask_are_ignored() {
        // Kernels set bits above 0xFF for things like TASK_NEW; they must not
        // be mistaken for a state.
        assert_eq!(thread_state_from_bits(0x1000), thread_state::RUNNABLE);
        assert!(!is_combined_state(0x1001));
    }
}
