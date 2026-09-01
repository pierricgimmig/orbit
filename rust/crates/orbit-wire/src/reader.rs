// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Walking pod-encoded events out of a byte slice. The reader is a cursor
//! that yields one `Event` at a time until the buffer is exhausted; a
//! truncated or malformed record is an error, never an out-of-bounds read.

use crate::{CallstackType, Event, EventTag};

#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    /// The buffer ended in the middle of a record.
    Truncated,
    /// The tag byte was not a known event.
    UnknownTag(u8),
    /// A callstack type byte was out of range.
    BadCallstackType(u8),
}

/// A forward cursor over a pod-encoded event stream.
pub struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, offset: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.offset >= self.bytes.len()
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], ReadError> {
        let end = self.offset.checked_add(count).ok_or(ReadError::Truncated)?;
        let slice = self.bytes.get(self.offset..end).ok_or(ReadError::Truncated)?;
        self.offset = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, ReadError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, ReadError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }
    fn i32(&mut self) -> Result<i32, ReadError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }
    fn u64(&mut self) -> Result<u64, ReadError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")))
    }
    fn u64_vec(&mut self) -> Result<Vec<u64>, ReadError> {
        let count = self.u32()? as usize;
        let bytes = self.take(count.checked_mul(8).ok_or(ReadError::Truncated)?)?;
        Ok(bytes.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().expect("8 bytes"))).collect())
    }

    /// Reads the next event, or None at the clean end of the stream.
    pub fn next_event(&mut self) -> Result<Option<Event>, ReadError> {
        if self.is_empty() {
            return Ok(None);
        }
        let tag = self.u8()?;
        let tag = EventTag::from_u8(tag).ok_or(ReadError::UnknownTag(tag))?;
        let event = match tag {
            EventTag::SchedulingSlice => Event::SchedulingSlice {
                pid: self.u32()?,
                tid: self.u32()?,
                core: self.i32()?,
                duration_ns: self.u64()?,
                out_timestamp_ns: self.u64()?,
            },
            EventTag::CallstackSample => Event::CallstackSample {
                pid: self.u32()?,
                tid: self.u32()?,
                callstack_id: self.u64()?,
                timestamp_ns: self.u64()?,
            },
            EventTag::FunctionCall => Event::FunctionCall {
                pid: self.u32()?,
                tid: self.u32()?,
                function_id: self.u64()?,
                duration_ns: self.u64()?,
                end_timestamp_ns: self.u64()?,
                depth: self.i32()?,
                return_value: self.u64()?,
                registers: self.u64_vec()?,
            },
            EventTag::InternedCallstack => {
                let key = self.u64()?;
                let type_byte = self.u8()?;
                let callstack_type =
                    CallstackType::from_u8(type_byte).ok_or(ReadError::BadCallstackType(type_byte))?;
                Event::InternedCallstack { key, callstack_type, pcs: self.u64_vec()? }
            }
            EventTag::InternedString => {
                let key = self.u64()?;
                let len = self.u32()? as usize;
                Event::InternedString { key, bytes: self.take(len)?.to_vec() }
            }
            EventTag::GpuJob => Event::GpuJob {
                pid: self.u32()?,
                tid: self.u32()?,
                context: self.u32()?,
                seqno: self.u32()?,
                depth: self.i32()?,
                amdgpu_cs_ioctl_time_ns: self.u64()?,
                amdgpu_sched_run_job_time_ns: self.u64()?,
                gpu_hardware_start_time_ns: self.u64()?,
                dma_fence_signaled_time_ns: self.u64()?,
                timeline: {
                    let len = self.u32()? as usize;
                    self.take(len)?.to_vec()
                },
            },
        };
        Ok(Some(event))
    }
}

impl Iterator for Reader<'_> {
    type Item = Result<Event, ReadError>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_event().transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Writer;

    fn sample_events() -> Vec<Event> {
        vec![
            Event::SchedulingSlice { pid: 100, tid: 101, core: 3, duration_ns: 5000, out_timestamp_ns: 90000 },
            Event::CallstackSample { pid: 100, tid: 102, callstack_id: 0xABCD, timestamp_ns: 91000 },
            Event::FunctionCall {
                pid: 100, tid: 103, function_id: 42, duration_ns: 250, end_timestamp_ns: 92000,
                depth: 2, return_value: 7, registers: vec![1, 2, 3, 4, 5, 6],
            },
            Event::InternedCallstack {
                key: 0x1234, callstack_type: CallstackType::Complete, pcs: vec![0xdead, 0xbeef, 0xcafe],
            },
            Event::InternedString { key: 0x5678, bytes: b"orbit::Frame::draw".to_vec() },
            Event::GpuJob {
                pid: 100, tid: 104, context: 7, seqno: 900, depth: 2,
                amdgpu_cs_ioctl_time_ns: 1000, amdgpu_sched_run_job_time_ns: 2000,
                gpu_hardware_start_time_ns: 2000, dma_fence_signaled_time_ns: 8000,
                timeline: b"gfx".to_vec(),
            },
        ]
    }

    #[test]
    fn round_trips_every_event_type() {
        let events = sample_events();
        let mut writer = Writer::new();
        for event in &events {
            writer.write(event);
        }
        let bytes = writer.into_bytes();
        let decoded: Result<Vec<Event>, _> = Reader::new(&bytes).collect();
        assert_eq!(decoded.unwrap(), events);
    }

    #[test]
    fn empty_arrays_round_trip() {
        let events = vec![
            Event::FunctionCall {
                pid: 1, tid: 2, function_id: 3, duration_ns: 4, end_timestamp_ns: 5,
                depth: 0, return_value: 0, registers: vec![],
            },
            Event::InternedCallstack { key: 9, callstack_type: CallstackType::InUprobes, pcs: vec![] },
            Event::InternedString { key: 10, bytes: vec![] },
        ];
        let mut writer = Writer::new();
        for event in &events {
            writer.write(event);
        }
        let bytes = writer.into_bytes();
        let decoded: Result<Vec<Event>, _> = Reader::new(&bytes).collect();
        assert_eq!(decoded.unwrap(), events);
    }

    #[test]
    fn truncation_is_an_error_not_a_panic() {
        let mut writer = Writer::new();
        writer.write(&sample_events()[2]); // a FunctionCall with registers
        let bytes = writer.into_bytes();
        for cut in 1..bytes.len() {
            let result: Result<Vec<Event>, _> = Reader::new(&bytes[..cut]).collect();
            assert!(result.is_err(), "cut at {cut} should error");
        }
    }

    #[test]
    fn unknown_tag_is_an_error() {
        let bytes = [0xFFu8, 0, 0, 0, 0];
        let result: Result<Vec<Event>, _> = Reader::new(&bytes).collect();
        assert_eq!(result, Err(ReadError::UnknownTag(0xFF)));
    }
}
