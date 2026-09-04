// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A whole capture, or a slice of one, as a single self-contained file.
//!
//! The events table alone is not a capture: it does not say what the
//! threads and processes were called, and it carries no sampled callstacks.
//! A [`CaptureBundle`] is everything the viewer needs to show a capture
//! again -- events with their names, the sample rows and the frame table
//! they point into, thread and process names, and which process was the
//! target -- and `to_zip` writes it as one `.orbit.zip`: the three Arrow
//! tables and a `manifest.json`, deflated, so any zip tool extracts it into
//! exactly the dataset directory `write_dataset` produces (and the Python
//! example opens).
//!
//! `slice` cuts a bundle down to a time window. Events that overlap the
//! window are kept whole -- a scope that straddles the edge is still a real
//! scope with its real duration, and clipping it would misreport it -- so
//! the manifest records the window that was asked for next to the bounds
//! of what was kept. Samples are point events and are kept by timestamp;
//! the frame table keeps only the frames those samples reference.

use std::collections::{HashMap, HashSet};

use orbit_live_event::LiveEvent;

use crate::zipstore::read_store_zip;
#[cfg(test)]
use crate::zipstore::write_store_zip;
use crate::{
    read_events_ipc, read_frames_ipc, read_samples_ipc, write_events_ipc, write_frames_ipc,
    write_samples_ipc, CaptureError, EventRow, FrameRow, SampleRow, DATASET_FORMAT, EVENTS_FILE,
    FRAMES_FILE, MANIFEST_FILE, SAMPLES_FILE,
};

/// A thread's name, as `/proc/<pid>/task/<tid>/comm` or the producer gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadName {
    pub pid: u32,
    pub tid: u32,
    pub name: String,
}

/// A process's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessName {
    pub pid: u32,
    pub name: String,
}

/// Everything a capture is. See the module docs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureBundle {
    /// The process the capture targeted; 0 for a capture with no target.
    pub target_pid: u32,
    /// The window this bundle was cut to, if it is a slice.
    pub slice_ns: Option<(u64, u64)>,
    pub processes: Vec<ProcessName>,
    pub threads: Vec<ThreadName>,
    pub events: Vec<EventRow>,
    pub samples: Vec<SampleRow>,
    pub frames: Vec<FrameRow>,
}

/// The file name suffix of a bundle. The viewer tells a bundle from a zipped
/// Chrome trace by it.
pub const BUNDLE_SUFFIX: &str = ".orbit.zip";

impl CaptureBundle {
    /// Earliest event start and latest event end (samples count as instants),
    /// or `None` when there is nothing in it.
    pub fn time_bounds(&self) -> Option<(u64, u64)> {
        let mut start = u64::MAX;
        let mut end = 0u64;
        for r in &self.events {
            start = start.min(r.event.start_ns);
            end = end.max(r.event.end_ns());
        }
        for s in &self.samples {
            start = start.min(s.timestamp_ns);
            end = end.max(s.timestamp_ns);
        }
        (start != u64::MAX).then_some((start, end.max(start)))
    }

