// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The classic kernel events, decoded from their raw bytes.
//!
//! Each event is identified by its provider GUID and opcode, and its payload
//! is a fixed MOF structure (variable-length strings last). The layouts here
//! are the 64-bit ones; a 32-bit kernel is not something the service runs on.
//! Field offsets are from the public MOF definitions (`Thread_V2_TypeGroup1`,
//! `CSwitch`, `ReadyThread`, `Image_Load`, `Process_TypeGroup1`,
//! `StackWalk_Event`, `SampledProfile`), the same ones the C++ tracer's
//! `EventTypes.h` transcribed in 2021.
//!
//! Decoding is total: a short payload yields `None`, never a read past the
//! end, and an unknown event yields [`KernelEvent::Other`] so the consumer
//! can count what it ignored.

/// `Thread` provider: 3d6fa8d1-fe05-11d0-9dda-00c04fd7ba7c.
pub const THREAD_GUID: [u8; 16] =
    guid(0x3d6fa8d1, 0xfe05, 0x11d0, [0x9d, 0xda, 0x00, 0xc0, 0x4f, 0xd7, 0xba, 0x7c]);
/// `Process` provider: 3d6fa8d0-fe05-11d0-9dda-00c04fd7ba7c.
pub const PROCESS_GUID: [u8; 16] =
    guid(0x3d6fa8d0, 0xfe05, 0x11d0, [0x9d, 0xda, 0x00, 0xc0, 0x4f, 0xd7, 0xba, 0x7c]);
/// `Image` provider: 2cb15d1d-5fc1-11d2-abe1-00a0c911f518.
pub const IMAGE_GUID: [u8; 16] =
    guid(0x2cb15d1d, 0x5fc1, 0x11d2, [0xab, 0xe1, 0x00, 0xa0, 0xc9, 0x11, 0xf5, 0x18]);
/// `PerfInfo` provider: ce1dbfb4-137e-4da6-87b0-3f59aa102cbc.
pub const PERF_INFO_GUID: [u8; 16] =
    guid(0xce1dbfb4, 0x137e, 0x4da6, [0x87, 0xb0, 0x3f, 0x59, 0xaa, 0x10, 0x2c, 0xbc]);
/// `StackWalk` provider: def2fe46-7bd6-4b80-bd94-f57fe20d0ce3.
pub const STACK_WALK_GUID: [u8; 16] =
    guid(0xdef2fe46, 0x7bd6, 0x4b80, [0xbd, 0x94, 0xf5, 0x7f, 0xe2, 0x0d, 0x0c, 0xe3]);
/// The session itself: `SystemTraceControlGuid`, 9e814aad-3204-11d2-9a82-006008a86939.
pub const SYSTEM_TRACE_CONTROL_GUID: [u8; 16] =
    guid(0x9e814aad, 0x3204, 0x11d2, [0x9a, 0x82, 0x00, 0x60, 0x08, 0xa8, 0x69, 0x39]);
/// `EventTraceGuid`, the session header event: 68fdd900-4a3e-11d1-84f4-0000f80464e3.
pub const EVENT_TRACE_GUID: [u8; 16] =
    guid(0x68fdd900, 0x4a3e, 0x11d1, [0x84, 0xf4, 0x00, 0x00, 0xf8, 0x04, 0x64, 0xe3]);

/// A GUID in its in-memory (mixed-endian) byte layout, as `EVENT_RECORD`
/// carries it.
pub const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let c = c.to_le_bytes();
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
}

/// Opcodes, per provider.
pub mod opcode {
    /// `Thread`/`Process`/`Image` lifecycle: Start, End, DCStart, DCEnd.
    pub const START: u8 = 1;
    pub const END: u8 = 2;
    pub const DC_START: u8 = 3;
    pub const DC_END: u8 = 4;
    /// `Image`: a load during the session (the C++ constants had 10).
    pub const IMAGE_LOAD: u8 = 10;
    /// `Thread`: a context switch.
    pub const CSWITCH: u8 = 36;
    /// `Thread`: a thread made ready to run.
    pub const READY_THREAD: u8 = 50;
    /// `Thread`: a thread named (Windows 10 1607+).
    pub const THREAD_SET_NAME: u8 = 72;
    /// `PerfInfo`: a sampled-profile tick.
    pub const SAMPLED_PROFILE: u8 = 46;
    /// `StackWalk`: the frames of the event that preceded it.
    pub const STACK: u8 = 32;
}

