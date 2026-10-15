// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Capture summaries and assistant tools, shared by the live viewer and by
//! anything else that wants to ask a model about an Orbit capture.
//!
//! The in-viewer assistant (`orbit-service`) fills a [`CaptureFacts`], calls
//! [`summarize`], then [`run_turn`]. An MCP server on the HTTP API can do
//! the same without going through the panel:
//!
//! - build hotspots with [`hotspots_from_report_json`] from
//!   `GET /api/sampling/report`
//! - build stacks with [`stacks_from_bottom_up_json`] from
//!   `GET /api/sampling/tree?mode=bottom_up`
//! - build a listing with [`disassembly_from_json`] from
//!   `GET /api/code/disassembly`
//! - fold scope and thread-state events with [`fold_timings`] when it has
//!   them (the sampling routes do not carry those)
//! - call [`summarize`] and, with the `client` feature, [`run_turn`]
//!
//! [`UiAction::SuggestHook`] is only a proposal. Dispatch never arms a hook.
//! The HTTP route `POST /api/assistant/chat` is the same turn the viewer
//! uses, if a caller would rather not link this crate.
//!
//! The `client` feature (on by default) speaks to an OpenAI-compatible
//! endpoint or Anthropic. The wasm viewer depends on this crate with that
//! feature off, so the page shares the action types and never links an HTTP
//! client or an API key.

mod facts;
#[cfg(feature = "client")]
mod mock;
#[cfg(feature = "client")]
mod provider;
mod summary;
mod tools;
#[cfg(feature = "client")]
mod turn;

pub use facts::{
    disassembly_from_json, fold_timings, hotspots_from_report_json, stacks_from_bottom_up_json,
    CaptureFacts, DisassemblySnippet, Hotspot, RangeNs, ScopeStat, SelectionWindow, StackFact,
    ThreadStateStat, TimedEvent,
};
pub use summary::{summarize, Evidence, Summary, TokenBudget, DEFAULT_TOKEN_BUDGET};
pub use tools::{
    dispatch, tool_specs, Dispatch, NoDisassembly, ToolCall, ToolHost, ToolSpec, UiAction,
};

#[cfg(feature = "client")]
pub use mock::MockModel;
#[cfg(feature = "client")]
pub use provider::{
    setup_hint, Completion, CompletionRequest, HttpModel, LanguageModel, ModelMessage,
    ProviderConfig, ProviderKind, ProviderStatus,
};
#[cfg(feature = "client")]
pub use turn::{run_turn, TurnInput, TurnOutput};
