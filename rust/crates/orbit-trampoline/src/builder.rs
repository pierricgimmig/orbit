// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Assembling a whole trampoline (Phase 6f), twin of `CreateTrampoline` plus
//! its `AppendRelocatedPrologueCode` and `CheckForRelativeJumpIntoFirstFive
//! Bytes` helpers -- everything except writing the bytes into the tracee.
//! The pieces are already ported: the register save/payload-call/restore
//! blocks (codegen, 6e) and the per-instruction relocation (relocate, 6d).
//! This is where they compose into the buffer that gets written at the
//! trampoline address.

use crate::codegen::{
    backup_code, call_to_entry_payload, jump_back_code, restore_code, SIZE_OF_JMP,
};
use crate::placement::{address_difference_as_i32, PlacementError};
use crate::relocate::{relocate_instruction, RelocateError};
use iced_x86::{Decoder, DecoderOptions, Instruction};
use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq)]
pub enum TrampolineError {
    /// The function jumps back into its own first five bytes, so overwriting
    /// them with a jump would corrupt a live jump target.
    HarmfulJumpIntoPrologue,
    /// Not enough of the function could be disassembled to move five bytes.
    CannotDisassemblePrologue,
    /// A prologue instruction could not be relocated.
    Relocate(RelocateError),
    /// The trampoline landed more than +/-2GB from the function.
    OutOfRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltTrampoline {
    /// The trampoline bytes, to be written at `trampoline_address`.
    pub code: Vec<u8>,
    /// The address in the original function just past the relocated
    /// prologue -- where the trampoline jumps back to, and where the
    /// function's own overwrite-with-jump ends.
    pub address_after_prologue: u64,
}

/// Twin of `CheckForRelativeJumpIntoFirstFiveBytes`: does any relative jump
/// in the (partial) function target its own first five bytes?
pub fn has_jump_into_first_five_bytes(function_address: u64, function: &[u8]) -> bool {
    let mut decoder = Decoder::with_ip(64, function, function_address, DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }
        if instruction.is_jmp_short()
            || instruction.is_jmp_near()
            || instruction.is_jcc_short_or_near()
        {
            let target = instruction.near_branch_target();
            if target >= function_address && target < function_address + SIZE_OF_JMP as u64 {
                return true;
            }
        }
    }
    false
}

