// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Twin of `UprobesReturnAddressManager` (Phase 5b): per-thread stacks of
//! return addresses hijacked by dynamic instrumentation, patched back into
//! stack samples and callchains. What counts as a hijacked frame is the
//! caller's business -- the `is_patchable_frame` predicate stands in for the
//! C++'s maps lookup ("[uprobes]") and return-trampoline check.

use crate::TidMap;

#[derive(Clone, Copy, Debug)]
struct OpenFunction {
    stack_pointer: u64,
    return_address: u64,
}

#[derive(Default)]
pub struct ReturnAddressManager {
    tid_to_stack_of_open_functions: TidMap<Vec<OpenFunction>>,
}

impl ReturnAddressManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process_function_entry(&mut self, tid: i32, stack_pointer: u64, return_address: u64) {
        self.tid_to_stack_of_open_functions
            .entry(tid)
            .or_default()
            .push(OpenFunction { stack_pointer, return_address });
    }

    pub fn process_function_exit(&mut self, tid: i32) {
        let Some(stack) = self.tid_to_stack_of_open_functions.get_mut(&tid) else {
            return;
        };
        assert!(!stack.is_empty());
        stack.pop();
        if stack.is_empty() {
            self.tid_to_stack_of_open_functions.remove(&tid);
        }
    }

    /// Applies saved return addresses to a copied stack, innermost-last, so
    /// that with two hijacks at the same stack pointer (tail calls) the
    /// original return address wins -- the same reverse walk as the C++.
    pub fn patch_sample(&self, tid: i32, stack_pointer: u64, stack_data: &mut [u8]) {
        let Some(stack) = self.tid_to_stack_of_open_functions.get(&tid) else {
            return;
        };
        assert!(!stack.is_empty());
        for open_function in stack.iter().rev() {
            if open_function.stack_pointer < stack_pointer {
                continue;
            }
            let offset = (open_function.stack_pointer - stack_pointer) as usize;
            if offset >= stack_data.len() {
                continue;
            }
            let end = offset + 8;
            if end > stack_data.len() {
                continue;
            }
            stack_data[offset..end].copy_from_slice(&open_function.return_address.to_le_bytes());
        }
    }

    /// Replaces hijacked frames in a callchain with the saved return
    /// addresses. Returns false when the sample must be discarded because
    /// the bookkeeping cannot explain the callchain (missed entries, lost
    /// events); the discard conditions mirror the C++ exactly.
    pub fn patch_callchain(
        &self,
        tid: i32,
        callchain: &mut [u64],
        mut is_patchable_frame: impl FnMut(u64) -> bool,
    ) -> bool {
        assert!(!callchain.is_empty());
        let frames_to_patch: Vec<usize> = callchain
            .iter()
            .enumerate()
            .filter(|(_, &ip)| is_patchable_frame(ip))
            .map(|(i, _)| i)
            .collect();

        let Some(stack) = self.tid_to_stack_of_open_functions.get(&tid) else {
            return frames_to_patch.is_empty();
        };
        assert!(!stack.is_empty());

        let mut num_unique_open_functions = 0usize;
        let mut prev_stack_pointer = u64::MAX;
        for open_function in stack {
            if open_function.stack_pointer != prev_stack_pointer {
                num_unique_open_functions += 1;
            }
            prev_stack_pointer = open_function.stack_pointer;
        }

        if num_unique_open_functions < frames_to_patch.len() {
            return false;
        }
        if num_unique_open_functions > frames_to_patch.len() + 1 {
            return false;
        }

        // Outermost first: frames_to_patch back to front, open functions
        // front to back, skipping tail-call duplicates, and skipping the
        // innermost open function when it has not hijacked anything yet
        // (function entry/exit edge).
        let skip_last_open_function = num_unique_open_functions == frames_to_patch.len() + 1;
        let mut frames_iter = frames_to_patch.iter().rev();
        let mut prev_stack_pointer = u64::MAX;
        let mut unique_so_far = 0usize;
        for open_function in stack {
            if skip_last_open_function && unique_so_far + 1 == num_unique_open_functions {
                break;
            }
            if open_function.stack_pointer == prev_stack_pointer {
                continue;
            }
            prev_stack_pointer = open_function.stack_pointer;
            unique_so_far += 1;

            let frame_to_patch = *frames_iter.next().expect("counted above");
            callchain[frame_to_patch] = open_function.return_address;
        }
        assert!(frames_iter.next().is_none());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patchable_above(threshold: u64) -> impl FnMut(u64) -> bool {
        move |ip| ip >= threshold
    }

    #[test]
    fn patch_sample_applies_saved_return_addresses() {
        let mut manager = ReturnAddressManager::new();
        manager.process_function_entry(1, 0x1010, 0xAAAA);
        let mut stack = vec![0u8; 0x20];
        manager.patch_sample(1, 0x1000, &mut stack);
        assert_eq!(u64::from_le_bytes(stack[0x10..0x18].try_into().unwrap()), 0xAAAA);
        // Below the sampled stack pointer: untouched.
        manager.patch_sample(1, 0x1018, &mut stack[..8]);
        assert_eq!(u64::from_le_bytes(stack[0..8].try_into().unwrap()), 0);
    }

    #[test]
    fn tail_call_keeps_the_original_return_address() {
        let mut manager = ReturnAddressManager::new();
        manager.process_function_entry(1, 0x1010, 0xAAAA);
        manager.process_function_entry(1, 0x1010, 0xBBBB);
        let mut stack = vec![0u8; 0x20];
        manager.patch_sample(1, 0x1000, &mut stack);
        // Reverse application: the first (caller's) address ends up in place.
        assert_eq!(u64::from_le_bytes(stack[0x10..0x18].try_into().unwrap()), 0xAAAA);
    }

    #[test]
    fn callchain_patched_outermost_first() {
        let mut manager = ReturnAddressManager::new();
        manager.process_function_entry(1, 0x2000, 0x111);
        manager.process_function_entry(1, 0x1000, 0x222);
        // Two hijacked frames (>= 0xF000 is "in [uprobes]").
        let mut callchain = vec![0x42, 0xF001, 0xF002, 0x43];
        assert!(manager.patch_callchain(1, &mut callchain, patchable_above(0xF000)));
        // Innermost hijack gets the innermost saved address.
        assert_eq!(callchain, vec![0x42, 0x222, 0x111, 0x43]);
    }

    #[test]
    fn extra_open_function_is_skipped_when_not_yet_hijacked() {
        let mut manager = ReturnAddressManager::new();
        manager.process_function_entry(1, 0x2000, 0x111);
        manager.process_function_entry(1, 0x1000, 0x222);
        let mut callchain = vec![0x42, 0xF001, 0x43];
        assert!(manager.patch_callchain(1, &mut callchain, patchable_above(0xF000)));
        assert_eq!(callchain, vec![0x42, 0x111, 0x43]);
    }

    #[test]
    fn discards_when_bookkeeping_cannot_explain_the_callchain() {
        let mut manager = ReturnAddressManager::new();
        // No open functions at all, but a hijacked frame: discard.
        let mut callchain = vec![0xF001];
        assert!(!manager.patch_callchain(7, &mut callchain, patchable_above(0xF000)));
        // Fewer open functions than hijacked frames: discard.
        manager.process_function_entry(7, 0x1000, 0x222);
        let mut callchain = vec![0xF001, 0xF002];
        assert!(!manager.patch_callchain(7, &mut callchain, patchable_above(0xF000)));
        // Too many open functions for the hijacked frames: discard.
        manager.process_function_entry(7, 0x2000, 0x111);
        manager.process_function_entry(7, 0x3000, 0x333);
        let mut callchain = vec![0xF001];
        assert!(!manager.patch_callchain(7, &mut callchain, patchable_above(0xF000)));
        // No hijacked frames and no bookkeeping for the tid: fine.
        let mut callchain = vec![0x42];
        assert!(manager.patch_callchain(8, &mut callchain, patchable_above(0xF000)));
    }

    #[test]
    fn exit_pops_and_erases_empty_stacks() {
        let mut manager = ReturnAddressManager::new();
        manager.process_function_entry(1, 0x1000, 0x111);
        manager.process_function_exit(1);
        manager.process_function_exit(1); // unknown tid: no-op
        let mut stack = vec![0u8; 8];
        manager.patch_sample(1, 0x1000, &mut stack);
        assert_eq!(stack, vec![0u8; 8]);
    }
}
