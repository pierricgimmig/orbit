// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What the process token allows: the kernel logger needs an elevated
//! token, and the sampling interval needs `SeSystemProfilePrivilege`
//! enabled on it (an administrator holds it, disabled, until asked).

use std::ffi::c_void;

type HANDLE = *mut c_void;

#[repr(C)]
struct LUID {
    low: u32,
    high: i32,
}

#[repr(C)]
struct LUID_AND_ATTRIBUTES {
    luid: LUID,
    attributes: u32,
}

#[repr(C)]
struct TOKEN_PRIVILEGES {
    count: u32,
    privileges: [LUID_AND_ATTRIBUTES; 1],
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> HANDLE;
    fn CloseHandle(handle: HANDLE) -> i32;
    fn GetLastError() -> u32;
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(process: HANDLE, access: u32, token: *mut HANDLE) -> i32;
    fn GetTokenInformation(token: HANDLE, class: u32, info: *mut c_void, length: u32, returned: *mut u32) -> i32;
    fn LookupPrivilegeValueW(system: *const u16, name: *const u16, luid: *mut LUID) -> i32;
    fn AdjustTokenPrivileges(
        token: HANDLE,
        disable_all: i32,
        new_state: *const TOKEN_PRIVILEGES,
        length: u32,
        previous: *mut TOKEN_PRIVILEGES,
        returned: *mut u32,
    ) -> i32;
}

const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
const TOKEN_ELEVATION: u32 = 20;
const SE_PRIVILEGE_ENABLED: u32 = 0x0000_0002;
const ERROR_NOT_ALL_ASSIGNED: u32 = 1300;

struct Token(HANDLE);

impl Token {
    fn open(access: u32) -> Result<Token, String> {
        let mut handle: HANDLE = std::ptr::null_mut();
        // SAFETY: the pseudo-handle for the current process is always valid.
        if unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut handle) } == 0 {
            return Err(format!("OpenProcessToken: {}", crate::ffi::error_text(unsafe { GetLastError() })));
        }
        Ok(Token(handle))
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: a handle this value owns.
        unsafe { CloseHandle(self.0) };
    }
}

/// Whether this process runs with an elevated token (UAC "Run as
/// administrator", or an elevated shell). The kernel logger refuses every
/// other token with `ERROR_ACCESS_DENIED`.
pub fn is_elevated() -> bool {
    let Ok(token) = Token::open(TOKEN_QUERY) else { return false };
    let mut elevation: u32 = 0;
    let mut returned: u32 = 0;
    // SAFETY: TOKEN_ELEVATION is one u32; the buffer is exactly that.
    let ok = unsafe {
        GetTokenInformation(token.0, TOKEN_ELEVATION, (&mut elevation as *mut u32).cast(), 4, &mut returned)
    };
    ok != 0 && elevation != 0
}

/// Enables `SeSystemProfilePrivilege` on this process's token, which
/// `TraceSetInformation(TraceSampledProfileIntervalInfo)` requires.
pub fn enable_system_profile_privilege() -> Result<(), String> {
    let token = Token::open(TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY)?;
    let name = crate::ffi::wide("SeSystemProfilePrivilege");
    let mut luid = LUID { low: 0, high: 0 };
    // SAFETY: the name is NUL-terminated; luid is a live local.
    if unsafe { LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) } == 0 {
        return Err(format!("LookupPrivilegeValue: {}", crate::ffi::error_text(unsafe { GetLastError() })));
    }
    let state = TOKEN_PRIVILEGES {
        count: 1,
        privileges: [LUID_AND_ATTRIBUTES { luid, attributes: SE_PRIVILEGE_ENABLED }],
    };
    // SAFETY: a one-entry TOKEN_PRIVILEGES on an open token.
    let ok = unsafe { AdjustTokenPrivileges(token.0, 0, &state, 0, std::ptr::null_mut(), std::ptr::null_mut()) };
    // The call succeeds even when the privilege is not held; the last error
    // says whether it was actually assigned.
    let error = unsafe { GetLastError() };
    if ok == 0 || error == ERROR_NOT_ALL_ASSIGNED {
        return Err(
            "SeSystemProfilePrivilege is not held by this token: run perfr from an elevated prompt".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevation_is_a_plain_boolean_and_the_privilege_follows_it() {
        // Not a claim about the test's own token: an elevated token must
        // be able to enable the privilege, an unelevated one must be told
        // why not, and neither path may panic.
        let elevated = is_elevated();
        match enable_system_profile_privilege() {
            Ok(()) => assert!(elevated, "the privilege was enabled on an unelevated token"),
            Err(message) => assert!(message.contains("elevated"), "{message}"),
        }
    }
}