    /// The bundle cut to `[t0, t1]`: overlapping events whole, samples by
    /// timestamp, frames those samples use, and only the threads and
    /// processes that still have something in them.
    pub fn slice(&self, t0: u64, t1: u64) -> CaptureBundle {
        let (t0, t1) = (t0.min(t1), t0.max(t1));
        let events: Vec<EventRow> = self
            .events
            .iter()
            .filter(|r| r.event.start_ns <= t1 && r.event.end_ns() >= t0)
            .cloned()
            .collect();
        let samples: Vec<SampleRow> = self
            .samples
            .iter()
            .filter(|s| s.timestamp_ns >= t0 && s.timestamp_ns <= t1)
            .cloned()
            .collect();
        let used: HashSet<u32> = samples.iter().flat_map(|s| s.frames.iter().copied()).collect();
        let frames: Vec<FrameRow> = self.frames.iter().filter(|f| used.contains(&f.id)).cloned().collect();

        let mut pids: HashSet<u32> = events.iter().map(|r| r.event.pid).collect();
        let mut tids: HashSet<(u32, u32)> = events.iter().map(|r| (r.event.pid, r.event.tid)).collect();
        for s in &samples {
            // A sample names only its thread; its process is whichever the
            // thread table says, and the target as a fallback.
            let pid = self
                .threads
                .iter()
                .find(|t| t.tid == s.tid)
                .map(|t| t.pid)
                .unwrap_or(self.target_pid);
            pids.insert(pid);
            tids.insert((pid, s.tid));
        }
        // The target is part of the capture even if it was idle in the window.
        pids.insert(self.target_pid);
        CaptureBundle {
            target_pid: self.target_pid,
            slice_ns: Some((t0, t1)),
            processes: self.processes.iter().filter(|p| pids.contains(&p.pid)).cloned().collect(),
            threads: self.threads.iter().filter(|t| tids.contains(&(t.pid, t.tid))).cloned().collect(),
            events,
            samples,
            frames,
        }
    }

    /// The distinct `(name_id, name)` pairs of the events, which is the
    /// intern table a reader has to rebuild.
    pub fn names(&self) -> Vec<(u32, String)> {
        let mut seen: HashMap<u32, &str> = HashMap::new();
        for r in &self.events {
            seen.entry(r.event.name_id).or_insert(&r.name);
        }
        let mut out: Vec<(u32, String)> = seen.into_iter().map(|(id, n)| (id, n.to_string())).collect();
        out.sort_by_key(|(id, _)| *id);
        out
    }

    fn manifest_json(&self) -> serde_json::Value {
        let bounds = self.time_bounds();
        serde_json::json!({
            "format": DATASET_FORMAT,
            "rows": {
                "events": self.events.len(),
                "samples": self.samples.len(),
                "frames": self.frames.len(),
            },
            "time_bounds_ns": bounds.map(|(a, b)| serde_json::json!({"start": a, "end": b})),
            "files": {
                "events": EVENTS_FILE,
                "samples": SAMPLES_FILE,
                "frames": FRAMES_FILE,
            },
            "bundle": {
                "target_pid": self.target_pid,
                "slice_ns": self.slice_ns.map(|(a, b)| serde_json::json!({"start": a, "end": b})),
                "processes": self.processes.iter().map(|p| serde_json::json!({"pid": p.pid, "name": p.name})).collect::<Vec<_>>(),
                "threads": self.threads.iter().map(|t| serde_json::json!({"pid": t.pid, "tid": t.tid, "name": t.name})).collect::<Vec<_>>(),
            },
        })
    }

    /// The bundle as one `.orbit.zip`, deflated.
    pub fn to_zip(&self) -> Result<Vec<u8>, CaptureError> {
        self.to_zip_with_level(Some(crate::zipstore::BUNDLE_DEFLATE_LEVEL))
    }

    /// As [`to_zip`](Self::to_zip) at a chosen deflate level, or stored
    /// with `None`.
    pub fn to_zip_with_level(&self, level: Option<u8>) -> Result<Vec<u8>, CaptureError> {
        let names: HashMap<u32, &str> = self.events.iter().map(|r| (r.event.name_id, r.name.as_str())).collect();
        let events: Vec<LiveEvent> = self.events.iter().map(|r| r.event).collect();
        let resolve = |id: u32| names.get(&id).map(|s| s.to_string()).unwrap_or_default();

        let mut events_buf = Vec::new();
        write_events_ipc(std::io::Cursor::new(&mut events_buf), &events, resolve)?;
        let mut samples_buf = Vec::new();
        write_samples_ipc(std::io::Cursor::new(&mut samples_buf), &self.samples)?;
        let mut frames_buf = Vec::new();
        write_frames_ipc(std::io::Cursor::new(&mut frames_buf), &self.frames)?;
        let manifest = serde_json::to_string_pretty(&self.manifest_json())?;

        Ok(crate::zipstore::write_zip(
            &[
                (MANIFEST_FILE, manifest.as_bytes()),
                (EVENTS_FILE, &events_buf),
                (SAMPLES_FILE, &samples_buf),
                (FRAMES_FILE, &frames_buf),
            ],
            level,
        )?)
    }

