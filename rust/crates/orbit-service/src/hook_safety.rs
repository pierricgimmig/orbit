// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Is a function safe to hook?
//!
//! Dynamic instrumentation crashes the target when a probe lands somewhere
//! that is not a clean function entry. The two engines fail differently:
//!
//! - **Kernel uprobes** put an `int3` at a file offset and single-step the
//!   displaced bytes out of line. If the offset is not the start of an
//!   instruction -- a symbol that points at padding, at data, or into the
//!   middle of an instruction -- the kernel relocates garbage and the target
//!   dies with SIGILL/SIGSEGV.
//! - **Frida** overwrites the first bytes with a jump to a trampoline and
//!   relocates what it displaced. A relative branch inside those bytes can be
//!   relocated to point at the wrong place.
//!
//! So the check is not "does it set up a frame pointer" -- most optimized
//! code (`-fomit-frame-pointer`), every CET build (`endbr64` first), leaf
//! functions and non-x86 code have no textbook prologue, yet hook fine. The
//! check is "does the entry decode as a real instruction, and is it free of
//! the known relocation hazards". This decodes the entry with the same
//! `iced_x86` the disassembly view uses and reports a verdict a person can
//! act on.
//!
//! x86 only: `iced_x86` decodes x86. On other architectures every function is
//! reported [`SafetyLevel::Unknown`] ("not analysed") rather than guessed at.

use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic};

/// `e_machine` for x86-64 and 32-bit x86 -- the ELF machines this analyser
/// can decode.
pub const EM_386: u16 = 3;
pub const EM_X86_64: u16 = 62;

/// A near `jmp rel32` is five bytes; an inline trampoline (Frida) overwrites
/// at least this much of the entry, so a control-flow instruction within the
/// first five bytes is a relocation hazard, and a function shorter than this
/// is too small to hook either way.
const TRAMPOLINE_BYTES: u64 = 5;

/// How safe a function is to hook, worst case first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafetyLevel {
    /// A clean entry: decodes to an ordinary instruction with no hazard.
    Safe,
    /// Hookable, but with a caveat worth showing (tiny function, a branch in
    /// the trampoline window).
    Risky,
    /// The entry is not a function prologue at all -- hooking it is likely to
    /// crash the target.
    Unsafe,
    /// Not analysed (a non-x86 module): no opinion.
    Unknown,
}

impl SafetyLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            SafetyLevel::Safe => "safe",
            SafetyLevel::Risky => "risky",
            SafetyLevel::Unsafe => "unsafe",
            SafetyLevel::Unknown => "unknown",
        }
    }
}

/// The verdict for one function, with the evidence a person needs to judge it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookSafety {
    pub level: SafetyLevel,
    /// One line saying why, for the tooltip and the crash report.
    pub reason: String,
    /// The first instruction, formatted (empty when it did not decode or was
    /// not analysed).
    pub entry: String,
    /// The first bytes of the function as hex, for the report.
    pub entry_hex: String,
}

impl HookSafety {
    fn new(level: SafetyLevel, reason: impl Into<String>, entry: String, entry_hex: String) -> Self {
        HookSafety { level, reason: reason.into(), entry, entry_hex }
    }

    /// A module this analyser does not decode.
    pub fn not_analysed() -> Self {
        HookSafety::new(SafetyLevel::Unknown, "not analysed (non-x86 module)", String::new(), String::new())
    }

    pub fn is_safe(&self) -> bool {
        matches!(self.level, SafetyLevel::Safe | SafetyLevel::Unknown)
    }
}

