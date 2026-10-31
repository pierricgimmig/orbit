// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! ORBL: the live-bench recording format.
//!
//! A workstation runs `orbit-service live-bench record` and writes one of
//! these files. `replay` feeds it back through the same ingest path as the
//! synthetic 72-core stress, at 1x or Nx.
//!
//! Little-endian. Layout:
//!
//! ```text
//!  0  magic            b"ORBL"
//!  4  version          u16 = 1
//!  6  flags            u16   bit0 kernel switches, bit1 kernel samples
//!  8  cores            u32   logical cpus the recording saw
//! 12  threads          u32   workload threads
//! 16  sample_hz        u32
//! 20  reserved         u32
//! 24  t0_ns            u64   CLOCK_MONOTONIC of the first record's clock
//! 32  header pad       32 bytes (records begin at offset 64)
//! ```
//!
//! Each record is a `u8` tag followed by a fixed body:
//!
//! * tag 1 switch: `ts:u64 cpu:u32 is_out:u8 pad:[u8;3] pid:i32 tid:i32`
//! * tag 2 sample: `ts:u64 pid:u32 tid:u32 nframes:u8 pad:[u8;3] frames:[u64; nframes]`
//!   `nframes` is 0..=12. Frames are instruction pointers, leaf first.
//! * tag 3 scope: `ts:u64 dur:u64 pid:u32 tid:u32 depth:u8 pad:[u8;3] name:u32`

use std::io::{self, Read, Write};

pub const MAGIC: &[u8; 4] = b"ORBL";
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 64;
pub const MAX_FRAMES: usize = 12;

pub const FLAG_KERNEL_SWITCHES: u16 = 1;
pub const FLAG_KERNEL_SAMPLES: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub flags: u16,
    pub cores: u32,
    pub threads: u32,
    pub sample_hz: u32,
    pub t0_ns: u64,
}

impl Default for Header {
    fn default() -> Self {
        Self { flags: 0, cores: 1, threads: 1, sample_hz: 1000, t0_ns: 0 }
    }
}

/// One recorded event. Samples keep at most [`MAX_FRAMES`] instruction pointers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rec {
    Switch { ts: u64, cpu: u32, is_out: bool, pid: i32, tid: i32 },
    Sample { ts: u64, pid: u32, tid: u32, nframes: u8, frames: [u64; MAX_FRAMES] },
    Scope { ts: u64, dur: u64, pid: u32, tid: u32, depth: u8, name: u32 },
}

impl Rec {
    pub fn ts(self) -> u64 {
        match self {
            Rec::Switch { ts, .. } | Rec::Sample { ts, .. } | Rec::Scope { ts, .. } => ts,
        }
    }
}

pub struct Writer<W: Write> {
    inner: W,
    pub switches: u64,
    pub samples: u64,
    pub scopes: u64,
}

impl<W: Write> Writer<W> {
    pub fn create(mut inner: W, header: &Header) -> io::Result<Self> {
        let mut buf = [0u8; HEADER_LEN];
        buf[0..4].copy_from_slice(MAGIC);
        buf[4..6].copy_from_slice(&VERSION.to_le_bytes());
        buf[6..8].copy_from_slice(&header.flags.to_le_bytes());
        buf[8..12].copy_from_slice(&header.cores.to_le_bytes());
        buf[12..16].copy_from_slice(&header.threads.to_le_bytes());
        buf[16..20].copy_from_slice(&header.sample_hz.to_le_bytes());
        buf[24..32].copy_from_slice(&header.t0_ns.to_le_bytes());
        inner.write_all(&buf)?;
        Ok(Self { inner, switches: 0, samples: 0, scopes: 0 })
    }

