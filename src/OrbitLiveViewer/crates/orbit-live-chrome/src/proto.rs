// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Minimal protobuf wire walker. No schema, no codegen, no allocation: a
//! message is a byte slice and [`fields`] yields its `(field number, value)`
//! pairs in order. Nested messages are walked by calling [`fields`] on the
//! `Bytes` value again. Unknown fields cost nothing; a malformed tail ends
//! the iteration.

/// One decoded field value. Numeric semantics (signed, zigzag, float) are
/// the caller's to interpret from the schema it knows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Val<'a> {
    Varint(u64),
    Fixed64(u64),
    Fixed32(u32),
    Bytes(&'a [u8]),
}

impl<'a> Val<'a> {
    pub fn u64(self) -> u64 {
        match self {
            Val::Varint(v) | Val::Fixed64(v) => v,
            Val::Fixed32(v) => v as u64,
            Val::Bytes(_) => 0,
        }
    }

    pub fn i64(self) -> i64 {
        self.u64() as i64
    }

    pub fn i32(self) -> i32 {
        self.u64() as i32
    }

    pub fn u32(self) -> u32 {
        self.u64() as u32
    }

    pub fn bool(self) -> bool {
        self.u64() != 0
    }

    pub fn f64(self) -> f64 {
        match self {
            Val::Fixed64(v) => f64::from_bits(v),
            Val::Fixed32(v) => f32::from_bits(v) as f64,
            Val::Varint(v) => v as f64,
            Val::Bytes(_) => 0.0,
        }
    }

    pub fn bytes(self) -> &'a [u8] {
        match self {
            Val::Bytes(b) => b,
            _ => &[],
        }
    }

    /// UTF-8 string; invalid input is replaced lossily rather than dropped.
    pub fn string(self) -> String {
        String::from_utf8_lossy(self.bytes()).into_owned()
    }

    /// A `repeated` scalar that was written packed (one length-delimited
    /// blob of varints) or unpacked (this single value): both shapes are
    /// legal for the same field, so callers ask for the values either way.
    pub fn varints(self) -> Varints<'a> {
        match self {
            Val::Bytes(b) => Varints::Packed { buf: b, pos: 0 },
            other => Varints::One(Some(other.u64())),
        }
    }

    /// Same for `repeated fixed64`.
    pub fn fixed64s(self) -> Vec<u64> {
        match self {
            Val::Bytes(b) => b
                .chunks_exact(8)
                .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
                .collect(),
            other => vec![other.u64()],
        }
    }
}

pub enum Varints<'a> {
    Packed { buf: &'a [u8], pos: usize },
    One(Option<u64>),
}

impl Iterator for Varints<'_> {
    type Item = u64;
    fn next(&mut self) -> Option<u64> {
        match self {
            Varints::One(v) => v.take(),
            Varints::Packed { buf, pos } => {
                let (v, n) = varint(buf, *pos)?;
                *pos += n;
                Some(v)
            }
        }
    }
}

/// Decode a varint at `pos`. Returns the value and the bytes consumed, or
/// `None` when the buffer ends first (or the varint is longer than 10 bytes).
pub fn varint(buf: &[u8], pos: usize) -> Option<(u64, usize)> {
    let mut v = 0u64;
    let mut shift = 0u32;
    let mut i = pos;
    while i < buf.len() && shift < 64 {
        let b = buf[i];
        v |= u64::from(b & 0x7f) << shift;
        i += 1;
        if b & 0x80 == 0 {
            return Some((v, i - pos));
        }
        shift += 7;
    }
    None
}

pub struct Fields<'a> {
    buf: &'a [u8],
    pos: usize,
}

/// Walk a message's fields in wire order.
pub fn fields(buf: &[u8]) -> Fields<'_> {
    Fields { buf, pos: 0 }
}

