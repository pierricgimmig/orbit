// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The controller: starts and stops the NT Kernel Logger.
//!
//! One session per machine, by name. A stale one left by a crashed
//! controller is stopped before starting; a second service starting while
//! the first captures gets a clear refusal rather than silently stealing
//! the session. Flags select what the kernel logs: thread and process
//! lifecycle always (the tid-to-pid map depends on them), context switches
//! and ready-thread events for the scheduler, image loads for the module
//! list, and the sampled profiler with stack walks when a rate is asked for.
//!
//! The sampling interval is a *machine* setting (`TraceSetInformation` with
//! a null session handle) and survives the session: it is set to the
//! requested rate on start and put back to the 1 ms default on stop, as
//! `xperf` does.

use crate::clock::QpcClock;
use crate::events::{PERF_INFO_GUID, SYSTEM_TRACE_CONTROL_GUID};
use crate::ffi::*;

/// What to enable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionOptions {
    /// Profiler ticks per second per core; 0 disables sampling.
    pub sampling_hz: u64,
    /// Context switches and ready-thread events.
    pub scheduler: bool,
    /// Image load and unload events (with a rundown of what is loaded).
    pub images: bool,
    /// Per-buffer size in KiB and the buffer count range. Real-time
    /// buffers are non-paged kernel memory.
    pub buffer_kb: u32,
    pub min_buffers: u32,
    pub max_buffers: u32,
}

impl Default for SessionOptions {
    fn default() -> Self {
        SessionOptions {
            sampling_hz: 1000,
            scheduler: true,
            images: true,
            // 256 KiB buffers: a 12-core box at 1 kHz with stacks and
            // tens of thousands of switches a second writes a few MB/s;
            // 64 buffers is seconds of slack, 256 caps it at 64 MiB.
            buffer_kb: 256,
            min_buffers: 64,
            max_buffers: 256,
        }
    }
}

/// What the kernel reports about a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionStats {
    pub buffers: u32,
    pub free_buffers: u32,
    pub events_lost: u32,
    pub buffers_written: u32,
    pub real_time_buffers_lost: u32,
}

/// The 1 ms default profile interval, in 100 ns units.
const DEFAULT_PROFILE_INTERVAL: u32 = 10_000;
/// The fastest the kernel allows (8192 Hz), in 100 ns units.
const MIN_PROFILE_INTERVAL: u32 = 1221;

pub struct KernelSession {
    handle: TRACEHANDLE,
    name: Vec<u16>,
    properties: Box<SessionProperties>,
    clock: QpcClock,
    options: SessionOptions,
    stopped: bool,
}

impl KernelSession {
    /// Starts the kernel logger with `options`. Needs an elevated token.
    pub fn start(options: SessionOptions) -> Result<KernelSession, String> {
        // The NT Kernel Logger requires SeSystemProfilePrivilege enabled on
        // the caller's token -- an elevated token holds it but disabled, so
        // StartTrace returns ERROR_PRIVILEGE_NOT_HELD (1314) without this.
        // The sampling-interval call below needs it too; enabling it once
        // here covers both.
        crate::token::enable_system_profile_privilege()?;
        let name = wide(KERNEL_LOGGER_NAME);
        // A session left behind by a controller that died holds the name.
        let _ = Self::stop_by_name(&name);

        let mut properties = SessionProperties::zeroed();
        {
            let p = &mut properties.properties;
            p.Wnode.Guid = SYSTEM_TRACE_CONTROL_GUID;
            p.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
            p.Wnode.ClientContext = CLOCK_QPC;
            p.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
            p.BufferSize = options.buffer_kb;
            p.MinimumBuffers = options.min_buffers;
            p.MaximumBuffers = options.max_buffers.max(options.min_buffers);
            p.FlushTimer = 1;
            let mut flags = EVENT_TRACE_FLAG_THREAD | EVENT_TRACE_FLAG_PROCESS;
            if options.scheduler {
                flags |= EVENT_TRACE_FLAG_CSWITCH | EVENT_TRACE_FLAG_DISPATCHER;
            }
            if options.images {
                flags |= EVENT_TRACE_FLAG_IMAGE_LOAD;
            }
            if options.sampling_hz > 0 {
                flags |= EVENT_TRACE_FLAG_PROFILE;
            }
            p.EnableFlags = flags;
        }
        let mut handle: TRACEHANDLE = 0;
        // SAFETY: the name is NUL-terminated and the properties block is
        // sized by its own Wnode.BufferSize.
        let mut status = unsafe { StartTraceW(&mut handle, name.as_ptr(), &mut properties.properties) };
        if status == ERROR_ALREADY_EXISTS {
            let _ = Self::stop_by_name(&name);
            status = unsafe { StartTraceW(&mut handle, name.as_ptr(), &mut properties.properties) };
        }
        match status {
            ERROR_SUCCESS => {}
            ERROR_ACCESS_DENIED => {
                return Err("the kernel logger needs an elevated token: run orbit-service from a prompt opened with \"Run as administrator\"".into());
            }
            other => return Err(format!("StartTrace(NT Kernel Logger): {} ({other})", error_text(other))),
        }
        let mut session = KernelSession { handle, name, properties, clock: QpcClock::host(), options, stopped: false };
        if options.sampling_hz > 0 {
            if let Err(error) = session.enable_sampling(options.sampling_hz) {
                let _ = session.stop();
                return Err(error);
            }
        }
        Ok(session)
    }