/// `KTHREAD_STATE`, the state a switched-out thread goes into
/// (`CSwitch.OldThreadState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i8)]
pub enum ThreadState {
    Initialized = 0,
    Ready = 1,
    Running = 2,
    Standby = 3,
    Terminated = 4,
    Waiting = 5,
    Transition = 6,
    DeferredReady = 7,
    GateWait = 8,
}

impl ThreadState {
    pub fn from_raw(raw: i8) -> Option<ThreadState> {
        Some(match raw {
            0 => ThreadState::Initialized,
            1 => ThreadState::Ready,
            2 => ThreadState::Running,
            3 => ThreadState::Standby,
            4 => ThreadState::Terminated,
            5 => ThreadState::Waiting,
            6 => ThreadState::Transition,
            7 => ThreadState::DeferredReady,
            8 => ThreadState::GateWait,
            _ => return None,
        })
    }
}

/// `KWAIT_REASON` values that say what a waiting thread waits for; only the
/// ones the state mapping distinguishes.
pub mod wait_reason {
    pub const DELAY_EXECUTION: i8 = 4;
    pub const SUSPENDED: i8 = 5;
    pub const WR_DELAY_EXECUTION: i8 = 20;
    pub const WR_SUSPENDED: i8 = 21;
    /// Page-in style waits: FreePage, PageIn, VirtualMemory, PageOut and
    /// their `Wr` twins (1, 2, 8, 9, 10, 11, 24, 25).
    pub fn is_memory(reason: i8) -> bool {
        matches!(reason, 1 | 2 | 8 | 9 | 10 | 11 | 24 | 25)
    }
}

/// One decoded event. Timestamps are the raw counter the header carried;
/// the consumer converts them with [`crate::QpcClock`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelEvent {
    /// A context switch on `cpu`: `old_tid` leaves (into `old_state`),
    /// `new_tid` takes the core.
    ContextSwitch {
        timestamp: i64,
        cpu: u16,
        new_tid: u32,
        old_tid: u32,
        old_state: Option<ThreadState>,
        old_wait_reason: i8,
    },
    /// `tid` was made ready by the thread this event was logged on
    /// (`by_tid` of `by_pid`): the wakeup.
    ReadyThread { timestamp: i64, tid: u32, by_tid: u32, by_pid: u32 },
    /// A thread exists (`rundown` for the DCStart enumeration at session
    /// start) or was just created (`by_tid`/`by_pid` is its creator).
    ThreadStart { timestamp: i64, pid: u32, tid: u32, rundown: bool, by_tid: u32, by_pid: u32, name: String },
    ThreadEnd { timestamp: i64, pid: u32, tid: u32, rundown: bool },
    ThreadName { timestamp: i64, pid: u32, tid: u32, name: String },
    ProcessStart { timestamp: i64, pid: u32, parent_pid: u32, rundown: bool, image_name: String },
    ProcessEnd { timestamp: i64, pid: u32, rundown: bool },
    /// An image mapped in `pid` at `base` for `size` bytes; `path` is an NT
    /// device path (`\Device\HarddiskVolume3\...`).
    ImageLoad { timestamp: i64, pid: u32, base: u64, size: u64, path: String, rundown: bool },
    ImageUnload { timestamp: i64, pid: u32, base: u64 },
    /// The profiler's tick on `tid`, at `ip`. The matching [`KernelEvent::Stack`]
    /// carries this event's `timestamp` as its `event_timestamp`.
    SampledProfile { timestamp: i64, tid: u32, ip: u64 },
    /// The frames of the event that fired at `event_timestamp` on `tid` of
    /// `pid`, innermost first, kernel and user addresses mixed.
    Stack { timestamp: i64, event_timestamp: i64, pid: u32, tid: u32, frames: Vec<u64> },
    /// Anything else, counted and dropped.
    Other,
}

/// The parts of `EVENT_RECORD` the decoder needs, so it can be driven from
/// bytes in a test without an `EVENT_RECORD` at all.
#[derive(Clone, Copy, Debug)]
pub struct RawEvent<'a> {
    pub provider: &'a [u8; 16],
    pub opcode: u8,
    pub version: u8,
    pub timestamp: i64,
    pub cpu: u16,
    /// The thread and process the event was logged on.
    pub header_tid: u32,
    pub header_pid: u32,
    pub payload: &'a [u8],
}

