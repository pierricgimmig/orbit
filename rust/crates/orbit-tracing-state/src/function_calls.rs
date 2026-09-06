// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `UprobesFunctionCallManager`: a stack per thread of entered instrumented
//! functions, matched with their exits into function calls.

use std::collections::HashMap;

use crate::FxBuildHasher;

/// The six argument registers captured at function entry, in the cross-
//  platform accessor order the C++ uses (GetArg0..GetArg5).
pub type ArgumentRegisters = [u64; 6];

/// One matched call, mirroring `FunctionCall` minus pid/tid, which the caller
/// already has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FunctionCall {
    pub function_id: u64,
    pub duration_ns: u64,
    pub end_timestamp_ns: u64,
    pub depth: i32,
    pub return_value: Option<u64>,
    pub registers: Option<ArgumentRegisters>,
}

#[derive(Clone, Copy, Debug)]
struct OpenFunction {
    function_id: u64,
    begin_timestamp: u64,
    registers: Option<ArgumentRegisters>,
}

#[derive(Debug, Default)]
pub struct FunctionCallManager {
    stacks_by_tid: HashMap<i32, Vec<OpenFunction>, FxBuildHasher>,
}

impl FunctionCallManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process_function_entry(
        &mut self,
        tid: i32,
        function_id: u64,
        begin_timestamp: u64,
        registers: Option<ArgumentRegisters>,
    ) {
        self.stacks_by_tid.entry(tid).or_default().push(OpenFunction {
            function_id,
            begin_timestamp,
            registers,
        });
    }

    pub fn process_function_exit(
        &mut self,
        tid: i32,
        end_timestamp: u64,
        return_value: Option<u64>,
    ) -> Option<FunctionCall> {
        // An exit with no recorded entry -- a capture that started mid-call.
        let stack = self.stacks_by_tid.get_mut(&tid)?;
        // Empty stacks are erased eagerly, so a present stack is never empty.
        let open = stack.pop().expect("stacks are erased when empty");

        let call = FunctionCall {
            function_id: open.function_id,
            duration_ns: end_timestamp.wrapping_sub(open.begin_timestamp),
            end_timestamp_ns: end_timestamp,
            depth: stack.len() as i32,
            return_value,
            registers: open.registers,
        };
        if stack.is_empty() {
            self.stacks_by_tid.remove(&tid);
        }
        Some(call)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_nested_calls_with_depths() {
        let mut manager = FunctionCallManager::new();
        manager.process_function_entry(7, 100, 10, None);
        manager.process_function_entry(7, 200, 20, None);

        let inner = manager.process_function_exit(7, 30, Some(42)).unwrap();
        assert_eq!(inner.function_id, 200);
        assert_eq!(inner.duration_ns, 10);
        assert_eq!(inner.depth, 1);
        assert_eq!(inner.return_value, Some(42));

        let outer = manager.process_function_exit(7, 40, None).unwrap();
        assert_eq!(outer.function_id, 100);
        assert_eq!(outer.depth, 0);
        assert_eq!(outer.return_value, None);
    }

    #[test]
    fn exit_without_entry_yields_nothing() {
        let mut manager = FunctionCallManager::new();
        assert_eq!(manager.process_function_exit(7, 30, None), None);
        // And after a full cycle, the stack is erased.
        manager.process_function_entry(7, 100, 10, None);
        manager.process_function_exit(7, 20, None).unwrap();
        assert_eq!(manager.process_function_exit(7, 30, None), None);
    }

    #[test]
    fn threads_have_independent_stacks() {
        let mut manager = FunctionCallManager::new();
        manager.process_function_entry(1, 100, 10, None);
        manager.process_function_entry(2, 200, 15, None);
        assert_eq!(manager.process_function_exit(1, 20, None).unwrap().function_id, 100);
        assert_eq!(manager.process_function_exit(2, 25, None).unwrap().function_id, 200);
    }

    #[test]
    fn registers_travel_with_the_entry() {
        let mut manager = FunctionCallManager::new();
        let registers = [1u64, 2, 3, 4, 5, 6];
        manager.process_function_entry(7, 100, 10, Some(registers));
        let call = manager.process_function_exit(7, 20, None).unwrap();
        assert_eq!(call.registers, Some(registers));
    }
}
