//! Tightly packed live-view events.
//!
//! These are **not** a new capture format. Each variant maps to an existing
//! `orbit_grpc_protos::ClientCaptureEvent` that is cheap to show live:
//! API scopes, function-call timers, scheduling slices, and thread-state slices.
//! ELF/DWARF/module parsing stays on the service; this crate only holds the
//! already-decoded fields the viewer needs.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Wire size of [`LiveEvent`]. Must stay 32 so C FFI and the WS stream agree.
pub const LIVE_EVENT_SIZE: usize = 32;

/// Kinds that already exist on the capture stream and are cheap to show live.
pub mod kind {
    pub const API_SCOPE: u8 = 1;
    pub const FUNCTION_CALL: u8 = 2;
    pub const SCHEDULING_SLICE: u8 = 3;
    pub const THREAD_STATE: u8 = 4;
    pub const API_TRACK: u8 = 5;
}

/// `ThreadStateSlice::ThreadState` values from `capture.proto`.
pub mod thread_state {
    pub const RUNNING: u8 = 0;
    pub const RUNNABLE: u8 = 1;
    pub const INTERRUPTIBLE_SLEEP: u8 = 2;
    pub const UNINTERRUPTIBLE_SLEEP: u8 = 3;
    pub const STOPPED: u8 = 4;
    pub const TRACED: u8 = 5;
    pub const DEAD: u8 = 6;
    pub const ZOMBIE: u8 = 7;
    pub const PARKED: u8 = 8;
    pub const IDLE: u8 = 9;
}

/// Packed 32-byte event. Layout is little-endian on the wire.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveEvent {
    pub start_ns: u64,
    pub duration_ns: u64,
    pub tid: u32,
    pub pid: u32,
    pub kind: u8,
    pub depth: u8,
    /// Core id for scheduling slices, `ThreadState` for thread-state slices.
    pub extra: u8,
    pub _pad: u8,
    pub name_id: u32,
}

impl LiveEvent {
    pub fn end_ns(self) -> u64 {
        self.start_ns.saturating_add(self.duration_ns)
    }

    pub fn color_rgba(self) -> u32 {
        palette_color(self.kind, self.extra, self.name_id)
    }

    pub fn lane_key(self) -> LaneKey {
        LaneKey {
            tid: self.tid,
            kind: self.kind,
            depth: self.depth,
            extra: if self.kind == kind::SCHEDULING_SLICE {
                self.extra
            } else {
                0
            },
        }
    }

    pub fn as_bytes(self) -> [u8; LIVE_EVENT_SIZE] {
        let mut out = [0u8; LIVE_EVENT_SIZE];
        out[0..8].copy_from_slice(&self.start_ns.to_le_bytes());
        out[8..16].copy_from_slice(&self.duration_ns.to_le_bytes());
        out[16..20].copy_from_slice(&self.tid.to_le_bytes());
        out[20..24].copy_from_slice(&self.pid.to_le_bytes());
        out[24] = self.kind;
        out[25] = self.depth;
        out[26] = self.extra;
        out[27] = self._pad;
        out[28..32].copy_from_slice(&self.name_id.to_le_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8; LIVE_EVENT_SIZE]) -> Self {
        Self {
            start_ns: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
            duration_ns: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            tid: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            pid: u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
            kind: bytes[24],
            depth: bytes[25],
            extra: bytes[26],
            _pad: bytes[27],
            name_id: u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
        }
    }

    pub fn write_bytes(self, out: &mut [u8]) {
        let bytes = self.as_bytes();
        out[..LIVE_EVENT_SIZE].copy_from_slice(&bytes);
    }
}

/// Lane identity for the per-track, non-overlapping interval index.
#[derive(
    Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct LaneKey {
    pub kind: u8,
    pub tid: u32,
    pub depth: u8,
    pub extra: u8,
}