/// Decodes one event. Pure: the whole classic-kernel vocabulary, in one match.
pub fn decode(event: &RawEvent) -> KernelEvent {
    let p = event.payload;
    let ts = event.timestamp;
    match (*event.provider, event.opcode) {
        (THREAD_GUID, opcode::CSWITCH) => {
            if p.len() < 16 {
                return KernelEvent::Other;
            }
            KernelEvent::ContextSwitch {
                timestamp: ts,
                cpu: event.cpu,
                new_tid: u32_at(p, 0),
                old_tid: u32_at(p, 4),
                old_state: ThreadState::from_raw(p[14] as i8),
                old_wait_reason: p[12] as i8,
            }
        }
        (THREAD_GUID, opcode::READY_THREAD) => {
            if p.len() < 4 {
                return KernelEvent::Other;
            }
            KernelEvent::ReadyThread {
                timestamp: ts,
                tid: u32_at(p, 0),
                by_tid: event.header_tid,
                by_pid: event.header_pid,
            }
        }
        (THREAD_GUID, opcode::START | opcode::DC_START) => {
            if p.len() < 8 {
                return KernelEvent::Other;
            }
            // Thread_V3_TypeGroup1 is 72 bytes; V4 appends the name.
            let name = if event.version >= 4 && p.len() > 72 { wide_string_at(p, 72) } else { String::new() };
            KernelEvent::ThreadStart {
                timestamp: ts,
                pid: u32_at(p, 0),
                tid: u32_at(p, 4),
                rundown: event.opcode == opcode::DC_START,
                by_tid: event.header_tid,
                by_pid: event.header_pid,
                name,
            }
        }
        (THREAD_GUID, opcode::END | opcode::DC_END) => {
            if p.len() < 8 {
                return KernelEvent::Other;
            }
            KernelEvent::ThreadEnd {
                timestamp: ts,
                pid: u32_at(p, 0),
                tid: u32_at(p, 4),
                rundown: event.opcode == opcode::DC_END,
            }
        }
        (THREAD_GUID, opcode::THREAD_SET_NAME) => {
            // ProcessId u32, ThreadId u32, ThreadName wide.
            if p.len() < 8 {
                return KernelEvent::Other;
            }
            KernelEvent::ThreadName { timestamp: ts, pid: u32_at(p, 0), tid: u32_at(p, 4), name: wide_string_at(p, 8) }
        }
        (PROCESS_GUID, opcode::START | opcode::DC_START) => {
            // UniqueProcessKey ptr, ProcessId, ParentId, SessionId,
            // ExitStatus, DirectoryTableBase ptr, (v4+: Flags u32), UserSID
            // (variable), ImageFileName ansi, CommandLine wide.
            if p.len() < 32 {
                return KernelEvent::Other;
            }
            let after_fixed = if event.version >= 4 { 36 } else { 32 };
            let image_name = sid_then_ansi(p, after_fixed).unwrap_or_default();
            KernelEvent::ProcessStart {
                timestamp: ts,
                pid: u32_at(p, 8),
                parent_pid: u32_at(p, 12),
                rundown: event.opcode == opcode::DC_START,
                image_name,
            }
        }
        (PROCESS_GUID, opcode::END | opcode::DC_END) => {
            if p.len() < 12 {
                return KernelEvent::Other;
            }
            KernelEvent::ProcessEnd { timestamp: ts, pid: u32_at(p, 8), rundown: event.opcode == opcode::DC_END }
        }
        (IMAGE_GUID, opcode::IMAGE_LOAD | opcode::DC_START) => {
            // ImageBase ptr, ImageSize ptr, ProcessId, ImageChecksum,
            // TimeDateStamp, 4 reserved bytes, DefaultBase ptr, 4 reserved
            // u32, FileName wide: the name starts at 56 in both v2 and v3.
            if p.len() < 56 {
                return KernelEvent::Other;
            }
            KernelEvent::ImageLoad {
                timestamp: ts,
                pid: u32_at(p, 16),
                base: u64_at(p, 0),
                size: u64_at(p, 8),
                path: wide_string_at(p, 56),
                rundown: event.opcode == opcode::DC_START,
            }
        }
        (IMAGE_GUID, opcode::END) => {
            if p.len() < 20 {
                return KernelEvent::Other;
            }
            KernelEvent::ImageUnload { timestamp: ts, pid: u32_at(p, 16), base: u64_at(p, 0) }
        }
        (PERF_INFO_GUID, opcode::SAMPLED_PROFILE) => {
            if p.len() < 12 {
                return KernelEvent::Other;
            }
            KernelEvent::SampledProfile { timestamp: ts, tid: u32_at(p, 8), ip: u64_at(p, 0) }
        }
        (STACK_WALK_GUID, opcode::STACK) => {
            // EventTimeStamp u64, StackProcess u32, StackThread u32, then
            // whole pointers; a trailing partial frame is dropped rather
            // than aborting on it, as the C++ did.
            if p.len() < 16 {
                return KernelEvent::Other;
            }
            let frames = p[16..].chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();
            KernelEvent::Stack {
                timestamp: ts,
                event_timestamp: u64_at(p, 0) as i64,
                pid: u32_at(p, 8),
                tid: u32_at(p, 12),
                frames,
            }
        }
        _ => KernelEvent::Other,
    }
}

