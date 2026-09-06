// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Cutting a time slice out of a bundle on disk without loading it
//! (TODO item 5).
//!
//! A bundle's tables are Parquet in row groups of [`CHUNK_ROWS`] rows, and
//! Parquet keeps the minimum and maximum of every column per row group in
//! the file footer. Events are written in the order the ring held them,
//! which is close to time order, so a row group covers a narrow stretch of
//! the capture and the footer says which stretch. A slice reads the
//! footer, keeps only the row groups whose start range can overlap the
//! window (an event may begin before the window and still be in it, so
//! the group's largest duration is added on), decodes just those, and
//! filters rows exactly. On a multi-gigabyte capture that is a few
//! groups out of thousands; the rest of the file is never read past the
//! footer.
//!
//! The zip is stored, not compressed, so each table's bytes are a
//! contiguous range of the file: the file is read once and sliced by
//! offset, no copies, and the Parquet reader works on the slice.

use std::path::Path;

use bytes::Bytes;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::statistics::Statistics;

use crate::bundle::CaptureBundle;
use crate::zipstore::{stored_entry_ranges, ZipError};
use crate::{
    read_frames_parquet, read_manifest_names, CaptureError, EventRow, SampleRow, EVENTS_FILE,
    EVENTS_PARQUET, FRAMES_PARQUET, MANIFEST_FILE, SAMPLES_PARQUET,
};

/// What a slice touched, for the caller to report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SliceStats {
    pub event_row_groups: usize,
    pub event_row_groups_read: usize,
    pub sample_row_groups: usize,
    pub sample_row_groups_read: usize,
}

/// The min and max of an integer column over a row group, from the footer;
/// `None` when the file carries no statistics for it.
fn column_range(meta: &parquet::file::metadata::RowGroupMetaData, name: &str) -> Option<(u64, u64)> {
    let col = meta.columns().iter().find(|c| c.column_path().string() == name)?;
    match col.statistics()? {
        Statistics::Int64(s) => Some((*s.min_opt()? as u64, *s.max_opt()? as u64)),
        Statistics::Int32(s) => Some((*s.min_opt()? as u64, *s.max_opt()? as u64)),
        _ => None,
    }
}

/// The row groups of an events or samples table that can hold a row
/// overlapping `[t0, t1]`. `start` is the start column, `duration` the
/// duration column or `None` for point rows. A group without statistics is
/// kept: never skip what cannot be ruled out.
fn overlapping_groups(
    builder: &ParquetRecordBatchReaderBuilder<Bytes>,
    start: &str,
    duration: Option<&str>,
    t0: u64,
    t1: u64,
) -> Vec<usize> {
    let meta = builder.metadata();
    (0..meta.num_row_groups())
        .filter(|&i| {
            let rg = meta.row_group(i);
            let Some((min_start, max_start)) = column_range(rg, start) else { return true };
            let max_dur = duration.and_then(|d| column_range(rg, d)).map(|(_, mx)| mx).unwrap_or(0);
            min_start <= t1 && max_start.saturating_add(max_dur) >= t0
        })
        .collect()
}

/// Reads the events overlapping `[t0, t1]` from an events Parquet table,
/// touching only the row groups that can hold one.
pub fn slice_events_parquet(table: Bytes, t0: u64, t1: u64) -> Result<(Vec<EventRow>, usize, usize), CaptureError> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(table)?;
    let total = builder.metadata().num_row_groups();
    let groups = overlapping_groups(&builder, "start_ns", Some("duration_ns"), t0, t1);
    let read = groups.len();
    let reader = builder.with_row_groups(groups).build()?;
    let mut out = Vec::new();
    for batch in reader {
        let mut rows = Vec::new();
        crate::event_rows_from_batch(&batch?, &mut rows)?;
        out.extend(rows.into_iter().filter(|r| r.event.start_ns <= t1 && r.event.end_ns() >= t0));
    }
    Ok((out, total, read))
}

/// Reads the samples inside `[t0, t1]` from a samples Parquet table.
pub fn slice_samples_parquet(table: Bytes, t0: u64, t1: u64) -> Result<(Vec<SampleRow>, usize, usize), CaptureError> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(table)?;
    let total = builder.metadata().num_row_groups();
    let groups = overlapping_groups(&builder, "timestamp_ns", None, t0, t1);
    let read = groups.len();
    let reader = builder.with_row_groups(groups).build()?;
    let mut out = Vec::new();
    for batch in reader {
        let mut rows = Vec::new();
        crate::sample_rows_from_batch(&batch?, &mut rows)?;
        out.extend(rows.into_iter().filter(|s| s.timestamp_ns >= t0 && s.timestamp_ns <= t1));
    }
    Ok((out, total, read))
}

