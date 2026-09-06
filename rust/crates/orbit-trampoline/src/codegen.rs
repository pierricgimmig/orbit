// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The fixed machine-code sequences of a trampoline (Phase 6e), twin of the
//! `Append*Code` emitters in `Trampoline.cpp`. On entry the trampoline backs
//! up every register the payload might touch, calls the entry payload, then
//! restores and jumps back into the relocated prologue; the return
//! trampoline mirrors it around the exit payload.
//!
//! The repetitive vector save/restore is generated from the register index
//! here rather than written out sixteen times; the byte encodings are
//! locked against the C++ source verbatim in the tests, which are the spec.

/// The `jmp rel32` that overwrites the start of an instrumented function; its
/// size drives how much prologue must be relocated.
pub const SIZE_OF_JMP: usize = 5;

/// Placeholder for the function id, patched per instrumentation.
const FUNCTION_ID_PLACEHOLDER: u64 = 0xDEAD_BEEF_DEAD_BEEF;

/// Offset of the function-id immediate within the entry trampoline; the C++
/// asserts this exact value (`kOffsetOfFunctionIdInCallToEntryPayload`).
pub const OFFSET_OF_FUNCTION_ID: usize = 178;

fn push_general_purpose_registers(code: &mut Vec<u8>) {
    // push rax, rcx, rdx, rsi, rdi, r8, r9, r10, r11.
    code.extend_from_slice(&[0x50, 0x51, 0x52, 0x56, 0x57]);
    code.extend_from_slice(&[0x41, 0x50, 0x41, 0x51, 0x41, 0x52, 0x41, 0x53]);
}

fn pop_general_purpose_registers(code: &mut Vec<u8>) {
    // pop r11, r10, r9, r8, rdi, rsi, rdx, rcx, rax.
    code.extend_from_slice(&[0x41, 0x5b, 0x41, 0x5a, 0x41, 0x59, 0x41, 0x58]);
    code.extend_from_slice(&[0x5f, 0x5e, 0x5a, 0x59, 0x58]);
}

fn align_stack_to_32(code: &mut Vec<u8>) {
    // mov rax, rsp; and rsp, ~0x1f; sub rsp, 0x18; push rax.
    code.extend_from_slice(&[0x48, 0x89, 0xe0]);
    code.extend_from_slice(&[0x48, 0x83, 0xe4, 0xe0]);
    code.extend_from_slice(&[0x48, 0x83, 0xec, 0x18]);
    code.push(0x50);
}

fn modrm_rsp(reg: u8) -> u8 {
    // [rsp] operand with register `reg`: mod=00 rm=100 (SIB follows), reg field.
    0x04 | ((reg & 7) << 3)
}

/// Save ymm0..15 (AVX) or xmm0..15 (SSE) onto the stack, each preceded by the
/// matching `sub rsp`.
fn save_vector_registers(code: &mut Vec<u8>, avx: bool) {
    for reg in 0u8..16 {
        if avx {
            code.extend_from_slice(&[0x48, 0x83, 0xec, 0x20]); // sub rsp, 32
            // vmovdqa [rsp], ymmN
            code.extend_from_slice(&[0xc5, if reg < 8 { 0xfd } else { 0x7d }, 0x7f, modrm_rsp(reg), 0x24]);
        } else {
            code.extend_from_slice(&[0x48, 0x83, 0xec, 0x10]); // sub rsp, 16
            // movdqa [rsp], xmmN
            if reg < 8 {
                code.extend_from_slice(&[0x66, 0x0f, 0x7f, modrm_rsp(reg), 0x24]);
            } else {
                code.extend_from_slice(&[0x66, 0x44, 0x0f, 0x7f, modrm_rsp(reg), 0x24]);
            }
        }
    }
}

/// Restore ymm15..0 (AVX) or xmm15..0 (SSE), each followed by the matching
/// `add rsp`.
fn restore_vector_registers(code: &mut Vec<u8>, avx: bool) {
    for reg in (0u8..16).rev() {
        if avx {
            // vmovdqa ymmN, [rsp]
            code.extend_from_slice(&[0xc5, if reg < 8 { 0xfd } else { 0x7d }, 0x6f, modrm_rsp(reg), 0x24]);
            code.extend_from_slice(&[0x48, 0x83, 0xc4, 0x20]); // add rsp, 32
        } else {
            // movdqa xmmN, [rsp]
            if reg < 8 {
                code.extend_from_slice(&[0x66, 0x0f, 0x6f, modrm_rsp(reg), 0x24]);
            } else {
                code.extend_from_slice(&[0x66, 0x44, 0x0f, 0x6f, modrm_rsp(reg), 0x24]);
            }
            code.extend_from_slice(&[0x48, 0x83, 0xc4, 0x10]); // add rsp, 16
        }
    }
}