/// Whether this analyser can decode the given ELF machine.
pub fn is_x86(machine: u16) -> bool {
    machine == EM_X86_64 || machine == EM_386
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

/// Assess the entry of a function. `code` is the function's bytes from the
/// entry on (at least the first instruction; more lets the branch scan work),
/// `size` its symbol size (0 when unknown), `is_64_bit` the module's class.
///
/// Only call this for an x86 module (see [`is_x86`]); other machines get
/// [`HookSafety::not_analysed`].
pub fn assess(code: &[u8], size: u64, is_64_bit: bool) -> HookSafety {
    if code.is_empty() {
        return HookSafety::new(SafetyLevel::Unsafe, "no bytes at the function entry", String::new(), String::new());
    }
    let entry_hex = hex(&code[..code.len().min(16)]);
    let bitness = if is_64_bit { 64 } else { 32 };

    let mut decoder = Decoder::new(bitness, code, DecoderOptions::NONE);
    let mut inst = Instruction::default();
    decoder.decode_out(&mut inst);
    if inst.is_invalid() {
        return HookSafety::new(
            SafetyLevel::Unsafe,
            "the entry does not decode as an instruction (the symbol points at data or into the middle of one)",
            String::new(),
            entry_hex,
        );
    }
    let entry = format!("{inst}");

    // A terminator or a trap as the very first instruction means the symbol is
    // not at a function entry: it points at inter-function padding (`int3`,
    // `nop`), at the tail of another function (`ret`), or at data that happens
    // to decode. Hooking it puts the probe where nothing calls, or mid-body.
    match inst.mnemonic() {
        Mnemonic::Ret | Mnemonic::Retf => {
            return HookSafety::new(SafetyLevel::Unsafe, format!("the entry is `{entry}` -- a return, not a prologue (the symbol likely points past the function)"), entry, entry_hex);
        }
        Mnemonic::Int3 | Mnemonic::Hlt | Mnemonic::Ud2 => {
            return HookSafety::new(SafetyLevel::Unsafe, format!("the entry is `{entry}` -- padding or a trap, not code the process runs"), entry, entry_hex);
        }
        _ => {}
    }

    if size > 0 && size < TRAMPOLINE_BYTES {
        return HookSafety::new(SafetyLevel::Risky, format!("the function is only {size} bytes -- too small to place a return probe or an inline trampoline safely"), entry, entry_hex);
    }

    // A relative branch, call or return inside the bytes an inline trampoline
    // would overwrite is the classic relocation hazard: its target can be
    // moved to point at the wrong place. uprobes are unaffected (they relocate
    // one instruction, out of line), but the default engine is Frida.
    let mut covered = 0u64;
    let mut scan = Decoder::new(bitness, code, DecoderOptions::NONE);
    while scan.can_decode() && covered < TRAMPOLINE_BYTES {
        let ins = scan.decode();
        if ins.is_invalid() {
            break;
        }
        let hazard = matches!(
            ins.flow_control(),
            FlowControl::UnconditionalBranch
                | FlowControl::ConditionalBranch
                | FlowControl::Call
                | FlowControl::IndirectBranch
                | FlowControl::IndirectCall
                | FlowControl::Return
        );
        if hazard {
            return HookSafety::new(
                SafetyLevel::Risky,
                format!("`{ins}` is within the first {TRAMPOLINE_BYTES} bytes -- an inline hook (Frida) may relocate its target wrongly; kernel uprobes are unaffected"),
                entry,
                entry_hex,
            );
        }
        covered += ins.len() as u64;
    }

    HookSafety::new(SafetyLevel::Safe, format!("clean entry (`{entry}`)"), entry, entry_hex)
}

/// Assess from an ELF module's bytes: slices the entry window at `file_offset`
/// and dispatches on the machine. `size` is the symbol size (0 unknown).
pub fn assess_in_module(bytes: &[u8], machine: u16, is_64_bit: bool, file_offset: u64, size: u64) -> HookSafety {
    if !is_x86(machine) {
        return HookSafety::not_analysed();
    }
    let start = file_offset as usize;
    if start >= bytes.len() {
        return HookSafety::new(SafetyLevel::Unsafe, "the entry offset is past the end of the module file", String::new(), String::new());
    }
    // Enough to decode the entry and scan the trampoline window; the symbol
    // size caps it when it is smaller (so the branch scan does not read into
    // the next function).
    let mut end = (start + 16).min(bytes.len());
    if size > 0 {
        end = end.min(start + size as usize);
    }
    assess(&bytes[start..end], size, is_64_bit)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A conventional frame-pointer prologue: push rbp; mov rbp, rsp.
    #[test]
    fn a_frame_pointer_prologue_is_safe() {
        let s = assess(&[0x55, 0x48, 0x89, 0xe5, 0x90, 0x90], 32, true);
        assert_eq!(s.level, SafetyLevel::Safe, "{s:?}");
        assert!(s.entry.contains("push"), "{}", s.entry);
    }

    // The common modern entry: endbr64; push rbp. No frame-pointer rule would
    // pass this, but it is perfectly safe.
    #[test]
    fn an_endbr64_entry_is_safe() {
        let s = assess(&[0xf3, 0x0f, 0x1e, 0xfa, 0x55, 0x48, 0x89, 0xe5], 64, true);
        assert_eq!(s.level, SafetyLevel::Safe, "{s:?}");
        assert!(s.entry.contains("endbr64"), "{}", s.entry);
    }

    // An -fomit-frame-pointer entry (sub rsp, 0x18) is safe too.
    #[test]
    fn an_omit_frame_pointer_entry_is_safe() {
        let s = assess(&[0x48, 0x83, 0xec, 0x18, 0x90, 0x90], 64, true);
        assert_eq!(s.level, SafetyLevel::Safe, "{s:?}");
    }

    // A symbol pointing at a `ret` (past the real function) is unsafe.
    #[test]
    fn a_ret_entry_is_unsafe() {
        let s = assess(&[0xc3], 1, true);
        assert_eq!(s.level, SafetyLevel::Unsafe, "{s:?}");
        assert!(s.reason.contains("return"), "{}", s.reason);
    }

    // Padding (int3) is unsafe.
    #[test]
    fn int3_padding_is_unsafe() {
        let s = assess(&[0xcc, 0xcc, 0xcc, 0xcc, 0xcc], 5, true);
        assert_eq!(s.level, SafetyLevel::Unsafe, "{s:?}");
    }

    // Bytes that do not decode (a lone REX prefix at EOF) are unsafe.
    #[test]
    fn undecodable_bytes_are_unsafe() {
        let s = assess(&[0x48], 1, true);
        assert_eq!(s.level, SafetyLevel::Unsafe, "{s:?}");
        assert!(s.reason.contains("decode"), "{}", s.reason);
    }

    // A tail-call thunk -- jmp rel32 as the first instruction -- is a
    // relocation hazard for an inline hook.
    #[test]
    fn a_leading_relative_jump_is_risky() {
        let s = assess(&[0xe9, 0x00, 0x01, 0x00, 0x00, 0x90], 6, true);
        assert_eq!(s.level, SafetyLevel::Risky, "{s:?}");
        assert!(s.reason.contains("relocate"), "{}", s.reason);
    }

    // A function too small for a trampoline is risky.
    #[test]
    fn a_tiny_function_is_risky() {
        // push rbp (1 byte) then the symbol says size 2.
        let s = assess(&[0x55, 0x5d], 2, true);
        assert_eq!(s.level, SafetyLevel::Risky, "{s:?}");
        assert!(s.reason.contains("too small"), "{}", s.reason);
    }

    #[test]
    fn non_x86_is_not_analysed() {
        // EM_AARCH64 = 183.
        let s = assess_in_module(&[0; 32], 183, true, 0, 8);
        assert_eq!(s.level, SafetyLevel::Unknown);
        assert!(s.is_safe(), "unknown must not be treated as dangerous");
    }

    #[test]
    fn assess_in_module_slices_the_entry() {
        // endbr64; push rbp; mov rbp,rsp at offset 4.
        let mut bytes = vec![0u8; 4];
        bytes.extend_from_slice(&[0xf3, 0x0f, 0x1e, 0xfa, 0x55, 0x48, 0x89, 0xe5]);
        let s = assess_in_module(&bytes, EM_X86_64, true, 4, 8);
        assert_eq!(s.level, SafetyLevel::Safe, "{s:?}");
        // An offset past the file is unsafe, not a panic.
        assert_eq!(assess_in_module(&bytes, EM_X86_64, true, 999, 8).level, SafetyLevel::Unsafe);
    }
}
