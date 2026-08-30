// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stream-parse Chrome Trace Event Format JSON (array or `{traceEvents:[…]}`).
//!
//! Bytes are pushed as they arrive. Complete events are deserialized one at a
//! time — never a `Vec<Value>` of the whole file. gzip is decoded into the
//! same scanner; zip is accepted when it holds a single deflated/stored JSON.

use std::io::{Cursor, Read, Write};

use flate2::read::MultiGzDecoder;
use crate::ingest::{ChromeEvent, ChromeIngestor, StackFrame};
use crate::json::{parse_json_string, skip_ws, value_end};
use crate::id::FlexId;

const COMPACT_EVERY: usize = 1 << 20;
/// Skip `args` for memory-dump events so a heap snapshot cannot explode RAM.
const DUMP_SCAN: &[u8] = br#""ph""#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Start,
    ArrayEvents,
    ObjectKey,
    ObjectColon,
    AfterValue,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingKey {
    None,
    TraceEvents,
    DisplayTimeUnit,
    StackFrames,
    Samples,
    Other,
}

enum Decode {
    Identity,
    /// Compressed gzip bytes. Decoded on `finish_input` with MultiGzDecoder
    /// (concatenated members, as in several catapult fixtures).
    Gzip(Vec<u8>),
    ZipPending,
    ZipDeflate {
        dec: Box<flate2::write::DeflateDecoder<Vec<u8>>>,
        remain: u64,
    },
    ZipStored {
        remain: u64,
    },
}

/// Incremental Chrome-trace reader. Feed compressed or raw bytes with
/// [`push`], then [`pump`] into an [`ChromeIngestor`].
pub struct ChromeStream {
    raw: Vec<u8>,
    pos: usize,
    phase: Phase,
    pending: PendingKey,
    decode: Decode,
    magic_buf: Vec<u8>,
    pub bytes_in: u64,
    pub bytes_decoded: u64,
    pub events_seen: u64,
    error: Option<String>,
}

impl Default for ChromeStream {
    fn default() -> Self {
        Self {
            raw: Vec::new(),
            pos: 0,
            phase: Phase::Start,
            pending: PendingKey::None,
            decode: Decode::Identity,
            magic_buf: Vec::new(),
            bytes_in: 0,
            bytes_decoded: 0,
            events_seen: 0,
            error: None,
        }
    }
}

