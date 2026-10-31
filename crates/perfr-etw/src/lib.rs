// Copyright (c) 2026 The perfr Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Event Tracing for Windows as a capture source.
//!
//! The Windows kernel publishes its scheduler, its sampling profiler and its
//! image loader through one ETW session, the *NT Kernel Logger*. This crate
//! runs that session and turns its events into what the Linux perf path
//! produces: [`SchedulingSlice`](perfr_tracing_state::context_switches::SchedulingSlice)s
//! through the same `ContextSwitchManager`, thread-state transitions through
//! the same `ThreadStateManager`, and stack samples as lists of user-mode
//! program counters, innermost first.
//!
//! Three decisions, each the opposite of a tempting default:
//!
//! - **Raw layouts, not TDH.** The classic kernel events are MOF structures
//!   whose layouts have not changed since Windows 7 (`CSwitch` is twenty-four
//!   bytes with the thread ids in the first eight). The C++ tracer went
//!   through a schema parser and lost every context switch on a Windows
//!   build whose `CSwitch` version TDH could not find (`52787b085`); a byte
//!   offset cannot fail that way, and it costs nothing per event. [`events`]
//!   is the decoder, and it is pure: every layout has a test on bytes
//!   written by hand from the documentation.
//! - **No bindings crate.** The surface is `StartTraceW`, `ControlTraceW`,
//!   `TraceSetInformation`, `OpenTraceW`, `ProcessTrace`, `CloseTrace` and a
//!   token call, declared in [`ffi`] with their structures sized by
//!   `const` assertions. `windows-sys` would be a hundred thousand lines
//!   for seven functions.
//! - **Samples come from `StackWalk` alone.** The sampled-profile event
//!   carries an instruction pointer the stack event repeats as its first
//!   frame, and on a modern kernel the user half of a stack can arrive in a
//!   second `StackWalk` for the same instant. [`samples`] merges the parts
//!   by `(thread, timestamp)` and emits one sample per instant.
//!
//! The session needs an elevated token (the kernel logger is
//! Administrators-only) and `SeSystemProfilePrivilege` for the sampling
//! interval; [`is_elevated`] says up front, and [`session::KernelSession::start`]
//! reports `ERROR_ACCESS_DENIED` as a sentence rather than a number.
//!
//! Everything but [`session`] and [`consumer`] compiles and is tested on
//! every OS, so the decoders and state feeders run in Linux CI too.

pub mod clock;
pub mod events;
pub mod samples;
pub mod scheduler;

#[cfg(windows)]
pub mod consumer;
#[cfg(windows)]
pub mod ffi;
#[cfg(windows)]
pub mod session;
#[cfg(windows)]
pub mod sysproc;
#[cfg(windows)]
pub mod token;

#[cfg(windows)]
pub use consumer::{Consumer, ConsumerStats};
#[cfg(windows)]
pub use session::{KernelSession, SessionOptions, SessionStats};
#[cfg(windows)]
pub use token::{enable_system_profile_privilege, is_elevated};

pub use clock::QpcClock;
pub use events::{KernelEvent, ThreadState as EtwThreadState};
pub use samples::{Sample, SampleAssembler};
pub use scheduler::{SchedulerFeed, SchedulerStats, StateEvent};
