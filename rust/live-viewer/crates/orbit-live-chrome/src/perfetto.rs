// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Perfetto proto traces (`.pftrace`, `.perfetto-trace`, `.pb`) into the
//! same [`ChromeIngestor`] the JSON path feeds.
//!
//! A trace is a stream of length-delimited `TracePacket`s. The packets this
//! understands, walked with the schema-less [`crate::proto`] reader:
//!
//! | Packet | Live viewer |
//! |---|---|
//! | `track_event` slice begin/end/instant on a thread track | `B`/`E`/`I` on that pid/tid |
//! | ... on a process, global, or named child track | nestable async `b`/`e`/`n`, one lane per track |
//! | `track_event` counter, `counter` tracks | `C` value series named by the track |
//! | `track_event.legacy_event` (Chrome's `X`, `S`/`T`/`F`, `M`, ...) | the JSON phase, verbatim |
//! | `track_descriptor`, `thread_descriptor`, `process_descriptor`, `process_tree` | process / thread names and sort order |
//! | `interned_data` + `sequence_flags` | per-sequence name, category, arg and callstack tables |
//! | `clock_snapshot`, `trace_packet_defaults` | incremental and sequence-scoped clocks → trace-clock ns |
//! | `debug_annotations` | hover args |
//! | `flow_ids` / `terminating_flow_ids` | flow arrows (first id of an event) |
//! | `ftrace_events` `sched_switch` / `sched_waking` (plain and compact) | scheduling slices per core, thread states |
//! | `ftrace_events` `print` (`B\|pid\|name` atrace marks) | scopes on the writing thread |
//! | `chrome_events` (`trace_events`, `legacy_json_trace`, `legacy_ftrace_output`) | the JSON path, embedded |
//! | `perf_sample` with an interned callstack | sampled call frames |
//!
//! Everything else (`trace_config`, `sys_stats`, GPU, memory snapshots,
//! ETW, ...) is counted in `stats.skipped_packets` and dropped.

use std::collections::HashMap;

use orbit_live_event::{kind, thread_state, LiveEvent};
use serde_json::{Map, Value};

use crate::id::{FlexId, Id2};
use crate::ingest::{ChromeEvent, ChromeIngestor, TimeUnit};
use crate::proto::fields;
use crate::stream::ChromeStream;

// TracePacket
const P_FTRACE_EVENTS: u32 = 1;
const P_PROCESS_TREE: u32 = 2;
const P_CHROME_EVENTS: u32 = 5;
const P_CLOCK_SNAPSHOT: u32 = 6;
const P_TIMESTAMP: u32 = 8;
const P_SEQUENCE_ID: u32 = 10;
const P_TRACK_EVENT: u32 = 11;
const P_INTERNED_DATA: u32 = 12;
const P_SEQUENCE_FLAGS: u32 = 13;
const P_INCREMENTAL_STATE_CLEARED: u32 = 41;
const P_PROCESS_DESCRIPTOR: u32 = 43;
const P_THREAD_DESCRIPTOR: u32 = 44;
const P_TIMESTAMP_CLOCK_ID: u32 = 58;
const P_PACKET_DEFAULTS: u32 = 59;
const P_TRACK_DESCRIPTOR: u32 = 60;
const P_PERF_SAMPLE: u32 = 66;
/// Trace-level packets that carry no event data; not "skipped", just
/// structure. (`trusted_uid` and friends ride on every packet and say
/// nothing about what it holds.)
const P_STRUCTURAL: &[u32] = &[
    33, // trace_config
    35, // trace_stats
    36, // synchronization_marker
    45, // system_info
    89, // trace_uuid
];

const SEQ_INCREMENTAL_STATE_CLEARED: u64 = 1;

// TrackEvent
const TE_TIMESTAMP_DELTA_US: u32 = 1;
const TE_CATEGORY_IIDS: u32 = 3;
const TE_DEBUG_ANNOTATIONS: u32 = 4;
const TE_LEGACY_EVENT: u32 = 6;
const TE_TYPE: u32 = 9;
const TE_NAME_IID: u32 = 10;
const TE_TRACK_UUID: u32 = 11;
const TE_TIMESTAMP_ABSOLUTE_US: u32 = 16;
const TE_CATEGORIES: u32 = 22;
const TE_NAME: u32 = 23;
const TE_CHROME_HISTOGRAM_SAMPLE: u32 = 28;
const TE_COUNTER_VALUE: u32 = 30;
const TE_FLOW_IDS_OLD: u32 = 36;
const TE_TERMINATING_FLOW_IDS_OLD: u32 = 42;
const TE_DOUBLE_COUNTER_VALUE: u32 = 44;
const TE_FLOW_IDS: u32 = 47;
const TE_TERMINATING_FLOW_IDS: u32 = 48;

const TYPE_SLICE_BEGIN: u64 = 1;
const TYPE_SLICE_END: u64 = 2;
const TYPE_INSTANT: u64 = 3;
const TYPE_COUNTER: u64 = 4;

// TrackEvent.LegacyEvent
const LE_NAME_IID: u32 = 1;
const LE_PHASE: u32 = 2;
const LE_DURATION_US: u32 = 3;
const LE_UNSCOPED_ID: u32 = 6;
const LE_BIND_ID: u32 = 8;
const LE_LOCAL_ID: u32 = 10;
const LE_GLOBAL_ID: u32 = 11;
const LE_FLOW_DIRECTION: u32 = 13;
const LE_INSTANT_SCOPE: u32 = 14;
const LE_PID_OVERRIDE: u32 = 18;
const LE_TID_OVERRIDE: u32 = 19;

// TrackDescriptor
const TD_UUID: u32 = 1;
const TD_NAME: u32 = 2;
const TD_PROCESS: u32 = 3;
const TD_THREAD: u32 = 4;
const TD_PARENT_UUID: u32 = 5;
const TD_CHROME_PROCESS: u32 = 6;
const TD_CHROME_THREAD: u32 = 7;
const TD_COUNTER: u32 = 8;
const TD_STATIC_NAME: u32 = 10;
const TD_ATRACE_NAME: u32 = 13;

// ThreadDescriptor / ProcessDescriptor
const THD_PID: u32 = 1;
const THD_TID: u32 = 2;
const THD_SORT_INDEX: u32 = 3;
const THD_NAME: u32 = 5;
const THD_REFERENCE_TIMESTAMP_US: u32 = 6;
const PD_PID: u32 = 1;
const PD_CMDLINE: u32 = 2;
const PD_SORT_INDEX: u32 = 3;
const PD_NAME: u32 = 6;
const CHROME_DESC_SORT_INDEX_PROCESS: u32 = 3;
const CHROME_DESC_SORT_INDEX_THREAD: u32 = 2;

// CounterDescriptor
const CD_UNIT_MULTIPLIER: u32 = 4;
const CD_IS_INCREMENTAL: u32 = 5;

// TrackEvent.ChromeHistogramSample
const HS_NAME: u32 = 2;
const HS_SAMPLE: u32 = 3;
const HS_NAME_IID: u32 = 4;

// InternedData
const ID_EVENT_CATEGORIES: u32 = 1;
const ID_EVENT_NAMES: u32 = 2;
const ID_DEBUG_ANNOTATION_NAMES: u32 = 3;
const ID_FUNCTION_NAMES: u32 = 5;
const ID_FRAMES: u32 = 6;
const ID_CALLSTACKS: u32 = 7;
const ID_MAPPING_PATHS: u32 = 17;
const ID_MAPPINGS: u32 = 19;
const ID_HISTOGRAM_NAMES: u32 = 25;
const ID_DEBUG_ANNOTATION_STRING_VALUES: u32 = 29;

// DebugAnnotation
const DA_NAME_IID: u32 = 1;
const DA_BOOL: u32 = 2;
const DA_UINT: u32 = 3;
const DA_INT: u32 = 4;
const DA_DOUBLE: u32 = 5;
const DA_STRING: u32 = 6;
const DA_POINTER: u32 = 7;
const DA_NESTED: u32 = 8;
const DA_LEGACY_JSON: u32 = 9;
const DA_NAME: u32 = 10;
const DA_DICT_ENTRIES: u32 = 11;
const DA_ARRAY_VALUES: u32 = 12;
const DA_PROTO_VALUE: u32 = 14;
const DA_STRING_IID: u32 = 17;

// DebugAnnotation.NestedValue
const NV_TYPE: u32 = 1;
const NV_DICT_KEYS: u32 = 2;
const NV_DICT_VALUES: u32 = 3;
const NV_ARRAY_VALUES: u32 = 4;
const NV_INT: u32 = 5;
const NV_DOUBLE: u32 = 6;
const NV_BOOL: u32 = 7;
const NV_STRING: u32 = 8;

// ClockSnapshot
const CS_CLOCKS: u32 = 1;
const CS_PRIMARY_TRACE_CLOCK: u32 = 2;
const CLK_ID: u32 = 1;
const CLK_TIMESTAMP: u32 = 2;
const CLK_IS_INCREMENTAL: u32 = 3;
const CLK_UNIT_MULTIPLIER_NS: u32 = 4;
const CLOCK_MONOTONIC: u32 = 3;
const CLOCK_BOOTTIME: u32 = 6;
/// Clock ids at or above this are scoped to the packet sequence.
const SEQ_CLOCK_MIN: u32 = 64;

// TracePacketDefaults
const PDF_TRACK_EVENT_DEFAULTS: u32 = 11;
const PDF_TIMESTAMP_CLOCK_ID: u32 = 58;
const TED_TRACK_UUID: u32 = 11;

// FtraceEventBundle / FtraceEvent
const FB_CPU: u32 = 1;
const FB_EVENT: u32 = 2;
const FB_COMPACT_SCHED: u32 = 4;
const FE_TIMESTAMP: u32 = 1;
const FE_PID: u32 = 2;
const FE_PRINT: u32 = 3;
const FE_SCHED_SWITCH: u32 = 4;
const FE_SCHED_WAKING: u32 = 20;
const SS_PREV_COMM: u32 = 1;
const SS_PREV_PID: u32 = 2;
const SS_PREV_STATE: u32 = 4;
const SS_NEXT_COMM: u32 = 5;
const SS_NEXT_PID: u32 = 6;
const SW_COMM: u32 = 1;
const SW_PID: u32 = 2;
const PR_BUF: u32 = 2;
const CS_SWITCH_TIMESTAMP: u32 = 1;
const CS_SWITCH_PREV_STATE: u32 = 2;
const CS_SWITCH_NEXT_PID: u32 = 3;
const CS_INTERN_TABLE: u32 = 5;
const CS_SWITCH_NEXT_COMM_INDEX: u32 = 6;
const CS_WAKING_TIMESTAMP: u32 = 7;
const CS_WAKING_PID: u32 = 8;
const CS_WAKING_COMM_INDEX: u32 = 11;

// ProcessTree
const PT_PROCESSES: u32 = 1;
const PT_THREADS: u32 = 2;
const PTP_PID: u32 = 1;
const PTP_CMDLINE: u32 = 3;
const PTT_TID: u32 = 1;
const PTT_NAME: u32 = 2;
const PTT_TGID: u32 = 3;

// ChromeEventBundle / ChromeTraceEvent
const CE_TRACE_EVENTS: u32 = 1;
const CE_STRING_TABLE: u32 = 3;
const CE_LEGACY_FTRACE_OUTPUT: u32 = 4;
const CE_LEGACY_JSON_TRACE: u32 = 5;
const CST_VALUE: u32 = 1;
const CST_INDEX: u32 = 2;
const CLJ_DATA: u32 = 2;
const CT_NAME: u32 = 1;
const CT_TIMESTAMP: u32 = 2;
const CT_PHASE: u32 = 3;
const CT_THREAD_ID: u32 = 4;
const CT_DURATION: u32 = 5;
const CT_SCOPE: u32 = 7;
const CT_ID: u32 = 8;
const CT_FLAGS: u32 = 9;
const CT_CATEGORY: u32 = 10;
const CT_PROCESS_ID: u32 = 11;
const CT_BIND_ID: u32 = 13;
const CT_ARGS: u32 = 14;
const CT_NAME_INDEX: u32 = 15;
const CT_CATEGORY_INDEX: u32 = 16;
const CTA_NAME: u32 = 1;
const CTA_BOOL: u32 = 2;
const CTA_UINT: u32 = 3;
const CTA_INT: u32 = 4;
const CTA_DOUBLE: u32 = 5;
const CTA_STRING: u32 = 6;
const CTA_POINTER: u32 = 7;
const CTA_JSON: u32 = 8;
/// `base/trace_event/common/trace_event_common.h` flag bits.
const CT_FLAG_SCOPE_SHIFT: u32 = 3;
const CT_FLAG_FLOW_IN: u32 = 1 << 8;
const CT_FLAG_FLOW_OUT: u32 = 1 << 9;
const CT_FLAG_HAS_LOCAL_ID: u32 = 1 << 11;
const CT_FLAG_HAS_GLOBAL_ID: u32 = 1 << 12;

// PerfSample
const PS_CPU: u32 = 1;
const PS_PID: u32 = 2;
const PS_TID: u32 = 3;
const PS_CALLSTACK_IID: u32 = 4;

/// Longest descriptor parent chain walked; deeper is a cycle or nonsense.
const MAX_TRACK_DEPTH: usize = 16;
/// Nested debug-annotation depth kept in a hover string.
const MAX_ANNOTATION_DEPTH: usize = 6;

#[derive(Default)]
struct Seq {
    event_names: HashMap<u64, String>,
    categories: HashMap<u64, String>,
    debug_names: HashMap<u64, String>,
    debug_strings: HashMap<u64, String>,
    function_names: HashMap<u64, String>,
    /// iid → (function name iid, mapping iid, rel_pc)
    frames: HashMap<u64, (u64, u64, u64)>,
    callstacks: HashMap<u64, Vec<u64>>,
    mapping_paths: HashMap<u64, String>,
    /// mapping iid → path string iids
    mappings: HashMap<u64, Vec<u64>>,
    histogram_names: HashMap<u64, String>,
    default_track: Option<u64>,
    default_clock: Option<u32>,
    /// The pre-descriptor way (2019 Chrome): one `thread_descriptor`
    /// packet per sequence, then events without `track_uuid`, timestamped
    /// as µs deltas from its `reference_timestamp_us`.
    legacy_thread: Option<(u32, u32)>,
    legacy_ts_us: i64,
}

