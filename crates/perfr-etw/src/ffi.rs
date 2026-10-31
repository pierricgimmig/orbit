// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The Win32 surface of an ETW controller and consumer, declared by hand.
//!
//! Every structure here is `#[repr(C)]` with its size pinned by a `const`
//! assertion against the SDK's 64-bit layout, so a field added in the wrong
//! place fails to compile rather than corrupting a session.

#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms)]

use std::ffi::c_void;

pub type TRACEHANDLE = u64;
pub type HANDLE = *mut c_void;

pub const INVALID_PROCESSTRACE_HANDLE: TRACEHANDLE = u64::MAX;

pub const ERROR_SUCCESS: u32 = 0;
pub const ERROR_ACCESS_DENIED: u32 = 5;
pub const ERROR_ALREADY_EXISTS: u32 = 183;
pub const ERROR_WMI_INSTANCE_NOT_FOUND: u32 = 4201;
pub const ERROR_CTX_CLOSE_PENDING: u32 = 7007;

pub const WNODE_FLAG_TRACED_GUID: u32 = 0x0002_0000;
/// `ClientContext` 1: QueryPerformanceCounter timestamps.
pub const CLOCK_QPC: u32 = 1;
pub const EVENT_TRACE_REAL_TIME_MODE: u32 = 0x0000_0100;

pub const EVENT_TRACE_FLAG_PROCESS: u32 = 0x0000_0001;
pub const EVENT_TRACE_FLAG_THREAD: u32 = 0x0000_0002;
pub const EVENT_TRACE_FLAG_IMAGE_LOAD: u32 = 0x0000_0004;
pub const EVENT_TRACE_FLAG_CSWITCH: u32 = 0x0000_0010;
pub const EVENT_TRACE_FLAG_DISPATCHER: u32 = 0x0000_0800;
pub const EVENT_TRACE_FLAG_PROFILE: u32 = 0x0100_0000;

pub const EVENT_TRACE_CONTROL_QUERY: u32 = 0;
pub const EVENT_TRACE_CONTROL_STOP: u32 = 1;

/// `TRACE_INFO_CLASS`.
pub const TRACE_STACK_TRACING_INFO: u32 = 3;
pub const TRACE_SAMPLED_PROFILE_INTERVAL_INFO: u32 = 5;

pub const PROCESS_TRACE_MODE_REAL_TIME: u32 = 0x0000_0100;
pub const PROCESS_TRACE_MODE_RAW_TIMESTAMP: u32 = 0x0000_1000;
pub const PROCESS_TRACE_MODE_EVENT_RECORD: u32 = 0x1000_0000;

pub const EVENT_HEADER_FLAG_32_BIT_HEADER: u16 = 0x0020;
pub const EVENT_HEADER_FLAG_64_BIT_HEADER: u16 = 0x0040;

/// The kernel logger's fixed session name.
pub const KERNEL_LOGGER_NAME: &str = "NT Kernel Logger";

#[repr(C)]
pub struct WNODE_HEADER {
    pub BufferSize: u32,
    pub ProviderId: u32,
    pub HistoricalContext: u64,
    pub TimeStamp: i64,
    pub Guid: [u8; 16],
    pub ClientContext: u32,
    pub Flags: u32,
}
const _: () = assert!(std::mem::size_of::<WNODE_HEADER>() == 48);

#[repr(C)]
pub struct EVENT_TRACE_PROPERTIES {
    pub Wnode: WNODE_HEADER,
    pub BufferSize: u32,
    pub MinimumBuffers: u32,
    pub MaximumBuffers: u32,
    pub MaximumFileSize: u32,
    pub LogFileMode: u32,
    pub FlushTimer: u32,
    pub EnableFlags: u32,
    pub AgeLimit: i32,
    pub NumberOfBuffers: u32,
    pub FreeBuffers: u32,
    pub EventsLost: u32,
    pub BuffersWritten: u32,
    pub LogBuffersLost: u32,
    pub RealTimeBuffersLost: u32,
    pub LoggerThreadId: HANDLE,
    pub LogFileNameOffset: u32,
    pub LoggerNameOffset: u32,
}
const _: () = assert!(std::mem::size_of::<EVENT_TRACE_PROPERTIES>() == 120);

