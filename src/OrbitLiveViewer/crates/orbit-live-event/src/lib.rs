//! Tightly packed live-view events.
//!
//! These are **not** a new capture format. Each variant maps to an existing
//! `orbit_grpc_protos::ClientCaptureEvent` that is cheap to show live:
//! API scopes, function-call timers, scheduling slices, and thread-state slices.
//! ELF/DWARF/module parsing stays on the service; this crate only holds the
//! already-decoded fields the viewer needs.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

mod color;
pub mod dev;
pub use color::{
    argb_to_css, async_scope_color, encode_manual_color, event_color, material_index_to_argb,
    name_hash, named_scope_color, palette_index, rgba_word_to_argb, scale_rgb, thread_scope_color,
    thread_state_color,
    BOX_BORDER, ORBIT_API_COLORS_RGBA, ORBIT_COLOR_RED, SAME_SCOPE_HIGHLIGHT, SELECTION,
    SHADE_LEFT, THREAD_PALETTE,
};
pub use color::{chrome, mode as color_mode};

/// Wire size of [`LiveEvent`]. Must stay 32 so C FFI and the WS stream agree.
pub const LIVE_EVENT_SIZE: usize = 32;

/// Kinds that already exist on the capture stream and are cheap to show live.
pub mod kind {
    pub const API_SCOPE: u8 = 1;
    pub const FUNCTION_CALL: u8 = 2;
    pub const SCHEDULING_SLICE: u8 = 3;
    pub const THREAD_STATE: u8 = 4;
    pub const API_TRACK: u8 = 5;
    /// Timestamped scalar sample. `duration_ns` holds `f32::to_bits(value) as u64`
    /// — it is not a duration. [`LiveEvent::end_ns`] is `start_ns + 1`.
    pub const VALUE: u8 = 6;
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
        if self.kind == kind::VALUE {
            self.start_ns.saturating_add(1)
        } else {
            self.start_ns.saturating_add(self.duration_ns)
        }
    }

    /// Decode a [`kind::VALUE`] sample. `duration_ns` stores `f32` bits.
    pub fn value_f32(self) -> Option<f32> {
        if self.kind == kind::VALUE {
            Some(f32::from_bits(self.duration_ns as u32))
        } else {
            None
        }
    }

    pub fn from_value(start_ns: u64, pid: u32, tid: u32, name_id: u32, value: f32) -> Self {
        Self {
            start_ns,
            duration_ns: f32::to_bits(value) as u64,
            tid,
            pid,
            kind: kind::VALUE,
            depth: 0,
            extra: 0,
            _pad: color_mode::AUTO_NAME,
            name_id,
        }
    }

    pub fn color_rgba(self) -> u32 {
        self.color_for(None)
    }

    pub fn color_for(self, intern: Option<&InternTable>) -> u32 {
        let name = intern.and_then(|t| t.get(self.name_id)).map(str::as_bytes);
        event_color(
            self.kind,
            self.tid,
            self.depth,
            self.extra,
            self._pad,
            self.name_id,
            name,
        )
    }

    pub fn lane_key(self) -> LaneKey {
        if self.kind == kind::SCHEDULING_SLICE {
            return LaneKey::scheduler(self.extra);
        }
        LaneKey {
            pid: self.pid,
            tid: self.tid,
            kind: self.kind,
            depth: self.depth,
            extra: 0,
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
///
/// Scoped by process so overlapping tids from different pids do not collide.
/// `LiveEvent` stays 32 bytes; pid is already on the event.
#[derive(
    Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct LaneKey {
    pub pid: u32,
    pub tid: u32,
    pub kind: u8,
    pub depth: u8,
    pub extra: u8,
}

impl LaneKey {
    pub fn thread(self) -> (u32, u32) {
        (self.pid, self.tid)
    }

    /// Capture-global CPU-core lane. Sentinel pid/tid so cores do not spawn
    /// fake process/thread rows; `extra` is the core id.
    pub fn scheduler(core: u8) -> Self {
        Self {
            pid: 0,
            tid: 0,
            kind: kind::SCHEDULING_SLICE,
            depth: 0,
            extra: core,
        }
    }

    pub fn is_scheduler(self) -> bool {
        self.kind == kind::SCHEDULING_SLICE
    }
}

/// Kept for call sites that still pass `(kind, extra, name_id)`.
/// Thread/CPU scopes need tid+depth — prefer [`LiveEvent::color_rgba`].
pub fn palette_color(kind: u8, extra: u8, name_id: u32) -> u32 {
    match kind {
        kind::THREAD_STATE => thread_state_color(extra),
        kind::API_SCOPE | kind::API_TRACK => named_scope_color(&name_id.to_le_bytes(), extra),
        _ => thread_scope_color(name_id, extra),
    }
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

    /// Resolve a scope search to matching `name_id`s. Empty query ⇒ empty set
    /// (callers treat that as “no filter”, not “match nothing”).
    pub fn ids_matching(&self, query: &str) -> HashSet<u32> {
        let q = query.trim();
        if q.is_empty() {
            return HashSet::new();
        }
        let lower = q.to_ascii_lowercase();
        let mut out = HashSet::new();
        let numeric = lower.strip_prefix('#').unwrap_or(&lower);
        if let Ok(id) = numeric.parse::<u32>() {
            out.insert(id);
        }
        for (id, text) in self.iter() {
            if text.to_ascii_lowercase().contains(&lower) {
                out.insert(id);
            }
        }
        out
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
        let (pad, extra) = encode_manual_color(open.color_rgba);
        Some(LiveEvent {
            start_ns: open.start_ns,
            duration_ns,
            tid,
            pid: if pid != 0 { pid } else { open.pid },
            kind: kind::API_SCOPE,
            depth,
            extra,
            _pad: pad,
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
    fn thread_palette_matches_orbit_hex() {
        assert_eq!(THREAD_PALETTE[0], 0xFFE7_4435);
        assert_eq!(THREAD_PALETTE[1], 0xFF2B_91AF);
        assert_eq!(THREAD_PALETTE[2], 0xFFB9_75B5);
        assert_eq!(THREAD_PALETTE[3], 0xFF57_A64A);
        assert_eq!(THREAD_PALETTE[4], 0xFFD7_AB69);
        assert_eq!(THREAD_PALETTE[5], 0xFFF8_6516);
        assert_eq!(argb_to_css(THREAD_PALETTE[0]), "#E74435");
        assert_eq!(BOX_BORDER, 0xFFFF_FFFF);
        assert_eq!(ORBIT_COLOR_RED, 0xF443_36FF);
    }

    #[test]
    fn cpu_scope_uses_tid_mod_6_and_even_depth_darkens() {
        let odd = thread_scope_color(0, 1);
        let even = thread_scope_color(0, 0);
        assert_eq!(odd, 0xFFE7_4435);
        assert_eq!(thread_scope_color(6, 1), 0xFFE7_4435);
        assert_eq!(thread_scope_color(1, 1), 0xFF2B_91AF);
        assert_eq!(even, scale_rgb(odd, 210, 255));
        assert_ne!(even, odd);
        let ev = LiveEvent {
            tid: 7,
            kind: kind::FUNCTION_CALL,
            depth: 1,
            extra: 0,
            _pad: color_mode::AUTO_THREAD,
            name_id: 99,
            start_ns: 0,
            duration_ns: 1,
            pid: 1,
        };
        assert_eq!(ev.color_rgba(), thread_scope_color(7, 1));
    }

    #[test]
    fn api_scope_color_is_name_hash_not_tid() {
        let mut intern = InternTable::default();
        let tick = intern.intern("Tick");
        let a = LiveEvent {
            tid: 101,
            kind: kind::API_SCOPE,
            depth: 1,
            extra: 0,
            _pad: color_mode::AUTO_NAME,
            name_id: tick,
            start_ns: 0,
            duration_ns: 1,
            pid: 1,
        };
        let mut b = a;
        b.tid = 107;
        b.pid = 11;
        assert_eq!(a.color_for(Some(&intern)), b.color_for(Some(&intern)));
        assert_eq!(
            a.color_for(Some(&intern)),
            named_scope_color(b"Tick", 1)
        );
        assert_ne!(a.color_for(Some(&intern)), thread_scope_color(101, 1));
        let even = LiveEvent { depth: 0, ..a };
        assert_eq!(
            even.color_for(Some(&intern)),
            scale_rgb(named_scope_color(b"Tick", 1), 210, 255)
        );
        let miss = LiveEvent {
            name_id: 99,
            tid: 1,
            kind: kind::API_SCOPE,
            depth: 1,
            extra: 0,
            _pad: color_mode::AUTO_NAME,
            start_ns: 0,
            duration_ns: 1,
            pid: 1,
        };
        assert_eq!(
            miss.color_rgba(),
            named_scope_color(&99u32.to_le_bytes(), 1)
        );
    }

    #[test]
    fn async_name_hash_indexes_the_same_six_colors() {
        let h = name_hash(b"GpuSubmit");
        assert_eq!(async_scope_color(h), THREAD_PALETTE[(h as usize) % 6]);
        let mut intern = InternTable::default();
        intern.insert_id(1, "GpuSubmit");
        let ev = LiveEvent {
            kind: kind::API_TRACK,
            extra: 0,
            _pad: color_mode::AUTO_NAME,
            tid: 99,
            depth: 1,
            name_id: 1,
            start_ns: 0,
            duration_ns: 1,
            pid: 1,
        };
        assert_eq!(
            ev.color_for(Some(&intern)),
            named_scope_color(b"GpuSubmit", 1)
        );
        assert_eq!(
            ev.color_for(Some(&intern)),
            THREAD_PALETTE[(h as usize) % 6]
        );
    }

    #[test]
    fn manual_api_material_red_is_orbit_h_word() {
        assert_eq!(
            encode_manual_color(0xF443_36FF),
            (color_mode::MANUAL_API, 1)
        );
        assert_eq!(material_index_to_argb(1), 0xFFF4_4336);
        let ev = LiveEvent {
            kind: kind::API_SCOPE,
            extra: 1,
            _pad: color_mode::MANUAL_API,
            tid: 0,
            depth: 1,
            name_id: 0,
            start_ns: 0,
            duration_ns: 1,
            pid: 1,
        };
        assert_eq!(ev.color_rgba(), 0xFFF4_4336);
    }

    #[test]
    fn thread_state_colors_match_thread_state_bar() {
        assert_eq!(thread_state_color(thread_state::RUNNING), 0xFF4C_AF50);
        assert_eq!(thread_state_color(thread_state::RUNNABLE), 0xFF21_96F3);
        assert_eq!(
            thread_state_color(thread_state::INTERRUPTIBLE_SLEEP),
            0xFF75_7575
        );
        assert_eq!(
            thread_state_color(thread_state::UNINTERRUPTIBLE_SLEEP),
            0xFFFF_9800
        );
        assert_eq!(thread_state_color(thread_state::STOPPED), 0xFFF4_4336);
        assert_eq!(thread_state_color(thread_state::TRACED), 0xFF9C_27B0);
        assert_eq!(thread_state_color(thread_state::DEAD), 0xFF00_0000);
        assert_eq!(thread_state_color(thread_state::ZOMBIE), 0xFF00_0000);
        assert_eq!(thread_state_color(thread_state::PARKED), 0xFF79_5548);
        assert_eq!(thread_state_color(thread_state::IDLE), 0xFF79_5548);
    }

    #[test]
    fn pairer_keeps_manual_api_color() {
        let mut p = ScopePairer::default();
        let a = p.intern("red");
        p.on_scope_start(1, 10, 100, 0xF443_36FF, a);
        let ev = p.on_scope_stop(1, 10, 200).unwrap();
        assert_eq!(ev._pad, color_mode::MANUAL_API);
        assert_eq!(ev.color_rgba(), 0xFFF4_4336);
    }

    #[test]
    fn packed_size_is_32() {
        assert_eq!(std::mem::size_of::<LiveEvent>(), 32);
        assert_eq!(LIVE_EVENT_SIZE, 32);
    }

    #[test]
    fn value_event_stores_f32_bits_in_duration() {
        let ev = LiveEvent::from_value(1_000, 1, 600, 7, -0.5);
        assert_eq!(ev.kind, kind::VALUE);
        assert_eq!(ev.end_ns(), 1_001);
        assert!((ev.value_f32().unwrap() + 0.5).abs() < 1e-6);
        assert_eq!(ev.duration_ns, f32::to_bits(-0.5) as u64);
        assert_eq!(std::mem::size_of_val(&ev), 32);
    }

    #[test]
    fn lane_key_includes_pid_so_tids_do_not_collide() {
        let a = LiveEvent {
            tid: 7,
            pid: 1,
            kind: kind::API_SCOPE,
            ..LiveEvent::default()
        };
        let b = LiveEvent {
            tid: 7,
            pid: 2,
            kind: kind::API_SCOPE,
            ..LiveEvent::default()
        };
        assert_ne!(a.lane_key(), b.lane_key());
        assert_eq!(a.lane_key().pid, 1);
        assert_eq!(b.lane_key().pid, 2);
    }

    #[test]
    fn scheduling_lane_key_is_core_only() {
        let a = LiveEvent {
            tid: 100,
            pid: 1,
            kind: kind::SCHEDULING_SLICE,
            extra: 3,
            _pad: color_mode::AUTO_THREAD,
            ..LiveEvent::default()
        };
        let b = LiveEvent {
            tid: 200,
            pid: 10,
            kind: kind::SCHEDULING_SLICE,
            extra: 3,
            _pad: color_mode::AUTO_THREAD,
            ..LiveEvent::default()
        };
        let c = LiveEvent {
            tid: 100,
            pid: 1,
            kind: kind::SCHEDULING_SLICE,
            extra: 4,
            _pad: color_mode::AUTO_THREAD,
            ..LiveEvent::default()
        };
        assert_eq!(a.lane_key(), b.lane_key());
        assert_eq!(a.lane_key(), LaneKey::scheduler(3));
        assert_ne!(a.lane_key(), c.lane_key());
        assert_eq!(a.pid, 1);
        assert_eq!(a.tid, 100);
        assert_eq!(b.pid, 10);
        assert_eq!(b.tid, 200);
        assert_eq!(
            a.color_rgba(),
            thread_scope_color(100, 1),
            "scheduler uses GetThreadColor, not even-depth darken"
        );
        assert_ne!(a.color_rgba(), thread_scope_color(200, 1));
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

    #[test]
    fn intern_ids_matching_is_substring_and_numeric() {
        let mut intern = InternTable::default();
        intern.insert_id(30_000, "Frame");
        intern.insert_id(30_001, "TimelinePayload");
        intern.insert_id(100, "Main");
        assert!(intern.ids_matching("").is_empty());
        let frame = intern.ids_matching("frame");
        assert!(frame.contains(&30_000));
        assert!(!frame.contains(&30_001));
        let hash = intern.ids_matching("#30000");
        assert!(hash.contains(&30_000));
        let num = intern.ids_matching("100");
        assert!(num.contains(&100));
    }
}
