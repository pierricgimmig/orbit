// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Twin of `LeafFunctionCallManager::PatchCallerOfLeafFunctionImpl` (Phase
//! 5c): the decision tree that fixes a frame-pointer callchain whose leaf
//! function has no frame pointer. The unwinding engine stays behind three
//! callbacks -- the CFI frame-pointer query, one unwinding step, and the
//! executable-map check -- so the same tree runs against libunwindstack
//! through the facade today and against orbit-unwind in the Rust collector.

/// Mirrors the `orbit_grpc_protos::Callstack::CallstackType` values the
/// C++ can return here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeafPatchResult {
    Complete,
    FramePointerUnwindingError,
    StackTopDwarfUnwindingError,
    StackTopForDwarfUnwindingTooSmall,
}

/// What one unwinding step reports back, in the registers the tree needs.
#[derive(Clone, Copy, Debug)]
pub struct LeafStepOutcome {
    /// The walk reached the root in one step: there is no caller to patch.
    pub success: bool,
    pub frames_empty: bool,
    pub new_pc: u64,
    pub new_sp: u64,
    pub new_frame_pointer: u64,
}

/// The registers of the sampled leaf frame.
#[derive(Clone, Copy, Debug)]
pub struct LeafRegs {
    pub ip: u64,
    pub sp: u64,
    pub frame_pointer: u64,
}

