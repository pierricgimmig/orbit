// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt;

use serde::de::{self, Deserializer, Visitor};
use serde::Deserialize;

/// Chrome `pid` / `tid` / `id` — number or string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlexId {
    Num(u64),
    Str(String),
}

impl FlexId {
    pub fn as_u32(&self) -> u32 {
        match self {
            FlexId::Num(n) => *n as u32,
            FlexId::Str(s) => parse_u64(s) as u32,
        }
    }

    pub fn as_u64(&self) -> u64 {
        match self {
            FlexId::Num(n) => *n,
            FlexId::Str(s) => parse_u64(s),
        }
    }

    pub fn as_str_cow(&self) -> std::borrow::Cow<'_, str> {
        match self {
            FlexId::Num(n) => std::borrow::Cow::Owned(n.to_string()),
            FlexId::Str(s) => std::borrow::Cow::Borrowed(s),
        }
    }
}

impl<'de> Deserialize<'de> for FlexId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = FlexId;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("string or number id")
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<FlexId, E> {
                Ok(FlexId::Num(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<FlexId, E> {
                Ok(FlexId::Num(v as u64))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<FlexId, E> {
                Ok(FlexId::Num(v as u64))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<FlexId, E> {
                Ok(FlexId::Str(v.to_string()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<FlexId, E> {
                Ok(FlexId::Str(v))
            }
        }
        deserializer.deserialize_any(V)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Id2 {
    #[serde(default)]
    pub local: Option<FlexId>,
    #[serde(default)]
    pub global: Option<FlexId>,
}

pub fn parse_u64(s: &str) -> u64 {
    let t = s.trim();
    if let Some(hex) = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
    {
        return u64::from_str_radix(hex, 16).unwrap_or_else(|_| hash64(t.as_bytes()));
    }
    t.parse::<u64>()
        .unwrap_or_else(|_| hash64(t.as_bytes()))
}

pub fn hash64(bytes: &[u8]) -> u64 {
    // FNV-1a 64. Stable across runs so string pids stay put.
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

pub fn hash32(bytes: &[u8]) -> u32 {
    hash64(bytes) as u32
}
