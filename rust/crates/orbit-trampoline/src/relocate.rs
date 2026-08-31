// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Relocating a single instruction from a function's prologue to a
//! trampoline (Phase 6d), twin of `RelocateInstruction`. Moving code to a new
//! address breaks anything encoded relative to the instruction pointer:
//! RIP-relative memory operands and relative branches. This rewrites those so
//! the relocated instruction does the same thing from its new home; every
//! other instruction is copied verbatim.
//!
//! The C++ decodes with capstone; this decodes with iced-x86 (pure Rust) but
//! emits exactly the same bytes -- the differential compares them byte for
//! byte, so the emission logic here mirrors the C++ hand-written sequences,
//! not iced's own re-encoder.

use crate::placement::{address_difference_as_i32, PlacementError};
use iced_x86::{Decoder, DecoderOptions, Instruction, Register};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelocatedInstruction {
    /// The relocated machine code (possibly several instructions emulating
    /// the original).
    pub code: Vec<u8>,
    /// If the code embeds an 8-byte absolute address that must be fixed up
    /// once all relocations are placed, its offset within `code`.
    pub position_of_absolute_address: Option<usize>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RelocateError {
    /// A RIP-relative operand's target is more than +/-2GB from the
    /// trampoline.
    RipRelativeOutOfRange,
    /// Relocating a `call` is deliberately unsupported (it would turn an
    /// unbounded tree of callees' samples into unwinding errors).
    CallUnsupported,
    /// `loop`/`loope`/`loopne`/`jrcxz` relocation is unimplemented (modern
    /// compilers do not emit them).
    LoopUnsupported,
    /// The bytes did not decode to a valid instruction.
    DecodeFailed,
}

/// The legacy prefixes that can precede an opcode, plus REX. Skipping them
/// finds the primary opcode byte -- what capstone exposes as `opcode[0]`.
fn opcode_offset(bytes: &[u8]) -> usize {
    const LEGACY_PREFIXES: [u8; 11] =
        [0x66, 0x67, 0xf0, 0xf2, 0xf3, 0x2e, 0x36, 0x3e, 0x26, 0x64, 0x65];
    let mut offset = 0;
    while offset < bytes.len() && LEGACY_PREFIXES.contains(&bytes[offset]) {
        offset += 1;
    }
    if offset < bytes.len() && (0x40..=0x4f).contains(&bytes[offset]) {
        offset += 1; // REX
    }
    offset
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

fn jump_via_absolute(absolute_address: u64) -> RelocatedInstruction {
    // jmp [rip + 0]  ff 25 00 00 00 00 ; then the 8-byte absolute target.
    let mut code = vec![0xff, 0x25, 0x00, 0x00, 0x00, 0x00];
    code.extend_from_slice(&absolute_address.to_le_bytes());
    RelocatedInstruction { code, position_of_absolute_address: Some(6) }
}

fn conditional_jump_via_absolute(inverted_opcode: u8, absolute_address: u64) -> RelocatedInstruction {
    // <inverted jcc> +0x0e (skip the 6-byte jmp and 8-byte address), then
    // jmp [rip+0], then the 8-byte absolute target.
    let mut code = vec![inverted_opcode, 0x0e, 0xff, 0x25, 0x00, 0x00, 0x00, 0x00];
    code.extend_from_slice(&absolute_address.to_le_bytes());
    RelocatedInstruction { code, position_of_absolute_address: Some(8) }
}

/// Relocates the single instruction at `raw` (which must contain exactly one
/// decoded instruction's bytes) from `old_address` to `new_address`.
pub fn relocate_instruction(
    raw: &[u8],
    old_address: u64,
    new_address: u64,
) -> Result<RelocatedInstruction, RelocateError> {
    let mut decoder = Decoder::with_ip(64, raw, old_address, DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    decoder.decode_out(&mut instruction);
    if instruction.is_invalid() {
        return Err(RelocateError::DecodeFailed);
    }
    let size = instruction.len();
    let offsets = decoder.get_constant_offsets(&instruction);
    let op_at = opcode_offset(raw);
    let opcode0 = raw[op_at];

    // RIP-relative memory operand (modrm & 0xC7 == 0x05): keep the
    // instruction, adjust its 32-bit displacement to the same absolute
    // target from the new location.
    if instruction.memory_base() == Register::RIP && offsets.has_displacement() {
        let disp_offset = offsets.displacement_offset();
        let old_displacement = read_i32(raw, disp_offset);
        let old_absolute = old_address
            .wrapping_add(size as u64)
            .wrapping_add(old_displacement as i64 as u64);
        let new_displacement = address_difference_as_i32(old_absolute, new_address + size as u64)
            .map_err(|error| match error {
                PlacementError::DifferenceTooLarge => RelocateError::RipRelativeOutOfRange,
                _ => RelocateError::RipRelativeOutOfRange,
            })?;
        let mut code = raw[..size].to_vec();
        code[disp_offset..disp_offset + 4].copy_from_slice(&new_displacement.to_le_bytes());
        return Ok(RelocatedInstruction { code, position_of_absolute_address: None });
    }

    let imm_offset = offsets.immediate_offset();
    match opcode0 {
        // jmp rel8 / rel32: emulate with an absolute jump.
        0xeb | 0xe9 => {
            let immediate = if opcode0 == 0xe9 {
                read_i32(raw, imm_offset) as i64
            } else {
                raw[imm_offset] as i8 as i64
            };
            let absolute = old_address.wrapping_add(size as u64).wrapping_add(immediate as u64);
            Ok(jump_via_absolute(absolute))
        }
        // call rel32: deliberately unsupported.
        0xe8 => Err(RelocateError::CallUnsupported),
        // jcc rel8 (0x70-0x7f): invert the condition, jump over an absolute jump.
        0x70..=0x7f => {
            let immediate = raw[imm_offset] as i8 as i64;
            let absolute = old_address.wrapping_add(size as u64).wrapping_add(immediate as u64);
            // Inverting bit 0 negates the condition (je 0x74 <-> jne 0x75).
            Ok(conditional_jump_via_absolute(0x01 ^ opcode0, absolute))
        }
        // loop / loope / loopne / jrcxz.
        0xe0..=0xe3 => Err(RelocateError::LoopUnsupported),
        // Two-byte opcodes.
        0x0f => {
            let opcode1 = raw[op_at + 1];
            if opcode1 & 0xf0 == 0x80 {
                // jcc rel32: invert into an 8-bit jcc over an absolute jump.
                let immediate = read_i32(raw, imm_offset) as i64;
                let absolute = old_address.wrapping_add(size as u64).wrapping_add(immediate as u64);
                let inverted = 0x70 | (0x01 ^ (opcode1 & 0x0f));
                Ok(conditional_jump_via_absolute(inverted, absolute))
            } else {
                Ok(copy_verbatim(&raw[..size]))
            }
        }
        _ => Ok(copy_verbatim(&raw[..size])),
    }
}

fn copy_verbatim(bytes: &[u8]) -> RelocatedInstruction {
    RelocatedInstruction { code: bytes.to_vec(), position_of_absolute_address: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_instruction_is_copied() {
        // xor eax, eax = 31 c0
        let out = relocate_instruction(&[0x31, 0xc0], 0x1000, 0x900000).unwrap();
        assert_eq!(out.code, vec![0x31, 0xc0]);
        assert_eq!(out.position_of_absolute_address, None);
    }

    #[test]
    fn rip_relative_displacement_is_adjusted() {
        // add qword [rip + 0x123456], 1 = 48 83 05 56 34 12 00 01 (8 bytes)
        let raw = [0x48, 0x83, 0x05, 0x56, 0x34, 0x12, 0x00, 0x01];
        let old = 0x1000u64;
        let new = 0x900000u64;
        let out = relocate_instruction(&raw, old, new).unwrap();
        // Same instruction, displacement now points at the same absolute
        // target: old + size + old_disp == new + size + new_disp.
        assert_eq!(out.code.len(), 8);
        assert_eq!(&out.code[..3], &raw[..3]);
        let new_disp = read_i32(&out.code, 3) as i64;
        let old_disp = 0x123456i64;
        // Both encode the same absolute target (wrapping, since the new
        // displacement is negative here).
        let old_target = old.wrapping_add(8).wrapping_add(old_disp as u64);
        let new_target = new.wrapping_add(8).wrapping_add(new_disp as u64);
        assert_eq!(old_target, new_target);
    }

    #[test]
    fn jmp_rel32_becomes_absolute() {
        // jmp +0x01020304 = e9 04 03 02 01
        let raw = [0xe9, 0x04, 0x03, 0x02, 0x01];
        let out = relocate_instruction(&raw, 0x1000, 0x900000).unwrap();
        assert_eq!(&out.code[..6], &[0xff, 0x25, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(out.position_of_absolute_address, Some(6));
        let absolute = u64::from_le_bytes(out.code[6..14].try_into().unwrap());
        assert_eq!(absolute, 0x1000 + 5 + 0x01020304);
    }

    #[test]
    fn short_conditional_jump_inverts_and_absolutizes() {
        // jne -10 = 75 f6
        let raw = [0x75, 0xf6];
        let out = relocate_instruction(&raw, 0x1000, 0x900000).unwrap();
        assert_eq!(out.code[0], 0x74); // inverted to je
        assert_eq!(out.code[1], 0x0e);
        assert_eq!(&out.code[2..8], &[0xff, 0x25, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(out.position_of_absolute_address, Some(8));
        let absolute = u64::from_le_bytes(out.code[8..16].try_into().unwrap());
        assert_eq!(absolute, (0x1000u64).wrapping_add(2).wrapping_add((-10i64) as u64));
    }

    #[test]
    fn near_conditional_jump_inverts_to_short() {
        // jne rel32 -10 = 0f 85 f6 ff ff ff
        let raw = [0x0f, 0x85, 0xf6, 0xff, 0xff, 0xff];
        let out = relocate_instruction(&raw, 0x1000, 0x900000).unwrap();
        assert_eq!(out.code[0], 0x74); // je rel8
        assert_eq!(out.code[1], 0x0e);
        assert_eq!(out.position_of_absolute_address, Some(8));
    }

    #[test]
    fn call_and_loop_are_rejected() {
        assert_eq!(
            relocate_instruction(&[0xe8, 0x04, 0x03, 0x02, 0x01], 0x1000, 0x900000),
            Err(RelocateError::CallUnsupported)
        );
        // loop -2 = e2 fe
        assert_eq!(
            relocate_instruction(&[0xe2, 0xfe], 0x1000, 0x900000),
            Err(RelocateError::LoopUnsupported)
        );
    }
}