impl<'a> Iterator for Fields<'a> {
    type Item = (u32, Val<'a>);

    fn next(&mut self) -> Option<(u32, Val<'a>)> {
        if self.pos >= self.buf.len() {
            return None;
        }
        let (tag, n) = varint(self.buf, self.pos)?;
        let mut pos = self.pos + n;
        let num = (tag >> 3) as u32;
        let wire = (tag & 7) as u8;
        if num == 0 {
            self.pos = self.buf.len();
            return None;
        }
        let val = match wire {
            0 => {
                let (v, n) = varint(self.buf, pos)?;
                pos += n;
                Val::Varint(v)
            }
            1 => {
                let end = pos.checked_add(8)?;
                let b = self.buf.get(pos..end)?;
                pos = end;
                Val::Fixed64(u64::from_le_bytes(b.try_into().ok()?))
            }
            5 => {
                let end = pos.checked_add(4)?;
                let b = self.buf.get(pos..end)?;
                pos = end;
                Val::Fixed32(u32::from_le_bytes(b.try_into().ok()?))
            }
            2 => {
                let (len, n) = varint(self.buf, pos)?;
                pos += n;
                let end = pos.checked_add(usize::try_from(len).ok()?)?;
                let b = self.buf.get(pos..end)?;
                pos = end;
                Val::Bytes(b)
            }
            // Groups (3/4) are not used by Perfetto; a stray one ends the walk.
            _ => {
                self.pos = self.buf.len();
                return None;
            }
        };
        self.pos = pos;
        Some((num, val))
    }
}

/// Tiny encoder for tests and fixtures: the inverse of [`fields`].
#[derive(Default, Clone)]
pub struct Encoder {
    pub buf: Vec<u8>,
}

impl Encoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn varint(&mut self, num: u32, v: u64) -> &mut Self {
        self.tag(num, 0);
        self.raw_varint(v);
        self
    }

    pub fn i32(&mut self, num: u32, v: i32) -> &mut Self {
        self.varint(num, v as i64 as u64)
    }

    pub fn i64(&mut self, num: u32, v: i64) -> &mut Self {
        self.varint(num, v as u64)
    }

    pub fn fixed64(&mut self, num: u32, v: u64) -> &mut Self {
        self.tag(num, 1);
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn double(&mut self, num: u32, v: f64) -> &mut Self {
        self.fixed64(num, v.to_bits())
    }

    pub fn bytes(&mut self, num: u32, b: &[u8]) -> &mut Self {
        self.tag(num, 2);
        self.raw_varint(b.len() as u64);
        self.buf.extend_from_slice(b);
        self
    }

    pub fn string(&mut self, num: u32, s: &str) -> &mut Self {
        self.bytes(num, s.as_bytes())
    }

    pub fn msg(&mut self, num: u32, m: &Encoder) -> &mut Self {
        self.bytes(num, &m.buf)
    }

    pub fn packed(&mut self, num: u32, vals: &[u64]) -> &mut Self {
        let mut inner = Encoder::new();
        for &v in vals {
            inner.raw_varint(v);
        }
        self.bytes(num, &inner.buf)
    }

    fn tag(&mut self, num: u32, wire: u8) {
        self.raw_varint((u64::from(num) << 3) | u64::from(wire));
    }

    fn raw_varint(&mut self, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(b);
                return;
            }
            self.buf.push(b | 0x80);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_scalars_and_nested() {
        let mut inner = Encoder::new();
        inner.varint(1, 7).string(2, "hi");
        let mut m = Encoder::new();
        m.varint(8, 300)
            .fixed64(47, 0xdead_beef)
            .double(44, 2.5)
            .msg(11, &inner)
            .packed(3, &[1, 128, 70000])
            .i32(18, -5);
        let got: Vec<_> = fields(&m.buf).collect();
        assert_eq!(got[0], (8, Val::Varint(300)));
        assert_eq!(got[1], (47, Val::Fixed64(0xdead_beef)));
        assert_eq!(got[2].1.f64(), 2.5);
        let nested: Vec<_> = fields(got[3].1.bytes()).collect();
        assert_eq!(nested[0], (1, Val::Varint(7)));
        assert_eq!(nested[1].1.string(), "hi");
        assert_eq!(got[4].1.varints().collect::<Vec<_>>(), vec![1, 128, 70000]);
        assert_eq!(got[5].1.i32(), -5);
        assert_eq!(got.len(), 6);
    }

    #[test]
    fn truncated_message_ends_walk() {
        let mut m = Encoder::new();
        m.varint(1, 1).string(2, "abcdef");
        let cut = &m.buf[..m.buf.len() - 2];
        let got: Vec<_> = fields(cut).collect();
        assert_eq!(got.len(), 1);
        assert_eq!(varint(&[0x80, 0x80], 0), None);
        assert_eq!(varint(&[0xac, 0x02], 0), Some((300, 2)));
    }

    #[test]
    fn unpacked_repeated_reads_as_one() {
        assert_eq!(Val::Varint(9).varints().collect::<Vec<_>>(), vec![9]);
        assert_eq!(Val::Fixed64(9).fixed64s(), vec![9]);
        let two = [1u8, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(Val::Bytes(&two).fixed64s(), vec![1, 2]);
    }
}
