// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Thread and process names, and the self-contained capture they go into.
//!
//! A track called `thread 48211 48211` tells you nothing; `thread 48211
//! RenderThread` does. The kernel knows the name (`comm`) of every thread,
//! and it is a file read away while the process lives -- so the capture
//! loop asks once a second for the processes it shows and hands each new
//! name to the viewer, which then also has it when the capture is saved.
//! A capture that is exported after the process has gone keeps whatever
//! was learnt while it ran.
//!
//! [`capture_bundle`] gathers a whole capture -- ring events with their
//! names, the sample store, the names -- into the [`CaptureBundle`] that
//! `orbit-capture` writes as one `.orbit.zip`.

use std::collections::{HashMap, HashSet};

use orbit_capture::{CaptureBundle, EventRow, FrameRow, ProcessName, SampleRow, ThreadName};
use orbit_live_event::{InternTable, LiveEvent};

use crate::report::SampleStore;

/// Reads `/proc/<pid>/comm`, trimmed; `None` when the process is gone.
pub fn process_comm(pid: u32) -> Option<String> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Reads `/proc/<pid>/task/<tid>/comm`, trimmed.
pub fn thread_comm(pid: u32, tid: u32) -> Option<String> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/task/{tid}/comm")).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// The tids of a live process, from `/proc/<pid>/task`.
pub fn thread_ids(pid: u32) -> Vec<u32> {
    std::fs::read_dir(format!("/proc/{pid}/task"))
        .map(|dir| {
            dir.flatten()
                .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse().ok()))
                .collect()
        })
        .unwrap_or_default()
}

/// What the capture loop has told the viewer so far, so each name goes out
/// once. Names do change (`prctl(PR_SET_NAME)`), and a thread that renames
/// itself is re-sent when the name read differs.
#[derive(Default)]
pub struct NameSync {
    threads: HashMap<(u32, u32), String>,
    processes: HashMap<u32, String>,
}

impl NameSync {
    /// Reads the names of `pids` and their threads from `/proc` and calls
    /// `on_process` / `on_thread` for every name not sent before (or
    /// changed since). Returns how many names went out.
    pub fn refresh(
        &mut self,
        pids: &[u32],
        on_process: impl FnMut(u32, &str),
        on_thread: impl FnMut(u32, u32, &str),
    ) -> usize {
        self.refresh_with(
            pids,
            process_comm,
            |pid| thread_ids(pid).into_iter().filter_map(|tid| Some((tid, thread_comm(pid, tid)?))).collect(),
            on_process,
            on_thread,
        )
    }

    /// [`refresh`](Self::refresh) with the readers passed in, so the
    /// once-only rule can be tested without a live process.
    pub fn refresh_with(
        &mut self,
        pids: &[u32],
        read_process: impl Fn(u32) -> Option<String>,
        read_threads: impl Fn(u32) -> Vec<(u32, String)>,
        mut on_process: impl FnMut(u32, &str),
        mut on_thread: impl FnMut(u32, u32, &str),
    ) -> usize {
        let mut sent = 0;
        for &pid in pids {
            if let Some(name) = read_process(pid) {
                if self.processes.get(&pid) != Some(&name) {
                    on_process(pid, &name);
                    self.processes.insert(pid, name);
                    sent += 1;
                }
            }
            for (tid, name) in read_threads(pid) {
                if self.threads.get(&(pid, tid)) != Some(&name) {
                    on_thread(pid, tid, &name);
                    self.threads.insert((pid, tid), name);
                    sent += 1;
                }
            }
        }
        sent
    }
}

