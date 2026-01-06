//! C FFI wrapper for py-spy to integrate with OrbitService
//!
//! This crate provides a C-compatible interface to py-spy's Python profiling
//! capabilities, allowing C++ code to sample Python stack traces.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::ptr;

use py_spy::{Config, PythonSpy, StackTrace};

/// Opaque wrapper around PythonSpy that can be passed across FFI boundary
pub struct PythonSpyWrapper {
    spy: PythonSpy,
}

/// A single frame in a Python stack trace (C-compatible layout)
#[repr(C)]
pub struct PySpyFrame {
    /// Function name (null-terminated C string, caller must free)
    pub function_name: *mut c_char,
    /// Source filename (null-terminated C string, caller must free)
    pub filename: *mut c_char,
    /// Line number in source file
    pub line: c_int,
}

/// A complete Python stack trace for one thread (C-compatible layout)
#[repr(C)]
pub struct PySpyStackTrace {
    /// Python thread ID
    pub thread_id: u64,
    /// OS thread ID (may be 0 if not available)
    pub os_thread_id: u64,
    /// Whether this thread holds the GIL
    pub owns_gil: bool,
    /// Whether this thread is active
    pub active: bool,
    /// Array of frames (caller must free)
    pub frames: *mut PySpyFrame,
    /// Number of frames in the array
    pub frame_count: usize,
}

/// Error codes returned by FFI functions
#[repr(C)]
pub enum PySpyError {
    Success = 0,
    NullPointer = -1,
    ProcessNotFound = -2,
    PermissionDenied = -3,
    NotPythonProcess = -4,
    SamplingError = -5,
    UnknownError = -99,
}

/// Create a new PythonSpy instance for the given process ID.
///
/// # Safety
/// The returned pointer must be freed with `pyspy_free`.
/// Returns null on error.
#[no_mangle]
pub unsafe extern "C" fn pyspy_new(pid: u32, error_out: *mut c_int) -> *mut PythonSpyWrapper {
    let config = Config {
        blocking: py_spy::config::LockingStrategy::Lock,
        native: false,
        ..Default::default()
    };

    match PythonSpy::new(pid as py_spy::Pid, &config) {
        Ok(spy) => {
            if !error_out.is_null() {
                *error_out = PySpyError::Success as c_int;
            }
            Box::into_raw(Box::new(PythonSpyWrapper { spy }))
        }
        Err(e) => {
            let error_code = if e.to_string().contains("Permission denied") {
                PySpyError::PermissionDenied
            } else if e.to_string().contains("No such process") {
                PySpyError::ProcessNotFound
            } else if e.to_string().contains("Couldn't find python") {
                PySpyError::NotPythonProcess
            } else {
                PySpyError::UnknownError
            };

            if !error_out.is_null() {
                *error_out = error_code as c_int;
            }
            ptr::null_mut()
        }
    }
}

/// Create a new PythonSpy instance with native stack unwinding enabled.
///
/// # Safety
/// The returned pointer must be freed with `pyspy_free`.
/// Returns null on error.
#[no_mangle]
pub unsafe extern "C" fn pyspy_new_with_native(
    pid: u32,
    error_out: *mut c_int,
) -> *mut PythonSpyWrapper {
    let config = Config {
        blocking: py_spy::config::LockingStrategy::Lock,
        native: true,
        ..Default::default()
    };

    match PythonSpy::new(pid as py_spy::Pid, &config) {
        Ok(spy) => {
            if !error_out.is_null() {
                *error_out = PySpyError::Success as c_int;
            }
            Box::into_raw(Box::new(PythonSpyWrapper { spy }))
        }
        Err(e) => {
            let error_code = if e.to_string().contains("Permission denied") {
                PySpyError::PermissionDenied
            } else if e.to_string().contains("No such process") {
                PySpyError::ProcessNotFound
            } else if e.to_string().contains("Couldn't find python") {
                PySpyError::NotPythonProcess
            } else {
                PySpyError::UnknownError
            };

            if !error_out.is_null() {
                *error_out = error_code as c_int;
            }
            ptr::null_mut()
        }
    }
}