impl ChromeStream {
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done && self.pos >= skip_ws(&self.raw, self.pos)
    }

    pub fn pending_bytes(&self) -> usize {
        self.raw.len().saturating_sub(self.pos)
    }

    /// Append a file chunk. Detects gzip / zip from the first bytes.
    pub fn push(&mut self, chunk: &[u8]) {
        if self.error.is_some() || chunk.is_empty() {
            return;
        }
        self.bytes_in += chunk.len() as u64;
        match &mut self.decode {
            Decode::Identity => {
                if self.bytes_in == chunk.len() as u64 {
                    self.magic_buf.extend_from_slice(chunk);
                    if self.magic_buf.len() < 4 && !looks_like_json(&self.magic_buf) {
                        return;
                    }
                    let magic = self.magic_buf.clone();
                    if magic.len() >= 2 && magic[0] == 0x1f && magic[1] == 0x8b {
                        let held = std::mem::take(&mut self.magic_buf);
                        self.decode = Decode::Gzip(held);
                        return;
                    }
                    if magic.len() >= 4 && magic.starts_with(b"PK\x03\x04") {
                        self.decode = Decode::ZipPending;
                        let held = std::mem::take(&mut self.magic_buf);
                        self.push_zip(&held);
                        return;
                    }
                    self.raw.extend_from_slice(&magic);
                    self.bytes_decoded += magic.len() as u64;
                    self.magic_buf.clear();
                    return;
                }
                self.raw.extend_from_slice(chunk);
                self.bytes_decoded += chunk.len() as u64;
            }
            Decode::Gzip(_) => self.push_gzip(chunk),
            Decode::ZipPending | Decode::ZipDeflate { .. } | Decode::ZipStored { .. } => {
                self.push_zip(chunk);
            }
        }
    }

    fn push_gzip(&mut self, chunk: &[u8]) {
        if let Decode::Gzip(buf) = &mut self.decode {
            buf.extend_from_slice(chunk);
        }
    }

    fn finish_gzip(&mut self) {
        let Decode::Gzip(buf) = &mut self.decode else {
            return;
        };
        let compressed = std::mem::take(buf);
        let mut dec = MultiGzDecoder::new(Cursor::new(compressed));
        let mut out = Vec::new();
        match dec.read_to_end(&mut out) {
            Ok(_) => {
                self.bytes_decoded += out.len() as u64;
                self.raw.extend_from_slice(&out);
                self.decode = Decode::Identity;
            }
            Err(e) => self.error = Some(format!("gzip: {e}")),
        }
    }

    fn push_decoded(&mut self, chunk: &[u8]) {
        match &mut self.decode {
            Decode::Gzip(_) => self.push_gzip(chunk),
            Decode::ZipPending | Decode::ZipDeflate { .. } | Decode::ZipStored { .. } => {
                self.push_zip(chunk)
            }
            Decode::Identity => {
                self.raw.extend_from_slice(chunk);
                self.bytes_decoded += chunk.len() as u64;
            }
        }
    }

    fn push_zip(&mut self, chunk: &[u8]) {
        self.magic_buf.extend_from_slice(chunk);
        loop {
            match &mut self.decode {
                Decode::ZipPending => {
                    if self.magic_buf.len() < 30 {
                        return;
                    }
                    if !self.magic_buf.starts_with(b"PK\x03\x04") {
                        self.error = Some("zip: expected local file header".into());
                        return;
                    }
                    let method = u16::from_le_bytes(self.magic_buf[8..10].try_into().unwrap());
                    let comp = u32::from_le_bytes(self.magic_buf[18..22].try_into().unwrap()) as u64;
                    let name_len =
                        u16::from_le_bytes(self.magic_buf[26..28].try_into().unwrap()) as usize;
                    let extra_len =
                        u16::from_le_bytes(self.magic_buf[28..30].try_into().unwrap()) as usize;
                    let header = 30 + name_len + extra_len;
                    if self.magic_buf.len() < header {
                        return;
                    }
                    let flags = u16::from_le_bytes(self.magic_buf[6..8].try_into().unwrap());
                    if flags & 0x8 != 0 && comp == 0 {
                        self.error = Some("zip: data-descriptor sizes not supported".into());
                        return;
                    }
                    self.magic_buf.drain(..header);
                    match method {
                        0 => self.decode = Decode::ZipStored { remain: comp },
                        8 => {
                            self.decode = Decode::ZipDeflate {
                                dec: Box::new(flate2::write::DeflateDecoder::new(Vec::new())),
                                remain: comp,
                            };
                        }
                        other => {
                            self.error = Some(format!("zip: unsupported method {other}"));
                            return;
                        }
                    }
                }
                Decode::ZipStored { remain } => {
                    let n = (*remain as usize).min(self.magic_buf.len());
                    if n == 0 {
                        return;
                    }
                    let take: Vec<u8> = self.magic_buf.drain(..n).collect();
                    *remain -= n as u64;
                    self.raw.extend_from_slice(&take);
                    self.bytes_decoded += take.len() as u64;
                    if *remain == 0 {
                        self.decode = Decode::Identity;
                    }
                }
                Decode::ZipDeflate { dec, remain } => {
                    let n = (*remain as usize).min(self.magic_buf.len());
                    if n == 0 {
                        return;
                    }
                    let take: Vec<u8> = self.magic_buf.drain(..n).collect();
                    *remain -= n as u64;
                    if let Err(e) = dec.write_all(&take) {
                        self.error = Some(format!("zip deflate: {e}"));
                        return;
                    }
                    if *remain == 0 {
                        if let Err(e) = dec.try_finish() {
                            self.error = Some(format!("zip deflate finish: {e}"));
                            return;
                        }
                    }
                    let out = dec.get_mut();
                    if !out.is_empty() {
                        self.bytes_decoded += out.len() as u64;
                        self.raw.extend_from_slice(out);
                        out.clear();
                    }
                    if *remain == 0 {
                        self.decode = Decode::Identity;
                    }
                }
                _ => return,
            }
        }
    }

    /// Signal end-of-file so gzip/zip can flush.
    pub fn finish_input(&mut self) {
        if matches!(self.decode, Decode::Gzip(_)) {
            self.finish_gzip();
            return;
        }
        match &mut self.decode {
            Decode::ZipDeflate { dec, remain } => {
                if *remain == 0 {
                    let _ = dec.try_finish();
                    let out = dec.get_mut();
                    if !out.is_empty() {
                        self.bytes_decoded += out.len() as u64;
                        self.raw.extend_from_slice(out);
                        out.clear();
                    }
                }
            }
            _ => {}
        }
    }

    /// Deserialize up to `budget` events into `ing`.
    pub fn pump(&mut self, ing: &mut ChromeIngestor, budget: usize) -> Vec<orbit_live_event::LiveEvent> {
        let mut out = Vec::new();
        if self.error.is_some() || budget == 0 {
            return out;
        }
        for _ in 0..budget {
            match self.step(ing, &mut out) {
                Step::NeedBytes | Step::Done => break,
                Step::Event => {}
            }
        }
        self.compact();
        out
    }

    fn compact(&mut self) {
        if self.pos < COMPACT_EVERY {
            return;
        }
        self.raw.drain(..self.pos);
        self.pos = 0;
    }

    fn step(&mut self, ing: &mut ChromeIngestor, out: &mut Vec<orbit_live_event::LiveEvent>) -> Step {
        self.pos = skip_ws(&self.raw, self.pos);
        if self.pos >= self.raw.len() {
            return if self.phase == Phase::Done {
                Step::Done
            } else {
                Step::NeedBytes
            };
        }
        match self.phase {
            Phase::Start => {
                match self.raw[self.pos] {
                    b'[' => {
                        self.pos += 1;
                        self.phase = Phase::ArrayEvents;
                        Step::Event
                    }
                    b'{' => {
                        self.pos += 1;
                        self.phase = Phase::ObjectKey;
                        Step::Event
                    }
                    other => {
                        self.error = Some(format!(
                            "chrome trace: expected [ or {{, got {}",
                            other as char
                        ));
                        Step::Done
                    }
                }
            }
            Phase::ArrayEvents => self.step_array(ing, out),
            Phase::ObjectKey => self.step_object_key(),
            Phase::ObjectColon => self.step_object_colon(ing, out),
            Phase::AfterValue => self.step_after_value(),
            Phase::Done => Step::Done,
        }
    }

    fn step_array(&mut self, ing: &mut ChromeIngestor, out: &mut Vec<orbit_live_event::LiveEvent>) -> Step {
        self.pos = skip_ws(&self.raw, self.pos);
        if self.pos >= self.raw.len() {
            return Step::NeedBytes;
        }
        match self.raw[self.pos] {
            b']' => {
                self.pos += 1;
                if self.pending == PendingKey::TraceEvents {
                    self.pending = PendingKey::None;
                    self.phase = Phase::AfterValue;
                } else {
                    self.phase = Phase::Done;
                }
                Step::Event
            }
            b',' => {
                self.pos += 1;
                Step::Event
            }
            _ => {
                let Some(end) = value_end(&self.raw, self.pos) else {
                    return Step::NeedBytes;
                };
                let slice = &self.raw[self.pos..end];
                self.pos = end;
                match deserialize_event(slice) {
                    Ok(mut ev) => {
                        if self.pending == PendingKey::Samples
                            && ev.ph.as_deref().unwrap_or("").is_empty()
                        {
                            ev.ph = Some("P".into());
                        }
                        out.extend(ing.ingest(ev));
                        self.events_seen += 1;
                    }
                    Err(e) => {
                        if self.events_seen == 0 {
                            self.error = Some(format!("chrome event: {e}"));
                            return Step::Done;
                        }
                    }
                }
                Step::Event
            }
        }
    }

    fn step_object_key(&mut self) -> Step {
        self.pos = skip_ws(&self.raw, self.pos);
        if self.pos >= self.raw.len() {
            return Step::NeedBytes;
        }
        match self.raw[self.pos] {
            b'}' => {
                self.pos += 1;
                self.phase = Phase::Done;
                Step::Event
            }
            b',' => {
                self.pos += 1;
                Step::Event
            }
            b'"' => {
                let Some(end) = value_end(&self.raw, self.pos) else {
                    return Step::NeedBytes;
                };
                let key = parse_json_string(&self.raw[self.pos..end]).unwrap_or_default();
                self.pos = end;
                self.pending = match key.as_str() {
                    "traceEvents" => PendingKey::TraceEvents,
                    "displayTimeUnit" => PendingKey::DisplayTimeUnit,
                    "stackFrames" => PendingKey::StackFrames,
                    "samples" => PendingKey::Samples,
                    _ => PendingKey::Other,
                };
                self.phase = Phase::ObjectColon;
                Step::Event
            }
            other => {
                self.error = Some(format!("chrome object: expected key, got {}", other as char));
                Step::Done
            }
        }
    }

    fn step_object_colon(
        &mut self,
        ing: &mut ChromeIngestor,
        _out: &mut Vec<orbit_live_event::LiveEvent>,
    ) -> Step {
        self.pos = skip_ws(&self.raw, self.pos);
        if self.pos >= self.raw.len() {
            return Step::NeedBytes;
        }
        if self.raw[self.pos] == b':' {
            self.pos += 1;
            self.pos = skip_ws(&self.raw, self.pos);
            if self.pos >= self.raw.len() {
                return Step::NeedBytes;
            }
        }
        match self.pending {
            PendingKey::TraceEvents | PendingKey::Samples => {
                if self.raw[self.pos] == b'[' {
                    self.pos += 1;
                    self.phase = Phase::ArrayEvents;
                    return Step::Event;
                }
                self.skip_value()
            }
            PendingKey::DisplayTimeUnit => {
                let Some(end) = value_end(&self.raw, self.pos) else {
                    return Step::NeedBytes;
                };
                if let Some(s) = parse_json_string(&self.raw[self.pos..end]) {
                    ing.set_display_time_unit(&s);
                }
                self.pos = end;
                self.phase = Phase::AfterValue;
                Step::Event
            }
            PendingKey::StackFrames => {
                let Some(end) = value_end(&self.raw, self.pos) else {
                    return Step::NeedBytes;
                };
                if let Ok(map) =
                    serde_json::from_slice::<std::collections::HashMap<String, StackFrame>>(
                        &self.raw[self.pos..end],
                    )
                {
                    for (id, frame) in map {
                        ing.add_stack_frame(id, frame);
                    }
                }
                self.pos = end;
                self.phase = Phase::AfterValue;
                Step::Event
            }
            PendingKey::Other | PendingKey::None => self.skip_value(),
        }
    }

    fn skip_value(&mut self) -> Step {
        let Some(end) = value_end(&self.raw, self.pos) else {
            return Step::NeedBytes;
        };
        self.pos = end;
        self.phase = Phase::AfterValue;
        Step::Event
    }

    fn step_after_value(&mut self) -> Step {
        self.pos = skip_ws(&self.raw, self.pos);
        if self.pos >= self.raw.len() {
            return Step::NeedBytes;
        }
        match self.raw[self.pos] {
            b',' => {
                self.pos += 1;
                self.phase = Phase::ObjectKey;
                Step::Event
            }
            b'}' => {
                self.pos += 1;
                self.phase = Phase::Done;
                Step::Event
            }
            _ => {
                self.phase = Phase::ObjectKey;
                Step::Event
            }
        }
    }
}