/// The slice `[t0, t1]` of the bundle at `path`, as a bundle. Bundles from
/// before the Parquet tables (Arrow entries, deflated) are loaded whole and
/// sliced in memory, since their tables have no footer to consult.
pub fn slice_bundle_file(path: impl AsRef<Path>, t0: u64, t1: u64) -> Result<(CaptureBundle, SliceStats), CaptureError> {
    let (t0, t1) = (t0.min(t1), t0.max(t1));
    let file = Bytes::from(std::fs::read(path)?);
    slice_bundle_bytes(file, t0, t1)
}

/// As [`slice_bundle_file`] over the bundle's bytes.
pub fn slice_bundle_bytes(file: Bytes, t0: u64, t1: u64) -> Result<(CaptureBundle, SliceStats), CaptureError> {
    let entries = match stored_entry_ranges(&file) {
        Ok(e) => e,
        Err(ZipError::Unsupported(_)) => {
            // Compressed entries: the first-day layout. Load it whole.
            let whole = CaptureBundle::from_zip(&file)?;
            return Ok((whole.slice(t0, t1), SliceStats::default()));
        }
        Err(e) => return Err(e.into()),
    };
    let find = |name: &str| -> Option<Bytes> {
        entries
            .iter()
            .find(|(n, _)| n == name || n.rsplit('/').next() == Some(name))
            .map(|(_, r)| file.slice(r.clone()))
    };
    let Some(manifest) = find(MANIFEST_FILE) else {
        return Err(CaptureError::Manifest("bundle has no manifest.json".into()));
    };
    if find(EVENTS_PARQUET).is_none() && find(EVENTS_FILE).is_some() {
        let whole = CaptureBundle::from_zip(&file)?;
        return Ok((whole.slice(t0, t1), SliceStats::default()));
    }
    let Some(events_table) = find(EVENTS_PARQUET) else {
        return Err(CaptureError::Manifest("bundle has no events table".into()));
    };
    let (target_pid, processes, threads) = read_manifest_names(&manifest)?;
    let (events, eg, egr) = slice_events_parquet(events_table, t0, t1)?;
    let (samples, sg, sgr) = match find(SAMPLES_PARQUET) {
        Some(t) => slice_samples_parquet(t, t0, t1)?,
        None => (Vec::new(), 0, 0),
    };
    let frames = match find(FRAMES_PARQUET) {
        Some(t) => read_frames_parquet(t)?,
        None => Vec::new(),
    };
    let whole = CaptureBundle { target_pid, slice_ns: None, processes, threads, events, samples, frames };
    // `slice` prunes the frames to those referenced and the names to what
    // is left; the events and samples are already inside the window.
    Ok((
        whole.slice(t0, t1),
        SliceStats {
            event_row_groups: eg,
            event_row_groups_read: egr,
            sample_row_groups: sg,
            sample_row_groups_read: sgr,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameRow, ProcessName, ThreadName, CHUNK_ROWS};
    use orbit_live_event::{kind, LiveEvent};

    /// A bundle of `n` events a microsecond apart on two threads, with a
    /// sample every tenth event.
    fn big_bundle(n: u64) -> CaptureBundle {
        let events = (0..n)
            .map(|i| EventRow {
                event: LiveEvent {
                    start_ns: 1_000_000 + i * 1_000,
                    duration_ns: 500,
                    tid: 70 + (i % 2) as u32,
                    pid: 7,
                    kind: kind::API_SCOPE,
                    depth: 0,
                    extra: 0,
                    _pad: 0,
                    name_id: 1 + (i % 5) as u32,
                },
                name: format!("scope{}", i % 5),
            })
            .collect();
        let samples = (0..n / 10)
            .map(|i| SampleRow { timestamp_ns: 1_000_000 + i * 10_000, tid: 70, frames: vec![(i % 3) as u32] })
            .collect();
        CaptureBundle {
            target_pid: 7,
            slice_ns: None,
            processes: vec![ProcessName { pid: 7, name: "game".into() }],
            threads: vec![ThreadName { pid: 7, tid: 70, name: "a".into() }, ThreadName { pid: 7, tid: 71, name: "b".into() }],
            events,
            samples,
            frames: (0..3).map(|i| FrameRow { id: i, name: format!("f{i}"), module: "m".into(), address: i as u64 }).collect(),
        }
    }

    #[test]
    fn a_slice_reads_only_the_row_groups_that_can_overlap_the_window() {
        let n = CHUNK_ROWS as u64 * 6 + 17; // seven row groups
        let b = big_bundle(n);
        let zip = Bytes::from(b.to_zip().unwrap());
        // A window inside the fourth group.
        let t0 = 1_000_000 + CHUNK_ROWS as u64 * 3 * 1_000 + 5_000;
        let t1 = t0 + 20_000;
        let (s, stats) = slice_bundle_bytes(zip.clone(), t0, t1).unwrap();
        assert_eq!(stats.event_row_groups, 7);
        assert_eq!(stats.event_row_groups_read, 1, "{stats:?}");
        let expect = b.slice(t0, t1);
        assert_eq!(s.events, expect.events);
        assert_eq!(s.samples, expect.samples);
        assert_eq!(s.frames, expect.frames);
        assert_eq!(s.threads, expect.threads);
        assert_eq!(s.slice_ns, Some((t0, t1)));
        assert!(s.events.len() > 15 && s.events.len() < 30, "{}", s.events.len());
        // A window spanning a group boundary reads two groups: the last
        // event of the earlier group (starting 1 us before the edge, 500 ns
        // long) still overlaps a window that opens 600 ns before the edge.
        let edge = 1_000_000 + CHUNK_ROWS as u64 * 2 * 1_000;
        let (s2, stats2) = slice_bundle_bytes(zip.clone(), edge - 600, edge + 100).unwrap();
        assert_eq!(stats2.event_row_groups_read, 2);
        assert_eq!(s2.events, b.slice(edge - 600, edge + 100).events);
        assert_eq!(s2.events.len(), 2);
        // Opening 100 ns before the edge misses it, and the footer says so:
        // one group.
        let (s3, stats3) = slice_bundle_bytes(zip.clone(), edge - 100, edge + 100).unwrap();
        assert_eq!(stats3.event_row_groups_read, 1);
        assert_eq!(s3.events.len(), 1);
        // A window past the end reads nothing and is empty, not an error.
        let (s4, stats4) = slice_bundle_bytes(zip, u64::MAX - 10, u64::MAX).unwrap();
        assert_eq!(stats4.event_row_groups_read, 0);
        assert!(s4.events.is_empty() && s4.samples.is_empty());
    }

    #[test]
    fn a_first_day_bundle_with_arrow_tables_is_sliced_in_memory() {
        let b = big_bundle(1000);
        let names: std::collections::HashMap<u32, &str> = b.events.iter().map(|r| (r.event.name_id, r.name.as_str())).collect();
        let events: Vec<LiveEvent> = b.events.iter().map(|r| r.event).collect();
        let mut ev = Vec::new();
        crate::write_events_ipc(std::io::Cursor::new(&mut ev), &events, |id| names[&id].to_string()).unwrap();
        let manifest = serde_json::json!({"format": "orbit-capture/1", "files": {"events": EVENTS_FILE}, "bundle": {"target_pid": 7}}).to_string();
        let zip = crate::zipstore::write_zip(&[(MANIFEST_FILE, manifest.as_bytes()), (EVENTS_FILE, &ev)], Some(6)).unwrap();
        let (s, stats) = slice_bundle_bytes(Bytes::from(zip), 1_050_000, 1_060_000).unwrap();
        assert_eq!(stats, SliceStats::default());
        assert_eq!(s.events.len(), 11);
    }

    /// `ORBIT_SLICE_BENCH_EVENTS=20000000 cargo test --release slice_bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn slice_bench() {
        let n: u64 = std::env::var("ORBIT_SLICE_BENCH_EVENTS").ok().and_then(|v| v.parse().ok()).unwrap_or(5_000_000);
        let b = big_bundle(n);
        let t = std::time::Instant::now();
        let zip = b.to_zip().unwrap();
        println!("{n} events: bundle {:.1} MB written in {:.2} s", zip.len() as f64 / 1e6, t.elapsed().as_secs_f64());
        let path = std::env::temp_dir().join("orbit-slice-bench.orbit.zip");
        std::fs::write(&path, &zip).unwrap();
        drop(zip);
        let span = n * 1_000;
        let t0 = 1_000_000 + span / 2;
        let t1 = t0 + span / 100; // a 1% window
        let t = std::time::Instant::now();
        let (s, stats) = slice_bundle_file(&path, t0, t1).unwrap();
        let by_footer = t.elapsed();
        println!(
            "  slice 1% by footer stats: {} events, {} of {} row groups read, {:.3} s",
            s.events.len(), stats.event_row_groups_read, stats.event_row_groups, by_footer.as_secs_f64()
        );
        let t = std::time::Instant::now();
        let whole = CaptureBundle::from_zip(&std::fs::read(&path).unwrap()).unwrap();
        let s2 = whole.slice(t0, t1);
        let by_load = t.elapsed();
        println!("  slice 1% by loading it all: {} events, {:.3} s", s2.events.len(), by_load.as_secs_f64());
        assert_eq!(s.events, s2.events);
        let _ = std::fs::remove_file(&path);
    }
}