/// Get stack traces for all Python threads in the process.
///
/// # Safety
/// - `spy` must be a valid pointer returned by `pyspy_new`
/// - `traces_out` must be a valid pointer to store the result array
/// - `count_out` must be a valid pointer to store the array length
/// - The returned traces must be freed with `pyspy_free_traces`
#[no_mangle]
pub unsafe extern "C" fn pyspy_get_stack_traces(
    spy: *mut PythonSpyWrapper,
    traces_out: *mut *mut PySpyStackTrace,
    count_out: *mut usize,
) -> c_int {
    if spy.is_null() || traces_out.is_null() || count_out.is_null() {
        return PySpyError::NullPointer as c_int;
    }

    let wrapper = &mut *spy;

    match wrapper.spy.get_stack_traces() {
        Ok(traces) => {
            let c_traces = convert_traces_to_c(&traces);
            let len = c_traces.len();

            if len == 0 {
                *traces_out = ptr::null_mut();
                *count_out = 0;
            } else {
                let boxed = c_traces.into_boxed_slice();
                *count_out = len;
                *traces_out = Box::into_raw(boxed) as *mut PySpyStackTrace;
            }

            PySpyError::Success as c_int
        }
        Err(_) => PySpyError::SamplingError as c_int,
    }
}

/// Convert py-spy StackTraces to C-compatible format
fn convert_traces_to_c(traces: &[StackTrace]) -> Vec<PySpyStackTrace> {
    traces
        .iter()
        .map(|trace| {
            let frames: Vec<PySpyFrame> = trace
                .frames
                .iter()
                .map(|frame| {
                    let function_name = CString::new(frame.name.as_str())
                        .unwrap_or_else(|_| CString::new("").unwrap())
                        .into_raw();
                    let filename = CString::new(frame.filename.as_str())
                        .unwrap_or_else(|_| CString::new("").unwrap())
                        .into_raw();

                    PySpyFrame {
                        function_name,
                        filename,
                        line: frame.line,
                    }
                })
                .collect();

            let frame_count = frames.len();
            let frames_ptr = if frames.is_empty() {
                ptr::null_mut()
            } else {
                let boxed = frames.into_boxed_slice();
                Box::into_raw(boxed) as *mut PySpyFrame
            };

            PySpyStackTrace {
                thread_id: trace.thread_id,
                os_thread_id: trace.os_thread_id.unwrap_or(0),
                owns_gil: trace.owns_gil,
                active: trace.active,
                frames: frames_ptr,
                frame_count,
            }
        })
        .collect()
}

/// Free a PythonSpy instance.
///
/// # Safety
/// - `spy` must be a valid pointer returned by `pyspy_new` or null
/// - The pointer must not be used after this call
#[no_mangle]
pub unsafe extern "C" fn pyspy_free(spy: *mut PythonSpyWrapper) {
    if !spy.is_null() {
        drop(Box::from_raw(spy));
    }
}

/// Free stack traces returned by `pyspy_get_stack_traces`.
///
/// # Safety
/// - `traces` must be a valid pointer returned by `pyspy_get_stack_traces` or null
/// - `count` must be the count returned by `pyspy_get_stack_traces`
#[no_mangle]
pub unsafe extern "C" fn pyspy_free_traces(traces: *mut PySpyStackTrace, count: usize) {
    if traces.is_null() || count == 0 {
        return;
    }

    let traces_slice = std::slice::from_raw_parts_mut(traces, count);

    for trace in traces_slice.iter_mut() {
        // Free frames
        if !trace.frames.is_null() && trace.frame_count > 0 {
            let frames_slice =
                std::slice::from_raw_parts_mut(trace.frames, trace.frame_count);

            for frame in frames_slice.iter_mut() {
                // Free strings
                if !frame.function_name.is_null() {
                    drop(CString::from_raw(frame.function_name));
                }
                if !frame.filename.is_null() {
                    drop(CString::from_raw(frame.filename));
                }
            }

            // Free frames array
            drop(Box::from_raw(std::slice::from_raw_parts_mut(
                trace.frames,
                trace.frame_count,
            )));
        }
    }

    // Free traces array
    drop(Box::from_raw(traces_slice));
}

/// Get the last error message (for debugging).
/// Returns a static string that should not be freed.
#[no_mangle]
pub extern "C" fn pyspy_error_string(error_code: c_int) -> *const c_char {
    let msg = match error_code {
        0 => "Success\0",
        -1 => "Null pointer argument\0",
        -2 => "Process not found\0",
        -3 => "Permission denied (try running as root)\0",
        -4 => "Not a Python process\0",
        -5 => "Error sampling stack traces\0",
        _ => "Unknown error\0",
    };
    msg.as_ptr() as *const c_char
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_string() {
        unsafe {
            let msg = pyspy_error_string(0);
            assert!(!msg.is_null());
            let s = CStr::from_ptr(msg).to_str().unwrap();
            assert_eq!(s, "Success");
        }
    }
}