/// Deterministic color from kind / extra / interned name.
pub fn palette_color(kind: u8, extra: u8, name_id: u32) -> u32 {
    match kind {
        kind::THREAD_STATE => thread_state_color(extra),
        kind::SCHEDULING_SLICE => hash_color(0xC0_u32.wrapping_add(extra as u32)),
        _ => hash_color(name_id.wrapping_add((kind as u32) << 24)),
    }
}

fn thread_state_color(state: u8) -> u32 {
    match state {
        thread_state::RUNNING => 0xFF3DDC84,
        thread_state::RUNNABLE => 0xFFE6B422,
        thread_state::INTERRUPTIBLE_SLEEP => 0xFF4C6A92,
        thread_state::UNINTERRUPTIBLE_SLEEP => 0xFF8B5A2B,
        thread_state::STOPPED => 0xFFB0B0B0,
        thread_state::TRACED => 0xFF9B59B6,
        thread_state::DEAD | thread_state::ZOMBIE => 0xFF5A5A5A,
        thread_state::PARKED => 0xFF2F4F6F,
        thread_state::IDLE => 0xFF2A2A2A,
        _ => 0xFF808080,
    }
}

fn hash_color(seed: u32) -> u32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ 0xA5A5_A5A5;
    x ^= x >> 16;
    let r = 80 + (x & 0x7F);
    let g = 80 + ((x >> 8) & 0x7F);
    let b = 80 + ((x >> 16) & 0x7F);
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

/// Pairs `ApiScopeStart` / `ApiScopeStop` (and async variants) into duration events.
///
/// `ClientCaptureEvent` emits start and stop separately. The live ring stores
/// already-paired intervals so the renderer can treat each (tid, depth) lane
/// as non-overlapping.
#[derive(Default)]
pub struct ScopePairer {
    stacks: HashMap<u32, Vec<OpenScope>>,
    intern: InternTable,
}

#[derive(Clone, Copy)]
struct OpenScope {
    start_ns: u64,
    pid: u32,
    #[allow(dead_code)]
    color_rgba: u32,
    name_id: u32,
}

#[derive(Default)]
pub struct InternTable {
    by_text: HashMap<String, u32>,
    by_id: HashMap<u32, String>,
    next_id: u32,
}

