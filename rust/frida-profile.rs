// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Setup-only telemetry. Use the same host clock as Orbit scopes, without
// initializing a second SDK inside the target or tracing hot callbacks.
use std::io::Write;
fn now_ns() -> u64 {
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
    }
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}
pub fn measure<T>(
    output: &mut impl Write,
    enabled: bool,
    name: &str,
    work: impl FnOnce() -> T,
) -> T {
    if !enabled {
        return work();
    }
    let start = now_ns();
    let result = work();
    let end = now_ns();
    // Profiling must not change the outcome of injection or cleanup.
    let _ = writeln!(
        output,
        "{}",
        serde_json::json!({"profile": {
            "name": name, "start_ns": start, "end_ns": end
        }})
    );
    let _ = output.flush();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_preserves_result_and_monotonic_interval() {
        let mut bytes = Vec::new();
        let before = now_ns();
        let result = measure(&mut bytes, true, "Frida: test", || Err::<(), _>(42));
        let after = now_ns();
        assert_eq!(result, Err(42));
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let p = &value["profile"];
        assert_eq!(p["name"], "Frida: test");
        let start = p["start_ns"].as_u64().unwrap();
        let end = p["end_ns"].as_u64().unwrap();
        assert!(before <= start && start <= end && end <= after);
    }

    #[test]
    fn disabled_or_disconnected_telemetry_does_not_change_work() {
        let mut bytes = Vec::new();
        assert_eq!(measure(&mut bytes, false, "Frida: test", || 42), 42);
        assert!(bytes.is_empty());
        struct Disconnected;
        impl Write for Disconnected {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(measure(&mut Disconnected, true, "Frida: test", || 42), 42);
    }
}
