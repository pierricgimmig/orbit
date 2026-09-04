//! Binary framing for the live viewer stream.
//!
//! The service already decoded `ClientCaptureEvent`s. This crate only packs
//! those fields into length-prefixed frames the WASM client can consume
//! without protobuf, ELF, or DWARF.

use orbit_live_event::{LiveEvent, LIVE_EVENT_SIZE};

pub const MAGIC: &[u8; 4] = b"OLIV";
pub const VERSION: u16 = 1;

pub const FRAME_HELLO: u8 = 1;
pub const FRAME_EVENT_BATCH: u8 = 2;
pub const FRAME_INTERNED_STRING: u8 = 3;
pub const FRAME_CAPTURE_STARTED: u8 = 4;
pub const FRAME_CAPTURE_FINISHED: u8 = 5;
pub const FRAME_STATUS: u8 = 6;
/// A thread's name. Sent when the producer knows it (a loaded capture, a
/// service that read `/proc`), replayed to late subscribers like the intern
/// table, so a reopened capture names its tracks without the process alive.
pub const FRAME_THREAD_NAME: u8 = 7;
pub const FRAME_PROCESS_NAME: u8 = 8;
/// An event batch, delta-varint packed (see [`WireFormat::Packed`]). Decodes
/// to the same [`LiveFrame::EventBatch`] as the raw batch.
pub const FRAME_EVENT_BATCH_PACKED: u8 = 9;
/// A packed batch, deflated on top (see [`WireFormat::Deflate`]).
pub const FRAME_EVENT_BATCH_DEFLATE: u8 = 10;

/// Sanity cap on a decoded batch, so a corrupt count cannot ask for gigabytes.
const MAX_BATCH_EVENTS: usize = 1 << 24;

/// How event batches go over the wire. Every decoder accepts every format;
/// the producer picks one, so this is a per-server toggle, not a negotiation.
///
/// The event is 32 fixed bytes; that is what `Raw` sends, and what the ring
/// stores, so a raw batch is one memcpy. But most of those bytes are zero:
/// a 64-bit timestamp whose difference from the previous event fits in
/// three bytes, a pid and tid that repeat all batch long, a name id under
/// 65,536. `Packed` spends bytes on what varies -- the start as a zigzag
/// varint delta from the previous event, the duration and name id as
/// varints, the thread as a one-byte index into a per-batch table -- and on
/// a real capture is about 11 bytes an event to raw's 32. `Deflate` runs
/// miniz over the packed bytes for another 40% at a few hundred
/// microseconds per batch. The measurements are in
/// `docs/blog/metrics/phase-10-wire-and-bundle-compression.txt`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WireFormat {
    Raw,
    #[default]
    Packed,
    Deflate,
}

impl WireFormat {
    /// `raw`, `packed` or `deflate`; anything else is `None`.
    pub fn parse(text: &str) -> Option<WireFormat> {
        match text.trim().to_ascii_lowercase().as_str() {
            "raw" => Some(WireFormat::Raw),
            "packed" => Some(WireFormat::Packed),
            "deflate" => Some(WireFormat::Deflate),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            WireFormat::Raw => "raw",
            WireFormat::Packed => "packed",
            WireFormat::Deflate => "deflate",
        }
    }
}

