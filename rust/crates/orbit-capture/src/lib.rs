// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A capture on disk, as Arrow.
//!
//! The viewer's whole model is a flat stream of [`LiveEvent`]s. This writes
//! that stream as an Arrow IPC file so a capture survives the session and can
//! be opened straight into pandas or DuckDB -- `pyarrow.ipc.open_file(...)`,
//! `.read_pandas()`, and every row is there. The same table can also go out as
//! Parquet ([`write_events_parquet`]) for tools that speak that instead.
//!
//! One events table, columnar, with the scope name resolved inline next to its
//! `name_id`: a reader gets human-readable names without carrying a second
//! table and joining on it, while `name_id` is still there for anyone who
//! wants the original ids. The Arrow schema doubles as a self-describing
//! header, so no separate format spec has to travel with the file.
//!
//! Rows go out in batches of [`CHUNK_ROWS`], so a reader can stream a large
//! capture one batch at a time instead of materialising it whole, and a
//! writer never builds one giant batch.
//!
//! A full capture is more than its events: the sampled callstacks and the
//! frame table they point into live in their own tables, and
//! [`write_dataset`] lays all three out in a directory with a `manifest.json`
//! that names the files and their row counts.
//!
//! Compression is deliberately off (no `lz4`/`zstd`/`snap` features): the
//! service ships as a static musl binary, and those features would drag a C
//! toolchain into it. Arrow IPC is already a tight columnar layout; a capture
//! that needs more can be re-encoded by any consumer once it is loaded.

pub mod bundle;
pub mod zipstore;

pub use bundle::{CaptureBundle, ProcessName, ThreadName, BUNDLE_SUFFIX};

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::builder::{ListBuilder, UInt32Builder};
use arrow_array::{
    Array, ListArray, RecordBatch, StringArray, UInt32Array, UInt64Array, UInt8Array,
};
use arrow_ipc::reader::FileReader;
use arrow_ipc::writer::FileWriter;
use arrow_schema::{ArrowError, DataType, Field, Schema};
use orbit_live_event::LiveEvent;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::errors::ParquetError;
use parquet::file::reader::ChunkReader;

/// Rows per record batch. Large enough that per-batch overhead is noise,
/// small enough that a reader streaming batch by batch never holds much.
pub const CHUNK_ROWS: usize = 65_536;

/// The dataset layout version written into `manifest.json`. Bump when a
/// table's schema changes shape, so a reader can tell what it has.
pub const DATASET_FORMAT: &str = "orbit-capture/1";

/// Anything that can go wrong writing or reading a capture.
#[derive(Debug)]
pub enum CaptureError {
    Arrow(ArrowError),
    Parquet(ParquetError),
    Io(std::io::Error),
    Json(serde_json::Error),
    /// A manifest that does not say what it should.
    Manifest(String),
    Zip(zipstore::ZipError),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::Arrow(e) => write!(f, "arrow: {e}"),
            CaptureError::Parquet(e) => write!(f, "parquet: {e}"),
            CaptureError::Io(e) => write!(f, "io: {e}"),
            CaptureError::Json(e) => write!(f, "json: {e}"),
            CaptureError::Manifest(e) => write!(f, "manifest: {e}"),
            CaptureError::Zip(e) => write!(f, "zip: {e}"),
        }
    }
}

impl std::error::Error for CaptureError {}

impl From<ArrowError> for CaptureError {
    fn from(e: ArrowError) -> Self {
        CaptureError::Arrow(e)
    }
}
impl From<ParquetError> for CaptureError {
    fn from(e: ParquetError) -> Self {
        CaptureError::Parquet(e)
    }
}
impl From<std::io::Error> for CaptureError {
    fn from(e: std::io::Error) -> Self {
        CaptureError::Io(e)
    }
}
impl From<zipstore::ZipError> for CaptureError {
    fn from(e: zipstore::ZipError) -> Self {
        CaptureError::Zip(e)
    }
}
impl From<serde_json::Error> for CaptureError {
    fn from(e: serde_json::Error) -> Self {
        CaptureError::Json(e)
    }
}