/// Runs the decision tree. Returns the result plus, when the callchain
/// needs patching, the new ips (caller inserted after the first two
/// entries). Never mutates anything -- applying the ips is the caller's
/// move, which also lets both-mode compare without double mutation.
pub fn patch_caller_of_leaf_function(
    regs: LeafRegs,
    stack_dump_size: u16,
    callchain: &[u64],
    has_frame_pointer_set: impl FnOnce(u64) -> Option<bool>,
    unwind_one_step: impl FnOnce(u64) -> LeafStepOutcome,
    is_executable: impl FnOnce(u64) -> bool,
) -> (LeafPatchResult, Option<Vec<u64>>) {
    if regs.frame_pointer < regs.sp {
        return (LeafPatchResult::FramePointerUnwindingError, None);
    }

    match has_frame_pointer_set(regs.ip) {
        None => return (LeafPatchResult::StackTopDwarfUnwindingError, None),
        Some(true) => return (LeafPatchResult::Complete, None),
        Some(false) => {}
    }

    // One unwinding step over the top of the stack: everything from $rsp up
    // to $rbp + 16 (previous frame pointer plus return address).
    let stack_size = regs.frame_pointer - regs.sp + 16;
    let slice_size = stack_size.min(u64::from(stack_dump_size));
    let step = unwind_one_step(slice_size);

    if step.success {
        return (LeafPatchResult::Complete, None);
    }

    let too_small_or_error = || {
        if stack_size > u64::from(stack_dump_size) {
            LeafPatchResult::StackTopForDwarfUnwindingTooSmall
        } else {
            LeafPatchResult::StackTopDwarfUnwindingError
        }
    };

    if (step.new_pc == regs.ip && step.new_sp == regs.sp) || step.frames_empty {
        return (too_small_or_error(), None);
    }

    if step.new_frame_pointer != regs.frame_pointer {
        // The frame pointer moved: either it was a real frame pointer and
        // the callchain was already correct, or (moving down the stack) it
        // was a general-purpose register and the chain is broken.
        if step.new_frame_pointer < regs.frame_pointer {
            return (LeafPatchResult::FramePointerUnwindingError, None);
        }
        return (LeafPatchResult::Complete, None);
    }

    // The frame pointer did not change: leaf function. The updated pc is
    // the missing caller.
    assert!(callchain.len() >= 2);
    let caller = step.new_pc;
    if !is_executable(caller) {
        return (too_small_or_error(), None);
    }

    let mut patched = Vec::with_capacity(callchain.len() + 1);
    patched.extend_from_slice(&callchain[..2]);
    patched.push(caller);
    patched.extend_from_slice(&callchain[2..]);
    (LeafPatchResult::Complete, Some(patched))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGS: LeafRegs = LeafRegs { ip: 0x100, sp: 0x2000, frame_pointer: 0x2040 };

    fn step(new_pc: u64, new_sp: u64, new_fp: u64) -> LeafStepOutcome {
        LeafStepOutcome { success: false, frames_empty: false, new_pc, new_sp, new_frame_pointer: new_fp }
    }

    #[test]
    fn frame_pointer_below_stack_pointer_is_an_error() {
        let regs = LeafRegs { ip: 0x100, sp: 0x2000, frame_pointer: 0x1000 };
        let (result, ips) = patch_caller_of_leaf_function(
            regs, 512, &[0, 1],
            |_| unreachable!(), |_| unreachable!(), |_| unreachable!(),
        );
        assert_eq!(result, LeafPatchResult::FramePointerUnwindingError);
        assert!(ips.is_none());
    }

    #[test]
    fn frame_pointer_already_set_means_complete() {
        let (result, ips) = patch_caller_of_leaf_function(
            REGS, 512, &[0, 1], |_| Some(true), |_| unreachable!(), |_| unreachable!(),
        );
        assert_eq!(result, LeafPatchResult::Complete);
        assert!(ips.is_none());
    }

    #[test]
    fn missing_debug_info_is_a_dwarf_error() {
        let (result, _) = patch_caller_of_leaf_function(
            REGS, 512, &[0, 1], |_| None, |_| unreachable!(), |_| unreachable!(),
        );
        assert_eq!(result, LeafPatchResult::StackTopDwarfUnwindingError);
    }

    #[test]
    fn leaf_function_gets_its_caller_patched_in() {
        let (result, ips) = patch_caller_of_leaf_function(
            REGS, 512, &[0xAAAA, 0x100, 0x300, 0x400],
            |_| Some(false),
            |slice| { assert_eq!(slice, 0x2040 - 0x2000 + 16); step(0x250, 0x2010, REGS.frame_pointer) },
            |caller| { assert_eq!(caller, 0x250); true },
        );
        assert_eq!(result, LeafPatchResult::Complete);
        assert_eq!(ips.unwrap(), vec![0xAAAA, 0x100, 0x250, 0x300, 0x400]);
    }

    #[test]
    fn unchanged_regs_report_too_small_when_stack_was_truncated() {
        let big_fp = LeafRegs { ip: 0x100, sp: 0x2000, frame_pointer: 0x2000 + 0x4000 };
        let (result, _) = patch_caller_of_leaf_function(
            big_fp, 512, &[0, 1], |_| Some(false),
            |_| step(big_fp.ip, big_fp.sp, big_fp.frame_pointer), |_| unreachable!(),
        );
        assert_eq!(result, LeafPatchResult::StackTopForDwarfUnwindingTooSmall);
        let (result, _) = patch_caller_of_leaf_function(
            REGS, 512, &[0, 1], |_| Some(false),
            |_| step(REGS.ip, REGS.sp, REGS.frame_pointer), |_| unreachable!(),
        );
        assert_eq!(result, LeafPatchResult::StackTopDwarfUnwindingError);
    }

    #[test]
    fn frame_pointer_moves_decide_between_complete_and_error() {
        let (result, _) = patch_caller_of_leaf_function(
            REGS, 512, &[0, 1], |_| Some(false),
            |_| step(0x250, 0x2010, REGS.frame_pointer + 0x40), |_| unreachable!(),
        );
        assert_eq!(result, LeafPatchResult::Complete);
        let (result, _) = patch_caller_of_leaf_function(
            REGS, 512, &[0, 1], |_| Some(false),
            |_| step(0x250, 0x2010, REGS.frame_pointer - 0x8), |_| unreachable!(),
        );
        assert_eq!(result, LeafPatchResult::FramePointerUnwindingError);
    }

    #[test]
    fn non_executable_caller_is_an_error() {
        let (result, ips) = patch_caller_of_leaf_function(
            REGS, 512, &[0, 1], |_| Some(false),
            |_| step(0x250, 0x2010, REGS.frame_pointer), |_| false,
        );
        assert_eq!(result, LeafPatchResult::StackTopDwarfUnwindingError);
        assert!(ips.is_none());
    }

    #[test]
    fn root_reached_in_one_step_is_complete() {
        let mut outcome = step(0, 0, 0);
        outcome.success = true;
        let (result, _) = patch_caller_of_leaf_function(
            REGS, 512, &[0, 1], |_| Some(false), |_| outcome, |_| unreachable!(),
        );
        assert_eq!(result, LeafPatchResult::Complete);
    }
}