/// The whole capture as a bundle: every ring event with its name resolved,
/// the sample store's rows and frame table, and a name for every thread and
/// process that appears -- from what the capture loop learnt, else from
/// `/proc` if the process is still alive, else the interned name the
/// producer registered under the tid (the demo does that), else nothing.
pub fn capture_bundle(
    events: &[LiveEvent],
    intern: &InternTable,
    store: &SampleStore,
    known_threads: &[((u32, u32), String)],
    known_processes: &[(u32, String)],
    target_pid: u32,
) -> CaptureBundle {
    let rows: Vec<EventRow> = events
        .iter()
        .map(|e| EventRow {
            event: *e,
            name: intern.get(e.name_id).unwrap_or_default().to_string(),
        })
        .collect();
    let (samples, frames) = store.export_rows();
    let samples: Vec<SampleRow> = samples
        .into_iter()
        .map(|(timestamp_ns, tid, frames)| SampleRow { timestamp_ns, tid, frames })
        .collect();
    let frames: Vec<FrameRow> = frames
        .into_iter()
        .map(|(id, info)| FrameRow { id, name: info.name, module: info.module, address: info.address })
        .collect();

    let known_t: HashMap<(u32, u32), &str> =
        known_threads.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let known_p: HashMap<u32, &str> = known_processes.iter().map(|(k, v)| (*k, v.as_str())).collect();

    let mut tids: Vec<(u32, u32)> = events.iter().map(|e| (e.pid, e.tid)).collect::<HashSet<_>>().into_iter().collect();
    tids.sort_unstable();
    let mut pids: Vec<u32> = tids.iter().map(|(p, _)| *p).collect::<HashSet<_>>().into_iter().collect();
    if target_pid != 0 && !pids.contains(&target_pid) {
        pids.push(target_pid);
    }
    pids.sort_unstable();

    let threads = tids
        .iter()
        .filter_map(|&(pid, tid)| {
            let name = known_t
                .get(&(pid, tid))
                .map(|s| s.to_string())
                .or_else(|| thread_comm(pid, tid))
                .or_else(|| intern.get(tid).map(str::to_string))?;
            Some(ThreadName { pid, tid, name })
        })
        .collect();
    let processes = pids
        .iter()
        .filter_map(|&pid| {
            let name = known_p
                .get(&pid)
                .map(|s| s.to_string())
                .or_else(|| process_comm(pid))?;
            Some(ProcessName { pid, name })
        })
        .collect();

    CaptureBundle { target_pid, slice_ns: None, processes, threads, events: rows, samples, frames }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{FrameInfo, StoredSample};
    use orbit_live_event::kind;

    #[test]
    fn this_process_names_itself_and_its_threads() {
        let me = std::process::id();
        assert!(process_comm(me).is_some());
        let tids = thread_ids(me);
        assert!(tids.contains(&me), "the main thread's tid is the pid");
        assert!(thread_comm(me, me).is_some());
        // A named thread shows up under its name.
        let t = std::thread::Builder::new()
            .name("orbit-name-test".into())
            .spawn(|| {
                let tid = unsafe { libc::gettid() } as u32;
                (tid, thread_comm(std::process::id(), tid))
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(t.1.as_deref(), Some("orbit-name-test"));
    }

    #[test]
    fn a_name_is_sent_once_and_again_only_when_it_changes() {
        let mut sync = NameSync::default();
        let table = std::cell::RefCell::new(vec![(70u32, "Main".to_string()), (71, "Worker".to_string())]);
        let read_p = |pid: u32| (pid == 7).then(|| "game".to_string());
        let read_t = |pid: u32| if pid == 7 { table.borrow().clone() } else { Vec::new() };
        let log: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
        let n = sync.refresh_with(
            &[7, 8],
            read_p,
            read_t,
            |p, n| log.borrow_mut().push(format!("p{p}={n}")),
            |p, t, n| log.borrow_mut().push(format!("t{p}/{t}={n}")),
        );
        assert_eq!(n, 3);
        assert_eq!(*log.borrow(), vec!["p7=game", "t7/70=Main", "t7/71=Worker"]);
        // Nothing changed: nothing goes out.
        let n = sync.refresh_with(&[7, 8], read_p, read_t, |_, _| panic!("resent"), |_, _, _| panic!("resent"));
        assert_eq!(n, 0);
        // A thread renamed itself: that one name goes out again.
        table.borrow_mut()[1].1 = "Worker-renamed".to_string();
        log.borrow_mut().clear();
        let n = sync.refresh_with(
            &[7],
            read_p,
            read_t,
            |_, _| panic!("resent"),
            |p, t, n| log.borrow_mut().push(format!("t{p}/{t}={n}")),
        );
        assert_eq!(n, 1);
        assert_eq!(*log.borrow(), vec!["t7/71=Worker-renamed"]);
        // The real readers on this process send something and do not fail.
        let mut fresh = NameSync::default();
        let me = std::process::id();
        assert!(fresh.refresh(&[me], |_, _| {}, |_, _, _| {}) >= 2);
        assert_eq!(fresh.refresh(&[u32::MAX - 1], |_, _| panic!(), |_, _, _| panic!()), 0);
    }

    #[test]
    fn the_bundle_names_every_thread_it_can_and_carries_the_samples() {
        let mut intern = InternTable::default();
        intern.insert_id(100, "Tick");
        intern.insert_id(555, "DemoThread"); // the demo's convention: name under the tid
        let events = vec![
            LiveEvent {
                start_ns: 10,
                duration_ns: 5,
                tid: 555,
                pid: 4_000_000, // no such process: falls back to the interned name
                kind: kind::API_SCOPE,
                depth: 0,
                extra: 0,
                _pad: 0,
                name_id: 100,
            },
            LiveEvent {
                start_ns: 20,
                duration_ns: 5,
                tid: 777,
                pid: 4_000_000,
                kind: kind::API_SCOPE,
                depth: 0,
                extra: 0,
                _pad: 0,
                name_id: 100,
            },
        ];
        let store = SampleStore::new();
        store.record_frame(9, FrameInfo { name: "f".into(), module: "m".into(), address: 0x9 });
        store.push(StoredSample { timestamp_ns: 12, tid: 555, frames: vec![9] });
        let known = vec![((4_000_000, 777), "Known".to_string())];
        let b = capture_bundle(&events, &intern, &store, &known, &[(4_000_000, "ghost".into())], 4_000_000);
        assert_eq!(b.events.len(), 2);
        assert_eq!(b.events[0].name, "Tick");
        assert_eq!(b.samples.len(), 1);
        assert_eq!(b.frames.len(), 1);
        assert_eq!(b.frames[0].name, "f");
        let mut names: Vec<(u32, &str)> = b.threads.iter().map(|t| (t.tid, t.name.as_str())).collect();
        names.sort();
        assert_eq!(names, vec![(555, "DemoThread"), (777, "Known")]);
        assert_eq!(b.processes, vec![ProcessName { pid: 4_000_000, name: "ghost".into() }]);
        assert_eq!(b.target_pid, 4_000_000);
    }
}

/// Wire and bundle sizes on a real capture. Point `ORBIT_WIRE_BENCH_BUNDLE`
/// at an `.orbit.zip` and run
/// `cargo test --release wire_bench_real -- --ignored --nocapture`.
#[cfg(test)]
mod bench {
    use orbit_live_server::{decode_frame, encode_event_batch_with, WireFormat};

    #[test]
    #[ignore]
    fn wire_bench_real() {
        let Ok(path) = std::env::var("ORBIT_WIRE_BENCH_BUNDLE") else {
            eprintln!("set ORBIT_WIRE_BENCH_BUNDLE to an .orbit.zip");
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let bundle = orbit_capture::CaptureBundle::from_zip(&bytes).unwrap();
        let events: Vec<_> = bundle.events.iter().map(|r| r.event).collect();
        println!("{}: {} events", path, events.len());
        for wire in [WireFormat::Raw, WireFormat::Packed, WireFormat::Deflate] {
            let t0 = std::time::Instant::now();
            let frames: Vec<Vec<u8>> = events.chunks(2048).map(|c| encode_event_batch_with(c, wire)).collect();
            let enc = t0.elapsed();
            let total: usize = frames.iter().map(Vec::len).sum();
            let t0 = std::time::Instant::now();
            let mut n = 0usize;
            for f in &frames {
                let (frame, _) = decode_frame(f).unwrap();
                if let orbit_live_server::LiveFrameRef::EventBatch(k) = orbit_live_server::frame_len(&frame) {
                    n += k;
                }
            }
            let dec = t0.elapsed();
            assert_eq!(n, events.len());
            println!(
                "  {:8} {:6.2} B/event  {:7.2} MB   encode {:6.1} us/batch   decode {:6.1} us/batch  (2048/batch)",
                wire.name(),
                total as f64 / events.len() as f64,
                total as f64 / 1e6,
                enc.as_secs_f64() * 1e6 / frames.len() as f64,
                dec.as_secs_f64() * 1e6 / frames.len() as f64
            );
        }
        // What a transport-level compressor would do to the raw stream: ssh -C
        // is zlib level 6 with the whole connection as context; a WebSocket
        // permessage-deflate is the same per message. Both bounds, on the
        // raw 32-byte records.
        let raw_frames: Vec<Vec<u8>> = events.chunks(2048).map(|c| encode_event_batch_with(c, WireFormat::Raw)).collect();
        let t0 = std::time::Instant::now();
        let per_batch: usize = raw_frames.iter().map(|f| miniz_oxide::deflate::compress_to_vec(f, 6).len()).sum();
        let per_batch_t = t0.elapsed();
        let stream: Vec<u8> = raw_frames.concat();
        let t0 = std::time::Instant::now();
        let whole = miniz_oxide::deflate::compress_to_vec(&stream, 6).len();
        let whole_t = t0.elapsed();
        println!(
            "  raw + zlib6 per message  {:6.2} B/event  ({:6.1} us/batch)   -- WebSocket permessage-deflate",
            per_batch as f64 / events.len() as f64,
            per_batch_t.as_secs_f64() * 1e6 / raw_frames.len() as f64
        );
        println!(
            "  raw + zlib6 whole stream {:6.2} B/event  ({:6.1} us/batch)   -- ssh -C (stream context)",
            whole as f64 / events.len() as f64,
            whole_t.as_secs_f64() * 1e6 / raw_frames.len() as f64
        );
        // Transfer time of one 2048-event batch on a link, the latency a
        // slower format adds before the first event of the batch can show.
        for wire in [WireFormat::Raw, WireFormat::Packed, WireFormat::Deflate] {
            let bytes = encode_event_batch_with(&events[..2048], wire).len() as f64;
            println!(
                "  {:8} batch of 2048 = {:6.1} KB: {:5.2} ms at 100 Mbit/s, {:5.2} ms at 1 Gbit/s, {:5.3} ms at 10 Gbit/s",
                wire.name(),
                bytes / 1e3,
                bytes * 8.0 / 100e6 * 1e3,
                bytes * 8.0 / 1e9 * 1e3,
                bytes * 8.0 / 10e9 * 1e3
            );
        }
        // The bundle itself, stored versus deflated.
        for (label, level) in [("store", None), ("deflate 1", Some(1u8)), ("deflate 6", Some(6u8))] {
            let t0 = std::time::Instant::now();
            let zip = bundle_zip(&bundle, level);
            println!(
                "  bundle {:9} {:7.2} MB in {:.2} s",
                label,
                zip.len() as f64 / 1e6,
                t0.elapsed().as_secs_f64()
            );
        }
    }

    fn bundle_zip(bundle: &orbit_capture::CaptureBundle, level: Option<u8>) -> Vec<u8> {
        let mut b = bundle.clone();
        b.slice_ns = bundle.slice_ns;
        orbit_capture::bundle_zip_with_level(&b, level).unwrap()
    }
}