/// Room for the session name and the (empty) log file name after the
/// properties, as `StartTrace` requires.
pub const NAME_BYTES: usize = 1024;

/// `EVENT_TRACE_PROPERTIES` with the two name buffers that follow it.
#[repr(C)]
pub struct SessionProperties {
    pub properties: EVENT_TRACE_PROPERTIES,
    pub logger_name: [u16; NAME_BYTES / 2],
    pub log_file_name: [u16; NAME_BYTES / 2],
}

impl SessionProperties {
    pub fn zeroed() -> Box<SessionProperties> {
        // SAFETY: every field is plain data for which all-zero is valid.
        let mut p: Box<SessionProperties> = Box::new(unsafe { std::mem::zeroed() });
        p.properties.Wnode.BufferSize = std::mem::size_of::<SessionProperties>() as u32;
        p.properties.LoggerNameOffset = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32;
        p.properties.LogFileNameOffset = (std::mem::size_of::<EVENT_TRACE_PROPERTIES>() + NAME_BYTES) as u32;
        p
    }
}

#[repr(C)]
pub struct CLASSIC_EVENT_ID {
    pub EventGuid: [u8; 16],
    pub Type: u8,
    pub Reserved: [u8; 7],
}
const _: () = assert!(std::mem::size_of::<CLASSIC_EVENT_ID>() == 24);

#[repr(C)]
pub struct TRACE_PROFILE_INTERVAL {
    pub Source: u32,
    /// In 100 ns units.
    pub Interval: u32,
}

#[repr(C)]
pub struct EVENT_DESCRIPTOR {
    pub Id: u16,
    pub Version: u8,
    pub Channel: u8,
    pub Level: u8,
    pub Opcode: u8,
    pub Task: u16,
    pub Keyword: u64,
}

#[repr(C)]
pub struct EVENT_HEADER {
    pub Size: u16,
    pub HeaderType: u16,
    pub Flags: u16,
    pub EventProperty: u16,
    pub ThreadId: u32,
    pub ProcessId: u32,
    pub TimeStamp: i64,
    pub ProviderId: [u8; 16],
    pub EventDescriptor: EVENT_DESCRIPTOR,
    pub ProcessorTime: u64,
    pub ActivityId: [u8; 16],
}
const _: () = assert!(std::mem::size_of::<EVENT_HEADER>() == 80);

#[repr(C)]
pub struct ETW_BUFFER_CONTEXT {
    pub ProcessorIndex: u16,
    pub LoggerId: u16,
}

#[repr(C)]
pub struct EVENT_RECORD {
    pub EventHeader: EVENT_HEADER,
    pub BufferContext: ETW_BUFFER_CONTEXT,
    pub ExtendedDataCount: u16,
    pub UserDataLength: u16,
    pub ExtendedData: *mut c_void,
    pub UserData: *mut c_void,
    pub UserContext: *mut c_void,
}
const _: () = assert!(std::mem::size_of::<EVENT_RECORD>() == 112);

#[repr(C)]
pub struct EVENT_TRACE_HEADER {
    pub Size: u16,
    pub FieldTypeFlags: u16,
    pub Version: u32,
    pub ThreadId: u32,
    pub ProcessId: u32,
    pub TimeStamp: i64,
    pub Guid: [u8; 16],
    pub ProcessorTime: u64,
}
const _: () = assert!(std::mem::size_of::<EVENT_TRACE_HEADER>() == 48);

#[repr(C)]
pub struct EVENT_TRACE {
    pub Header: EVENT_TRACE_HEADER,
    pub InstanceId: u32,
    pub ParentInstanceId: u32,
    pub ParentGuid: [u8; 16],
    pub MofData: *mut c_void,
    pub MofLength: u32,
    pub ClientContext: u32,
}
const _: () = assert!(std::mem::size_of::<EVENT_TRACE>() == 88);