/// A decoded row: a [`LiveEvent`] plus the resolved name it was written with.
/// Reading a capture back yields these -- enough to rebuild the viewer's index
/// or to inspect a capture without the running service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRow {
    pub event: LiveEvent,
    pub name: String,
}

/// One sampled callstack: when, on which thread, and the frame ids innermost
/// first, as the unwinder produced them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleRow {
    pub timestamp_ns: u64,
    pub tid: u32,
    pub frames: Vec<u32>,
}

/// What a frame id in a [`SampleRow`] stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameRow {
    pub id: u32,
    pub name: String,
    pub module: String,
    pub address: u64,
}

// ---------------------------------------------------------------- events ---

/// The columns of the events table, in order. Kept as a function so the
/// writer and the reader cannot drift: both name the same fields the same way.
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

/// One record batch from a slice of events, resolving each `name_id` through
/// `resolve`. The batch borrows nothing, so the caller can drop the events.
fn events_batch(
    schema: &Arc<Schema>,
    events: &[LiveEvent],
    resolve: &impl Fn(u32) -> String,
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
        schema.clone(),
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

/// The events as batches of [`CHUNK_ROWS`]. An empty capture yields no
/// batches: the file then carries just its schema, which is still a valid
/// file that reads back as zero rows.
fn events_batches(
    events: &[LiveEvent],
    resolve: impl Fn(u32) -> String,
) -> Result<Vec<RecordBatch>, ArrowError> {
    let schema = Arc::new(events_schema());
    events
        .chunks(CHUNK_ROWS)
        .map(|chunk| events_batch(&schema, chunk, &resolve))
        .collect()
}

/// Writes the events as an Arrow IPC file to `writer`.
pub fn write_events_ipc<W: Write>(
    writer: W,
    events: &[LiveEvent],
    resolve: impl Fn(u32) -> String,
) -> Result<(), ArrowError> {
    let schema = events_schema();
    let mut file = FileWriter::try_new(writer, &schema)?;
    for batch in events_batches(events, resolve)? {
        file.write(&batch)?;
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

/// Writes the events as an (uncompressed) Parquet file to `writer`.
pub fn write_events_parquet<W: Write + Send>(
    writer: W,
    events: &[LiveEvent],
    resolve: impl Fn(u32) -> String,
) -> Result<(), CaptureError> {
    let schema = Arc::new(events_schema());
    let mut file = ArrowWriter::try_new(writer, schema, None)?;
    for batch in events_batches(events, resolve)? {
        file.write(&batch)?;
    }
    file.close()?;
    Ok(())
}

/// As [`write_events_parquet`], into a buffer.
pub fn write_events_parquet_to_vec(
    events: &[LiveEvent],
    resolve: impl Fn(u32) -> String,
) -> Result<Vec<u8>, CaptureError> {
    let mut buf = Vec::new();
    write_events_parquet(&mut buf, events, resolve)?;
    Ok(buf)
}

fn column<'a, T: 'static>(batch: &'a RecordBatch, i: usize, what: &str) -> Result<&'a T, ArrowError> {
    batch
        .column(i)
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| ArrowError::CastError(format!("column {i} is not {what}")))
}

/// Decodes one events batch, whichever container it came from.
fn event_rows_from_batch(batch: &RecordBatch, out: &mut Vec<EventRow>) -> Result<(), ArrowError> {
    let start: &UInt64Array = column(batch, 0, "u64")?;
    let dur: &UInt64Array = column(batch, 1, "u64")?;
    let pid: &UInt32Array = column(batch, 2, "u32")?;
    let tid: &UInt32Array = column(batch, 3, "u32")?;
    let kind: &UInt8Array = column(batch, 4, "u8")?;
    let depth: &UInt8Array = column(batch, 5, "u8")?;
    let extra: &UInt8Array = column(batch, 6, "u8")?;
    let name_id: &UInt32Array = column(batch, 7, "u32")?;
    let name: &StringArray = column(batch, 8, "utf8")?;
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
    Ok(())
}

/// Reads an events IPC file back into rows, in file order.
pub fn read_events_ipc<R: Read + Seek>(reader: R) -> Result<Vec<EventRow>, ArrowError> {
    let file = FileReader::try_new(reader, None)?;
    let mut out = Vec::new();
    for batch in file {
        event_rows_from_batch(&batch?, &mut out)?;
    }
    Ok(out)
}

/// How many record batches an IPC file holds -- the chunk count.
pub fn ipc_batch_count<R: Read + Seek>(reader: R) -> Result<usize, ArrowError> {
    Ok(FileReader::try_new(reader, None)?.num_batches())
}

/// Reads an events Parquet file back into rows. `reader` is anything parquet
/// can seek in: a `std::fs::File`, or `bytes::Bytes` for an in-memory copy.
pub fn read_events_parquet<R: ChunkReader + 'static>(reader: R) -> Result<Vec<EventRow>, CaptureError> {
    let batches = ParquetRecordBatchReaderBuilder::try_new(reader)?.build()?;
    let mut out = Vec::new();
    for batch in batches {
        event_rows_from_batch(&batch?, &mut out)?;
    }
    Ok(out)
}

// --------------------------------------------------------------- samples ---

/// The columns of the samples table: one row per sampled callstack, frames as
/// a list of ids into the frames table, innermost first.
pub fn samples_schema() -> Schema {
    Schema::new(vec![
        Field::new("timestamp_ns", DataType::UInt64, false),
        Field::new("tid", DataType::UInt32, false),
        Field::new(
            "frames",
            DataType::List(Arc::new(Field::new("item", DataType::UInt32, false))),
            false,
        ),
    ])
}

fn samples_batch(schema: &Arc<Schema>, samples: &[SampleRow]) -> Result<RecordBatch, ArrowError> {
    let ts: UInt64Array = samples.iter().map(|s| s.timestamp_ns).collect();
    let tid: UInt32Array = samples.iter().map(|s| s.tid).collect();
    // The list builder's item field must match the schema's, including
    // nullability, or the batch is rejected as a schema mismatch.
    let items = UInt32Builder::new();
    let mut frames = ListBuilder::new(items)
        .with_field(Arc::new(Field::new("item", DataType::UInt32, false)));
    for s in samples {
        frames.values().append_slice(&s.frames);
        frames.append(true);
    }
    RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(ts) as Arc<dyn Array>,
            Arc::new(tid),
            Arc::new(frames.finish()),
        ],
    )
}