#[derive(Clone, Debug, Default)]
struct Track {
    name: String,
    parent: Option<u64>,
    thread: Option<(u32, u32)>,
    process: Option<u32>,
    counter: Option<CounterDesc>,
}

#[derive(Clone, Copy, Debug)]
struct CounterDesc {
    incremental: bool,
    multiplier: f64,
    acc: f64,
}

/// What a track resolves to once its parent chain is followed.
#[derive(Clone, Debug, Default)]
struct Target {
    pid: Option<u32>,
    tid: Option<u32>,
    /// The track is a thread's own track (not a named child of one).
    on_thread: bool,
    name: String,
}

#[derive(Clone, Copy, Debug)]
struct Clock {
    mult_ns: u64,
    incremental: bool,
    /// Running absolute value in clock units for an incremental clock.
    acc: u64,
    snap_units: u64,
    snap_trace_ns: i128,
}

#[derive(Clone, Copy, Debug)]
struct OnCpu {
    tid: u32,
    since: u64,
}

/// Cross-packet state for one trace. Owned by the [`ChromeStream`] that
/// detected the proto format.
#[derive(Default)]
pub struct PerfettoState {
    seqs: HashMap<u32, Seq>,
    tracks: HashMap<u64, Track>,
    clocks: HashMap<(u32, u32), Clock>,
    trace_clock: Option<u32>,
    /// tid → pid, from descriptors and the process tree, for ftrace rows
    /// that only know a tid.
    pid_of_tid: HashMap<u32, u32>,
    on_cpu: HashMap<u32, OnCpu>,
    /// tid → (off-cpu since, state)
    off_cpu: HashMap<u32, (u64, u8)>,
    last_ftrace_ns: u64,
    /// Thread states of threads whose process is not known yet. A Perfetto
    /// file is not in time order across producers: the process tree that
    /// says which process a tid belongs to often sits at the end, after
    /// every sched packet. A thread-state bar lanes by (pid, tid), so it
    /// waits here until the tid's process is known (or the file ends).
    /// Scheduler slices lane by core and go out at once.
    pending_states: HashMap<u32, Vec<LiveEvent>>,
    /// States released by a tid → pid discovery inside a packet, handed
    /// back at the end of that packet.
    released: Vec<LiveEvent>,
    /// The last sched comm seen per tid, applied at the end to any
    /// `tid N` placeholder a late process id minted.
    comms: HashMap<u32, String>,
}

struct Packet<'a> {
    seq: u32,
    ts: Option<u64>,
    clock: Option<u32>,
    cleared: bool,
    interned: Vec<&'a [u8]>,
    defaults: Option<&'a [u8]>,
    snapshot: Option<&'a [u8]>,
    track_desc: Vec<&'a [u8]>,
    thread_desc: Option<&'a [u8]>,
    process_desc: Option<&'a [u8]>,
    payloads: Vec<(u32, &'a [u8])>,
    structural: bool,
}

impl PerfettoState {
    /// One `TracePacket`. Structure (interned data, descriptors, clocks) is
    /// applied before the packet's own events so a packet that carries both
    /// reads its own names.
    pub fn on_packet(&mut self, buf: &[u8], ing: &mut ChromeIngestor) -> Vec<LiveEvent> {
        ing.stats.packets += 1;
        let mut pk = Packet {
            seq: 0,
            ts: None,
            clock: None,
            cleared: false,
            interned: Vec::new(),
            defaults: None,
            snapshot: None,
            track_desc: Vec::new(),
            thread_desc: None,
            process_desc: None,
            payloads: Vec::new(),
            structural: false,
        };
        for (num, val) in fields(buf) {
            match num {
                P_SEQUENCE_ID => pk.seq = val.u32(),
                P_TIMESTAMP => pk.ts = Some(val.u64()),
                P_TIMESTAMP_CLOCK_ID => pk.clock = Some(val.u32()),
                P_SEQUENCE_FLAGS => {
                    if val.u64() & SEQ_INCREMENTAL_STATE_CLEARED != 0 {
                        pk.cleared = true;
                    }
                }
                P_INCREMENTAL_STATE_CLEARED => pk.cleared |= val.bool(),
                P_INTERNED_DATA => pk.interned.push(val.bytes()),
                P_PACKET_DEFAULTS => pk.defaults = Some(val.bytes()),
                P_CLOCK_SNAPSHOT => pk.snapshot = Some(val.bytes()),
                P_TRACK_DESCRIPTOR => pk.track_desc.push(val.bytes()),
                P_THREAD_DESCRIPTOR => pk.thread_desc = Some(val.bytes()),
                P_PROCESS_DESCRIPTOR => pk.process_desc = Some(val.bytes()),
                P_TRACK_EVENT | P_FTRACE_EVENTS | P_PROCESS_TREE | P_CHROME_EVENTS
                | P_PERF_SAMPLE => pk.payloads.push((num, val.bytes())),
                n if P_STRUCTURAL.contains(&n) => pk.structural = true,
                _ => {}
            }
        }

        if pk.cleared {
            self.seqs.insert(pk.seq, Seq::default());
        }
        if let Some(b) = pk.snapshot {
            self.on_clock_snapshot(pk.seq, b);
        }
        if let Some(b) = pk.defaults {
            self.on_packet_defaults(pk.seq, b);
        }
        for b in &pk.interned {
            self.on_interned_data(pk.seq, b);
        }
        for b in &pk.track_desc {
            self.on_track_descriptor(b, ing);
        }
        if let Some(b) = pk.process_desc {
            self.on_legacy_process_descriptor(b, ing);
        }
        if let Some(b) = pk.thread_desc {
            self.on_legacy_thread_descriptor(pk.seq, b, ing);
        }

        let mut out = Vec::new();
        let had_payload = !pk.payloads.is_empty();
        for (num, b) in std::mem::take(&mut pk.payloads) {
            match num {
                P_TRACK_EVENT => out.extend(self.on_track_event(&pk, b, ing)),
                P_FTRACE_EVENTS => out.extend(self.on_ftrace_bundle(b, ing)),
                P_PROCESS_TREE => self.on_process_tree(b, ing),
                P_CHROME_EVENTS => out.extend(self.on_chrome_events(b, ing)),
                P_PERF_SAMPLE => out.extend(self.on_perf_sample(&pk, b, ing)),
                _ => {}
            }
        }
        if !self.released.is_empty() {
            let released = std::mem::take(&mut self.released);
            ing.note_direct_events(&released);
            out.extend(released);
        }
        let structural = had_payload
            || pk.structural
            || pk.snapshot.is_some()
            || pk.defaults.is_some()
            || !pk.interned.is_empty()
            || !pk.track_desc.is_empty()
            || pk.thread_desc.is_some()
            || pk.process_desc.is_some();
        if !structural {
            ing.stats.skipped_packets += 1;
        }
        out
    }

    /// End of input: threads still on a core get their slice closed at the
    /// last ftrace timestamp, as a live capture's stop does; states still
    /// waiting for a process id go out under their tid; sched comms name
    /// the placeholders that late process ids minted.
    pub fn finish(&mut self, ing: &mut ChromeIngestor) -> Vec<LiveEvent> {
        let end = self.last_ftrace_ns;
        let mut out = Vec::new();
        for (cpu, cur) in std::mem::take(&mut self.on_cpu) {
            if cur.tid == 0 || end <= cur.since {
                continue;
            }
            self.push_sched(&mut out, cpu, cur, end);
        }
        for (tid, states) in std::mem::take(&mut self.pending_states) {
            let pid = self.pid_of(tid);
            out.extend(states.into_iter().map(|e| LiveEvent { pid, ..e }));
        }
        out.extend(std::mem::take(&mut self.released));
        ing.note_direct_events(&out);
        for ((_, tid), name) in ing.thread_names.iter_mut() {
            if name.starts_with("tid ") {
                if let Some(comm) = self.comms.get(tid) {
                    *name = comm.clone();
                }
            }
        }
        out
    }

    /// A tid's process became known: release the states that waited, and
    /// give the thread the comm the scheduler already saw.
    fn learn_pid(&mut self, ing: &mut ChromeIngestor, tid: u32, pid: u32) {
        if pid == 0 || tid == 0 {
            return;
        }
        self.pid_of_tid.insert(tid, pid);
        if let Some(states) = self.pending_states.remove(&tid) {
            self.released
                .extend(states.into_iter().map(|e| LiveEvent { pid, ..e }));
        }
        if let Some(comm) = self.comms.get(&tid).cloned() {
            self.name_from_comm(ing, tid, Some(&comm));
        }
    }

    // ---- clocks -----------------------------------------------------------

    fn clock_key(seq: u32, id: u32) -> (u32, u32) {
        if id >= SEQ_CLOCK_MIN {
            (seq, id)
        } else {
            (0, id)
        }
    }

    fn on_clock_snapshot(&mut self, seq: u32, buf: &[u8]) {
        let mut clocks: Vec<(u32, u64, bool, u64)> = Vec::new();
        let mut primary = None;
        for (num, val) in fields(buf) {
            match num {
                CS_CLOCKS => {
                    let (mut id, mut ts, mut inc, mut mult) = (0u32, 0u64, false, 1u64);
                    for (n, v) in fields(val.bytes()) {
                        match n {
                            CLK_ID => id = v.u32(),
                            CLK_TIMESTAMP => ts = v.u64(),
                            CLK_IS_INCREMENTAL => inc = v.bool(),
                            CLK_UNIT_MULTIPLIER_NS => mult = v.u64().max(1),
                            _ => {}
                        }
                    }
                    clocks.push((id, ts, inc, mult));
                }
                CS_PRIMARY_TRACE_CLOCK => primary = Some(val.u32()),
                _ => {}
            }
        }
        if clocks.is_empty() {
            return;
        }
        let has = |id: u32| clocks.iter().any(|c| c.0 == id);
        let trace_clock = match self.trace_clock {
            Some(t) if has(t) => t,
            _ => primary
                .filter(|p| has(*p))
                .or_else(|| has(CLOCK_BOOTTIME).then_some(CLOCK_BOOTTIME))
                .or_else(|| has(CLOCK_MONOTONIC).then_some(CLOCK_MONOTONIC))
                .unwrap_or(clocks[0].0),
        };
        if self.trace_clock.is_none() {
            self.trace_clock = Some(trace_clock);
        }
        let Some(t) = clocks.iter().find(|c| c.0 == trace_clock) else {
            return;
        };
        let trace_ns = i128::from(t.1) * i128::from(t.3);
        for (id, ts, inc, mult) in clocks {
            self.clocks.insert(
                Self::clock_key(seq, id),
                Clock {
                    mult_ns: mult,
                    incremental: inc,
                    acc: ts,
                    snap_units: ts,
                    snap_trace_ns: trace_ns,
                },
            );
        }
    }

    /// A packet timestamp in trace-clock nanoseconds. Without a snapshot
    /// for its clock the raw value is taken as ns already.
    fn packet_ns(&mut self, seq: u32, clock: Option<u32>, raw: u64) -> u64 {
        let clock_id = clock
            .or_else(|| self.seqs.get(&seq).and_then(|s| s.default_clock))
            .unwrap_or(CLOCK_BOOTTIME);
        let Some(c) = self.clocks.get_mut(&Self::clock_key(seq, clock_id)) else {
            return raw;
        };
        let units = if c.incremental {
            c.acc = c.acc.wrapping_add(raw);
            c.acc
        } else {
            raw
        };
        let ns = i128::from(units) * i128::from(c.mult_ns)
            - i128::from(c.snap_units) * i128::from(c.mult_ns)
            + c.snap_trace_ns;
        ns.clamp(0, i128::from(u64::MAX)) as u64
    }

