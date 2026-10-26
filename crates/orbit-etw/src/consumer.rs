// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The consumer: a thread inside `ProcessTrace`, decoding events into a
//! queue the capture loop drains on its own schedule.
//!
//! `ProcessTrace` blocks for the life of the session and invokes the
//! record callback for every event, in timestamp order across buffers. The
//! callback decodes with [`crate::events::decode`] and appends to a vector
//! under a mutex; [`Consumer::drain`] swaps that vector out, so the capture
//! loop takes a whole batch per pass for one lock. Nothing else is shared.
//!
//! The same consumer opens a saved `.etl` file: the callback does not know
//! the difference, which is how the decoders are exercised against a
//! recording without an elevated prompt.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::events::{decode, KernelEvent, RawEvent};
use crate::ffi::*;

struct Shared {
    pending: Mutex<Vec<KernelEvent>>,
    events: AtomicU64,
    ignored: AtomicU64,
    lost: AtomicU64,
    stop: AtomicBool,
}

/// Counters for the end-of-capture summary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConsumerStats {
    /// Events the callback saw.
    pub events: u64,
    /// Events decoded as `Other` and dropped.
    pub ignored: u64,
    /// Events the kernel reported lost from buffers this consumer read.
    pub lost: u64,
}

pub struct Consumer {
    handle: TRACEHANDLE,
    thread: Option<std::thread::JoinHandle<u32>>,
    shared: Arc<Shared>,
    // Keeps the name buffers alive for the life of the handle.
    _logfile: Box<EVENT_TRACE_LOGFILEW>,
    _names: (Vec<u16>, Vec<u16>),
}

unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    // SAFETY: ETW hands a valid record for the duration of the callback,
    // and UserContext is the Arc<Shared> pointer installed by `open`.
    let record = &*record;
    let shared = &*(record.UserContext as *const Shared);
    shared.events.fetch_add(1, Ordering::Relaxed);
    let header = &record.EventHeader;
    if header.Flags & EVENT_HEADER_FLAG_32_BIT_HEADER != 0 && header.Flags & EVENT_HEADER_FLAG_64_BIT_HEADER == 0 {
        // A 32-bit payload layout; the decoders are 64-bit only.
        shared.ignored.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let payload: &[u8] = if record.UserData.is_null() || record.UserDataLength == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(record.UserData as *const u8, record.UserDataLength as usize)
    };
    let raw = RawEvent {
        provider: &header.ProviderId,
        opcode: header.EventDescriptor.Opcode,
        version: header.EventDescriptor.Version,
        timestamp: header.TimeStamp,
        cpu: record.BufferContext.ProcessorIndex,
        header_tid: header.ThreadId,
        header_pid: header.ProcessId,
        payload,
    };
    match decode(&raw) {
        KernelEvent::Other => {
            shared.ignored.fetch_add(1, Ordering::Relaxed);
        }
        event => {
            shared.pending.lock().unwrap_or_else(|e| e.into_inner()).push(event);
        }
    }
}

unsafe extern "system" fn on_buffer(logfile: *mut EVENT_TRACE_LOGFILEW) -> u32 {
    // SAFETY: as for on_event.
    let logfile = &*logfile;
    let shared = &*(logfile.Context as *const Shared);
    shared.lost.fetch_add(u64::from(logfile.EventsLost), Ordering::Relaxed);
    // Returning FALSE ends ProcessTrace at the next buffer.
    (!shared.stop.load(Ordering::Acquire)) as u32
}

impl Consumer {
    /// Opens the real-time session `logger_name` and starts consuming.
    pub fn open_realtime(logger_name: &str) -> Result<Consumer, String> {
        let names = (wide(logger_name), Vec::new());
        Self::open(names, true)
    }

    /// Replays a saved `.etl` file; the thread ends when the file does.
    pub fn open_file(path: &str) -> Result<Consumer, String> {
        let names = (Vec::new(), wide(path));
        Self::open(names, false)
    }