/// Writes the samples as an Arrow IPC file, in batches of [`CHUNK_ROWS`].
pub fn write_samples_ipc<W: Write>(writer: W, samples: &[SampleRow]) -> Result<(), ArrowError> {
    let schema = Arc::new(samples_schema());
    let mut file = FileWriter::try_new(writer, &schema)?;
    for chunk in samples.chunks(CHUNK_ROWS) {
        file.write(&samples_batch(&schema, chunk)?)?;
    }
    file.finish()?;
    Ok(())
}

/// Reads a samples IPC file back, in file order.
pub fn read_samples_ipc<R: Read + Seek>(reader: R) -> Result<Vec<SampleRow>, ArrowError> {
    let file = FileReader::try_new(reader, None)?;
    let mut out = Vec::new();
    for batch in file {
        let batch = batch?;
        let ts: &UInt64Array = column(&batch, 0, "u64")?;
        let tid: &UInt32Array = column(&batch, 1, "u32")?;
        let frames: &ListArray = column(&batch, 2, "list")?;
        for r in 0..batch.num_rows() {
            let items = frames.value(r);
            let ids = items
                .as_any()
                .downcast_ref::<UInt32Array>()
                .ok_or_else(|| ArrowError::CastError("frames items are not u32".into()))?;
            out.push(SampleRow {
                timestamp_ns: ts.value(r),
                tid: tid.value(r),
                frames: ids.values().to_vec(),
            });
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- frames ---

/// The columns of the frames table: what each frame id stands for.
pub fn frames_schema() -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::UInt32, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("module", DataType::Utf8, false),
        Field::new("address", DataType::UInt64, false),
    ])
}