enum Step {
    Event,
    NeedBytes,
    Done,
}

fn looks_like_json(b: &[u8]) -> bool {
    let i = skip_ws(b, 0);
    i < b.len() && (b[i] == b'[' || b[i] == b'{')
}

fn is_memory_dump(bytes: &[u8]) -> bool {
    // Cheap scan: `"ph":"v"` / `"ph": "v"` / `"ph":'v'` — ph is a short field.
    let Some(i) = find_sub(bytes, DUMP_SCAN) else {
        return false;
    };
    let rest = &bytes[i + DUMP_SCAN.len()..];
    let j = skip_ws(rest, 0);
    if j >= rest.len() || rest[j] != b':' {
        return false;
    }
    let k = skip_ws(rest, j + 1);
    matches!(rest.get(k..k + 3), Some(b"\"v\"" | b"'v'")) || rest.get(k) == Some(&b'v')
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn deserialize_event(bytes: &[u8]) -> Result<ChromeEvent, serde_json::Error> {
    if is_memory_dump(bytes) {
        return deserialize_dump_marker(bytes);
    }
    serde_json::from_slice(bytes)
}

fn deserialize_dump_marker(bytes: &[u8]) -> Result<ChromeEvent, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct Slim {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        ph: Option<String>,
        #[serde(default)]
        ts: Option<f64>,
        #[serde(default)]
        pid: Option<FlexId>,
        #[serde(default)]
        tid: Option<FlexId>,
    }
    let slim: Slim = serde_json::from_slice(bytes)?;
    Ok(ChromeEvent {
        name: slim.name,
        cat: None,
        ph: slim.ph,
        ts: slim.ts,
        dur: None,
        pid: slim.pid,
        tid: slim.tid,
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
    })
}