    /// The profiler interval, then stack walks on every profile tick.
    fn enable_sampling(&mut self, hz: u64) -> Result<(), String> {
        // The privilege was enabled in `start`, before StartTrace.
        let interval = (10_000_000 / hz.max(1)).clamp(u64::from(MIN_PROFILE_INTERVAL), u64::from(DEFAULT_PROFILE_INTERVAL) * 100) as u32;
        Self::set_profile_interval(interval)?;
        let ids = [CLASSIC_EVENT_ID { EventGuid: PERF_INFO_GUID, Type: crate::events::opcode::SAMPLED_PROFILE, Reserved: [0; 7] }];
        // SAFETY: an array of one CLASSIC_EVENT_ID, on the live session.
        let status = unsafe {
            TraceSetInformation(
                self.handle,
                TRACE_STACK_TRACING_INFO,
                ids.as_ptr().cast(),
                std::mem::size_of::<CLASSIC_EVENT_ID>() as u32,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("TraceSetInformation(TraceStackTracingInfo): {}", error_text(status)));
        }
        Ok(())
    }

    fn set_profile_interval(interval: u32) -> Result<(), String> {
        let info = TRACE_PROFILE_INTERVAL { Source: 0, Interval: interval };
        // SAFETY: a machine-wide setting; the handle is 0 by contract.
        let status = unsafe {
            TraceSetInformation(0, TRACE_SAMPLED_PROFILE_INTERVAL_INFO, (&info as *const TRACE_PROFILE_INTERVAL).cast(), 8)
        };
        if status != ERROR_SUCCESS {
            return Err(format!("TraceSetInformation(TraceSampledProfileIntervalInfo): {} (needs SeSystemProfilePrivilege)", error_text(status)));
        }
        Ok(())
    }

    fn stop_by_name(name: &[u16]) -> Result<(), u32> {
        let mut properties = SessionProperties::zeroed();
        // SAFETY: a zeroed, sized properties block; stop by name only.
        let status = unsafe { ControlTraceW(0, name.as_ptr(), &mut properties.properties, EVENT_TRACE_CONTROL_STOP) };
        if status == ERROR_SUCCESS || status == ERROR_WMI_INSTANCE_NOT_FOUND {
            Ok(())
        } else {
            Err(status)
        }
    }

    pub fn clock(&self) -> QpcClock {
        self.clock
    }

    pub fn options(&self) -> SessionOptions {
        self.options
    }

    /// The session name the consumer opens.
    pub fn name(&self) -> &str {
        KERNEL_LOGGER_NAME
    }

    fn stats_from(&self) -> SessionStats {
        let p = &self.properties.properties;
        SessionStats {
            buffers: p.NumberOfBuffers,
            free_buffers: p.FreeBuffers,
            events_lost: p.EventsLost,
            buffers_written: p.BuffersWritten,
            real_time_buffers_lost: p.RealTimeBuffersLost,
        }
    }

    /// The kernel's own counters for the running session.
    pub fn query(&mut self) -> Result<SessionStats, String> {
        // SAFETY: the live handle and its own properties block.
        let status = unsafe {
            ControlTraceW(self.handle, self.name.as_ptr(), &mut self.properties.properties, EVENT_TRACE_CONTROL_QUERY)
        };
        if status != ERROR_SUCCESS {
            return Err(format!("ControlTrace(query): {}", error_text(status)));
        }
        Ok(self.stats_from())
    }

    /// Stops the session and returns its final counters.
    pub fn stop(&mut self) -> Result<SessionStats, String> {
        if self.stopped {
            return Ok(self.stats_from());
        }
        self.stopped = true;
        // SAFETY: as for query.
        let status = unsafe {
            ControlTraceW(self.handle, self.name.as_ptr(), &mut self.properties.properties, EVENT_TRACE_CONTROL_STOP)
        };
        if self.options.sampling_hz > 0 {
            let _ = Self::set_profile_interval(DEFAULT_PROFILE_INTERVAL);
        }
        if status != ERROR_SUCCESS {
            return Err(format!("ControlTrace(stop): {}", error_text(status)));
        }
        Ok(self.stats_from())
    }
}

impl Drop for KernelSession {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