fn frames_batch(schema: &Arc<Schema>, frames: &[FrameRow]) -> Result<RecordBatch, ArrowError> {
    let id: UInt32Array = frames.iter().map(|f| f.id).collect();
    let name = StringArray::from(frames.iter().map(|f| f.name.as_str()).collect::<Vec<_>>());
    let module = StringArray::from(frames.iter().map(|f| f.module.as_str()).collect::<Vec<_>>());
    let address: UInt64Array = frames.iter().map(|f| f.address).collect();
    RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(id) as Arc<dyn Array>,
            Arc::new(name),
            Arc::new(module),
            Arc::new(address),
        ],
    )
}

/// Writes the frame table as an Arrow IPC file.
pub fn write_frames_ipc<W: Write>(writer: W, frames: &[FrameRow]) -> Result<(), ArrowError> {
    let schema = Arc::new(frames_schema());
    let mut file = FileWriter::try_new(writer, &schema)?;
    for chunk in frames.chunks(CHUNK_ROWS) {
        file.write(&frames_batch(&schema, chunk)?)?;
    }
    file.finish()?;
    Ok(())
}

/// Reads a frames IPC file back, in file order.
pub fn read_frames_ipc<R: Read + Seek>(reader: R) -> Result<Vec<FrameRow>, ArrowError> {
    let file = FileReader::try_new(reader, None)?;
    let mut out = Vec::new();
    for batch in file {
        let batch = batch?;
        let id: &UInt32Array = column(&batch, 0, "u32")?;
        let name: &StringArray = column(&batch, 1, "utf8")?;
        let module: &StringArray = column(&batch, 2, "utf8")?;
        let address: &UInt64Array = column(&batch, 3, "u64")?;
        for r in 0..batch.num_rows() {
            out.push(FrameRow {
                id: id.value(r),
                name: name.value(r).to_string(),
                module: module.value(r).to_string(),
                address: address.value(r),
            });
        }
    }
    Ok(out)
}

// --------------------------------------------------------------- dataset ---

pub const EVENTS_FILE: &str = "events.arrow";
pub const SAMPLES_FILE: &str = "samples.arrow";
pub const FRAMES_FILE: &str = "frames.arrow";
pub const MANIFEST_FILE: &str = "manifest.json";

/// What `manifest.json` says about a dataset directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub format: String,
    pub events: u64,
    pub samples: u64,
    pub frames: u64,
    /// Earliest event start and latest event end, or `None` for an empty
    /// capture -- a sentinel like `0..u64::MAX` would read as a real span.
    pub time_bounds_ns: Option<(u64, u64)>,
    pub files: Vec<String>,
}

fn time_bounds(events: &[LiveEvent]) -> Option<(u64, u64)> {
    let start = events.iter().map(|e| e.start_ns).min()?;
    let end = events.iter().map(|e| e.end_ns()).max()?;
    Some((start, end))
}