    pub fn write_rec(&mut self, rec: &Rec) -> io::Result<()> {
        match *rec {
            Rec::Switch { ts, cpu, is_out, pid, tid } => {
                let mut b = [0u8; 1 + 8 + 4 + 1 + 3 + 4 + 4];
                b[0] = 1;
                b[1..9].copy_from_slice(&ts.to_le_bytes());
                b[9..13].copy_from_slice(&cpu.to_le_bytes());
                b[13] = u8::from(is_out);
                b[17..21].copy_from_slice(&pid.to_le_bytes());
                b[21..25].copy_from_slice(&tid.to_le_bytes());
                self.inner.write_all(&b)?;
                self.switches += 1;
            }
            Rec::Sample { ts, pid, tid, nframes, frames } => {
                let n = (nframes as usize).min(MAX_FRAMES);
                let mut b = Vec::with_capacity(1 + 8 + 4 + 4 + 1 + 3 + n * 8);
                b.push(2);
                b.extend_from_slice(&ts.to_le_bytes());
                b.extend_from_slice(&pid.to_le_bytes());
                b.extend_from_slice(&tid.to_le_bytes());
                b.push(n as u8);
                b.extend_from_slice(&[0u8; 3]);
                for frame in frames.iter().take(n) {
                    b.extend_from_slice(&frame.to_le_bytes());
                }
                self.inner.write_all(&b)?;
                self.samples += 1;
            }
            Rec::Scope { ts, dur, pid, tid, depth, name } => {
                let mut b = [0u8; 1 + 8 + 8 + 4 + 4 + 1 + 3 + 4];
                b[0] = 3;
                b[1..9].copy_from_slice(&ts.to_le_bytes());
                b[9..17].copy_from_slice(&dur.to_le_bytes());
                b[17..21].copy_from_slice(&pid.to_le_bytes());
                b[21..25].copy_from_slice(&tid.to_le_bytes());
                b[25] = depth;
                b[29..33].copy_from_slice(&name.to_le_bytes());
                self.inner.write_all(&b)?;
                self.scopes += 1;
            }
        }
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub struct Reader<R: Read> {
    inner: R,
    pub header: Header,
}

impl<R: Read> Reader<R> {
    pub fn open(mut inner: R) -> io::Result<Self> {
        let mut buf = [0u8; HEADER_LEN];
        inner.read_exact(&mut buf)?;
        if &buf[0..4] != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not an ORBL file"));
        }
        let version = u16::from_le_bytes(buf[4..6].try_into().unwrap());
        if version != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ORBL version {version} is not supported"),
            ));
        }
        let header = Header {
            flags: u16::from_le_bytes(buf[6..8].try_into().unwrap()),
            cores: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            threads: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
            sample_hz: u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            t0_ns: u64::from_le_bytes(buf[24..32].try_into().unwrap()),
        };
        Ok(Self { inner, header })
    }

    pub fn next_rec(&mut self) -> io::Result<Option<Rec>> {
        let mut tag = [0u8; 1];
        match self.inner.read(&mut tag) {
            Ok(0) => return Ok(None),
            Ok(_) => {}
            Err(e) => return Err(e),
        }
        match tag[0] {
            1 => {
                let mut b = [0u8; 24];
                self.inner.read_exact(&mut b)?;
                Ok(Some(Rec::Switch {
                    ts: u64::from_le_bytes(b[0..8].try_into().unwrap()),
                    cpu: u32::from_le_bytes(b[8..12].try_into().unwrap()),
                    is_out: b[12] != 0,
                    pid: i32::from_le_bytes(b[16..20].try_into().unwrap()),
                    tid: i32::from_le_bytes(b[20..24].try_into().unwrap()),
                }))
            }
            2 => {
                let mut head = [0u8; 8 + 4 + 4 + 1 + 3];
                self.inner.read_exact(&mut head)?;
                let n = (head[16] as usize).min(MAX_FRAMES);
                let mut frames = [0u64; MAX_FRAMES];
                for frame in frames.iter_mut().take(n) {
                    let mut fb = [0u8; 8];
                    self.inner.read_exact(&mut fb)?;
                    *frame = u64::from_le_bytes(fb);
                }
                Ok(Some(Rec::Sample {
                    ts: u64::from_le_bytes(head[0..8].try_into().unwrap()),
                    pid: u32::from_le_bytes(head[8..12].try_into().unwrap()),
                    tid: u32::from_le_bytes(head[12..16].try_into().unwrap()),
                    nframes: n as u8,
                    frames,
                }))
            }
            3 => {
                let mut b = [0u8; 32];
                self.inner.read_exact(&mut b)?;
                Ok(Some(Rec::Scope {
                    ts: u64::from_le_bytes(b[0..8].try_into().unwrap()),
                    dur: u64::from_le_bytes(b[8..16].try_into().unwrap()),
                    pid: u32::from_le_bytes(b[16..20].try_into().unwrap()),
                    tid: u32::from_le_bytes(b[20..24].try_into().unwrap()),
                    depth: b[24],
                    name: u32::from_le_bytes(b[28..32].try_into().unwrap()),
                }))
            }
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown ORBL tag {other}"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_header_and_three_record_kinds() {
        let header = Header { flags: FLAG_KERNEL_SWITCHES | FLAG_KERNEL_SAMPLES, cores: 72, threads: 576, sample_hz: 1000, t0_ns: 50 };
        let mut bytes = Vec::new();
        {
            let mut w = Writer::create(&mut bytes, &header).unwrap();
            w.write_rec(&Rec::Switch { ts: 10, cpu: 3, is_out: false, pid: 9, tid: 8 }).unwrap();
            let mut frames = [0u64; MAX_FRAMES];
            frames[0] = 0x1000;
            frames[1] = 0x2000;
            w.write_rec(&Rec::Sample { ts: 11, pid: 9, tid: 8, nframes: 2, frames }).unwrap();
            w.write_rec(&Rec::Scope { ts: 12, dur: 40, pid: 9, tid: 8, depth: 1, name: 7 }).unwrap();
        }
        let mut r = Reader::open(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(r.header, header);
        assert_eq!(r.next_rec().unwrap(), Some(Rec::Switch { ts: 10, cpu: 3, is_out: false, pid: 9, tid: 8 }));
        match r.next_rec().unwrap() {
            Some(Rec::Sample { nframes, frames, ts, .. }) => {
                assert_eq!(ts, 11);
                assert_eq!(nframes, 2);
                assert_eq!(frames[0], 0x1000);
                assert_eq!(frames[1], 0x2000);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(r.next_rec().unwrap(), Some(Rec::Scope { name: 7, dur: 40, .. })));
        assert_eq!(r.next_rec().unwrap(), None);
    }
}