    fn on_packet_defaults(&mut self, seq: u32, buf: &[u8]) {
        let s = self.seqs.entry(seq).or_default();
        for (num, val) in fields(buf) {
            match num {
                PDF_TIMESTAMP_CLOCK_ID => s.default_clock = Some(val.u32()),
                PDF_TRACK_EVENT_DEFAULTS => {
                    for (n, v) in fields(val.bytes()) {
                        if n == TED_TRACK_UUID {
                            s.default_track = Some(v.u64());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // ---- interned data ----------------------------------------------------

    fn on_interned_data(&mut self, seq: u32, buf: &[u8]) {
        let s = self.seqs.entry(seq).or_default();
        for (num, val) in fields(buf) {
            match num {
                ID_EVENT_CATEGORIES => {
                    let (iid, name) = iid_string(val.bytes());
                    s.categories.insert(iid, name);
                }
                ID_EVENT_NAMES => {
                    let (iid, name) = iid_string(val.bytes());
                    s.event_names.insert(iid, name);
                }
                ID_DEBUG_ANNOTATION_NAMES => {
                    let (iid, name) = iid_string(val.bytes());
                    s.debug_names.insert(iid, name);
                }
                ID_DEBUG_ANNOTATION_STRING_VALUES => {
                    let (iid, name) = iid_string(val.bytes());
                    s.debug_strings.insert(iid, name);
                }
                ID_FUNCTION_NAMES => {
                    let (iid, name) = iid_string(val.bytes());
                    s.function_names.insert(iid, name);
                }
                ID_MAPPING_PATHS => {
                    let (iid, name) = iid_string(val.bytes());
                    s.mapping_paths.insert(iid, name);
                }
                ID_HISTOGRAM_NAMES => {
                    let (iid, name) = iid_string(val.bytes());
                    s.histogram_names.insert(iid, name);
                }
                ID_FRAMES => {
                    let (mut iid, mut f, mut m, mut pc) = (0, 0, 0, 0);
                    for (n, v) in fields(val.bytes()) {
                        match n {
                            1 => iid = v.u64(),
                            2 => f = v.u64(),
                            3 => m = v.u64(),
                            4 => pc = v.u64(),
                            _ => {}
                        }
                    }
                    s.frames.insert(iid, (f, m, pc));
                }
                ID_CALLSTACKS => {
                    let mut iid = 0;
                    let mut ids = Vec::new();
                    for (n, v) in fields(val.bytes()) {
                        match n {
                            1 => iid = v.u64(),
                            2 => ids.extend(v.varints()),
                            _ => {}
                        }
                    }
                    s.callstacks.insert(iid, ids);
                }
                ID_MAPPINGS => {
                    let mut iid = 0;
                    let mut paths = Vec::new();
                    for (n, v) in fields(val.bytes()) {
                        match n {
                            1 => iid = v.u64(),
                            7 => paths.extend(v.varints()),
                            _ => {}
                        }
                    }
                    s.mappings.insert(iid, paths);
                }
                _ => {}
            }
        }
    }

    // ---- descriptors ------------------------------------------------------

    fn on_track_descriptor(&mut self, buf: &[u8], ing: &mut ChromeIngestor) {
        let mut uuid = 0u64;
        let mut track = Track::default();
        let mut static_name = None;
        let mut atrace_name = None;
        let mut thread_name = None;
        let mut process_name = None;
        let mut thread_sort = None;
        let mut process_sort = None;
        for (num, val) in fields(buf) {
            match num {
                TD_UUID => uuid = val.u64(),
                TD_NAME => track.name = val.string(),
                TD_STATIC_NAME => static_name = Some(val.string()),
                TD_ATRACE_NAME => atrace_name = Some(val.string()),
                TD_PARENT_UUID => track.parent = Some(val.u64()),
                TD_THREAD => {
                    let (mut pid, mut tid) = (0u32, 0u32);
                    for (n, v) in fields(val.bytes()) {
                        match n {
                            THD_PID => pid = v.i32() as u32,
                            THD_TID => tid = v.i64() as u32,
                            THD_NAME => thread_name = Some(v.string()),
                            THD_SORT_INDEX => thread_sort = Some(v.i32()),
                            _ => {}
                        }
                    }
                    track.thread = Some((pid, tid));
                }
                TD_PROCESS => {
                    let mut pid = 0u32;
                    let mut cmdline = None;
                    for (n, v) in fields(val.bytes()) {
                        match n {
                            PD_PID => pid = v.i32() as u32,
                            PD_NAME => process_name = Some(v.string()),
                            PD_CMDLINE => {
                                if cmdline.is_none() {
                                    cmdline = Some(v.string());
                                }
                            }
                            PD_SORT_INDEX => process_sort = Some(v.i32()),
                            _ => {}
                        }
                    }
                    if process_name.is_none() {
                        process_name = cmdline.map(|c| basename(&c));
                    }
                    track.process = Some(pid);
                }
                TD_CHROME_THREAD => {
                    for (n, v) in fields(val.bytes()) {
                        if n == CHROME_DESC_SORT_INDEX_THREAD {
                            thread_sort = Some(v.i32());
                        }
                    }
                }
                TD_CHROME_PROCESS => {
                    for (n, v) in fields(val.bytes()) {
                        if n == CHROME_DESC_SORT_INDEX_PROCESS {
                            process_sort = Some(v.i32());
                        }
                    }
                }
                TD_COUNTER => {
                    let mut desc = CounterDesc {
                        incremental: false,
                        multiplier: 1.0,
                        acc: 0.0,
                    };
                    for (n, v) in fields(val.bytes()) {
                        match n {
                            CD_UNIT_MULTIPLIER => {
                                let m = v.i64();
                                if m != 0 {
                                    desc.multiplier = m as f64;
                                }
                            }
                            CD_IS_INCREMENTAL => desc.incremental = v.bool(),
                            _ => {}
                        }
                    }
                    track.counter = Some(desc);
                }
                _ => {}
            }
        }
        if track.name.is_empty() {
            track.name = static_name.or(atrace_name).unwrap_or_default();
        }
        if let Some((pid, tid)) = track.thread {
            self.learn_pid(ing, tid, pid);
            let name = thread_name.as_deref().unwrap_or(&track.name);
            ing.name_thread(pid, tid, name);
            if let Some(s) = thread_sort {
                ing.thread_sort.insert((pid, tid), s);
            }
            if pid != 0 && !ing.process_names.contains_key(&pid) {
                ing.name_process(pid, &format!("pid {pid}"));
            }
        }
        if let Some(pid) = track.process {
            if let Some(name) = process_name.as_deref().filter(|n| !n.is_empty()) {
                ing.name_process(pid, name);
            } else if !track.name.is_empty() && !ing.process_names.contains_key(&pid) {
                ing.name_process(pid, &track.name);
            }
            if let Some(s) = process_sort {
                ing.process_sort.insert(pid, s);
            }
        }
        // A re-sent descriptor keeps a counter's running total; a track
        // re-described as something else starts over.
        if let (Some(old), Some(new)) = (
            self.tracks.get(&uuid).and_then(|t| t.counter),
            track.counter.as_mut(),
        ) {
            new.acc = old.acc;
        }
        self.tracks.insert(uuid, track);
    }

    fn on_legacy_thread_descriptor(&mut self, seq: u32, buf: &[u8], ing: &mut ChromeIngestor) {
        let (mut pid, mut tid, mut name, mut sort, mut ref_us) = (0u32, 0u32, None, None, 0i64);
        for (num, val) in fields(buf) {
            match num {
                THD_PID => pid = val.i32() as u32,
                THD_TID => tid = val.i64() as u32,
                THD_NAME => name = Some(val.string()),
                THD_SORT_INDEX => sort = Some(val.i32()),
                THD_REFERENCE_TIMESTAMP_US => ref_us = val.i64(),
                _ => {}
            }
        }
        self.learn_pid(ing, tid, pid);
        if let Some(n) = name {
            ing.name_thread(pid, tid, &n);
        }
        if let Some(s) = sort {
            ing.thread_sort.insert((pid, tid), s);
        }
        let s = self.seqs.entry(seq).or_default();
        s.legacy_thread = Some((pid, tid));
        s.legacy_ts_us = ref_us;
    }

    fn on_legacy_process_descriptor(&mut self, buf: &[u8], ing: &mut ChromeIngestor) {
        let (mut pid, mut name, mut cmdline, mut sort) = (0u32, None, None, None);
        for (num, val) in fields(buf) {
            match num {
                PD_PID => pid = val.i32() as u32,
                PD_NAME => name = Some(val.string()),
                PD_CMDLINE => {
                    if cmdline.is_none() {
                        cmdline = Some(val.string());
                    }
                }
                PD_SORT_INDEX => sort = Some(val.i32()),
                _ => {}
            }
        }
        if let Some(n) = name.or_else(|| cmdline.map(|c| basename(&c))) {
            ing.name_process(pid, &n);
        }
        if let Some(s) = sort {
            ing.process_sort.insert(pid, s);
        }
    }

    fn on_process_tree(&mut self, buf: &[u8], ing: &mut ChromeIngestor) {
        for (num, val) in fields(buf) {
            match num {
                PT_PROCESSES => {
                    let mut pid = 0u32;
                    let mut cmd = None;
                    for (n, v) in fields(val.bytes()) {
                        match n {
                            PTP_PID => pid = v.i32() as u32,
                            PTP_CMDLINE => {
                                if cmd.is_none() {
                                    cmd = Some(v.string());
                                }
                            }
                            _ => {}
                        }
                    }
                    if let Some(c) = cmd {
                        ing.name_process(pid, &basename(&c));
                    }
                    self.learn_pid(ing, pid, pid);
                }
                PT_THREADS => {
                    let (mut tid, mut tgid, mut name) = (0u32, 0u32, None);
                    for (n, v) in fields(val.bytes()) {
                        match n {
                            PTT_TID => tid = v.i32() as u32,
                            PTT_TGID => tgid = v.i32() as u32,
                            PTT_NAME => name = Some(v.string()),
                            _ => {}
                        }
                    }
                    if tgid == 0 {
                        tgid = tid;
                    }
                    self.learn_pid(ing, tid, tgid);
                    if let Some(n) = name.filter(|n| !n.is_empty()) {
                        ing.name_thread(tgid, tid, &n);
                    }
                }
                _ => {}
            }
        }
    }

    /// Follow `parent_uuid` up to the thread or process that owns a track.
    fn resolve(&self, uuid: u64) -> Target {
        let mut t = Target::default();
        let mut cur = Some(uuid);
        let mut first = true;
        for _ in 0..MAX_TRACK_DEPTH {
            let Some(id) = cur else { break };
            let Some(track) = self.tracks.get(&id) else {
                break;
            };
            if first {
                t.name = track.name.clone();
                t.on_thread = track.thread.is_some();
                first = false;
            }
            if let Some((pid, tid)) = track.thread {
                if t.pid.is_none() {
                    t.pid = Some(pid);
                    t.tid = Some(tid);
                }
                break;
            }
            if let Some(pid) = track.process {
                if t.pid.is_none() {
                    t.pid = Some(pid);
                }
                break;
            }
            cur = track.parent;
        }
        t
    }

    // ---- track events -----------------------------------------------------

    fn on_track_event(&mut self, pk: &Packet, buf: &[u8], ing: &mut ChromeIngestor) -> Vec<LiveEvent> {
        ing.stats.track_event += 1;
        let mut typ = 0u64;
        let mut track_uuid = None;
        let mut name: Option<String> = None;
        let mut name_iid = None;
        let mut cats: Vec<String> = Vec::new();
        let mut cat_iids: Vec<u64> = Vec::new();
        let mut counter: Option<f64> = None;
        let mut annotations: Vec<&[u8]> = Vec::new();
        let mut legacy: Option<&[u8]> = None;
        let mut flow_ids: Vec<u64> = Vec::new();
        let mut terminating: Vec<u64> = Vec::new();
        let mut ts_delta_us = None;
        let mut ts_abs_us = None;
        let mut histogram: Option<&[u8]> = None;
        for (num, val) in fields(buf) {
            match num {
                TE_TYPE => typ = val.u64(),
                TE_CHROME_HISTOGRAM_SAMPLE => histogram = Some(val.bytes()),
                TE_TRACK_UUID => track_uuid = Some(val.u64()),
                TE_NAME => name = Some(val.string()),
                TE_NAME_IID => name_iid = Some(val.u64()),
                TE_CATEGORIES => cats.push(val.string()),
                TE_CATEGORY_IIDS => cat_iids.extend(val.varints()),
                TE_COUNTER_VALUE => counter = Some(val.i64() as f64),
                TE_DOUBLE_COUNTER_VALUE => counter = Some(val.f64()),
                TE_DEBUG_ANNOTATIONS => annotations.push(val.bytes()),
                TE_LEGACY_EVENT => legacy = Some(val.bytes()),
                TE_FLOW_IDS => flow_ids.extend(val.fixed64s()),
                TE_FLOW_IDS_OLD => flow_ids.extend(val.varints()),
                TE_TERMINATING_FLOW_IDS => terminating.extend(val.fixed64s()),
                TE_TERMINATING_FLOW_IDS_OLD => terminating.extend(val.varints()),
                TE_TIMESTAMP_DELTA_US => ts_delta_us = Some(val.i64()),
                TE_TIMESTAMP_ABSOLUTE_US => ts_abs_us = Some(val.i64()),
                _ => {}
            }
        }

        let ts_ns = match pk.ts {
            Some(raw) => self.packet_ns(pk.seq, pk.clock, raw),
            None => {
                let s = self.seqs.entry(pk.seq).or_default();
                if let Some(a) = ts_abs_us {
                    s.legacy_ts_us = a;
                } else if let Some(d) = ts_delta_us {
                    s.legacy_ts_us = s.legacy_ts_us.saturating_add(d);
                }
                s.legacy_ts_us.max(0) as u64 * 1000
            }
        };

        let le = legacy.map(parse_legacy);
        let seq = self.seqs.entry(pk.seq).or_default();
        let name = name
            .or_else(|| name_iid.and_then(|i| seq.event_names.get(&i).cloned()))
            .or_else(|| {
                le.as_ref()
                    .and_then(|l| l.name_iid)
                    .and_then(|i| seq.event_names.get(&i).cloned())
            });
        // Chrome's histogram samples: one value series per histogram name.
        let histogram = histogram.map(|h| {
            let (mut hname, mut sample, mut iid) = (None, 0i64, None);
            for (n, v) in fields(h) {
                match n {
                    HS_NAME => hname = Some(v.string()),
                    HS_SAMPLE => sample = v.i64(),
                    HS_NAME_IID => iid = Some(v.u64()),
                    _ => {}
                }
            }
            let hname = hname
                .or_else(|| iid.and_then(|i| seq.histogram_names.get(&i).cloned()))
                .unwrap_or_else(|| "histogram".into());
            (hname, sample)
        });
        for iid in cat_iids {
            if let Some(c) = seq.categories.get(&iid) {
                cats.push(c.clone());
            }
        }
        let cat = if cats.is_empty() {
            None
        } else {
            Some(cats.join(","))
        };
        let args = self.annotations_value(pk.seq, &annotations);

        let uuid = track_uuid
            .or_else(|| self.seqs.get(&pk.seq).and_then(|s| s.default_track))
            .unwrap_or(0);
        let mut target = if uuid != 0 && self.tracks.contains_key(&uuid) {
            self.resolve(uuid)
        } else {
            // Chrome points legacy async events and `M` metadata at track
            // uuids it never describes (the async id doubles as the uuid).
            // They belong to the sequence's own thread; a typed slice on
            // such a track becomes an async lane of that thread's process.
            let seq = self.seqs.get(&pk.seq);
            let base = seq
                .and_then(|s| s.default_track)
                .filter(|d| self.tracks.contains_key(d))
                .map(|d| self.resolve(d))
                .or_else(|| {
                    seq.and_then(|s| s.legacy_thread).map(|(pid, tid)| Target {
                        pid: Some(pid),
                        tid: Some(tid),
                        on_thread: true,
                        name: String::new(),
                    })
                })
                .unwrap_or_default();
            if typ != 0 && uuid != 0 {
                Target {
                    pid: base.pid,
                    tid: None,
                    on_thread: false,
                    name: String::new(),
                }
            } else {
                base
            }
        };
        if let Some(l) = &le {
            if let Some(p) = l.pid_override {
                target.pid = Some(p);
            }
            if let Some(t) = l.tid_override {
                target.tid = Some(t);
                target.on_thread = true;
            }
        }

        let mut ev = ChromeEvent {
            name,
            cat,
            ph: None,
            ts: Some(ts_ns as f64),
            dur: None,
            pid: target.pid.map(|p| FlexId::Num(u64::from(p))),
            tid: target.tid.map(|t| FlexId::Num(u64::from(t))),
            id: None,
            id2: None,
            args,
            s: None,
            bind_id: None,
            flow_in: None,
            flow_out: None,
            sf: None,
            stack: None,
            tts: None,
            lane: None,
        };
        // The first flow id an event carries becomes its arrow. An id seen
        // before draws the incoming edge; every id re-opens for the next hop.
        if let Some(&id) = flow_ids.first() {
            ev.bind_id = Some(FlexId::Num(id));
            ev.flow_in = Some(true);
            ev.flow_out = Some(true);
        } else if let Some(&id) = terminating.first() {
            ev.bind_id = Some(FlexId::Num(id));
            ev.flow_in = Some(true);
        }
        // Chrome (2019–2022) writes typed slices whose ids, flows and
        // instant scope still live on `legacy_event`.
        if let Some(l) = &le {
            apply_legacy_fields(&mut ev, l);
        }
        if let Some((hname, sample)) = histogram {
            ev.ph = Some("C".into());
            ev.name = Some(hname);
            ev.pid = Some(FlexId::Num(u64::from(target.pid.unwrap_or(0))));
            let mut m = Map::new();
            m.insert("value".into(), Value::from(sample));
            ev.args = Some(Value::Object(m));
            return ing.ingest(ev);
        }

        let is_counter = typ == TYPE_COUNTER
            || (typ == 0 && le.is_none() && self.tracks.get(&uuid).is_some_and(|t| t.counter.is_some()));
        if is_counter {
            let Some(mut v) = counter else {
                return Vec::new();
            };
            if let Some(desc) = self.tracks.get_mut(&uuid).and_then(|t| t.counter.as_mut()) {
                v *= desc.multiplier;
                if desc.incremental {
                    desc.acc += v;
                    v = desc.acc;
                }
            }
            let label = if target.name.is_empty() {
                ev.name.clone().unwrap_or_else(|| format!("counter {uuid:#x}"))
            } else {
                target.name.clone()
            };
            ev.ph = Some("C".into());
            ev.name = Some(label);
            ev.pid = Some(FlexId::Num(u64::from(target.pid.unwrap_or(0))));
            let mut m = Map::new();
            m.insert("value".into(), Value::from(v));
            ev.args = Some(Value::Object(m));
            return ing.ingest(ev);
        }

        match typ {
            TYPE_SLICE_BEGIN | TYPE_SLICE_END | TYPE_INSTANT => {
                let ph = if target.on_thread {
                    match typ {
                        TYPE_SLICE_BEGIN => "B",
                        TYPE_SLICE_END => "E",
                        _ => "I",
                    }
                } else if target.pid.is_some() || typ != TYPE_INSTANT {
                    // Process, global and named child tracks: one async
                    // lane per track, slices nest in packet order. Chrome
                    // mints one unnamed track per async id; those lane by
                    // event name, as the JSON path does. A track uuid is
                    // trace-wide, so a begin and an end written by two
                    // processes (Chrome's input latency) still pair.
                    ev.id2 = Some(Id2 {
                        local: None,
                        global: Some(FlexId::Num(uuid)),
                    });
                    if !target.name.is_empty() {
                        ev.lane = Some(target.name.clone());
                    }
                    if ev.pid.is_none() {
                        ev.pid = Some(FlexId::Num(0));
                    }
                    match typ {
                        TYPE_SLICE_BEGIN => "b",
                        TYPE_SLICE_END => "e",
                        _ => "n",
                    }
                } else {
                    ev.s = Some("g".into());
                    "I"
                };
                ev.ph = Some(ph.into());
                ing.ingest(ev)
            }
            _ => match le.as_ref().and_then(|l| l.phase) {
                Some(phase) => {
                    ev.ph = Some(phase.to_string());
                    if ev.pid.is_none() {
                        ev.pid = Some(FlexId::Num(u64::from(target.pid.unwrap_or(0))));
                    }
                    ing.ingest(ev)
                }
                None => {
                    ing.stats.skipped_other += 1;
                    Vec::new()
                }
            },
        }
    }

    fn annotations_value(&self, seq: u32, annotations: &[&[u8]]) -> Option<Value> {
        if annotations.is_empty() {
            return None;
        }
        let s = self.seqs.get(&seq);
        let mut m = Map::new();
        for a in annotations {
            let (name, v) = annotation(s, a, 0);
            let key = if name.is_empty() {
                format!("arg{}", m.len())
            } else {
                name
            };
            m.insert(key, v);
        }
        Some(Value::Object(m))
    }

    // ---- ftrace -----------------------------------------------------------

    fn on_ftrace_bundle(&mut self, buf: &[u8], ing: &mut ChromeIngestor) -> Vec<LiveEvent> {
        let mut cpu = 0u32;
        let mut events: Vec<&[u8]> = Vec::new();
        let mut compact = None;
        for (num, val) in fields(buf) {
            match num {
                FB_CPU => cpu = val.u32(),
                FB_EVENT => events.push(val.bytes()),
                FB_COMPACT_SCHED => compact = Some(val.bytes()),
                _ => {}
            }
        }
        let mut out = Vec::new();
        let mut direct = Vec::new();
        for e in events {
            let (mut ts, mut pid) = (0u64, 0u32);
            let mut payload = None;
            for (n, v) in fields(e) {
                match n {
                    FE_TIMESTAMP => ts = v.u64(),
                    FE_PID => pid = v.u32(),
                    FE_PRINT | FE_SCHED_SWITCH | FE_SCHED_WAKING => payload = Some((n, v.bytes())),
                    _ => {}
                }
            }
            let Some((n, b)) = payload else { continue };
            match n {
                FE_SCHED_SWITCH => {
                    let (mut prev_comm, mut prev_pid, mut prev_state, mut next_comm, mut next_pid) =
                        (None, 0u32, 0i64, None, 0u32);
                    for (k, v) in fields(b) {
                        match k {
                            SS_PREV_COMM => prev_comm = Some(v.string()),
                            SS_PREV_PID => prev_pid = v.i32() as u32,
                            SS_PREV_STATE => prev_state = v.i64(),
                            SS_NEXT_COMM => next_comm = Some(v.string()),
                            SS_NEXT_PID => next_pid = v.i32() as u32,
                            _ => {}
                        }
                    }
                    self.sched_switch(
                        &mut direct,
                        ing,
                        cpu,
                        ts,
                        prev_pid,
                        prev_state,
                        next_pid,
                        prev_comm.as_deref(),
                        next_comm.as_deref(),
                    );
                }
                FE_SCHED_WAKING => {
                    let (mut comm, mut wpid) = (None, 0u32);
                    for (k, v) in fields(b) {
                        match k {
                            SW_COMM => comm = Some(v.string()),
                            SW_PID => wpid = v.i32() as u32,
                            _ => {}
                        }
                    }
                    self.sched_waking(&mut direct, ing, ts, wpid, comm.as_deref());
                }
                FE_PRINT => {
                    let mut text = None;
                    for (k, v) in fields(b) {
                        if k == PR_BUF {
                            text = Some(v.string());
                        }
                    }
                    if let Some(t) = text {
                        // `B|tgid|name`: the mark carries the process id
                        // the sched rows for this tid are waiting for.
                        if let Some(tgid) = atrace_tgid(&t) {
                            self.learn_pid(ing, pid, tgid);
                        }
                        out.extend(ing.ingest_atrace_mark(&t, ts as f64, pid));
                    }
                }
                _ => {}
            }
        }
        if let Some(c) = compact {
            self.compact_sched(&mut direct, ing, cpu, c);
        }
        ing.note_direct_events(&direct);
        out.extend(direct);
        out
    }

    fn compact_sched(&mut self, out: &mut Vec<LiveEvent>, ing: &mut ChromeIngestor, cpu: u32, buf: &[u8]) {
        let mut sw_ts: Vec<u64> = Vec::new();
        let mut sw_state: Vec<i64> = Vec::new();
        let mut sw_next: Vec<u32> = Vec::new();
        let mut sw_comm: Vec<u32> = Vec::new();
        let mut wk_ts: Vec<u64> = Vec::new();
        let mut wk_pid: Vec<u32> = Vec::new();
        let mut wk_comm: Vec<u32> = Vec::new();
        let mut table: Vec<String> = Vec::new();
        for (num, val) in fields(buf) {
            match num {
                CS_SWITCH_TIMESTAMP => sw_ts.extend(val.varints()),
                CS_SWITCH_PREV_STATE => sw_state.extend(val.varints().map(|v| v as i64)),
                CS_SWITCH_NEXT_PID => sw_next.extend(val.varints().map(|v| v as u32)),
                CS_SWITCH_NEXT_COMM_INDEX => sw_comm.extend(val.varints().map(|v| v as u32)),
                CS_WAKING_TIMESTAMP => wk_ts.extend(val.varints()),
                CS_WAKING_PID => wk_pid.extend(val.varints().map(|v| v as u32)),
                CS_WAKING_COMM_INDEX => wk_comm.extend(val.varints().map(|v| v as u32)),
                CS_INTERN_TABLE => table.push(val.string()),
                _ => {}
            }
        }
        // Timestamps are delta-encoded: the first is absolute, each next one
        // is relative to its predecessor. Switch and waking streams are
        // independent, so both are replayed in their own order; the wakings
        // are applied first because a waking precedes the switch it
        // enables within a bundle's window.
        let mut t = 0u64;
        for (i, delta) in wk_ts.iter().enumerate() {
            t = t.wrapping_add(*delta);
            let pid = wk_pid.get(i).copied().unwrap_or(0);
            let comm = wk_comm.get(i).and_then(|&c| table.get(c as usize)).map(String::as_str);
            self.sched_waking(out, ing, t, pid, comm);
        }
        let mut t = 0u64;
        for (i, delta) in sw_ts.iter().enumerate() {
            t = t.wrapping_add(*delta);
            let next = sw_next.get(i).copied().unwrap_or(0);
            let state = sw_state.get(i).copied().unwrap_or(0);
            let comm = sw_comm.get(i).and_then(|&c| table.get(c as usize)).map(String::as_str);
            let prev = self.on_cpu.get(&cpu).map(|c| c.tid).unwrap_or(0);
            self.sched_switch(out, ing, cpu, t, prev, state, next, None, comm);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn sched_switch(
        &mut self,
        out: &mut Vec<LiveEvent>,
        ing: &mut ChromeIngestor,
        cpu: u32,
        ts: u64,
        prev_pid: u32,
        prev_state: i64,
        next_pid: u32,
        prev_comm: Option<&str>,
        next_comm: Option<&str>,
    ) {
        ing.stats.sched += 1;
        self.last_ftrace_ns = self.last_ftrace_ns.max(ts);
        if let Some(cur) = self.on_cpu.remove(&cpu) {
            if cur.tid != 0 && ts > cur.since {
                self.push_sched(out, cpu, cur, ts);
            }
        }
        if prev_pid != 0 {
            self.name_from_comm(ing, prev_pid, prev_comm);
            self.off_cpu.insert(prev_pid, (ts, state_code(prev_state)));
        }
        if next_pid != 0 {
            self.name_from_comm(ing, next_pid, next_comm);
            if let Some((since, state)) = self.off_cpu.remove(&next_pid) {
                if ts > since {
                    self.push_state(out, next_pid, since, ts, state);
                }
            }
        }
        self.on_cpu.insert(cpu, OnCpu { tid: next_pid, since: ts });
    }

    fn sched_waking(&mut self, out: &mut Vec<LiveEvent>, ing: &mut ChromeIngestor, ts: u64, tid: u32, comm: Option<&str>) {
        if tid == 0 {
            return;
        }
        self.last_ftrace_ns = self.last_ftrace_ns.max(ts);
        self.name_from_comm(ing, tid, comm);
        if let Some((since, state)) = self.off_cpu.get(&tid).copied() {
            if state == thread_state::RUNNABLE {
                return;
            }
            if ts > since {
                self.push_state(out, tid, since, ts, state);
            }
            self.off_cpu.insert(tid, (ts, thread_state::RUNNABLE));
        }
    }

    fn pid_of(&self, tid: u32) -> u32 {
        self.pid_of_tid.get(&tid).copied().unwrap_or(tid)
    }

    /// A sched comm names a thread that has no real name yet. The `tid N`
    /// placeholder a first state slice minted does not count as one, and a
    /// descriptor or process-tree name is never overwritten by a comm.
    fn name_from_comm(&mut self, ing: &mut ChromeIngestor, tid: u32, comm: Option<&str>) {
        let Some(c) = comm else { return };
        self.comms.insert(tid, c.to_string());
        // Until the process is known, naming (tid, tid) would mint a
        // phantom process; the comm is applied when the pid arrives, or
        // at the end.
        let Some(pid) = self.pid_of_tid.get(&tid).copied() else {
            return;
        };
        let placeholder = ing
            .thread_names
            .get(&(pid, tid))
            .is_none_or(|n| n.starts_with("tid "));
        if placeholder {
            ing.name_thread(pid, tid, c);
        }
    }

    fn push_sched(&mut self, out: &mut Vec<LiveEvent>, cpu: u32, cur: OnCpu, end: u64) {
        out.push(LiveEvent {
            start_ns: cur.since,
            duration_ns: end - cur.since,
            tid: cur.tid,
            pid: self.pid_of(cur.tid),
            kind: kind::SCHEDULING_SLICE,
            depth: 0,
            extra: cpu.min(255) as u8,
            _pad: 0,
            name_id: 0,
        });
        self.push_state(out, cur.tid, cur.since, end, thread_state::RUNNING);
    }

    fn push_state(&mut self, out: &mut Vec<LiveEvent>, tid: u32, since: u64, end: u64, state: u8) {
        let ev = LiveEvent {
            start_ns: since,
            duration_ns: end - since,
            tid,
            pid: self.pid_of(tid),
            kind: kind::THREAD_STATE,
            depth: 0,
            extra: state,
            _pad: 0,
            name_id: 0,
        };
        if self.pid_of_tid.contains_key(&tid) {
            out.push(ev);
        } else {
            self.pending_states.entry(tid).or_default().push(ev);
        }
    }

    // ---- chrome_events bundle --------------------------------------------

    fn on_chrome_events(&mut self, buf: &[u8], ing: &mut ChromeIngestor) -> Vec<LiveEvent> {
        let mut out = Vec::new();
        let mut table: HashMap<u32, String> = HashMap::new();
        let mut events: Vec<&[u8]> = Vec::new();
        for (num, val) in fields(buf) {
            match num {
                CE_STRING_TABLE => {
                    let (mut v, mut i) = (String::new(), 0u32);
                    for (n, x) in fields(val.bytes()) {
                        match n {
                            CST_VALUE => v = x.string(),
                            CST_INDEX => i = x.u32(),
                            _ => {}
                        }
                    }
                    table.insert(i, v);
                }
                CE_TRACE_EVENTS => events.push(val.bytes()),
                CE_LEGACY_FTRACE_OUTPUT => out.extend(ing.ingest_systrace_text(&val.string())),
                CE_LEGACY_JSON_TRACE => {
                    for (n, x) in fields(val.bytes()) {
                        if n == CLJ_DATA {
                            out.extend(nested_json(ing, x.bytes()));
                        }
                    }
                }
                _ => {}
            }
        }
        for e in events {
            if let Some(ev) = chrome_trace_event(e, &table) {
                out.extend(ing.ingest(ev));
            }
        }
        out
    }

    // ---- perf samples -----------------------------------------------------

    fn on_perf_sample(&mut self, pk: &Packet, buf: &[u8], ing: &mut ChromeIngestor) -> Vec<LiveEvent> {
        let (mut pid, mut tid, mut callstack) = (0u32, 0u32, None);
        for (num, val) in fields(buf) {
            match num {
                PS_PID => pid = val.u32(),
                PS_TID => tid = val.u32(),
                PS_CALLSTACK_IID => callstack = Some(val.u64()),
                PS_CPU => {}
                _ => {}
            }
        }
        let Some(cs) = callstack else {
            return Vec::new();
        };
        let Some(raw) = pk.ts else {
            return Vec::new();
        };
        let ts_ns = self.packet_ns(pk.seq, pk.clock, raw);
        let Some(seq) = self.seqs.get(&pk.seq) else {
            return Vec::new();
        };
        let Some(frame_ids) = seq.callstacks.get(&cs) else {
            return Vec::new();
        };
        let mut stack = Vec::with_capacity(frame_ids.len());
        for fid in frame_ids {
            let Some(&(fname, mapping, rel_pc)) = seq.frames.get(fid) else {
                stack.push(format!("frame {fid:#x}"));
                continue;
            };
            if let Some(n) = seq.function_names.get(&fname).filter(|n| !n.is_empty()) {
                stack.push(n.clone());
                continue;
            }
            let path = seq
                .mappings
                .get(&mapping)
                .and_then(|ids| ids.last())
                .and_then(|id| seq.mapping_paths.get(id))
                .map(|p| basename(p));
            stack.push(match path {
                Some(p) => format!("{p}+{rel_pc:#x}"),
                None => format!("{rel_pc:#x}"),
            });
        }
        if stack.is_empty() {
            return Vec::new();
        }
        if pid == 0 {
            pid = self.pid_of(tid);
        }
        let ev = ChromeEvent {
            name: None,
            cat: None,
            ph: Some("P".into()),
            ts: Some(ts_ns as f64),
            dur: None,
            pid: Some(FlexId::Num(u64::from(pid))),
            tid: Some(FlexId::Num(u64::from(tid))),
            id: None,
            id2: None,
            args: None,
            s: None,
            bind_id: None,
            flow_in: None,
            flow_out: None,
            sf: None,
            stack: Some(stack),
            tts: None,
            lane: None,
        };
        ing.ingest(ev)
    }
}

struct Legacy {
    name_iid: Option<u64>,
    phase: Option<char>,
    duration_us: Option<i64>,
    unscoped_id: Option<u64>,
    local_id: Option<u64>,
    global_id: Option<u64>,
    bind_id: Option<u64>,
    flow_direction: u64,
    instant_scope: u64,
    pid_override: Option<u32>,
    tid_override: Option<u32>,
}

fn parse_legacy(buf: &[u8]) -> Legacy {
    let mut l = Legacy {
        name_iid: None,
        phase: None,
        duration_us: None,
        unscoped_id: None,
        local_id: None,
        global_id: None,
        bind_id: None,
        flow_direction: 0,
        instant_scope: 0,
        pid_override: None,
        tid_override: None,
    };
    for (num, val) in fields(buf) {
        match num {
            LE_NAME_IID => l.name_iid = Some(val.u64()),
            LE_PHASE => l.phase = char::from_u32(val.i32() as u32).filter(|c| c.is_ascii_graphic()),
            LE_DURATION_US => l.duration_us = Some(val.i64()),
            LE_UNSCOPED_ID => l.unscoped_id = Some(val.u64()),
            LE_LOCAL_ID => l.local_id = Some(val.u64()),
            LE_GLOBAL_ID => l.global_id = Some(val.u64()),
            LE_BIND_ID => l.bind_id = Some(val.u64()),
            LE_FLOW_DIRECTION => l.flow_direction = val.u64(),
            LE_INSTANT_SCOPE => l.instant_scope = val.u64(),
            LE_PID_OVERRIDE => l.pid_override = Some(val.i32() as u32),
            LE_TID_OVERRIDE => l.tid_override = Some(val.i32() as u32),
            _ => {}
        }
    }
    l
}

/// Chrome's pre-TrackEvent fields ride along verbatim: the JSON ingestor
/// already knows every id, scope and flow shape. The phase is applied by
/// the caller, only when the event has no `type`.
fn apply_legacy_fields(ev: &mut ChromeEvent, l: &Legacy) {
    if let Some(d) = l.duration_us {
        ev.dur = Some(d.max(0) as f64 * 1000.0);
    }
    if let Some(id) = l.unscoped_id {
        ev.id = Some(FlexId::Num(id));
    } else if l.local_id.is_some() || l.global_id.is_some() {
        ev.id2 = Some(Id2 {
            local: l.local_id.map(FlexId::Num),
            global: l.global_id.map(FlexId::Num),
        });
    }
    ev.s = match l.instant_scope {
        1 => Some("g".into()),
        2 => Some("p".into()),
        3 => Some("t".into()),
        _ => None,
    };
    if let (Some(b), None) = (l.bind_id, &ev.bind_id) {
        ev.bind_id = Some(FlexId::Num(b));
        ev.flow_in = Some(l.flow_direction & 1 != 0);
        ev.flow_out = Some(l.flow_direction & 2 != 0);
    }
}

/// `(iid, name)` of an `EventName` / `InternedString`-shaped message.
fn iid_string(buf: &[u8]) -> (u64, String) {
    let mut iid = 0;
    let mut s = String::new();
    for (n, v) in fields(buf) {
        match n {
            1 => iid = v.u64(),
            2 => s = v.string(),
            _ => {}
        }
    }
    (iid, s)
}

fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// The process id an atrace mark names: `B|1204|name`, `E|1204`, `C|1204|k|v`.
fn atrace_tgid(mark: &str) -> Option<u32> {
    let mut parts = mark.trim().split('|');
    let ph = parts.next()?;
    if ph.len() != 1 {
        return None;
    }
    parts.next()?.trim().parse::<u32>().ok().filter(|p| *p != 0)
}

/// Linux `prev_state` bits → the viewer's thread-state code.
fn state_code(prev_state: i64) -> u8 {
    let s = prev_state & 0x7ff;
    if s & 0x01 != 0 {
        thread_state::INTERRUPTIBLE_SLEEP
    } else if s & 0x02 != 0 {
        thread_state::UNINTERRUPTIBLE_SLEEP
    } else if s & 0x04 != 0 {
        thread_state::STOPPED
    } else if s & 0x08 != 0 {
        thread_state::TRACED
    } else if s & 0x10 != 0 {
        thread_state::DEAD
    } else if s & 0x20 != 0 {
        thread_state::ZOMBIE
    } else if s & 0x40 != 0 {
        thread_state::PARKED
    } else if s & 0x80 != 0 {
        thread_state::IDLE
    } else {
        thread_state::RUNNABLE
    }
}

fn annotation(seq: Option<&Seq>, buf: &[u8], depth: usize) -> (String, Value) {
    let mut name = String::new();
    let mut value = Value::Null;
    let mut dict = Map::new();
    let mut array = Vec::new();
    for (num, val) in fields(buf) {
        match num {
            DA_NAME => name = val.string(),
            DA_NAME_IID => {
                if let Some(n) = seq.and_then(|s| s.debug_names.get(&val.u64())) {
                    name = n.clone();
                }
            }
            DA_BOOL => value = Value::Bool(val.bool()),
            DA_UINT => value = Value::from(val.u64()),
            DA_INT => value = Value::from(val.i64()),
            DA_DOUBLE => value = Value::from(val.f64()),
            DA_STRING => value = Value::String(val.string()),
            DA_STRING_IID => {
                if let Some(s) = seq.and_then(|s| s.debug_strings.get(&val.u64())) {
                    value = Value::String(s.clone());
                }
            }
            DA_POINTER => value = Value::String(format!("{:#x}", val.u64())),
            DA_LEGACY_JSON => {
                value = serde_json::from_slice(val.bytes()).unwrap_or_else(|_| Value::String(val.string()));
            }
            DA_NESTED => {
                if depth < MAX_ANNOTATION_DEPTH {
                    value = nested_value(val.bytes(), depth + 1);
                }
            }
            DA_DICT_ENTRIES => {
                if depth < MAX_ANNOTATION_DEPTH {
                    let (k, v) = annotation(seq, val.bytes(), depth + 1);
                    let key = if k.is_empty() {
                        format!("arg{}", dict.len())
                    } else {
                        k
                    };
                    dict.insert(key, v);
                }
            }
            DA_ARRAY_VALUES => {
                if depth < MAX_ANNOTATION_DEPTH {
                    array.push(annotation(seq, val.bytes(), depth + 1).1);
                }
            }
            DA_PROTO_VALUE => value = Value::String("<proto>".into()),
            _ => {}
        }
    }
    if !dict.is_empty() {
        value = Value::Object(dict);
    } else if !array.is_empty() {
        value = Value::Array(array);
    }
    (name, value)
}

fn nested_value(buf: &[u8], depth: usize) -> Value {
    let mut typ = 0u64;
    let mut keys = Vec::new();
    let mut dict_vals = Vec::new();
    let mut arr = Vec::new();
    let mut scalar = Value::Null;
    for (num, val) in fields(buf) {
        match num {
            NV_TYPE => typ = val.u64(),
            NV_DICT_KEYS => keys.push(val.string()),
            NV_DICT_VALUES => {
                if depth < MAX_ANNOTATION_DEPTH {
                    dict_vals.push(nested_value(val.bytes(), depth + 1));
                }
            }
            NV_ARRAY_VALUES => {
                if depth < MAX_ANNOTATION_DEPTH {
                    arr.push(nested_value(val.bytes(), depth + 1));
                }
            }
            NV_INT => scalar = Value::from(val.i64()),
            NV_DOUBLE => scalar = Value::from(val.f64()),
            NV_BOOL => scalar = Value::Bool(val.bool()),
            NV_STRING => scalar = Value::String(val.string()),
            _ => {}
        }
    }
    match typ {
        1 => Value::Object(keys.into_iter().zip(dict_vals).collect()),
        2 => Value::Array(arr),
        _ => scalar,
    }
}

/// A `ChromeTraceEvent` (the proto twin of one JSON event, µs timestamps).
fn chrome_trace_event(buf: &[u8], table: &HashMap<u32, String>) -> Option<ChromeEvent> {
    let mut ev = ChromeEvent {
        name: None,
        cat: None,
        ph: None,
        ts: None,
        dur: None,
        pid: None,
        tid: None,
        id: None,
        id2: None,
        args: None,
        s: None,
        bind_id: None,
        flow_in: None,
        flow_out: None,
        sf: None,
        stack: None,
        tts: None,
        lane: None,
    };
    let mut flags = 0u32;
    let mut id = None;
    let mut args = Map::new();
    for (num, val) in fields(buf) {
        match num {
            CT_NAME => ev.name = Some(val.string()),
            CT_NAME_INDEX => ev.name = table.get(&val.u32()).cloned(),
            CT_CATEGORY => ev.cat = Some(val.string()),
            CT_CATEGORY_INDEX => ev.cat = table.get(&val.u32()).cloned(),
            CT_TIMESTAMP => ev.ts = Some(val.i64() as f64 * 1000.0),
            CT_DURATION => ev.dur = Some(val.i64().max(0) as f64 * 1000.0),
            CT_PHASE => ev.ph = char::from_u32(val.i32() as u32).map(|c| c.to_string()),
            CT_THREAD_ID => ev.tid = Some(FlexId::Num(val.i32() as u32 as u64)),
            CT_PROCESS_ID => ev.pid = Some(FlexId::Num(val.i32() as u32 as u64)),
            CT_SCOPE => ev.s = Some(val.string()),
            CT_ID => id = Some(val.u64()),
            CT_FLAGS => flags = val.u32(),
            CT_BIND_ID => ev.bind_id = Some(FlexId::Num(val.u64())),
            CT_ARGS => {
                let (mut name, mut v) = (String::new(), Value::Null);
                for (n, x) in fields(val.bytes()) {
                    match n {
                        CTA_NAME => name = x.string(),
                        CTA_BOOL => v = Value::Bool(x.bool()),
                        CTA_UINT => v = Value::from(x.u64()),
                        CTA_INT => v = Value::from(x.i64()),
                        CTA_DOUBLE => v = Value::from(x.f64()),
                        CTA_STRING => v = Value::String(x.string()),
                        CTA_POINTER => v = Value::String(format!("{:#x}", x.u64())),
                        CTA_JSON => {
                            v = serde_json::from_slice(x.bytes()).unwrap_or_else(|_| Value::String(x.string()));
                        }
                        _ => {}
                    }
                }
                args.insert(name, v);
            }
            _ => {}
        }
    }
    ev.ph.as_ref()?;
    if let Some(i) = id {
        if flags & CT_FLAG_HAS_GLOBAL_ID != 0 {
            ev.id2 = Some(Id2 {
                local: None,
                global: Some(FlexId::Num(i)),
            });
        } else if flags & CT_FLAG_HAS_LOCAL_ID != 0 {
            ev.id2 = Some(Id2 {
                local: Some(FlexId::Num(i)),
                global: None,
            });
        } else {
            ev.id = Some(FlexId::Num(i));
        }
    }
    if ev.s.is_none() && ev.ph.as_deref() == Some("I") {
        ev.s = Some(
            match (flags >> CT_FLAG_SCOPE_SHIFT) & 3 {
                0 => "g",
                1 => "p",
                _ => "t",
            }
            .into(),
        );
    }
    if ev.bind_id.is_some() {
        ev.flow_in = Some(flags & CT_FLAG_FLOW_IN != 0);
        ev.flow_out = Some(flags & CT_FLAG_FLOW_OUT != 0);
    }
    if !args.is_empty() {
        ev.args = Some(Value::Object(args));
    }
    Some(ev)
}

/// A JSON trace embedded in a proto packet: run it through the JSON path
/// in µs, then put the ingestor back in ns.
fn nested_json(ing: &mut ChromeIngestor, data: &[u8]) -> Vec<LiveEvent> {
    let saved = ing.unit;
    ing.unit = TimeUnit::Micros;
    let mut stream = ChromeStream::default();
    stream.push(data);
    stream.finish_input();
    let mut out = Vec::new();
    loop {
        let batch = stream.pump(ing, 64 * 1024);
        if batch.is_empty() {
            break;
        }
        out.extend(batch);
    }
    ing.unit = saved;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::Encoder;
    use crate::{ingest_collect, ArgKey, TID_ASYNC_BASE, TID_COUNTER_BASE};
    use orbit_live_event::kind;

    fn packet(p: &Encoder) -> Vec<u8> {
        let mut t = Encoder::new();
        t.msg(1, p);
        t.buf
    }

    fn thread_track(uuid: u64, pid: i32, tid: i64, name: &str) -> Vec<u8> {
        let mut th = Encoder::new();
        th.i32(THD_PID, pid).i64(THD_TID, tid).string(THD_NAME, name);
        let mut td = Encoder::new();
        td.varint(TD_UUID, uuid).msg(TD_THREAD, &th);
        let mut p = Encoder::new();
        p.msg(P_TRACK_DESCRIPTOR, &td);
        packet(&p)
    }

    fn process_track(uuid: u64, pid: i32, name: &str) -> Vec<u8> {
        let mut pd = Encoder::new();
        pd.i32(PD_PID, pid).string(PD_NAME, name);
        let mut td = Encoder::new();
        td.varint(TD_UUID, uuid).msg(TD_PROCESS, &pd);
        let mut p = Encoder::new();
        p.msg(P_TRACK_DESCRIPTOR, &td);
        packet(&p)
    }

    fn named_track(uuid: u64, parent: u64, name: &str) -> Vec<u8> {
        let mut td = Encoder::new();
        td.varint(TD_UUID, uuid).string(TD_NAME, name).varint(TD_PARENT_UUID, parent);
        let mut p = Encoder::new();
        p.msg(P_TRACK_DESCRIPTOR, &td);
        packet(&p)
    }

    fn te(ts: u64, seq: u32, f: impl FnOnce(&mut Encoder)) -> Vec<u8> {
        let mut e = Encoder::new();
        f(&mut e);
        let mut p = Encoder::new();
        p.varint(P_TIMESTAMP, ts).varint(P_SEQUENCE_ID, seq as u64).msg(P_TRACK_EVENT, &e);
        packet(&p)
    }

    fn collect(bytes: &[u8]) -> (ChromeIngestor, Vec<LiveEvent>) {
        ingest_collect(bytes).expect("ingest")
    }

    #[test]
    fn thread_slices_nest_and_name() {
        let mut trace = thread_track(11, 7, 70, "Main");
        trace.extend(te(1000, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_BEGIN).varint(TE_TRACK_UUID, 11).string(TE_NAME, "outer");
        }));
        trace.extend(te(1100, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_BEGIN).varint(TE_TRACK_UUID, 11).string(TE_NAME, "inner");
        }));
        trace.extend(te(1300, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_END).varint(TE_TRACK_UUID, 11);
        }));
        trace.extend(te(2000, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_END).varint(TE_TRACK_UUID, 11);
        }));
        trace.extend(te(2500, 1, |e| {
            e.varint(TE_TYPE, TYPE_INSTANT).varint(TE_TRACK_UUID, 11).string(TE_NAME, "tick");
        }));
        let (ing, evs) = collect(&trace);
        assert_eq!(ing.thread_names.get(&(7, 70)).map(String::as_str), Some("Main"));
        let name = |e: &LiveEvent| ing.intern.get(e.name_id).unwrap_or("").to_string();
        let scopes: Vec<_> = evs.iter().filter(|e| e.kind == kind::API_SCOPE).collect();
        assert_eq!(scopes.len(), 3);
        let inner = scopes.iter().find(|e| name(e) == "inner").unwrap();
        assert_eq!((inner.start_ns, inner.duration_ns, inner.depth, inner.pid, inner.tid), (1100, 200, 1, 7, 70));
        let outer = scopes.iter().find(|e| name(e) == "outer").unwrap();
        assert_eq!((outer.start_ns, outer.duration_ns, outer.depth), (1000, 1000, 0));
        let tick = scopes.iter().find(|e| name(e) == "tick").unwrap();
        assert_eq!((tick.start_ns, tick.duration_ns), (2500, 1));
        assert_eq!(ing.stats.packets, 6);
        assert_eq!(ing.stats.track_event, 5);
        assert_eq!(ing.stats.skipped_packets, 0);
    }