impl InternTable {
    pub fn intern(&mut self, text: &str) -> u32 {
        if let Some(&id) = self.by_text.get(text) {
            return id;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.by_text.insert(text.to_string(), id);
        self.by_id.insert(id, text.to_string());
        id
    }

    pub fn insert_id(&mut self, id: u32, text: &str) {
        self.by_text.insert(text.to_string(), id);
        self.by_id.insert(id, text.to_string());
        if id >= self.next_id {
            self.next_id = id.wrapping_add(1);
        }
    }

    pub fn get(&self, id: u32) -> Option<&str> {
        self.by_id.get(&id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &str)> {
        self.by_id.iter().map(|(&id, text)| (id, text.as_str()))
    }
}

impl ScopePairer {
    pub fn intern(&mut self, text: &str) -> u32 {
        self.intern.intern(text)
    }

    pub fn intern_table(&self) -> &InternTable {
        &self.intern
    }

    pub fn intern_table_mut(&mut self) -> &mut InternTable {
        &mut self.intern
    }

    pub fn on_scope_start(
        &mut self,
        pid: u32,
        tid: u32,
        timestamp_ns: u64,
        color_rgba: u32,
        name_id: u32,
    ) {
        let _ = color_rgba;
        self.stacks.entry(tid).or_default().push(OpenScope {
            start_ns: timestamp_ns,
            pid,
            color_rgba,
            name_id,
        });
    }

    pub fn on_scope_stop(&mut self, pid: u32, tid: u32, timestamp_ns: u64) -> Option<LiveEvent> {
        let stack = self.stacks.get_mut(&tid)?;
        let open = stack.pop()?;
        let duration_ns = timestamp_ns.saturating_sub(open.start_ns);
        let depth = stack.len().min(255) as u8;
        Some(LiveEvent {
            start_ns: open.start_ns,
            duration_ns,
            tid,
            pid: if pid != 0 { pid } else { open.pid },
            kind: kind::API_SCOPE,
            depth,
            extra: 0,
            _pad: 0,
            name_id: open.name_id,
        })
    }

    pub fn function_call(
        &self,
        pid: u32,
        tid: u32,
        function_id: u64,
        duration_ns: u64,
        end_timestamp_ns: u64,
        depth: i32,
    ) -> LiveEvent {
        let name_id = function_id as u32;
        LiveEvent {
            start_ns: end_timestamp_ns.saturating_sub(duration_ns),
            duration_ns,
            tid,
            pid,
            kind: kind::FUNCTION_CALL,
            depth: depth.clamp(0, 255) as u8,
            extra: 0,
            _pad: 0,
            name_id,
        }
    }

    pub fn scheduling_slice(
        &self,
        pid: u32,
        tid: u32,
        core: i32,
        duration_ns: u64,
        out_timestamp_ns: u64,
    ) -> LiveEvent {
        let extra = core.clamp(0, 255) as u8;
        LiveEvent {
            start_ns: out_timestamp_ns.saturating_sub(duration_ns),
            duration_ns,
            tid,
            pid,
            kind: kind::SCHEDULING_SLICE,
            depth: 0,
            extra,
            _pad: 0,
            name_id: extra as u32,
        }
    }

    pub fn thread_state_slice(
        &self,
        pid: u32,
        tid: u32,
        state: u32,
        duration_ns: u64,
        end_timestamp_ns: u64,
    ) -> LiveEvent {
        let extra = state.min(255) as u8;
        LiveEvent {
            start_ns: end_timestamp_ns.saturating_sub(duration_ns),
            duration_ns,
            tid,
            pid,
            kind: kind::THREAD_STATE,
            depth: 0,
            extra,
            _pad: 0,
            name_id: extra as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_size_is_32() {
        assert_eq!(std::mem::size_of::<LiveEvent>(), 32);
        assert_eq!(LIVE_EVENT_SIZE, 32);
    }

    #[test]
    fn bytes_roundtrip_preserves_fields() {
        let ev = LiveEvent {
            start_ns: 1_000_000,
            duration_ns: 250,
            tid: 42,
            pid: 7,
            kind: kind::FUNCTION_CALL,
            depth: 3,
            extra: 0,
            _pad: 0,
            name_id: 99,
        };
        let decoded = LiveEvent::from_bytes(&ev.as_bytes());
        assert_eq!(decoded, ev);
        assert_ne!(decoded.color_rgba(), 0);
    }

    #[test]
    fn pairer_nests_and_computes_depth() {
        let mut p = ScopePairer::default();
        let a = p.intern("outer");
        let b = p.intern("inner");
        p.on_scope_start(1, 10, 100, 0, a);
        p.on_scope_start(1, 10, 110, 0, b);
        let inner = p.on_scope_stop(1, 10, 130).unwrap();
        let outer = p.on_scope_stop(1, 10, 200).unwrap();
        assert_eq!(inner.depth, 1);
        assert_eq!(inner.duration_ns, 20);
        assert_eq!(inner.name_id, b);
        assert_eq!(outer.depth, 0);
        assert_eq!(outer.duration_ns, 100);
        assert_eq!(outer.name_id, a);
    }

    #[test]
    fn stop_without_start_is_none() {
        let mut p = ScopePairer::default();
        assert!(p.on_scope_stop(1, 10, 100).is_none());
    }

    #[test]
    fn function_call_uses_end_minus_duration() {
        let p = ScopePairer::default();
        let ev = p.function_call(1, 2, 55, 40, 140, 2);
        assert_eq!(ev.start_ns, 100);
        assert_eq!(ev.duration_ns, 40);
        assert_eq!(ev.kind, kind::FUNCTION_CALL);
        assert_eq!(ev.depth, 2);
    }
}
