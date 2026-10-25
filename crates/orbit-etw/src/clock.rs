// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! QueryPerformanceCounter ticks to nanoseconds.
//!
//! The session is opened with `PROCESS_TRACE_MODE_RAW_TIMESTAMP`, so every
//! event carries the raw counter the kernel read, in the same units
//! `QueryPerformanceCounter` returns to the instrumented process. Both sides
//! convert with the same arithmetic -- `ticks * 1e9 / frequency` in 128-bit
//! -- so a manual scope and the context switch that interrupted it land on
//! one axis to the nanosecond. The C++ tracer multiplied by a truncated
//! integer period instead, which is exact on a 10 MHz counter and drifts on
//! any other; this does not.

/// One counter frequency, applied to raw timestamps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QpcClock {
    frequency: u64,
}

impl QpcClock {
    /// A clock at `frequency` ticks per second (zero is treated as one).
    pub fn with_frequency(frequency: u64) -> QpcClock {
        QpcClock { frequency: frequency.max(1) }
    }

    /// The host's counter frequency, the one `orbit-scope-ring` uses.
    #[cfg(windows)]
    pub fn host() -> QpcClock {
        #[link(name = "kernel32")]
        extern "system" {
            fn QueryPerformanceFrequency(freq: *mut i64) -> i32;
        }
        let mut f = 0i64;
        // SAFETY: writes one i64.
        unsafe { QueryPerformanceFrequency(&mut f) };
        QpcClock::with_frequency(f.max(1) as u64)
    }

    pub fn frequency(&self) -> u64 {
        self.frequency
    }

    /// Raw ticks to nanoseconds, exactly as `orbit_scope_ring::platform::monotonic_ns`.
    #[inline]
    pub fn to_ns(&self, ticks: i64) -> u64 {
        ((ticks.max(0) as u128 * 1_000_000_000) / self.frequency as u128) as u64
    }

    /// Nanoseconds to raw ticks, rounding down; for intervals handed to the
    /// kernel.
    pub fn to_ticks(&self, ns: u64) -> u64 {
        ((ns as u128 * self.frequency as u128) / 1_000_000_000) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ten_megahertz_counter_is_a_hundred_nanoseconds_a_tick() {
        let clock = QpcClock::with_frequency(10_000_000);
        assert_eq!(clock.to_ns(1), 100);
        assert_eq!(clock.to_ns(12_345_678), 1_234_567_800);
        assert_eq!(clock.to_ticks(1_000_000), 10_000);
    }

    #[test]
    fn an_awkward_frequency_does_not_drift() {
        // The truncated-period arithmetic the C++ used gives 279 ns a tick
        // here; the real period is 279.365 ns, which is 0.13 % off -- a
        // second of capture lands 1.3 ms away from the scope clock.
        let clock = QpcClock::with_frequency(3_579_545);
        assert_eq!(clock.to_ns(3_579_545), 1_000_000_000);
        assert_eq!(clock.to_ns(3_579_545 * 60), 60_000_000_000);
    }

    #[test]
    fn large_counters_do_not_overflow() {
        let clock = QpcClock::with_frequency(10_000_000);
        // Four hundred years of uptime at 10 MHz.
        let ticks = 10_000_000i64 * 86_400 * 365 * 400;
        assert_eq!(clock.to_ns(ticks), ticks as u64 * 100);
        assert_eq!(clock.to_ns(-5), 0, "a negative counter is clamped, never wrapped");
    }

    #[test]
    fn zero_frequency_is_not_a_division_by_zero() {
        assert_eq!(QpcClock::with_frequency(0).frequency(), 1);
    }
}