/// One-shot helper for tests and native tools.
pub fn ingest_bytes(bytes: &[u8]) -> Result<ChromeIngestor, String> {
    let mut stream = ChromeStream::default();
    stream.push(bytes);
    stream.finish_input();
    let mut ing = ChromeIngestor::default();
    loop {
        let batch = stream.pump(&mut ing, 64 * 1024);
        if batch.is_empty() {
            if let Some(e) = stream.error() {
                return Err(e.to_string());
            }
            break;
        }
        // Events already recorded on the ingestor via ingest(); pump returns
        // them for the caller to insert. Drain so we do not hold two copies
        // in the one-shot helper — the tests read `ing` after.
        let _ = batch;
    }
    if let Some(e) = stream.error() {
        return Err(e.to_string());
    }
    Ok(ing)
}

/// Ingest and collect emitted [`LiveEvent`]s (including finish()).
pub fn ingest_collect(bytes: &[u8]) -> Result<(ChromeIngestor, Vec<orbit_live_event::LiveEvent>), String> {
    let mut stream = ChromeStream::default();
    stream.push(bytes);
    stream.finish_input();
    let mut ing = ChromeIngestor::default();
    let mut events = Vec::new();
    loop {
        let batch = stream.pump(&mut ing, 64 * 1024);
        if batch.is_empty() {
            if let Some(e) = stream.error() {
                return Err(e.to_string());
            }
            break;
        }
        events.extend(batch);
    }
    let mut max_end = events.iter().map(|e| e.end_ns()).max().unwrap_or(0);
    if max_end == 0 {
        max_end = 1;
    }
    events.extend(ing.finish(max_end));
    Ok((ing, events))
}