/// Writes a whole capture as a directory: the three tables plus a manifest.
/// Creates `dir` if it is missing; overwrites the files if it is not.
pub fn write_dataset(
    dir: impl AsRef<Path>,
    events: &[LiveEvent],
    resolve: impl Fn(u32) -> String,
    samples: &[SampleRow],
    frames: &[FrameRow],
) -> Result<Manifest, CaptureError> {
    let dir = dir.as_ref();
    std::fs::create_dir_all(dir)?;
    let file = |name: &str| -> Result<std::io::BufWriter<std::fs::File>, CaptureError> {
        Ok(std::io::BufWriter::new(std::fs::File::create(dir.join(name))?))
    };
    write_events_ipc(file(EVENTS_FILE)?, events, resolve)?;
    write_samples_ipc(file(SAMPLES_FILE)?, samples)?;
    write_frames_ipc(file(FRAMES_FILE)?, frames)?;

    let manifest = Manifest {
        format: DATASET_FORMAT.to_string(),
        events: events.len() as u64,
        samples: samples.len() as u64,
        frames: frames.len() as u64,
        time_bounds_ns: time_bounds(events),
        // Sorted, so it compares equal with what `read_manifest` yields from
        // the JSON object (whose keys come back in sorted order).
        files: {
            let mut f = vec![EVENTS_FILE.to_string(), SAMPLES_FILE.to_string(), FRAMES_FILE.to_string()];
            f.sort();
            f
        },
    };
    let json = serde_json::json!({
        "format": manifest.format,
        "rows": {
            "events": manifest.events,
            "samples": manifest.samples,
            "frames": manifest.frames,
        },
        "time_bounds_ns": manifest.time_bounds_ns.map(|(a, b)| serde_json::json!({"start": a, "end": b})),
        "files": {
            "events": EVENTS_FILE,
            "samples": SAMPLES_FILE,
            "frames": FRAMES_FILE,
        },
    });
    std::fs::write(dir.join(MANIFEST_FILE), serde_json::to_string_pretty(&json)?)?;
    Ok(manifest)
}

