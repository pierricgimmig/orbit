// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A capture on disk, as Arrow.
//!
//! The viewer's whole model is a flat stream of [`LiveEvent`]s. This writes
//! that stream as an Arrow IPC file so a capture survives the session and can
//! be opened straight into pandas or DuckDB -- `pyarrow.ipc.open_file(...)`,
//! `.read_pandas()`, and every row is there.
//!
//! One table, columnar, with the scope name resolved inline next to its
//! `name_id`: a reader gets human-readable names without carrying a second
//! table and joining on it, while `name_id` is still there for anyone who
//! wants the original ids. The Arrow schema doubles as a self-describing
//! header, so no separate format spec has to travel with the file.
//!
//! Compression is deliberately off (no `lz4`/`zstd` features): the service
//! ships as a static musl binary, and those features would drag a C toolchain
//! into it. Arrow IPC is already a tight columnar layout; a capture that needs
//! more can be re-encoded to Parquet by any consumer once it is loaded.

use std::io::{Read, Seek, Write};
use std::sync::Arc;

use arrow_array::{
    Array, RecordBatch, StringArray, UInt32Array, UInt64Array, UInt8Array,
};
use arrow_ipc::reader::FileReader;
use arrow_ipc::writer::FileWriter;
use arrow_schema::{ArrowError, DataType, Field, Schema};
use orbit_live_event::LiveEvent;

/// A decoded row: a [`LiveEvent`] plus the resolved name it was written with.
/// Reading a capture back yields these -- enough to rebuild the viewer's index
/// or to inspect a capture without the running service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRow {
    pub event: LiveEvent,
    pub name: String,
}

/// The columns of a capture file, in order. Kept as a function so the writer
/// and the reader cannot drift: both name the same fields the same way.
pub fn events_schema() -> Schema {
    Schema::new(vec![
        Field::new("start_ns", DataType::UInt64, false),
        Field::new("duration_ns", DataType::UInt64, false),
        Field::new("pid", DataType::UInt32, false),
        Field::new("tid", DataType::UInt32, false),
        Field::new("kind", DataType::UInt8, false),
        Field::new("depth", DataType::UInt8, false),
        Field::new("extra", DataType::UInt8, false),
        Field::new("name_id", DataType::UInt32, false),
        Field::new("name", DataType::Utf8, false),
    ])
}

/// Builds one record batch from the events, resolving each `name_id` through
/// `resolve`. The batch borrows nothing, so the caller can drop the events.
fn events_batch(
    events: &[LiveEvent],
    resolve: impl Fn(u32) -> String,
) -> Result<RecordBatch, ArrowError> {
    let start: UInt64Array = events.iter().map(|e| e.start_ns).collect();
    let dur: UInt64Array = events.iter().map(|e| e.duration_ns).collect();
    let pid: UInt32Array = events.iter().map(|e| e.pid).collect();
    let tid: UInt32Array = events.iter().map(|e| e.tid).collect();
    let kind: UInt8Array = events.iter().map(|e| e.kind).collect();
    let depth: UInt8Array = events.iter().map(|e| e.depth).collect();
    let extra: UInt8Array = events.iter().map(|e| e.extra).collect();
    let name_id: UInt32Array = events.iter().map(|e| e.name_id).collect();
    let names: Vec<String> = events.iter().map(|e| resolve(e.name_id)).collect();
    let name = StringArray::from(names);

    RecordBatch::try_new(
        Arc::new(events_schema()),
        vec![
            Arc::new(start) as Arc<dyn Array>,
            Arc::new(dur),
            Arc::new(pid),
            Arc::new(tid),
            Arc::new(kind),
            Arc::new(depth),
            Arc::new(extra),
            Arc::new(name_id),
            Arc::new(name),
        ],
    )
}

/// Writes the events as an Arrow IPC file to `writer`.
pub fn write_events_ipc<W: Write>(
    writer: W,
    events: &[LiveEvent],
    resolve: impl Fn(u32) -> String,
) -> Result<(), ArrowError> {
    let schema = events_schema();
    let mut file = FileWriter::try_new(writer, &schema)?;
    // An empty capture is still a valid file: schema, no batches. Writing a
    // zero-row batch would be legal too, but skipping it keeps the file to
    // exactly what there is.
    if !events.is_empty() {
        file.write(&events_batch(events, resolve)?)?;
    }
    file.finish()?;
    Ok(())
}