/// Whether this CPU has AVX (drives which vector-register width is saved).
pub fn host_has_avx() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::is_x86_feature_detected!("avx")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Twin of `AppendBackupCode`: save the GP registers, align the stack, save
/// the vector registers.
pub fn backup_code(avx: bool) -> Vec<u8> {
    let mut code = Vec::new();
    push_general_purpose_registers(&mut code);
    align_stack_to_32(&mut code);
    save_vector_registers(&mut code, avx);
    code
}

/// Twin of `AppendRestoreCode`: restore the vector registers, undo the
/// alignment, restore the GP registers.
pub fn restore_code(avx: bool) -> Vec<u8> {
    let mut code = Vec::new();
    restore_vector_registers(&mut code, avx);
    code.push(0x5c); // pop rsp
    pop_general_purpose_registers(&mut code);
    code
}

/// Twin of `AppendCallToEntryPayload`. Emitted right after `backup_code`, so
/// the running trampoline offset of the function-id immediate is
/// OFFSET_OF_FUNCTION_ID; the caller patches that id per instrumentation.
pub fn call_to_entry_payload(entry_payload_address: u64, return_trampoline_address: u64) -> Vec<u8> {
    let mut code = Vec::new();
    code.extend_from_slice(&[0x48, 0x83, 0xc0, 0x48]); // add rax, 0x48
    code.extend_from_slice(&[0x48, 0x8b, 0x38]); // mov rdi, [rax]
    code.extend_from_slice(&[0x48, 0xbe]); // mov rsi, imm64 (function id)
    code.extend_from_slice(&FUNCTION_ID_PLACEHOLDER.to_le_bytes());
    code.extend_from_slice(&[0x48, 0x89, 0xc2]); // mov rdx, rax
    code.extend_from_slice(&[0x48, 0xb9]); // mov rcx, imm64 (return trampoline)
    code.extend_from_slice(&return_trampoline_address.to_le_bytes());
    code.extend_from_slice(&[0x48, 0xb8]); // mov rax, imm64 (entry payload)
    code.extend_from_slice(&entry_payload_address.to_le_bytes());
    code.extend_from_slice(&[0xff, 0xd0]); // call rax
    code
}

/// Twin of `AppendJumpBackCode`: `jmp rel32` back to the instrumented
/// function after the prologue.
pub fn jump_back_code(offset: i32) -> Vec<u8> {
    let mut code = vec![0xe9];
    code.extend_from_slice(&offset.to_le_bytes());
    code
}