    #[test]
    fn interned_names_and_incremental_state() {
        let mut names = Encoder::new();
        let mut n1 = Encoder::new();
        n1.varint(1, 1).string(2, "Interned");
        names.msg(ID_EVENT_NAMES, &n1);
        let mut cat = Encoder::new();
        cat.varint(1, 5).string(2, "cat");
        names.msg(ID_EVENT_CATEGORIES, &cat);
        let mut dn = Encoder::new();
        dn.varint(1, 3).string(2, "key");
        names.msg(ID_DEBUG_ANNOTATION_NAMES, &dn);

        let mut trace = thread_track(11, 7, 70, "Main");
        let mut e = Encoder::new();
        let mut ann = Encoder::new();
        ann.varint(DA_NAME_IID, 3).varint(DA_INT, 42);
        e.varint(TE_TYPE, TYPE_SLICE_BEGIN)
            .varint(TE_TRACK_UUID, 11)
            .varint(TE_NAME_IID, 1)
            .packed(TE_CATEGORY_IIDS, &[5])
            .msg(TE_DEBUG_ANNOTATIONS, &ann);
        let mut p = Encoder::new();
        p.varint(P_TIMESTAMP, 10)
            .varint(P_SEQUENCE_ID, 2)
            .varint(P_SEQUENCE_FLAGS, SEQ_INCREMENTAL_STATE_CLEARED)
            .msg(P_INTERNED_DATA, &names)
            .msg(P_TRACK_EVENT, &e);
        trace.extend(packet(&p));
        trace.extend(te(20, 2, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_END).varint(TE_TRACK_UUID, 11);
        }));
        let (ing, evs) = collect(&trace);
        assert_eq!(evs.len(), 1);
        assert_eq!(ing.intern.get(evs[0].name_id), Some("Interned"));
        let args = ing.intern.get(*ing.args.get(&ArgKey::from_event(evs[0])).unwrap()).unwrap();
        assert_eq!(args, r#"{"key":42}"#);
    }

    #[test]
    fn incremental_sequence_clock_converts_to_trace_clock() {
        // Chrome: sequence clock 64 is incremental in µs, snapshotted
        // against MONOTONIC (3). ts 1000 µs = 1_000_000 ns after the
        // snapshot's 5_000_000 ns.
        let mut snap = Encoder::new();
        let mut mono = Encoder::new();
        mono.varint(CLK_ID, CLOCK_MONOTONIC as u64).varint(CLK_TIMESTAMP, 5_000_000);
        let mut inc = Encoder::new();
        inc.varint(CLK_ID, 64)
            .varint(CLK_TIMESTAMP, 100)
            .varint(CLK_IS_INCREMENTAL, 1)
            .varint(CLK_UNIT_MULTIPLIER_NS, 1000);
        snap.msg(CS_CLOCKS, &mono).msg(CS_CLOCKS, &inc).varint(CS_PRIMARY_TRACE_CLOCK, CLOCK_MONOTONIC as u64);
        let mut defaults = Encoder::new();
        defaults.varint(PDF_TIMESTAMP_CLOCK_ID, 64);
        let mut p = Encoder::new();
        p.varint(P_SEQUENCE_ID, 9)
            .varint(P_SEQUENCE_FLAGS, SEQ_INCREMENTAL_STATE_CLEARED)
            .msg(P_CLOCK_SNAPSHOT, &snap)
            .msg(P_PACKET_DEFAULTS, &defaults);
        let mut trace = packet(&p);
        trace.extend(thread_track(11, 7, 70, "Main"));
        trace.extend(te(1000, 9, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_BEGIN).varint(TE_TRACK_UUID, 11).string(TE_NAME, "a");
        }));
        trace.extend(te(500, 9, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_END).varint(TE_TRACK_UUID, 11);
        }));
        let (_ing, evs) = collect(&trace);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].start_ns, 6_000_000);
        assert_eq!(evs[0].duration_ns, 500_000);
    }

    #[test]
    fn process_and_child_tracks_are_async_lanes() {
        let mut trace = process_track(1, 7, "Renderer");
        trace.extend(named_track(2, 1, "Frames"));
        trace.extend(te(100, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_BEGIN).varint(TE_TRACK_UUID, 2).string(TE_NAME, "Frame 1");
        }));
        trace.extend(te(150, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_BEGIN).varint(TE_TRACK_UUID, 2).string(TE_NAME, "Draw");
        }));
        trace.extend(te(180, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_END).varint(TE_TRACK_UUID, 2);
        }));
        trace.extend(te(200, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_END).varint(TE_TRACK_UUID, 2);
        }));
        trace.extend(te(300, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_BEGIN).varint(TE_TRACK_UUID, 2).string(TE_NAME, "Frame 2");
        }));
        trace.extend(te(400, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_END).varint(TE_TRACK_UUID, 2);
        }));
        trace.extend(te(410, 1, |e| {
            e.varint(TE_TYPE, TYPE_INSTANT).varint(TE_TRACK_UUID, 1).string(TE_NAME, "pmark");
        }));
        let (ing, evs) = collect(&trace);
        assert_eq!(ing.process_names.get(&7).map(String::as_str), Some("Renderer"));
        let tracks: Vec<_> = evs.iter().filter(|e| e.kind == kind::API_TRACK).collect();
        assert_eq!(tracks.len(), 4);
        let lanes: std::collections::HashSet<_> = tracks.iter().filter(|e| e.duration_ns > 1).map(|e| e.tid).collect();
        assert_eq!(lanes.len(), 1, "one lane per track, not per slice name");
        let lane = *lanes.iter().next().unwrap();
        assert!(lane >= TID_ASYNC_BASE);
        assert_eq!(ing.thread_names.get(&(7, lane)).map(String::as_str), Some("Frames"));
        let draw = tracks.iter().find(|e| ing.intern.get(e.name_id) == Some("Draw")).unwrap();
        assert_eq!((draw.start_ns, draw.duration_ns, draw.depth), (150, 30, 1));
        let f2 = tracks.iter().find(|e| ing.intern.get(e.name_id) == Some("Frame 2")).unwrap();
        assert_eq!((f2.start_ns, f2.duration_ns, f2.depth), (300, 100, 0));
        let pmark = tracks.iter().find(|e| ing.intern.get(e.name_id) == Some("pmark")).unwrap();
        assert_ne!(pmark.tid, lane, "the process track is its own lane");
        assert_eq!(
            ing.thread_names.get(&(7, pmark.tid)).map(String::as_str),
            Some("pmark"),
            "an unnamed track lanes by event name"
        );
    }

    #[test]
    fn undescribed_tracks_fall_back_to_the_sequence_thread() {
        let mut defaults = Encoder::new();
        let mut ted = Encoder::new();
        ted.varint(TED_TRACK_UUID, 11);
        defaults.msg(PDF_TRACK_EVENT_DEFAULTS, &ted);
        let mut p = Encoder::new();
        p.varint(P_SEQUENCE_ID, 3).varint(P_SEQUENCE_FLAGS, SEQ_INCREMENTAL_STATE_CLEARED).msg(P_PACKET_DEFAULTS, &defaults);
        let mut trace = packet(&p);
        trace.extend(thread_track(11, 7, 70, "Main"));
        // Chrome legacy `M` thread_name with a tid override, on a uuid no
        // descriptor ever names: the name lands on (7, 71), not (0, 71).
        trace.extend(te(0, 3, |e| {
            let mut l = Encoder::new();
            l.i32(LE_PHASE, 'M' as i32).i32(LE_TID_OVERRIDE, 71);
            let mut ann = Encoder::new();
            ann.string(DA_NAME, "name").string(DA_STRING, "IO");
            e.varint(TE_TRACK_UUID, 0x5555).string(TE_NAME, "thread_name").msg(TE_DEBUG_ANNOTATIONS, &ann).msg(TE_LEGACY_EVENT, &l);
        }));
        // A typed slice on an undescribed uuid: an async lane of pid 7.
        trace.extend(te(100, 3, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_BEGIN).varint(TE_TRACK_UUID, 2560).string(TE_NAME, "InputLatency");
        }));
        trace.extend(te(150, 3, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_END).varint(TE_TRACK_UUID, 2560);
        }));
        // Legacy `X` with no track: the sequence's default thread.
        trace.extend(te(200, 3, |e| {
            let mut l = Encoder::new();
            l.i32(LE_PHASE, 'X' as i32).i64(LE_DURATION_US, 1);
            e.string(TE_NAME, "task").msg(TE_LEGACY_EVENT, &l);
        }));
        let (ing, evs) = collect(&trace);
        assert_eq!(ing.thread_names.get(&(7, 71)).map(String::as_str), Some("IO"));
        assert!(!ing.thread_names.contains_key(&(0, 71)));
        let asy = evs.iter().find(|e| e.kind == kind::API_TRACK).unwrap();
        assert_eq!((asy.pid, asy.start_ns, asy.duration_ns), (7, 100, 50));
        assert_eq!(ing.thread_names.get(&(7, asy.tid)).map(String::as_str), Some("InputLatency"));
        let task = evs.iter().find(|e| ing.intern.get(e.name_id) == Some("task")).unwrap();
        assert_eq!((task.pid, task.tid, task.duration_ns), (7, 70, 1000));
        assert!(!ing.process_names.contains_key(&0), "nothing lands on pid 0: {:?}", ing.process_names);
    }

    #[test]
    fn typed_slices_keep_legacy_flows_and_histograms_are_values() {
        let mut trace = thread_track(11, 7, 70, "Main");
        trace.extend(thread_track(12, 7, 71, "IO"));
        trace.extend(te(100, 1, |e| {
            let mut l = Encoder::new();
            l.varint(LE_BIND_ID, 5).varint(LE_FLOW_DIRECTION, 2);
            e.varint(TE_TYPE, TYPE_SLICE_BEGIN).varint(TE_TRACK_UUID, 11).string(TE_NAME, "post").msg(TE_LEGACY_EVENT, &l);
        }));
        trace.extend(te(120, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_END).varint(TE_TRACK_UUID, 11);
        }));
        trace.extend(te(200, 1, |e| {
            let mut l = Encoder::new();
            l.varint(LE_BIND_ID, 5).varint(LE_FLOW_DIRECTION, 1);
            e.varint(TE_TYPE, TYPE_INSTANT).varint(TE_TRACK_UUID, 12).string(TE_NAME, "run").msg(TE_LEGACY_EVENT, &l);
        }));
        trace.extend(te(300, 1, |e| {
            let mut h = Encoder::new();
            h.string(HS_NAME, "Memory.Foo").i64(HS_SAMPLE, 42);
            e.varint(TE_TYPE, TYPE_INSTANT).varint(TE_TRACK_UUID, 11).string(TE_NAME, "HistogramSample").msg(TE_CHROME_HISTOGRAM_SAMPLE, &h);
        }));
        let (ing, evs) = collect(&trace);
        assert_eq!(ing.flows.len(), 1, "{:?}", ing.flows);
        assert_eq!((ing.flows[0].from.tid, ing.flows[0].to.tid), (70, 71));
        let v = evs.iter().find(|e| e.kind == kind::VALUE).unwrap();
        assert_eq!((v.value_f32(), v.pid), (Some(42.0), 7));
        assert_eq!(ing.intern.get(v.name_id), Some("Memory.Foo"));
    }

    #[test]
    fn counters_multiply_and_accumulate() {
        let mut cd = Encoder::new();
        cd.i64(CD_UNIT_MULTIPLIER, 1000).varint(CD_IS_INCREMENTAL, 1);
        let mut td = Encoder::new();
        td.varint(TD_UUID, 5).string(TD_NAME, "mem").varint(TD_PARENT_UUID, 1).msg(TD_COUNTER, &cd);
        let mut p = Encoder::new();
        p.msg(P_TRACK_DESCRIPTOR, &td);
        let mut trace = process_track(1, 7, "Renderer");
        trace.extend(packet(&p));
        trace.extend(te(10, 1, |e| {
            e.varint(TE_TYPE, TYPE_COUNTER).varint(TE_TRACK_UUID, 5).i64(TE_COUNTER_VALUE, 2);
        }));
        trace.extend(te(20, 1, |e| {
            e.varint(TE_TYPE, TYPE_COUNTER).varint(TE_TRACK_UUID, 5).i64(TE_COUNTER_VALUE, 3);
        }));
        trace.extend(te(30, 1, |e| {
            e.varint(TE_TYPE, TYPE_COUNTER).varint(TE_TRACK_UUID, 5).double(TE_DOUBLE_COUNTER_VALUE, 0.5);
        }));
        let (ing, evs) = collect(&trace);
        let vals: Vec<_> = evs.iter().filter(|e| e.kind == kind::VALUE).collect();
        assert_eq!(vals.len(), 3);
        assert_eq!(vals[0].value_f32(), Some(2000.0));
        assert_eq!(vals[1].value_f32(), Some(5000.0));
        assert_eq!(vals[2].value_f32(), Some(5500.0));
        assert!(vals[0].tid >= TID_COUNTER_BASE);
        assert_eq!(vals[0].pid, 7);
        assert_eq!(ing.intern.get(vals[0].name_id), Some("mem"));
    }

    #[test]
    fn legacy_event_phases_ride_along() {
        let mut trace = thread_track(11, 7, 70, "Main");
        trace.extend(te(1000, 1, |e| {
            let mut l = Encoder::new();
            l.i32(LE_PHASE, 'X' as i32).i64(LE_DURATION_US, 5);
            e.varint(TE_TRACK_UUID, 11).string(TE_NAME, "complete").msg(TE_LEGACY_EVENT, &l);
        }));
        trace.extend(te(2000, 1, |e| {
            let mut l = Encoder::new();
            l.i32(LE_PHASE, 'S' as i32).varint(LE_UNSCOPED_ID, 77);
            e.varint(TE_TRACK_UUID, 11).string(TE_NAME, "async").msg(TE_LEGACY_EVENT, &l);
        }));
        trace.extend(te(2500, 1, |e| {
            let mut l = Encoder::new();
            l.i32(LE_PHASE, 'F' as i32).varint(LE_UNSCOPED_ID, 77);
            e.varint(TE_TRACK_UUID, 11).string(TE_NAME, "async").msg(TE_LEGACY_EVENT, &l);
        }));
        trace.extend(te(3000, 1, |e| {
            let mut l = Encoder::new();
            l.i32(LE_PHASE, 'i' as i32).varint(LE_INSTANT_SCOPE, 1);
            e.varint(TE_TRACK_UUID, 11).string(TE_NAME, "global").msg(TE_LEGACY_EVENT, &l);
        }));
        let (ing, evs) = collect(&trace);
        let name = |e: &LiveEvent| ing.intern.get(e.name_id).unwrap_or("").to_string();
        let complete = evs.iter().find(|e| name(e) == "complete").unwrap();
        assert_eq!((complete.kind, complete.start_ns, complete.duration_ns, complete.tid), (kind::API_SCOPE, 1000, 5000, 70));
        let asy = evs.iter().find(|e| name(e) == "async").unwrap();
        assert_eq!((asy.kind, asy.start_ns, asy.duration_ns), (kind::API_TRACK, 2000, 500));
        let g = evs.iter().find(|e| name(e) == "global").unwrap();
        assert_eq!((g.pid, g.tid), (crate::PID_GLOBAL, crate::TID_GLOBAL));
    }

    #[test]
    fn flow_ids_link_thread_slices() {
        let mut trace = thread_track(11, 7, 70, "Main");
        trace.extend(thread_track(12, 7, 71, "IO"));
        trace.extend(te(100, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_BEGIN).varint(TE_TRACK_UUID, 11).string(TE_NAME, "post").fixed64(TE_FLOW_IDS, 99);
        }));
        trace.extend(te(120, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_END).varint(TE_TRACK_UUID, 11);
        }));
        trace.extend(te(200, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_BEGIN).varint(TE_TRACK_UUID, 12).string(TE_NAME, "run").fixed64(TE_TERMINATING_FLOW_IDS, 99);
        }));
        trace.extend(te(250, 1, |e| {
            e.varint(TE_TYPE, TYPE_SLICE_END).varint(TE_TRACK_UUID, 12);
        }));
        let (ing, evs) = collect(&trace);
        assert_eq!(evs.len(), 2);
        assert_eq!(ing.flows.len(), 1);
        assert_eq!((ing.flows[0].from.tid, ing.flows[0].from.start_ns), (70, 100));
        assert_eq!((ing.flows[0].to.tid, ing.flows[0].to.start_ns), (71, 200));
    }

    #[test]
    fn legacy_thread_descriptor_and_delta_timestamps() {
        let mut th = Encoder::new();
        th.i32(THD_PID, 3).i64(THD_TID, 30).string(THD_NAME, "Compositor").i64(THD_REFERENCE_TIMESTAMP_US, 1000);
        let mut p = Encoder::new();
        p.varint(P_SEQUENCE_ID, 4).varint(P_INCREMENTAL_STATE_CLEARED, 1).msg(P_THREAD_DESCRIPTOR, &th);
        let mut trace = packet(&p);
        let mut e = Encoder::new();
        e.i64(TE_TIMESTAMP_DELTA_US, 10).varint(TE_TYPE, TYPE_SLICE_BEGIN).string(TE_NAME, "d");
        let mut p = Encoder::new();
        p.varint(P_SEQUENCE_ID, 4).msg(P_TRACK_EVENT, &e);
        trace.extend(packet(&p));
        let mut e = Encoder::new();
        e.i64(TE_TIMESTAMP_DELTA_US, 5).varint(TE_TYPE, TYPE_SLICE_END);
        let mut p = Encoder::new();
        p.varint(P_SEQUENCE_ID, 4).msg(P_TRACK_EVENT, &e);
        trace.extend(packet(&p));
        let (ing, evs) = collect(&trace);
        assert_eq!(evs.len(), 1);
        assert_eq!((evs[0].pid, evs[0].tid, evs[0].start_ns, evs[0].duration_ns), (3, 30, 1_010_000, 5_000));
        assert_eq!(ing.thread_names.get(&(3, 30)).map(String::as_str), Some("Compositor"));
    }

    #[test]
    fn ftrace_sched_switch_plain_and_compact() {
        let mut tree = Encoder::new();
        let mut t = Encoder::new();
        t.i32(PTT_TID, 21).string(PTT_NAME, "worker").i32(PTT_TGID, 20);
        tree.msg(PT_THREADS, &t);
        let mut pr = Encoder::new();
        pr.i32(PTP_PID, 20).string(PTP_CMDLINE, "/usr/bin/app");
        tree.msg(PT_PROCESSES, &pr);
        let mut p = Encoder::new();
        p.msg(P_PROCESS_TREE, &tree);
        let mut trace = packet(&p);

        // cpu 0: swapper -> 21 at 100, 21 -> 22 at 300 (21 sleeps), waking 21 at 350, 22 -> 21 at 400.
        let mut bundle = Encoder::new();
        bundle.varint(FB_CPU, 0);
        for (ts, prev, state, next, next_comm) in [(100u64, 0i32, 0i64, 21i32, "worker"), (300, 21, 1, 22, "other"), (400, 22, 0, 21, "worker")] {
            let mut ss = Encoder::new();
            ss.string(SS_PREV_COMM, "x").i32(SS_PREV_PID, prev).i64(SS_PREV_STATE, state).string(SS_NEXT_COMM, next_comm).i32(SS_NEXT_PID, next);
            let mut ev = Encoder::new();
            ev.varint(FE_TIMESTAMP, ts).varint(FE_PID, prev as u64).msg(FE_SCHED_SWITCH, &ss);
            bundle.msg(FB_EVENT, &ev);
            if ts == 300 {
                let mut w = Encoder::new();
                w.string(SW_COMM, "worker").i32(SW_PID, 21);
                let mut ev = Encoder::new();
                ev.varint(FE_TIMESTAMP, 350).varint(FE_PID, 22).msg(FE_SCHED_WAKING, &w);
                bundle.msg(FB_EVENT, &ev);
            }
        }
        let mut p = Encoder::new();
        p.msg(P_FTRACE_EVENTS, &bundle);
        trace.extend(packet(&p));

        // cpu 1, compact: 21? no — 31 on at 1000, off to 32 at 1500 (D), then 31 back at 2000.
        let mut cs = Encoder::new();
        cs.packed(CS_SWITCH_TIMESTAMP, &[1000, 500, 500])
            .packed(CS_SWITCH_PREV_STATE, &[0, 2, 0])
            .packed(CS_SWITCH_NEXT_PID, &[31, 32, 31])
            .packed(CS_SWITCH_NEXT_COMM_INDEX, &[0, 1, 0])
            .string(CS_INTERN_TABLE, "thirty-one")
            .string(CS_INTERN_TABLE, "thirty-two");
        let mut bundle = Encoder::new();
        bundle.varint(FB_CPU, 1).msg(FB_COMPACT_SCHED, &cs);
        let mut p = Encoder::new();
        p.msg(P_FTRACE_EVENTS, &bundle);
        trace.extend(packet(&p));

        // cpu 1, later bundles: 31 -> 33 at 2500 (no comm for 33), 33 -> 31
        // at 2800 (33's slice lands and mints its "tid 33" placeholder),
        // then 31 -> 33 at 3000 with the comm, which must win.
        for (ts, next, comm) in [(2500u64, 33u64, None), (2800, 31, None), (3000, 33, Some("thirty-three"))] {
            let mut cs = Encoder::new();
            cs.packed(CS_SWITCH_TIMESTAMP, &[ts]).packed(CS_SWITCH_PREV_STATE, &[0]).packed(CS_SWITCH_NEXT_PID, &[next]);
            if let Some(c) = comm {
                cs.packed(CS_SWITCH_NEXT_COMM_INDEX, &[0]).string(CS_INTERN_TABLE, c);
            }
            let mut bundle = Encoder::new();
            bundle.varint(FB_CPU, 1).msg(FB_COMPACT_SCHED, &cs);
            let mut p = Encoder::new();
            p.msg(P_FTRACE_EVENTS, &bundle);
            trace.extend(packet(&p));
        }

        let (ing, evs) = collect(&trace);
        assert_eq!(ing.process_names.get(&20).map(String::as_str), Some("app"));
        assert_eq!(ing.thread_names.get(&(20, 21)).map(String::as_str), Some("worker"));
        assert_eq!(ing.thread_names.get(&(31, 31)).map(String::as_str), Some("thirty-one"));
        assert_eq!(
            ing.thread_names.get(&(33, 33)).map(String::as_str),
            Some("thirty-three"),
            "a comm replaces the placeholder a first slice minted"
        );
        let sched: Vec<_> = evs.iter().filter(|e| e.kind == kind::SCHEDULING_SLICE).collect();
        // cpu0: 21 [100,300), 22 [300,400), 21 [400,end=3000).
        // cpu1: 31 [1000,1500), 32 [1500,2000), 31 [2000,2500), 33 [2500,2800),
        // 31 [2800,3000); 33's last slice is empty at the end and dropped.
        assert_eq!(sched.len(), 8, "{sched:?}");
        let s21 = sched.iter().find(|e| e.tid == 21 && e.start_ns == 100).unwrap();
        assert_eq!((s21.pid, s21.duration_ns, s21.extra), (20, 200, 0));
        let s32 = sched.iter().find(|e| e.tid == 32).unwrap();
        assert_eq!((s32.start_ns, s32.duration_ns, s32.extra), (1500, 500, 1));
        let tail = sched.iter().find(|e| e.tid == 21 && e.start_ns == 400).unwrap();
        assert_eq!(tail.duration_ns, 2600, "open slice closed at the last ftrace timestamp");
        let states: Vec<_> = evs.iter().filter(|e| e.kind == kind::THREAD_STATE && e.tid == 21).collect();
        let sleep = states.iter().find(|e| e.extra == thread_state::INTERRUPTIBLE_SLEEP).unwrap();
        assert_eq!((sleep.start_ns, sleep.duration_ns), (300, 50));
        let runnable = states.iter().find(|e| e.extra == thread_state::RUNNABLE).unwrap();
        assert_eq!((runnable.start_ns, runnable.duration_ns), (350, 50));
        let d31 = evs.iter().find(|e| e.kind == kind::THREAD_STATE && e.tid == 31 && e.extra == thread_state::UNINTERRUPTIBLE_SLEEP).unwrap();
        assert_eq!((d31.start_ns, d31.duration_ns), (1500, 500));
        assert_eq!(ing.stats.sched, 9);
    }

    #[test]
    fn late_process_tree_moves_states_and_names_threads() {
        // ftrace first: 2695 runs on cpu 0 from 100 to 300, comm "Binder".
        let mut bundle = Encoder::new();
        bundle.varint(FB_CPU, 0);
        for (ts, prev, next, prev_comm, next_comm) in [
            (100u64, 0i32, 2695i32, "swapper", "Binder:1204_15"),
            (300, 2695, 0, "Binder:1204_15", "swapper"),
        ] {
            let mut ss = Encoder::new();
            ss.string(SS_PREV_COMM, prev_comm).i32(SS_PREV_PID, prev).i64(SS_PREV_STATE, 1).string(SS_NEXT_COMM, next_comm).i32(SS_NEXT_PID, next);
            let mut ev = Encoder::new();
            ev.varint(FE_TIMESTAMP, ts).varint(FE_PID, prev as u64).msg(FE_SCHED_SWITCH, &ss);
            bundle.msg(FB_EVENT, &ev);
        }
        let mut p = Encoder::new();
        p.msg(P_FTRACE_EVENTS, &bundle);
        let mut trace = packet(&p);
        // An atrace mark from 2696 names its process before any tree does.
        let mut bundle = Encoder::new();
        bundle.varint(FB_CPU, 1);
        for (ts, prev, next) in [(100u64, 0i32, 2696i32), (200, 2696, 0)] {
            let mut ss = Encoder::new();
            ss.i32(SS_PREV_PID, prev).i64(SS_PREV_STATE, 0).i32(SS_NEXT_PID, next);
            let mut ev = Encoder::new();
            ev.varint(FE_TIMESTAMP, ts).msg(FE_SCHED_SWITCH, &ss);
            bundle.msg(FB_EVENT, &ev);
        }
        let mut pr = Encoder::new();
        pr.string(PR_BUF, "B|1204|Work\n");
        let mut ev = Encoder::new();
        ev.varint(FE_TIMESTAMP, 150).varint(FE_PID, 2696).msg(FE_PRINT, &pr);
        bundle.msg(FB_EVENT, &ev);
        let mut p = Encoder::new();
        p.msg(P_FTRACE_EVENTS, &bundle);
        trace.extend(packet(&p));
        let before_tree = ingest_collect(&trace).expect("ingest").1;
        // The tree (unnamed threads, as Android writes them) comes last.
        let mut tree = Encoder::new();
        let mut t = Encoder::new();
        t.i32(PTT_TID, 2695).i32(PTT_TGID, 1204);
        tree.msg(PT_THREADS, &t);
        let mut pr = Encoder::new();
        pr.i32(PTP_PID, 1204).string(PTP_CMDLINE, "system_server");
        tree.msg(PT_PROCESSES, &pr);
        let mut p = Encoder::new();
        p.msg(P_PROCESS_TREE, &tree);
        trace.extend(packet(&p));
        let (ing, evs) = collect(&trace);
        let states: Vec<_> = evs.iter().filter(|e| e.kind == kind::THREAD_STATE && e.tid == 2695).collect();
        assert!(!states.is_empty());
        assert!(states.iter().all(|e| e.pid == 1204), "states lane under the tree's tgid: {states:?}");
        assert_eq!(ing.thread_names.get(&(1204, 2695)).map(String::as_str), Some("Binder:1204_15"));
        assert!(!ing.thread_names.contains_key(&(2695, 2695)), "{:?}", ing.thread_names);
        assert!(!ing.thread_names.contains_key(&(2696, 2696)), "{:?}", ing.thread_names);
        assert!(!ing.process_names.contains_key(&2695), "{:?}", ing.process_names);
        let s2696: Vec<_> = evs.iter().filter(|e| e.kind == kind::THREAD_STATE && e.tid == 2696).collect();
        assert!(s2696.iter().all(|e| e.pid == 1204), "the atrace mark's tgid: {s2696:?}");
        // Without any tree the states still come out at the end, under the tid.
        let s_before: Vec<_> = before_tree.iter().filter(|e| e.kind == kind::THREAD_STATE && e.tid == 2695).collect();
        assert!(!s_before.is_empty() && s_before.iter().all(|e| e.pid == 2695));
    }

    #[test]
    fn ftrace_print_marks_become_scopes() {
        let mut bundle = Encoder::new();
        bundle.varint(FB_CPU, 2);
        for (ts, text) in [(10u64, "B|500|Pump\n"), (40, "E|500\n"), (50, "C|500|queue|3\n")] {
            let mut pr = Encoder::new();
            pr.string(PR_BUF, text);
            let mut ev = Encoder::new();
            ev.varint(FE_TIMESTAMP, ts).varint(FE_PID, 501).msg(FE_PRINT, &pr);
            bundle.msg(FB_EVENT, &ev);
        }
        let mut p = Encoder::new();
        p.msg(P_FTRACE_EVENTS, &bundle);
        let (ing, evs) = collect(&packet(&p));
        let scope = evs.iter().find(|e| e.kind == kind::API_SCOPE).unwrap();
        assert_eq!((scope.pid, scope.tid, scope.start_ns, scope.duration_ns), (500, 501, 10, 30));
        assert_eq!(ing.intern.get(scope.name_id), Some("Pump"));
        assert!(evs.iter().any(|e| e.kind == kind::VALUE && e.value_f32() == Some(3.0)));
        assert_eq!(ing.stats.system_trace, 3);
    }

    #[test]
    fn chrome_events_bundle_proto_and_json() {
        let mut ct = Encoder::new();
        ct.string(CT_NAME, "legacy").i64(CT_TIMESTAMP, 100).i32(CT_PHASE, 'X' as i32).i32(CT_THREAD_ID, 5).i64(CT_DURATION, 7).i32(CT_PROCESS_ID, 4);
        let mut arg = Encoder::new();
        arg.string(CTA_NAME, "k").i64(CTA_INT, 9);
        ct.msg(CT_ARGS, &arg);
        let mut json = Encoder::new();
        json.string(CLJ_DATA, r#"[{"name":"J","ph":"X","ts":1,"dur":2,"pid":8,"tid":8}]"#);
        let mut b = Encoder::new();
        b.msg(CE_TRACE_EVENTS, &ct).msg(CE_LEGACY_JSON_TRACE, &json).string(CE_LEGACY_FTRACE_OUTPUT, "  x-9  [000] ....  2.000000: tracing_mark_write: B|9|F\n  x-9  [000] ....  2.000001: tracing_mark_write: E|9\n");
        let mut p = Encoder::new();
        p.msg(P_CHROME_EVENTS, &b);
        let (ing, evs) = collect(&packet(&p));
        let name = |e: &LiveEvent| ing.intern.get(e.name_id).unwrap_or("").to_string();
        let l = evs.iter().find(|e| name(e) == "legacy").unwrap();
        assert_eq!((l.start_ns, l.duration_ns, l.pid, l.tid), (100_000, 7_000, 4, 5));
        assert!(ing.args.contains_key(&ArgKey::from_event(*l)));
        let j = evs.iter().find(|e| name(e) == "J").unwrap();
        assert_eq!((j.start_ns, j.duration_ns, j.pid), (1_000, 2_000, 8));
        let f = evs.iter().find(|e| name(e) == "F").unwrap();
        assert_eq!((f.start_ns, f.duration_ns), (2_000_000_000, 1_000));
        assert_eq!(ing.unit, TimeUnit::Nanos, "the ingestor is back in ns after the nested JSON");
    }

    #[test]
    fn perf_samples_become_call_frames() {
        let mut interned = Encoder::new();
        for (iid, name) in [(1u64, "main"), (2, "work")] {
            let mut s = Encoder::new();
            s.varint(1, iid).string(2, name);
            interned.msg(ID_FUNCTION_NAMES, &s);
        }
        for (iid, f) in [(10u64, 1u64), (11, 2), (12, 0)] {
            let mut fr = Encoder::new();
            fr.varint(1, iid).varint(2, f).varint(3, 5).varint(4, 0x1234);
            interned.msg(ID_FRAMES, &fr);
        }
        let mut mp = Encoder::new();
        mp.varint(1, 7).string(2, "/lib/libc.so");
        interned.msg(ID_MAPPING_PATHS, &mp);
        let mut m = Encoder::new();
        m.varint(1, 5).packed(7, &[7]);
        interned.msg(ID_MAPPINGS, &m);
        let mut cs = Encoder::new();
        cs.varint(1, 20).packed(2, &[10, 11, 12]);
        interned.msg(ID_CALLSTACKS, &cs);
        let mut ps = Encoder::new();
        ps.varint(PS_PID, 3).varint(PS_TID, 30).varint(PS_CALLSTACK_IID, 20);
        let mut p = Encoder::new();
        p.varint(P_TIMESTAMP, 777).varint(P_SEQUENCE_ID, 1).varint(P_SEQUENCE_FLAGS, 1).msg(P_INTERNED_DATA, &interned).msg(P_PERF_SAMPLE, &ps);
        let (ing, evs) = collect(&packet(&p));
        let calls: Vec<_> = evs.iter().filter(|e| e.kind == kind::FUNCTION_CALL).collect();
        assert_eq!(calls.len(), 3);
        let names: Vec<_> = calls.iter().map(|e| ing.intern.get(e.name_id).unwrap()).collect();
        assert_eq!(names, ["main", "work", "libc.so+0x1234"]);
        assert!(evs.iter().any(|e| e.kind == kind::SAMPLE && e.start_ns == 777 && e.tid == 30));
    }

    #[test]
    fn unknown_packets_are_counted_and_gzip_still_detects() {
        use std::io::Write;
        let mut p = Encoder::new();
        p.bytes(36, &[0xde; 16]); // synchronization_marker
        let mut trace = packet(&p);
        let mut p = Encoder::new();
        p.bytes(7, &[]); // sys_stats: nothing maps
        trace.extend(packet(&p));
        trace.extend(thread_track(11, 7, 70, "Main"));
        trace.extend(te(1, 1, |e| {
            e.varint(TE_TYPE, TYPE_INSTANT).varint(TE_TRACK_UUID, 11).string(TE_NAME, "i");
        }));
        let (ing, evs) = collect(&trace);
        assert_eq!(evs.len(), 1);
        assert_eq!(ing.stats.packets, 4);
        assert_eq!(ing.stats.skipped_packets, 1);
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&trace).unwrap();
        let gz = enc.finish().unwrap();
        let (_ing, evs) = collect(&gz);
        assert_eq!(evs.len(), 1);
    }

    #[test]
    fn truncated_tail_is_dropped_at_eof() {
        let mut trace = thread_track(11, 7, 70, "Main");
        trace.extend(te(5, 1, |e| {
            e.varint(TE_TYPE, TYPE_INSTANT).varint(TE_TRACK_UUID, 11).string(TE_NAME, "i");
        }));
        let whole = trace.len();
        trace.extend(te(6, 1, |e| {
            e.varint(TE_TYPE, TYPE_INSTANT).varint(TE_TRACK_UUID, 11).string(TE_NAME, "cut");
        }));
        trace.truncate(whole + 4);
        let (ing, evs) = collect(&trace);
        assert_eq!(evs.len(), 1);
        assert_eq!(ing.stats.packets, 2);
    }
}
