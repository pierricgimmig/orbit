// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Streaming Chrome Trace Event Format → 32-byte [`LiveEvent`]s.
//!
//! Accepts a JSON array of events (legacy `chrome://tracing`) or a JSON object
//! with `traceEvents[]`, plus `displayTimeUnit`, `stackFrames`, and `samples`.
//! gzip and a single-file zip are decoded into the parser — the file is never
//! materialized as a `Vec<serde_json::Value>`.
//!
//! [`LiveEvent`] stays 32 bytes. Args are interned as a compact hover string
//! and looked up with [`ArgKey`]. Memory-dump (`ph: v`) payloads are skipped.

mod id;
mod ingest;
mod json;
mod stream;

pub use ingest::{
    ArgKey, ChromeIngestor, FlowEdge, FlowEnd, IngestStats, TimeUnit, PID_GLOBAL, TID_ASYNC_BASE,
    TID_COUNTER_BASE, TID_GLOBAL, TID_OBJECT_BASE, TID_PROCESS_MARKERS,
};
pub use stream::{ingest_bytes, ingest_collect, ChromeStream};

use orbit_live_event::LIVE_EVENT_SIZE;

/// Documented WASM / process heap budget for a loaded file session.
/// The live ring is not used; events go into the viewer's `TrackIndex`.
pub const TRACE_HEAP_HINT: &str = "\
Loaded traces use the viewer TrackIndex (32 bytes per event + interned \
names/args), not the 64 MB capture ring. gzip is inflated as chunks arrive \
(not buffered then decoded). wasm32 heap is capped at 2 GiB (`--max-memory`); \
a 1–2 GB uncompressed JSON fits when streamed (JSON is not retained). A \
single enormous memory-dump object still transits the scan window and is \
then dropped.";