/// Whether a 64-bit address is a kernel-mode one: the canonical upper half.
#[inline]
pub fn is_kernel_address(address: u64) -> bool {
    address >= 0xFFFF_8000_0000_0000
}

#[inline]
fn u32_at(p: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(p[at..at + 4].try_into().unwrap())
}

#[inline]
fn u64_at(p: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(p[at..at + 8].try_into().unwrap())
}

/// A NUL-terminated UTF-16 string at `at`; empty when out of range.
fn wide_string_at(p: &[u8], at: usize) -> String {
    if at >= p.len() {
        return String::new();
    }
    let units: Vec<u16> = p[at..]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Skips the `UserSID` field of a process event and reads the ANSI image
/// name after it. The field is a `TOKEN_USER`-shaped blob: a pointer-sized
/// `Sid` that is zero when there is no SID (then the field is 8 bytes), else
/// two pointers followed by the SID itself (`8 + 4 * SubAuthorityCount`).
fn sid_then_ansi(p: &[u8], at: usize) -> Option<String> {
    if p.len() < at + 8 {
        return None;
    }
    let sid_ptr = u64_at(p, at);
    let name_at = if sid_ptr == 0 {
        at + 8
    } else {
        let sid = at + 16;
        if p.len() < sid + 8 {
            return None;
        }
        let sub_authorities = p[sid + 1] as usize;
        sid + 8 + 4 * sub_authorities
    };
    if name_at > p.len() {
        return None;
    }
    let end = p[name_at..].iter().position(|&b| b == 0).map(|n| name_at + n).unwrap_or(p.len());
    Some(String::from_utf8_lossy(&p[name_at..end]).into_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn raw<'a>(provider: &'a [u8; 16], opcode: u8, version: u8, payload: &'a [u8]) -> RawEvent<'a> {
        RawEvent { provider, opcode, version, timestamp: 1000, cpu: 3, header_tid: 77, header_pid: 70, payload }
    }

    fn wide(s: &str) -> Vec<u8> {
        s.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
    }

    pub fn cswitch_bytes(new_tid: u32, old_tid: u32, old_state: i8, wait_reason: i8) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&new_tid.to_le_bytes());
        p.extend_from_slice(&old_tid.to_le_bytes());
        p.extend_from_slice(&[8, 8, 0, 0]); // priorities, cstate, spare
        p.extend_from_slice(&[wait_reason as u8, 1, old_state as u8, 0]); // reason, mode, state, ideal cpu
        p.extend_from_slice(&0u32.to_le_bytes()); // NewThreadWaitTime
        p.extend_from_slice(&0u32.to_le_bytes()); // Reserved
        p
    }

    #[test]
    fn guid_bytes_match_the_registry_form() {
        // {3d6fa8d1-fe05-11d0-9dda-00c04fd7ba7c} as Windows stores it.
        assert_eq!(
            THREAD_GUID,
            [0xd1, 0xa8, 0x6f, 0x3d, 0x05, 0xfe, 0xd0, 0x11, 0x9d, 0xda, 0x00, 0xc0, 0x4f, 0xd7, 0xba, 0x7c]
        );
    }

    #[test]
    fn a_context_switch_decodes_both_threads_and_the_old_state() {
        let bytes = cswitch_bytes(4321, 1234, 5, 6);
        assert_eq!(bytes.len(), 24);
        let event = decode(&raw(&THREAD_GUID, opcode::CSWITCH, 4, &bytes));
        assert_eq!(
            event,
            KernelEvent::ContextSwitch {
                timestamp: 1000,
                cpu: 3,
                new_tid: 4321,
                old_tid: 1234,
                old_state: Some(ThreadState::Waiting),
                old_wait_reason: 6,
            }
        );
        // A version TDH never heard of decodes just the same: that is the point.
        let event = decode(&raw(&THREAD_GUID, opcode::CSWITCH, 9, &bytes));
        assert!(matches!(event, KernelEvent::ContextSwitch { new_tid: 4321, .. }));
    }

    #[test]
    fn a_short_context_switch_is_dropped_not_read_past_the_end() {
        assert_eq!(decode(&raw(&THREAD_GUID, opcode::CSWITCH, 4, &[1, 0, 0, 0, 2, 0, 0, 0])), KernelEvent::Other);
    }

    #[test]
    fn a_ready_thread_names_the_waker_from_the_header() {
        let event = decode(&raw(&THREAD_GUID, opcode::READY_THREAD, 2, &[0x10, 0x27, 0, 0, 0, 0, 0, 0]));
        assert_eq!(event, KernelEvent::ReadyThread { timestamp: 1000, tid: 10000, by_tid: 77, by_pid: 70 });
    }

    #[test]
    fn thread_start_and_rundown_carry_pid_tid_and_a_v4_name() {
        let mut p = vec![0u8; 72];
        p[0..4].copy_from_slice(&500u32.to_le_bytes());
        p[4..8].copy_from_slice(&504u32.to_le_bytes());
        let event = decode(&raw(&THREAD_GUID, opcode::DC_START, 3, &p));
        assert_eq!(
            event,
            KernelEvent::ThreadStart { timestamp: 1000, pid: 500, tid: 504, rundown: true, by_tid: 77, by_pid: 70, name: String::new() }
        );
        p.extend(wide("physics worker"));
        let event = decode(&raw(&THREAD_GUID, opcode::START, 4, &p));
        assert_eq!(
            event,
            KernelEvent::ThreadStart { timestamp: 1000, pid: 500, tid: 504, rundown: false, by_tid: 77, by_pid: 70, name: "physics worker".into() }
        );
        let event = decode(&raw(&THREAD_GUID, opcode::END, 4, &p));
        assert_eq!(event, KernelEvent::ThreadEnd { timestamp: 1000, pid: 500, tid: 504, rundown: false });
    }

    #[test]
    fn a_thread_set_name_event_decodes() {
        let mut p = Vec::new();
        p.extend_from_slice(&500u32.to_le_bytes());
        p.extend_from_slice(&504u32.to_le_bytes());
        p.extend(wide("render"));
        let event = decode(&raw(&THREAD_GUID, opcode::THREAD_SET_NAME, 2, &p));
        assert_eq!(event, KernelEvent::ThreadName { timestamp: 1000, pid: 500, tid: 504, name: "render".into() });
    }

    #[test]
    fn a_process_event_skips_the_sid_to_reach_the_image_name() {
        // v4: key, pid, ppid, session, exit, dtb, flags, then a SID with two
        // sub-authorities, then "game.exe\0", then a command line.
        let mut p = vec![0u8; 36];
        p[8..12].copy_from_slice(&4242u32.to_le_bytes());
        p[12..16].copy_from_slice(&1u32.to_le_bytes());
        p.extend_from_slice(&0x1234_5678u64.to_le_bytes()); // Sid pointer, non-zero
        p.extend_from_slice(&0u64.to_le_bytes()); // Attributes
        p.extend_from_slice(&[1, 2, 0, 0, 0, 0, 0, 5]); // revision, 2 sub-authorities, authority
        p.extend_from_slice(&[0u8; 8]); // the two sub-authorities
        p.extend_from_slice(b"game.exe\0");
        p.extend(wide("game.exe --fullscreen"));
        let event = decode(&raw(&PROCESS_GUID, opcode::DC_START, 4, &p));
        assert_eq!(
            event,
            KernelEvent::ProcessStart { timestamp: 1000, pid: 4242, parent_pid: 1, rundown: true, image_name: "game.exe".into() }
        );
        // No SID at all: the field is one zero pointer.
        let mut p = vec![0u8; 32];
        p[8..12].copy_from_slice(&4u32.to_le_bytes());
        p.extend_from_slice(&0u64.to_le_bytes());
        p.extend_from_slice(b"System\0");
        let event = decode(&raw(&PROCESS_GUID, opcode::START, 3, &p));
        assert_eq!(event, KernelEvent::ProcessStart { timestamp: 1000, pid: 4, parent_pid: 0, rundown: false, image_name: "System".into() });
        let event = decode(&raw(&PROCESS_GUID, opcode::END, 3, &p));
        assert_eq!(event, KernelEvent::ProcessEnd { timestamp: 1000, pid: 4, rundown: false });
    }

    #[test]
    fn an_image_load_decodes_base_size_pid_and_the_device_path() {
        let mut p = vec![0u8; 56];
        p[0..8].copy_from_slice(&0x7ff6_0000_0000u64.to_le_bytes());
        p[8..16].copy_from_slice(&0x20000u64.to_le_bytes());
        p[16..20].copy_from_slice(&4242u32.to_le_bytes());
        p.extend(wide(r"\Device\HarddiskVolume3\Games\game.exe"));
        let event = decode(&raw(&IMAGE_GUID, opcode::IMAGE_LOAD, 3, &p));
        assert_eq!(
            event,
            KernelEvent::ImageLoad {
                timestamp: 1000,
                pid: 4242,
                base: 0x7ff6_0000_0000,
                size: 0x20000,
                path: r"\Device\HarddiskVolume3\Games\game.exe".into(),
                rundown: false,
            }
        );
        let event = decode(&raw(&IMAGE_GUID, opcode::END, 3, &p));
        assert_eq!(event, KernelEvent::ImageUnload { timestamp: 1000, pid: 4242, base: 0x7ff6_0000_0000 });
    }

    #[test]
    fn a_sampled_profile_decodes_ip_and_thread() {
        let mut p = Vec::new();
        p.extend_from_slice(&0x7ff6_1234_5678u64.to_le_bytes());
        p.extend_from_slice(&504u32.to_le_bytes());
        p.extend_from_slice(&[1, 0, 0, 0]);
        let event = decode(&raw(&PERF_INFO_GUID, opcode::SAMPLED_PROFILE, 2, &p));
        assert_eq!(event, KernelEvent::SampledProfile { timestamp: 1000, tid: 504, ip: 0x7ff6_1234_5678 });
    }

    #[test]
    fn a_stack_walk_decodes_whole_frames_and_drops_a_partial_one() {
        let mut p = Vec::new();
        p.extend_from_slice(&987u64.to_le_bytes());
        p.extend_from_slice(&4242u32.to_le_bytes());
        p.extend_from_slice(&504u32.to_le_bytes());
        for frame in [0xFFFF_F800_0000_1000u64, 0x7ff6_0000_0100, 0x7ff6_0000_0200] {
            p.extend_from_slice(&frame.to_le_bytes());
        }
        p.extend_from_slice(&[1, 2, 3]); // a torn tail
        let event = decode(&raw(&STACK_WALK_GUID, opcode::STACK, 2, &p));
        assert_eq!(
            event,
            KernelEvent::Stack {
                timestamp: 1000,
                event_timestamp: 987,
                pid: 4242,
                tid: 504,
                frames: vec![0xFFFF_F800_0000_1000, 0x7ff6_0000_0100, 0x7ff6_0000_0200],
            }
        );
        assert!(is_kernel_address(0xFFFF_F800_0000_1000));
        assert!(!is_kernel_address(0x7ff6_0000_0100));
    }

    #[test]
    fn unknown_events_are_other() {
        assert_eq!(decode(&raw(&THREAD_GUID, 99, 2, &[0; 64])), KernelEvent::Other);
        assert_eq!(decode(&raw(&EVENT_TRACE_GUID, 0, 2, &[0; 64])), KernelEvent::Other);
    }
}