#[repr(C)]
pub struct SYSTEMTIME {
    pub wYear: u16,
    pub wMonth: u16,
    pub wDayOfWeek: u16,
    pub wDay: u16,
    pub wHour: u16,
    pub wMinute: u16,
    pub wSecond: u16,
    pub wMilliseconds: u16,
}

#[repr(C)]
pub struct TIME_ZONE_INFORMATION {
    pub Bias: i32,
    pub StandardName: [u16; 32],
    pub StandardDate: SYSTEMTIME,
    pub StandardBias: i32,
    pub DaylightName: [u16; 32],
    pub DaylightDate: SYSTEMTIME,
    pub DaylightBias: i32,
}
const _: () = assert!(std::mem::size_of::<TIME_ZONE_INFORMATION>() == 172);

#[repr(C)]
pub struct TRACE_LOGFILE_HEADER {
    pub BufferSize: u32,
    pub Version: u32,
    pub ProviderVersion: u32,
    pub NumberOfProcessors: u32,
    pub EndTime: i64,
    pub TimerResolution: u32,
    pub MaximumFileSize: u32,
    pub LogFileMode: u32,
    pub BuffersWritten: u32,
    pub LogInstanceGuid: [u8; 16],
    pub LoggerName: *mut u16,
    pub LogFileName: *mut u16,
    pub TimeZone: TIME_ZONE_INFORMATION,
    pub BootTime: i64,
    pub PerfFreq: i64,
    pub StartTime: i64,
    pub ReservedFlags: u32,
    pub BuffersLost: u32,
}
const _: () = assert!(std::mem::size_of::<TRACE_LOGFILE_HEADER>() == 280);

pub type EventRecordCallback = unsafe extern "system" fn(*mut EVENT_RECORD);
pub type BufferCallback = unsafe extern "system" fn(*mut EVENT_TRACE_LOGFILEW) -> u32;

#[repr(C)]
pub struct EVENT_TRACE_LOGFILEW {
    pub LogFileName: *const u16,
    pub LoggerName: *const u16,
    pub CurrentTime: i64,
    pub BuffersRead: u32,
    pub ProcessTraceMode: u32,
    pub CurrentEvent: EVENT_TRACE,
    pub LogfileHeader: TRACE_LOGFILE_HEADER,
    pub BufferCallback: Option<BufferCallback>,
    pub BufferSize: u32,
    pub Filled: u32,
    pub EventsLost: u32,
    pub EventRecordCallback: Option<EventRecordCallback>,
    pub IsKernelTrace: u32,
    pub Context: *mut c_void,
}
const _: () = assert!(std::mem::size_of::<EVENT_TRACE_LOGFILEW>() == 448);

#[link(name = "advapi32")]
extern "system" {
    pub fn StartTraceW(handle: *mut TRACEHANDLE, name: *const u16, properties: *mut EVENT_TRACE_PROPERTIES) -> u32;
    pub fn ControlTraceW(handle: TRACEHANDLE, name: *const u16, properties: *mut EVENT_TRACE_PROPERTIES, control: u32) -> u32;
    pub fn TraceSetInformation(handle: TRACEHANDLE, class: u32, info: *const c_void, length: u32) -> u32;
    pub fn OpenTraceW(logfile: *mut EVENT_TRACE_LOGFILEW) -> TRACEHANDLE;
    pub fn ProcessTrace(handles: *const TRACEHANDLE, count: u32, start: *const c_void, end: *const c_void) -> u32;
    pub fn CloseTrace(handle: TRACEHANDLE) -> u32;
}

/// A Rust string as a NUL-terminated UTF-16 buffer.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The text of a Win32 error code, from the system message table.
pub fn error_text(code: u32) -> String {
    std::io::Error::from_raw_os_error(code as i32).to_string()
}