/// Twin of `AppendCallToExitPayloadAndJumpToReturnAddress`: the whole return
/// trampoline. Runs after the instrumented function returns, calls the exit
/// payload, then returns to the original caller.
pub fn call_to_exit_payload_and_jump_to_return_address(
    exit_payload_address: u64,
    avx: bool,
) -> Vec<u8> {
    let mut code = Vec::new();
    code.extend_from_slice(&[0x48, 0x83, 0xec, 0x08]); // sub rsp, 8 (space for return addr)
    push_general_purpose_registers(&mut code);
    align_stack_to_32(&mut code);
    save_vector_registers(&mut code, avx);
    code.extend_from_slice(&[0x48, 0x83, 0xc0, 0x48]); // add rax, 72
    code.push(0x50); // push rax (location of return address)
    code.extend_from_slice(&[0x48, 0x83, 0xec, 0x08]); // sub rsp, 8 (realign to 16)
    code.extend_from_slice(&[0x48, 0xb8]); // mov rax, imm64 (exit payload)
    code.extend_from_slice(&exit_payload_address.to_le_bytes());
    code.extend_from_slice(&[0xff, 0xd0]); // call rax
    code.extend_from_slice(&[0x48, 0x83, 0xc4, 0x08]); // add rsp, 8
    code.push(0x59); // pop rcx (return-address location)
    code.extend_from_slice(&[0x48, 0x89, 0x01]); // mov [rcx], rax (store original return addr)
    restore_vector_registers(&mut code, avx);
    code.push(0x5c); // pop rsp
    pop_general_purpose_registers(&mut code);
    code.push(0xc3); // ret
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    // The golden byte sequences transcribed from Trampoline.cpp. These ARE
    // the spec: the C++ emits exactly these constants.
    #[test]
    fn backup_code_matches_the_cpp_avx() {
        let code = backup_code(true);
        // GP pushes (13 bytes) then stack alignment (12 bytes).
        assert_eq!(
            &code[..25],
            &[
                0x50, 0x51, 0x52, 0x56, 0x57, 0x41, 0x50, 0x41, 0x51, 0x41, 0x52, 0x41, 0x53, //
                0x48, 0x89, 0xe0, 0x48, 0x83, 0xe4, 0xe0, 0x48, 0x83, 0xec, 0x18, 0x50
            ]
        );
        // First vector save at 25: sub rsp,32 ; vmovdqa [rsp], ymm0.
        assert_eq!(&code[25..34], &[0x48, 0x83, 0xec, 0x20, 0xc5, 0xfd, 0x7f, 0x04, 0x24]);
        // ymm8 (the ninth, 9 bytes each) switches the second vex byte to 0x7d.
        let ymm8 = &code[25 + 8 * 9..25 + 8 * 9 + 9];
        assert_eq!(ymm8, &[0x48, 0x83, 0xec, 0x20, 0xc5, 0x7d, 0x7f, 0x04, 0x24]);
        // ymm15 is the last.
        assert_eq!(&code[code.len() - 5..], &[0xc5, 0x7d, 0x7f, 0x3c, 0x24]);
    }

    #[test]
    fn backup_code_matches_the_cpp_sse() {
        let code = backup_code(false);
        // xmm0 at 25: sub rsp,16 ; movdqa [rsp], xmm0.
        assert_eq!(&code[25..34], &[0x48, 0x83, 0xec, 0x10, 0x66, 0x0f, 0x7f, 0x04, 0x24]);
        // xmm15 is the last (with the 0x44 REX).
        assert_eq!(&code[code.len() - 6..], &[0x66, 0x44, 0x0f, 0x7f, 0x3c, 0x24]);
    }

    #[test]
    fn restore_code_is_the_reverse_shape() {
        let code = restore_code(true);
        // First restore: vmovdqa ymm15, [rsp] ; add rsp,32.
        assert_eq!(&code[..9], &[0xc5, 0x7d, 0x6f, 0x3c, 0x24, 0x48, 0x83, 0xc4, 0x20]);
        // Ends with pop rsp then the GP pops, last byte pop rax.
        assert_eq!(
            &code[code.len() - 14..],
            &[0x5c, 0x41, 0x5b, 0x41, 0x5a, 0x41, 0x59, 0x41, 0x58, 0x5f, 0x5e, 0x5a, 0x59, 0x58]
        );
    }

    #[test]
    fn call_to_entry_payload_puts_function_id_at_the_fixed_offset() {
        let backup = backup_code(host_has_avx());
        let call = call_to_entry_payload(0x1111_2222_3333_4444, 0x5555_6666_7777_8888);
        // The function-id immediate sits 178 bytes into backup + call.
        let offset_in_call = OFFSET_OF_FUNCTION_ID - backup.len();
        let id = u64::from_le_bytes(call[offset_in_call..offset_in_call + 8].try_into().unwrap());
        assert_eq!(id, FUNCTION_ID_PLACEHOLDER);
        // The two absolute addresses follow, in order.
        assert!(call.windows(8).any(|w| w == 0x5555_6666_7777_8888u64.to_le_bytes()));
        assert!(call.windows(8).any(|w| w == 0x1111_2222_3333_4444u64.to_le_bytes()));
        assert_eq!(&call[call.len() - 2..], &[0xff, 0xd0]); // call rax
    }

    #[test]
    fn jump_back_is_e9_plus_offset() {
        assert_eq!(jump_back_code(0x01020304), vec![0xe9, 0x04, 0x03, 0x02, 0x01]);
        assert_eq!(jump_back_code(-2), vec![0xe9, 0xfe, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn exit_trampoline_starts_with_space_and_ends_with_ret() {
        let code = call_to_exit_payload_and_jump_to_return_address(0xAABB_CCDD_EEFF_0011, true);
        assert_eq!(&code[..4], &[0x48, 0x83, 0xec, 0x08]); // sub rsp, 8
        assert_eq!(*code.last().unwrap(), 0xc3); // ret
        assert!(code.windows(8).any(|w| w == 0xAABB_CCDD_EEFF_0011u64.to_le_bytes()));
    }
}