    fn open(names: (Vec<u16>, Vec<u16>), realtime: bool) -> Result<Consumer, String> {
        let shared = Arc::new(Shared {
            pending: Mutex::new(Vec::with_capacity(4096)),
            events: AtomicU64::new(0),
            ignored: AtomicU64::new(0),
            lost: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        });
        // SAFETY: all-zero is a valid EVENT_TRACE_LOGFILEW.
        let mut logfile: Box<EVENT_TRACE_LOGFILEW> = Box::new(unsafe { std::mem::zeroed() });
        if realtime {
            logfile.LoggerName = names.0.as_ptr();
            logfile.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
        } else {
            logfile.LogFileName = names.1.as_ptr();
            logfile.ProcessTraceMode = PROCESS_TRACE_MODE_EVENT_RECORD | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
        }
        logfile.EventRecordCallback = Some(on_event);
        logfile.BufferCallback = Some(on_buffer);
        logfile.Context = Arc::as_ptr(&shared) as *mut c_void;
        // SAFETY: the logfile block outlives the handle (it is kept in the
        // Consumer), and its name pointers point into `names`, also kept.
        let handle = unsafe { OpenTraceW(&mut *logfile) };
        if handle == INVALID_PROCESSTRACE_HANDLE {
            let error = std::io::Error::last_os_error();
            return Err(format!("OpenTrace: {error}"));
        }
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("orbit-etw".to_string())
            .spawn(move || {
                let _keep = thread_shared;
                // SAFETY: a handle OpenTrace returned; blocks until the
                // session stops or the buffer callback says so.
                unsafe { ProcessTrace(&handle, 1, std::ptr::null(), std::ptr::null()) }
            })
            .map_err(|e| e.to_string())?;
        Ok(Consumer { handle, thread: Some(thread), shared, _logfile: logfile, _names: names })
    }

    /// Takes every event decoded since the last call, in delivery order.
    pub fn drain(&self, out: &mut Vec<KernelEvent>) {
        let mut pending = self.shared.pending.lock().unwrap_or_else(|e| e.into_inner());
        if out.is_empty() {
            std::mem::swap(out, &mut pending);
        } else {
            out.append(&mut pending);
        }
    }

    pub fn stats(&self) -> ConsumerStats {
        ConsumerStats {
            events: self.shared.events.load(Ordering::Relaxed),
            ignored: self.shared.ignored.load(Ordering::Relaxed),
            lost: self.shared.lost.load(Ordering::Relaxed),
        }
    }

    /// Whether `ProcessTrace` has returned (the session ended, or a file
    /// was fully read).
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// Ends consumption and waits for the thread; events delivered up to
    /// the end stay in the queue for a last [`Consumer::drain`]. Returns
    /// `ProcessTrace`'s status: `ERROR_SUCCESS` for a session that was
    /// stopped, `ERROR_CANCELLED` when the buffer callback ended it.
    pub fn close(&mut self) -> Result<u32, String> {
        if self.handle == INVALID_PROCESSTRACE_HANDLE {
            return Err("consumer already closed".into());
        }
        self.shared.stop.store(true, Ordering::Release);
        // SAFETY: the handle this consumer opened. On a real-time session
        // CloseTrace returns ERROR_CTX_CLOSE_PENDING and ProcessTrace
        // returns once the current buffer is delivered.
        let status = unsafe { CloseTrace(self.handle) };
        self.handle = INVALID_PROCESSTRACE_HANDLE;
        if status != ERROR_SUCCESS && status != ERROR_CTX_CLOSE_PENDING {
            return Err(format!("CloseTrace: {}", error_text(status)));
        }
        let thread = self.thread.take().ok_or("consumer thread already joined")?;
        thread.join().map_err(|_| "the ETW consumer thread panicked".to_string())
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        if self.handle != INVALID_PROCESSTRACE_HANDLE {
            self.shared.stop.store(true, Ordering::Release);
            // SAFETY: as in close.
            unsafe { CloseTrace(self.handle) };
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}