/// Deflate level for [`WireFormat::Deflate`]: the fast end. Level 1 gets
/// nearly all of level 6's saving on packed batches at a third of the time.
const WIRE_DEFLATE_LEVEL: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveFrame {
    Hello {
        version: u16,
        event_size: u16,
    },
    EventBatch {
        events: Vec<LiveEvent>,
    },
    InternedString {
        id: u32,
        text: String,
    },
    CaptureStarted {
        pid: u32,
        start_ns: u64,
    },
    CaptureFinished,
    Status {
        capturing: bool,
        demo: bool,
        events_live: u64,
        events_capacity: u64,
        dropped: u64,
        spilled: u64,
        produced: u64,
        oldest_start_ns: u64,
        newest_end_ns: u64,
        ring_bytes: u64,
    },
    ThreadName {
        pid: u32,
        tid: u32,
        name: String,
    },
    ProcessName {
        pid: u32,
        name: String,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProtocolError {
    Truncated,
    BadMagic,
    UnsupportedVersion(u16),
    UnknownFrame(u8),
    InvalidPayload(&'static str),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::Truncated => write!(f, "truncated frame"),
            ProtocolError::BadMagic => write!(f, "bad hello magic"),
            ProtocolError::UnsupportedVersion(v) => write!(f, "unsupported protocol version {v}"),
            ProtocolError::UnknownFrame(t) => write!(f, "unknown frame type {t}"),
            ProtocolError::InvalidPayload(s) => write!(f, "invalid payload: {s}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

pub fn encode_frame(frame: &LiveFrame) -> Vec<u8> {
    let mut payload = Vec::new();
    let typ = match frame {
        LiveFrame::Hello {
            version,
            event_size,
        } => {
            payload.extend_from_slice(MAGIC);
            payload.extend_from_slice(&version.to_le_bytes());
            payload.extend_from_slice(&event_size.to_le_bytes());
            FRAME_HELLO
        }
        LiveFrame::EventBatch { events } => {
            payload.extend_from_slice(&(events.len() as u32).to_le_bytes());
            payload.reserve(events.len() * LIVE_EVENT_SIZE);
            for ev in events {
                payload.extend_from_slice(&ev.as_bytes());
            }
            FRAME_EVENT_BATCH
        }
        LiveFrame::InternedString { id, text } => {
            let bytes = text.as_bytes();
            payload.extend_from_slice(&id.to_le_bytes());
            payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            payload.extend_from_slice(bytes);
            FRAME_INTERNED_STRING
        }
        LiveFrame::CaptureStarted { pid, start_ns } => {
            payload.extend_from_slice(&pid.to_le_bytes());
            payload.extend_from_slice(&start_ns.to_le_bytes());
            FRAME_CAPTURE_STARTED
        }
        LiveFrame::CaptureFinished => FRAME_CAPTURE_FINISHED,
        LiveFrame::Status {
            capturing,
            demo,
            events_live,
            events_capacity,
            dropped,
            spilled,
            produced,
            oldest_start_ns,
            newest_end_ns,
            ring_bytes,
        } => {
            payload.push(u8::from(*capturing));
            payload.push(u8::from(*demo));
            payload.extend_from_slice(&0u16.to_le_bytes());
            for v in [
                events_live,
                events_capacity,
                dropped,
                spilled,
                produced,
                oldest_start_ns,
                newest_end_ns,
                ring_bytes,
            ] {
                payload.extend_from_slice(&v.to_le_bytes());
            }
            FRAME_STATUS
        }
        LiveFrame::ThreadName { pid, tid, name } => {
            let bytes = name.as_bytes();
            payload.extend_from_slice(&pid.to_le_bytes());
            payload.extend_from_slice(&tid.to_le_bytes());
            payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            payload.extend_from_slice(bytes);
            FRAME_THREAD_NAME
        }
        LiveFrame::ProcessName { pid, name } => {
            let bytes = name.as_bytes();
            payload.extend_from_slice(&pid.to_le_bytes());
            payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            payload.extend_from_slice(bytes);
            FRAME_PROCESS_NAME
        }
    };
    let mut out = Vec::with_capacity(5 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.push(typ);
    out.extend_from_slice(&payload);
    out
}

/// `encode_frame(&LiveFrame::EventBatch { events })` without building the
/// enum: the frame is written straight from the slice into one buffer sized
/// up front. This is what the server sends per pass, so it neither clones the
/// batch into a Vec nor copies the payload a second time into the frame.
pub fn encode_event_batch(events: &[LiveEvent]) -> Vec<u8> {
    let payload_len = 4 + events.len() * LIVE_EVENT_SIZE;
    let mut out = Vec::with_capacity(5 + payload_len);
    out.extend_from_slice(&(payload_len as u32).to_le_bytes());
    out.push(FRAME_EVENT_BATCH);
    out.extend_from_slice(&(events.len() as u32).to_le_bytes());
    for ev in events {
        out.extend_from_slice(&ev.as_bytes());
    }
    out
}

/// A `u32` length followed by that many UTF-8 bytes.
fn utf8_field(bytes: &[u8], what: &'static str) -> Result<String, ProtocolError> {
    if bytes.len() < 4 {
        return Err(ProtocolError::Truncated);
    }
    let len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    if bytes.len() < 4 + len {
        return Err(ProtocolError::Truncated);
    }
    std::str::from_utf8(&bytes[4..4 + len])
        .map(str::to_string)
        .map_err(|_| ProtocolError::InvalidPayload(match what {
            "thread name" => "thread name is not utf-8",
            _ => "process name is not utf-8",
        }))
}

// ------------------------------------------------------------ varints ---

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Reads a varint at `*pos`, advancing it. Ten bytes at most; more is an
/// error rather than a silent wrap.
fn get_varint(bytes: &[u8], pos: &mut usize) -> Result<u64, ProtocolError> {
    let mut v = 0u64;
    let mut shift = 0u32;
    loop {
        let b = *bytes.get(*pos).ok_or(ProtocolError::Truncated)?;
        *pos += 1;
        if shift >= 64 {
            return Err(ProtocolError::InvalidPayload("varint too long"));
        }
        v |= u64::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
        shift += 7;
    }
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

/// The packed payload of a batch: see [`WireFormat::Packed`].
pub fn pack_events(events: &[LiveEvent]) -> Vec<u8> {
    // Per-batch thread table, in order of first appearance.
    let mut threads: Vec<(u32, u32)> = Vec::new();
    let mut index: Vec<u32> = Vec::with_capacity(events.len());
    for e in events {
        let key = (e.pid, e.tid);
        let i = match threads.iter().position(|t| *t == key) {
            Some(i) => i,
            None => {
                threads.push(key);
                threads.len() - 1
            }
        };
        index.push(i as u32);
    }
    let mut out = Vec::with_capacity(events.len() * 12 + threads.len() * 8 + 8);
    put_varint(&mut out, events.len() as u64);
    put_varint(&mut out, threads.len() as u64);
    for (pid, tid) in &threads {
        put_varint(&mut out, u64::from(*pid));
        put_varint(&mut out, u64::from(*tid));
    }
    let mut prev = 0u64;
    for (e, i) in events.iter().zip(index) {
        put_varint(&mut out, zigzag(e.start_ns.wrapping_sub(prev) as i64));
        prev = e.start_ns;
        put_varint(&mut out, e.duration_ns);
        put_varint(&mut out, u64::from(i));
        out.push(e.kind);
        out.push(e.depth);
        out.push(e.extra);
        put_varint(&mut out, u64::from(e.name_id));
    }
    out
}

/// The inverse of [`pack_events`].
pub fn unpack_events(payload: &[u8]) -> Result<Vec<LiveEvent>, ProtocolError> {
    let mut pos = 0usize;
    let count = get_varint(payload, &mut pos)? as usize;
    let nthreads = get_varint(payload, &mut pos)? as usize;
    // Each event is at least 7 bytes, each thread at least 2: a count the
    // payload cannot hold is corruption, not a large batch.
    if count > MAX_BATCH_EVENTS || count * 7 > payload.len() || nthreads * 2 > payload.len() {
        return Err(ProtocolError::InvalidPayload("packed batch count exceeds payload"));
    }
    let mut threads: Vec<(u32, u32)> = Vec::with_capacity(nthreads);
    for _ in 0..nthreads {
        let pid = get_varint(payload, &mut pos)?;
        let tid = get_varint(payload, &mut pos)?;
        if pid > u64::from(u32::MAX) || tid > u64::from(u32::MAX) {
            return Err(ProtocolError::InvalidPayload("thread id out of range"));
        }
        threads.push((pid as u32, tid as u32));
    }
    let mut events = Vec::with_capacity(count);
    let mut prev = 0u64;
    for _ in 0..count {
        let delta = unzigzag(get_varint(payload, &mut pos)?);
        let start_ns = prev.wrapping_add(delta as u64);
        prev = start_ns;
        let duration_ns = get_varint(payload, &mut pos)?;
        let ti = get_varint(payload, &mut pos)? as usize;
        let (pid, tid) = *threads.get(ti).ok_or(ProtocolError::InvalidPayload("thread index out of range"))?;
        if pos + 3 > payload.len() {
            return Err(ProtocolError::Truncated);
        }
        let (kind, depth, extra) = (payload[pos], payload[pos + 1], payload[pos + 2]);
        pos += 3;
        let name_id = get_varint(payload, &mut pos)?;
        if name_id > u64::from(u32::MAX) {
            return Err(ProtocolError::InvalidPayload("name id out of range"));
        }
        events.push(LiveEvent {
            start_ns,
            duration_ns,
            tid,
            pid,
            kind,
            depth,
            extra,
            _pad: 0,
            name_id: name_id as u32,
        });
    }
    Ok(events)
}

/// Frames a batch in the given format. `Raw` is [`encode_event_batch`].
pub fn encode_event_batch_with(events: &[LiveEvent], wire: WireFormat) -> Vec<u8> {
    match wire {
        WireFormat::Raw => encode_event_batch(events),
        WireFormat::Packed => {
            let payload = pack_events(events);
            let mut out = Vec::with_capacity(5 + payload.len());
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.push(FRAME_EVENT_BATCH_PACKED);
            out.extend_from_slice(&payload);
            out
        }
        WireFormat::Deflate => {
            let packed = pack_events(events);
            let deflated = miniz_oxide::deflate::compress_to_vec(&packed, WIRE_DEFLATE_LEVEL);
            let mut payload = Vec::with_capacity(deflated.len() + 10);
            put_varint(&mut payload, packed.len() as u64);
            payload.extend_from_slice(&deflated);
            let mut out = Vec::with_capacity(5 + payload.len());
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.push(FRAME_EVENT_BATCH_DEFLATE);
            out.extend_from_slice(&payload);
            out
        }
    }
}

/// Inflates a [`FRAME_EVENT_BATCH_DEFLATE`] payload back to its packed form.
fn inflate_batch(payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let mut pos = 0usize;
    let packed_len = get_varint(payload, &mut pos)? as usize;
    if packed_len > MAX_BATCH_EVENTS * 40 {
        return Err(ProtocolError::InvalidPayload("deflated batch claims an absurd size"));
    }
    let out = miniz_oxide::inflate::decompress_to_vec_with_limit(&payload[pos..], packed_len)
        .map_err(|_| ProtocolError::InvalidPayload("deflate stream is corrupt"))?;
    if out.len() != packed_len {
        return Err(ProtocolError::InvalidPayload("deflated batch length mismatch"));
    }
    Ok(out)
}

pub fn decode_frame(bytes: &[u8]) -> Result<(LiveFrame, usize), ProtocolError> {
    if bytes.len() < 5 {
        return Err(ProtocolError::Truncated);
    }
    let payload_len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let typ = bytes[4];
    let total = 5 + payload_len;
    if bytes.len() < total {
        return Err(ProtocolError::Truncated);
    }
    let payload = &bytes[5..total];
    let frame = decode_payload(typ, payload)?;
    Ok((frame, total))
}

fn decode_payload(typ: u8, payload: &[u8]) -> Result<LiveFrame, ProtocolError> {
    match typ {
        FRAME_HELLO => {
            if payload.len() < 8 {
                return Err(ProtocolError::Truncated);
            }
            if &payload[0..4] != MAGIC {
                return Err(ProtocolError::BadMagic);
            }
            let version = u16::from_le_bytes(payload[4..6].try_into().unwrap());
            if version != VERSION {
                return Err(ProtocolError::UnsupportedVersion(version));
            }
            let event_size = u16::from_le_bytes(payload[6..8].try_into().unwrap());
            Ok(LiveFrame::Hello {
                version,
                event_size,
            })
        }
        FRAME_EVENT_BATCH => {
            if payload.len() < 4 {
                return Err(ProtocolError::Truncated);
            }
            let count = u32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize;
            let need = 4 + count * LIVE_EVENT_SIZE;
            if payload.len() < need {
                return Err(ProtocolError::Truncated);
            }
            let mut events = Vec::with_capacity(count);
            for i in 0..count {
                let off = 4 + i * LIVE_EVENT_SIZE;
                let mut arr = [0u8; LIVE_EVENT_SIZE];
                arr.copy_from_slice(&payload[off..off + LIVE_EVENT_SIZE]);
                events.push(LiveEvent::from_bytes(&arr));
            }
            Ok(LiveFrame::EventBatch { events })
        }
        FRAME_INTERNED_STRING => {
            if payload.len() < 8 {
                return Err(ProtocolError::Truncated);
            }
            let id = u32::from_le_bytes(payload[0..4].try_into().unwrap());
            let len = u32::from_le_bytes(payload[4..8].try_into().unwrap()) as usize;
            if payload.len() < 8 + len {
                return Err(ProtocolError::Truncated);
            }
            let text = std::str::from_utf8(&payload[8..8 + len])
                .map_err(|_| ProtocolError::InvalidPayload("interned string is not utf-8"))?
                .to_string();
            Ok(LiveFrame::InternedString { id, text })
        }
        FRAME_CAPTURE_STARTED => {
            if payload.len() < 12 {
                return Err(ProtocolError::Truncated);
            }
            let pid = u32::from_le_bytes(payload[0..4].try_into().unwrap());
            let start_ns = u64::from_le_bytes(payload[4..12].try_into().unwrap());
            Ok(LiveFrame::CaptureStarted { pid, start_ns })
        }
        FRAME_CAPTURE_FINISHED => Ok(LiveFrame::CaptureFinished),
        FRAME_STATUS => {
            if payload.len() < 4 + 8 * 8 {
                return Err(ProtocolError::Truncated);
            }
            let capturing = payload[0] != 0;
            let demo = payload[1] != 0;
            let mut vals = [0u64; 8];
            for (i, v) in vals.iter_mut().enumerate() {
                let off = 4 + i * 8;
                *v = u64::from_le_bytes(payload[off..off + 8].try_into().unwrap());
            }
            Ok(LiveFrame::Status {
                capturing,
                demo,
                events_live: vals[0],
                events_capacity: vals[1],
                dropped: vals[2],
                spilled: vals[3],
                produced: vals[4],
                oldest_start_ns: vals[5],
                newest_end_ns: vals[6],
                ring_bytes: vals[7],
            })
        }
        FRAME_EVENT_BATCH_PACKED => Ok(LiveFrame::EventBatch { events: unpack_events(payload)? }),
        FRAME_EVENT_BATCH_DEFLATE => {
            let packed = inflate_batch(payload)?;
            Ok(LiveFrame::EventBatch { events: unpack_events(&packed)? })
        }
        FRAME_THREAD_NAME => {
            if payload.len() < 12 {
                return Err(ProtocolError::Truncated);
            }
            let pid = u32::from_le_bytes(payload[0..4].try_into().unwrap());
            let tid = u32::from_le_bytes(payload[4..8].try_into().unwrap());
            let name = utf8_field(&payload[8..], "thread name")?;
            Ok(LiveFrame::ThreadName { pid, tid, name })
        }
        FRAME_PROCESS_NAME => {
            if payload.len() < 8 {
                return Err(ProtocolError::Truncated);
            }
            let pid = u32::from_le_bytes(payload[0..4].try_into().unwrap());
            let name = utf8_field(&payload[4..], "process name")?;
            Ok(LiveFrame::ProcessName { pid, name })
        }
        other => Err(ProtocolError::UnknownFrame(other)),
    }
}

/// Decode a concatenated stream of frames.
pub fn decode_all(mut bytes: &[u8]) -> Result<Vec<LiveFrame>, ProtocolError> {
    let mut frames = Vec::new();
    while !bytes.is_empty() {
        let (frame, n) = decode_frame(bytes)?;
        frames.push(frame);
        bytes = &bytes[n..];
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_live_event::kind;

    fn sample_event(i: u32) -> LiveEvent {
        LiveEvent {
            start_ns: i as u64 * 100,
            duration_ns: 40,
            tid: 7,
            pid: 3,
            kind: kind::API_SCOPE,
            depth: 1,
            extra: 0,
            _pad: 0,
            name_id: i,
        }
    }

    fn roundtrip(frame: LiveFrame) {
        let encoded = encode_frame(&frame);
        let (decoded, n) = decode_frame(&encoded).unwrap();
        assert_eq!(n, encoded.len());
        assert_eq!(decoded, frame);
    }

    #[test]
    fn hello_roundtrip() {
        roundtrip(LiveFrame::Hello {
            version: VERSION,
            event_size: LIVE_EVENT_SIZE as u16,
        });
    }

    #[test]
    fn event_batch_roundtrip() {
        roundtrip(LiveFrame::EventBatch {
            events: vec![sample_event(1), sample_event(2), sample_event(3)],
        });
    }

    #[test]
    fn interned_string_roundtrip() {
        roundtrip(LiveFrame::InternedString {
            id: 12,
            text: "ORBIT_SCOPE".to_string(),
        });
    }

    #[test]
    fn capture_lifecycle_and_status_roundtrip() {
        roundtrip(LiveFrame::CaptureStarted {
            pid: 99,
            start_ns: 1_000_000,
        });
        roundtrip(LiveFrame::CaptureFinished);
        roundtrip(LiveFrame::Status {
            capturing: true,
            demo: false,
            events_live: 10,
            events_capacity: 100,
            dropped: 1,
            spilled: 2,
            produced: 12,
            oldest_start_ns: 5,
            newest_end_ns: 50,
            ring_bytes: 3200,
        });
    }

    #[test]
    fn thread_and_process_names_round_trip() {
        for frame in [
            LiveFrame::ThreadName { pid: 7, tid: 70, name: "Worker-1".into() },
            LiveFrame::ProcessName { pid: 7, name: "game".into() },
            LiveFrame::ThreadName { pid: 1, tid: 1, name: String::new() },
        ] {
            let bytes = encode_frame(&frame);
            let (back, used) = decode_frame(&bytes).unwrap();
            assert_eq!(back, frame);
            assert_eq!(used, bytes.len());
        }
        // A name cut short is truncated, not garbage.
        let bytes = encode_frame(&LiveFrame::ThreadName { pid: 7, tid: 70, name: "Worker-1".into() });
        let mut short = bytes.clone();
        short[0] = 10; // payload length claims fewer bytes than the name needs
        assert_eq!(decode_frame(&short[..15]).unwrap_err(), ProtocolError::Truncated);
    }

    /// A batch shaped like a capture: a few threads, sorted starts a few
    /// microseconds apart, short durations, small name ids.
    fn capture_like_batch(n: usize) -> Vec<LiveEvent> {
        let mut t = 1_700_000_000_000_000u64;
        (0..n)
            .map(|i| {
                t += 1_000 + (i as u64 * 7919) % 20_000;
                LiveEvent {
                    start_ns: t,
                    duration_ns: 500 + (i as u64 * 104_729) % 90_000,
                    tid: 4_000 + (i as u32 % 6),
                    pid: 4_000,
                    kind: 1 + (i as u8 % 3),
                    depth: (i as u8 % 5),
                    extra: (i as u8 % 8),
                    _pad: 0,
                    name_id: 100 + (i as u32 % 40),
                }
            })
            .collect()
    }

    #[test]
    fn packed_and_deflated_batches_decode_to_the_raw_batch() {
        let events = capture_like_batch(2048);
        let raw = encode_event_batch(&events);
        for wire in [WireFormat::Raw, WireFormat::Packed, WireFormat::Deflate] {
            let bytes = encode_event_batch_with(&events, wire);
            let (frame, used) = decode_frame(&bytes).unwrap();
            assert_eq!(used, bytes.len(), "{wire:?}");
            assert_eq!(frame, LiveFrame::EventBatch { events: events.clone() }, "{wire:?}");
            if wire == WireFormat::Raw {
                assert_eq!(bytes, raw);
            }
        }
        // The empty batch is fine in every format.
        for wire in [WireFormat::Raw, WireFormat::Packed, WireFormat::Deflate] {
            let (frame, _) = decode_frame(&encode_event_batch_with(&[], wire)).unwrap();
            assert_eq!(frame, LiveFrame::EventBatch { events: vec![] });
        }
    }

    #[test]
    fn packing_handles_out_of_order_starts_and_extreme_values() {
        let events = vec![
            LiveEvent { start_ns: u64::MAX, duration_ns: u64::MAX, tid: u32::MAX, pid: u32::MAX, kind: 255, depth: 255, extra: 255, _pad: 0, name_id: u32::MAX },
            LiveEvent { start_ns: 0, duration_ns: 0, tid: 0, pid: 0, kind: 0, depth: 0, extra: 0, _pad: 0, name_id: 0 },
            LiveEvent { start_ns: 5, duration_ns: 1, tid: 0, pid: 7, kind: 1, depth: 0, extra: 0, _pad: 0, name_id: 1 },
            LiveEvent { start_ns: 3, duration_ns: 1, tid: 0, pid: 7, kind: 1, depth: 0, extra: 0, _pad: 0, name_id: 1 },
        ];
        let packed = pack_events(&events);
        assert_eq!(unpack_events(&packed).unwrap(), events);
    }

    #[test]
    fn packed_is_a_third_of_raw_on_capture_like_events() {
        let events = capture_like_batch(2048);
        let raw = encode_event_batch(&events).len() as f64;
        let packed = encode_event_batch_with(&events, WireFormat::Packed).len() as f64;
        let deflated = encode_event_batch_with(&events, WireFormat::Deflate).len() as f64;
        let per = |b: f64| b / events.len() as f64;
        assert!((per(raw) - 32.0).abs() < 0.01);
        assert!(per(packed) < 12.0, "packed {} B/event", per(packed));
        assert!(deflated < packed, "deflate {} vs packed {}", per(deflated), per(packed));
    }

    #[test]
    fn corrupt_packed_batches_are_errors_not_panics() {
        let events = capture_like_batch(64);
        let bytes = encode_event_batch_with(&events, WireFormat::Packed);
        for cut in 5..bytes.len() {
            let mut short = bytes[..cut].to_vec();
            // Fix the length prefix so the cut is inside the payload.
            let len = (cut - 5) as u32;
            short[0..4].copy_from_slice(&len.to_le_bytes());
            assert!(decode_frame(&short).is_err(), "cut at {cut}");
        }
        // A count the payload cannot hold.
        let mut huge = vec![0u8; 5];
        let mut payload = Vec::new();
        put_varint(&mut payload, 1 << 40);
        put_varint(&mut payload, 1);
        huge[0..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        huge[4] = FRAME_EVENT_BATCH_PACKED;
        huge.extend_from_slice(&payload);
        assert!(matches!(decode_frame(&huge), Err(ProtocolError::InvalidPayload(_))));
        // A deflate stream that is not one.
        let mut bad = vec![0u8; 5];
        let mut payload = Vec::new();
        put_varint(&mut payload, 100);
        payload.extend_from_slice(b"not deflate at all");
        bad[0..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        bad[4] = FRAME_EVENT_BATCH_DEFLATE;
        bad.extend_from_slice(&payload);
        assert!(matches!(decode_frame(&bad), Err(ProtocolError::InvalidPayload(_))));
    }

    #[test]
    fn wire_format_parses_its_names() {
        assert_eq!(WireFormat::parse("packed"), Some(WireFormat::Packed));
        assert_eq!(WireFormat::parse(" Deflate "), Some(WireFormat::Deflate));
        assert_eq!(WireFormat::parse("raw"), Some(WireFormat::Raw));
        assert_eq!(WireFormat::parse("gzip"), None);
        assert_eq!(WireFormat::default().name(), "packed");
    }

    /// `cargo test --release -p orbit-live-protocol wire_bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn wire_bench() {
        let events = capture_like_batch(10_000);
        for wire in [WireFormat::Raw, WireFormat::Packed, WireFormat::Deflate] {
            let t0 = std::time::Instant::now();
            let mut bytes = Vec::new();
            for _ in 0..200 {
                bytes = encode_event_batch_with(&events, wire);
            }
            let enc = t0.elapsed().as_secs_f64() / 200.0;
            let t0 = std::time::Instant::now();
            for _ in 0..200 {
                let _ = decode_frame(&bytes).unwrap();
            }
            let dec = t0.elapsed().as_secs_f64() / 200.0;
            println!(
                "{:8} {:6.2} B/event  encode {:8.1} us/batch  decode {:8.1} us/batch  (10,000 events)",
                wire.name(),
                bytes.len() as f64 / events.len() as f64,
                enc * 1e6,
                dec * 1e6
            );
        }
    }

    #[test]
    fn decode_all_concatenated() {
        let mut buf = encode_frame(&LiveFrame::Hello {
            version: VERSION,
            event_size: LIVE_EVENT_SIZE as u16,
        });
        buf.extend_from_slice(&encode_frame(&LiveFrame::EventBatch {
            events: vec![sample_event(8)],
        }));
        let frames = decode_all(&buf).unwrap();
        assert_eq!(frames.len(), 2);
        match &frames[1] {
            LiveFrame::EventBatch { events } => assert_eq!(events[0].name_id, 8),
            _ => panic!("expected batch"),
        }
    }

    #[test]
    fn truncated_is_error() {
        let encoded = encode_frame(&LiveFrame::EventBatch {
            events: vec![sample_event(1)],
        });
        assert_eq!(
            decode_frame(&encoded[..encoded.len() - 1]),
            Err(ProtocolError::Truncated)
        );
    }

    #[test]
    fn unknown_type_is_error() {
        let mut bytes = encode_frame(&LiveFrame::CaptureFinished);
        bytes[4] = 99;
        assert_eq!(decode_frame(&bytes), Err(ProtocolError::UnknownFrame(99)));
    }

    #[test]
    fn empty_batch_is_valid() {
        roundtrip(LiveFrame::EventBatch { events: vec![] });
    }

    /// Baseline for what LiveService::push_events pays per batch.
    /// Run with `cargo test --release -p orbit-live-protocol encode_bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn encode_bench() {
        let events: Vec<LiveEvent> = (0..10_000u64)
            .map(|i| LiveEvent {
                start_ns: i * 10,
                duration_ns: 5,
                tid: (i % 16) as u32,
                pid: 1,
                kind: 1,
                depth: (i % 8) as u8,
                extra: 0,
                _pad: 0,
                name_id: (i % 100) as u32,
            })
            .collect();
        let iters = 200;
        let t = std::time::Instant::now();
        let mut total = 0usize;
        for _ in 0..iters {
            // What push_events does today: clone the slice into the enum,
            // then encode.
            let bytes = encode_frame(&LiveFrame::EventBatch { events: events.to_vec() });
            total += bytes.len();
        }
        let per_batch_us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
        println!("ENCODE_BENCH batch=10000 events clone_then_encode_us={per_batch_us:.1} (checksum {total})");
        let t = std::time::Instant::now();
        let mut total2 = 0usize;
        for _ in 0..iters {
            total2 += encode_event_batch(&events).len();
        }
        let direct_us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
        println!("ENCODE_BENCH batch=10000 events encode_event_batch_us={direct_us:.1} (checksum {total2})");
    }

    #[test]
    fn encode_event_batch_matches_encode_frame() {
        let events: Vec<LiveEvent> = (0..37u64)
            .map(|i| LiveEvent {
                start_ns: i,
                duration_ns: 2,
                tid: 3,
                pid: 4,
                kind: 1,
                depth: 0,
                extra: 0,
                _pad: 0,
                name_id: 9,
            })
            .collect();
        assert_eq!(
            encode_event_batch(&events),
            encode_frame(&LiveFrame::EventBatch { events: events.clone() })
        );
        assert_eq!(encode_event_batch(&[]), encode_frame(&LiveFrame::EventBatch { events: vec![] }));
    }
}