/// As [`write_events_ipc`], into a freshly allocated buffer -- what the service
/// hands back over HTTP.
pub fn write_events_ipc_to_vec(
    events: &[LiveEvent],
    resolve: impl Fn(u32) -> String,
) -> Result<Vec<u8>, ArrowError> {
    let mut buf = Vec::new();
    write_events_ipc(std::io::Cursor::new(&mut buf), events, resolve)?;
    Ok(buf)
}

/// Reads a capture file back into rows, in file order.
pub fn read_events_ipc<R: Read + Seek>(reader: R) -> Result<Vec<EventRow>, ArrowError> {
    let file = FileReader::try_new(reader, None)?;
    let mut out = Vec::new();
    for batch in file {
        let batch = batch?;
        let col = |i: usize| batch.column(i);
        let u64c = |i: usize| {
            col(i)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .ok_or_else(|| ArrowError::CastError(format!("column {i} is not u64")))
        };
        let u32c = |i: usize| {
            col(i)
                .as_any()
                .downcast_ref::<UInt32Array>()
                .ok_or_else(|| ArrowError::CastError(format!("column {i} is not u32")))
        };
        let u8c = |i: usize| {
            col(i)
                .as_any()
                .downcast_ref::<UInt8Array>()
                .ok_or_else(|| ArrowError::CastError(format!("column {i} is not u8")))
        };
        let start = u64c(0)?;
        let dur = u64c(1)?;
        let pid = u32c(2)?;
        let tid = u32c(3)?;
        let kind = u8c(4)?;
        let depth = u8c(5)?;
        let extra = u8c(6)?;
        let name_id = u32c(7)?;
        let name = col(8)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| ArrowError::CastError("column 8 is not utf8".into()))?;

        for r in 0..batch.num_rows() {
            out.push(EventRow {
                event: LiveEvent {
                    start_ns: start.value(r),
                    duration_ns: dur.value(r),
                    tid: tid.value(r),
                    pid: pid.value(r),
                    kind: kind.value(r),
                    depth: depth.value(r),
                    extra: extra.value(r),
                    _pad: 0,
                    name_id: name_id.value(r),
                },
                name: name.value(r).to_string(),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(start: u64, dur: u64, pid: u32, tid: u32, kind: u8, depth: u8, name_id: u32) -> LiveEvent {
        LiveEvent {
            start_ns: start,
            duration_ns: dur,
            tid,
            pid,
            kind,
            depth,
            extra: 0,
            _pad: 0,
            name_id,
        }
    }

    fn names(id: u32) -> String {
        match id {
            1 => "main".to_string(),
            2 => "work".to_string(),
            _ => format!("<{id}>"),
        }
    }

    #[test]
    fn round_trips_events_with_resolved_names() {
        let events = vec![
            ev(100, 50, 7, 8, 1, 0, 1),
            ev(160, 20, 7, 8, 1, 1, 2),
            ev(200, 0, 7, 9, 3, 0, 99),
        ];
        let bytes = write_events_ipc_to_vec(&events, names).unwrap();
        let rows = read_events_ipc(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].event, events[0]);
        assert_eq!(rows[0].name, "main");
        assert_eq!(rows[1].name, "work");
        // An unknown id keeps whatever the resolver returned.
        assert_eq!(rows[2].name, "<99>");
        assert_eq!(rows[2].event.kind, 3);
    }

    #[test]
    fn an_empty_capture_writes_a_readable_empty_file() {
        let bytes = write_events_ipc_to_vec(&[], names).unwrap();
        // A real Arrow file, with the magic, that reads back as zero rows.
        assert!(bytes.starts_with(b"ARROW1"));
        let rows = read_events_ipc(std::io::Cursor::new(bytes)).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn the_schema_names_every_column_the_reader_expects() {
        let schema = events_schema();
        let cols: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            cols,
            [
                "start_ns",
                "duration_ns",
                "pid",
                "tid",
                "kind",
                "depth",
                "extra",
                "name_id",
                "name",
            ]
        );
    }
}
