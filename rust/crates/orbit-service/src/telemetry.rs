// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! GPU telemetry from a helper process.
//!
//! The service ships as a fully static musl binary, and static musl does not
//! support `dlopen` (it returns "Dynamic loading not supported"), so the
//! service itself cannot load `libnvidia-ml` or `libcupti`. That does NOT
//! mean a static binary cannot use them: `fork`/`exec` and pipes work fine.
//!
//! So NVIDIA support is a small helper process -- dynamically linked, links
//! NVML/CUPTI, no constraints -- that writes pod events to its stdout, and
//! this module drains that stream into the capture. The helper speaks the
//! same pod format as everything else, so it needs no bespoke protocol, and
//! the boundary is a pipe rather than a symbol table: the static service
//! keeps its zero runtime dependencies while still getting vendor telemetry.
//!
//! A helper is any program that writes a pod event stream to stdout, which
//! also makes the path testable without an NVIDIA GPU.

use orbit_wire::{Event, Reader};
use std::io::Read;
use std::process::{Child, Command, Stdio};

pub struct TelemetryHelper {
    child: Child,
    /// Bytes read but not yet a whole event.
    pending: Vec<u8>,
    events: u64,
    decode_errors: u64,
}

impl TelemetryHelper {
    /// Spawns `path` with `args`, reading its stdout as a pod event stream.
    pub fn spawn(path: &str, args: &[String]) -> std::io::Result<TelemetryHelper> {
        let child = Command::new(path)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        Ok(TelemetryHelper { child, pending: Vec::new(), events: 0, decode_errors: 0 })
    }

    /// Reads whatever the helper has produced and returns the whole events in
    /// it, keeping any partial trailing record for next time. Non-blocking in
    /// effect: it only consumes what is already buffered in the pipe.
    pub fn drain(&mut self) -> Vec<Event> {
        let Some(stdout) = self.child.stdout.as_mut() else { return Vec::new() };
        let mut chunk = [0u8; 16 * 1024];
        match stdout.read(&mut chunk) {
            Ok(0) => {}
            Ok(count) => self.pending.extend_from_slice(&chunk[..count]),
            Err(_) => return Vec::new(),
        }

        let mut events = Vec::new();
        let mut reader = Reader::new(&self.pending);
        let mut consumed = 0usize;
        loop {
            match reader.next_event() {
                Ok(Some(event)) => {
                    consumed = reader.consumed();
                    events.push(event);
                }
                // A truncated tail is the normal case mid-stream: stop and
                // keep it for the next drain.
                Ok(None) | Err(_) => break,
            }
        }
        // A malformed (not merely truncated) record would stall the stream
        // forever; count it and drop the buffer rather than wedging.
        if consumed == 0 && self.pending.len() > MAX_PENDING_BYTES {
            self.decode_errors += 1;
            self.pending.clear();
        } else {
            self.pending.drain(..consumed);
        }
        self.events += events.len() as u64;
        events
    }

    pub fn events_received(&self) -> u64 {
        self.events
    }

    pub fn decode_errors(&self) -> u64 {
        self.decode_errors
    }

    /// Stops the helper and drains whatever it wrote before exiting.
    pub fn shutdown(mut self) -> Vec<Event> {
        let _ = self.child.kill();
        let mut remaining = Vec::new();
        if let Some(stdout) = self.child.stdout.as_mut() {
            let mut rest = Vec::new();
            let _ = stdout.read_to_end(&mut rest);
            self.pending.extend_from_slice(&rest);
        }
        let mut reader = Reader::new(&self.pending);
        while let Ok(Some(event)) = reader.next_event() {
            remaining.push(event);
        }
        self.events += remaining.len() as u64;
        let _ = self.child.wait();
        remaining
    }
}

/// A record larger than this is malformed, not merely incomplete: the biggest
/// pod event is an interned callstack, bounded by the unwinder's frame cap.
const MAX_PENDING_BYTES: usize = 1 << 20;

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_wire::Writer;

    fn metrics_event(timestamp_ns: u64, utilization: u32) -> Event {
        Event::GpuMetrics {
            timestamp_ns,
            device_index: 0,
            gpu_utilization_percent: utilization,
            memory_utilization_percent: 10,
            memory_used_bytes: 1 << 30,
            memory_total_bytes: 24 << 30,
            process_memory_used_bytes: 1 << 29,
            temperature_celsius: 65,
            power_milliwatts: 180_000,
            sm_clock_mhz: 2100,
            memory_clock_mhz: 10501,
        }
    }

    /// A stand-in for the real NVML helper: writes a pod stream to stdout,
    /// exactly as the dynamically-linked helper would. This is what proves a
    /// STATIC binary can consume vendor telemetry without dlopen.
    fn fake_helper_writing(events: &[Event]) -> (String, Vec<String>) {
        let mut writer = Writer::new();
        for event in events {
            writer.write(event);
        }
        let bytes = writer.into_bytes();
        let path = std::env::temp_dir().join(format!(
            "orbit-fake-gpu-helper-{}-{}.bin",
            std::process::id(),
            events.len()
        ));
        std::fs::write(&path, &bytes).unwrap();
        // `cat` is the helper: it emits the pod stream on stdout.
        ("/bin/cat".to_string(), vec![path.to_string_lossy().into_owned()])
    }

    #[test]
    fn drains_pod_events_from_a_helper_process() {
        let expected = vec![metrics_event(1000, 50), metrics_event(2000, 75)];
        let (path, args) = fake_helper_writing(&expected);
        let mut helper = TelemetryHelper::spawn(&path, &args).unwrap();
        // Give the child a moment to write, then drain until we have both.
        let mut received = Vec::new();
        for _ in 0..200 {
            received.extend(helper.drain());
            if received.len() >= expected.len() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        received.extend(helper.shutdown());
        assert_eq!(received.len(), expected.len(), "got {received:?}");
        assert_eq!(received, expected);
        let _ = std::fs::remove_file(&args[0]);
    }

    #[test]
    fn a_partial_record_is_held_until_it_completes() {
        // Encode one event, then feed it in two halves through the same
        // decode path the drain uses.
        let mut writer = Writer::new();
        writer.write(&metrics_event(4242, 99));
        let bytes = writer.into_bytes();
        let split = bytes.len() / 2;

        let mut pending: Vec<u8> = bytes[..split].to_vec();
        let mut reader = Reader::new(&pending);
        assert!(matches!(reader.next_event(), Err(_)), "half a record must not decode");

        pending.extend_from_slice(&bytes[split..]);
        let mut reader = Reader::new(&pending);
        let event = reader.next_event().unwrap().unwrap();
        assert_eq!(event, metrics_event(4242, 99));
        assert_eq!(reader.consumed(), bytes.len());
    }

    #[test]
    fn a_missing_helper_is_an_error_not_a_panic() {
        let result = TelemetryHelper::spawn("/nonexistent/orbit-gpu-helper", &[]);
        assert!(result.is_err());
    }
}