/// Reads `manifest.json` from a dataset directory.
pub fn read_manifest(dir: impl AsRef<Path>) -> Result<Manifest, CaptureError> {
    let text = std::fs::read_to_string(dir.as_ref().join(MANIFEST_FILE))?;
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let format = v["format"]
        .as_str()
        .ok_or_else(|| CaptureError::Manifest("no format".into()))?
        .to_string();
    let count = |k: &str| -> Result<u64, CaptureError> {
        v["rows"][k]
            .as_u64()
            .ok_or_else(|| CaptureError::Manifest(format!("no rows.{k}")))
    };
    let time_bounds_ns = match (&v["time_bounds_ns"]["start"], &v["time_bounds_ns"]["end"]) {
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
            Some((a.as_u64().unwrap_or(0), b.as_u64().unwrap_or(0)))
        }
        _ => None,
    };
    let mut files: Vec<String> = v["files"]
        .as_object()
        .map(|o| o.values().filter_map(|f| f.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    files.sort();
    Ok(Manifest {
        format,
        events: count("events")?,
        samples: count("samples")?,
        frames: count("frames")?,
        time_bounds_ns,
        files,
    })
}

/// The path of one of a dataset's tables.
pub fn dataset_path(dir: impl AsRef<Path>, file: &str) -> PathBuf {
    dir.as_ref().join(file)
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

    fn many(n: usize) -> Vec<LiveEvent> {
        (0..n)
            .map(|i| ev(i as u64 * 10, 5, 7, (i % 4) as u32, 1, (i % 3) as u8, (i % 3) as u32))
            .collect()
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

    #[test]
    fn a_large_capture_is_written_in_chunks_and_reads_back_whole() {
        // Two full chunks plus a partial third.
        let n = CHUNK_ROWS * 2 + 17;
        let events = many(n);
        let bytes = write_events_ipc_to_vec(&events, names).unwrap();
        assert_eq!(ipc_batch_count(std::io::Cursor::new(bytes.clone())).unwrap(), 3);
        let rows = read_events_ipc(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(rows.len(), n);
        // Order and content survive the chunk boundaries exactly.
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row.event, events[i], "row {i}");
        }
        assert_eq!(rows[CHUNK_ROWS].name, names(events[CHUNK_ROWS].name_id));
    }

    #[test]
    fn exactly_one_chunk_is_one_batch() {
        let bytes = write_events_ipc_to_vec(&many(CHUNK_ROWS), names).unwrap();
        assert_eq!(ipc_batch_count(std::io::Cursor::new(bytes)).unwrap(), 1);
    }

    #[test]
    fn parquet_round_trips_events() {
        let events = vec![
            ev(100, 50, 7, 8, 1, 0, 1),
            ev(160, 20, 7, 8, 1, 1, 2),
            ev(200, 0, 7, 9, 6, 0, 99),
        ];
        let bytes = write_events_parquet_to_vec(&events, names).unwrap();
        // A real Parquet file: magic at both ends.
        assert!(bytes.starts_with(b"PAR1") && bytes.ends_with(b"PAR1"));
        let rows = read_events_parquet(bytes::Bytes::from(bytes)).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].event, events[0]);
        assert_eq!(rows[0].name, "main");
        assert_eq!(rows[2].event.kind, 6);
    }

    #[test]
    fn parquet_survives_chunk_boundaries_too() {
        let n = CHUNK_ROWS + 5;
        let events = many(n);
        let bytes = write_events_parquet_to_vec(&events, names).unwrap();
        let rows = read_events_parquet(bytes::Bytes::from(bytes)).unwrap();
        assert_eq!(rows.len(), n);
        assert_eq!(rows[n - 1].event, events[n - 1]);
    }

    #[test]
    fn samples_round_trip_with_their_frame_lists() {
        let samples = vec![
            SampleRow { timestamp_ns: 10, tid: 7, frames: vec![3, 2, 1] },
            SampleRow { timestamp_ns: 20, tid: 8, frames: vec![] },
            SampleRow { timestamp_ns: 30, tid: 7, frames: vec![4, 1] },
        ];
        let mut buf = Vec::new();
        write_samples_ipc(std::io::Cursor::new(&mut buf), &samples).unwrap();
        let back = read_samples_ipc(std::io::Cursor::new(buf)).unwrap();
        assert_eq!(back, samples);
    }

    #[test]
    fn frames_round_trip() {
        let frames = vec![
            FrameRow { id: 1, name: "main".into(), module: "app".into(), address: 0x1000 },
            FrameRow { id: 2, name: "".into(), module: "".into(), address: 0 },
        ];
        let mut buf = Vec::new();
        write_frames_ipc(std::io::Cursor::new(&mut buf), &frames).unwrap();
        let back = read_frames_ipc(std::io::Cursor::new(buf)).unwrap();
        assert_eq!(back, frames);
    }

    #[test]
    fn a_dataset_writes_three_tables_and_a_manifest_that_read_back() {
        let dir = std::env::temp_dir().join(format!("orbit-capture-ds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let events = many(CHUNK_ROWS + 3);
        let samples = vec![SampleRow { timestamp_ns: 5, tid: 1, frames: vec![2, 1] }];
        let frames = vec![
            FrameRow { id: 1, name: "main".into(), module: "app".into(), address: 1 },
            FrameRow { id: 2, name: "work".into(), module: "app".into(), address: 2 },
        ];
        let written = write_dataset(&dir, &events, names, &samples, &frames).unwrap();
        assert_eq!(written.events, events.len() as u64);
        assert_eq!(written.time_bounds_ns, Some((0, (events.len() as u64 - 1) * 10 + 5)));

        let manifest = read_manifest(&dir).unwrap();
        assert_eq!(manifest, written);
        assert_eq!(manifest.format, DATASET_FORMAT);
        assert_eq!(manifest.files.len(), 3);

        let ev_back = read_events_ipc(std::fs::File::open(dataset_path(&dir, EVENTS_FILE)).unwrap()).unwrap();
        assert_eq!(ev_back.len(), events.len());
        let s_back = read_samples_ipc(std::fs::File::open(dataset_path(&dir, SAMPLES_FILE)).unwrap()).unwrap();
        assert_eq!(s_back, samples);
        let f_back = read_frames_ipc(std::fs::File::open(dataset_path(&dir, FRAMES_FILE)).unwrap()).unwrap();
        assert_eq!(f_back, frames);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_dataset_has_no_time_bounds() {
        let dir = std::env::temp_dir().join(format!("orbit-capture-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let m = write_dataset(&dir, &[], names, &[], &[]).unwrap();
        assert_eq!(m.time_bounds_ns, None);
        assert_eq!(read_manifest(&dir).unwrap().time_bounds_ns, None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