/// Twin of `CreateTrampoline` (minus the tracee write): build the trampoline
/// bytes for `function` located at `function_address`, to be placed at
/// `trampoline_address`, calling `entry_payload_address` and paired with the
/// return trampoline at `return_trampoline_address`.
pub fn build_trampoline(
    function: &[u8],
    function_address: u64,
    trampoline_address: u64,
    entry_payload_address: u64,
    return_trampoline_address: u64,
    avx: bool,
) -> Result<BuiltTrampoline, TrampolineError> {
    if has_jump_into_first_five_bytes(function_address, function) {
        return Err(TrampolineError::HarmfulJumpIntoPrologue);
    }

    let mut trampoline = Vec::new();
    trampoline.extend_from_slice(&backup_code(avx));
    trampoline.extend_from_slice(&call_to_entry_payload(
        entry_payload_address,
        return_trampoline_address,
    ));
    trampoline.extend_from_slice(&restore_code(avx));

    // Relocate the prologue into the trampoline until at least five bytes of
    // the function have been moved.
    let prologue_start = trampoline_address + trampoline.len() as u64;
    let mut prologue_code: Vec<u8> = Vec::new();
    let mut relocation_map: HashMap<u64, u64> = HashMap::new();
    let mut relocatable_positions: Vec<usize> = Vec::new();

    let mut decoder = Decoder::with_ip(64, function, function_address, DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    while decoder.ip() - function_address < SIZE_OF_JMP as u64 {
        if !decoder.can_decode() {
            break;
        }
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }
        let original_address = instruction.ip();
        let size = instruction.len();
        let start = (original_address - function_address) as usize;
        let raw = &function[start..start + size];
        let relocated_address = prologue_start + prologue_code.len() as u64;
        relocation_map.insert(original_address, relocated_address);
        let relocated = relocate_instruction(raw, original_address, relocated_address)
            .map_err(TrampolineError::Relocate)?;
        if let Some(offset) = relocated.position_of_absolute_address {
            relocatable_positions.push(prologue_code.len() + offset);
        }
        prologue_code.extend_from_slice(&relocated.code);
    }
    let address_after_prologue = decoder.ip();
    if address_after_prologue - function_address < SIZE_OF_JMP as u64 {
        return Err(TrampolineError::CannotDisassemblePrologue);
    }

    // Fix up absolute addresses that pointed inside the moved prologue (e.g.
    // a forward conditional jump into a later prologue instruction).
    for position in relocatable_positions {
        let encoded = u64::from_le_bytes(prologue_code[position..position + 8].try_into().unwrap());
        if let Some(&relocated) = relocation_map.get(&encoded) {
            prologue_code[position..position + 8].copy_from_slice(&relocated.to_le_bytes());
        }
    }
    trampoline.extend_from_slice(&prologue_code);

    // Jump back into the function just past the overwritten prologue.
    let address_after_jmp = trampoline_address + trampoline.len() as u64 + SIZE_OF_JMP as u64;
    let offset = address_difference_as_i32(address_after_prologue, address_after_jmp)
        .map_err(|error| match error {
            PlacementError::DifferenceTooLarge => TrampolineError::OutOfRange,
            _ => TrampolineError::OutOfRange,
        })?;
    trampoline.extend_from_slice(&jump_back_code(offset));

    Ok(BuiltTrampoline { code: trampoline, address_after_prologue })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::host_has_avx;

    #[test]
    fn detects_a_jump_into_the_prologue() {
        // At function start: jmp $-2 (eb fe) targets byte 0, inside the first
        // five. Then padding.
        let function = [0xeb, 0xfe, 0x90, 0x90, 0x90, 0x90];
        assert!(has_jump_into_first_five_bytes(0x400000, &function));
        // A jump far forward is fine.
        let ok = [0x90, 0x90, 0xeb, 0x40, 0x90, 0x90];
        assert!(!has_jump_into_first_five_bytes(0x400000, &ok));
    }

    #[test]
    fn builds_a_trampoline_ending_in_a_jump_back() {
        // A simple prologue: push rbp (55); mov rbp, rsp (48 89 e5);
        // sub rsp, 0x10 (48 83 ec 10); then more.
        let function = [0x55, 0x48, 0x89, 0xe5, 0x48, 0x83, 0xec, 0x10, 0x90, 0x90];
        let function_address = 0x400000u64;
        let trampoline_address = 0x10400000u64; // within 32-bit reach
        let built = build_trampoline(
            &function,
            function_address,
            trampoline_address,
            0x1111_0000,
            0x2222_0000,
            host_has_avx(),
        )
        .unwrap();
        // At least five bytes of prologue were moved.
        assert!(built.address_after_prologue - function_address >= SIZE_OF_JMP as u64);
        // The trampoline ends with a jmp rel32 back.
        let tail = &built.code[built.code.len() - 5..];
        assert_eq!(tail[0], 0xe9);
        // The jump lands at address_after_prologue.
        let offset = i32::from_le_bytes(tail[1..5].try_into().unwrap());
        let address_after_jmp = trampoline_address + built.code.len() as u64;
        assert_eq!(
            (address_after_jmp as i64 + offset as i64) as u64,
            built.address_after_prologue
        );
    }

    #[test]
    fn rejects_a_prologue_that_cannot_be_disassembled() {
        // A truncated function with fewer than five decodable bytes.
        let function = [0x90, 0x90]; // two nops, then nothing
        let built = build_trampoline(&function, 0x400000, 0x10400000, 0, 0, host_has_avx());
        assert_eq!(built, Err(TrampolineError::CannotDisassemblePrologue));
    }
}