    /// A bundle back from its zip. The manifest's `bundle` section is
    /// optional, so a zipped dataset directory (no names, no target) opens
    /// too.
    pub fn from_zip(bytes: &[u8]) -> Result<CaptureBundle, CaptureError> {
        let entries = read_store_zip(bytes)?;
        let find = |name: &str| -> Result<&[u8], CaptureError> {
            entries
                .iter()
                .find(|(n, _)| n == name || n.rsplit('/').next() == Some(name))
                .map(|(_, d)| d.as_slice())
                .ok_or_else(|| CaptureError::Manifest(format!("bundle has no {name}")))
        };
        let manifest: serde_json::Value = serde_json::from_slice(find(MANIFEST_FILE)?)?;
        let format = manifest["format"].as_str().unwrap_or_default();
        if !format.starts_with("orbit-capture/") {
            return Err(CaptureError::Manifest(format!("not an Orbit capture: format {format:?}")));
        }
        let events = read_events_ipc(std::io::Cursor::new(find(EVENTS_FILE)?))?;
        let samples = match find(SAMPLES_FILE) {
            Ok(b) => read_samples_ipc(std::io::Cursor::new(b))?,
            Err(_) => Vec::new(),
        };
        let frames = match find(FRAMES_FILE) {
            Ok(b) => read_frames_ipc(std::io::Cursor::new(b))?,
            Err(_) => Vec::new(),
        };
        let b = &manifest["bundle"];
        let slice_ns = match (&b["slice_ns"]["start"], &b["slice_ns"]["end"]) {
            (serde_json::Value::Number(a), serde_json::Value::Number(z)) => {
                Some((a.as_u64().unwrap_or(0), z.as_u64().unwrap_or(0)))
            }
            _ => None,
        };
        let processes = b["processes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| {
                        Some(ProcessName {
                            pid: p["pid"].as_u64()? as u32,
                            name: p["name"].as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let threads = b["threads"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        Some(ThreadName {
                            pid: t["pid"].as_u64()? as u32,
                            tid: t["tid"].as_u64()? as u32,
                            name: t["name"].as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(CaptureBundle {
            target_pid: b["target_pid"].as_u64().unwrap_or(0) as u32,
            slice_ns,
            processes,
            threads,
            events,
            samples,
            frames,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::kind;

    fn ev(start: u64, dur: u64, pid: u32, tid: u32, name_id: u32, name: &str) -> EventRow {
        EventRow {
            event: LiveEvent {
                start_ns: start,
                duration_ns: dur,
                tid,
                pid,
                kind: kind::API_SCOPE,
                depth: 0,
                extra: 0,
                _pad: 0,
                name_id,
            },
            name: name.to_string(),
        }
    }

    fn sample_bundle() -> CaptureBundle {
        CaptureBundle {
            target_pid: 7,
            slice_ns: None,
            processes: vec![
                ProcessName { pid: 7, name: "game".into() },
                ProcessName { pid: 9, name: "other".into() },
            ],
            threads: vec![
                ThreadName { pid: 7, tid: 70, name: "Main".into() },
                ThreadName { pid: 7, tid: 71, name: "Worker".into() },
                ThreadName { pid: 9, tid: 90, name: "Idle".into() },
            ],
            events: vec![
                ev(100, 50, 7, 70, 1, "Tick"),      // 100..150
                ev(140, 100, 7, 71, 2, "Physics"),  // 140..240, straddles 200
                ev(300, 10, 7, 70, 1, "Tick"),      // 300..310
                ev(500, 10, 9, 90, 3, "Sleep"),     // 500..510
            ],
            samples: vec![
                SampleRow { timestamp_ns: 120, tid: 70, frames: vec![1, 2] },
                SampleRow { timestamp_ns: 210, tid: 71, frames: vec![3] },
                SampleRow { timestamp_ns: 505, tid: 90, frames: vec![4] },
            ],
            frames: vec![
                FrameRow { id: 1, name: "main".into(), module: "game".into(), address: 0x10 },
                FrameRow { id: 2, name: "tick".into(), module: "game".into(), address: 0x20 },
                FrameRow { id: 3, name: "physics".into(), module: "game".into(), address: 0x30 },
                FrameRow { id: 4, name: "nanosleep".into(), module: "libc".into(), address: 0x40 },
            ],
        }
    }

    #[test]
    fn a_bundle_round_trips_through_its_zip() {
        let b = sample_bundle();
        let zip = b.to_zip().unwrap();
        let back = CaptureBundle::from_zip(&zip).unwrap();
        assert_eq!(back, b);
        assert_eq!(back.time_bounds(), Some((100, 510)));
    }

    #[test]
    fn a_slice_keeps_straddling_events_whole_and_cuts_samples_by_time() {
        let s = sample_bundle().slice(110, 220);
        assert_eq!(s.slice_ns, Some((110, 220)));
        // Tick (100..150) and Physics (140..240) overlap; the later Tick and
        // Sleep do not.
        let names: Vec<&str> = s.events.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["Tick", "Physics"]);
        assert_eq!(s.events[1].event.duration_ns, 100, "not clipped");
        // Samples at 120 and 210 are in, 505 is out; frame 4 is unreferenced.
        assert_eq!(s.samples.len(), 2);
        let ids: Vec<u32> = s.frames.iter().map(|f| f.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        // Process 9 and its thread had nothing in the window.
        assert_eq!(s.processes.len(), 1);
        assert_eq!(s.processes[0].pid, 7);
        assert_eq!(s.threads.len(), 2);
        // And the slice round-trips like the whole.
        let back = CaptureBundle::from_zip(&s.to_zip().unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn a_reversed_window_and_an_empty_one_are_fine() {
        let b = sample_bundle();
        assert_eq!(b.slice(220, 110), b.slice(110, 220));
        let empty = b.slice(1000, 2000);
        assert!(empty.events.is_empty() && empty.samples.is_empty() && empty.frames.is_empty());
        assert_eq!(empty.time_bounds(), None);
        // The target process is still named, so the capture reads as its.
        assert_eq!(empty.processes.len(), 1);
        let back = CaptureBundle::from_zip(&empty.to_zip().unwrap()).unwrap();
        assert_eq!(back, empty);
    }

    #[test]
    fn names_are_the_distinct_intern_pairs() {
        assert_eq!(
            sample_bundle().names(),
            vec![(1, "Tick".to_string()), (2, "Physics".to_string()), (3, "Sleep".to_string())]
        );
    }

    #[test]
    fn the_zip_extracts_to_a_dataset_directory_the_manifest_reader_accepts() {
        let dir = std::env::temp_dir().join(format!("orbit-bundle-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, data) in read_store_zip(&sample_bundle().to_zip().unwrap()).unwrap() {
            std::fs::write(dir.join(name), data).unwrap();
        }
        let m = crate::read_manifest(&dir).unwrap();
        assert_eq!(m.events, 4);
        assert_eq!(m.samples, 3);
        assert_eq!(m.frames, 4);
        assert_eq!(m.time_bounds_ns, Some((100, 510)));
        let rows = read_events_ipc(std::fs::File::open(dir.join(EVENTS_FILE)).unwrap()).unwrap();
        assert_eq!(rows.len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn something_that_is_not_a_capture_is_refused() {
        assert!(CaptureBundle::from_zip(b"not a zip").is_err());
        let zip = write_store_zip(&[("manifest.json", b"{\"format\":\"something-else/1\"}")]).unwrap();
        let err = CaptureBundle::from_zip(&zip).unwrap_err().to_string();
        assert!(err.contains("not an Orbit capture"), "{err}");
    }
}