pub fn live_event_size() -> usize {
    LIVE_EVENT_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::{kind, LIVE_EVENT_SIZE};
    use std::collections::HashSet;
    use std::io::Write;

    const FIXTURE: &str = r#"{
  "displayTimeUnit": "ms",
  "traceEvents": [
    {"name":"process_name","ph":"M","pid":1,"args":{"name":"Renderer"}},
    {"name":"thread_name","ph":"M","pid":1,"tid":10,"args":{"name":"Main"}},
    {"name":"process_sort_index","ph":"M","pid":1,"args":{"sort_index":-5}},
    {"name":"thread_sort_index","ph":"M","pid":1,"tid":10,"args":{"sort_index":1}},
    {"name":"outer","cat":"foo","ph":"B","ts":100,"pid":1,"tid":10,"args":{"k":1}},
    {"name":"inner","cat":"foo","ph":"B","ts":110,"pid":1,"tid":10},
    {"name":"inner","ph":"E","ts":130,"pid":1,"tid":10},
    {"name":"outer","ph":"E","ts":200,"pid":1,"tid":10},
    {"name":"complete","ph":"X","ts":210,"dur":40,"pid":1,"tid":10,"args":{"n":2}},
    {"name":"tick","ph":"I","s":"t","ts":220,"pid":1,"tid":10},
    {"name":"procMark","ph":"i","s":"p","ts":221,"pid":1},
    {"name":"globMark","ph":"I","s":"g","ts":222},
    {"name":"ram","ph":"C","ts":230,"pid":1,"args":{"a":10,"b":20}},
    {"name":"gpu","cat":"async","ph":"S","ts":240,"pid":1,"tid":10,"id":"0xabc"},
    {"name":"gpu","cat":"async","ph":"T","ts":250,"pid":1,"tid":99,"id":"0xabc"},
    {"name":"gpu","cat":"async","ph":"F","ts":280,"pid":1,"tid":10,"id":"0xabc"},
    {"name":"flowA","ph":"s","ts":150,"pid":1,"tid":10,"id":7},
    {"name":"flowB","ph":"t","ts":215,"pid":1,"tid":10,"id":7},
    {"name":"flowC","ph":"f","ts":225,"pid":1,"tid":10,"id":7},
    {"name":"mark","ph":"R","ts":300,"pid":1,"tid":10},
    {"name":"clock","ph":"c","ts":301,"pid":1,"tid":10},
    {"name":"obj","ph":"N","ts":310,"pid":1,"id":"o1"},
    {"name":"obj","ph":"O","ts":320,"pid":1,"id":"o1","args":{"x":1}},
    {"name":"obj","ph":"D","ts":330,"pid":1,"id":"o1"},
    {"name":"periodic_interval","ph":"v","ts":340,"pid":1,"tid":10,"args":{"dumps":{"huge":true}}},
    {"name":"sample","ph":"P","ts":350,"pid":1,"tid":10,"sf":"1"}
  ],
  "stackFrames": {
    "1": {"name":"leaf","parent":"2"},
    "2": {"name":"root"}
  }
}"#;

    fn collect(json: &str) -> (ChromeIngestor, Vec<orbit_live_event::LiveEvent>) {
        ingest_collect(json.as_bytes()).expect("ingest")
    }

    #[test]
    fn live_event_stays_32() {
        assert_eq!(LIVE_EVENT_SIZE, 32);
        assert_eq!(live_event_size(), 32);
    }

    #[test]
    fn fixture_pairs_and_names() {
        let (ing, evs) = collect(FIXTURE);
        assert_eq!(ing.process_names.get(&1).map(String::as_str), Some("Renderer"));
        assert_eq!(
            ing.thread_names.get(&(1, 10)).map(String::as_str),
            Some("Main")
        );
        assert_eq!(ing.process_sort.get(&1).copied(), Some(-5));
        assert_eq!(ing.thread_sort.get(&(1, 10)).copied(), Some(1));

        let scopes: Vec<_> = evs
            .iter()
            .filter(|e| e.kind == kind::API_SCOPE && e.tid == 10)
            .copied()
            .collect();
        let names = |id| ing.intern.get(id).unwrap_or("").to_string();
        let by_name = |n: &str| {
            scopes
                .iter()
                .find(|e| names(e.name_id) == n)
                .copied()
                .unwrap_or_else(|| panic!("missing {n}"))
        };
        let inner = by_name("inner");
        let outer = by_name("outer");
        assert_eq!(inner.start_ns, 110_000);
        assert_eq!(inner.duration_ns, 20_000);
        assert_eq!(inner.depth, 1);
        assert_eq!(outer.start_ns, 100_000);
        assert_eq!(outer.duration_ns, 100_000);
        assert_eq!(outer.depth, 0);
        let complete = by_name("complete");
        assert_eq!(complete.duration_ns, 40_000);
        assert!(ing.args.contains_key(&ArgKey::from_event(outer)));
        assert!(ing.args.contains_key(&ArgKey::from_event(complete)));

        let instants = evs
            .iter()
            .filter(|e| matches!(names(e.name_id).as_str(), "tick" | "procMark" | "globMark" | "mark" | "clock"))
            .count();
        assert_eq!(instants, 5);
        assert!(evs.iter().any(|e| e.tid == TID_PROCESS_MARKERS));
        assert!(evs.iter().any(|e| e.pid == PID_GLOBAL && e.tid == TID_GLOBAL));

        let counters: Vec<_> = evs.iter().filter(|e| e.kind == kind::VALUE).collect();
        assert_eq!(counters.len(), 2);
        let labels: HashSet<_> = counters
            .iter()
            .map(|e| names(e.name_id))
            .collect();
        assert!(labels.contains("ram:a"));
        assert!(labels.contains("ram:b"));
        assert_ne!(counters[0].tid, counters[1].tid);
        assert!(counters[0].tid >= TID_COUNTER_BASE);

        let asyncs: Vec<_> = evs.iter().filter(|e| e.kind == kind::API_TRACK).collect();
        assert!(asyncs.iter().any(|e| names(e.name_id) == "gpu" && e.duration_ns > 1));
        let gpu = asyncs
            .iter()
            .find(|e| names(e.name_id) == "gpu" && e.duration_ns > 1)
            .unwrap();
        assert_eq!(gpu.duration_ns, 40_000);
        assert!(gpu.tid >= TID_ASYNC_BASE);
        assert_ne!(gpu.tid, 10, "async must not sit on the emitting tid");

        assert!(ing.flows.len() >= 2, "s→t and t→f");
        let flows = evs
            .iter()
            .filter(|e| names(e.name_id).starts_with("flow"))
            .count();
        assert_eq!(flows, 3);

        assert!(evs.iter().any(|e| names(e.name_id) == "periodic_interval"));
        let dump_key = evs
            .iter()
            .find(|e| names(e.name_id) == "periodic_interval")
            .map(|e| ArgKey::from_event(*e))
            .unwrap();
        let args = ing.intern.get(*ing.args.get(&dump_key).unwrap()).unwrap();
        assert!(args.contains("skipped"));
        assert!(!args.contains("huge"));

        let samples: Vec<_> = evs
            .iter()
            .filter(|e| e.kind == kind::FUNCTION_CALL)
            .collect();
        assert_eq!(samples.len(), 2);
        assert_eq!(names(samples[0].name_id), "root");
        assert_eq!(samples[0].depth, 0);
        assert_eq!(names(samples[1].name_id), "leaf");
        assert_eq!(samples[1].depth, 1);

        assert!(ing.stats.metadata >= 4);
        assert!(ing.stats.memory_dump >= 1);
    }

    #[test]
    fn legacy_array_and_numeric_ids() {
        let json = r#"[
          {"name":"A","ph":"B","ts":0,"pid":"42","tid":"7"},
          {"name":"A","ph":"E","ts":5,"pid":"42","tid":"7"}
        ]"#;
        let (ing, evs) = collect(json);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].pid, 42);
        assert_eq!(evs[0].tid, 7);
        assert_eq!(evs[0].duration_ns, 5_000);
        assert_eq!(ing.intern.get(evs[0].name_id), Some("A"));
    }

    #[test]
    fn display_time_unit_ns() {
        let json = r#"{"displayTimeUnit":"ns","traceEvents":[
          {"name":"X","ph":"X","ts":1000,"dur":50,"pid":1,"tid":1}
        ]}"#;
        let (_ing, evs) = collect(json);
        assert_eq!(evs[0].start_ns, 1000);
        assert_eq!(evs[0].duration_ns, 50);
    }

    #[test]
    fn nestable_async_b_e() {
        let json = r#"[
          {"name":"job","ph":"b","ts":0,"pid":1,"tid":1,"id":"x"},
          {"name":"job","ph":"e","ts":40,"pid":1,"tid":1,"id":"x"}
        ]"#;
        let (ing, evs) = collect(json);
        let tracks: Vec<_> = evs.iter().filter(|e| e.kind == kind::API_TRACK).collect();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].duration_ns, 40_000);
        assert!(tracks[0].tid >= TID_ASYNC_BASE);
        assert_ne!(tracks[0].tid, 1);
        assert_eq!(ing.stats.async_ev, 2);
    }

    #[test]
    fn system_trace_events_array_and_systrace_string() {
        let json = r#"{
          "traceEvents":[{"name":"X","ph":"X","ts":1,"dur":1,"pid":1,"tid":1}],
          "systemTraceEvents":[
            {"name":"sys","ph":"X","ts":2,"dur":3,"pid":9,"tid":9}
          ]
        }"#;
        let (_ing, evs) = collect(json);
        assert_eq!(evs.len(), 2);
        let names: HashSet<_> = evs.iter().map(|e| e.pid).collect();
        assert!(names.contains(&1) && names.contains(&9));

        let systrace = r##"{
          "traceEvents":[],
          "systemTraceEvents":"# tracer: nop\n          chrome-42  [000] ....  1.500000: tracing_mark_write: B|42|Pump\n          chrome-42  [000] ....  1.500010: tracing_mark_write: E|42\n          chrome-42  [000] ....  1.500020: tracing_mark_write: C|42|cpu|7\n"
        }"##;
        let (ing, evs) = collect(systrace);
        assert_eq!(ing.stats.system_trace, 3);
        assert!(evs.iter().any(|e| e.kind == kind::API_SCOPE && e.duration_ns == 10_000));
        assert!(evs.iter().any(|e| e.kind == kind::VALUE));
    }

    #[test]
    fn theverge_public_fixture_if_present() {
        let path = "/tmp/chrome-traces/theverge_trace.json";
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let (ing, evs) = ingest_collect(&bytes).expect("theverge");
        assert_eq!(ing.stats.events_in, 58_103);
        assert_eq!(ing.stats.duration, 54_224);
        assert_eq!(ing.stats.counter, 344);
        assert_eq!(ing.stats.async_ev, 785);
        assert!(ing.stats.object >= 2_101);
        assert!(evs.len() >= 30_000);
        assert!(ing.process_names.len() >= 6);
        let n66343 = ing
            .thread_names
            .keys()
            .filter(|(p, _)| *p == 66343)
            .count();
        assert!(
            n66343 < 32,
            "pid 66343 must not explode into {n66343} threads (group O/D and async by name)"
        );
    }

    #[test]
    fn many_object_and_async_ids_share_name_lanes() {
        let mut json = String::from("[");
        json.push_str(r#"{"name":"process_name","ph":"M","pid":7,"args":{"name":"Renderer"}},"#);
        json.push_str(r#"{"name":"thread_name","ph":"M","pid":7,"tid":1,"args":{"name":"Main"}},"#);
        for i in 0..400 {
            if i > 0 {
                json.push(',');
            }
            use std::fmt::Write;
            write!(
                json,
                r#"{{"name":"cc::Tile","ph":"O","ts":{i},"pid":7,"id":"{i}"}},"#,
            )
            .unwrap();
            write!(
                json,
                r#"{{"name":"cc::Tile","ph":"D","ts":{},"pid":7,"id":"{i}"}},"#,
                i + 1
            )
            .unwrap();
            write!(
                json,
                r#"{{"name":"PendingTree","ph":"S","ts":{i},"pid":7,"tid":1,"id":{i}}},"#,
            )
            .unwrap();
            write!(
                json,
                r#"{{"name":"PendingTree","ph":"F","ts":{},"pid":7,"tid":1,"id":{i}}}"#,
                i + 5
            )
            .unwrap();
        }
        json.push(']');
        let (ing, evs) = collect(&json);
        let tids: HashSet<u32> = ing
            .thread_names
            .keys()
            .filter(|(p, _)| *p == 7)
            .map(|(_, t)| *t)
            .collect();
        assert!(
            tids.len() < 8,
            "400 object ids + 400 async ids must share name lanes, got {} tids: {tids:?}",
            tids.len()
        );
        let objects: Vec<_> = evs
            .iter()
            .filter(|e| ing.intern.get(e.name_id) == Some("cc::Tile"))
            .collect();
        assert_eq!(objects.len(), 800);
        let obj_tids: HashSet<_> = objects.iter().map(|e| e.tid).collect();
        assert_eq!(obj_tids.len(), 1);
        let asyncs: Vec<_> = evs.iter().filter(|e| e.kind == kind::API_TRACK).collect();
        assert!(!asyncs.is_empty());
        let async_tids: HashSet<_> = asyncs.iter().map(|e| e.tid).collect();
        assert_eq!(async_tids.len(), 1);
        assert_ne!(*obj_tids.iter().next().unwrap(), 1);
        assert_ne!(*async_tids.iter().next().unwrap(), 1);
    }

    #[test]
    fn nested_async_n_o_d() {
        let json = r#"[
          {"name":"job","ph":"n","ts":0,"pid":2,"id":1},
          {"name":"step","ph":"o","ts":1,"pid":2,"id":1},
          {"name":"job","ph":"d","ts":10,"pid":2,"id":1}
        ]"#;
        let (ing, evs) = collect(json);
        let tracks: Vec<_> = evs.iter().filter(|e| e.kind == kind::API_TRACK).collect();
        assert!(tracks.iter().any(|e| e.duration_ns == 10_000));
        assert!(tracks.iter().any(|e| ing.intern.get(e.name_id) == Some("step")));
        assert_eq!(tracks.iter().map(|e| e.tid).collect::<HashSet<_>>().len(), 1);
    }

    #[test]
    fn gzip_multimember() {
        let mut a = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        a.write_all(br#"[{"name":"A","ph":"X","ts":1,"dur":1,"pid":1,"tid":1},"#)
            .unwrap();
        let mut gz = a.finish().unwrap();
        let mut b = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        b.write_all(br#"{"name":"B","ph":"X","ts":2,"dur":1,"pid":1,"tid":1}]"#)
            .unwrap();
        gz.extend(b.finish().unwrap());
        let (_ing, evs) = ingest_collect(&gz).expect("multi-gz");
        assert_eq!(evs.len(), 2);
    }

    #[test]
    fn gzip_roundtrip() {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(br#"[{"name":"Z","ph":"X","ts":1,"dur":2,"pid":3,"tid":4}]"#)
            .unwrap();
        let gz = enc.finish().unwrap();
        let (_ing, evs) = ingest_collect(&gz).expect("gzip");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].pid, 3);
        assert_eq!(evs[0].duration_ns, 2_000);
    }

    #[test]
    fn gzip_emits_events_before_eof() {
        // Inflate-as-we-go: events must appear after a prefix of the gzip,
        // before finish_input(), so a multi-GB .json.gz cannot require the
        // whole file in RAM first.
        const N: usize = 8_000;
        let mut json = String::from("[");
        for i in 0..N {
            if i > 0 {
                json.push(',');
            }
            use std::fmt::Write;
            write!(
                json,
                r#"{{"name":"X","ph":"X","ts":{i},"dur":10,"pid":1,"tid":1}}"#
            )
            .unwrap();
        }
        json.push(']');
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(json.as_bytes()).unwrap();
        let gz = enc.finish().unwrap();
        let mut stream = ChromeStream::default();
        let mut ing = ChromeIngestor::default();
        let mut evs = 0usize;
        let mid = gz.len() / 2;
        stream.push(&gz[..mid.max(64)]);
        evs += stream.pump(&mut ing, 64 * 1024).len();
        assert!(
            evs > 0,
            "streaming gzip must emit events before the last compressed byte (got {evs})"
        );
        stream.push(&gz[mid.max(64)..]);
        stream.finish_input();
        evs += stream.pump(&mut ing, 64 * 1024).len();
        evs += ing.finish(1).len();
        assert_eq!(evs, N);
    }

    #[test]
    fn stream_chunks_match_whole() {
        let bytes = FIXTURE.as_bytes();
        let whole = ingest_collect(bytes).unwrap().1;
        let mut stream = ChromeStream::default();
        let mut ing = ChromeIngestor::default();
        let mut evs = Vec::new();
        for chunk in bytes.chunks(17) {
            stream.push(chunk);
            evs.extend(stream.pump(&mut ing, 8));
        }
        stream.finish_input();
        evs.extend(stream.pump(&mut ing, 1024));
        evs.extend(ing.finish(evs.iter().map(|e| e.end_ns()).max().unwrap_or(1)));
        assert_eq!(evs.len(), whole.len());
    }

    #[test]
    fn stream_hundreds_of_thousands() {
        const N: usize = 250_000;
        let mut json = String::with_capacity(N * 64);
        json.push('[');
        for i in 0..N {
            if i > 0 {
                json.push(',');
            }
            use std::fmt::Write;
            write!(
                json,
                r#"{{"name":"X","ph":"X","ts":{i},"dur":10,"pid":1,"tid":1}}"#
            )
            .unwrap();
        }
        json.push(']');
        let t0 = std::time::Instant::now();
        let (ing, evs) = ingest_collect(json.as_bytes()).expect("stream");
        let dt = t0.elapsed();
        assert_eq!(evs.len(), N);
        assert_eq!(ing.stats.complete as usize, N);
        assert!(evs.iter().all(|e| e.kind == kind::API_SCOPE));
        assert!(
            dt.as_secs_f64() < 30.0,
            "250k events took {dt:?} (should be well under 30s)"
        );
    }

    #[test]
    fn be_pairing_by_id() {
        let json = r#"[
          {"name":"A","ph":"B","ts":0,"pid":1,"tid":1,"id":"x"},
          {"name":"B","ph":"B","ts":1,"pid":1,"tid":1,"id":"y"},
          {"name":"A","ph":"E","ts":10,"pid":1,"tid":1,"id":"x"},
          {"name":"B","ph":"E","ts":20,"pid":1,"tid":1,"id":"y"}
        ]"#;
        let (ing, evs) = collect(json);
        let a = evs
            .iter()
            .find(|e| ing.intern.get(e.name_id) == Some("A"))
            .unwrap();
        let b = evs
            .iter()
            .find(|e| ing.intern.get(e.name_id) == Some("B"))
            .unwrap();
        assert_eq!(a.duration_ns, 10_000);
        assert_eq!(b.duration_ns, 19_000);
    }
}
