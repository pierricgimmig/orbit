// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Incremental JSON helpers. Scan complete values without allocating a tree.

pub fn skip_ws(buf: &[u8], mut i: usize) -> usize {
    while i < buf.len() && buf[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// End index (exclusive) of the JSON value starting at `start`, or `None` if
/// the buffer does not yet hold a complete value.
pub fn value_end(buf: &[u8], start: usize) -> Option<usize> {
    let i = skip_ws(buf, start);
    if i >= buf.len() {
        return None;
    }
    match buf[i] {
        b'{' | b'[' => container_end(buf, i),
        b'"' => string_end(buf, i),
        b't' => keyword_end(buf, i, b"true"),
        b'f' => keyword_end(buf, i, b"false"),
        b'n' => keyword_end(buf, i, b"null"),
        b'-' | b'0'..=b'9' => number_end(buf, i),
        _ => None,
    }
}

fn keyword_end(buf: &[u8], start: usize, kw: &[u8]) -> Option<usize> {
    if start + kw.len() > buf.len() {
        return None;
    }
    if &buf[start..start + kw.len()] == kw {
        Some(start + kw.len())
    } else {
        None
    }
}

fn number_end(buf: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    if buf[i] == b'-' {
        i += 1;
        if i >= buf.len() {
            return None;
        }
    }
    let mut saw_digit = false;
    while i < buf.len() && buf[i].is_ascii_digit() {
        saw_digit = true;
        i += 1;
    }
    if !saw_digit {
        return None;
    }
    if i < buf.len() && buf[i] == b'.' {
        i += 1;
        let mut frac = false;
        while i < buf.len() && buf[i].is_ascii_digit() {
            frac = true;
            i += 1;
        }
        if !frac {
            return None;
        }
    }
    if i < buf.len() && (buf[i] == b'e' || buf[i] == b'E') {
        i += 1;
        if i < buf.len() && (buf[i] == b'+' || buf[i] == b'-') {
            i += 1;
        }
        let mut exp = false;
        while i < buf.len() && buf[i].is_ascii_digit() {
            exp = true;
            i += 1;
        }
        if !exp {
            return None;
        }
    }
    Some(i)
}

fn string_end(buf: &[u8], start: usize) -> Option<usize> {
    debug_assert_eq!(buf[start], b'"');
    let mut i = start + 1;
    let mut escape = false;
    while i < buf.len() {
        let c = buf[i];
        if escape {
            escape = false;
        } else if c == b'\\' {
            escape = true;
        } else if c == b'"' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn container_end(buf: &[u8], start: usize) -> Option<usize> {
    let open = buf[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let mut depth = 1u32;
    let mut i = start + 1;
    let mut in_str = false;
    let mut escape = false;
    while i < buf.len() {
        let c = buf[i];
        if in_str {
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                if c == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                } else if (c == b'}' && open == b'[') || (c == b']' && open == b'{') {
                    // mismatched closer — still count down if we opened the other
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Parse a JSON string token (`"..."`) into an owned UTF-8 string.
pub fn parse_json_string(token: &[u8]) -> Option<String> {
    serde_json::from_slice::<String>(token).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_object_and_array() {
        let buf = br#"  {"a":[1,2],"b":"x\"y"}  extra"#;
        let end = value_end(buf, 0).unwrap();
        assert_eq!(&buf[..end], br#"  {"a":[1,2],"b":"x\"y"}"#);
        assert!(value_end(br#"{"a":1"#, 0).is_none());
    }

    #[test]
    fn scans_number_and_kw() {
        assert_eq!(value_end(b"12.5e-2,", 0), Some(7));
        assert_eq!(value_end(b"true,", 0), Some(4));
        assert_eq!(value_end(b"null", 0), Some(4));
    }
}
